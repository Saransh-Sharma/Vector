//! Layered configuration, trust, and immutable run-spec resolution.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::Utc;
use serde_json::{Map, Value, json};
use thiserror::Error;
use uuid::Uuid;
use vector_core::capabilities::*;
use vector_core::*;

const DEFAULTS: &str = r#"
apiVersion: vector.dev/v1alpha1
kind: VectorWorkspace
trust: untrusted
providers: {}
profiles: {}
"#;

#[derive(Debug, Clone)]
pub struct ConfigLayer {
    pub name: String,
    pub path: Option<PathBuf>,
    pub value: Value,
}

#[derive(Debug, Clone)]
pub struct ResolvedWorkspace {
    pub config: VectorWorkspace,
    pub provenance: BTreeMap<String, String>,
    pub layers: Vec<ConfigLayer>,
}

pub fn user_config_path() -> Option<PathBuf> {
    application_paths().map(|paths| paths.config_dir.join("config.yaml"))
}

pub fn load_workspace(root: &Path) -> Result<ResolvedWorkspace, ConfigError> {
    let defaults: Value = serde_yaml::from_str(DEFAULTS)?;
    let mut layers = vec![ConfigLayer {
        name: "built-in defaults".into(),
        path: None,
        value: defaults,
    }];

    if let Some(path) = user_config_path().filter(|path| path.exists()) {
        layers.push(read_layer("user configuration", &path, false)?);
    }

    let repo = root.join(".vector/vector.yaml");
    if repo.exists() {
        layers.push(read_layer("repository configuration", &repo, true)?);
    }
    let local = root.join(".vector/local.yaml");
    if local.exists() {
        layers.push(read_layer("machine-local configuration", &local, false)?);
    }

    let mut merged = Value::Object(Map::new());
    let mut provenance = BTreeMap::new();
    for layer in &layers {
        deep_merge(
            &mut merged,
            layer.value.clone(),
            "",
            &layer.name,
            &mut provenance,
        );
    }
    let config: VectorWorkspace = serde_json::from_value(merged)?;
    validate_workspace(&config)?;
    Ok(ResolvedWorkspace {
        config,
        provenance,
        layers,
    })
}

fn read_layer(name: &str, path: &Path, committed: bool) -> Result<ConfigLayer, ConfigError> {
    let text = fs::read_to_string(path).map_err(|source| ConfigError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let yaml: serde_yaml::Value = serde_yaml::from_str(&text)?;
    let mut value = serde_json::to_value(yaml)?;
    if committed {
        reject_raw_secrets(&value, "")?;
    }
    // A repository cannot mark itself trusted. Trust is a user-owned local decision.
    if committed {
        if let Value::Object(map) = &mut value {
            if map.contains_key("trust") {
                map.insert("trust".into(), Value::String("untrusted".into()));
            }
        }
    }
    Ok(ConfigLayer {
        name: name.into(),
        path: Some(path.to_path_buf()),
        value,
    })
}

fn deep_merge(
    target: &mut Value,
    incoming: Value,
    prefix: &str,
    layer: &str,
    provenance: &mut BTreeMap<String, String>,
) {
    match (target, incoming) {
        (Value::Object(target), Value::Object(incoming)) => {
            for (key, value) in incoming {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                if let Some(existing) = target.get_mut(&key) {
                    deep_merge(existing, value, &path, layer, provenance);
                } else {
                    mark_provenance(&value, &path, layer, provenance);
                    target.insert(key, value);
                }
            }
        }
        (target, incoming) => {
            mark_provenance(&incoming, prefix, layer, provenance);
            *target = incoming;
        }
    }
}

fn mark_provenance(
    value: &Value,
    prefix: &str,
    layer: &str,
    provenance: &mut BTreeMap<String, String>,
) {
    provenance.insert(prefix.to_string(), layer.to_string());
    if let Value::Object(map) = value {
        for (key, value) in map {
            let child = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            mark_provenance(value, &child, layer, provenance);
        }
    }
}

fn reject_raw_secrets(value: &Value, path: &str) -> Result<(), ConfigError> {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                let sensitive = matches!(
                    key.to_ascii_lowercase().as_str(),
                    "apikey" | "api_key" | "token" | "password" | "secret"
                );
                if sensitive
                    && value
                        .as_str()
                        .is_some_and(|text| !is_secret_reference(text))
                {
                    return Err(ConfigError::RawSecret(child));
                }
                reject_raw_secrets(value, &child)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                reject_raw_secrets(value, path)?;
            }
        }
        _ => {}
    }
    Ok(())
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

fn validate_workspace(config: &VectorWorkspace) -> Result<(), ConfigError> {
    if config.api_version != API_VERSION {
        return Err(ConfigError::UnsupportedApi(config.api_version.clone()));
    }
    if config.kind != "VectorWorkspace" {
        return Err(ConfigError::InvalidKind(config.kind.clone()));
    }
    for (name, provider) in &config.providers {
        if provider.base_url.trim().is_empty() {
            return Err(ConfigError::InvalidProvider(name.clone()));
        }
        if let Some(secret) = &provider.secret_ref {
            if !is_secret_reference(secret) {
                return Err(ConfigError::RawSecret(format!(
                    "providers.{name}.secretRef"
                )));
            }
        }
    }
    Ok(())
}

pub fn resolve_run(
    root: &Path,
    requested_profile: Option<&str>,
    grant_yolo: bool,
) -> Result<PortableRunSpec, ConfigError> {
    let resolved = load_workspace(root)?;
    let profile_name = requested_profile
        .map(str::to_owned)
        .or_else(|| resolved.config.default_profile.clone())
        .ok_or(ConfigError::ProfileRequired)?;
    let profile = resolved
        .config
        .profiles
        .get(&profile_name)
        .ok_or_else(|| ConfigError::UnknownProfile(profile_name.clone()))?;
    if profile.yolo && !grant_yolo {
        return Err(ConfigError::YoloGrantRequired(profile_name));
    }
    let provider = resolved
        .config
        .providers
        .get(&profile.provider)
        .ok_or_else(|| ConfigError::UnknownProvider(profile.provider.clone()))?;
    if profile.models.primary.trim().is_empty() {
        return Err(ConfigError::ModelRequired(profile_name));
    }

    let capabilities = resolve_policy(profile, resolved.config.trust, grant_yolo);
    let (commit, dirty) = git_state(root);
    let harness = installation(profile.harness);
    Ok(PortableRunSpec {
        api_version: API_VERSION.to_string(),
        run_id: Uuid::now_v7(),
        created_at: Utc::now(),
        profile: profile_name,
        workspace: WorkspaceIdentity {
            root: root.canonicalize().unwrap_or_else(|_| root.to_path_buf()),
            git_commit: commit,
            git_dirty: dirty,
            trust: resolved.config.trust,
        },
        harness,
        provider: ResolvedProvider {
            id: profile.provider.clone(),
            kind: provider.kind.clone(),
            base_url: provider.base_url.clone(),
            secret_ref: provider.secret_ref.clone(),
            local: provider.local,
            service_fingerprint: None,
        },
        models: profile.models.clone(),
        capabilities,
        isolation: profile.isolation.clone(),
        packs: profile.packs.clone(),
        verification: profile.verification.clone(),
        context: profile.context.clone(),
        native: profile.native.clone(),
        grant: RunGrant {
            yolo: grant_yolo,
            granted_by: grant_yolo.then(|| whoami()),
            granted_at: grant_yolo.then(Utc::now),
        },
        provenance: resolved.provenance,
    })
}

fn installation(harness: HarnessId) -> HarnessInstallation {
    match harness {
        HarnessId::Omp => HarnessInstallation {
            harness,
            adapter_version: env!("CARGO_PKG_VERSION").into(),
            runtime: "bun@1.3.14".into(),
            package: "@oh-my-pi/pi-coding-agent".into(),
            version: "18.0.4".into(),
            compatibility: CompatibilityState::ManagedTested,
        },
        HarnessId::Pi => HarnessInstallation {
            harness,
            adapter_version: env!("CARGO_PKG_VERSION").into(),
            runtime: "node@22.19.0".into(),
            package: "@earendil-works/pi-coding-agent".into(),
            version: "0.84.3".into(),
            compatibility: CompatibilityState::ManagedTested,
        },
        HarnessId::Deepseek => HarnessInstallation {
            harness,
            adapter_version: env!("CARGO_PKG_VERSION").into(),
            runtime: "node@22.19.0+pnpm@11.7.0".into(),
            package: "@deepseek-ai/dsh".into(),
            version: "0.1.1-rc.2".into(),
            compatibility: CompatibilityState::ManagedExperimental,
        },
    }
}

fn base_policy() -> BTreeMap<Capability, PolicyDecision> {
    [
        (FILESYSTEM_READ, PolicyDecision::Allow),
        (FILESYSTEM_WORKSPACE_WRITE, PolicyDecision::Allow),
        (FILESYSTEM_EXTERNAL_WRITE, PolicyDecision::Deny),
        (FILESYSTEM_DELETE, PolicyDecision::Prompt),
        (PROCESS_EXECUTE, PolicyDecision::Prompt),
        (PROCESS_BACKGROUND, PolicyDecision::Prompt),
        (PROCESS_PTY, PolicyDecision::Allow),
        (NETWORK_READ, PolicyDecision::Prompt),
        (NETWORK_WRITE, PolicyDecision::Prompt),
        (NETWORK_LISTEN, PolicyDecision::Prompt),
        (NETWORK_UNRESTRICTED, PolicyDecision::Deny),
        (GIT_READ, PolicyDecision::Allow),
        (GIT_WRITE, PolicyDecision::Prompt),
        (GIT_COMMIT, PolicyDecision::Prompt),
        (GIT_PUSH, PolicyDecision::Prompt),
        (GIT_FORCE_PUSH, PolicyDecision::Deny),
        (CODE_SEARCH, PolicyDecision::Allow),
        (CODE_LSP, PolicyDecision::Allow),
        (CODE_DEBUGGER, PolicyDecision::Prompt),
        (AGENTS_SPAWN, PolicyDecision::Prompt),
        (COMPUTER_INSPECT, PolicyDecision::Prompt),
        (COMPUTER_CONTROL, PolicyDecision::Prompt),
        (CLIPBOARD_READ, PolicyDecision::Prompt),
        (CLIPBOARD_WRITE, PolicyDecision::Prompt),
        (MCP_READ, PolicyDecision::Prompt),
        (MCP_WRITE, PolicyDecision::Deny),
        (SECRETS_USE, PolicyDecision::Prompt),
        (SECRETS_MATERIALIZE_ENV, PolicyDecision::Deny),
        (SECRETS_MATERIALIZE_FILE, PolicyDecision::Deny),
    ]
    .into_iter()
    .map(|(capability, decision)| (Capability::from(capability), decision))
    .collect()
}

fn hard_floor() -> BTreeMap<Capability, PolicyDecision> {
    [
        (FILESYSTEM_EXTERNAL_WRITE, PolicyDecision::Deny),
        (GIT_FORCE_PUSH, PolicyDecision::Deny),
        (NETWORK_UNRESTRICTED, PolicyDecision::Deny),
        (MCP_WRITE, PolicyDecision::Deny),
        (SECRETS_MATERIALIZE_ENV, PolicyDecision::Deny),
        (SECRETS_MATERIALIZE_FILE, PolicyDecision::Deny),
    ]
    .into_iter()
    .map(|(capability, decision)| (Capability::from(capability), decision))
    .collect()
}

fn resolve_policy(
    profile: &Profile,
    trust: RepositoryTrust,
    grant_yolo: bool,
) -> BTreeMap<Capability, EffectiveCapability> {
    let base = base_policy();
    let floor = hard_floor();
    let mut keys: BTreeSet<Capability> = base.keys().cloned().collect();
    keys.extend(profile.policy.keys().cloned());
    keys.extend(floor.keys().cloned());

    keys.into_iter()
        .map(|capability| {
            let requested = profile
                .policy
                .get(&capability)
                .copied()
                .or_else(|| base.get(&capability).copied())
                .unwrap_or(PolicyDecision::Prompt);
            let mut decision = requested;
            let mut provenance = vec![PolicySource {
                layer: "profile/default".into(),
                decision: requested,
                detail: None,
            }];

            if grant_yolo && decision == PolicyDecision::Prompt {
                decision = PolicyDecision::Allow;
                provenance.push(PolicySource {
                    layer: "explicit run grant".into(),
                    decision,
                    detail: Some("YOLO converts prompt to allow".into()),
                });
            }
            if matches!(
                trust,
                RepositoryTrust::Untrusted | RepositoryTrust::ConfigOnly
            ) && is_executable(&capability)
            {
                decision = decision.max(PolicyDecision::Deny);
                provenance.push(PolicySource {
                    layer: "repository trust".into(),
                    decision: PolicyDecision::Deny,
                    detail: Some(format!("repository is {trust:?}")),
                });
            }
            if let Some(hard) = floor.get(&capability) {
                decision = decision.max(*hard);
                provenance.push(PolicySource {
                    layer: "Vector hard floor".into(),
                    decision: *hard,
                    detail: Some(
                        "cannot be weakened by profiles, Packs, native settings, or YOLO".into(),
                    ),
                });
            }
            (
                capability,
                EffectiveCapability {
                    requested,
                    effective: decision,
                    provenance,
                },
            )
        })
        .collect()
}

fn is_executable(capability: &Capability) -> bool {
    capability.0.starts_with("process.")
        || capability.0.starts_with("git.") && capability.0 != GIT_READ
        || capability.0.ends_with(".control")
        || capability.0 == MCP_WRITE
}

fn git_state(root: &Path) -> (Option<String>, bool) {
    let commit = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string());
    let dirty = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|out| out.status.success())
        .is_some_and(|out| !out.stdout.is_empty());
    (commit, dirty)
}

fn whoami() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "local-user".into())
}

pub fn starter_workspace(
    model: &str,
    vision_model: Option<&str>,
    _enable_computer: bool,
) -> VectorWorkspace {
    let provider = ProviderConfig {
        kind: "lm-studio".into(),
        base_url: "http://127.0.0.1:1234/v1".into(),
        secret_ref: None,
        local: true,
    };
    let roles = ModelRoles {
        primary: model.into(),
        vision: vision_model.map(str::to_owned),
        ..Default::default()
    };
    let mut policy = BTreeMap::new();
    policy.insert(Capability::from(COMPUTER_INSPECT), PolicyDecision::Deny);
    policy.insert(Capability::from(COMPUTER_CONTROL), PolicyDecision::Deny);

    let packs = vec![
        PackRef {
            id: "vector/base".into(),
            source: Some("builtin".into()),
            digest: None,
            lazy: false,
        },
        PackRef {
            id: "vector/local-lean".into(),
            source: Some("builtin".into()),
            digest: None,
            lazy: true,
        },
        PackRef {
            id: "vector/coding".into(),
            source: Some("builtin".into()),
            digest: None,
            lazy: true,
        },
    ];
    let mut profiles = BTreeMap::new();
    for harness in [HarnessId::Omp, HarnessId::Pi] {
        let prefix = harness.to_string();
        profiles.insert(
            format!("{prefix}-safe"),
            Profile {
                harness,
                provider: "lm-studio".into(),
                models: roles.clone(),
                policy: policy.clone(),
                isolation: IsolationConfig {
                    kind: IsolationKind::WorkspaceSandbox,
                    network: NetworkMode::ProviderOnly,
                    ..Default::default()
                },
                packs: packs.clone(),
                verification: VerificationConfig::default(),
                context: ContextConfig {
                    max_tokens: None,
                    stable_prefix: true,
                    lazy_resources: true,
                },
                native: json!({}),
                yolo: false,
            },
        );
        profiles.insert(
            format!("{prefix}-yolo"),
            Profile {
                harness,
                provider: "lm-studio".into(),
                models: roles.clone(),
                policy: policy.clone(),
                isolation: IsolationConfig {
                    kind: IsolationKind::WorkspaceSandbox,
                    network: NetworkMode::ProviderOnly,
                    ..Default::default()
                },
                packs: packs.clone(),
                verification: VerificationConfig::default(),
                context: ContextConfig {
                    max_tokens: None,
                    stable_prefix: true,
                    lazy_resources: true,
                },
                native: json!({}),
                yolo: true,
            },
        );
    }
    profiles.insert(
        "deepseek-preview".into(),
        Profile {
            harness: HarnessId::Deepseek,
            provider: "lm-studio".into(),
            models: roles,
            policy,
            isolation: IsolationConfig::default(),
            packs,
            verification: VerificationConfig::default(),
            context: ContextConfig {
                max_tokens: None,
                stable_prefix: true,
                lazy_resources: true,
            },
            native: json!({}),
            yolo: false,
        },
    );
    VectorWorkspace {
        api_version: API_VERSION.into(),
        kind: "VectorWorkspace".into(),
        trust: RepositoryTrust::Executable,
        default_profile: Some("pi-safe".into()),
        providers: BTreeMap::from([("lm-studio".into(), provider)]),
        profiles,
    }
}

pub fn write_workspace_atomic(
    root: &Path,
    config: &VectorWorkspace,
) -> Result<PathBuf, ConfigError> {
    let dir = root.join(".vector");
    fs::create_dir_all(&dir).map_err(|source| ConfigError::Read {
        path: dir.clone(),
        source,
    })?;
    let target = dir.join("vector.yaml");
    let temp = dir.join("vector.yaml.tmp");
    let mut committed = config.clone();
    let local_trust = committed.trust;
    committed.trust = RepositoryTrust::Untrusted;
    let text = serde_yaml::to_string(&committed)?;
    fs::write(&temp, text).map_err(|source| ConfigError::Read {
        path: temp.clone(),
        source,
    })?;
    fs::rename(&temp, &target).map_err(|source| ConfigError::Read {
        path: target.clone(),
        source,
    })?;
    let local_target = dir.join("local.yaml");
    let local_temp = dir.join("local.yaml.tmp");
    let local = serde_yaml::to_string(&json!({ "trust": local_trust }))?;
    fs::write(&local_temp, local).map_err(|source| ConfigError::Read {
        path: local_temp.clone(),
        source,
    })?;
    fs::rename(&local_temp, &local_target).map_err(|source| ConfigError::Read {
        path: local_target,
        source,
    })?;
    fs::write(dir.join(".gitignore"), "local.yaml\ngenerated/\nruns/\n").map_err(|source| {
        ConfigError::Read {
            path: dir.join(".gitignore"),
            source,
        }
    })?;
    Ok(target)
}

/// Enables computer use only after the runtime conformance probe has passed.
/// The change is atomic and applies to the Safe/YOLO pair for the selected
/// harness so switching launch grants never silently drops the verified role.
pub fn enable_verified_computer_use(
    root: &Path,
    harness: HarnessId,
    vision_model: &str,
) -> Result<PathBuf, ConfigError> {
    let path = root.join(".vector/vector.yaml");
    let text = fs::read_to_string(&path).map_err(|source| ConfigError::Read {
        path: path.clone(),
        source,
    })?;
    let mut config: VectorWorkspace = serde_yaml::from_str(&text)?;
    for profile in config
        .profiles
        .values_mut()
        .filter(|profile| profile.harness == harness)
    {
        profile.models.vision = Some(vision_model.to_owned());
        profile
            .policy
            .insert(Capability::from(COMPUTER_INSPECT), PolicyDecision::Allow);
        profile
            .policy
            .insert(Capability::from(COMPUTER_CONTROL), PolicyDecision::Prompt);
        if !profile
            .packs
            .iter()
            .any(|pack| pack.id == "vector/computer-use")
        {
            profile.packs.push(PackRef {
                id: "vector/computer-use".into(),
                source: Some("builtin".into()),
                digest: None,
                lazy: true,
            });
        }
    }
    write_workspace_atomic(root, &config)
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("VCTR_CONFIG_INVALID: could not read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("VCTR_CONFIG_INVALID: YAML is invalid: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("VCTR_CONFIG_INVALID: configuration does not match the schema: {0}")]
    Json(#[from] serde_json::Error),
    #[error("VCTR_CONFIG_INVALID: unsupported apiVersion {0}")]
    UnsupportedApi(String),
    #[error("VCTR_CONFIG_INVALID: expected kind VectorWorkspace, found {0}")]
    InvalidKind(String),
    #[error("VCTR_CONFIG_INVALID: provider {0} has no endpoint")]
    InvalidProvider(String),
    #[error("VCTR_CONFIG_INVALID: raw secret at {0}; use an OS secret reference")]
    RawSecret(String),
    #[error("VCTR_CONFIG_INVALID: select a profile or set defaultProfile")]
    ProfileRequired,
    #[error("VCTR_CONFIG_INVALID: unknown profile {0}")]
    UnknownProfile(String),
    #[error("VCTR_PROVIDER_UNAVAILABLE: unknown provider {0}")]
    UnknownProvider(String),
    #[error("VCTR_MODEL_UNAVAILABLE: profile {0} has no primary model")]
    ModelRequired(String),
    #[error("VCTR_POLICY_DENIED: profile {0} requires an explicit YOLO run grant")]
    YoloGrantRequired(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn yolo_does_not_override_hard_floor() {
        let profile = Profile {
            harness: HarnessId::Pi,
            provider: "local".into(),
            models: ModelRoles {
                primary: "qwen".into(),
                ..Default::default()
            },
            policy: BTreeMap::from([
                (
                    Capability::from(FILESYSTEM_EXTERNAL_WRITE),
                    PolicyDecision::Allow,
                ),
                (Capability::from(PROCESS_EXECUTE), PolicyDecision::Prompt),
            ]),
            isolation: IsolationConfig::default(),
            packs: vec![],
            verification: VerificationConfig::default(),
            context: ContextConfig::default(),
            native: json!({}),
            yolo: true,
        };
        let policy = resolve_policy(&profile, RepositoryTrust::Executable, true);
        assert_eq!(
            policy[&Capability::from(FILESYSTEM_EXTERNAL_WRITE)].effective,
            PolicyDecision::Deny
        );
        assert_eq!(
            policy[&Capability::from(PROCESS_EXECUTE)].effective,
            PolicyDecision::Allow
        );
    }

    #[test]
    fn layers_record_provenance_and_resolve_deterministically() {
        let dir = tempdir().unwrap();
        let config = starter_workspace("qwen-test", None, false);
        write_workspace_atomic(dir.path(), &config).unwrap();
        let first = resolve_run(dir.path(), Some("pi-safe"), false).unwrap();
        let second = resolve_run(dir.path(), Some("pi-safe"), false).unwrap();
        assert_eq!(first.models.primary, second.models.primary);
        assert_eq!(first.fingerprint().unwrap(), second.fingerprint().unwrap());
        assert_eq!(
            first.capabilities.keys().collect::<Vec<_>>(),
            second.capabilities.keys().collect::<Vec<_>>()
        );
        assert!(
            first
                .provenance
                .values()
                .any(|source| source == "repository configuration")
        );
    }

    #[test]
    fn committed_raw_secrets_are_rejected() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".vector")).unwrap();
        fs::write(
            dir.path().join(".vector/vector.yaml"),
            "apiVersion: vector.dev/v1alpha1\nkind: VectorWorkspace\napiKey: raw-secret\n",
        )
        .unwrap();
        assert!(matches!(
            load_workspace(dir.path()),
            Err(ConfigError::RawSecret(_))
        ));
    }

    #[test]
    fn repository_cannot_mark_itself_trusted() {
        let dir = tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".vector")).unwrap();
        fs::write(
            dir.path().join(".vector/vector.yaml"),
            "apiVersion: vector.dev/v1alpha1\nkind: VectorWorkspace\ntrust: publisher-trusted\n",
        )
        .unwrap();
        assert_eq!(
            load_workspace(dir.path()).unwrap().config.trust,
            RepositoryTrust::Untrusted
        );
    }

    #[test]
    fn computer_use_is_enabled_only_after_verified_atomic_update() {
        let dir = tempdir().unwrap();
        let config = starter_workspace("qwen-text", None, false);
        write_workspace_atomic(dir.path(), &config).unwrap();
        enable_verified_computer_use(dir.path(), HarnessId::Pi, "qwen-vision").unwrap();
        let resolved = load_workspace(dir.path()).unwrap();
        for name in ["pi-safe", "pi-yolo"] {
            let profile = &resolved.config.profiles[name];
            assert_eq!(profile.models.vision.as_deref(), Some("qwen-vision"));
            assert_eq!(
                profile.policy[&Capability::from(COMPUTER_INSPECT)],
                PolicyDecision::Allow
            );
            assert_eq!(
                profile.policy[&Capability::from(COMPUTER_CONTROL)],
                PolicyDecision::Prompt
            );
            assert!(
                profile
                    .packs
                    .iter()
                    .any(|pack| pack.id == "vector/computer-use")
            );
        }
        assert_eq!(resolved.config.profiles["omp-safe"].models.vision, None);
    }
}
