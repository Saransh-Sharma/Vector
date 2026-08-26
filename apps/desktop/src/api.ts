import { invoke } from "@tauri-apps/api/core";

export type ModelDescriptor = {
  id: string;
  ownedBy?: string;
  contextWindow?: number;
  vision?: boolean;
};

export type ProviderSnapshot = {
  kind: string;
  baseUrl: string;
  healthy: boolean;
  models: ModelDescriptor[];
  latencyMs: number;
  note?: string;
};

export type SystemSnapshot = {
  os: string;
  architecture: string;
  cwd: string;
  telemetry: false;
  updateChecks: false;
  tools: Record<string, boolean>;
  configured: boolean;
  defaultProfile?: string;
};

export type HarnessInstallationRecord = {
  harness: "omp" | "pi" | "deepseek";
  source: "managed" | "external" | "missing";
  executable?: string;
  package?: string;
  version?: string;
  integrity?: string;
  runtime: string;
  compatibility: "managed-tested" | "managed-experimental" | "external-compatible" | "external-unverified" | "external-incompatible";
  ready: boolean;
  notes: string[];
};

export type PreflightReport = {
  workspace: string;
  profile: string;
  harness: HarnessInstallationRecord;
  checks: Array<{ id: string; label: string; passed: boolean; detail: string; remediation?: string }>;
  readyForSmoke: boolean;
  smokePassed: boolean;
  readyToWork: boolean;
};

export type SmokeTestReport = {
  passed: boolean;
  modelStreamed: boolean;
  toolObserved: boolean;
  policyDenialObserved: boolean;
  fixtureDigestBefore: string;
  fixtureDigestAfter: string;
  events: string[];
  diagnostic?: { code: string; summary: string; detail: string };
};

export type ComputerUseVerificationReport = {
  visionModel: string;
  visionProbePassed: boolean;
  screenRecording: boolean;
  accessibility: boolean;
  fixtureIdentified: boolean;
  fixtureClicked: boolean;
  screenshotPath?: string;
  enabled: boolean;
  checks: Array<{ id: string; label: string; passed: boolean; detail: string; remediation?: string }>;
};

export type InteractiveSessionState = {
  runId: string;
  harness: "omp" | "pi" | "deepseek";
  surface: "integrated" | "native";
  phase: "preparing" | "ready" | "streaming" | "waiting-for-approval" | "stopping" | "completed" | "failed";
  nativeSessionId?: string;
  nextSequence: number;
};

export type SessionEvent = { runId: string; sequence: number; kind: string; occurredAt: string; payload: unknown; nativeRef?: string };

const browserFallback: SystemSnapshot = {
  os: navigator.platform || "Desktop",
  architecture: "local",
  cwd: ".",
  telemetry: false,
  updateChecks: false,
  tools: { git: true, bun: false, node: true, omp: false, pi: false, npx: true },
  configured: false,
};

export async function systemSnapshot(): Promise<SystemSnapshot> {
  if ("__TAURI_INTERNALS__" in window) {
    return invoke<SystemSnapshot>("system_snapshot");
  }
  const response = await fetch("/__vector/system");
  return response.ok ? response.json() as Promise<SystemSnapshot> : browserFallback;
}

async function browserOperation<T>(input: Record<string, unknown>): Promise<T> {
  const response = await fetch("/__vector/api", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
  const result = await response.json().catch(() => undefined) as (T & { error?: string }) | undefined;
  if (!response.ok || !result) throw new Error(result?.error ?? `Vector returned HTTP ${response.status}`);
  return result;
}

export async function daemonCall<T>(method: string, params: Record<string, unknown> = {}, confirmationToken?: string): Promise<T> {
  if ("__TAURI_INTERNALS__" in window) {
    return invoke<T>("daemon_call", { method, params, confirmationToken });
  }
  return browserOperation<T>({ action: "daemon", method, params, confirmationToken });
}

export type ResourceState = "ready" | "queued" | "running" | "stopped" | "failed" | "interrupted" | "unsupported";
export type ModelFolder = { id: string; path: string; writable: boolean; addedAt: string };
export type ManagedModel = { id: string; folderId: string; path: string; displayName: string; sizeBytes: number; fingerprint: string; format: string; architecture?: string; quantization?: string; shardPaths: string[]; projectorPath?: string; embedding: boolean; vision: boolean; indexedAt: string };
export type BackendRecord = { id: string; name: string; executable: string; kind: string; version?: string; devices: string[]; fingerprint: string; verified: boolean; addedAt: string };
export type BackendGroup = { id: string; name: string; backendIds: string[]; activeBackendId?: string };
export type ManagedServer = { spec: { id: string; name: string; backendId: string; modelId: string; host: string; port: number; contextSize: number; gpuLayers: number; tensorSplit: number[]; extraArgs: string[]; embedding: boolean; speculativeModelId?: string; autoStart: boolean; fingerprint: string }; state: ResourceState; pid?: number; lastError?: string; startedAt?: string };
export type DownloadJob = { id: string; url: string; destination: string; state: ResourceState; receivedBytes: number; totalBytes?: number; expectedSha256?: string; error?: string; createdAt: string; updatedAt: string };
export type RecipeSpec = { id: string; name: string; executable: string; args: string[]; workingDirectory?: string; digest: string; trusted: boolean; source: string };
export type ProxyAlias = { id: string; alias: string; serverId: string; modelName: string; enabled: boolean };
export type CheckpointRecord = { id: string; serverId: string; path: string; modelFingerprint: string; slot: number; tokenCount: number; sizeBytes: number; createdAt: string };
export type McpServerRecord = { id: string; name: string; command: string; args: string[]; secretRefs: Record<string, string>; decision: "allow" | "prompt" | "deny"; enabled: boolean };
export type VoiceService = { id: string; name: string; kind: string; executable: string; args: string[]; endpoint: string; state: ResourceState; pid?: number };
export type ChatMessage = { id: string; parentId?: string; role: string; content: string; createdAt: string; runId?: string; attachments: string[] };
export type ChatThread = { id: string; title: string; folder?: string; profile?: string; rootMessageId?: string; messages: ChatMessage[]; createdAt: string; updatedAt: string };
export type NotificationRecord = { id: string; level: string; title: string; detail: string; read: boolean; createdAt: string };
export type WorkbenchSnapshot = { modelFolders: ModelFolder[]; models: ManagedModel[]; backends: BackendRecord[]; backendGroups: BackendGroup[]; servers: ManagedServer[]; downloads: DownloadJob[]; recipes: RecipeSpec[]; proxyAliases: ProxyAlias[]; checkpoints: CheckpointRecord[]; mcpServers: McpServerRecord[]; voiceServices: VoiceService[]; chatThreads: ChatThread[]; notifications: NotificationRecord[]; revision: number };
export type HardwareSnapshot = { os: string; architecture: string; logicalCpus: number; memoryBytes?: number; devices: string[] };

export function workbenchSnapshot(): Promise<WorkbenchSnapshot> { return daemonCall("workbench.snapshot"); }
export function hardwareSnapshot(): Promise<HardwareSnapshot> { return daemonCall("hardware.snapshot"); }
export function mutateWorkbench(method: string, params: Record<string, unknown>, confirmationToken?: string): Promise<WorkbenchSnapshot> { return daemonCall(method, params, confirmationToken); }
export function searchHub(query: string): Promise<{ models: Array<{ id?: string; modelId?: string; downloads?: number; likes?: number; tags?: string[] }> }> { return daemonCall("hub.search", { query, limit: 30 }); }
export function searchWorkspace(workspace: string, query: string): Promise<{ results: Array<{ path: string; line: number; preview: string }> }> { return daemonCall("workspace.search", { workspace, query, limit: 100 }); }
export function codeGraph(workspace: string): Promise<{ nodes: Array<{ id: string; kind: string; path: string; line: number; symbol: string }>; edges: unknown[] }> { return daemonCall("workspace.code-graph", { workspace, limit: 1500 }); }

export async function onboardingPreflight(workspace: string, profile: string): Promise<PreflightReport> {
  if ("__TAURI_INTERNALS__" in window) return invoke<PreflightReport>("onboarding_preflight", { workspace, profile });
  return browserOperation({ action: "preflight", workspace, profile });
}

export async function installHarness(harness: "pi" | "omp" | "deepseek"): Promise<HarnessInstallationRecord> {
  if ("__TAURI_INTERNALS__" in window) return invoke<HarnessInstallationRecord>("harness_install", { harness });
  return browserOperation({ action: "install", harness });
}

export async function runSmokeTest(workspace: string, profile: string): Promise<SmokeTestReport> {
  if ("__TAURI_INTERNALS__" in window) return invoke<SmokeTestReport>("onboarding_smoke", { workspace, profile });
  return browserOperation({ action: "smoke", workspace, profile });
}

export async function verifyComputerUse(input: {
  workspace: string; profile: string; visionModel: string; requestPermissions: boolean;
}): Promise<ComputerUseVerificationReport> {
  if ("__TAURI_INTERNALS__" in window) {
    return invoke("daemon_call", { method: "onboarding.computer", params: input });
  }
  return browserOperation({ action: "computer", ...input });
}

export async function startRun(input: {
  workspace: string; profile: string; surface: "integrated" | "native"; grantYolo: boolean; prompt?: string;
}): Promise<InteractiveSessionState> {
  if ("__TAURI_INTERNALS__" in window) {
    return invoke<InteractiveSessionState>("daemon_call", {
      method: "runs.start", params: input, confirmationToken: input.grantYolo ? "VECTOR-YOLO" : undefined,
    });
  }
  return browserOperation({ action: "start", ...input });
}

export async function promptRun(runId: string, prompt: string): Promise<InteractiveSessionState> {
  if ("__TAURI_INTERNALS__" in window) return invoke("daemon_call", { method: "runs.prompt", params: { runId, prompt } });
  return browserOperation({ action: "prompt", runId, prompt });
}

export async function abortRun(runId: string): Promise<InteractiveSessionState> {
  if ("__TAURI_INTERNALS__" in window) return invoke("daemon_call", { method: "runs.abort", params: { runId } });
  return browserOperation({ action: "abort", runId });
}

export async function stopRun(runId: string): Promise<InteractiveSessionState> {
  if ("__TAURI_INTERNALS__" in window) return invoke("daemon_call", { method: "runs.stop", params: { runId } });
  return browserOperation({ action: "stop", runId });
}

export async function sessionEvents(runId: string, afterSequence = 0): Promise<{ events: SessionEvent[]; afterSequence: number }> {
  if ("__TAURI_INTERNALS__" in window) return invoke("daemon_call", { method: "events.subscribe", params: { runId, afterSequence } });
  return browserOperation({ action: "events", runId, afterSequence });
}

export async function discoverLmStudio(endpoint = "http://127.0.0.1:1234/v1"): Promise<ProviderSnapshot> {
  if ("__TAURI_INTERNALS__" in window) {
    return invoke<ProviderSnapshot>("discover_lm_studio", { endpoint });
  }

  // The browser preview cannot execute native commands. Vite proxies this
  // loopback-only request so the UI can still be reviewed without CORS mode.
  const started = performance.now();
  const response = await fetch("/__vector/lm-studio/v1/models");
  if (!response.ok) {
    const detail = await response.json().catch(() => undefined) as { error?: string } | undefined;
    throw new Error(detail?.error ?? `LM Studio returned HTTP ${response.status}`);
  }
  const body = await response.json() as { data: Array<Record<string, unknown> & { id: string; owned_by?: string }> };
  return {
    kind: "lm-studio",
    baseUrl: endpoint,
    healthy: true,
    latencyMs: Math.round(performance.now() - started),
    note: "Connected through Vector's loopback development bridge.",
    models: body.data.map((model) => ({
      id: model.id,
      ownedBy: model.owned_by,
      contextWindow: typeof model.max_context_length === "number" ? model.max_context_length : undefined,
      vision: typeof (model.capabilities as { vision?: unknown } | undefined)?.vision === "boolean"
        ? Boolean((model.capabilities as { vision: boolean }).vision)
        : undefined,
    })),
  };
}

export async function initializeWorkspace(input: {
  workspace: string;
  model: string;
  visionModel?: string;
  computerUse: boolean;
  harness: "pi" | "omp" | "deepseek";
}) {
  if ("__TAURI_INTERNALS__" in window) {
    return invoke<{ path: string; defaultProfile: string }>("initialize_workspace", { input });
  }
  const response = await fetch("/__vector/initialize", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
  const result = await response.json().catch(() => undefined) as { path?: string; defaultProfile?: string; error?: string } | undefined;
  if (!response.ok || !result?.path || !result.defaultProfile) {
    throw new Error(result?.error ?? `Vector initialization returned HTTP ${response.status}`);
  }
  return { path: result.path, defaultProfile: result.defaultProfile };
}
