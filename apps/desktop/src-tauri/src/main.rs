use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use vector_config::{starter_workspace, write_workspace_atomic};
use vector_providers::{DiscoveredProvider, ProviderDiscovery};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SystemSnapshot {
    os: &'static str,
    architecture: &'static str,
    cwd: String,
    telemetry: bool,
    update_checks: bool,
    tools: BTreeMap<String, bool>,
}

#[tauri::command]
fn system_snapshot() -> SystemSnapshot {
    let tools = ["git", "bun", "node", "omp", "pi", "npx"]
        .into_iter()
        .map(|tool| (tool.to_string(), executable_on_path(tool)))
        .collect();
    SystemSnapshot {
        os: std::env::consts::OS,
        architecture: std::env::consts::ARCH,
        cwd: std::env::current_dir()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| ".".into()),
        telemetry: false,
        update_checks: false,
        tools,
    }
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
    let root = std::path::PathBuf::from(&input.workspace)
        .canonicalize()
        .map_err(|error| format!("Workspace path is not accessible: {error}"))?;
    let mut config = starter_workspace(
        &input.model,
        input.vision_model.as_deref(),
        input.computer_use,
    );
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
        .lm_studio(&endpoint)
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
            initialize_workspace
        ])
        .run(tauri::generate_context!())
        .expect("Vector desktop failed to start");
}
