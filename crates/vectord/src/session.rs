use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;
use vector_core::{
    HarnessId, InteractiveSessionState, LaunchSurface, RunStartRequest, SessionEvent, SessionPhase,
};
use vector_harness::{EnvironmentValue, NativeRunPlan};
use vector_runtime::{PreparedRun, ready_state, substitute};

#[derive(Clone, Default)]
pub struct SessionManager {
    sessions: Arc<RwLock<HashMap<Uuid, ManagedSession>>>,
}

#[derive(Clone)]
struct ManagedSession {
    state: Arc<RwLock<InteractiveSessionState>>,
    events: Arc<RwLock<Vec<SessionEvent>>>,
    stdin: Option<Arc<Mutex<ChildStdin>>>,
    child: Arc<Mutex<Child>>,
    run_dir: PathBuf,
}

impl SessionManager {
    pub async fn start(
        &self,
        request: &RunStartRequest,
        prepared: PreparedRun,
    ) -> Result<InteractiveSessionState> {
        let run_id = prepared.ledger.manifest.id;
        let harness = prepared.plan.harness;
        let run_dir = prepared.ledger.dir.clone();
        if request.surface == LaunchSurface::Native {
            let child = open_native_terminal(&prepared.plan, &prepared.overlay, &run_dir).await?;
            let state = ready_state(run_id, harness, request.surface);
            let session = ManagedSession {
                state: Arc::new(RwLock::new(state.clone())),
                events: Arc::new(RwLock::new(vec![])),
                stdin: None,
                child: Arc::new(Mutex::new(child)),
                run_dir,
            };
            self.sessions.write().await.insert(run_id, session.clone());
            append_event(
                &session,
                "lifecycle.native-opened",
                json!({"surface":"native"}),
            )
            .await?;
            return Ok(state);
        }

        let mut command = command_for_plan(&prepared.plan, &prepared.overlay, &run_dir)?;
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command
            .spawn()
            .context("VCTR_HARNESS_INCOMPATIBLE: integrated harness failed to start")?;
        let stdin = Arc::new(Mutex::new(
            child.stdin.take().context("harness stdin unavailable")?,
        ));
        let stdout = child.stdout.take().context("harness stdout unavailable")?;
        let state = Arc::new(RwLock::new(InteractiveSessionState {
            run_id,
            harness,
            surface: request.surface,
            phase: if harness == HarnessId::Omp {
                SessionPhase::Preparing
            } else {
                SessionPhase::Ready
            },
            native_session_id: None,
            next_sequence: 1,
        }));
        let session = ManagedSession {
            state: state.clone(),
            events: Arc::new(RwLock::new(vec![])),
            stdin: Some(stdin.clone()),
            child: Arc::new(Mutex::new(child)),
            run_dir,
        };
        self.sessions.write().await.insert(run_id, session.clone());
        append_event(
            &session,
            "lifecycle.started",
            json!({"surface":"integrated","harness":harness}),
        )
        .await?;
        if harness == HarnessId::Omp {
            send(&stdin, json!({
                "jsonrpc":"2.0","id":1,"method":"initialize",
                "params":{"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"Vector","version":"0.1.0"}}
            })).await?;
        }
        tokio::spawn(observe_stdout(session.clone(), stdout));
        if let Some(prompt) = request.prompt.as_deref() {
            self.prompt(run_id, prompt).await?;
        }
        Ok(state.read().await.clone())
    }

    pub async fn prompt(&self, run_id: Uuid, prompt: &str) -> Result<InteractiveSessionState> {
        if prompt.trim().is_empty() {
            bail!("VCTR_CONFIG_INVALID: prompt cannot be empty");
        }
        let session = self.session(run_id).await?;
        let stdin = session
            .stdin
            .as_ref()
            .context("VCTR_RUN_FAILED: native terminal input stays in Terminal.app")?;
        let state = session.state.read().await.clone();
        match state.harness {
            HarnessId::Pi => {
                send(
                    stdin,
                    json!({"id":Uuid::now_v7().to_string(),"type":"prompt","message":prompt}),
                )
                .await?
            }
            HarnessId::Omp => {
                let native_id = state
                    .native_session_id
                    .context("VCTR_RUN_FAILED: OMP ACP session is still initializing")?;
                send(
                    stdin,
                    json!({
                        "jsonrpc":"2.0","id":3,"method":"session/prompt",
                        "params":{"sessionId":native_id,"prompt":[{"type":"text","text":prompt}]}
                    }),
                )
                .await?;
            }
            HarnessId::Deepseek => {
                bail!("VCTR_HARNESS_INCOMPATIBLE: integrated DeepSeek preview is not enabled")
            }
        }
        session.state.write().await.phase = SessionPhase::Streaming;
        append_event(&session, "prompt.submitted", json!({"text":prompt})).await?;
        Ok(session.state.read().await.clone())
    }

    pub async fn steer(&self, run_id: Uuid, message: &str) -> Result<InteractiveSessionState> {
        let session = self.session(run_id).await?;
        let stdin = session
            .stdin
            .as_ref()
            .context("VCTR_RUN_FAILED: native terminal steering stays in Terminal.app")?;
        match session.state.read().await.harness {
            HarnessId::Pi => {
                send(
                    stdin,
                    json!({"id":Uuid::now_v7().to_string(),"type":"steer","message":message}),
                )
                .await?
            }
            _ => bail!(
                "VCTR_CAPABILITY_UNSATISFIED: steering is currently available for Pi RPC sessions"
            ),
        }
        append_event(&session, "prompt.steered", json!({"text":message})).await?;
        Ok(session.state.read().await.clone())
    }

    pub async fn abort(&self, run_id: Uuid) -> Result<InteractiveSessionState> {
        let session = self.session(run_id).await?;
        let stdin = session
            .stdin
            .as_ref()
            .context("VCTR_RUN_FAILED: native terminal abort stays in Terminal.app")?;
        let state = session.state.read().await.clone();
        match state.harness {
            HarnessId::Pi => {
                send(
                    stdin,
                    json!({"id":Uuid::now_v7().to_string(),"type":"abort"}),
                )
                .await?
            }
            HarnessId::Omp => {
                let id = state
                    .native_session_id
                    .context("OMP session is not initialized")?;
                send(
                    stdin,
                    json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":id}}),
                )
                .await?;
            }
            HarnessId::Deepseek => bail!("DeepSeek integrated abort is unavailable"),
        }
        session.state.write().await.phase = SessionPhase::Ready;
        append_event(&session, "lifecycle.aborted", json!({})).await?;
        Ok(session.state.read().await.clone())
    }

    pub async fn stop(&self, run_id: Uuid) -> Result<InteractiveSessionState> {
        let session = self.session(run_id).await?;
        session.state.write().await.phase = SessionPhase::Stopping;
        session
            .child
            .lock()
            .await
            .start_kill()
            .context("could not stop harness")?;
        session.state.write().await.phase = SessionPhase::Completed;
        append_event(&session, "lifecycle.stopped", json!({})).await?;
        Ok(session.state.read().await.clone())
    }

    pub async fn approval(
        &self,
        run_id: Uuid,
        request_id: Value,
        option_id: Value,
    ) -> Result<InteractiveSessionState> {
        let session = self.session(run_id).await?;
        let stdin = session
            .stdin
            .as_ref()
            .context("approval channel unavailable")?;
        send(
            stdin,
            json!({
                "jsonrpc":"2.0","id":request_id,
                "result":{"outcome":{"outcome":"selected","optionId":option_id}}
            }),
        )
        .await?;
        session.state.write().await.phase = SessionPhase::Streaming;
        append_event(
            &session,
            "approval.responded",
            json!({"requestId":request_id,"optionId":option_id}),
        )
        .await?;
        Ok(session.state.read().await.clone())
    }

    pub async fn snapshot(&self, run_id: Uuid, after: u64) -> Result<Vec<SessionEvent>> {
        let session = self.session(run_id).await?;
        Ok(session
            .events
            .read()
            .await
            .iter()
            .filter(|event| event.sequence > after)
            .cloned()
            .collect())
    }

    async fn session(&self, run_id: Uuid) -> Result<ManagedSession> {
        self.sessions
            .read()
            .await
            .get(&run_id)
            .cloned()
            .context("VCTR_RUN_NOT_FOUND: interactive session is not active")
    }
}

async fn observe_stdout(session: ManagedSession, stdout: tokio::process::ChildStdout) {
    let mut reader = BufReader::new(stdout);
    let mut buffer = Vec::new();
    loop {
        buffer.clear();
        let Ok(read) = reader.read_until(b'\n', &mut buffer).await else {
            break;
        };
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
        let state = session.state.read().await.clone();
        if state.harness == HarnessId::Omp && value.get("id").and_then(Value::as_i64) == Some(1) {
            if let Some(stdin) = &session.stdin {
                let cwd = std::fs::read_to_string(session.run_dir.join("runspec.sanitized.json"))
                    .ok()
                    .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                    .and_then(|spec| {
                        spec.pointer("/workspace/root")
                            .and_then(Value::as_str)
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| ".".into());
                let _ = send(stdin, json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":cwd,"mcpServers":[]}})).await;
            }
            continue;
        }
        if state.harness == HarnessId::Omp && value.get("id").and_then(Value::as_i64) == Some(2) {
            if let Some(id) = value.pointer("/result/sessionId").and_then(Value::as_str) {
                let mut current = session.state.write().await;
                current.native_session_id = Some(id.into());
                drop(current);
                if let Some(stdin) = &session.stdin {
                    let model =
                        std::fs::read_to_string(session.run_dir.join("runspec.sanitized.json"))
                            .ok()
                            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                            .and_then(|spec| {
                                spec.pointer("/models/primary")
                                    .and_then(Value::as_str)
                                    .map(str::to_owned)
                            });
                    if let Some(model) = model {
                        let _ = send(stdin, json!({
                            "jsonrpc":"2.0","id":4,"method":"session/set_config_option",
                            "params":{"sessionId":id,"configId":"model","value":format!("lm-studio/{model}")}
                        })).await;
                    }
                }
            }
        }
        if state.harness == HarnessId::Omp && value.get("id").and_then(Value::as_i64) == Some(4) {
            session.state.write().await.phase = if value.get("error").is_some() {
                SessionPhase::Failed
            } else {
                SessionPhase::Ready
            };
        }
        if value.get("method").and_then(Value::as_str) == Some("session/request_permission") {
            session.state.write().await.phase = SessionPhase::WaitingForApproval;
        }
        if state.harness == HarnessId::Pi
            && value.get("type").and_then(Value::as_str) == Some("agent_end")
        {
            session.state.write().await.phase = SessionPhase::Ready;
        }
        let kind = portable_kind(&value);
        let _ = append_event(&session, kind, value).await;
    }
    if session.state.read().await.phase != SessionPhase::Completed {
        session.state.write().await.phase = SessionPhase::Failed;
        let _ = append_event(&session, "lifecycle.exited", json!({"unexpected":true})).await;
    }
}

fn portable_kind(value: &Value) -> &'static str {
    let encoded = value.to_string();
    if value.get("method").and_then(Value::as_str) == Some("session/request_permission") {
        "approval.requested"
    } else if encoded.contains("tool_call")
        || encoded.contains("toolCall")
        || encoded.contains("tool_execution")
    {
        "tool.native"
    } else if encoded.contains("message") || encoded.contains("text") || encoded.contains("delta") {
        "message.delta"
    } else {
        "harness.event"
    }
}

async fn append_event(
    session: &ManagedSession,
    kind: &str,
    payload: Value,
) -> Result<SessionEvent> {
    let sequence = {
        let mut state = session.state.write().await;
        let sequence = state.next_sequence;
        state.next_sequence += 1;
        sequence
    };
    let event = SessionEvent {
        run_id: session.state.read().await.run_id,
        sequence,
        kind: kind.into(),
        occurred_at: chrono::Utc::now(),
        payload,
        native_ref: None,
    };
    session.events.write().await.push(event.clone());
    let mut bytes = serde_json::to_vec(&event)?;
    bytes.push(b'\n');
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(session.run_dir.join("events.jsonl"))
        .await?;
    file.write_all(&bytes).await?;
    file.flush().await?;
    Ok(event)
}

async fn send(stdin: &Arc<Mutex<ChildStdin>>, value: Value) -> Result<()> {
    let mut bytes = serde_json::to_vec(&value)?;
    bytes.push(b'\n');
    let mut stdin = stdin.lock().await;
    stdin.write_all(&bytes).await?;
    stdin.flush().await?;
    Ok(())
}

fn command_for_plan(plan: &NativeRunPlan, overlay: &Path, run_dir: &Path) -> Result<Command> {
    let mut command = Command::new(&plan.executable);
    command
        .args(
            plan.argv
                .iter()
                .map(|arg| substitute(arg, overlay, run_dir)),
        )
        .current_dir(&plan.cwd);
    for (name, value) in &plan.environment {
        match value {
            EnvironmentValue::Literal(value) => {
                command.env(name, substitute(value, overlay, run_dir));
            }
            EnvironmentValue::SecretReference(reference) => {
                bail!("VCTR_SECRET_UNAVAILABLE: {reference}")
            }
        }
    }
    Ok(command)
}

async fn open_native_terminal(
    plan: &NativeRunPlan,
    overlay: &Path,
    run_dir: &Path,
) -> Result<Child> {
    #[cfg(not(target_os = "macos"))]
    bail!(
        "VCTR_RUNTIME_UNAVAILABLE: external native terminal launch is currently implemented on macOS"
    );
    #[cfg(target_os = "macos")]
    {
        let launcher = run_dir.join("native/launch.command");
        let mut lines = vec![
            "#!/bin/zsh".into(),
            "set -eu".into(),
            format!("cd -- {}", shell_quote(&plan.cwd)),
        ];
        for (name, value) in &plan.environment {
            if let EnvironmentValue::Literal(value) = value {
                lines.push(format!(
                    "export {}={}",
                    name,
                    shell_quote(&substitute(value, overlay, run_dir))
                ));
            }
        }
        let mut argv = vec![shell_quote(&plan.executable)];
        argv.extend(
            plan.argv
                .iter()
                .map(|arg| shell_quote(&substitute(arg, overlay, run_dir))),
        );
        lines.push(format!("exec {}", argv.join(" ")));
        tokio::fs::write(&launcher, format!("{}\n", lines.join("\n"))).await?;
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&launcher, std::fs::Permissions::from_mode(0o700)).await?;
        Command::new("open")
            .args(["-a", "Terminal", launcher.to_string_lossy().as_ref()])
            .spawn()
            .context("could not open Terminal.app")
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
