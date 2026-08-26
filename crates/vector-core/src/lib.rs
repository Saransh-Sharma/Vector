//! Stable public domain types for the Vector control plane.

use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use directories::BaseDirs;
use schemars::{JsonSchema, schema_for};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

pub const API_VERSION: &str = "vector.dev/v1alpha1";
pub const PROTOCOL_VERSION: &str = "1.0";

#[derive(Debug, Clone)]
pub struct ApplicationPaths {
    pub config_dir: PathBuf,
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
}

/// Returns the public, user-facing Vector directories without reverse-DNS nesting.
pub fn application_paths() -> Option<ApplicationPaths> {
    let base = BaseDirs::new()?;
    #[cfg(target_os = "macos")]
    return Some(ApplicationPaths {
        config_dir: base.home_dir().join("Library/Application Support/Vector"),
        data_dir: base.home_dir().join("Library/Application Support/Vector"),
        cache_dir: base.home_dir().join("Library/Caches/Vector"),
    });
    #[cfg(target_os = "linux")]
    return Some(ApplicationPaths {
        config_dir: base.config_dir().join("Vector"),
        data_dir: base.data_local_dir().join("Vector"),
        cache_dir: base.cache_dir().join("Vector"),
    });
    #[cfg(target_os = "windows")]
    return Some(ApplicationPaths {
        config_dir: base.config_dir().join("Vector"),
        data_dir: base.data_local_dir().join("Vector"),
        cache_dir: base.cache_dir().join("Vector"),
    });
    #[allow(unreachable_code)]
    None
}

pub mod capabilities {
    pub const FILESYSTEM_READ: &str = "filesystem.read";
    pub const FILESYSTEM_WORKSPACE_WRITE: &str = "filesystem.workspace-write";
    pub const FILESYSTEM_EXTERNAL_WRITE: &str = "filesystem.external-write";
    pub const FILESYSTEM_DELETE: &str = "filesystem.delete";
    pub const PROCESS_EXECUTE: &str = "process.execute";
    pub const PROCESS_BACKGROUND: &str = "process.background";
    pub const PROCESS_PTY: &str = "process.pty";
    pub const NETWORK_READ: &str = "network.read";
    pub const NETWORK_WRITE: &str = "network.write";
    pub const NETWORK_LISTEN: &str = "network.listen";
    pub const NETWORK_UNRESTRICTED: &str = "network.unrestricted";
    pub const GIT_READ: &str = "git.read";
    pub const GIT_WRITE: &str = "git.write";
    pub const GIT_COMMIT: &str = "git.commit";
    pub const GIT_PUSH: &str = "git.push";
    pub const GIT_FORCE_PUSH: &str = "git.force-push";
    pub const CODE_SEARCH: &str = "code.search";
    pub const CODE_LSP: &str = "code.lsp";
    pub const CODE_DEBUGGER: &str = "code.debugger";
    pub const AGENTS_SPAWN: &str = "agents.spawn";
    pub const AGENTS_PARALLEL: &str = "agents.parallel";
    pub const SESSION_RESUME: &str = "session.resume";
    pub const SESSION_EXPORT: &str = "session.export";
    pub const WEB_SEARCH: &str = "web.search";
    pub const WEB_FETCH: &str = "web.fetch";
    pub const BROWSER_INSPECT: &str = "browser.inspect";
    pub const BROWSER_CONTROL: &str = "browser.control";
    pub const COMPUTER_INSPECT: &str = "computer.inspect";
    pub const COMPUTER_CONTROL: &str = "computer.control";
    pub const CLIPBOARD_READ: &str = "clipboard.read";
    pub const CLIPBOARD_WRITE: &str = "clipboard.write";
    pub const MCP_READ: &str = "mcp.read";
    pub const MCP_WRITE: &str = "mcp.write";
    pub const SECRETS_USE: &str = "secrets.use";
    pub const SECRETS_MATERIALIZE_ENV: &str = "secrets.materialize-env";
    pub const SECRETS_MATERIALIZE_FILE: &str = "secrets.materialize-file";
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct Capability(pub String);

impl Capability {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl From<&str> for Capability {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl Display for Capability {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum PolicyDecision {
    Allow,
    Prompt,
    Deny,
}

impl Display for PolicyDecision {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Allow => f.write_str("allow"),
            Self::Prompt => f.write_str("prompt"),
            Self::Deny => f.write_str("deny"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum HarnessId {
    Omp,
    Pi,
    Deepseek,
}

impl Default for HarnessId {
    fn default() -> Self {
        Self::Pi
    }
}

impl Display for HarnessId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Omp => "omp",
            Self::Pi => "pi",
            Self::Deepseek => "deepseek",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CompatibilityState {
    ManagedTested,
    ManagedExperimental,
    ExternalCompatible,
    ExternalUnverified,
    ExternalIncompatible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "kebab-case")]
pub enum LaunchSurface {
    #[default]
    Integrated,
    Native,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum InstallationSource {
    Managed,
    External,
    Missing,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarnessInstallationRecord {
    pub harness: HarnessId,
    pub source: InstallationSource,
    #[serde(default)]
    pub executable: Option<PathBuf>,
    #[serde(default)]
    pub runtime_executable: Option<PathBuf>,
    #[serde(default)]
    pub package_root: Option<PathBuf>,
    #[serde(default)]
    pub package: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub integrity: Option<String>,
    pub runtime: String,
    pub compatibility: CompatibilityState,
    pub ready: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PreflightCheck {
    pub id: String,
    pub label: String,
    pub passed: bool,
    pub detail: String,
    #[serde(default)]
    pub remediation: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PreflightReport {
    pub workspace: PathBuf,
    pub profile: String,
    pub harness: HarnessInstallationRecord,
    pub checks: Vec<PreflightCheck>,
    pub ready_for_smoke: bool,
    #[serde(default)]
    pub smoke_passed: bool,
    #[serde(default)]
    pub ready_to_work: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SmokeTestReport {
    pub passed: bool,
    pub harness: HarnessId,
    pub profile: String,
    pub fixture_digest_before: String,
    pub fixture_digest_after: String,
    pub model_streamed: bool,
    pub tool_observed: bool,
    pub policy_denial_observed: bool,
    pub events: Vec<String>,
    #[serde(default)]
    pub diagnostic: Option<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ComputerUseVerificationRequest {
    pub workspace: PathBuf,
    pub profile: String,
    pub vision_model: String,
    #[serde(default)]
    pub request_permissions: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ComputerUseVerificationReport {
    pub vision_model: String,
    pub vision_probe_passed: bool,
    pub screen_recording: bool,
    pub accessibility: bool,
    pub fixture_identified: bool,
    pub fixture_clicked: bool,
    #[serde(default)]
    pub screenshot_path: Option<PathBuf>,
    pub enabled: bool,
    pub checks: Vec<PreflightCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunStartRequest {
    pub workspace: PathBuf,
    pub profile: String,
    pub surface: LaunchSurface,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub grant_yolo: bool,
    #[serde(default)]
    pub granted_by: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SessionPhase {
    Preparing,
    Ready,
    Streaming,
    WaitingForApproval,
    Stopping,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InteractiveSessionState {
    pub run_id: Uuid,
    pub harness: HarnessId,
    pub surface: LaunchSurface,
    pub phase: SessionPhase,
    #[serde(default)]
    pub native_session_id: Option<String>,
    pub next_sequence: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionEvent {
    pub run_id: Uuid,
    pub sequence: u64,
    pub kind: String,
    pub occurred_at: DateTime<Utc>,
    #[serde(default)]
    pub payload: Value,
    #[serde(default)]
    pub native_ref: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SupportLevel {
    Native,
    Adapter,
    Experimental,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum EnforcementLevel {
    Hard,
    Harness,
    Advisory,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CapabilitySupport {
    pub support: SupportLevel,
    pub enforcement: EnforcementLevel,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RepositoryTrust {
    #[default]
    Untrusted,
    ConfigOnly,
    Executable,
    PublisherTrusted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "kebab-case")]
pub enum IsolationKind {
    #[default]
    Host,
    WorkspaceSandbox,
    Container,
    Vm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "kebab-case")]
pub enum NetworkMode {
    Off,
    #[default]
    Loopback,
    ProviderOnly,
    Allowlist,
    OnDemand,
    Unrestricted,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct IsolationConfig {
    #[serde(default)]
    pub kind: IsolationKind,
    #[serde(default)]
    pub network: NetworkMode,
    #[serde(default)]
    pub network_allowlist: Vec<String>,
    #[serde(default)]
    pub mounts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderConfig {
    pub kind: String,
    pub base_url: String,
    #[serde(default)]
    pub secret_ref: Option<String>,
    #[serde(default = "default_true")]
    pub local: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelRoles {
    pub primary: String,
    #[serde(default)]
    pub fast: Option<String>,
    #[serde(default)]
    pub planner: Option<String>,
    #[serde(default)]
    pub reviewer: Option<String>,
    #[serde(default)]
    pub vision: Option<String>,
    #[serde(default)]
    pub compactor: Option<String>,
    #[serde(default)]
    pub embeddings: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct VerificationConfig {
    #[serde(default)]
    pub quick: Vec<Vec<String>>,
    #[serde(default)]
    pub after_change: Vec<Vec<String>>,
    #[serde(default)]
    pub before_complete: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PackRef {
    pub id: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub digest: Option<String>,
    #[serde(default)]
    pub lazy: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct ContextConfig {
    #[serde(default)]
    pub max_tokens: Option<u64>,
    #[serde(default = "default_true")]
    pub stable_prefix: bool,
    #[serde(default = "default_true")]
    pub lazy_resources: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub harness: HarnessId,
    pub provider: String,
    pub models: ModelRoles,
    #[serde(default)]
    pub policy: BTreeMap<Capability, PolicyDecision>,
    #[serde(default)]
    pub isolation: IsolationConfig,
    #[serde(default)]
    pub packs: Vec<PackRef>,
    #[serde(default)]
    pub verification: VerificationConfig,
    #[serde(default)]
    pub context: ContextConfig,
    #[serde(default)]
    pub native: Value,
    #[serde(default)]
    pub yolo: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VectorWorkspace {
    pub api_version: String,
    pub kind: String,
    #[serde(default)]
    pub trust: RepositoryTrust,
    #[serde(default)]
    pub default_profile: Option<String>,
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderConfig>,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
}

impl Default for VectorWorkspace {
    fn default() -> Self {
        Self {
            api_version: API_VERSION.to_string(),
            kind: "VectorWorkspace".to_string(),
            trust: RepositoryTrust::Untrusted,
            default_profile: None,
            providers: BTreeMap::new(),
            profiles: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceIdentity {
    pub root: PathBuf,
    #[serde(default)]
    pub git_commit: Option<String>,
    pub git_dirty: bool,
    pub trust: RepositoryTrust,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HarnessInstallation {
    pub harness: HarnessId,
    pub adapter_version: String,
    pub runtime: String,
    pub package: String,
    pub version: String,
    pub compatibility: CompatibilityState,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedProvider {
    pub id: String,
    pub kind: String,
    pub base_url: String,
    #[serde(default)]
    pub secret_ref: Option<String>,
    pub local: bool,
    #[serde(default)]
    pub service_fingerprint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ResourceState {
    Ready,
    Queued,
    Running,
    Stopped,
    Failed,
    Interrupted,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelFolder {
    pub id: Uuid,
    pub path: PathBuf,
    pub writable: bool,
    pub added_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ManagedModel {
    pub id: Uuid,
    pub folder_id: Uuid,
    pub path: PathBuf,
    pub display_name: String,
    pub size_bytes: u64,
    pub fingerprint: String,
    pub format: String,
    #[serde(default)]
    pub architecture: Option<String>,
    #[serde(default)]
    pub quantization: Option<String>,
    #[serde(default)]
    pub shard_paths: Vec<PathBuf>,
    #[serde(default)]
    pub projector_path: Option<PathBuf>,
    pub embedding: bool,
    pub vision: bool,
    pub indexed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BackendRecord {
    pub id: Uuid,
    pub name: String,
    pub executable: PathBuf,
    pub kind: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub devices: Vec<String>,
    pub fingerprint: String,
    pub verified: bool,
    pub added_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BackendGroup {
    pub id: Uuid,
    pub name: String,
    pub backend_ids: Vec<Uuid>,
    #[serde(default)]
    pub active_backend_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModelServerSpec {
    pub id: Uuid,
    pub name: String,
    pub backend_id: Uuid,
    pub model_id: Uuid,
    pub host: String,
    pub port: u16,
    pub context_size: u64,
    pub gpu_layers: i32,
    #[serde(default)]
    pub tensor_split: Vec<f32>,
    #[serde(default)]
    pub extra_args: Vec<String>,
    pub embedding: bool,
    #[serde(default)]
    pub speculative_model_id: Option<Uuid>,
    pub auto_start: bool,
    pub fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ManagedServer {
    pub spec: ModelServerSpec,
    pub state: ResourceState,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub started_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DownloadJob {
    pub id: Uuid,
    pub url: String,
    pub destination: PathBuf,
    pub state: ResourceState,
    pub received_bytes: u64,
    #[serde(default)]
    pub total_bytes: Option<u64>,
    #[serde(default)]
    pub expected_sha256: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RecipeSpec {
    pub id: Uuid,
    pub name: String,
    pub executable: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub working_directory: Option<PathBuf>,
    pub digest: String,
    pub trusted: bool,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProxyAlias {
    pub id: Uuid,
    pub alias: String,
    pub server_id: Uuid,
    pub model_name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointRecord {
    pub id: Uuid,
    pub server_id: Uuid,
    pub path: PathBuf,
    pub model_fingerprint: String,
    pub slot: u32,
    pub token_count: u64,
    pub size_bytes: u64,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct McpServerRecord {
    pub id: Uuid,
    pub name: String,
    pub command: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub secret_refs: BTreeMap<String, String>,
    pub decision: PolicyDecision,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VoiceService {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub executable: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    pub endpoint: String,
    pub state: ResourceState,
    #[serde(default)]
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub id: Uuid,
    #[serde(default)]
    pub parent_id: Option<Uuid>,
    pub role: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub run_id: Option<Uuid>,
    #[serde(default)]
    pub attachments: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ChatThread {
    pub id: Uuid,
    pub title: String,
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub root_message_id: Option<Uuid>,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NotificationRecord {
    pub id: Uuid,
    pub level: String,
    pub title: String,
    pub detail: String,
    pub read: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HardwareSnapshot {
    pub os: String,
    pub architecture: String,
    pub logical_cpus: usize,
    #[serde(default)]
    pub memory_bytes: Option<u64>,
    #[serde(default)]
    pub devices: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkbenchSnapshot {
    #[serde(default)]
    pub model_folders: Vec<ModelFolder>,
    #[serde(default)]
    pub models: Vec<ManagedModel>,
    #[serde(default)]
    pub backends: Vec<BackendRecord>,
    #[serde(default)]
    pub backend_groups: Vec<BackendGroup>,
    #[serde(default)]
    pub servers: Vec<ManagedServer>,
    #[serde(default)]
    pub downloads: Vec<DownloadJob>,
    #[serde(default)]
    pub recipes: Vec<RecipeSpec>,
    #[serde(default)]
    pub proxy_aliases: Vec<ProxyAlias>,
    #[serde(default)]
    pub checkpoints: Vec<CheckpointRecord>,
    #[serde(default)]
    pub mcp_servers: Vec<McpServerRecord>,
    #[serde(default)]
    pub voice_services: Vec<VoiceService>,
    #[serde(default)]
    pub chat_threads: Vec<ChatThread>,
    #[serde(default)]
    pub notifications: Vec<NotificationRecord>,
    #[serde(default)]
    pub revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkbenchEvent {
    pub sequence: u64,
    pub id: Uuid,
    pub kind: String,
    pub occurred_at: DateTime<Utc>,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PolicySource {
    pub layer: String,
    pub decision: PolicyDecision,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveCapability {
    pub requested: PolicyDecision,
    pub effective: PolicyDecision,
    pub provenance: Vec<PolicySource>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RunGrant {
    pub yolo: bool,
    #[serde(default)]
    pub granted_by: Option<String>,
    #[serde(default)]
    pub granted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PortableRunSpec {
    pub api_version: String,
    pub run_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub profile: String,
    pub workspace: WorkspaceIdentity,
    pub harness: HarnessInstallation,
    pub provider: ResolvedProvider,
    pub models: ModelRoles,
    pub capabilities: BTreeMap<Capability, EffectiveCapability>,
    pub isolation: IsolationConfig,
    pub packs: Vec<PackRef>,
    pub verification: VerificationConfig,
    pub context: ContextConfig,
    pub native: Value,
    pub grant: RunGrant,
    pub provenance: BTreeMap<String, String>,
}

impl PortableRunSpec {
    pub fn canonical_json(&self) -> Result<Vec<u8>, CoreError> {
        Ok(serde_json::to_vec(self)?)
    }

    pub fn fingerprint(&self) -> Result<String, CoreError> {
        let mut stable = serde_json::to_value(self)?;
        if let Value::Object(root) = &mut stable {
            root.remove("runId");
            root.remove("createdAt");
            if let Some(Value::Object(grant)) = root.get_mut("grant") {
                grant.remove("grantedBy");
                grant.remove("grantedAt");
            }
        }
        Ok(blake3::hash(&serde_json::to_vec(&stable)?)
            .to_hex()
            .to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct VectorLock {
    pub lock_version: u32,
    pub generated_at: DateTime<Utc>,
    pub profile_fingerprints: BTreeMap<String, String>,
    pub harnesses: BTreeMap<String, String>,
    pub packs: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RequestEnvelope {
    pub protocol_version: String,
    pub request_id: Uuid,
    pub idempotency_key: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
    #[serde(default)]
    pub auth: Option<String>,
    #[serde(default)]
    pub confirmation_token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResponseEnvelope {
    pub protocol_version: String,
    pub request_id: Uuid,
    pub ok: bool,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub diagnostic: Option<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
    Fatal,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub code: String,
    pub severity: Severity,
    pub summary: String,
    pub detail: String,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub remediation: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

impl Diagnostic {
    pub fn error(
        code: impl Into<String>,
        summary: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            severity: Severity::Error,
            summary: summary.into(),
            detail: detail.into(),
            source: None,
            remediation: None,
            metadata: BTreeMap::new(),
        }
    }
}

pub fn workspace_schema() -> Value {
    serde_json::to_value(schema_for!(VectorWorkspace)).expect("schema serializes")
}

pub fn runspec_schema() -> Value {
    serde_json::to_value(schema_for!(PortableRunSpec)).expect("schema serializes")
}

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restriction_order_is_monotonic() {
        assert!(PolicyDecision::Deny > PolicyDecision::Prompt);
        assert!(PolicyDecision::Prompt > PolicyDecision::Allow);
    }

    #[test]
    fn workspace_schema_uses_public_version() {
        let schema = workspace_schema();
        assert!(schema.get("$schema").is_some());
        assert_eq!(API_VERSION, "vector.dev/v1alpha1");
    }

    #[test]
    fn fingerprint_excludes_volatile_run_identity() {
        let mut first = PortableRunSpec {
            api_version: API_VERSION.into(),
            run_id: Uuid::now_v7(),
            created_at: Utc::now(),
            profile: "safe".into(),
            workspace: WorkspaceIdentity {
                root: "/tmp/vector".into(),
                git_commit: None,
                git_dirty: false,
                trust: RepositoryTrust::Executable,
            },
            harness: HarnessInstallation {
                harness: HarnessId::Pi,
                adapter_version: "1".into(),
                runtime: "node".into(),
                package: "pi".into(),
                version: "1".into(),
                compatibility: CompatibilityState::ManagedTested,
            },
            provider: ResolvedProvider {
                id: "local".into(),
                kind: "lm-studio".into(),
                base_url: "http://localhost/v1".into(),
                secret_ref: None,
                local: true,
                service_fingerprint: None,
            },
            models: ModelRoles {
                primary: "qwen".into(),
                ..Default::default()
            },
            capabilities: BTreeMap::new(),
            isolation: IsolationConfig::default(),
            packs: vec![],
            verification: VerificationConfig::default(),
            context: ContextConfig::default(),
            native: Value::Null,
            grant: RunGrant {
                yolo: false,
                granted_by: None,
                granted_at: None,
            },
            provenance: BTreeMap::new(),
        };
        let expected = first.fingerprint().unwrap();
        first.run_id = Uuid::now_v7();
        first.created_at = Utc::now();
        assert_eq!(expected, first.fingerprint().unwrap());
    }
}
