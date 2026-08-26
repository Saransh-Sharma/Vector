//! Harness adapters compile a portable spec into an inspectable native plan.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
    process::Stdio,
};

use async_trait::async_trait;
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tar::Archive;
use thiserror::Error;
use tokio::process::Command;
use vector_core::capabilities::*;
use vector_core::*;

pub const OMP_PACKAGE: &str = "@oh-my-pi/pi-coding-agent";
pub const OMP_VERSION: &str = "18.0.4";
pub const OMP_INTEGRITY: &str = "sha512-vi2vZGsZ/OigD3f8M+Qixreuk7afU5P6Qe2JlcW6nTWOC48zYXeY4QZGPsM3R0Ata4xPGNWDtCKCgKjx6KO00A==";
pub const PI_PACKAGE: &str = "@earendil-works/pi-coding-agent";
pub const PI_VERSION: &str = "0.84.3";
pub const PI_INTEGRITY: &str = "sha512-Yr2p9PubrbFZmYEPYI+C8KmZP9xlFuLDnAG64RtU0ZDgrdiXYWa+y7WGyJO5OlqPliOkVCMd9IzVszO3/t0D0w==";
pub const BUN_VERSION: &str = "1.3.14";
pub const NODE_VERSION: &str = "22.19.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedFile {
    pub relative_path: String,
    pub content: String,
    pub sensitive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
pub enum EnvironmentValue {
    Literal(String),
    SecretReference(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeRunPlan {
    pub harness: HarnessId,
    pub executable: String,
    pub argv: Vec<String>,
    pub environment: BTreeMap<String, EnvironmentValue>,
    pub cwd: String,
    pub generated_files: Vec<GeneratedFile>,
    pub observation: String,
    pub cleanup: Vec<String>,
    pub native_summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorReport {
    pub harness: HarnessId,
    pub executable: String,
    pub detected: bool,
    pub compatibility: CompatibilityState,
    pub notes: Vec<String>,
}

#[async_trait]
pub trait HarnessAdapter: Send + Sync {
    fn id(&self) -> HarnessId;
    fn package_pin(&self) -> &'static str;
    fn capabilities(&self) -> BTreeMap<Capability, CapabilitySupport>;
    fn compile(&self, spec: &PortableRunSpec) -> Result<NativeRunPlan, HarnessError>;

    async fn doctor(&self) -> DoctorReport {
        let record = inspect_harness(self.id()).await;
        DoctorReport {
            harness: self.id(),
            detected: record.source != InstallationSource::Missing,
            executable: record
                .executable
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| self.executable().to_string()),
            compatibility: record.compatibility,
            notes: record.notes,
        }
    }

    fn executable(&self) -> &'static str;
}

pub fn adapter(harness: HarnessId) -> Box<dyn HarnessAdapter> {
    match harness {
        HarnessId::Omp => Box::new(OmpAdapter),
        HarnessId::Pi => Box::new(PiAdapter),
        HarnessId::Deepseek => Box::new(DeepseekAdapter),
    }
}

pub fn compile_for_surface(
    spec: &PortableRunSpec,
    surface: LaunchSurface,
) -> Result<NativeRunPlan, HarnessError> {
    let mut plan = adapter(spec.harness.harness).compile(spec)?;
    if surface == LaunchSurface::Native {
        match spec.harness.harness {
            HarnessId::Omp => {
                plan.argv = vec![
                    "launch".into(),
                    "--config".into(),
                    "${VECTOR_OVERLAY}/omp/config.yml".into(),
                    "--cwd".into(),
                    spec.workspace.root.display().to_string(),
                    "--model".into(),
                    format!("lm-studio/{}", spec.models.primary),
                    "--session-dir".into(),
                    "${VECTOR_RUN_DIR}/native/omp-sessions".into(),
                ];
                if spec.grant.yolo {
                    plan.argv.push("--yolo".into());
                } else {
                    plan.argv
                        .extend(["--approval-mode".into(), "always-ask".into()]);
                }
                plan.observation = "External OMP terminal; Vector records process and native session metadata only".into();
                plan.native_summary = format!(
                    "OMP native terminal {} with {}",
                    if spec.grant.yolo { "YOLO" } else { "guarded" },
                    spec.models.primary
                );
            }
            HarnessId::Pi => {
                if plan.argv.get(0).map(String::as_str) == Some("--mode") {
                    plan.argv.drain(0..2);
                }
                plan.observation =
                    "External Pi terminal; Vector records process and native session metadata only"
                        .into();
                plan.native_summary = format!(
                    "Pi native terminal {} through vector-local/{}",
                    if spec.grant.yolo { "YOLO" } else { "guarded" },
                    spec.models.primary
                );
            }
            HarnessId::Deepseek => {}
        }
    }
    Ok(plan)
}

pub struct OmpAdapter;

#[async_trait]
impl HarnessAdapter for OmpAdapter {
    fn id(&self) -> HarnessId {
        HarnessId::Omp
    }
    fn package_pin(&self) -> &'static str {
        "@oh-my-pi/pi-coding-agent@18.0.4"
    }
    fn executable(&self) -> &'static str {
        "omp"
    }

    fn capabilities(&self) -> BTreeMap<Capability, CapabilitySupport> {
        matrix(&[
            (
                FILESYSTEM_READ,
                SupportLevel::Native,
                EnforcementLevel::Harness,
            ),
            (
                FILESYSTEM_WORKSPACE_WRITE,
                SupportLevel::Native,
                EnforcementLevel::Harness,
            ),
            (
                PROCESS_EXECUTE,
                SupportLevel::Native,
                EnforcementLevel::Harness,
            ),
            (PROCESS_PTY, SupportLevel::Native, EnforcementLevel::Harness),
            (CODE_SEARCH, SupportLevel::Native, EnforcementLevel::Harness),
            (CODE_LSP, SupportLevel::Native, EnforcementLevel::Harness),
            (
                CODE_DEBUGGER,
                SupportLevel::Native,
                EnforcementLevel::Harness,
            ),
            (
                AGENTS_SPAWN,
                SupportLevel::Native,
                EnforcementLevel::Harness,
            ),
            (
                BROWSER_CONTROL,
                SupportLevel::Native,
                EnforcementLevel::Harness,
            ),
            (
                COMPUTER_INSPECT,
                SupportLevel::Native,
                EnforcementLevel::Harness,
            ),
            (
                COMPUTER_CONTROL,
                SupportLevel::Native,
                EnforcementLevel::Harness,
            ),
        ])
    }

    fn compile(&self, spec: &PortableRunSpec) -> Result<NativeRunPlan, HarnessError> {
        ensure_harness(spec, self.id())?;
        validate_supported(spec, &self.capabilities())?;
        let approval = tool_policy(spec);
        let config = serde_yaml::to_string(&json!({
            "tools": {
                "approvalMode": if spec.grant.yolo { "yolo" } else { "always-ask" },
            },
            "skills": { "enableSkillCommands": false },
            "extensions": [],
        }))?;
        let models = serde_yaml::to_string(&json!({
            "providers": {
                "lm-studio": {
                    "baseUrl": spec.provider.base_url,
                    "api": "openai-completions",
                    "auth": "none",
                    "models": [{
                        "id": spec.models.primary,
                        "name": spec.models.primary,
                        "reasoning": true,
                        "input": if spec.models.vision.is_some() { vec!["text", "image"] } else { vec!["text"] },
                        "supportsTools": true,
                        "contextWindow": spec.context.max_tokens.unwrap_or(32_768),
                        "maxTokens": 8_192,
                    }]
                }
            }
        }))?;
        // OMP's ACP entry point accepts no launch flags. Model selection and
        // approvals are negotiated over ACP; the overlay remains ledgered as
        // the auditable policy input for that negotiation.
        let argv = vec!["acp".into()];
        Ok(NativeRunPlan {
            harness: self.id(),
            executable: self.executable().into(),
            argv,
            environment: BTreeMap::from([
                (
                    "LM_STUDIO_BASE_URL".into(),
                    EnvironmentValue::Literal(spec.provider.base_url.clone()),
                ),
                (
                    "PI_CODING_AGENT_DIR".into(),
                    EnvironmentValue::Literal("${VECTOR_OVERLAY}/omp/agent".into()),
                ),
                (
                    "PI_CONFIG_FILES".into(),
                    EnvironmentValue::Literal("${VECTOR_OVERLAY}/omp/config.yml".into()),
                ),
                (
                    "OTEL_SDK_DISABLED".into(),
                    EnvironmentValue::Literal("true".into()),
                ),
            ]),
            cwd: spec.workspace.root.display().to_string(),
            generated_files: vec![
                GeneratedFile {
                    relative_path: "omp/config.yml".into(),
                    content: config,
                    sensitive: false,
                },
                GeneratedFile {
                    relative_path: "omp/agent/models.yml".into(),
                    content: models,
                    sensitive: false,
                },
                GeneratedFile {
                    relative_path: "omp/vector-policy.json".into(),
                    content: serde_json::to_string_pretty(&json!({
                        "runId": spec.run_id,
                        "runSpecFingerprint": spec.fingerprint()?,
                        "capabilities": approval,
                    }))?,
                    sensitive: false,
                },
            ],
            observation: "ACP structured events and permission requests".into(),
            cleanup: vec!["remove run-scoped overlay".into()],
            native_summary: format!(
                "OMP ACP {} with {} ({})",
                if spec.grant.yolo { "YOLO" } else { "guarded" },
                spec.models.primary,
                spec.provider.base_url
            ),
        })
    }
}

pub struct PiAdapter;

#[async_trait]
impl HarnessAdapter for PiAdapter {
    fn id(&self) -> HarnessId {
        HarnessId::Pi
    }
    fn package_pin(&self) -> &'static str {
        "@earendil-works/pi-coding-agent@0.84.3"
    }
    fn executable(&self) -> &'static str {
        "pi"
    }

    fn capabilities(&self) -> BTreeMap<Capability, CapabilitySupport> {
        matrix(&[
            (
                FILESYSTEM_READ,
                SupportLevel::Native,
                EnforcementLevel::Harness,
            ),
            (
                FILESYSTEM_WORKSPACE_WRITE,
                SupportLevel::Native,
                EnforcementLevel::Harness,
            ),
            (
                PROCESS_EXECUTE,
                SupportLevel::Native,
                EnforcementLevel::Harness,
            ),
            (PROCESS_PTY, SupportLevel::Native, EnforcementLevel::Harness),
            (
                CODE_SEARCH,
                SupportLevel::Adapter,
                EnforcementLevel::Harness,
            ),
            (CODE_LSP, SupportLevel::Adapter, EnforcementLevel::Harness),
            (
                AGENTS_SPAWN,
                SupportLevel::Adapter,
                EnforcementLevel::Harness,
            ),
            (
                COMPUTER_INSPECT,
                SupportLevel::Experimental,
                EnforcementLevel::Harness,
            ),
            (
                COMPUTER_CONTROL,
                SupportLevel::Experimental,
                EnforcementLevel::Harness,
            ),
        ])
    }

    fn compile(&self, spec: &PortableRunSpec) -> Result<NativeRunPlan, HarnessError> {
        ensure_harness(spec, self.id())?;
        validate_supported(spec, &self.capabilities())?;
        let input = if spec.models.vision.is_some() {
            "['text', 'image']"
        } else {
            "['text']"
        };
        let provider_extension = format!(
            r#"import type {{ ExtensionAPI }} from "@earendil-works/pi-coding-agent";

export default function vectorProvider(pi: ExtensionAPI) {{
  pi.registerProvider("vector-local", {{
    name: "Vector Local",
    baseUrl: {base_url:?},
    api: "openai-completions",
    apiKey: "vector-local",
    models: [{{
      id: {model:?}, name: {model:?}, input: {input}, reasoning: true,
      cost: {{ input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }},
      contextWindow: 32768, maxTokens: 8192
    }}]
  }});
}}
"#,
            base_url = spec.provider.base_url,
            model = spec.models.primary,
            input = input
        );
        let policy_document = serde_json::to_string_pretty(&spec.capabilities)?;
        let policy_extension = include_str!("../../../extensions/pi-vector-policy/index.ts")
            .replace(
                "__VECTOR_POLICY_DOCUMENT__",
                &serde_json::to_string(&policy_document)?,
            )
            .replace("__VECTOR_RUN_ID__", &spec.run_id.to_string())
            .replace(
                "__VECTOR_YOLO__",
                if spec.grant.yolo { "true" } else { "false" },
            );
        let argv = vec![
            "--mode".into(),
            "rpc".into(),
            "--provider".into(),
            "vector-local".into(),
            "--model".into(),
            spec.models.primary.clone(),
            "--api-key".into(),
            "vector-local".into(),
            "--session-dir".into(),
            "${VECTOR_RUN_DIR}/native/pi-sessions".into(),
            "--extension".into(),
            "${VECTOR_OVERLAY}/pi/vector-provider.ts".into(),
            "--extension".into(),
            "${VECTOR_OVERLAY}/pi/vector-policy.ts".into(),
        ];
        Ok(NativeRunPlan {
            harness: self.id(),
            executable: self.executable().into(),
            argv,
            environment: BTreeMap::from([
                (
                    "PI_CODING_AGENT_DIR".into(),
                    EnvironmentValue::Literal("${VECTOR_OVERLAY}/pi/agent".into()),
                ),
                (
                    "PI_SKIP_VERSION_CHECK".into(),
                    EnvironmentValue::Literal("1".into()),
                ),
            ]),
            cwd: spec.workspace.root.display().to_string(),
            generated_files: vec![
                GeneratedFile {
                    relative_path: "pi/vector-provider.ts".into(),
                    content: provider_extension,
                    sensitive: false,
                },
                GeneratedFile {
                    relative_path: "pi/vector-policy.ts".into(),
                    content: policy_extension,
                    sensitive: false,
                },
            ],
            observation: "Pi newline-delimited RPC over stdin/stdout".into(),
            cleanup: vec!["remove run-scoped overlay".into()],
            native_summary: format!(
                "Pi RPC {} through vector-local/{}",
                if spec.grant.yolo { "YOLO" } else { "guarded" },
                spec.models.primary
            ),
        })
    }
}

pub struct DeepseekAdapter;

#[async_trait]
impl HarnessAdapter for DeepseekAdapter {
    fn id(&self) -> HarnessId {
        HarnessId::Deepseek
    }
    fn package_pin(&self) -> &'static str {
        "@deepseek-ai/dsh@0.1.1-rc.2"
    }
    fn executable(&self) -> &'static str {
        "npx"
    }

    fn capabilities(&self) -> BTreeMap<Capability, CapabilitySupport> {
        matrix(&[
            (
                FILESYSTEM_READ,
                SupportLevel::Native,
                EnforcementLevel::Harness,
            ),
            (
                FILESYSTEM_WORKSPACE_WRITE,
                SupportLevel::Native,
                EnforcementLevel::Harness,
            ),
            (
                PROCESS_EXECUTE,
                SupportLevel::Native,
                EnforcementLevel::Harness,
            ),
            (CODE_SEARCH, SupportLevel::Native, EnforcementLevel::Harness),
            (CODE_LSP, SupportLevel::Native, EnforcementLevel::Harness),
            (
                AGENTS_SPAWN,
                SupportLevel::Native,
                EnforcementLevel::Harness,
            ),
            (
                COMPUTER_INSPECT,
                SupportLevel::Experimental,
                EnforcementLevel::Harness,
            ),
            (
                COMPUTER_CONTROL,
                SupportLevel::Experimental,
                EnforcementLevel::Harness,
            ),
        ])
    }

    fn compile(&self, spec: &PortableRunSpec) -> Result<NativeRunPlan, HarnessError> {
        ensure_harness(spec, self.id())?;
        if spec.grant.yolo {
            return Err(HarnessError::YoloUnverified);
        }
        validate_supported(spec, &self.capabilities())?;
        let composition = serde_yaml::to_string(&json!({
            "$patch": "vector.dev/deepseek/0.1.1-rc.2",
            "vector": { "runId": spec.run_id, "policy": spec.capabilities },
            "llm": { "provider": "openai-compatible", "baseUrl": spec.provider.base_url, "model": spec.models.primary },
            "workspace": { "cwd": spec.workspace.root },
            "telemetry": { "enabled": false }
        }))?;
        Ok(NativeRunPlan {
            harness: self.id(),
            executable: self.executable().into(),
            argv: vec![
                "--yes".into(),
                "@deepseek-ai/dsh@0.1.1-rc.2".into(),
                "web".into(),
                "--no-open".into(),
                "--host".into(),
                "127.0.0.1".into(),
                "--port".into(),
                "3080".into(),
                "--patch".into(),
                "${VECTOR_OVERLAY}/deepseek/vector.cordis.yml".into(),
            ],
            environment: provider_environment(spec),
            cwd: spec.workspace.root.display().to_string(),
            generated_files: vec![GeneratedFile {
                relative_path: "deepseek/vector.cordis.yml".into(),
                content: composition,
                sensitive: false,
            }],
            observation:
                "DeepSeek web/SDK structured session events; loopback UI at 127.0.0.1:3080".into(),
            cleanup: vec![
                "stop loopback server".into(),
                "remove run-scoped overlay".into(),
            ],
            native_summary: format!(
                "DeepSeek Harness preview 0.1.1-rc.2 web profile with {}",
                spec.models.primary
            ),
        })
    }
}

fn matrix(
    entries: &[(&str, SupportLevel, EnforcementLevel)],
) -> BTreeMap<Capability, CapabilitySupport> {
    entries
        .iter()
        .map(|(capability, support, enforcement)| {
            (
                Capability::from(*capability),
                CapabilitySupport {
                    support: *support,
                    enforcement: *enforcement,
                    note: None,
                },
            )
        })
        .collect()
}

fn ensure_harness(spec: &PortableRunSpec, expected: HarnessId) -> Result<(), HarnessError> {
    if spec.harness.harness != expected {
        return Err(HarnessError::WrongAdapter {
            expected,
            actual: spec.harness.harness,
        });
    }
    Ok(())
}

fn validate_supported(
    spec: &PortableRunSpec,
    support: &BTreeMap<Capability, CapabilitySupport>,
) -> Result<(), HarnessError> {
    for (capability, decision) in &spec.capabilities {
        if decision.effective != PolicyDecision::Deny
            && support
                .get(capability)
                .is_some_and(|support| support.support == SupportLevel::Unsupported)
        {
            return Err(HarnessError::UnsupportedCapability(capability.clone()));
        }
    }
    if enabled(spec, COMPUTER_CONTROL)
        && spec.models.vision.is_none()
        && spec.harness.harness != HarnessId::Omp
    {
        return Err(HarnessError::VisionModelRequired);
    }
    Ok(())
}

fn enabled(spec: &PortableRunSpec, capability: &str) -> bool {
    spec.capabilities
        .get(&Capability::from(capability))
        .is_some_and(|policy| policy.effective != PolicyDecision::Deny)
}

fn tool_policy(spec: &PortableRunSpec) -> BTreeMap<String, String> {
    spec.capabilities
        .iter()
        .map(|(capability, policy)| (capability.0.clone(), policy.effective.to_string()))
        .collect()
}

fn provider_environment(spec: &PortableRunSpec) -> BTreeMap<String, EnvironmentValue> {
    let mut env = BTreeMap::from([(
        "OPENAI_BASE_URL".into(),
        EnvironmentValue::Literal(spec.provider.base_url.clone()),
    )]);
    if let Some(secret) = &spec.provider.secret_ref {
        env.insert(
            "OPENAI_API_KEY".into(),
            EnvironmentValue::SecretReference(secret.clone()),
        );
    }
    env
}

fn expected(harness: HarnessId) -> (&'static str, &'static str, &'static str, &'static str) {
    match harness {
        HarnessId::Omp => (OMP_PACKAGE, OMP_VERSION, OMP_INTEGRITY, "bun@1.3.14"),
        HarnessId::Pi => (PI_PACKAGE, PI_VERSION, PI_INTEGRITY, "node@22.19.0"),
        HarnessId::Deepseek => (
            "@deepseek-ai/dsh",
            "0.1.1-rc.2",
            "preview-lock-required",
            "node@22.19.0+pnpm@11.7.0",
        ),
    }
}

fn managed_harness_root(harness: HarnessId) -> Option<PathBuf> {
    let paths = application_paths()?;
    let (_, version, _, _) = expected(harness);
    Some(
        paths
            .data_dir
            .join("runtimes/harnesses")
            .join(harness.to_string())
            .join(version),
    )
}

fn managed_runtime_executable(harness: HarnessId) -> Option<PathBuf> {
    let paths = application_paths()?;
    match harness {
        HarnessId::Omp => Some(
            paths
                .data_dir
                .join("runtimes/bun")
                .join(BUN_VERSION)
                .join("bin/bun"),
        ),
        HarnessId::Pi | HarnessId::Deepseek => Some(
            paths
                .data_dir
                .join("runtimes/node")
                .join(NODE_VERSION)
                .join(if cfg!(windows) {
                    "node.exe"
                } else {
                    "bin/node"
                }),
        ),
    }
}

fn package_root(root: &Path, package: &str) -> PathBuf {
    let mut path = root.join("node_modules");
    for segment in package.split('/') {
        path.push(segment);
    }
    path
}

fn managed_record(harness: HarnessId) -> Option<HarnessInstallationRecord> {
    let root = managed_harness_root(harness)?;
    let runtime_executable = managed_runtime_executable(harness)?;
    let (package, version, integrity, runtime) = expected(harness);
    let package_root = package_root(&root, package);
    let executable = root.join("node_modules/.bin").join(match harness {
        HarnessId::Omp => "omp",
        HarnessId::Pi => "pi",
        HarnessId::Deepseek => "dsh",
    });
    let installed = read_package_identity(&package_root.join("package.json"));
    let manifest_valid = std::fs::read(root.join("installation.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|manifest| {
            manifest.get("package").and_then(serde_json::Value::as_str) == Some(package)
                && manifest.get("version").and_then(serde_json::Value::as_str) == Some(version)
                && manifest
                    .get("integrity")
                    .and_then(serde_json::Value::as_str)
                    == Some(integrity)
        });
    if !runtime_executable.is_file()
        || !manifest_valid
        || installed.as_ref().map(|value| value.0.as_str()) != Some(package)
        || installed.as_ref().map(|value| value.1.as_str()) != Some(version)
    {
        return None;
    }
    Some(HarnessInstallationRecord {
        harness,
        source: InstallationSource::Managed,
        executable: Some(executable),
        runtime_executable: Some(runtime_executable),
        package_root: Some(package_root),
        package: Some(package.into()),
        version: Some(version.into()),
        integrity: Some(integrity.into()),
        runtime: runtime.into(),
        compatibility: if harness == HarnessId::Deepseek {
            CompatibilityState::ManagedExperimental
        } else {
            CompatibilityState::ManagedTested
        },
        ready: true,
        notes: vec![format!("Vector-managed exact pin {package}@{version}")],
    })
}

pub async fn inspect_harness(harness: HarnessId) -> HarnessInstallationRecord {
    if let Some(record) = managed_record(harness) {
        return record;
    }
    let (expected_package, expected_version, _, runtime) = expected(harness);
    let executable_name = match harness {
        HarnessId::Omp => "omp",
        HarnessId::Pi => "pi",
        HarnessId::Deepseek => "dsh",
    };
    if let Some(executable) = find_executable(executable_name) {
        let canonical = std::fs::canonicalize(&executable).unwrap_or(executable.clone());
        let identity = find_package_identity(&canonical);
        let exact = identity.as_ref().is_some_and(|(package, version, _)| {
            package == expected_package && version == expected_version
        });
        let (package, version, package_root) = identity
            .map(|(package, version, root)| (Some(package), Some(version), Some(root)))
            .unwrap_or_default();
        return HarnessInstallationRecord {
            harness,
            source: InstallationSource::External,
            executable: Some(executable),
            runtime_executable: None,
            package_root,
            package: package.clone(),
            version: version.clone(),
            integrity: None,
            runtime: runtime.into(),
            compatibility: if exact {
                CompatibilityState::ExternalCompatible
            } else {
                CompatibilityState::ExternalUnverified
            },
            ready: exact,
            notes: vec![if exact {
                format!("External installation matches {expected_package}@{expected_version}")
            } else {
                format!(
                    "External executable is {}@{}; Vector requires {expected_package}@{expected_version}",
                    package.as_deref().unwrap_or("unknown"),
                    version.as_deref().unwrap_or("unknown")
                )
            }],
        };
    }
    HarnessInstallationRecord {
        harness,
        source: InstallationSource::Missing,
        executable: None,
        runtime_executable: None,
        package_root: None,
        package: None,
        version: None,
        integrity: None,
        runtime: runtime.into(),
        compatibility: CompatibilityState::ExternalIncompatible,
        ready: false,
        notes: vec![format!(
            "No verified executable found; managed pin is {expected_package}@{expected_version}"
        )],
    }
}

pub async fn harness_inventory() -> Vec<HarnessInstallationRecord> {
    let (omp, pi, deepseek) = tokio::join!(
        inspect_harness(HarnessId::Omp),
        inspect_harness(HarnessId::Pi),
        inspect_harness(HarnessId::Deepseek)
    );
    vec![omp, pi, deepseek]
}

pub async fn install_managed_harness(
    harness: HarnessId,
) -> Result<HarnessInstallationRecord, HarnessError> {
    if harness == HarnessId::Deepseek {
        return Err(HarnessError::InstallUnsupported(
            "DeepSeek managed installation remains preview-gated".into(),
        ));
    }
    if let Some(record) = managed_record(harness) {
        return Ok(record);
    }
    let install_root = managed_harness_root(harness).ok_or_else(|| {
        HarnessError::InstallFailed("Vector application directory is unavailable".into())
    })?;
    tokio::fs::create_dir_all(&install_root).await?;
    let (package, version, integrity, _) = expected(harness);
    match harness {
        HarnessId::Omp => {
            let bun = ensure_managed_bun().await?;
            run_install(
                &bun,
                &["add", "--exact", &format!("{package}@{version}")],
                &install_root,
                None,
            )
            .await?;
        }
        HarnessId::Pi => {
            let node = ensure_managed_node().await?;
            let npm = node
                .parent()
                .unwrap_or(Path::new("."))
                .join(if cfg!(windows) { "npm.cmd" } else { "npm" });
            let mut path = OsString::from(node.parent().unwrap_or(Path::new(".")).as_os_str());
            if let Some(existing) = std::env::var_os("PATH") {
                path.push(if cfg!(windows) { ";" } else { ":" });
                path.push(existing);
            }
            run_install(
                &npm,
                &[
                    "install",
                    "--save-exact",
                    "--prefix",
                    install_root.to_string_lossy().as_ref(),
                    &format!("{package}@{version}"),
                ],
                &install_root,
                Some(path),
            )
            .await?;
        }
        HarnessId::Deepseek => unreachable!(),
    }
    let manifest = json!({
        "harness": harness,
        "package": package,
        "version": version,
        "integrity": integrity,
        "installedAt": chrono_like_now(),
    });
    tokio::fs::write(
        install_root.join("installation.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )
    .await?;
    managed_record(harness).ok_or_else(|| {
        HarnessError::InstallFailed("installed files did not pass exact-pin validation".into())
    })
}

pub fn bind_installation(
    plan: &mut NativeRunPlan,
    installation: &HarnessInstallationRecord,
) -> Result<(), HarnessError> {
    if !installation.ready {
        return Err(HarnessError::InstallationUnverified(
            installation.notes.join("; "),
        ));
    }
    match installation.source {
        InstallationSource::Managed => {
            let runtime = installation.runtime_executable.as_ref().ok_or_else(|| {
                HarnessError::InstallFailed("managed runtime executable is missing".into())
            })?;
            let package_root = installation.package_root.as_ref().ok_or_else(|| {
                HarnessError::InstallFailed("managed package root is missing".into())
            })?;
            let cli = match installation.harness {
                HarnessId::Omp => package_root.join("dist/cli.js"),
                HarnessId::Pi => package_root.join("dist/bundle/cli.js"),
                HarnessId::Deepseek => package_root.join("dist/cli.js"),
            };
            plan.executable = runtime.display().to_string();
            plan.argv.insert(0, cli.display().to_string());
        }
        InstallationSource::External => {
            plan.executable = installation
                .executable
                .as_ref()
                .ok_or_else(|| HarnessError::InstallationUnverified("missing executable".into()))?
                .display()
                .to_string();
        }
        InstallationSource::Missing => {
            return Err(HarnessError::InstallationUnverified(
                "harness is not installed".into(),
            ));
        }
    }
    Ok(())
}

async fn ensure_managed_bun() -> Result<PathBuf, HarnessError> {
    let destination = managed_runtime_executable(HarnessId::Omp).ok_or_else(|| {
        HarnessError::InstallFailed("Vector application directory is unavailable".into())
    })?;
    if command_version(&destination).await.as_deref() == Some(BUN_VERSION) {
        return Ok(destination);
    }
    let source = find_executable("bun")
        .ok_or_else(|| HarnessError::InstallFailed("Bun 1.3.14 is not installed".into()))?;
    if command_version(&source).await.as_deref() != Some(BUN_VERSION) {
        return Err(HarnessError::InstallFailed(format!(
            "managed OMP requires Bun {BUN_VERSION}"
        )));
    }
    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::copy(source, &destination).await?;
    Ok(destination)
}

async fn ensure_managed_node() -> Result<PathBuf, HarnessError> {
    let destination = managed_runtime_executable(HarnessId::Pi).ok_or_else(|| {
        HarnessError::InstallFailed("Vector application directory is unavailable".into())
    })?;
    if command_version(&destination).await.as_deref() == Some(&format!("v{NODE_VERSION}")) {
        return Ok(destination);
    }
    #[cfg(windows)]
    return Err(HarnessError::InstallUnsupported(
        "managed Node extraction on Windows is not enabled yet".into(),
    ));
    #[cfg(not(windows))]
    {
        let platform = match (std::env::consts::OS, std::env::consts::ARCH) {
            ("macos", "aarch64") => "darwin-arm64",
            ("macos", "x86_64") => "darwin-x64",
            ("linux", "aarch64") => "linux-arm64",
            ("linux", "x86_64") => "linux-x64",
            (os, arch) => {
                return Err(HarnessError::InstallUnsupported(format!(
                    "Node {NODE_VERSION} is not packaged for {os}/{arch}"
                )));
            }
        };
        let archive_name = format!("node-v{NODE_VERSION}-{platform}.tar.gz");
        let base = format!("https://nodejs.org/dist/v{NODE_VERSION}");
        let client = reqwest::Client::builder().build()?;
        let checksums = client
            .get(format!("{base}/SHASUMS256.txt"))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let expected_checksum = checksums
            .lines()
            .find_map(|line| {
                let (checksum, name) = line.split_once("  ")?;
                (name == archive_name).then(|| checksum.to_string())
            })
            .ok_or_else(|| HarnessError::InstallFailed("Node checksum is missing".into()))?;
        let bytes = client
            .get(format!("{base}/{archive_name}"))
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        let actual = format!("{:x}", Sha256::digest(&bytes));
        if actual != expected_checksum {
            return Err(HarnessError::InstallFailed(
                "Node archive checksum did not match".into(),
            ));
        }
        let runtime_root = destination
            .parent()
            .and_then(Path::parent)
            .ok_or_else(|| HarnessError::InstallFailed("invalid runtime path".into()))?
            .to_path_buf();
        let parent = runtime_root
            .parent()
            .ok_or_else(|| HarnessError::InstallFailed("invalid runtime parent".into()))?;
        tokio::fs::create_dir_all(parent).await?;
        let temp = tempfile::Builder::new()
            .prefix("node-install-")
            .tempdir_in(parent)?;
        let temp_path = temp.path().to_path_buf();
        let bytes = bytes.to_vec();
        tokio::task::spawn_blocking(move || -> Result<(), HarnessError> {
            Archive::new(GzDecoder::new(bytes.as_slice())).unpack(&temp_path)?;
            Ok(())
        })
        .await
        .map_err(|error| HarnessError::InstallFailed(error.to_string()))??;
        let extracted = temp.path().join(format!("node-v{NODE_VERSION}-{platform}"));
        if runtime_root.exists() {
            std::fs::remove_dir_all(&runtime_root)?;
        }
        std::fs::rename(extracted, &runtime_root)?;
        Ok(destination)
    }
}

async fn run_install(
    executable: &Path,
    args: &[&str],
    cwd: &Path,
    path: Option<OsString>,
) -> Result<(), HarnessError> {
    let mut command = Command::new(executable);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = path {
        command.env("PATH", path);
    }
    let output = command.output().await?;
    if !output.status.success() {
        return Err(HarnessError::InstallFailed(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    Ok(())
}

async fn command_version(path: &Path) -> Option<String> {
    let output = Command::new(path).arg("--version").output().await.ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn find_executable(executable: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|path| path.join(executable))
        .find(|path| path.is_file())
}

fn read_package_identity(path: &Path) -> Option<(String, String)> {
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    Some((
        value.get("name")?.as_str()?.to_string(),
        value.get("version")?.as_str()?.to_string(),
    ))
}

fn find_package_identity(executable: &Path) -> Option<(String, String, PathBuf)> {
    let mut current = executable.parent()?;
    loop {
        let package = current.join("package.json");
        if let Some((name, version)) = read_package_identity(&package) {
            return Some((name, version, current.to_path_buf()));
        }
        current = current.parent()?;
    }
}

fn chrono_like_now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .unwrap_or_else(|_| "0".into())
}

#[derive(Debug, Error)]
pub enum HarnessError {
    #[error("VCTR_HARNESS_INCOMPATIBLE: adapter {expected} cannot compile a {actual} run")]
    WrongAdapter {
        expected: HarnessId,
        actual: HarnessId,
    },
    #[error("VCTR_CAPABILITY_UNSATISFIED: {0} is unsupported by the selected harness")]
    UnsupportedCapability(Capability),
    #[error(
        "VCTR_CAPABILITY_UNSATISFIED: computer control requires a vision model role for this harness"
    )]
    VisionModelRequired,
    #[error(
        "VCTR_HARNESS_INCOMPATIBLE: DeepSeek YOLO is blocked until approval-bypass conformance passes"
    )]
    YoloUnverified,
    #[error("VCTR_RUN_FAILED: serialization failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("VCTR_RUN_FAILED: YAML serialization failed: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("VCTR_RUN_FAILED: run-spec hashing failed: {0}")]
    Core(#[from] vector_core::CoreError),
    #[error("VCTR_HARNESS_UNVERIFIED: {0}")]
    InstallationUnverified(String),
    #[error("VCTR_RUNTIME_INSTALL_FAILED: {0}")]
    InstallFailed(String),
    #[error("VCTR_RUNTIME_UNAVAILABLE: {0}")]
    InstallUnsupported(String),
    #[error("VCTR_RUNTIME_INSTALL_FAILED: filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("VCTR_RUNTIME_INSTALL_FAILED: download failed: {0}")]
    Download(#[from] reqwest::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use vector_config_for_test::sample;

    mod vector_config_for_test {
        use super::*;
        use chrono::Utc;
        use uuid::Uuid;

        pub fn sample(harness: HarnessId, yolo: bool) -> PortableRunSpec {
            PortableRunSpec {
                api_version: API_VERSION.into(),
                run_id: Uuid::now_v7(),
                created_at: Utc::now(),
                profile: "test".into(),
                workspace: WorkspaceIdentity {
                    root: Path::new("/tmp/vector").into(),
                    git_commit: None,
                    git_dirty: false,
                    trust: RepositoryTrust::Executable,
                },
                harness: HarnessInstallation {
                    harness,
                    adapter_version: "0.1.0".into(),
                    runtime: "test".into(),
                    package: "test".into(),
                    version: "test".into(),
                    compatibility: CompatibilityState::ManagedTested,
                },
                provider: ResolvedProvider {
                    id: "local".into(),
                    kind: "lm-studio".into(),
                    base_url: "http://127.0.0.1:1234/v1".into(),
                    secret_ref: None,
                    local: true,
                    service_fingerprint: None,
                },
                models: ModelRoles {
                    primary: "qwen".into(),
                    vision: Some("qwen-vl".into()),
                    ..Default::default()
                },
                capabilities: BTreeMap::from([(
                    Capability::from(FILESYSTEM_READ),
                    EffectiveCapability {
                        requested: PolicyDecision::Allow,
                        effective: PolicyDecision::Allow,
                        provenance: vec![],
                    },
                )]),
                isolation: IsolationConfig::default(),
                packs: vec![],
                verification: VerificationConfig::default(),
                context: ContextConfig::default(),
                native: json!({}),
                grant: RunGrant {
                    yolo,
                    granted_by: None,
                    granted_at: None,
                },
                provenance: BTreeMap::new(),
            }
        }
    }

    #[test]
    fn omp_yolo_uses_native_flag() {
        let plan =
            compile_for_surface(&sample(HarnessId::Omp, true), LaunchSurface::Native).unwrap();
        assert!(plan.argv.contains(&"--yolo".to_string()));
    }

    #[test]
    fn pi_uses_rpc_and_run_scoped_extensions() {
        let plan = PiAdapter.compile(&sample(HarnessId::Pi, false)).unwrap();
        assert!(plan.argv.windows(2).any(|args| args == ["--mode", "rpc"]));
        assert_eq!(plan.generated_files.len(), 2);
    }

    #[test]
    fn deepseek_yolo_is_fail_closed() {
        assert!(matches!(
            DeepseekAdapter.compile(&sample(HarnessId::Deepseek, true)),
            Err(HarnessError::YoloUnverified)
        ));
    }
}
