use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Parser;
use serde_json::json;
use uuid::Uuid;
use vector_core::{
    Diagnostic, PROTOCOL_VERSION, RequestEnvelope, ResponseEnvelope, application_paths,
};
use vector_db::VectorDatabase;
use vector_harness::adapter;
use vector_providers::ProviderDiscovery;

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
    let state = State {
        database,
        auth_token,
        started_at: chrono_like_now(),
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
                dispatch(request, &state).await
            }
        };
        write
            .write_all(format!("{}\n", serde_json::to_string(&response)?).as_bytes())
            .await?;
    }
    Ok(())
}

async fn dispatch(request: RequestEnvelope, state: &State) -> ResponseEnvelope {
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
        _ => failure(
            request.request_id,
            "VCTR_CONFIG_INVALID",
            "Unknown daemon method",
            &request.method,
        ),
    }
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
