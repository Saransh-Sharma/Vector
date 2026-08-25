//! Harness adapters compile a portable spec into an inspectable native plan.

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use thiserror::Error;
use vector_core::capabilities::*;
use vector_core::*;

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
        let executable = self.executable().to_string();
        DoctorReport {
            harness: self.id(),
            detected: executable_on_path(&executable),
            executable,
            compatibility: match self.id() {
                HarnessId::Deepseek => CompatibilityState::ManagedExperimental,
                _ => CompatibilityState::ManagedTested,
            },
            notes: vec![format!("managed package pin: {}", self.package_pin())],
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
                "approvalMode": if spec.grant.yolo { "yolo" } else { "confirm" },
                "approval": approval,
            },
            "model": { "provider": "openai", "id": spec.models.primary },
            "vector": { "runId": spec.run_id, "runSpecFingerprint": spec.fingerprint()? }
        }))?;
        let mut argv = vec![
            "acp".into(),
            "--config".into(),
            "${VECTOR_OVERLAY}/omp/config.yml".into(),
        ];
        if spec.grant.yolo {
            argv.push("--yolo".into());
        }
        Ok(NativeRunPlan {
            harness: self.id(),
            executable: self.executable().into(),
            argv,
            environment: provider_environment(spec),
            cwd: spec.workspace.root.display().to_string(),
            generated_files: vec![GeneratedFile {
                relative_path: "omp/config.yml".into(),
                content: config,
                sensitive: false,
            }],
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
    models: [{{ id: {model:?}, name: {model:?}, input: {input}, reasoning: true }}]
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
            environment: BTreeMap::new(),
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

fn executable_on_path(executable: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|path| {
        let candidate = path.join(executable);
        candidate.is_file() || cfg!(windows) && path.join(format!("{executable}.exe")).is_file()
    })
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
        let plan = OmpAdapter.compile(&sample(HarnessId::Omp, true)).unwrap();
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
