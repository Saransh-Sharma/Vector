use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{Value, json};
use uuid::Uuid;
use vector_core::PolicyDecision;
use vector_runtime::workbench::Workbench;

pub fn is_mutating(method: &str) -> bool {
    matches!(
        method,
        "models.folders.add"
            | "models.scan"
            | "backends.add"
            | "backend-groups.add"
            | "servers.create"
            | "servers.start"
            | "servers.stop"
            | "downloads.start"
            | "recipes.add"
            | "recipes.run"
            | "proxy.aliases.add"
            | "checkpoints.add"
            | "mcp.servers.add"
            | "voice.services.add"
            | "voice.services.start"
            | "chat.threads.create"
            | "chat.messages.append"
            | "notifications.create"
    )
}

pub async fn dispatch(
    method: &str,
    params: &Value,
    confirmation: Option<&str>,
    workbench: &Workbench,
) -> Option<Result<Value, String>> {
    if !matches!(
        method,
        "workbench.snapshot"
            | "models.list"
            | "backends.list"
            | "servers.list"
            | "downloads.list"
            | "recipes.list"
            | "proxy.aliases.list"
            | "checkpoints.list"
            | "mcp.servers.list"
            | "voice.services.list"
            | "chat.threads.list"
            | "notifications.list"
            | "hardware.snapshot"
            | "hub.search"
            | "models.folders.add"
            | "models.scan"
            | "backends.add"
            | "backend-groups.add"
            | "servers.create"
            | "servers.start"
            | "servers.stop"
            | "downloads.start"
            | "recipes.add"
            | "recipes.run"
            | "proxy.aliases.add"
            | "checkpoints.add"
            | "mcp.servers.add"
            | "voice.services.add"
            | "voice.services.start"
            | "chat.threads.create"
            | "chat.messages.append"
            | "notifications.create"
            | "workspace.search"
            | "workspace.code-graph"
    ) {
        return None;
    }
    let result: Result<Value, String> = async {
    match method {
        "workbench.snapshot" | "models.list" | "backends.list" | "servers.list" |
        "downloads.list" | "recipes.list" | "proxy.aliases.list" | "checkpoints.list" |
        "mcp.servers.list" | "voice.services.list" | "chat.threads.list" | "notifications.list" => {
            Ok(json!(workbench.snapshot().await))
        }
        "hardware.snapshot" => Ok(json!(workbench.hardware().await)),
        "hub.search" => workbench.search_hub(required_string(params, "query")?, params.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize).await.map_err(stringify),
        "models.folders.add" => workbench.add_model_folder(&PathBuf::from(required_string(params, "path")?)).await.map(|value| json!(value)).map_err(stringify),
        "models.scan" => workbench.scan_model_folder(required_uuid(params, "folderId")?).await.map(|value| json!(value)).map_err(stringify),
        "backends.add" => workbench.add_backend(required_string(params, "name")?.into(), PathBuf::from(required_string(params, "executable")?), params.get("kind").and_then(Value::as_str).unwrap_or("llama.cpp").into()).await.map(|value| json!(value)).map_err(stringify),
        "backend-groups.add" => workbench.add_backend_group(required_string(params, "name")?.into(), uuid_array(params, "backendIds")?).await.map(|value| json!(value)).map_err(stringify),
        "servers.create" => workbench.create_server(
            required_string(params, "name")?.into(), required_uuid(params, "backendId")?, required_uuid(params, "modelId")?,
            params.get("host").and_then(Value::as_str).unwrap_or("127.0.0.1").into(),
            params.get("port").and_then(Value::as_u64).unwrap_or(8080) as u16,
            params.get("contextSize").and_then(Value::as_u64).unwrap_or(8192),
            params.get("gpuLayers").and_then(Value::as_i64).unwrap_or(-1) as i32,
            float_array(params, "tensorSplit"), string_array(params, "extraArgs"),
            params.get("embedding").and_then(Value::as_bool).unwrap_or(false),
            optional_uuid(params, "speculativeModelId")?, params.get("autoStart").and_then(Value::as_bool).unwrap_or(false),
        ).await.map(|value| json!(value)).map_err(stringify),
        "servers.start" => workbench.start_server(required_uuid(params, "serverId")?).await.map(|value| json!(value)).map_err(stringify),
        "servers.stop" => workbench.stop_server(required_uuid(params, "serverId")?).await.map(|value| json!(value)).map_err(stringify),
        "downloads.start" => workbench.download(required_string(params, "url")?.into(), PathBuf::from(required_string(params, "destination")?), params.get("expectedSha256").and_then(Value::as_str).map(str::to_owned)).await.map(|value| json!(value)).map_err(stringify),
        "recipes.add" => workbench.add_recipe(
            required_string(params, "name")?.into(), PathBuf::from(required_string(params, "executable")?), string_array(params, "args"),
            params.get("workingDirectory").and_then(Value::as_str).map(PathBuf::from),
            params.get("trusted").and_then(Value::as_bool).unwrap_or(false), params.get("source").and_then(Value::as_str).unwrap_or("user").into(),
        ).await.map(|value| json!(value)).map_err(stringify),
        "recipes.run" => {
            if confirmation != Some("VECTOR-RECIPE") { Err("VCTR_POLICY_DENIED: recipe execution requires an explicit one-time confirmation".into()) }
            else { workbench.run_recipe(required_uuid(params, "recipeId")?).await.map_err(stringify) }
        },
        "proxy.aliases.add" => workbench.add_proxy_alias(required_string(params, "alias")?.into(), required_uuid(params, "serverId")?, required_string(params, "modelName")?.into()).await.map(|value| json!(value)).map_err(stringify),
        "checkpoints.add" => workbench.add_checkpoint(required_uuid(params, "serverId")?, PathBuf::from(required_string(params, "path")?), params.get("slot").and_then(Value::as_u64).unwrap_or(0) as u32, params.get("tokenCount").and_then(Value::as_u64).unwrap_or(0)).await.map(|value| json!(value)).map_err(stringify),
        "mcp.servers.add" => workbench.add_mcp_server(
            required_string(params, "name")?.into(), PathBuf::from(required_string(params, "command")?), string_array(params, "args"),
            string_map(params, "secretRefs"), parse_decision(params.get("decision").and_then(Value::as_str).unwrap_or("prompt"))?,
        ).await.map(|value| json!(value)).map_err(stringify),
        "voice.services.add" => workbench.add_voice_service(required_string(params, "name")?.into(), required_string(params, "kind")?.into(), PathBuf::from(required_string(params, "executable")?), string_array(params, "args"), required_string(params, "endpoint")?.into()).await.map(|value| json!(value)).map_err(stringify),
        "voice.services.start" => workbench.start_voice_service(required_uuid(params, "serviceId")?).await.map(|value| json!(value)).map_err(stringify),
        "chat.threads.create" => workbench.create_thread(required_string(params, "title")?.into(), params.get("profile").and_then(Value::as_str).map(str::to_owned)).await.map(|value| json!(value)).map_err(stringify),
        "chat.messages.append" => workbench.append_message(
            required_uuid(params, "threadId")?, optional_uuid(params, "parentId")?, params.get("role").and_then(Value::as_str).unwrap_or("user").into(),
            required_string(params, "content")?.into(), optional_uuid(params, "runId")?, string_array(params, "attachments"),
        ).await.map(|value| json!(value)).map_err(stringify),
        "notifications.create" => workbench.notify(params.get("level").and_then(Value::as_str).unwrap_or("info").into(), required_string(params, "title")?.into(), required_string(params, "detail")?.into()).await.map(|value| json!(value)).map_err(stringify),
        "workspace.search" => workbench.search_workspace(&PathBuf::from(required_string(params, "workspace")?), required_string(params, "query")?, params.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize).await.map_err(stringify),
        "workspace.code-graph" => workbench.code_graph(&PathBuf::from(required_string(params, "workspace")?), params.get("limit").and_then(Value::as_u64).unwrap_or(1000) as usize).await.map_err(stringify),
        _ => unreachable!(),
    }
    }.await;
    Some(result)
}

fn required_string<'a>(params: &'a Value, field: &str) -> Result<&'a str, String> {
    params
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("VCTR_CONFIG_INVALID: {field} is required"))
}
fn required_uuid(params: &Value, field: &str) -> Result<Uuid, String> {
    Uuid::parse_str(required_string(params, field)?)
        .map_err(|_| format!("VCTR_CONFIG_INVALID: {field} must be a UUID"))
}
fn optional_uuid(params: &Value, field: &str) -> Result<Option<Uuid>, String> {
    params
        .get(field)
        .and_then(Value::as_str)
        .map(|value| {
            Uuid::parse_str(value)
                .map_err(|_| format!("VCTR_CONFIG_INVALID: {field} must be a UUID"))
        })
        .transpose()
}
fn uuid_array(params: &Value, field: &str) -> Result<Vec<Uuid>, String> {
    match params.get(field).and_then(Value::as_array) {
        Some(values) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .ok_or_else(|| format!("VCTR_CONFIG_INVALID: {field} values must be UUIDs"))
                    .and_then(|value| {
                        Uuid::parse_str(value).map_err(|_| {
                            format!("VCTR_CONFIG_INVALID: {field} values must be UUIDs")
                        })
                    })
            })
            .collect(),
        None => Ok(Vec::new()),
    }
}
fn string_array(params: &Value, field: &str) -> Vec<String> {
    params
        .get(field)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}
fn float_array(params: &Value, field: &str) -> Vec<f32> {
    params
        .get(field)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_f64)
        .map(|value| value as f32)
        .collect()
}
fn string_map(params: &Value, field: &str) -> BTreeMap<String, String> {
    params
        .get(field)
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| value.as_str().map(|value| (key.clone(), value.into())))
        .collect()
}
fn parse_decision(value: &str) -> Result<PolicyDecision, String> {
    match value {
        "allow" => Ok(PolicyDecision::Allow),
        "prompt" => Ok(PolicyDecision::Prompt),
        "deny" => Ok(PolicyDecision::Deny),
        _ => Err("VCTR_CONFIG_INVALID: decision must be allow, prompt, or deny".into()),
    }
}
fn stringify(error: impl ToString) -> String {
    error.to_string()
}
