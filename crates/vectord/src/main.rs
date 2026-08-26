use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::Parser;
use serde_json::{Value, json};
use tokio::sync::Mutex;
use uuid::Uuid;
use vector_core::{
    ComputerUseVerificationRequest, Diagnostic, HarnessId, PROTOCOL_VERSION, RequestEnvelope,
    ResponseEnvelope, RunStartRequest, application_paths,
};
use vector_db::VectorDatabase;
use vector_harness::{adapter, harness_inventory, install_managed_harness};
use vector_providers::ProviderDiscovery;
use vector_runtime::workbench::Workbench;
use vector_runtime::{preflight, prepare_run, smoke_test, verify_computer_use};

mod session;
mod workbench_dispatch;
use session::SessionManager;

#[derive(Parser)]
#[command(
    name = "vectord",
    version,
    about = "Vector's local operational authority"
)]
struct Args {
    #[arg(long)]
    socket: Option<PathBuf>,
    #[arg(long)]
    data_dir: Option<PathBuf>,
}

#[derive(Clone)]
struct State {
    database: VectorDatabase,
    auth_token: String,
    started_at: String,
    sessions: SessionManager,
    workbench: Workbench,
    idempotency: Arc<Mutex<HashMap<String, ResponseEnvelope>>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let paths = application_paths().context("could not determine Vector application directory")?;
    let data_dir = args.data_dir.unwrap_or(paths.data_dir);
    tokio::fs::create_dir_all(&data_dir).await?;
    let auth_path = data_dir.join("state/daemon.token");
    let auth_token = load_or_create_token(&auth_path).await?;
    let database = VectorDatabase::open(&data_dir.join("state/vector.db")).await?;
    let workbench = Workbench::open(&data_dir).await?;
    let state = State {
        database,
        auth_token,
        started_at: chrono_like_now(),
        sessions: SessionManager::default(),
        workbench,
        idempotency: Arc::new(Mutex::new(HashMap::new())),
    };

    #[cfg(unix)]
    {
        let socket = args
            .socket
            .unwrap_or_else(|| data_dir.join("state/vectord.sock"));
        serve_unix(&socket, state).await
    }
    #[cfg(not(unix))]
    {
        let _ = state;
        bail!(
            "VCTR_RUNTIME_UNAVAILABLE: Windows named-pipe transport is not yet enabled in this foundation build"
        )
    }
}

async fn load_or_create_token(path: &Path) -> Result<String> {
    if let Ok(token) = tokio::fs::read_to_string(path).await {
        return Ok(token.trim().to_string());
    }
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let token = format!("{}{}", Uuid::now_v7(), Uuid::new_v4());
    tokio::fs::write(path, &token).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).await?;
    }
    Ok(token)
}

#[cfg(unix)]
async fn serve_unix(socket: &Path, state: State) -> Result<()> {
    use std::os::unix::fs::FileTypeExt;
    use tokio::net::UnixListener;

    if let Ok(metadata) = std::fs::symlink_metadata(socket) {
        if !metadata.file_type().is_socket() {
            bail!("refusing to replace non-socket path {}", socket.display());
        }
        std::fs::remove_file(socket)?;
    }
    if let Some(parent) = socket.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let listener = UnixListener::bind(socket)?;
    use std::os::unix::fs::PermissionsExt;
    tokio::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600)).await?;
    println!("vectord listening on {}", socket.display());
    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(error) = serve_connection(stream, state).await {
                eprintln!("vectord connection error: {error:#}");
            }
        });
    }
}

#[cfg(unix)]
async fn serve_connection(stream: tokio::net::UnixStream, state: State) -> Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    let challenge = Uuid::new_v4().to_string();
    while let Some(line) = lines.next_line().await? {
        let request: RequestEnvelope = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                let response = ResponseEnvelope {
                    protocol_version: PROTOCOL_VERSION.into(),
                    request_id: Uuid::nil(),
                    ok: false,
                    result: None,
                    diagnostic: Some(Diagnostic::error(
                        "VCTR_CONFIG_INVALID",
                        "Malformed daemon request",
                        error.to_string(),
                    )),
                };
                write
                    .write_all(format!("{}\n", serde_json::to_string(&response)?).as_bytes())
                    .await?;
                continue;
            }
        };
        let response = if request.method == "auth.challenge" {
            success(request.request_id, json!({"challenge": challenge}))
        } else {
            let expected = blake3::hash(format!("{}:{challenge}", state.auth_token).as_bytes())
                .to_hex()
                .to_string();
            if request.auth.as_deref() != Some(expected.as_str()) {
                failure(
                    request.request_id,
                    "VCTR_POLICY_DENIED",
                    "Daemon authentication failed",
                    "Read the per-user token and complete the local challenge response.",
                )
            } else {
                dispatch_idempotent(request, &state).await
            }
        };
        write
            .write_all(format!("{}\n", serde_json::to_string(&response)?).as_bytes())
            .await?;
    }
    Ok(())
}

fn is_mutating(method: &str) -> bool {
    matches!(
        method,
        "harness.install"
            | "onboarding.smoke"
            | "onboarding.computer"
            | "runs.prepare"
            | "runs.start"
            | "runs.prompt"
            | "runs.steer"
            | "runs.abort"
            | "runs.stop"
            | "approvals.respond"
    ) || workbench_dispatch::is_mutating(method)
}

async fn dispatch_idempotent(request: RequestEnvelope, state: &State) -> ResponseEnvelope {
    if !is_mutating(&request.method) {
        return dispatch(request, state).await;
    }
    if request.idempotency_key.trim().is_empty() {
        return failure(
            request.request_id,
            "VCTR_CONFIG_INVALID",
            "Idempotency key required",
            "Every mutating daemon method requires a non-empty idempotency key.",
        );
    }
    if let Some(cached) = state
        .idempotency
        .lock()
        .await
        .get(&request.idempotency_key)
        .cloned()
    {
        let mut response = cached;
        response.request_id = request.request_id;
        return response;
    }
    let key = request.idempotency_key.clone();
    let response = dispatch(request, state).await;
    state.idempotency.lock().await.insert(key, response.clone());
    response
}

async fn dispatch(request: RequestEnvelope, state: &State) -> ResponseEnvelope {
    if let Some(result) = workbench_dispatch::dispatch(
        &request.method,
        &request.params,
        request.confirmation_token.as_deref(),
        &state.workbench,
    )
    .await
    {
        return match result {
            Ok(value) => success(request.request_id, value),
            Err(error) => runtime_failure(request.request_id, &error),
        };
    }
    match request.method.as_str() {
        "status" => success(
            request.request_id,
            json!({"status":"ready","startedAt":state.started_at,"protocolVersion":PROTOCOL_VERSION,"telemetry":false}),
        ),
        "providers.discover" => match ProviderDiscovery::new() {
            Ok(discovery) => success(
                request.request_id,
                json!(discovery.discover_defaults().await),
            ),
            Err(error) => failure(
                request.request_id,
                "VCTR_PROVIDER_UNAVAILABLE",
                "Provider discovery could not start",
                &error.to_string(),
            ),
        },
        "runs.list" => match state.database.recent_runs(100).await {
            Ok(runs) => success(request.request_id, json!(runs)),
            Err(error) => failure(
                request.request_id,
                "VCTR_RUN_FAILED",
                "Run history could not be read",
                &error.to_string(),
            ),
        },
        "harness.doctor" => {
            let harness = request
                .params
                .get("harness")
                .and_then(|value| value.as_str());
            let id = match harness {
                Some("omp") => Some(vector_core::HarnessId::Omp),
                Some("pi") => Some(vector_core::HarnessId::Pi),
                Some("deepseek") => Some(vector_core::HarnessId::Deepseek),
                _ => None,
            };
            match id {
                Some(id) => success(request.request_id, json!(adapter(id).doctor().await)),
                None => failure(
                    request.request_id,
                    "VCTR_CONFIG_INVALID",
                    "Unknown harness",
                    "Use omp, pi, or deepseek.",
                ),
            }
        }
        "harness.inventory" => success(request.request_id, json!(harness_inventory().await)),
        "harness.install" => match parse_harness(&request.params) {
            Ok(harness) => match install_managed_harness(harness).await {
                Ok(record) => success(request.request_id, json!(record)),
                Err(error) => runtime_failure(request.request_id, &error.to_string()),
            },
            Err(detail) => failure(
                request.request_id,
                "VCTR_CONFIG_INVALID",
                "Unknown harness",
                detail,
            ),
        },
        "onboarding.preflight" => match workspace_profile(&request.params) {
            Ok((workspace, profile)) => match preflight(&workspace, &profile).await {
                Ok(report) => success(request.request_id, json!(report)),
                Err(error) => runtime_failure(request.request_id, &error.to_string()),
            },
            Err(detail) => failure(
                request.request_id,
                "VCTR_CONFIG_INVALID",
                "Invalid preflight request",
                detail,
            ),
        },
        "onboarding.smoke" => match workspace_profile(&request.params) {
            Ok((workspace, profile)) => match smoke_test(&workspace, &profile).await {
                Ok(report) => success(request.request_id, json!(report)),
                Err(error) => runtime_failure(request.request_id, &error.to_string()),
            },
            Err(detail) => failure(
                request.request_id,
                "VCTR_CONFIG_INVALID",
                "Invalid smoke request",
                detail,
            ),
        },
        "onboarding.computer" => {
            match serde_json::from_value::<ComputerUseVerificationRequest>(request.params.clone()) {
                Ok(input) => match verify_computer_use(&input).await {
                    Ok(report) => success(request.request_id, json!(report)),
                    Err(error) => runtime_failure(request.request_id, &error.to_string()),
                },
                Err(error) => failure(
                    request.request_id,
                    "VCTR_CONFIG_INVALID",
                    "Invalid computer-use request",
                    &error.to_string(),
                ),
            }
        }
        "runs.prepare" => match parse_start_request(&request.params) {
            Ok(start) => match prepare_run(
                &start.workspace,
                &start.profile,
                start.surface,
                start.grant_yolo,
            )
            .await
            {
                Ok(prepared) => success(
                    request.request_id,
                    json!({
                        "runId": prepared.ledger.manifest.id,
                        "directory": prepared.ledger.dir,
                        "plan": prepared.plan,
                    }),
                ),
                Err(error) => runtime_failure(request.request_id, &error.to_string()),
            },
            Err(error) => failure(
                request.request_id,
                "VCTR_CONFIG_INVALID",
                "Invalid run request",
                &error,
            ),
        },
        "runs.start" => match parse_start_request(&request.params) {
            Ok(start) => {
                if start.grant_yolo && request.confirmation_token.as_deref() != Some("VECTOR-YOLO")
                {
                    failure(
                        request.request_id,
                        "VCTR_POLICY_DENIED",
                        "YOLO acknowledgement required",
                        "Pass the one-time confirmation token VECTOR-YOLO for this development build.",
                    )
                } else {
                    match preflight(&start.workspace, &start.profile).await {
                        Ok(report) if report.ready_to_work => match prepare_run(
                            &start.workspace,
                            &start.profile,
                            start.surface,
                            start.grant_yolo,
                        )
                        .await
                        {
                            Ok(prepared) => match state.sessions.start(&start, prepared).await {
                                Ok(session) => success(request.request_id, json!(session)),
                                Err(error) => {
                                    runtime_failure(request.request_id, &error.to_string())
                                }
                            },
                            Err(error) => runtime_failure(request.request_id, &error.to_string()),
                        },
                        Ok(_) => failure(
                            request.request_id,
                            "VCTR_PREFLIGHT_FAILED",
                            "Coding smoke test required",
                            "Complete the exact profile's disposable coding smoke test before the first real launch.",
                        ),
                        Err(error) => runtime_failure(request.request_id, &error.to_string()),
                    }
                }
            }
            Err(error) => failure(
                request.request_id,
                "VCTR_CONFIG_INVALID",
                "Invalid run request",
                &error,
            ),
        },
        "runs.prompt" => {
            session_text(
                request.request_id,
                &request.params,
                "prompt",
                |run_id, text| async move { state.sessions.prompt(run_id, &text).await },
            )
            .await
        }
        "runs.steer" => {
            session_text(
                request.request_id,
                &request.params,
                "message",
                |run_id, text| async move { state.sessions.steer(run_id, &text).await },
            )
            .await
        }
        "runs.abort" => match session_id(&request.params) {
            Ok(run_id) => async_session(request.request_id, state.sessions.abort(run_id)).await,
            Err(detail) => failure(
                request.request_id,
                "VCTR_CONFIG_INVALID",
                "Invalid abort request",
                detail,
            ),
        },
        "runs.stop" => match session_id(&request.params) {
            Ok(run_id) => async_session(request.request_id, state.sessions.stop(run_id)).await,
            Err(detail) => failure(
                request.request_id,
                "VCTR_CONFIG_INVALID",
                "Invalid stop request",
                detail,
            ),
        },
        "approvals.respond" => {
            let parsed = session_id(&request.params);
            match parsed {
                Ok(run_id) => {
                    let native_id = request
                        .params
                        .get("requestId")
                        .cloned()
                        .unwrap_or(Value::Null);
                    let option = request
                        .params
                        .get("optionId")
                        .cloned()
                        .unwrap_or(Value::Null);
                    async_session(
                        request.request_id,
                        state.sessions.approval(run_id, native_id, option),
                    )
                    .await
                }
                Err(detail) => failure(
                    request.request_id,
                    "VCTR_CONFIG_INVALID",
                    "Invalid approval response",
                    detail,
                ),
            }
        }
        "events.snapshot" | "events.subscribe" => match session_id(&request.params) {
            Ok(run_id) => {
                let after = request
                    .params
                    .get("afterSequence")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                match state.sessions.snapshot(run_id, after).await {
                    Ok(events) => success(
                        request.request_id,
                        json!({"events":events,"afterSequence":after}),
                    ),
                    Err(error) => runtime_failure(request.request_id, &error.to_string()),
                }
            }
            Err(detail) => failure(
                request.request_id,
                "VCTR_CONFIG_INVALID",
                "Invalid event request",
                detail,
            ),
        },
        _ => failure(
            request.request_id,
            "VCTR_CONFIG_INVALID",
            "Unknown daemon method",
            &request.method,
        ),
    }
}

fn parse_harness(params: &Value) -> Result<HarnessId, &'static str> {
    match params.get("harness").and_then(Value::as_str) {
        Some("omp") => Ok(HarnessId::Omp),
        Some("pi") => Ok(HarnessId::Pi),
        Some("deepseek") => Ok(HarnessId::Deepseek),
        _ => Err("Use omp, pi, or deepseek."),
    }
}

fn workspace_profile(params: &Value) -> Result<(PathBuf, String), &'static str> {
    let workspace = params
        .get("workspace")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or("workspace is required")?;
    let profile = params
        .get("profile")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or("profile is required")?;
    Ok((PathBuf::from(workspace), profile.into()))
}

fn parse_start_request(params: &Value) -> Result<RunStartRequest, String> {
    serde_json::from_value(params.clone()).map_err(|error| error.to_string())
}

fn session_id(params: &Value) -> Result<Uuid, &'static str> {
    params
        .get("runId")
        .and_then(Value::as_str)
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or("runId must be a UUID")
}

async fn async_session(
    request_id: Uuid,
    future: impl std::future::Future<Output = anyhow::Result<vector_core::InteractiveSessionState>>,
) -> ResponseEnvelope {
    match future.await {
        Ok(session) => success(request_id, json!(session)),
        Err(error) => runtime_failure(request_id, &error.to_string()),
    }
}

async fn session_text<F, Fut>(
    request_id: Uuid,
    params: &Value,
    field: &'static str,
    operation: F,
) -> ResponseEnvelope
where
    F: FnOnce(Uuid, String) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<vector_core::InteractiveSessionState>>,
{
    let Ok(run_id) = session_id(params) else {
        return failure(
            request_id,
            "VCTR_CONFIG_INVALID",
            "Invalid session request",
            "runId must be a UUID",
        );
    };
    let Some(text) = params.get(field).and_then(Value::as_str) else {
        return failure(
            request_id,
            "VCTR_CONFIG_INVALID",
            "Invalid session request",
            "prompt text is required",
        );
    };
    async_session(request_id, operation(run_id, text.into())).await
}

fn runtime_failure(request_id: Uuid, message: &str) -> ResponseEnvelope {
    let code = message
        .split(':')
        .next()
        .filter(|value| value.starts_with("VCTR_"))
        .unwrap_or("VCTR_RUN_FAILED");
    failure(request_id, code, "Vector operation failed", message)
}

fn success(request_id: Uuid, result: serde_json::Value) -> ResponseEnvelope {
    ResponseEnvelope {
        protocol_version: PROTOCOL_VERSION.into(),
        request_id,
        ok: true,
        result: Some(result),
        diagnostic: None,
    }
}

fn failure(request_id: Uuid, code: &str, summary: &str, detail: &str) -> ResponseEnvelope {
    ResponseEnvelope {
        protocol_version: PROTOCOL_VERSION.into(),
        request_id,
        ok: false,
        result: None,
        diagnostic: Some(Diagnostic::error(code, summary, detail)),
    }
}

fn chrono_like_now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs().to_string())
        .unwrap_or_else(|_| "0".into())
}
