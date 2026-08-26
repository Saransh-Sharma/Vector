//! Shared preflight, smoke, and run preparation for every Vector client.

pub mod workbench;

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use base64::Engine;
use serde_json::{Value, json};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::time::{Instant, timeout};
use vector_config::{enable_verified_computer_use, load_workspace, resolve_run};
use vector_core::capabilities::FILESYSTEM_EXTERNAL_WRITE;
use vector_core::{
    Capability, ComputerUseVerificationReport, ComputerUseVerificationRequest, Diagnostic,
    HarnessId, InteractiveSessionState, LaunchSurface, PolicyDecision, PreflightCheck,
    PreflightReport, SessionPhase, SmokeTestReport, application_paths,
};
use vector_db::RunLedger;
use vector_harness::{
    EnvironmentValue, NativeRunPlan, bind_installation, compile_for_surface, inspect_harness,
};
use vector_providers::ProviderDiscovery;

const SMOKE_TIMEOUT: Duration = Duration::from_secs(300);
const SMOKE_MARKER: &str = "VECTOR_SMOKE_MARKER_7D4E8B19";
const SMOKE_CONFORMANCE_VERSION: u64 = 2;
const COMPUTER_NONCE: &str = "VECTOR CLICK 7D4E";

pub struct PreparedRun {
    pub plan: NativeRunPlan,
    pub ledger: RunLedger,
    pub overlay: PathBuf,
}

pub async fn preflight(workspace: &Path, profile: &str) -> Result<PreflightReport, RuntimeError> {
    let workspace = workspace.canonicalize().map_err(RuntimeError::Io)?;
    let spec = resolve_profile(&workspace, profile)?;
    let harness = inspect_harness(spec.harness.harness).await;
    let mut checks = vec![check(
        "configuration",
        "Configuration saved",
        true,
        format!("Resolved profile {profile}."),
        None,
    )];
    checks.push(check(
        "harness",
        "Harness installation verified",
        harness.ready,
        harness.notes.join(" "),
        (!harness.ready).then(|| "Install the isolated managed harness or switch profiles.".into()),
    ));

    let discovery = ProviderDiscovery::new()?;
    let provider_result = discovery.ensure_lm_studio(&spec.provider.base_url).await;
    let (provider_ok, provider_detail, model_ok, model_detail) = match provider_result {
        Ok(provider) => {
            let model_ok = provider
                .models
                .iter()
                .any(|model| model.id == spec.models.primary);
            (
                true,
                format!("LM Studio responded in {} ms.", provider.latency_ms),
                model_ok,
                if model_ok {
                    format!("Exact model {} is available.", spec.models.primary)
                } else {
                    format!("LM Studio did not report {}.", spec.models.primary)
                },
            )
        }
        Err(error) => (
            false,
            error.to_string(),
            false,
            "Model inventory was unavailable.".into(),
        ),
    };
    checks.push(check(
        "provider",
        "LM Studio reachable",
        provider_ok,
        provider_detail,
        (!provider_ok).then(|| "Start LM Studio; Vector will also try its official CLI.".into()),
    ));
    checks.push(check(
        "model",
        "Configured model available",
        model_ok,
        model_detail,
        (!model_ok).then(|| "Load the exact configured model or select another model.".into()),
    ));
    let trust_ok = !matches!(
        spec.workspace.trust,
        vector_core::RepositoryTrust::Untrusted
    );
    checks.push(check(
        "trust",
        "Workspace trusted for execution",
        trust_ok,
        format!("Trust state is {:?}.", spec.workspace.trust),
        (!trust_ok).then(|| "Grant machine-local executable trust before launching tools.".into()),
    ));
    let ready_for_smoke = checks.iter().all(|item| item.passed);
    let fingerprint = spec.fingerprint()?;
    let smoke_passed = read_onboarding_state(&workspace)
        .await
        .is_some_and(|state| {
            let current = state.get("smokes").and_then(|smokes| smokes.get(profile));
            current.is_some_and(|entry| {
                entry.get("fingerprint").and_then(Value::as_str) == Some(fingerprint.as_str())
                    && entry.get("passed").and_then(Value::as_bool) == Some(true)
                    && entry.get("conformanceVersion").and_then(Value::as_u64)
                        == Some(SMOKE_CONFORMANCE_VERSION)
            })
        });
    Ok(PreflightReport {
        workspace,
        profile: profile.into(),
        harness,
        checks,
        ready_for_smoke,
        smoke_passed,
        ready_to_work: ready_for_smoke && smoke_passed,
    })
}

fn check(
    id: &str,
    label: &str,
    passed: bool,
    detail: String,
    remediation: Option<String>,
) -> PreflightCheck {
    PreflightCheck {
        id: id.into(),
        label: label.into(),
        passed,
        detail,
        remediation,
    }
}

fn resolve_profile(
    workspace: &Path,
    profile: &str,
) -> Result<vector_core::PortableRunSpec, RuntimeError> {
    let resolved = load_workspace(workspace)?;
    let yolo = resolved
        .config
        .profiles
        .get(profile)
        .is_some_and(|value| value.yolo);
    Ok(resolve_run(workspace, Some(profile), yolo)?)
}

pub async fn prepare_run(
    workspace: &Path,
    profile: &str,
    surface: LaunchSurface,
    grant_yolo: bool,
) -> Result<PreparedRun, RuntimeError> {
    let workspace = workspace.canonicalize().map_err(RuntimeError::Io)?;
    let spec = resolve_run(&workspace, Some(profile), grant_yolo)?;
    let installation = inspect_harness(spec.harness.harness).await;
    if !installation.ready {
        return Err(RuntimeError::HarnessUnverified(
            installation.notes.join(" "),
        ));
    }
    let mut plan = compile_for_surface(&spec, surface)?;
    bind_installation(&mut plan, &installation)?;
    let paths = application_paths().ok_or(RuntimeError::ApplicationPaths)?;
    let mut ledger = RunLedger::create(&paths.data_dir.join("runs"), &spec).await?;
    let overlay = ledger.dir.join("generated");
    materialize(&plan, &overlay, &ledger.dir).await?;
    ledger
        .append(
            "run.prepared",
            json!({
                "surface": surface,
                "plan": plan,
                "fingerprint": spec.fingerprint()?,
                "installation": installation,
            }),
        )
        .await?;
    Ok(PreparedRun {
        plan,
        ledger,
        overlay,
    })
}

pub async fn smoke_test(workspace: &Path, profile: &str) -> Result<SmokeTestReport, RuntimeError> {
    let report = preflight(workspace, profile).await?;
    if !report.ready_for_smoke {
        return Err(RuntimeError::PreflightFailed(Box::new(report)));
    }
    let spec = resolve_profile(workspace, profile)?;
    let fixture = tempfile::Builder::new().prefix("vector-smoke-").tempdir()?;
    let marker_path = fixture.path().join("marker.txt");
    tokio::fs::write(&marker_path, format!("{SMOKE_MARKER}\n")).await?;
    let before = digest_tree(fixture.path()).await?;

    let mut smoke_spec = spec.clone();
    smoke_spec.workspace.root = fixture.path().to_path_buf();
    smoke_spec.workspace.git_commit = None;
    smoke_spec.workspace.git_dirty = false;
    let installation = inspect_harness(smoke_spec.harness.harness).await;
    let mut plan = compile_for_surface(&smoke_spec, LaunchSurface::Integrated)?;
    bind_installation(&mut plan, &installation)?;
    let overlay_owner = tempfile::Builder::new()
        .prefix("vector-smoke-overlay-")
        .tempdir()?;
    let overlay = overlay_owner.path().join("generated");
    let run_dir = overlay_owner.path().join("run");
    tokio::fs::create_dir_all(&run_dir).await?;
    materialize(&plan, &overlay, &run_dir).await?;

    let prompt = format!(
        "Use your native file-reading tool to read marker.txt in the current workspace. Reply with exactly {SMOKE_MARKER}. Do not write or modify any file.\nVECTOR_MODEL={}",
        smoke_spec.models.primary,
    );
    let outcome = timeout(
        SMOKE_TIMEOUT,
        run_structured_smoke(&plan, &overlay, &run_dir, fixture.path(), &prompt),
    )
    .await
    .map_err(|_| RuntimeError::SmokeTimeout)??;
    let after = digest_tree(fixture.path()).await?;
    let external_deny = smoke_spec
        .capabilities
        .get(&Capability::from(FILESYSTEM_EXTERNAL_WRITE))
        .is_some_and(|decision| decision.effective == PolicyDecision::Deny);
    let passed = outcome.model_streamed
        && outcome.tool_observed
        && outcome.marker_observed
        && !outcome.protocol_error
        && external_deny
        && before == after;
    let report = SmokeTestReport {
        passed,
        harness: smoke_spec.harness.harness,
        profile: profile.into(),
        fixture_digest_before: before,
        fixture_digest_after: after,
        model_streamed: outcome.model_streamed,
        tool_observed: outcome.tool_observed,
        policy_denial_observed: external_deny,
        events: outcome.events,
        diagnostic: (!passed).then(|| Diagnostic::error(
            "VCTR_SMOKE_FAILED",
            "Disposable harness smoke test did not pass",
            "The harness must stream the marker through its structured protocol, use a native tool, preserve the fixture, and retain the external-write hard deny.",
        )),
    };
    if report.passed {
        write_onboarding_smoke_state(workspace, profile, &spec.fingerprint()?).await?;
    }
    Ok(report)
}

pub async fn verify_computer_use(
    request: &ComputerUseVerificationRequest,
) -> Result<ComputerUseVerificationReport, RuntimeError> {
    let workspace = request.workspace.canonicalize().map_err(RuntimeError::Io)?;
    let spec = resolve_profile(&workspace, &request.profile)?;
    let inventory = ProviderDiscovery::new()?
        .ensure_lm_studio(&spec.provider.base_url)
        .await?;
    if !inventory
        .models
        .iter()
        .any(|model| model.id == request.vision_model)
    {
        return Err(RuntimeError::Unsupported(format!(
            "vision-role model {} is not currently reported by LM Studio",
            request.vision_model
        )));
    }

    #[cfg(not(target_os = "macos"))]
    return Err(RuntimeError::Unsupported(
        "the computer-use verification helper is currently available on macOS".into(),
    ));

    #[cfg(target_os = "macos")]
    {
        let paths = application_paths().ok_or(RuntimeError::ApplicationPaths)?;
        let run_dir = paths
            .data_dir
            .join("computer-verification")
            .join(uuid::Uuid::now_v7().to_string());
        tokio::fs::create_dir_all(&run_dir).await?;
        let mut helper = ComputerHelper::start(&workspace, &run_dir).await?;
        let mut status = helper.call("status", json!({})).await?;
        let mut accessibility = status
            .pointer("/result/accessibility")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut screen_recording = status
            .pointer("/result/screenCapture")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if request.request_permissions && (!accessibility || !screen_recording) {
            let _ = helper.call("request-permissions", json!({})).await?;
            status = helper.call("status", json!({})).await?;
            accessibility = status
                .pointer("/result/accessibility")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            screen_recording = status
                .pointer("/result/screenCapture")
                .and_then(Value::as_bool)
                .unwrap_or(false);
        }

        let mut checks = vec![
            check(
                "screen-recording",
                "Screen Recording permission",
                screen_recording,
                if screen_recording {
                    "Granted to the Vector computer helper.".into()
                } else {
                    "Required to capture the local verification fixture.".into()
                },
                (!screen_recording).then(|| {
                    "Choose Grant permissions, enable Screen Recording in System Settings, then retry.".into()
                }),
            ),
            check(
                "accessibility",
                "Accessibility permission",
                accessibility,
                if accessibility {
                    "Granted to the Vector computer helper.".into()
                } else {
                    "Required to perform the controlled fixture click.".into()
                },
                (!accessibility).then(|| {
                    "Choose Grant permissions, enable Accessibility in System Settings, then retry.".into()
                }),
            ),
        ];
        if !accessibility || !screen_recording {
            return Ok(ComputerUseVerificationReport {
                vision_model: request.vision_model.clone(),
                vision_probe_passed: false,
                screen_recording,
                accessibility,
                fixture_identified: false,
                fixture_clicked: false,
                screenshot_path: None,
                enabled: false,
                checks,
            });
        }

        let screenshot_path = run_dir.join("fixture.png");
        let fixture = helper
            .call("fixture-open", json!({ "path": screenshot_path }))
            .await?;
        let x = fixture
            .pointer("/result/x")
            .and_then(Value::as_f64)
            .ok_or_else(|| RuntimeError::Helper("fixture did not return x".into()))?;
        let y = fixture
            .pointer("/result/y")
            .and_then(Value::as_f64)
            .ok_or_else(|| RuntimeError::Helper("fixture did not return y".into()))?;
        let image = tokio::fs::read(&screenshot_path).await?;
        let vision_text =
            probe_vision_model(&spec.provider.base_url, &request.vision_model, &image).await?;
        let fixture_identified = vision_text.to_ascii_uppercase().contains(COMPUTER_NONCE);
        checks.push(check(
            "vision-nonce",
            "Vision-role nonce probe",
            fixture_identified,
            if fixture_identified {
                format!("{} read the target label from pixels.", request.vision_model)
            } else {
                "The model did not return the nonce shown in the fixture screenshot.".into()
            },
            (!fixture_identified).then(|| {
                "Select a loaded vision-capable model; Vector does not infer vision support from its name.".into()
            }),
        ));
        let fixture_clicked = if fixture_identified {
            let clicked = helper.call("click", json!({ "x": x, "y": y })).await?;
            clicked
                .pointer("/result/fixtureActivated")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        } else {
            false
        };
        let _ = helper.call("fixture-close", json!({})).await;
        checks.push(check(
            "control-fixture",
            "Local click fixture",
            fixture_clicked,
            if fixture_clicked {
                "The helper clicked the isolated Vector target and observed activation.".into()
            } else {
                "The controlled click was not confirmed.".into()
            },
            (!fixture_clicked)
                .then(|| "Verify Accessibility permission and retry the local fixture.".into()),
        ));
        let enabled = fixture_identified && fixture_clicked;
        if enabled {
            enable_verified_computer_use(&workspace, spec.harness.harness, &request.vision_model)?;
        }
        Ok(ComputerUseVerificationReport {
            vision_model: request.vision_model.clone(),
            vision_probe_passed: fixture_identified,
            screen_recording,
            accessibility,
            fixture_identified,
            fixture_clicked,
            screenshot_path: Some(screenshot_path),
            enabled,
            checks,
        })
    }
}

#[cfg(target_os = "macos")]
struct ComputerHelper {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<tokio::process::ChildStdout>,
    grant: String,
}

#[cfg(target_os = "macos")]
impl ComputerHelper {
    async fn start(workspace: &Path, run_dir: &Path) -> Result<Self, RuntimeError> {
        let executable = ensure_computer_helper(workspace).await?;
        let grant = uuid::Uuid::new_v4().to_string();
        let mut child = Command::new(executable)
            .env("VECTOR_COMPUTER_GRANT", &grant)
            .env("VECTOR_RUN_DIR", run_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or(RuntimeError::MissingPipe("computer helper stdin"))?;
        let stdout = BufReader::new(
            child
                .stdout
                .take()
                .ok_or(RuntimeError::MissingPipe("computer helper stdout"))?,
        );
        Ok(Self {
            child,
            stdin,
            stdout,
            grant,
        })
    }

    async fn call(&mut self, action: &str, params: Value) -> Result<Value, RuntimeError> {
        let id = uuid::Uuid::new_v4().to_string();
        let mut bytes = serde_json::to_vec(&json!({
            "protocolVersion": "1.0",
            "id": id,
            "grant": self.grant,
            "action": action,
            "params": params,
        }))?;
        bytes.push(b'\n');
        self.stdin.write_all(&bytes).await?;
        self.stdin.flush().await?;
        let mut line = String::new();
        timeout(Duration::from_secs(20), self.stdout.read_line(&mut line))
            .await
            .map_err(|_| {
                RuntimeError::Helper(format!("computer helper timed out during {action}"))
            })??;
        let value: Value = serde_json::from_str(&line)?;
        if value.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(RuntimeError::Helper(
                value
                    .pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("computer helper rejected the request")
                    .into(),
            ));
        }
        Ok(value)
    }
}

#[cfg(target_os = "macos")]
impl Drop for ComputerHelper {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

#[cfg(target_os = "macos")]
async fn ensure_computer_helper(workspace: &Path) -> Result<PathBuf, RuntimeError> {
    if let Some(path) = std::env::var_os("VECTOR_COMPUTER_HELPER")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
    {
        return Ok(path);
    }
    let package = workspace.join("helpers/macos/VectorComputerHelper");
    let executable = package.join(".build/release/vector-computer-helper");
    if executable.is_file() {
        return Ok(executable);
    }
    if !package.join("Package.swift").is_file() {
        return Err(RuntimeError::Unsupported(
            "the packaged macOS computer helper is not installed".into(),
        ));
    }
    let output = Command::new("swift")
        .args(["build", "-c", "release", "--package-path"])
        .arg(&package)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await?;
    if !output.status.success() {
        return Err(RuntimeError::Helper(
            String::from_utf8_lossy(&output.stderr).trim().into(),
        ));
    }
    Ok(executable)
}

async fn probe_vision_model(
    base_url: &str,
    model: &str,
    png: &[u8],
) -> Result<String, RuntimeError> {
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let encoded = base64::engine::general_purpose::STANDARD.encode(png);
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .no_proxy()
        .build()?
        .post(url)
        .json(&json!({
            "model": model,
            "temperature": 0,
            "max_tokens": 32,
            "messages": [{"role":"user","content":[
                {"type":"text","text":"Read the exact label on the single button. Reply with only that label."},
                {"type":"image_url","image_url":{"url":format!("data:image/png;base64,{encoded}")}}
            ]}]
        }))
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    Ok(response
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .into())
}

async fn read_onboarding_state(workspace: &Path) -> Option<Value> {
    let bytes = tokio::fs::read(workspace.join(".vector/onboarding.local.json"))
        .await
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

async fn write_onboarding_smoke_state(
    workspace: &Path,
    profile: &str,
    fingerprint: &str,
) -> Result<(), RuntimeError> {
    let mut value = read_onboarding_state(workspace)
        .await
        .unwrap_or_else(|| json!({}));
    let root = value
        .as_object_mut()
        .ok_or_else(|| RuntimeError::Helper("onboarding state is not an object".into()))?;
    let smokes = root.entry("smokes").or_insert_with(|| json!({}));
    let smokes = smokes
        .as_object_mut()
        .ok_or_else(|| RuntimeError::Helper("onboarding smoke state is not an object".into()))?;
    smokes.insert(
        profile.into(),
        json!({
            "fingerprint": fingerprint,
            "passed": true,
            "conformanceVersion": SMOKE_CONFORMANCE_VERSION,
            "completedAt": chrono_like_now(),
        }),
    );
    let path = workspace.join(".vector/onboarding.local.json");
    let temp = workspace.join(".vector/onboarding.local.tmp");
    tokio::fs::create_dir_all(workspace.join(".vector")).await?;
    tokio::fs::write(&temp, serde_json::to_vec_pretty(&value)?).await?;
    tokio::fs::rename(temp, path).await?;
    Ok(())
}

fn chrono_like_now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs().to_string())
        .unwrap_or_else(|_| "0".into())
}

#[derive(Default)]
struct SmokeOutcome {
    model_streamed: bool,
    tool_observed: bool,
    marker_observed: bool,
    protocol_error: bool,
    events: Vec<String>,
}

async fn run_structured_smoke(
    plan: &NativeRunPlan,
    overlay: &Path,
    run_dir: &Path,
    cwd: &Path,
    prompt: &str,
) -> Result<SmokeOutcome, RuntimeError> {
    let mut smoke_plan = plan.clone();
    if plan.harness == HarnessId::Omp {
        let model = marker_model_from_prompt(prompt);
        let user_prompt = prompt
            .split_once("\nVECTOR_MODEL=")
            .map(|(text, _)| text)
            .unwrap_or(prompt);
        let mut argv = Vec::new();
        if plan.argv.first().is_some_and(|arg| arg.ends_with(".js")) {
            argv.push(plan.argv[0].clone());
        }
        argv.extend([
            "launch".into(),
            "--mode".into(),
            "json".into(),
            "--model".into(),
            format!("lm-studio/{model}"),
            "--thinking".into(),
            "off".into(),
            "--max-time".into(),
            "240".into(),
            "--tools".into(),
            "read".into(),
            "--no-lsp".into(),
            "--no-pty".into(),
            "--no-extensions".into(),
            "--no-skills".into(),
            "--no-rules".into(),
            "--no-session".into(),
            "--approval-mode".into(),
            "always-ask".into(),
            "--print".into(),
            user_prompt.into(),
        ]);
        smoke_plan.argv = argv;
    }
    let mut child = spawn_plan(&smoke_plan, overlay, run_dir, cwd).await?;
    let stderr = child.stderr.take();
    let stderr_task = tokio::spawn(async move {
        let mut bytes = Vec::new();
        if let Some(mut stderr) = stderr {
            let _ = stderr.read_to_end(&mut bytes).await;
        }
        String::from_utf8_lossy(&bytes).trim().to_string()
    });
    let stdout = child
        .stdout
        .take()
        .ok_or(RuntimeError::MissingPipe("stdout"))?;
    let result = match plan.harness {
        HarnessId::Pi => {
            let stdin = child
                .stdin
                .take()
                .ok_or(RuntimeError::MissingPipe("stdin"))?;
            smoke_pi(stdin, stdout, prompt).await
        }
        HarnessId::Omp => smoke_omp_json(stdout).await,
        HarnessId::Deepseek => Err(RuntimeError::Unsupported(
            "DeepSeek smoke conformance is not enabled for the preview adapter".into(),
        )),
    };
    let _ = child.start_kill();
    let _ = child.wait().await;
    let stderr = stderr_task.await.unwrap_or_default();
    match result {
        Ok(outcome) if !stderr.is_empty() && !outcome.model_streamed => {
            Err(RuntimeError::HarnessProtocol(stderr))
        }
        other => other,
    }
}

async fn spawn_plan(
    plan: &NativeRunPlan,
    overlay: &Path,
    run_dir: &Path,
    cwd: &Path,
) -> Result<Child, RuntimeError> {
    let mut command = Command::new(&plan.executable);
    command
        .args(
            plan.argv
                .iter()
                .map(|arg| substitute(arg, overlay, run_dir)),
        )
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (name, value) in &plan.environment {
        match value {
            EnvironmentValue::Literal(value) => {
                command.env(name, substitute(value, overlay, run_dir));
            }
            EnvironmentValue::SecretReference(reference) => {
                return Err(RuntimeError::SecretUnavailable(reference.clone()));
            }
        }
    }
    command.spawn().map_err(RuntimeError::Io)
}

async fn smoke_pi(
    mut stdin: ChildStdin,
    stdout: tokio::process::ChildStdout,
    prompt: &str,
) -> Result<SmokeOutcome, RuntimeError> {
    send_line(
        &mut stdin,
        &json!({"id":"vector-smoke","type":"prompt","message":prompt}),
    )
    .await?;
    let mut reader = BufReader::new(stdout);
    let mut buffer = Vec::new();
    let mut outcome = SmokeOutcome::default();
    let started = Instant::now();
    while started.elapsed() < SMOKE_TIMEOUT {
        buffer.clear();
        let read = reader.read_until(b'\n', &mut buffer).await?;
        if read == 0 {
            break;
        }
        if buffer.last() == Some(&b'\n') {
            buffer.pop();
        }
        if buffer.last() == Some(&b'\r') {
            buffer.pop();
        }
        let Ok(value) = serde_json::from_slice::<Value>(&buffer) else {
            continue;
        };
        let encoded = value.to_string();
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("native");
        if kind.contains("tool") || encoded.contains("marker.txt") {
            outcome.tool_observed = true;
            outcome.events.push("tool".into());
        }
        let assistant = value.pointer("/message/role").and_then(Value::as_str) == Some("assistant");
        if (assistant && kind.contains("message")) || kind.contains("text_delta") {
            outcome.model_streamed = true;
        }
        if encoded.contains(SMOKE_MARKER) {
            outcome.marker_observed = true;
        }
        if value.pointer("/message/stopReason").and_then(Value::as_str) == Some("error")
            || value.pointer("/message/errorMessage").is_some()
        {
            outcome.protocol_error = true;
            outcome.events.push("model.error".into());
        }
        if kind == "response"
            && value.get("id").and_then(Value::as_str) == Some("vector-smoke")
            && value.get("success").and_then(Value::as_bool) == Some(false)
        {
            outcome.events.push("prompt.rejected".into());
            break;
        }
        if kind == "agent_end" {
            break;
        }
    }
    outcome.events.push("lifecycle.completed".into());
    Ok(outcome)
}

async fn smoke_omp_json(stdout: tokio::process::ChildStdout) -> Result<SmokeOutcome, RuntimeError> {
    let mut reader = BufReader::new(stdout);
    let mut buffer = Vec::new();
    let mut outcome = SmokeOutcome::default();
    let started = Instant::now();
    while started.elapsed() < SMOKE_TIMEOUT {
        buffer.clear();
        let read = reader.read_until(b'\n', &mut buffer).await?;
        if read == 0 {
            break;
        }
        if buffer.last() == Some(&b'\n') {
            buffer.pop();
        }
        if buffer.last() == Some(&b'\r') {
            buffer.pop();
        }
        let Ok(value) = serde_json::from_slice::<Value>(&buffer) else {
            continue;
        };
        let encoded = value.to_string();
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("native");
        if kind.contains("tool")
            || encoded.contains("tool_call")
            || encoded.contains("toolCall")
            || encoded.contains("marker.txt")
        {
            outcome.tool_observed = true;
            outcome.events.push("tool".into());
        }
        if kind.contains("message")
            || kind.contains("assistant")
            || kind.contains("text")
            || encoded.contains("message_update")
        {
            outcome.model_streamed = true;
        }
        if encoded.contains(SMOKE_MARKER) {
            outcome.marker_observed = true;
        }
        if value.get("error").is_some()
            || value.get("stopReason").and_then(Value::as_str) == Some("error")
        {
            outcome.protocol_error = true;
            outcome.events.push("model.error".into());
        }
    }
    outcome.events.push("lifecycle.completed".into());
    Ok(outcome)
}

fn marker_model_from_prompt(prompt: &str) -> &str {
    prompt
        .rsplit_once("\nVECTOR_MODEL=")
        .map(|(_, model)| model.trim())
        .unwrap_or("")
}

async fn send_line(stdin: &mut ChildStdin, value: &Value) -> Result<(), RuntimeError> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    stdin.write_all(&bytes).await?;
    stdin.flush().await?;
    Ok(())
}

pub async fn materialize(
    plan: &NativeRunPlan,
    overlay: &Path,
    run_dir: &Path,
) -> Result<(), RuntimeError> {
    for generated in &plan.generated_files {
        let relative = Path::new(&generated.relative_path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|part| part == Component::ParentDir)
        {
            return Err(RuntimeError::UnsafeGeneratedPath(
                generated.relative_path.clone(),
            ));
        }
        let path = overlay.join(relative);
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(path, substitute(&generated.content, overlay, run_dir)).await?;
    }
    Ok(())
}

pub fn substitute(value: &str, overlay: &Path, run_dir: &Path) -> String {
    value
        .replace("${VECTOR_OVERLAY}", &overlay.to_string_lossy())
        .replace("${VECTOR_RUN_DIR}", &run_dir.to_string_lossy())
}

async fn digest_tree(root: &Path) -> Result<String, RuntimeError> {
    let mut stack = vec![root.to_path_buf()];
    let mut entries = BTreeMap::new();
    while let Some(dir) = stack.pop() {
        let mut reader = tokio::fs::read_dir(&dir).await?;
        while let Some(entry) = reader.next_entry().await? {
            let path = entry.path();
            if entry.file_type().await?.is_dir() {
                stack.push(path);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();
                entries.insert(
                    relative,
                    blake3::hash(&tokio::fs::read(&path).await?)
                        .to_hex()
                        .to_string(),
                );
            }
        }
    }
    Ok(blake3::hash(&serde_json::to_vec(&entries)?)
        .to_hex()
        .to_string())
}

pub fn ready_state(
    run_id: uuid::Uuid,
    harness: HarnessId,
    surface: LaunchSurface,
) -> InteractiveSessionState {
    InteractiveSessionState {
        run_id,
        harness,
        surface,
        phase: SessionPhase::Ready,
        native_session_id: None,
        next_sequence: 1,
    }
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("VCTR_CONFIG_INVALID: {0}")]
    Config(#[from] vector_config::ConfigError),
    #[error("VCTR_HARNESS_INCOMPATIBLE: {0}")]
    Harness(#[from] vector_harness::HarnessError),
    #[error("VCTR_PROVIDER_UNAVAILABLE: {0}")]
    Provider(#[from] vector_providers::ProviderError),
    #[error("VCTR_RUN_FAILED: {0}")]
    Database(#[from] vector_db::DbError),
    #[error("VCTR_RUN_FAILED: filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("VCTR_RUN_FAILED: structured protocol error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("VCTR_RUNTIME_UNAVAILABLE: application paths are unavailable")]
    ApplicationPaths,
    #[error("VCTR_HARNESS_INCOMPATIBLE: {0}")]
    HarnessUnverified(String),
    #[error("VCTR_PREFLIGHT_FAILED: selected profile is not ready for a smoke test")]
    PreflightFailed(Box<PreflightReport>),
    #[error("VCTR_SMOKE_FAILED: structured smoke test timed out")]
    SmokeTimeout,
    #[error("VCTR_RUN_FAILED: harness process did not expose {0}")]
    MissingPipe(&'static str),
    #[error("VCTR_SECRET_UNAVAILABLE: secret materialization is unavailable for {0}")]
    SecretUnavailable(String),
    #[error("VCTR_POLICY_DENIED: generated path escaped the Vector overlay: {0}")]
    UnsafeGeneratedPath(String),
    #[error("VCTR_CAPABILITY_UNSATISFIED: {0}")]
    Unsupported(String),
    #[error("VCTR_HARNESS_INCOMPATIBLE: structured harness error: {0}")]
    HarnessProtocol(String),
    #[error("VCTR_COMPUTER_USE_FAILED: {0}")]
    Helper(String),
    #[error("VCTR_PROVIDER_UNAVAILABLE: HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("VCTR_RUN_FAILED: run-spec error: {0}")]
    Core(#[from] vector_core::CoreError),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn materialization_rejects_parent_paths() {
        let plan = NativeRunPlan {
            harness: HarnessId::Pi,
            executable: "pi".into(),
            argv: vec![],
            environment: BTreeMap::new(),
            cwd: ".".into(),
            generated_files: vec![vector_harness::GeneratedFile {
                relative_path: "../escape".into(),
                content: "x".into(),
                sensitive: false,
            }],
            observation: String::new(),
            cleanup: vec![],
            native_summary: String::new(),
        };
        let dir = tempfile::tempdir().unwrap();
        assert!(materialize(&plan, dir.path(), dir.path()).await.is_err());
    }
}
