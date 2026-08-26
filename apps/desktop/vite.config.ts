import { execFile, spawn } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { delimiter, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";

const execFileAsync = promisify(execFile);
const lmStudioOrigin = "http://127.0.0.1:1234";
const sourceRoot = resolve(fileURLToPath(new URL("../..", import.meta.url)));
const previewWorkspace = resolve(process.env.VECTOR_PREVIEW_WORKSPACE ?? sourceRoot);

type InitializeInput = {
  model: string;
  visionModel?: string;
  computerUse: boolean;
  harness: "pi" | "omp" | "deepseek";
};

type VectorOperation = {
  action: "preflight" | "install" | "smoke" | "computer" | "start" | "prompt" | "abort" | "stop" | "events";
  workspace?: string;
  profile?: string;
  harness?: "pi" | "omp" | "deepseek";
  surface?: "integrated" | "native";
  grantYolo?: boolean;
  prompt?: string;
  runId?: string;
  afterSequence?: number;
  visionModel?: string;
  requestPermissions?: boolean;
};

async function runVector(args: string[], timeout = 120_000): Promise<unknown> {
  const { stdout } = await execFileAsync("cargo", ["run", "-q", "-p", "vector-agent", "--", "--workspace", previewWorkspace, "--json", ...args], {
    cwd: sourceRoot,
    timeout,
    maxBuffer: 8 * 1024 * 1024,
  });
  return JSON.parse(stdout);
}

let daemonStarting: Promise<void> | undefined;
async function ensurePreviewDaemon(): Promise<void> {
  if (daemonStarting) return daemonStarting;
  daemonStarting = (async () => {
    try {
      await runVector(["status"], 10_000);
      return;
    } catch { /* launch below */ }
    const child = spawn("cargo", ["run", "-q", "-p", "vectord"], { cwd: sourceRoot, detached: true, stdio: "ignore" });
    child.unref();
    for (let attempt = 0; attempt < 80; attempt += 1) {
      await new Promise((resolve) => setTimeout(resolve, 250));
      try {
        await runVector(["status"], 10_000);
        return;
      } catch { /* wait for compilation and socket */ }
    }
    throw new Error("vectord did not become ready within 20 seconds.");
  })().finally(() => { daemonStarting = undefined; });
  return daemonStarting;
}

function validOperation(value: unknown): value is VectorOperation {
  if (!value || typeof value !== "object") return false;
  const action = (value as Record<string, unknown>).action;
  return typeof action === "string" && ["preflight", "install", "smoke", "computer", "start", "prompt", "abort", "stop", "events"].includes(action);
}

function json(response: import("node:http").ServerResponse, status: number, value: unknown): void {
  response.statusCode = status;
  response.setHeader("content-type", "application/json");
  response.end(JSON.stringify(value));
}

async function readJson(request: import("node:http").IncomingMessage): Promise<unknown> {
  const chunks: Buffer[] = [];
  let size = 0;
  for await (const chunk of request) {
    const buffer = Buffer.from(chunk);
    size += buffer.length;
    if (size > 64 * 1024) throw new Error("Request body exceeds 64 KiB.");
    chunks.push(buffer);
  }
  return JSON.parse(Buffer.concat(chunks).toString("utf8"));
}

function validInitializeInput(value: unknown): value is InitializeInput {
  if (!value || typeof value !== "object") return false;
  const input = value as Record<string, unknown>;
  return typeof input.model === "string"
    && input.model.length > 0
    && input.model.length <= 512
    && (input.visionModel === undefined || typeof input.visionModel === "string")
    && typeof input.computerUse === "boolean"
    && ["pi", "omp", "deepseek"].includes(String(input.harness));
}

function lmsCli(): string | undefined {
  const executable = process.platform === "win32" ? "lms.exe" : "lms";
  const candidates = [
    process.env.LMS_CLI_PATH,
    ...(process.env.PATH ?? "").split(delimiter).map((path) => join(path, executable)),
    join(homedir(), ".lmstudio", "bin", executable),
  ];
  return candidates.find((path): path is string => Boolean(path && existsSync(path)));
}

async function ensureLmStudioForPreview(): Promise<void> {
  try {
    const response = await fetch(`${lmStudioOrigin}/v1/models`);
    if (response.ok) return;
  } catch { /* start below */ }

  const cli = lmsCli();
  if (!cli) throw new Error("The LM Studio `lms` CLI was not found.");
  await execFileAsync(cli, ["server", "start", "--port", "1234", "--bind", "127.0.0.1"]);
  for (let attempt = 0; attempt < 40; attempt += 1) {
    try {
      const response = await fetch(`${lmStudioOrigin}/v1/models`);
      if (response.ok) return;
    } catch { /* retry during startup */ }
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
  throw new Error("LM Studio did not become healthy within 10 seconds.");
}

function lmStudioPreviewBridge(): Plugin {
  return {
    name: "vector-lm-studio-preview-bridge",
    apply: "serve",
    configureServer(server) {
      server.middlewares.use("/__vector/lm-studio", async (request, response) => {
        if (request.method !== "GET") {
          response.statusCode = 405;
          response.end("Method not allowed");
          return;
        }
        try {
          await ensureLmStudioForPreview();
          const upstream = await fetch(`${lmStudioOrigin}${request.url ?? "/v1/models"}`);
          response.statusCode = upstream.status;
          response.setHeader("content-type", upstream.headers.get("content-type") ?? "application/json");
          response.end(Buffer.from(await upstream.arrayBuffer()));
        } catch (error) {
          response.statusCode = 503;
          response.setHeader("content-type", "application/json");
          response.end(JSON.stringify({ error: error instanceof Error ? error.message : String(error) }));
        }
      });
    },
  };
}

function vectorPreviewBridge(): Plugin {
  return {
    name: "vector-native-preview-bridge",
    apply: "serve",
    configureServer(server) {
      server.middlewares.use("/__vector/system", (request, response) => {
        if (request.method !== "GET") return json(response, 405, { error: "Method not allowed" });
        const executable = process.platform === "win32" ? ".exe" : "";
        const pathEntries = (process.env.PATH ?? "").split(delimiter);
        const hasTool = (name: string) => pathEntries.some((path) => existsSync(join(path, `${name}${executable}`)));
        const configPath = join(previewWorkspace, ".vector", "vector.yaml");
        const configured = existsSync(configPath);
        const defaultProfile = configured
          ? readFileSync(configPath, "utf8").match(/^defaultProfile:\s*([^\s#]+)/m)?.[1]
          : undefined;
        json(response, 200, {
          os: process.platform,
          architecture: process.arch,
          cwd: previewWorkspace,
          telemetry: false,
          updateChecks: false,
          tools: Object.fromEntries(["git", "bun", "node", "omp", "pi", "npx"].map((tool) => [tool, hasTool(tool)])),
          configured,
          defaultProfile,
        });
      });

      server.middlewares.use("/__vector/initialize", async (request, response) => {
        if (request.method !== "POST") return json(response, 405, { error: "Method not allowed" });
        try {
          const input = await readJson(request);
          if (!validInitializeInput(input)) return json(response, 400, { error: "Invalid initialization request." });
          const args = [
            "run", "-q", "-p", "vector-agent", "--",
            "--workspace", previewWorkspace, "--json", "init",
            "--non-interactive", "--model", input.model, "--harness", input.harness,
          ];
          if (input.visionModel) args.push("--vision-model", input.visionModel);
          if (input.computerUse) args.push("--computer-use");
          const { stdout } = await execFileAsync("cargo", args, {
            cwd: sourceRoot,
            timeout: 60_000,
            maxBuffer: 2 * 1024 * 1024,
          });
          const result = JSON.parse(stdout) as { path: string; defaultProfile: string };
          json(response, 200, { path: result.path, defaultProfile: result.defaultProfile });
        } catch (error) {
          json(response, 500, { error: error instanceof Error ? error.message : String(error) });
        }
      });

      server.middlewares.use("/__vector/api", async (request, response) => {
        if (request.method !== "POST") return json(response, 405, { error: "Method not allowed" });
        try {
          const operation = await readJson(request);
          if (!validOperation(operation)) return json(response, 400, { error: "Invalid Vector operation." });
          const profile = operation.profile ?? "pi-safe";
          let result: unknown;
          switch (operation.action) {
            case "preflight":
              result = await runVector(["onboarding", "preflight", "--profile", profile]);
              break;
            case "install":
              if (!operation.harness) throw new Error("Harness is required.");
              result = await runVector(["harness", "install", operation.harness], 10 * 60_000);
              break;
            case "smoke":
              result = await runVector(["onboarding", "smoke", "--profile", profile], 3 * 60_000);
              break;
            case "computer":
              await ensurePreviewDaemon();
              if (!operation.visionModel) throw new Error("A vision-role model is required.");
              result = await runVector([
                "onboarding", "computer", "--profile", profile,
                "--vision-model", operation.visionModel,
                ...(operation.requestPermissions ? ["--request-permissions"] : []),
              ], 3 * 60_000);
              break;
            case "start": {
              await ensurePreviewDaemon();
              const args = ["start", "--profile", profile, "--surface", operation.surface ?? "integrated"];
              if (operation.prompt) args.push("--prompt", operation.prompt);
              if (operation.grantYolo) args.push("--grant-yolo", "--confirm-yolo", "VECTOR-YOLO");
              result = await runVector(args);
              break;
            }
            case "prompt":
              await ensurePreviewDaemon();
              if (!operation.runId || !operation.prompt) throw new Error("Run ID and prompt are required.");
              result = await runVector(["prompt", operation.runId, operation.prompt]);
              break;
            case "abort":
              await ensurePreviewDaemon();
              if (!operation.runId) throw new Error("Run ID is required.");
              result = await runVector(["abort", operation.runId]);
              break;
            case "stop":
              await ensurePreviewDaemon();
              if (!operation.runId) throw new Error("Run ID is required.");
              result = await runVector(["stop", operation.runId]);
              break;
            case "events":
              await ensurePreviewDaemon();
              if (!operation.runId) throw new Error("Run ID is required.");
              result = await runVector(["events", operation.runId, "--after", String(operation.afterSequence ?? 0)]);
              break;
          }
          json(response, 200, result);
        } catch (error) {
          const detail = error instanceof Error ? error.message : String(error);
          json(response, 500, { error: detail });
        }
      });
    },
  };
}

export default defineConfig({
  plugins: [react(), lmStudioPreviewBridge(), vectorPreviewBridge()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: { target: ["es2021", "chrome100", "safari13"], minify: "esbuild", sourcemap: true },
});
