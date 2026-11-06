import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { fileURLToPath } from "node:url";
import { Type } from "typebox";
import { requestBrowser } from "../../ipc.mjs";
export default function probe(pi: ExtensionAPI) {
  pi.registerTool({
    name: "browser_route_probe",
    label: "Browser route probe",
    description: "Exercise the browser hook's session ownership and cancelled reviews.",
    parameters: Type.Object({}),
    async execute(_id, _params, _signal, _update, ctx) {
      const session = ctx.sessionManager.getSessionId();
      const url = "http://localhost:12345";
      const receipts = [];
      for (const id of ["pn-111111", "pn-222222"]) {
        pi.events.emit("plannotator:session-opened", { id, piSessionId: session, url });
        pi.events.emit("plannotator:session-closed", { id, piSessionId: session, url });
        const child = await pi.exec(process.execPath, [fileURLToPath(new URL("../../open.mjs", import.meta.url)), url]);
        if (child.code !== 0) throw new Error(child.stderr);
        receipts.push(JSON.parse(child.stdout));
      }
      const wrongOwner = await requestBrowser(url, { ...process.env, HERDR_PLANNOTATOR_SESSION_ID: "another-session" });
      const wrongToken = await requestBrowser(url, { ...process.env, HERDR_PLANNOTATOR_TOKEN: "another-token" });
      pi.events.emit("plannotator:session-opened", { id: "pn-foreign", piSessionId: "another-session", url });
      const unmatched = await requestBrowser(url);
      const details = { session, configuredSession: process.env.HERDR_PLANNOTATOR_SESSION_ID, browser: process.env.PLANNOTATOR_BROWSER, socket: process.env.HERDR_PLANNOTATOR_SOCKET, receipts, wrongOwner, wrongToken, unmatched };
      return { content: [{ type: "text", text: JSON.stringify(details) }], details };
    },
  });
}
