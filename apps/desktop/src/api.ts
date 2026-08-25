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
  if ("__TAURI_INTERNALS__" in window) {
    return invoke<SystemSnapshot>("system_snapshot");
  }
  const response = await fetch("/__vector/system");
  return response.ok ? response.json() as Promise<SystemSnapshot> : browserFallback;
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
