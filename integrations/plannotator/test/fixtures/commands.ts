import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { Type } from "typebox";
import { requestCommand } from "../../ipc.mjs";

// Plannotator's commands, recorded instead of opening a review, and a tool
// that asks for them the way Herdr's tab bar does.
export default function commands(pi: ExtensionAPI) {
  const ran: string[] = [];
  for (const name of ["plannotator-review", "plannotator-last"]) {
    pi.registerCommand(name, { description: name, handler: async () => { ran.push(name); } });
  }
  pi.registerTool({
    name: "annotation_request_probe",
    label: "Annotation request probe",
    description: "Ask this Pi for Plannotator commands through its terminal label.",
    parameters: Type.Object({}),
    async execute(_id, _params, _signal, _update, ctx) {
      const request = fileURLToPath(new URL("../../request.mjs", import.meta.url));
      const labels = () => JSON.parse(readFileSync(process.env.FAKE_HC_STATE!, "utf8")).labels;
      for (let i = 0; i < 50; i++) {
        try { if (labels()["herdr.plannotator"]) break; } catch { /* not written yet */ }
        await new Promise(resolve => setTimeout(resolve, 100));
      }
      const label = JSON.parse(labels()["herdr.plannotator"]);
      const runs = [];
      for (const command of ["review", "last", "other"]) {
        const child = await pi.exec(process.execPath, [request, "t1", command]);
        runs.push({ command, code: child.code, stderr: child.stderr.trim() });
      }
      for (let i = 0; i < 50 && ran.length < 2; i++) await new Promise(resolve => setTimeout(resolve, 100));
      const wrongToken = await requestCommand("review", { ...label, token: "another-token" });
      const details = { session: ctx.sessionManager.getSessionId(), label: { ...label, token: typeof label.token }, runs, ran, wrongToken };
      return { content: [{ type: "text", text: JSON.stringify(details) }], details };
    },
  });
}
