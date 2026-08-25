import { execFile } from "node:child_process";
import { existsSync } from "node:fs";
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
        json(response, 200, {
          os: process.platform,
          architecture: process.arch,
          cwd: previewWorkspace,
          telemetry: false,
          updateChecks: false,
          tools: Object.fromEntries(["git", "bun", "node", "omp", "pi", "npx"].map((tool) => [tool, hasTool(tool)])),
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
