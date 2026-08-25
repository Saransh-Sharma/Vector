import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const runId = "__VECTOR_RUN_ID__";
const yolo = __VECTOR_YOLO__;
const policy = JSON.parse(__VECTOR_POLICY_DOCUMENT__) as Record<string, {
  effective: "allow" | "prompt" | "deny";
}>;

function capabilityForTool(name: string): string {
  if (["read", "grep", "find", "ls"].includes(name)) return "filesystem.read";
  if (["write", "edit"].includes(name)) return "filesystem.workspace-write";
  if (name === "bash") return "process.execute";
  if (name.startsWith("computer_")) return "computer.control";
  return "mcp.write";
}

export default function vectorPolicy(pi: ExtensionAPI) {
  pi.on("tool_call", async (event) => {
    const capability = capabilityForTool(event.toolName);
    const decision = policy[capability]?.effective ?? "deny";
    if (decision === "deny") {
      return { block: true, reason: `Vector denied ${capability} for run ${runId}` };
    }
    if (decision === "prompt" && !yolo) {
      const approved = await pi.ui.confirm("Vector approval", `${event.toolName} requests ${capability}`);
      if (!approved) return { block: true, reason: `User denied ${capability}` };
    }
    return undefined;
  });
}

