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
};

export type SystemSnapshot = {
  os: string;
  architecture: string;
  cwd: string;
  telemetry: false;
  updateChecks: false;
  tools: Record<string, boolean>;
};

const browserFallback: SystemSnapshot = {
  os: navigator.platform || "Desktop",
  architecture: "local",
  cwd: ".",
  telemetry: false,
  updateChecks: false,
  tools: { git: true, bun: false, node: true, omp: false, pi: false, npx: true },
};

export async function systemSnapshot(): Promise<SystemSnapshot> {
  try { return await invoke<SystemSnapshot>("system_snapshot"); }
  catch { return browserFallback; }
}

export async function discoverLmStudio(endpoint = "http://127.0.0.1:1234/v1"): Promise<ProviderSnapshot> {
  return invoke<ProviderSnapshot>("discover_lm_studio", { endpoint });
}

export async function initializeWorkspace(input: {
  workspace: string;
  model: string;
  visionModel?: string;
  computerUse: boolean;
  harness: "pi" | "omp" | "deepseek";
}) {
  return invoke<{ path: string; defaultProfile: string }>("initialize_workspace", { input });
}
