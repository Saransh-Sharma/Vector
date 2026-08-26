use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;
use vector_config::{load_workspace, starter_workspace, write_workspace_atomic};
use vector_core::{
    HarnessId, PROTOCOL_VERSION, RequestEnvelope, ResponseEnvelope, application_paths,
};
use vector_harness::install_managed_harness;
use vector_providers::{DiscoveredProvider, ProviderDiscovery};
use vector_runtime::{preflight, smoke_test};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SystemSnapshot {
    os: &'static str,
    architecture: &'static str,
    cwd: String,
    telemetry: bool,
    update_checks: bool,
    tools: BTreeMap<String, bool>,
    configured: bool,
    default_profile: Option<String>,
}

#[tauri::command]
fn system_snapshot() -> SystemSnapshot {
    let tools = ["git", "bun", "node", "omp", "pi", "npx"]
        .into_iter()
        .map(|tool| (tool.to_string(), executable_on_path(tool)))
        .collect();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let configured = load_workspace(&cwd).ok();
    SystemSnapshot {
        os: std::env::consts::OS,
        architecture: std::env::consts::ARCH,
        cwd: cwd.display().to_string(),
        telemetry: false,
        update_checks: false,
        tools,
        default_profile: configured
            .as_ref()
            .and_then(|resolved| resolved.config.default_profile.clone()),
        configured: configured.is_some(),
    }
}

#[tauri::command]
async fn onboarding_preflight(workspace: String, profile: String) -> Result<Value, String> {
    preflight(std::path::Path::new(&workspace), &profile)
        .await
        .map(|value| json!(value))
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn onboarding_smoke(workspace: String, profile: String) -> Result<Value, String> {
    smoke_test(std::path::Path::new(&workspace), &profile)
        .await
        .map(|value| json!(value))
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn harness_install(harness: String) -> Result<Value, String> {
    let harness = parse_harness(&harness)?;
    install_managed_harness(harness)
        .await
        .map(|value| json!(value))
        .map_err(|error| error.to_string())
}

fn parse_harness(value: &str) -> Result<HarnessId, String> {
    match value {
        "omp" => Ok(HarnessId::Omp),
        "pi" => Ok(HarnessId::Pi),
        "deepseek" => Ok(HarnessId::Deepseek),
        _ => Err("Use omp, pi, or deepseek.".into()),
    }
}

#[tauri::command]
async fn daemon_call(
    method: String,
    params: Value,
    confirmation_token: Option<String>,
) -> Result<Value, String> {
    let response = daemon_request(&method, params, confirmation_token)
        .await
        .map_err(|error| error.to_string())?;
    if response.ok {
        Ok(response.result.unwrap_or(Value::Null))
    } else {
        Err(response
            .diagnostic
            .map(|diagnostic| diagnostic.detail)
            .unwrap_or_else(|| "Daemon request failed".into()))
    }
}

#[cfg(unix)]
async fn daemon_request(
    method: &str,
    params: Value,
    confirmation_token: Option<String>,
) -> Result<ResponseEnvelope, Box<dyn std::error::Error>> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;
    let paths = application_paths().ok_or("Vector application paths are unavailable")?;
    let socket = paths.data_dir.join("state/vectord.sock");
    let stream = match UnixStream::connect(&socket).await {
        Ok(stream) => stream,
        Err(_) => start_desktop_daemon(&socket).await?,
    };
    let token = tokio::fs::read_to_string(paths.data_dir.join("state/daemon.token")).await?;
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    let challenge = RequestEnvelope {
        protocol_version: PROTOCOL_VERSION.into(),
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::new_v4().to_string(),
        method: "auth.challenge".into(),
        params: json!({}),
        auth: None,
        confirmation_token: None,
    };
    write
        .write_all(format!("{}\n", serde_json::to_string(&challenge)?).as_bytes())
        .await?;
    let response: ResponseEnvelope =
        serde_json::from_str(&lines.next_line().await?.ok_or("daemon closed")?)?;
    let challenge = response
        .result
        .and_then(|value| {
            value
                .get("challenge")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .ok_or("missing challenge")?;
    let auth = blake3::hash(format!("{}:{challenge}", token.trim()).as_bytes())
        .to_hex()
        .to_string();
    let request = RequestEnvelope {
        protocol_version: PROTOCOL_VERSION.into(),
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::new_v4().to_string(),
        method: method.into(),
        params,
        auth: Some(auth),
        confirmation_token,
    };
    write
        .write_all(format!("{}\n", serde_json::to_string(&request)?).as_bytes())
        .await?;
    Ok(serde_json::from_str(
        &lines.next_line().await?.ok_or("daemon closed")?,
    )?)
}

#[cfg(unix)]
async fn start_desktop_daemon(
    socket: &std::path::Path,
) -> Result<tokio::net::UnixStream, Box<dyn std::error::Error>> {
    use std::process::Stdio;
    use tokio::net::UnixStream;
    let explicit = std::env::var_os("VECTOR_DAEMON").map(std::path::PathBuf::from);
    let sibling = std::env::current_exe()?
        .parent()
        .map(|parent| parent.join("vectord"));
    if let Some(executable) = explicit.or(sibling.filter(|path| path.is_file())) {
        std::process::Command::new(executable)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
    } else {
        let source_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        std::process::Command::new("cargo")
            .args(["run", "-q", "-p", "vectord"])
            .current_dir(source_root)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
    }
    for _ in 0..80 {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        if let Ok(stream) = UnixStream::connect(socket).await {
            return Ok(stream);
        }
    }
    Err("vectord did not become ready within 20 seconds".into())
}

#[cfg(not(unix))]
async fn daemon_request(
    _method: &str,
    _params: Value,
    _confirmation_token: Option<String>,
) -> Result<ResponseEnvelope, Box<dyn std::error::Error>> {
    Err("Named-pipe desktop transport is not enabled".into())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InitializeInput {
    workspace: String,
    model: String,
    vision_model: Option<String>,
    computer_use: bool,
    harness: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct InitializeResult {
    path: String,
    default_profile: String,
}

#[tauri::command]
fn initialize_workspace(input: InitializeInput) -> Result<InitializeResult, String> {
    let _ = input.computer_use; // Computer capabilities remain denied until the nonce/OS fixture passes.
    let root = std::path::PathBuf::from(&input.workspace)
        .canonicalize()
        .map_err(|error| format!("Workspace path is not accessible: {error}"))?;
    let mut config = starter_workspace(&input.model, input.vision_model.as_deref(), false);
    config.default_profile = Some(
        match input.harness.as_str() {
            "omp" => "omp-safe",
            "deepseek" => "deepseek-preview",
            _ => "pi-safe",
        }
        .into(),
    );
    let path = write_workspace_atomic(&root, &config).map_err(|error| error.to_string())?;
    Ok(InitializeResult {
        path: path.display().to_string(),
        default_profile: config.default_profile.unwrap_or_else(|| "pi-safe".into()),
    })
}

#[tauri::command]
async fn discover_lm_studio(endpoint: String) -> Result<DiscoveredProvider, String> {
    ProviderDiscovery::new()
        .map_err(|error| error.to_string())?
        .ensure_lm_studio(&endpoint)
        .await
        .map_err(|error| error.to_string())
}

fn executable_on_path(executable: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|path| path.join(executable).is_file())
    })
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            system_snapshot,
            discover_lm_studio,
            initialize_workspace,
            onboarding_preflight,
            onboarding_smoke,
            harness_install,
            daemon_call
        ])
        .run(tauri::generate_context!())
        .expect("Vector desktop failed to start");
}
