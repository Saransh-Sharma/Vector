import { execFile } from "node:child_process";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { delimiter, join } from "node:path";
import { promisify } from "node:util";
import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";

const execFileAsync = promisify(execFile);
const lmStudioOrigin = "http://127.0.0.1:1234";

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

export default defineConfig({
  plugins: [react(), lmStudioPreviewBridge()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: { target: ["es2021", "chrome100", "safari13"], minify: "esbuild", sourcemap: true },
});
