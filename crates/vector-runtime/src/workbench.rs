//! Append-only operational resources for the local model workbench.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use chrono::Utc;
use futures_util::StreamExt;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;
use vector_core::{
    BackendGroup, BackendRecord, ChatMessage, ChatThread, CheckpointRecord, DownloadJob,
    HardwareSnapshot, ManagedModel, ManagedServer, McpServerRecord, ModelFolder, ModelServerSpec,
    NotificationRecord, PolicyDecision, ProxyAlias, RecipeSpec, ResourceState, VoiceService,
    WorkbenchEvent, WorkbenchSnapshot,
};

#[derive(Clone)]
pub struct Workbench {
    root: PathBuf,
    state: Arc<RwLock<WorkbenchSnapshot>>,
    children: Arc<Mutex<HashMap<Uuid, Child>>>,
}

impl Workbench {
    pub async fn open(data_dir: &Path) -> Result<Self, WorkbenchError> {
        let root = data_dir.join("workbench");
        tokio::fs::create_dir_all(root.join("artifacts")).await?;
        let projection = root.join("projection.json");
        let mut state: WorkbenchSnapshot = match tokio::fs::read(&projection).await {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                WorkbenchSnapshot::default()
            }
            Err(error) => return Err(error.into()),
        };
        for server in &mut state.servers {
            if server.state == ResourceState::Running {
                server.state = ResourceState::Interrupted;
                server.pid = None;
            }
        }
        for service in &mut state.voice_services {
            if service.state == ResourceState::Running {
                service.state = ResourceState::Interrupted;
                service.pid = None;
            }
        }
        let workbench = Self {
            root,
            state: Arc::new(RwLock::new(state)),
            children: Arc::new(Mutex::new(HashMap::new())),
        };
        workbench.persist().await?;
        Ok(workbench)
    }

    pub async fn snapshot(&self) -> WorkbenchSnapshot {
        self.state.read().await.clone()
    }

    pub async fn hardware(&self) -> HardwareSnapshot {
        let memory_bytes = memory_bytes().await;
        let mut devices = Vec::new();
        for backend in &self.state.read().await.backends {
            for device in &backend.devices {
                if !devices.contains(device) {
                    devices.push(device.clone());
                }
            }
        }
        if devices.is_empty() {
            devices.push("CPU".into());
        }
        HardwareSnapshot {
            os: std::env::consts::OS.into(),
            architecture: std::env::consts::ARCH.into(),
            logical_cpus: std::thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1),
            memory_bytes,
            devices,
        }
    }

    pub async fn search_hub(&self, query: &str, limit: usize) -> Result<Value, WorkbenchError> {
        if query.trim().is_empty() {
            return Err(WorkbenchError::Invalid(
                "Hub search query cannot be empty".into(),
            ));
        }
        let response = reqwest::Client::builder()
            .build()?
            .get("https://huggingface.co/api/models")
            .query(&[
                ("search", query),
                ("filter", "gguf"),
                ("limit", &limit.min(50).to_string()),
            ])
            .send()
            .await?
            .error_for_status()?;
        let models: Value = response.json().await?;
        Ok(json!({"models":models}))
    }

    pub async fn add_model_folder(&self, path: &Path) -> Result<WorkbenchSnapshot, WorkbenchError> {
        let canonical = path.canonicalize()?;
        if !canonical.is_dir() {
            return Err(WorkbenchError::Invalid(
                "Model folder must be a directory".into(),
            ));
        }
        let writable = is_writable_directory(&canonical).await;
        let folder_id = {
            let mut state = self.state.write().await;
            if let Some(existing) = state
                .model_folders
                .iter()
                .find(|folder| folder.path == canonical)
            {
                existing.id
            } else {
                let folder = ModelFolder {
                    id: Uuid::now_v7(),
                    path: canonical.clone(),
                    writable,
                    added_at: Utc::now(),
                };
                let id = folder.id;
                state.model_folders.push(folder);
                id
            }
        };
        self.record(
            "models.folder-added",
            json!({"folderId":folder_id,"path":canonical,"writable":writable}),
        )
        .await?;
        self.scan_model_folder(folder_id).await
    }

    pub async fn scan_model_folder(
        &self,
        folder_id: Uuid,
    ) -> Result<WorkbenchSnapshot, WorkbenchError> {
        let folder = self
            .state
            .read()
            .await
            .model_folders
            .iter()
            .find(|item| item.id == folder_id)
            .cloned()
            .ok_or_else(|| WorkbenchError::NotFound("model folder".into()))?;
        let root = folder.path.clone();
        let groups = tokio::task::spawn_blocking(move || scan_gguf_files(&root))
            .await
            .map_err(|error| WorkbenchError::Task(error.to_string()))??;
        let previous = self.state.read().await.models.clone();
        let mut models = Vec::new();
        for group in groups {
            let first = group
                .files
                .first()
                .ok_or_else(|| WorkbenchError::Invalid("empty model group".into()))?;
            let id = previous
                .iter()
                .find(|model| model.folder_id == folder_id && model.path == *first)
                .map(|model| model.id)
                .unwrap_or_else(Uuid::now_v7);
            let size_bytes = group
                .files
                .iter()
                .filter_map(|path| path.metadata().ok().map(|value| value.len()))
                .sum();
            let fingerprint = file_group_fingerprint(&group.files)?;
            let name = first
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("model.gguf")
                .to_string();
            let lower = name.to_ascii_lowercase();
            models.push(ManagedModel {
                id,
                folder_id,
                path: first.clone(),
                display_name: display_model_name(&name),
                size_bytes,
                fingerprint,
                format: gguf_version(first).unwrap_or_else(|| "GGUF".into()),
                architecture: infer_architecture(&lower),
                quantization: infer_quantization(&name),
                shard_paths: group.files.iter().skip(1).cloned().collect(),
                projector_path: group.projector,
                embedding: lower.contains("embed"),
                vision: lower.contains("vision")
                    || lower.contains("vl")
                    || lower.contains("mmproj"),
                indexed_at: Utc::now(),
            });
        }
        {
            let mut state = self.state.write().await;
            state.models.retain(|model| model.folder_id != folder_id);
            state.models.extend(models);
        }
        self.record("models.scanned", json!({"folderId":folder_id}))
            .await?;
        self.persist_and_snapshot().await
    }

    pub async fn add_backend(
        &self,
        name: String,
        executable: PathBuf,
        kind: String,
    ) -> Result<WorkbenchSnapshot, WorkbenchError> {
        let executable = executable.canonicalize()?;
        if !executable.is_file() {
            return Err(WorkbenchError::Invalid(
                "Backend executable was not found".into(),
            ));
        }
        let output = Command::new(&executable)
            .arg("--version")
            .stdin(Stdio::null())
            .output()
            .await;
        let (verified, version) = match output {
            Ok(output) if output.status.success() => {
                let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
                (true, (!text.is_empty()).then_some(text))
            }
            _ => (false, None),
        };
        let devices = detect_backend_devices(&executable).await;
        let metadata = executable.metadata()?;
        let fingerprint = blake3::hash(
            format!(
                "{}:{}:{}",
                executable.display(),
                metadata.len(),
                version.as_deref().unwrap_or("unknown")
            )
            .as_bytes(),
        )
        .to_hex()
        .to_string();
        let record = BackendRecord {
            id: Uuid::now_v7(),
            name,
            executable,
            kind,
            version,
            devices,
            fingerprint,
            verified,
            added_at: Utc::now(),
        };
        let id = record.id;
        self.state.write().await.backends.push(record);
        self.record("backends.added", json!({"backendId":id}))
            .await?;
        self.persist_and_snapshot().await
    }

    pub async fn add_backend_group(
        &self,
        name: String,
        backend_ids: Vec<Uuid>,
    ) -> Result<WorkbenchSnapshot, WorkbenchError> {
        let state = self.state.read().await;
        if backend_ids
            .iter()
            .any(|id| !state.backends.iter().any(|backend| backend.id == *id))
        {
            return Err(WorkbenchError::NotFound("backend group member".into()));
        }
        drop(state);
        let group = BackendGroup {
            id: Uuid::now_v7(),
            name,
            active_backend_id: backend_ids.first().copied(),
            backend_ids,
        };
        let id = group.id;
        self.state.write().await.backend_groups.push(group);
        self.record("backend-groups.added", json!({"groupId":id}))
            .await?;
        self.persist_and_snapshot().await
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_server(
        &self,
        name: String,
        backend_id: Uuid,
        model_id: Uuid,
        host: String,
        port: u16,
        context_size: u64,
        gpu_layers: i32,
        tensor_split: Vec<f32>,
        extra_args: Vec<String>,
        embedding: bool,
        speculative_model_id: Option<Uuid>,
        auto_start: bool,
    ) -> Result<WorkbenchSnapshot, WorkbenchError> {
        if !is_loopback_host(&host) {
            return Err(WorkbenchError::Policy(
                "Non-loopback server binds require the protected remote-access gateway".into(),
            ));
        }
        let state = self.state.read().await;
        let backend = state
            .backends
            .iter()
            .find(|item| item.id == backend_id)
            .ok_or_else(|| WorkbenchError::NotFound("backend".into()))?;
        let model = state
            .models
            .iter()
            .find(|item| item.id == model_id)
            .ok_or_else(|| WorkbenchError::NotFound("model".into()))?;
        if let Some(draft) = speculative_model_id
            && !state.models.iter().any(|item| item.id == draft)
        {
            return Err(WorkbenchError::NotFound("speculative model".into()));
        }
        let stable = json!({"backend":backend.fingerprint,"model":model.fingerprint,"host":host,"port":port,"context":context_size,
            "gpuLayers":gpu_layers,"tensorSplit":tensor_split,"extraArgs":extra_args,"embedding":embedding,"draft":speculative_model_id});
        let fingerprint = blake3::hash(&serde_json::to_vec(&stable)?)
            .to_hex()
            .to_string();
        drop(state);
        let spec = ModelServerSpec {
            id: Uuid::now_v7(),
            name,
            backend_id,
            model_id,
            host,
            port,
            context_size,
            gpu_layers,
            tensor_split,
            extra_args,
            embedding,
            speculative_model_id,
            auto_start,
            fingerprint,
        };
        let id = spec.id;
        self.state.write().await.servers.push(ManagedServer {
            spec,
            state: ResourceState::Stopped,
            pid: None,
            last_error: None,
            started_at: None,
        });
        self.record("servers.created", json!({"serverId":id}))
            .await?;
        self.persist_and_snapshot().await
    }

    pub async fn start_server(&self, server_id: Uuid) -> Result<WorkbenchSnapshot, WorkbenchError> {
        if self.children.lock().await.contains_key(&server_id) {
            return Ok(self.snapshot().await);
        }
        let state = self.state.read().await;
        let server = state
            .servers
            .iter()
            .find(|item| item.spec.id == server_id)
            .cloned()
            .ok_or_else(|| WorkbenchError::NotFound("server".into()))?;
        let backend = state
            .backends
            .iter()
            .find(|item| item.id == server.spec.backend_id)
            .cloned()
            .ok_or_else(|| WorkbenchError::NotFound("backend".into()))?;
        let model = state
            .models
            .iter()
            .find(|item| item.id == server.spec.model_id)
            .cloned()
            .ok_or_else(|| WorkbenchError::NotFound("model".into()))?;
        drop(state);
        if !backend.verified {
            return Err(WorkbenchError::Policy(
                "Unverified backends cannot be launched".into(),
            ));
        }
        let mut args = vec![
            "-m".into(),
            model.path.display().to_string(),
            "--host".into(),
            server.spec.host.clone(),
            "--port".into(),
            server.spec.port.to_string(),
            "-c".into(),
            server.spec.context_size.to_string(),
            "-ngl".into(),
            server.spec.gpu_layers.to_string(),
        ];
        if !server.spec.tensor_split.is_empty() {
            args.extend([
                "--tensor-split".into(),
                server
                    .spec
                    .tensor_split
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(","),
            ]);
        }
        if server.spec.embedding {
            args.push("--embedding".into());
        }
        if let Some(draft_id) = server.spec.speculative_model_id {
            if let Some(draft) = self
                .state
                .read()
                .await
                .models
                .iter()
                .find(|item| item.id == draft_id)
            {
                args.extend(["-md".into(), draft.path.display().to_string()]);
            }
        }
        args.extend(server.spec.extra_args.clone());
        let child = Command::new(&backend.executable)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let pid = child.id();
        self.children.lock().await.insert(server_id, child);
        if let Some(target) = self
            .state
            .write()
            .await
            .servers
            .iter_mut()
            .find(|item| item.spec.id == server_id)
        {
            target.state = ResourceState::Running;
            target.pid = pid;
            target.started_at = Some(Utc::now());
            target.last_error = None;
        }
        self.record(
            "servers.started",
            json!({"serverId":server_id,"pid":pid,"fingerprint":server.spec.fingerprint}),
        )
        .await?;
        self.persist_and_snapshot().await
    }

    pub async fn stop_server(&self, server_id: Uuid) -> Result<WorkbenchSnapshot, WorkbenchError> {
        if let Some(mut child) = self.children.lock().await.remove(&server_id) {
            child.start_kill()?;
            let _ = child.wait().await;
        }
        let mut state = self.state.write().await;
        let server = state
            .servers
            .iter_mut()
            .find(|item| item.spec.id == server_id)
            .ok_or_else(|| WorkbenchError::NotFound("server".into()))?;
        server.state = ResourceState::Stopped;
        server.pid = None;
        drop(state);
        self.record("servers.stopped", json!({"serverId":server_id}))
            .await?;
        self.persist_and_snapshot().await
    }

    pub async fn download(
        &self,
        url: String,
        destination: PathBuf,
        expected_sha256: Option<String>,
    ) -> Result<WorkbenchSnapshot, WorkbenchError> {
        let destination = absolute_destination(&destination)?;
        let folders = self.state.read().await.model_folders.clone();
        if !folders
            .iter()
            .any(|folder| folder.writable && destination.starts_with(&folder.path))
        {
            return Err(WorkbenchError::Policy(
                "Downloads must target a registered writable model folder".into(),
            ));
        }
        if let Some(parent) = destination.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let now = Utc::now();
        let job = DownloadJob {
            id: Uuid::now_v7(),
            url: url.clone(),
            destination: destination.clone(),
            state: ResourceState::Running,
            received_bytes: 0,
            total_bytes: None,
            expected_sha256: expected_sha256.clone(),
            error: None,
            created_at: now,
            updated_at: now,
        };
        let job_id = job.id;
        self.state.write().await.downloads.push(job);
        self.record(
            "downloads.started",
            json!({"downloadId":job_id,"destination":destination}),
        )
        .await?;
        let part = destination.with_extension(format!(
            "{}part",
            destination
                .extension()
                .and_then(|value| value.to_str())
                .map(|value| format!("{value}."))
                .unwrap_or_default()
        ));
        let existing = tokio::fs::metadata(&part)
            .await
            .map(|value| value.len())
            .unwrap_or(0);
        let client = reqwest::Client::builder().build()?;
        let mut request = client.get(&url);
        if existing > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={existing}-"));
        }
        let response = request.send().await?.error_for_status()?;
        let total = response.content_length().map(|value| value + existing);
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(existing > 0)
            .write(true)
            .truncate(existing == 0)
            .open(&part)
            .await?;
        let mut received = existing;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            received += chunk.len() as u64;
        }
        file.flush().await?;
        if let Some(expected) = expected_sha256 {
            let actual = sha256_file(&part).await?;
            if !actual.eq_ignore_ascii_case(&expected) {
                self.fail_download(
                    job_id,
                    format!("Checksum mismatch: expected {expected}, got {actual}"),
                )
                .await?;
                return Err(WorkbenchError::Checksum);
            }
        }
        tokio::fs::rename(&part, &destination).await?;
        if let Some(job) = self
            .state
            .write()
            .await
            .downloads
            .iter_mut()
            .find(|item| item.id == job_id)
        {
            job.state = ResourceState::Ready;
            job.received_bytes = received;
            job.total_bytes = total;
            job.updated_at = Utc::now();
        }
        self.record(
            "downloads.completed",
            json!({"downloadId":job_id,"bytes":received}),
        )
        .await?;
        self.persist_and_snapshot().await
    }

    async fn fail_download(&self, id: Uuid, error: String) -> Result<(), WorkbenchError> {
        if let Some(job) = self
            .state
            .write()
            .await
            .downloads
            .iter_mut()
            .find(|item| item.id == id)
        {
            job.state = ResourceState::Failed;
            job.error = Some(error.clone());
            job.updated_at = Utc::now();
        }
        self.record("downloads.failed", json!({"downloadId":id,"error":error}))
            .await?;
        self.persist().await
    }

    pub async fn add_recipe(
        &self,
        name: String,
        executable: PathBuf,
        args: Vec<String>,
        working_directory: Option<PathBuf>,
        trusted: bool,
        source: String,
    ) -> Result<WorkbenchSnapshot, WorkbenchError> {
        let digest = blake3::hash(&serde_json::to_vec(
            &json!({"executable":executable,"args":args,"workingDirectory":working_directory}),
        )?)
        .to_hex()
        .to_string();
        let recipe = RecipeSpec {
            id: Uuid::now_v7(),
            name,
            executable,
            args,
            working_directory,
            digest,
            trusted,
            source,
        };
        let id = recipe.id;
        self.state.write().await.recipes.push(recipe);
        self.record("recipes.added", json!({"recipeId":id})).await?;
        self.persist_and_snapshot().await
    }

    pub async fn run_recipe(&self, recipe_id: Uuid) -> Result<Value, WorkbenchError> {
        let recipe = self
            .state
            .read()
            .await
            .recipes
            .iter()
            .find(|item| item.id == recipe_id)
            .cloned()
            .ok_or_else(|| WorkbenchError::NotFound("recipe".into()))?;
        if !recipe.trusted || recipe.source == "repository" {
            return Err(WorkbenchError::Policy(
                "Only explicitly trusted user-owned or bundled recipes may execute".into(),
            ));
        }
        let mut command = Command::new(&recipe.executable);
        command
            .args(&recipe.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = &recipe.working_directory {
            command.current_dir(cwd);
        }
        let output = command.output().await?;
        let result = json!({"recipeId":recipe_id,"success":output.status.success(),"code":output.status.code(),
            "stdout":redact_output(&String::from_utf8_lossy(&output.stdout)),"stderr":redact_output(&String::from_utf8_lossy(&output.stderr))});
        self.record("recipes.completed", result.clone()).await?;
        Ok(result)
    }

    pub async fn add_proxy_alias(
        &self,
        alias: String,
        server_id: Uuid,
        model_name: String,
    ) -> Result<WorkbenchSnapshot, WorkbenchError> {
        if alias.is_empty()
            || alias.chars().any(|character| {
                !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
            })
        {
            return Err(WorkbenchError::Invalid(
                "Proxy aliases may contain letters, numbers, dot, dash, and underscore".into(),
            ));
        }
        if !self
            .state
            .read()
            .await
            .servers
            .iter()
            .any(|server| server.spec.id == server_id)
        {
            return Err(WorkbenchError::NotFound("server".into()));
        }
        let item = ProxyAlias {
            id: Uuid::now_v7(),
            alias,
            server_id,
            model_name,
            enabled: true,
        };
        let id = item.id;
        self.state.write().await.proxy_aliases.push(item);
        self.record("proxy.alias-added", json!({"aliasId":id}))
            .await?;
        self.persist_and_snapshot().await
    }

    pub async fn add_checkpoint(
        &self,
        server_id: Uuid,
        path: PathBuf,
        slot: u32,
        token_count: u64,
    ) -> Result<WorkbenchSnapshot, WorkbenchError> {
        let path = path.canonicalize()?;
        let metadata = path.metadata()?;
        let server = self
            .state
            .read()
            .await
            .servers
            .iter()
            .find(|item| item.spec.id == server_id)
            .cloned()
            .ok_or_else(|| WorkbenchError::NotFound("server".into()))?;
        let model = self
            .state
            .read()
            .await
            .models
            .iter()
            .find(|item| item.id == server.spec.model_id)
            .cloned()
            .ok_or_else(|| WorkbenchError::NotFound("model".into()))?;
        let record = CheckpointRecord {
            id: Uuid::now_v7(),
            server_id,
            path,
            model_fingerprint: model.fingerprint,
            slot,
            token_count,
            size_bytes: metadata.len(),
            created_at: Utc::now(),
        };
        let id = record.id;
        self.state.write().await.checkpoints.push(record);
        self.record("checkpoints.added", json!({"checkpointId":id}))
            .await?;
        self.persist_and_snapshot().await
    }

    pub async fn add_mcp_server(
        &self,
        name: String,
        command: PathBuf,
        args: Vec<String>,
        secret_refs: BTreeMap<String, String>,
        decision: PolicyDecision,
    ) -> Result<WorkbenchSnapshot, WorkbenchError> {
        if secret_refs
            .values()
            .any(|value| !is_secret_reference(value))
        {
            return Err(WorkbenchError::Policy(
                "MCP environment values must be secret references".into(),
            ));
        }
        let item = McpServerRecord {
            id: Uuid::now_v7(),
            name,
            command,
            args,
            secret_refs,
            decision,
            enabled: decision != PolicyDecision::Deny,
        };
        let id = item.id;
        self.state.write().await.mcp_servers.push(item);
        self.record(
            "mcp.server-added",
            json!({"serverId":id,"decision":decision}),
        )
        .await?;
        self.persist_and_snapshot().await
    }

    pub async fn add_voice_service(
        &self,
        name: String,
        kind: String,
        executable: PathBuf,
        args: Vec<String>,
        endpoint: String,
    ) -> Result<WorkbenchSnapshot, WorkbenchError> {
        if !endpoint_is_loopback(&endpoint) {
            return Err(WorkbenchError::Policy(
                "Voice services must bind to loopback".into(),
            ));
        }
        let item = VoiceService {
            id: Uuid::now_v7(),
            name,
            kind,
            executable,
            args,
            endpoint,
            state: ResourceState::Stopped,
            pid: None,
        };
        let id = item.id;
        self.state.write().await.voice_services.push(item);
        self.record("voice.service-added", json!({"serviceId":id}))
            .await?;
        self.persist_and_snapshot().await
    }

    pub async fn start_voice_service(
        &self,
        service_id: Uuid,
    ) -> Result<WorkbenchSnapshot, WorkbenchError> {
        let service = self
            .state
            .read()
            .await
            .voice_services
            .iter()
            .find(|item| item.id == service_id)
            .cloned()
            .ok_or_else(|| WorkbenchError::NotFound("voice service".into()))?;
        let child = Command::new(&service.executable)
            .args(&service.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let pid = child.id();
        self.children.lock().await.insert(service_id, child);
        if let Some(item) = self
            .state
            .write()
            .await
            .voice_services
            .iter_mut()
            .find(|item| item.id == service_id)
        {
            item.state = ResourceState::Running;
            item.pid = pid;
        }
        self.record("voice.started", json!({"serviceId":service_id,"pid":pid}))
            .await?;
        self.persist_and_snapshot().await
    }

    pub async fn create_thread(
        &self,
        title: String,
        profile: Option<String>,
    ) -> Result<ChatThread, WorkbenchError> {
        let now = Utc::now();
        let thread = ChatThread {
            id: Uuid::now_v7(),
            title,
            folder: None,
            profile,
            root_message_id: None,
            messages: Vec::new(),
            created_at: now,
            updated_at: now,
        };
        self.state.write().await.chat_threads.push(thread.clone());
        self.record("chat.thread-created", json!({"threadId":thread.id}))
            .await?;
        self.persist().await?;
        Ok(thread)
    }

    pub async fn append_message(
        &self,
        thread_id: Uuid,
        parent_id: Option<Uuid>,
        role: String,
        content: String,
        run_id: Option<Uuid>,
        attachments: Vec<String>,
    ) -> Result<ChatThread, WorkbenchError> {
        if content.trim().is_empty() {
            return Err(WorkbenchError::Invalid(
                "Message content cannot be empty".into(),
            ));
        }
        let message = ChatMessage {
            id: Uuid::now_v7(),
            parent_id,
            role,
            content,
            created_at: Utc::now(),
            run_id,
            attachments,
        };
        let mut state = self.state.write().await;
        let thread = state
            .chat_threads
            .iter_mut()
            .find(|item| item.id == thread_id)
            .ok_or_else(|| WorkbenchError::NotFound("chat thread".into()))?;
        if let Some(parent) = parent_id
            && !thread.messages.iter().any(|item| item.id == parent)
        {
            return Err(WorkbenchError::NotFound("parent message".into()));
        }
        if thread.root_message_id.is_none() {
            thread.root_message_id = Some(message.id);
        }
        thread.updated_at = Utc::now();
        thread.messages.push(message.clone());
        let result = thread.clone();
        drop(state);
        self.record(
            "chat.message-appended",
            json!({"threadId":thread_id,"messageId":message.id,"role":message.role}),
        )
        .await?;
        self.persist().await?;
        Ok(result)
    }

    pub async fn notify(
        &self,
        level: String,
        title: String,
        detail: String,
    ) -> Result<WorkbenchSnapshot, WorkbenchError> {
        let item = NotificationRecord {
            id: Uuid::now_v7(),
            level,
            title,
            detail,
            read: false,
            created_at: Utc::now(),
        };
        self.state.write().await.notifications.push(item.clone());
        self.record("notifications.created", json!({"notificationId":item.id}))
            .await?;
        self.persist_and_snapshot().await
    }

    pub async fn search_workspace(
        &self,
        root: &Path,
        query: &str,
        limit: usize,
    ) -> Result<Value, WorkbenchError> {
        if query.trim().is_empty() {
            return Err(WorkbenchError::Invalid(
                "Search query cannot be empty".into(),
            ));
        }
        let root = root.canonicalize()?;
        let needle = query.to_ascii_lowercase();
        let results =
            tokio::task::spawn_blocking(move || search_files(&root, &needle, limit.min(200)))
                .await
                .map_err(|error| WorkbenchError::Task(error.to_string()))??;
        Ok(json!({"results":results}))
    }

    pub async fn code_graph(&self, root: &Path, limit: usize) -> Result<Value, WorkbenchError> {
        let root = root.canonicalize()?;
        let nodes = tokio::task::spawn_blocking(move || scan_symbols(&root, limit.min(2000)))
            .await
            .map_err(|error| WorkbenchError::Task(error.to_string()))??;
        Ok(json!({"nodes":nodes,"edges":[]}))
    }

    async fn record(&self, kind: &str, payload: Value) -> Result<WorkbenchEvent, WorkbenchError> {
        let sequence = {
            let mut state = self.state.write().await;
            state.revision += 1;
            state.revision
        };
        let event = WorkbenchEvent {
            sequence,
            id: Uuid::now_v7(),
            kind: kind.into(),
            occurred_at: Utc::now(),
            payload,
        };
        let mut bytes = serde_json::to_vec(&event)?;
        bytes.push(b'\n');
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join("events.jsonl"))
            .await?;
        file.write_all(&bytes).await?;
        file.flush().await?;
        Ok(event)
    }

    async fn persist_and_snapshot(&self) -> Result<WorkbenchSnapshot, WorkbenchError> {
        self.persist().await?;
        Ok(self.snapshot().await)
    }

    async fn persist(&self) -> Result<(), WorkbenchError> {
        let path = self.root.join("projection.json");
        let temp = self.root.join("projection.tmp");
        let bytes = serde_json::to_vec_pretty(&*self.state.read().await)?;
        tokio::fs::write(&temp, bytes).await?;
        tokio::fs::rename(temp, path).await?;
        Ok(())
    }
}

struct ModelGroup {
    files: Vec<PathBuf>,
    projector: Option<PathBuf>,
}

fn scan_gguf_files(root: &Path) -> Result<Vec<ModelGroup>, WorkbenchError> {
    let mut stack = vec![root.to_path_buf()];
    let mut model_files = Vec::new();
    let mut projectors = Vec::new();
    while let Some(directory) = stack.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            let path = entry.path();
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                if !matches!(
                    entry.file_name().to_str(),
                    Some(".git" | "node_modules" | "target")
                ) {
                    stack.push(path);
                }
            } else if path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("gguf"))
            {
                if path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.to_ascii_lowercase().contains("mmproj"))
                {
                    projectors.push(path);
                } else {
                    model_files.push(path);
                }
            }
        }
    }
    let mut groups: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for path in model_files {
        groups.entry(shard_key(&path)).or_default().push(path);
    }
    Ok(groups
        .into_values()
        .map(|mut files| {
            files.sort();
            let parent = files.first().and_then(|path| path.parent());
            let projector = projectors
                .iter()
                .find(|path| path.parent() == parent)
                .cloned();
            ModelGroup { files, projector }
        })
        .collect())
}

fn shard_key(path: &Path) -> String {
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let bytes = name.as_bytes();
    for index in 0..bytes.len().saturating_sub(14) {
        let segment = &name[index..];
        if segment.starts_with('-')
            && segment.get(6..10) == Some("-of-")
            && segment
                .get(1..6)
                .is_some_and(|value| value.chars().all(|c| c.is_ascii_digit()))
        {
            return path
                .with_file_name(format!(
                    "{}{}",
                    &name[..index],
                    segment.get(15..).unwrap_or("")
                ))
                .display()
                .to_string();
        }
    }
    path.display().to_string()
}

fn file_group_fingerprint(paths: &[PathBuf]) -> Result<String, WorkbenchError> {
    let mut hasher = blake3::Hasher::new();
    for path in paths {
        let metadata = path.metadata()?;
        hasher.update(path.to_string_lossy().as_bytes());
        hasher.update(&metadata.len().to_le_bytes());
        if let Ok(modified) = metadata.modified().and_then(|time| {
            time.duration_since(std::time::UNIX_EPOCH)
                .map_err(std::io::Error::other)
        }) {
            hasher.update(&modified.as_secs().to_le_bytes());
        }
    }
    Ok(hasher.finalize().to_hex().to_string())
}

fn gguf_version(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut header = [0u8; 8];
    std::io::Read::read_exact(&mut file, &mut header).ok()?;
    if &header[0..4] != b"GGUF" {
        return None;
    }
    Some(format!(
        "GGUF v{}",
        u32::from_le_bytes(header[4..8].try_into().ok()?)
    ))
}
fn display_model_name(name: &str) -> String {
    name.trim_end_matches(".gguf").replace(['_', '-'], " ")
}
fn infer_architecture(lower: &str) -> Option<String> {
    ["qwen", "llama", "mistral", "gemma", "phi", "deepseek"]
        .into_iter()
        .find(|value| lower.contains(value))
        .map(str::to_owned)
}
fn infer_quantization(name: &str) -> Option<String> {
    name.split(['-', '_', '.'])
        .find(|part| part.starts_with('Q') && part.chars().any(|c| c.is_ascii_digit()))
        .map(str::to_owned)
}
fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}
fn endpoint_is_loopback(endpoint: &str) -> bool {
    reqwest::Url::parse(endpoint)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .is_some_and(|host| is_loopback_host(&host))
}
fn is_secret_reference(value: &str) -> bool {
    [
        "keychain://",
        "secret-service://",
        "credential-manager://",
        "env-ref://",
    ]
    .iter()
    .any(|prefix| value.starts_with(prefix))
}
fn absolute_destination(path: &Path) -> Result<PathBuf, WorkbenchError> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        Err(WorkbenchError::Invalid(
            "Download destination must be absolute".into(),
        ))
    }
}
async fn is_writable_directory(path: &Path) -> bool {
    let probe = path.join(format!(".vector-write-probe-{}", Uuid::new_v4()));
    match tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .await
    {
        Ok(_) => {
            let _ = tokio::fs::remove_file(probe).await;
            true
        }
        Err(_) => false,
    }
}
async fn detect_backend_devices(executable: &Path) -> Vec<String> {
    Command::new(executable)
        .arg("--list-devices")
        .output()
        .await
        .ok()
        .filter(|output| output.status.success())
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_else(|| vec!["CPU".into()])
}
async fn memory_bytes() -> Option<u64> {
    if cfg!(target_os = "linux") {
        let text = tokio::fs::read_to_string("/proc/meminfo").await.ok()?;
        text.lines()
            .find(|line| line.starts_with("MemTotal:"))?
            .split_whitespace()
            .nth(1)?
            .parse::<u64>()
            .ok()
            .map(|kb| kb * 1024)
    } else if cfg!(target_os = "macos") {
        let output = Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .await
            .ok()?;
        String::from_utf8_lossy(&output.stdout).trim().parse().ok()
    } else {
        None
    }
}
async fn sha256_file(path: &Path) -> Result<String, WorkbenchError> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}
fn redact_output(value: &str) -> String {
    value
        .lines()
        .take(200)
        .map(|line| {
            if line.to_ascii_lowercase().contains("token=")
                || line.to_ascii_lowercase().contains("api_key=")
            {
                "[redacted]"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn walk_text_files(
    root: &Path,
    mut visit: impl FnMut(&Path, &str) -> bool,
) -> Result<(), WorkbenchError> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            let path = entry.path();
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if !matches!(
                    entry.file_name().to_str(),
                    Some(".git" | "node_modules" | "target" | "dist")
                ) {
                    stack.push(path);
                }
                continue;
            }
            if entry.metadata()?.len() > 2 * 1024 * 1024 {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&path)
                && !visit(&path, &text)
            {
                return Ok(());
            }
        }
    }
    Ok(())
}
fn search_files(root: &Path, needle: &str, limit: usize) -> Result<Vec<Value>, WorkbenchError> {
    let mut results = Vec::new();
    walk_text_files(root, |path, text| {
        for (index, line) in text.lines().enumerate() {
            if line.to_ascii_lowercase().contains(needle) {
                results.push(json!({"path":path,"line":index + 1,"preview":line.trim().chars().take(240).collect::<String>()}));
                if results.len() >= limit {
                    return false;
                }
            }
        }
        true
    })?;
    Ok(results)
}
fn scan_symbols(root: &Path, limit: usize) -> Result<Vec<Value>, WorkbenchError> {
    let mut nodes = Vec::new();
    walk_text_files(root, |path, text| {
        for (index, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            let kind = if trimmed.starts_with("fn ")
                || trimmed.starts_with("pub fn ")
                || trimmed.starts_with("def ")
                || trimmed.starts_with("function ")
            {
                Some("function")
            } else if trimmed.starts_with("struct ")
                || trimmed.starts_with("pub struct ")
                || trimmed.starts_with("class ")
            {
                Some("type")
            } else {
                None
            };
            if let Some(kind) = kind {
                nodes.push(json!({"id":Uuid::now_v7(),"kind":kind,"path":path,"line":index + 1,"symbol":trimmed.split_whitespace().nth(1).unwrap_or("unknown").trim_matches(|c: char| !c.is_alphanumeric() && c != '_')}));
                if nodes.len() >= limit {
                    return false;
                }
            }
        }
        true
    })?;
    Ok(nodes)
}

#[derive(Debug, Error)]
pub enum WorkbenchError {
    #[error("VCTR_WORKBENCH_IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("VCTR_WORKBENCH_INVALID: {0}")]
    Invalid(String),
    #[error("VCTR_WORKBENCH_NOT_FOUND: {0} was not found")]
    NotFound(String),
    #[error("VCTR_POLICY_DENIED: {0}")]
    Policy(String),
    #[error("VCTR_DOWNLOAD_FAILED: request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("VCTR_DOWNLOAD_FAILED: downloaded content did not match its checksum")]
    Checksum,
    #[error("VCTR_WORKBENCH_INVALID: serialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("VCTR_RUNTIME_UNAVAILABLE: background task failed: {0}")]
    Task(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn model_scan_groups_shards_and_projector() {
        let root = tempdir().unwrap();
        let models = root.path().join("models");
        std::fs::create_dir(&models).unwrap();
        for name in [
            "qwen-Q4-00001-of-00002.gguf",
            "qwen-Q4-00002-of-00002.gguf",
            "mmproj-qwen.gguf",
        ] {
            let mut bytes = b"GGUF".to_vec();
            bytes.extend(3u32.to_le_bytes());
            bytes.extend([0u8; 8]);
            std::fs::write(models.join(name), bytes).unwrap();
        }
        let workbench = Workbench::open(root.path()).await.unwrap();
        let snapshot = workbench.add_model_folder(&models).await.unwrap();
        assert_eq!(snapshot.models.len(), 1);
        assert_eq!(snapshot.models[0].shard_paths.len(), 1);
        assert!(snapshot.models[0].projector_path.is_some());
    }

    #[tokio::test]
    async fn raw_mcp_secrets_fail_closed() {
        let root = tempdir().unwrap();
        let workbench = Workbench::open(root.path()).await.unwrap();
        let result = workbench
            .add_mcp_server(
                "bad".into(),
                "/bin/false".into(),
                vec![],
                BTreeMap::from([("TOKEN".into(), "literal".into())]),
                PolicyDecision::Prompt,
            )
            .await;
        assert!(matches!(result, Err(WorkbenchError::Policy(_))));
    }
}
