import { useEffect, useMemo, useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import {
  Activity, AppWindow, Archive, Blocks, Bot, Box, BrainCircuit, ChevronRight,
  CircleDot, Command, Cpu, Download, FlaskConical, Gauge, HardDrive, KeyRound,
  Laptop, ListChecks, PackageCheck, Play, Search, Settings2, ShieldCheck, Sparkles,
  TerminalSquare, TestTube2, Waypoints, Wrench,
} from "lucide-react";
import { discoverLmStudio, initializeWorkspace, systemSnapshot } from "./api";
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
  const [selectedModel, setSelectedModel] = useState<string>();
  const [selectedHarness, setSelectedHarness] = useState<"pi" | "omp" | "deepseek">("pi");
  const lm = useQuery({ queryKey: ["lm-studio"], queryFn: () => discoverLmStudio(), enabled: discover, retry: false });
  const initialize = useMutation({ mutationFn: initializeWorkspace });
  const tools = system?.tools ?? {};
  const detected = Object.values(tools).filter(Boolean).length;

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
        {onboardingStep === 1 && lm.data && <div className="model-list">{lm.data.models.map((model) => <button key={model.id} onClick={() => { setSelectedModel(model.id); setOnboardingStep(2); }}><BrainCircuit size={18}/><span><strong>{model.id}</strong><small>{model.vision ? "Text + vision" : "Text model"}{model.contextWindow ? ` · ${model.contextWindow.toLocaleString()} context` : ""}</small></span><ChevronRight size={17}/></button>)}</div>}
        {onboardingStep === 2 && <HarnessChoice onChoose={(harness) => { setSelectedHarness(harness); setOnboardingStep(3); }} />}
        {onboardingStep >= 3 && <SetupStep title={initialize.isSuccess ? "Workspace configured" : "Run spec ready"} text={initialize.isSuccess ? `Vector wrote ${initialize.data.path}. Safe and YOLO profiles are now available.` : "Start guarded. Every capability decision and native argument remains available in the ledger."} action={initialize.isPending ? "Writing configuration…" : initialize.isSuccess ? "Configuration ready" : "Configure workspace"} disabled={initialize.isPending || initialize.isSuccess || !selectedModel} onAction={() => selectedModel && initialize.mutate({ workspace: system?.cwd ?? ".", model: selectedModel, visionModel: lm.data?.models.find((model) => model.vision)?.id, computerUse: Boolean(lm.data?.models.some((model) => model.vision)), harness: selectedHarness })} detail={initialize.isError ? `Configuration was not written: ${initialize.error.message}` : "Safe is the default. YOLO always asks for a run grant."}/>} 
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
