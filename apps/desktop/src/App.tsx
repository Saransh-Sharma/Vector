import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import {
  Activity, AppWindow, Archive, Blocks, Bot, Box, BrainCircuit, ChevronRight,
  Check, CircleDot, Command, Cpu, Download, FlaskConical, Gauge, HardDrive, KeyRound,
  Laptop, ListChecks, PackageCheck, Play, Search, Settings2, ShieldCheck, Sparkles,
  TerminalSquare, TestTube2, Waypoints, Wrench, X,
} from "lucide-react";
import {
  abortRun, discoverLmStudio, initializeWorkspace, installHarness, onboardingPreflight,
  promptRun, runSmokeTest, sessionEvents, startRun, stopRun, systemSnapshot, verifyComputerUse,
  type InteractiveSessionState, type SessionEvent,
} from "./api";
import { useUi } from "./store";

const sections = [
  ["Cockpit", Gauge], ["Workspaces", HardDrive], ["Sessions & Runs", Activity],
  ["Harnesses", Bot], ["Providers & Models", BrainCircuit], ["Local Model Lab", Cpu],
  ["Packs", Blocks], ["Tools & MCP", Wrench], ["Recipes", ListChecks],
  ["Evals & Benchmarks", FlaskConical], ["Optimize", Sparkles], ["Context Inspector", Search],
  ["Policy & Isolation", ShieldCheck], ["Computer Use", Laptop], ["Jobs & Downloads", Download],
  ["Diagnostics", CircleDot], ["Updates", PackageCheck], ["Settings", Settings2],
] as const;

export function App() {
  const { active, setActive, paletteOpen, setPaletteOpen } = useUi();
  const system = useQuery({ queryKey: ["system"], queryFn: systemSnapshot });

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
        event.preventDefault(); setPaletteOpen(!paletteOpen);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [paletteOpen, setPaletteOpen]);

  return (
    <div className="app-shell">
      <aside className="rail" aria-label="Vector navigation">
        <div className="brand-lockup"><VectorMark /><div><strong>VECTOR</strong><span>Harness control plane</span></div></div>
        <nav>
          {sections.map(([label, Icon]) => (
            <button key={label} className={active === label ? "nav-item active" : "nav-item"} onClick={() => setActive(label)} aria-current={active === label ? "page" : undefined}>
              <Icon size={16} aria-hidden="true"/><span>{label}</span>
            </button>
          ))}
        </nav>
        <div className="rail-foot"><span className="status-lamp"/> Local only <small>Telemetry off</small></div>
      </aside>

      <main id="main-content" className="workspace">
        <header className="topbar">
          <div><span className="eyebrow">{active}</span><h1>{active === "Cockpit" ? "Ready the workbench." : active}</h1></div>
          <button className="command-trigger" onClick={() => setPaletteOpen(true)}><Command size={15}/> Search or run a command <kbd>⌘ K</kbd></button>
        </header>
        {active === "Cockpit" ? <Cockpit system={system.data} /> : <EmptyWorkbench name={active} />}
      </main>
      {paletteOpen && <CommandPalette close={() => setPaletteOpen(false)} />}
    </div>
  );
}

function VectorMark() {
  return <svg className="vector-mark" viewBox="0 0 44 44" role="img" aria-label="Vector"><path d="M6 7l16 31L38 7 22 18 6 7z"/><path d="M22 18v20"/></svg>;
}

function Cockpit({ system }: { system?: Awaited<ReturnType<typeof systemSnapshot>> }) {
  const { onboardingStep, setOnboardingStep } = useUi();
  const [discover, setDiscover] = useState(false);
  const [selectedModel, setSelectedModel] = useState<string | undefined>(() => localStorage.getItem("vector.onboarding.model") ?? undefined);
  const [selectedHarness, setSelectedHarness] = useState<"pi" | "omp" | "deepseek">(() => {
    const saved = localStorage.getItem("vector.onboarding.harness");
    return saved === "omp" || saved === "deepseek" ? saved : "pi";
  });
  const lm = useQuery({ queryKey: ["lm-studio"], queryFn: () => discoverLmStudio(), enabled: discover, retry: false });
  const initialize = useMutation({ mutationFn: initializeWorkspace });
  const tools = system?.tools ?? {};
  const detected = Object.values(tools).filter(Boolean).length;

  useEffect(() => {
    if (system?.configured && onboardingStep < 3) setOnboardingStep(3);
    else if (!system?.configured && onboardingStep > 1 && !selectedModel) setOnboardingStep(1);
  }, [onboardingStep, selectedModel, setOnboardingStep, system?.configured]);

  const configured = Boolean(system?.configured || initialize.isSuccess);
  const configuredProfile = initialize.data?.defaultProfile ?? system?.defaultProfile;

  return <div className="cockpit">
    <section className="mission-strip" aria-label="System status">
      <div><span>LOCAL SUBSTRATE</span><strong>{system?.os ?? "Inspecting…"}</strong><small>{system?.architecture ?? ""}</small></div>
      <div><span>TOOLS READY</span><strong>{detected}<i>/ 6</i></strong><small>managed runtimes stay isolated</small></div>
      <div><span>POLICY FLOOR</span><strong>HARD</strong><small>deny outranks every grant</small></div>
      <div><span>NETWORK</span><strong>LOOPBACK</strong><small>cloud routing disabled</small></div>
    </section>

    <div className="cockpit-grid">
      <section className="onboarding-panel" aria-labelledby="setup-title">
        <div className="section-heading"><span className="index">01</span><div><p className="eyebrow">First local run</p><h2 id="setup-title">Connect the pieces you already own.</h2></div></div>
        <p className="lead">Vector does not replace your harness. It makes the path from model to native run inspectable and repeatable.</p>
        <div className="setup-progress" aria-label={`Setup step ${onboardingStep + 1} of 4`}>
          {["Inspect", "Model", "Harness", "Launch"].map((label, index) => <div key={label} className={index <= onboardingStep ? "done" : ""}><span>{index < onboardingStep ? "✓" : index + 1}</span>{label}</div>)}
        </div>

        {onboardingStep === 0 && <SetupStep title="Check this Mac" text={`Inspect local runtimes for ${system?.cwd ?? "this workspace"} without changing system configuration.`} action="Inspect system" onAction={() => setOnboardingStep(1)} detail={`${detected} tools are already available.`}/>} 
        {onboardingStep === 1 && <SetupStep title="Find LM Studio" text="Read the exact model IDs currently exposed on 127.0.0.1. Vector starts the loopback server through the official LM Studio CLI when needed." action={lm.isFetching ? "Starting and scanning…" : "Discover models"} onAction={() => discover ? void lm.refetch() : setDiscover(true)} disabled={lm.isFetching} detail={lm.data ? `${lm.data.models.length} model${lm.data.models.length === 1 ? "" : "s"} found · ${lm.data.latencyMs} ms${lm.data.note ? ` · ${lm.data.note}` : ""}` : lm.isError ? `Automatic startup failed: ${lm.error.message}` : "No API keys required. No network-facing bind."} />}
        {onboardingStep === 1 && lm.data && <div className="model-list">{lm.data.models.map((model) => <button key={model.id} onClick={() => { localStorage.setItem("vector.onboarding.model", model.id); setSelectedModel(model.id); setOnboardingStep(2); }}><BrainCircuit size={18}/><span><strong>{model.id}</strong><small>{model.vision ? "Text + vision" : "Text model"}{model.contextWindow ? ` · ${model.contextWindow.toLocaleString()} context` : ""}</small></span><ChevronRight size={17}/></button>)}</div>}
        {onboardingStep === 2 && <HarnessChoice onChoose={(harness) => { localStorage.setItem("vector.onboarding.harness", harness); setSelectedHarness(harness); setOnboardingStep(3); }} />}
        {onboardingStep >= 3 && !configured && <SetupStep
          title="Run spec ready"
          text="Save the model, harness, Safe and YOLO profiles before validating the native runtime."
          action={initialize.isPending ? "Writing configuration…" : "Configure workspace"}
          disabled={initialize.isPending || !selectedModel}
          onAction={() => selectedModel && initialize.mutate({ workspace: system?.cwd ?? ".", model: selectedModel, visionModel: lm.data?.models.find((model) => model.vision)?.id, computerUse: false, harness: selectedHarness })}
          detail={initialize.isError ? `Configuration was not written: ${initialize.error.message}` : "Computer use is verified separately after the coding smoke test."}
        />}
        {onboardingStep >= 3 && configured && <LaunchCenter workspace={system?.cwd ?? "."} initialHarness={(configuredProfile?.split("-")[0] as "pi" | "omp" | "deepseek" | undefined) ?? selectedHarness} />}
      </section>

      <section className="topology" aria-labelledby="topology-title">
        <div className="section-heading compact"><span className="index">02</span><div><p className="eyebrow">Resolved topology</p><h2 id="topology-title">Intent becomes a native plan.</h2></div></div>
        <div className="flow-map">
          <FlowNode icon={BrainCircuit} overline="PROVIDER" label={lm.data ? "LM Studio" : "Not selected"} meta="OpenAI-compatible · local" />
          <FlowArrow label="exact model ID" />
          <FlowNode icon={Bot} overline="HARNESS" label="Pi / OMP" meta="native agent loop" />
          <FlowArrow label="compile" />
          <FlowNode icon={ShieldCheck} overline="VECTOR" label="Policy meet" meta="deny › prompt › allow" emph />
          <FlowArrow label="immutable" />
          <FlowNode icon={Play} overline="RUN" label="Native plan" meta="events + artifacts" />
        </div>
        <div className="topology-note"><Waypoints size={17}/><p><strong>No fourth loop.</strong> Vector resolves, compiles, observes, and gets out of the harness’s way.</p></div>
      </section>
    </div>

    <section className="lower-grid">
      <div className="run-queue"><div className="section-heading compact"><span className="index">03</span><div><p className="eyebrow">Run ledger</p><h2>Nothing has run yet.</h2></div></div><p>Your native sessions, tool calls, approvals, diffs, and verification evidence will collect here.</p><button className="text-action"><Archive size={16}/> Inspect ledger format</button></div>
      <div className="approval-inbox"><div className="approval-head"><KeyRound size={18}/><span>APPROVAL INBOX</span><b>0</b></div><p>No run is waiting for you.</p><small>Prompt-level decisions appear globally without weakening the policy floor.</small></div>
    </section>
  </div>;
}

function LaunchCenter({ workspace, initialHarness }: { workspace: string; initialHarness: "pi" | "omp" | "deepseek" }) {
  const [harness, setHarness] = useState(initialHarness);
  const [mode, setMode] = useState<"safe" | "yolo">("safe");
  const [yoloAcknowledged, setYoloAcknowledged] = useState(false);
  const [session, setSession] = useState<InteractiveSessionState>();
  const [events, setEvents] = useState<SessionEvent[]>([]);
  const [draft, setDraft] = useState("");
  const [showComputerSetup, setShowComputerSetup] = useState(false);
  const [visionModel, setVisionModel] = useState("");
  const profile = harness === "deepseek" ? "deepseek-preview" : `${harness}-${mode}`;
  const preflight = useQuery({
    queryKey: ["onboarding-preflight", workspace, profile],
    queryFn: () => onboardingPreflight(workspace, profile),
    retry: false,
  });
  const install = useMutation({
    mutationFn: () => installHarness(harness),
    onSuccess: () => void preflight.refetch(),
  });
  const smoke = useMutation({
    mutationFn: () => runSmokeTest(workspace, profile),
    onSuccess: () => void preflight.refetch(),
  });
  const visionInventory = useQuery({
    queryKey: ["computer-model-inventory"],
    queryFn: () => discoverLmStudio(),
    enabled: showComputerSetup,
    retry: false,
  });
  useEffect(() => {
    if (!visionModel && visionInventory.data?.models[0]) setVisionModel(visionInventory.data.models[0].id);
  }, [visionInventory.data, visionModel]);
  const computer = useMutation({
    mutationFn: (requestPermissions: boolean) => {
      if (!visionModel) throw new Error("Select a loaded model for the vision role.");
      return verifyComputerUse({ workspace, profile, visionModel, requestPermissions });
    },
    onSuccess: (result) => { if (result.enabled) void preflight.refetch(); },
  });
  const launch = useMutation({
    mutationFn: (surface: "integrated" | "native") => startRun({
      workspace, profile, surface, grantYolo: mode === "yolo",
    }),
    onSuccess: (state) => { if (state.surface === "integrated") setSession(state); },
  });
  const sendPrompt = useMutation({
    mutationFn: (prompt: string) => {
      if (!session) throw new Error("Start an integrated session first.");
      return promptRun(session.runId, prompt);
    },
    onSuccess: (state) => { setSession(state); setDraft(""); },
  });
  const abortSession = useMutation({
    mutationFn: () => {
      if (!session) throw new Error("Start an integrated session first.");
      return abortRun(session.runId);
    },
    onSuccess: setSession,
  });
  const stopSession = useMutation({
    mutationFn: () => {
      if (!session) throw new Error("Start an integrated session first.");
      return stopRun(session.runId);
    },
    onSuccess: setSession,
  });
  const eventQuery = useQuery({
    queryKey: ["session-events", session?.runId],
    queryFn: () => sessionEvents(session!.runId, events.at(-1)?.sequence ?? 0),
    enabled: Boolean(session),
    refetchInterval: 900,
    retry: false,
  });

  useEffect(() => {
    if (eventQuery.data?.events.length) {
      setEvents((current) => [...current, ...eventQuery.data.events.filter((event) => !current.some((item) => item.sequence === event.sequence))]);
      const latest = eventQuery.data.events.at(-1);
      if (latest && eventQuery.data.events.some((event) => (event.payload as { type?: string })?.type === "agent_end")) {
        setSession((current) => current ? { ...current, phase: "ready", nextSequence: latest.sequence + 1 } : current);
      }
    }
  }, [eventQuery.data]);

  const report = preflight.data;
  const ready = Boolean(report?.readyToWork || smoke.data?.passed);
  const canLaunch = ready && (mode === "safe" || yoloAcknowledged) && !launch.isPending;
  const failure = preflight.error ?? install.error ?? smoke.error ?? computer.error ?? launch.error ?? sendPrompt.error ?? abortSession.error ?? stopSession.error;

  return <section className="launch-center" aria-labelledby="launch-center-title">
    <div className="launch-head">
      <div><p className="eyebrow">Launch Center</p><h3 id="launch-center-title">Configuration is the first checkpoint.</h3><p>Vector verifies the selected native harness and a disposable coding run before unlocking your workspace.</p></div>
      <span className={ready ? "readiness ready" : "readiness"}>{ready ? "READY TO WORK" : "PREFLIGHT"}</span>
    </div>

    <div className="harness-switch" aria-label="Selected harness">
      {(["omp", "pi", "deepseek"] as const).map((item) => <button key={item} className={harness === item ? "selected" : ""} onClick={() => { setHarness(item); setMode("safe"); setYoloAcknowledged(false); setSession(undefined); setEvents([]); }}>{item === "deepseek" ? "DeepSeek preview" : item.toUpperCase()}</button>)}
    </div>

    <ol className="readiness-list">
      <ReadinessItem label="Configuration saved" passed detail={`${workspace}/.vector/vector.yaml`} />
      <ReadinessItem label="Harness installation verified" passed={Boolean(report?.harness.ready)} pending={preflight.isFetching} detail={report?.harness.notes.join(" ") ?? "Inspecting exact package and runtime pins…"} />
      <ReadinessItem label="Disposable coding smoke test passed" passed={Boolean(report?.smokePassed || smoke.data?.passed)} pending={smoke.isPending} detail={smoke.isPending ? "Streaming a native tool read against an immutable marker fixture…" : report?.smokePassed || smoke.data?.passed ? "Model, tool, policy, lifecycle, and fixture-integrity checks passed." : "Required once for this exact resolved configuration."} />
      <ReadinessItem label="Ready to work" passed={ready} detail={ready ? "Integrated and native launch surfaces are unlocked." : "Complete the checks above without weakening the policy floor."} />
    </ol>

    {!preflight.isFetching && report && !report.harness.ready && <div className="managed-install">
      <div><strong>Use Vector’s isolated managed installation</strong><small>{harness === "omp" ? "OMP 18.0.4 · Bun 1.3.14" : harness === "pi" ? "Pi 0.84.3 · Node 22.19.0" : "DeepSeek remains preview-gated"}</small><p>External and global installations remain untouched. {report.harness.compatibility === "external-unverified" ? "The detected external version is unverified." : "No tested installation was found."}</p></div>
      <button className="primary-action" disabled={install.isPending || harness === "deepseek"} onClick={() => install.mutate()}>{install.isPending ? "Installing exact pin…" : "Install managed harness"}<Download size={16}/></button>
    </div>}

    {report?.readyForSmoke && !report.smokePassed && !smoke.data?.passed && <button className="smoke-action" disabled={smoke.isPending} onClick={() => smoke.mutate()}><TestTube2 size={18}/><span><strong>{smoke.isPending ? "Running disposable smoke test…" : "Verify coding harness"}</strong><small>Reads a nonce marker with a native tool, streams events, proves the external-write deny, and checks byte identity.</small></span><ChevronRight size={17}/></button>}

    {ready && <div className="launch-deck">
      <div className="profile-picker">
        <button className={mode === "safe" ? "selected" : ""} onClick={() => { setMode("safe"); setYoloAcknowledged(false); }}>Safe <small>default</small></button>
        {harness !== "deepseek" && <button className={mode === "yolo" ? "selected danger" : ""} onClick={() => setMode("yolo")}>YOLO <small>run grant</small></button>}
      </div>
      {mode === "yolo" && <label className="risk-ack"><input type="checkbox" checked={yoloAcknowledged} onChange={(event) => setYoloAcknowledged(event.target.checked)} /><span>I understand eligible prompts become allows for this run. Hard denies, external writes, secrets, trust, and OS permissions remain enforced.</span></label>}
      <div className="surface-actions">
        <button disabled={!canLaunch} onClick={() => launch.mutate("integrated")}><AppWindow size={20}/><span><strong>Start integrated session</strong><small>Prompt composer, structured events, approvals, and cancellation in Vector.</small></span><ChevronRight/></button>
        <button disabled={!canLaunch} onClick={() => launch.mutate("native")}><TerminalSquare size={20}/><span><strong>Open native harness</strong><small>Same run-scoped provider and policy overlay in Terminal.app.</small></span><ChevronRight/></button>
      </div>
      <div className="computer-followup"><Laptop size={18}/><div><strong>Computer use is a separate optional verification.</strong><p>Select a vision-role model, pass the nonce-image probe, then grant Screen Recording and Accessibility. Computer control stays denied until every check passes.</p></div><button onClick={() => setShowComputerSetup((shown) => !shown)}>{showComputerSetup ? "Close setup" : "Set up now"}</button></div>
      {showComputerSetup && <div className="computer-setup">
        <div className="computer-model-row"><label htmlFor="vision-role-model"><strong>Vision-role model</strong><small>Vector tests pixels; model names and metadata are not trusted.</small></label><select id="vision-role-model" value={visionModel} onChange={(event) => setVisionModel(event.target.value)} disabled={visionInventory.isLoading || computer.isPending}>{visionInventory.data?.models.map((model) => <option key={model.id} value={model.id}>{model.id}</option>)}</select></div>
        {computer.data && <ol className="readiness-list computer-checks">{computer.data.checks.map((item) => <ReadinessItem key={item.id} label={item.label} passed={item.passed} detail={item.passed ? item.detail : item.remediation ?? item.detail}/>)}</ol>}
        <div className="computer-actions"><button disabled={!visionModel || computer.isPending} onClick={() => computer.mutate(false)}>{computer.isPending ? "Running verification…" : "Check permissions & model"}</button>{computer.data && (!computer.data.screenRecording || !computer.data.accessibility) && <button className="primary-action" disabled={computer.isPending} onClick={() => computer.mutate(true)}>Grant macOS permissions<ShieldCheck size={15}/></button>}</div>
        {computer.data?.enabled && <p className="computer-enabled"><Check size={15}/> Computer use is verified for {harness.toUpperCase()}. The changed run spec now needs one fresh coding smoke test.</p>}
        {visionInventory.error && <p className="computer-warning">{visionInventory.error.message}</p>}
      </div>}
    </div>}

    {!ready && computer.data?.enabled && <div className="computer-enabled outside"><Check size={15}/>Computer use passed. Re-run the coding smoke test because the verified vision role changed the run-spec fingerprint.</div>}

    {session && <div className="integrated-session">
      <header><div><span className="status-lamp"/><strong>{session.harness.toUpperCase()} integrated</strong><small>{session.runId}</small></div><div className="session-controls"><b>{session.phase}</b><button disabled={session.phase !== "streaming" || abortSession.isPending} onClick={() => abortSession.mutate()}>Abort task</button><button disabled={session.phase === "completed" || stopSession.isPending} onClick={() => stopSession.mutate()}>Stop session</button></div></header>
      <div className="event-timeline" aria-live="polite">
        {events.length === 0 ? <p>Session started. Send the first task when the harness is ready.</p> : events.map((event) => <article key={event.sequence}><span>{String(event.sequence).padStart(3, "0")}</span><div><strong>{event.kind}</strong><pre>{JSON.stringify(event.payload, null, 2)}</pre></div></article>)}
      </div>
      <form onSubmit={(event) => { event.preventDefault(); if (draft.trim()) sendPrompt.mutate(draft.trim()); }}><textarea value={draft} onChange={(event) => setDraft(event.target.value)} placeholder="Ask the harness to inspect, change, or verify this workspace…" aria-label="Session prompt" disabled={session.phase === "completed"}/><button className="primary-action" disabled={!draft.trim() || sendPrompt.isPending || session.phase === "completed"}>{sendPrompt.isPending ? "Sending…" : "Send prompt"}<Play size={16}/></button></form>
    </div>}

    {failure && <div className="launch-error" role="alert"><X size={18}/><div><strong>Action did not complete</strong><p>{failure.message}</p></div><button onClick={() => void preflight.refetch()}>Retry preflight</button></div>}
  </section>;
}

function ReadinessItem({ label, passed, pending, detail }: { label: string; passed: boolean; pending?: boolean; detail: string }) {
  return <li className={passed ? "passed" : pending ? "pending" : "blocked"}><span>{passed ? <Check size={15}/> : pending ? "…" : "—"}</span><div><strong>{label}</strong><small>{detail}</small></div></li>;
}

function SetupStep({ title, text, action, onAction, detail, disabled }: { title: string; text: string; action: string; onAction: () => void; detail: string; disabled?: boolean }) {
  return <div className="setup-step"><div><h3>{title}</h3><p>{text}</p><small>{detail}</small></div><button className="primary-action" disabled={disabled} onClick={onAction}>{action}<ChevronRight size={17}/></button></div>;
}

function HarnessChoice({ onChoose }: { onChoose: (harness: "pi" | "omp" | "deepseek") => void }) {
  return <div className="harness-choice"><button onClick={() => onChoose("pi")}><span className="choice-code">PI</span><span><strong>Pi · lean local default</strong><small>Small context, four native tools, Vector policy extension.</small></span><ChevronRight/></button><button onClick={() => onChoose("omp")}><span className="choice-code">OMP</span><span><strong>Oh My Pi · power workbench</strong><small>LSP, debugger, subagents, browser, and native computer use.</small></span><ChevronRight/></button><button onClick={() => onChoose("deepseek")}><span className="choice-code preview">DSH</span><span><strong>DeepSeek Harness</strong><small>Plugin-first preview · exact version pin.</small></span><ChevronRight/></button></div>;
}

function FlowNode({ icon: Icon, overline, label, meta, emph }: { icon: typeof Bot; overline: string; label: string; meta: string; emph?: boolean }) {
  return <div className={emph ? "flow-node emph" : "flow-node"}><Icon size={19}/><span>{overline}</span><strong>{label}</strong><small>{meta}</small></div>;
}

function FlowArrow({ label }: { label: string }) { return <div className="flow-arrow"><span>{label}</span><i>→</i></div>; }

function EmptyWorkbench({ name }: { name: string }) {
  const icon = useMemo(() => sections.find(([label]) => label === name)?.[1] ?? Box, [name]);
  const Icon = icon;
  return <section className="empty-workbench"><div className="empty-graphic"><Icon size={32}/><div className="axis x"/><div className="axis y"/></div><p className="eyebrow">{name}</p><h2>This surface is wired for the daemon.</h2><p>Its data model is present; the next implementation gate connects durable jobs and native event streams.</p><button className="secondary-action"><TerminalSquare size={16}/> View command equivalent</button></section>;
}

function CommandPalette({ close }: { close: () => void }) {
  const [query, setQuery] = useState("");
  const { setActive } = useUi();
  const results = sections.filter(([label]) => label.toLowerCase().includes(query.toLowerCase())).slice(0, 7);
  return <div className="palette-backdrop" onMouseDown={close}><div className="palette" role="dialog" aria-modal="true" aria-label="Command palette" onMouseDown={(event) => event.stopPropagation()}><div className="palette-search"><Search size={18}/><input autoFocus value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Search surfaces or type a command…" aria-label="Search Vector"/><kbd>esc</kbd></div><div className="palette-results">{results.map(([label, Icon]) => <button key={label} onClick={() => { setActive(label); close(); }}><Icon size={16}/><span>{label}</span><ChevronRight size={15}/></button>)}</div><footer><span>↑↓ navigate</span><span>↵ open</span><span>Local commands only</span></footer></div></div>;
}
