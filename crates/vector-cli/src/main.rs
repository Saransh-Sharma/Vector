use std::fs;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Parser, Subcommand, ValueEnum};
use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use dialoguer::{Confirm, Select, theme::ColorfulTheme};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};
use serde_json::{Value, json};
use uuid::Uuid;
use vector_config::{load_workspace, resolve_run, starter_workspace, write_workspace_atomic};
use vector_core::{
    HarnessId, PROTOCOL_VERSION, RequestEnvelope, ResponseEnvelope, application_paths,
    runspec_schema, workspace_schema,
};
use vector_db::{RunLedger, VectorDatabase};
use vector_harness::{EnvironmentValue, NativeRunPlan, adapter};
use vector_providers::ProviderDiscovery;

#[derive(Parser)]
#[command(
    name = "vector-agent",
    bin_name = "vctr",
    version,
    about = "Local-first control plane for agent harnesses"
)]
struct Cli {
    #[arg(long, global = true, default_value = ".")]
    workspace: PathBuf,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    Init(InitArgs),
    Doctor,
    Status,
    Resolve(ResolveArgs),
    Run(RunArgs),
    Harness {
        #[command(subcommand)]
        command: HarnessCommands,
    },
    Provider {
        #[command(subcommand)]
        command: ProviderCommands,
    },
    Workspace {
        #[command(subcommand)]
        command: WorkspaceCommands,
    },
    Schema {
        #[arg(value_enum, default_value = "workspace")]
        kind: SchemaKind,
    },
}

#[derive(Args)]
struct InitArgs {
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    vision_model: Option<String>,
    #[arg(long)]
    computer_use: bool,
    #[arg(long)]
    non_interactive: bool,
}

#[derive(Args)]
struct ResolveArgs {
    #[arg(long)]
    profile: Option<String>,
    #[arg(long)]
    explain: bool,
    #[arg(long)]
    grant_yolo: bool,
}

#[derive(Args)]
struct RunArgs {
    #[arg(long)]
    profile: Option<String>,
    #[arg(long)]
    grant_yolo: bool,
    #[arg(long)]
    confirm_yolo: Option<String>,
    #[arg(long)]
    launch: bool,
}

#[derive(Subcommand)]
enum HarnessCommands {
    Plan {
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        grant_yolo: bool,
    },
    Doctor {
        #[arg(value_enum)]
        harness: HarnessArg,
    },
}

#[derive(Subcommand)]
enum ProviderCommands {
    Discover {
        #[arg(long, default_value = "http://127.0.0.1:1234/v1")]
        endpoint: String,
    },
}

#[derive(Subcommand)]
enum WorkspaceCommands {
    Explain,
    Lock,
}

#[derive(Clone, ValueEnum)]
enum HarnessArg {
    Omp,
    Pi,
    Deepseek,
}

impl From<HarnessArg> for HarnessId {
    fn from(value: HarnessArg) -> Self {
        match value {
            HarnessArg::Omp => Self::Omp,
            HarnessArg::Pi => Self::Pi,
            HarnessArg::Deepseek => Self::Deepseek,
        }
    }
}

#[derive(Clone, ValueEnum)]
enum SchemaKind {
    Workspace,
    Runspec,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = cli
        .workspace
        .canonicalize()
        .unwrap_or(cli.workspace.clone());
    match cli.command {
        None => tui(&root),
        Some(Commands::Init(args)) => init(&root, args, cli.json).await,
        Some(Commands::Doctor) => doctor(&root, cli.json).await,
        Some(Commands::Status) => status(cli.json).await,
        Some(Commands::Resolve(args)) => show_resolve(&root, args, cli.json),
        Some(Commands::Run(args)) => run(&root, args, cli.json).await,
        Some(Commands::Harness { command }) => harness(&root, command, cli.json).await,
        Some(Commands::Provider { command }) => provider(command, cli.json).await,
        Some(Commands::Workspace { command }) => workspace(&root, command, cli.json),
        Some(Commands::Schema { kind }) => {
            print_json(match kind {
                SchemaKind::Workspace => workspace_schema(),
                SchemaKind::Runspec => runspec_schema(),
            });
            Ok(())
        }
    }
}

async fn init(root: &Path, args: InitArgs, json_output: bool) -> Result<()> {
    let discovery = ProviderDiscovery::new()?;
    let lm = discovery.lm_studio("http://127.0.0.1:1234/v1").await.context("VCTR_PROVIDER_UNAVAILABLE: LM Studio was not reachable at 127.0.0.1:1234. Start its local server, then retry")?;
    if lm.models.is_empty() {
        bail!("VCTR_MODEL_UNAVAILABLE: LM Studio returned no loaded models");
    }
    let interactive = !args.non_interactive && io::stdin().is_terminal();
    let model = match args.model {
        Some(model) => model,
        None if interactive => {
            let labels: Vec<_> = lm
                .models
                .iter()
                .map(|model| {
                    format!(
                        "{}{}",
                        model.id,
                        model
                            .context_window
                            .map(|value| format!("  ·  {value} tokens"))
                            .unwrap_or_default()
                    )
                })
                .collect();
            let choice = Select::with_theme(&ColorfulTheme::default())
                .with_prompt("Choose the primary model served by LM Studio")
                .items(&labels)
                .default(0)
                .interact()?;
            lm.models[choice].id.clone()
        }
        None => bail!("VCTR_MODEL_UNAVAILABLE: --model is required in non-interactive mode"),
    };
    if !lm.models.iter().any(|candidate| candidate.id == model) {
        bail!("VCTR_MODEL_UNAVAILABLE: LM Studio did not report model {model}");
    }
    let computer = if interactive && !args.computer_use {
        Confirm::with_theme(&ColorfulTheme::default())
            .with_prompt("Configure computer use now?")
            .default(false)
            .interact()?
    } else {
        args.computer_use
    };
    let vision = args.vision_model.or_else(|| {
        lm.models
            .iter()
            .find(|candidate| candidate.id == model && candidate.vision == Some(true))
            .map(|candidate| candidate.id.clone())
    });
    if computer && vision.is_none() {
        bail!(
            "VCTR_CAPABILITY_UNSATISFIED: computer use needs a model that LM Studio reports as vision-capable, or --vision-model"
        );
    }
    let config = starter_workspace(&model, vision.as_deref(), computer);
    let path = write_workspace_atomic(root, &config)?;
    let output = json!({"configured":true,"path":path,"provider":lm,"defaultProfile":"pi-safe","computerUse":computer});
    if json_output {
        print_json(output);
    } else {
        println!(
            "Vector is ready.\n\n  Model: {model}\n  Profile: pi-safe\n  Config: {}\n\nNext: vctr resolve --explain",
            path.display()
        );
    }
    Ok(())
}

async fn doctor(root: &Path, json_output: bool) -> Result<()> {
    let discovery = ProviderDiscovery::new()?;
    let providers = discovery.discover_defaults().await;
    let harnesses = futures_doctor().await;
    let config = load_workspace(root).ok().map(|resolved| json!({"valid":true,"profiles":resolved.config.profiles.len(),"layers":resolved.layers.iter().map(|layer| &layer.name).collect::<Vec<_>>()})).unwrap_or_else(|| json!({"valid":false}));
    let report =
        json!({"config":config,"providers":providers,"harnesses":harnesses,"telemetry":false});
    if json_output {
        print_json(report);
    } else {
        println!(
            "Vector doctor\n  Config: {}\n  Local providers: {}\n  Harnesses detected: {}/3\n  Telemetry: disabled",
            if config["valid"] == true {
                "valid"
            } else {
                "not initialized"
            },
            providers.len(),
            harnesses.iter().filter(|report| report.detected).count()
        );
    }
    Ok(())
}

async fn futures_doctor() -> Vec<vector_harness::DoctorReport> {
    let omp_adapter = adapter(HarnessId::Omp);
    let pi_adapter = adapter(HarnessId::Pi);
    let deepseek_adapter = adapter(HarnessId::Deepseek);
    let (omp, pi, deepseek) = tokio::join!(
        omp_adapter.doctor(),
        pi_adapter.doctor(),
        deepseek_adapter.doctor()
    );
    vec![omp, pi, deepseek]
}

fn show_resolve(root: &Path, args: ResolveArgs, json_output: bool) -> Result<()> {
    let spec = resolve_run(root, args.profile.as_deref(), args.grant_yolo)?;
    let fingerprint = spec.fingerprint()?;
    if json_output || !args.explain {
        print_json(json!({"fingerprint":fingerprint,"runSpec":spec}));
    } else {
        println!(
            "PortableRunSpec {}\n  profile: {}\n  harness: {}@{}\n  provider: {}\n  model: {}\n  isolation: {:?}\n  YOLO grant: {}\n\nCapability resolution:",
            &fingerprint[..16],
            spec.profile,
            spec.harness.package,
            spec.harness.version,
            spec.provider.base_url,
            spec.models.primary,
            spec.isolation.kind,
            spec.grant.yolo
        );
        for (capability, policy) in &spec.capabilities {
            println!(
                "  {:<32} {:<7} (requested {})",
                capability, policy.effective, policy.requested
            );
        }
        println!("\nConfiguration provenance:");
        for (field, source) in &spec.provenance {
            println!("  {field:<48} {source}");
        }
    }
    Ok(())
}

async fn harness(root: &Path, command: HarnessCommands, json_output: bool) -> Result<()> {
    match command {
        HarnessCommands::Plan {
            profile,
            grant_yolo,
        } => {
            let spec = resolve_run(root, profile.as_deref(), grant_yolo)?;
            let plan = adapter(spec.harness.harness).compile(&spec)?;
            if json_output {
                print_json(json!(plan));
            } else {
                println!(
                    "{}\n\nExecutable: {}\nArguments:\n  {}\nObservation: {}",
                    plan.native_summary,
                    plan.executable,
                    plan.argv.join("\n  "),
                    plan.observation
                );
            }
        }
        HarnessCommands::Doctor { harness } => {
            let report = adapter(harness.into()).doctor().await;
            if json_output {
                print_json(json!(report));
            } else {
                println!(
                    "{}: {} ({:?})",
                    report.executable,
                    if report.detected {
                        "detected"
                    } else {
                        "not installed"
                    },
                    report.compatibility
                );
            }
        }
    }
    Ok(())
}

async fn provider(command: ProviderCommands, json_output: bool) -> Result<()> {
    match command {
        ProviderCommands::Discover { endpoint } => {
            let provider = ProviderDiscovery::new()?.lm_studio(&endpoint).await?;
            if json_output {
                print_json(json!(provider));
            } else {
                println!(
                    "LM Studio · {} ms\n{}",
                    provider.latency_ms,
                    provider
                        .models
                        .iter()
                        .map(|model| format!("  {}", model.id))
                        .collect::<Vec<_>>()
                        .join("\n")
                );
            }
        }
    }
    Ok(())
}

fn workspace(root: &Path, command: WorkspaceCommands, json_output: bool) -> Result<()> {
    match command {
        WorkspaceCommands::Explain => {
            let resolved = load_workspace(root)?;
            if json_output {
                print_json(json!({"config":resolved.config,"provenance":resolved.provenance}));
            } else {
                for (field, source) in resolved.provenance {
                    println!("{field:<48} {source}");
                }
            }
        }
        WorkspaceCommands::Lock => {
            let resolved = load_workspace(root)?;
            let mut fingerprints = std::collections::BTreeMap::new();
            for name in resolved.config.profiles.keys() {
                if let Ok(spec) = resolve_run(root, Some(name), resolved.config.profiles[name].yolo)
                {
                    fingerprints.insert(name.clone(), spec.fingerprint()?);
                }
            }
            let lock = vector_core::VectorLock {
                lock_version: 1,
                generated_at: chrono::Utc::now(),
                profile_fingerprints: fingerprints,
                harnesses: std::collections::BTreeMap::from([
                    ("omp".into(), "18.0.4".into()),
                    ("pi".into(), "0.84.3".into()),
                    ("deepseek".into(), "0.1.1-rc.2".into()),
                ]),
                packs: std::collections::BTreeMap::new(),
            };
            let path = root.join(".vector/vector.lock");
            fs::write(&path, serde_yaml::to_string(&lock)?)?;
            if json_output {
                print_json(json!({"path":path,"lock":lock}));
            } else {
                println!(
                    "Locked {} profiles in {}",
                    lock.profile_fingerprints.len(),
                    path.display()
                );
            }
        }
    }
    Ok(())
}

async fn run(root: &Path, args: RunArgs, json_output: bool) -> Result<()> {
    if args.grant_yolo && args.confirm_yolo.as_deref() != Some("VECTOR-YOLO") {
        bail!("VCTR_POLICY_DENIED: --grant-yolo requires --confirm-yolo VECTOR-YOLO");
    }
    let spec = resolve_run(root, args.profile.as_deref(), args.grant_yolo)?;
    let plan = adapter(spec.harness.harness).compile(&spec)?;
    let paths = application_paths()
        .ok_or_else(|| anyhow!("VCTR_RUNTIME_UNAVAILABLE: no application data directory"))?;
    let runs_dir = paths.data_dir.join("runs");
    let mut ledger = RunLedger::create(&runs_dir, &spec).await?;
    let overlay = ledger.dir.join("generated");
    materialize(&plan, &overlay, &ledger.dir)?;
    ledger
        .append(
            "run.prepared",
            json!({"plan":plan,"fingerprint":spec.fingerprint()?}),
        )
        .await?;
    if !args.launch {
        if json_output {
            print_json(
                json!({"runId":spec.run_id,"status":"prepared","directory":ledger.dir,"hint":"Pass --launch to start the native harness"}),
            );
        } else {
            println!(
                "Prepared run {}\nLedger: {}\nNative launch was not requested. Add --launch after inspecting `vctr harness plan`.",
                spec.run_id,
                ledger.dir.display()
            );
        }
        return Ok(());
    }
    ledger.set_status("running").await?;
    let db = VectorDatabase::open(&paths.data_dir.join("state/vector.db")).await?;
    db.project_run(&ledger.manifest).await?;
    let mut command = Command::new(&plan.executable);
    command
        .args(
            plan.argv
                .iter()
                .map(|arg| substitute(arg, &overlay, &ledger.dir)),
        )
        .current_dir(root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    for (name, value) in &plan.environment {
        match value {
            EnvironmentValue::Literal(value) => {
                command.env(name, value);
            }
            EnvironmentValue::SecretReference(reference) => bail!(
                "VCTR_SECRET_UNAVAILABLE: secret materialization is not enabled for {reference}"
            ),
        }
    }
    let mut child = command.spawn().with_context(|| {
        format!(
            "VCTR_HARNESS_INCOMPATIBLE: could not launch {}",
            plan.executable
        )
    })?;
    ledger
        .append(
            "process.started",
            json!({"pid":child.id(),"executable":plan.executable}),
        )
        .await?;
    let status = child.wait()?;
    ledger
        .set_status(if status.success() {
            "succeeded"
        } else {
            "failed"
        })
        .await?;
    ledger
        .append(
            "process.exited",
            json!({"success":status.success(),"code":status.code()}),
        )
        .await?;
    db.project_run(&ledger.manifest).await?;
    Ok(())
}

fn materialize(plan: &NativeRunPlan, overlay: &Path, run_dir: &Path) -> Result<()> {
    for generated in &plan.generated_files {
        let relative = Path::new(&generated.relative_path);
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            bail!("VCTR_POLICY_DENIED: generated path escaped the Vector overlay");
        }
        let path = overlay.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, substitute(&generated.content, overlay, run_dir))?;
    }
    Ok(())
}

fn substitute(value: &str, overlay: &Path, run_dir: &Path) -> String {
    value
        .replace("${VECTOR_OVERLAY}", &overlay.to_string_lossy())
        .replace("${VECTOR_RUN_DIR}", &run_dir.to_string_lossy())
}

async fn status(json_output: bool) -> Result<()> {
    #[cfg(unix)]
    {
        let response = daemon_request("status", json!({})).await?;
        if json_output {
            print_json(json!(response));
        } else if response.ok {
            println!("vectord is ready · protocol {PROTOCOL_VERSION}");
        } else {
            bail!(
                "{}",
                response.diagnostic.map(|d| d.detail).unwrap_or_default()
            );
        }
        Ok(())
    }
    #[cfg(not(unix))]
    bail!("VCTR_RUNTIME_UNAVAILABLE: named-pipe client is not enabled in this foundation build")
}

#[cfg(unix)]
async fn daemon_request(method: &str, params: Value) -> Result<ResponseEnvelope> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;
    let paths = application_paths().context("no Vector data directory")?;
    let token = tokio::fs::read_to_string(paths.data_dir.join("state/daemon.token"))
        .await
        .context("vectord is not initialized; start it with `vectord`")?;
    let stream = UnixStream::connect(paths.data_dir.join("state/vectord.sock"))
        .await
        .context("vectord is not running")?;
    let (read, mut write) = stream.into_split();
    let mut lines = BufReader::new(read).lines();
    let challenge_request = RequestEnvelope {
        protocol_version: PROTOCOL_VERSION.into(),
        request_id: Uuid::now_v7(),
        idempotency_key: Uuid::new_v4().to_string(),
        method: "auth.challenge".into(),
        params: json!({}),
        auth: None,
        confirmation_token: None,
    };
    write
        .write_all(format!("{}\n", serde_json::to_string(&challenge_request)?).as_bytes())
        .await?;
    let challenge_response: ResponseEnvelope = serde_json::from_str(
        &lines
            .next_line()
            .await?
            .context("daemon closed during challenge")?,
    )?;
    let challenge = challenge_response
        .result
        .and_then(|result| {
            result
                .get("challenge")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .context("daemon did not issue a challenge")?;
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
        confirmation_token: None,
    };
    write
        .write_all(format!("{}\n", serde_json::to_string(&request)?).as_bytes())
        .await?;
    Ok(serde_json::from_str(
        &lines
            .next_line()
            .await?
            .context("daemon closed without a response")?,
    )?)
}

fn tui(root: &Path) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let configured = root.join(".vector/vector.yaml").exists();
    loop {
        terminal.draw(|frame| {
            let areas = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(5),
                    Constraint::Min(8),
                    Constraint::Length(3),
                ])
                .split(frame.area());
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(Span::styled(
                        "VECTOR",
                        Style::default()
                            .fg(Color::LightYellow)
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from("Local harness control plane"),
                    Line::from(format!("{}", root.display())),
                ])
                .block(Block::default().borders(Borders::ALL)),
                areas[0],
            );
            let items = [
                format!(
                    "Workspace      {}",
                    if configured {
                        "configured"
                    } else {
                        "run `vctr init`"
                    }
                ),
                "Harnesses      OMP · Pi · DeepSeek preview".into(),
                "Provider       LM Studio · Ollama · OpenAI-compatible".into(),
                "Policy         deny > prompt > allow".into(),
                "Telemetry      disabled".into(),
            ]
            .into_iter()
            .map(ListItem::new)
            .collect::<Vec<_>>();
            frame.render_widget(
                List::new(items)
                    .block(Block::default().title(" Cockpit ").borders(Borders::ALL))
                    .highlight_style(Style::default().fg(Color::LightYellow)),
                areas[1],
            );
            frame.render_widget(
                Paragraph::new("q quit  ·  i initialize  ·  d doctor")
                    .block(Block::default().borders(Borders::ALL)),
                areas[2],
            );
        })?;
        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    _ => {}
                }
            }
        }
    }
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

fn print_json(value: Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(&value).expect("JSON output serializes")
    );
}
