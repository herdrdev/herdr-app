import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { release } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { randomUUID } from "node:crypto";
import { importFresh } from "./fresh-import.mjs";
import { createReviewRouter } from "./router.mjs";
import { createRequestNamer } from "./requested.mjs";
import type { startBrowserIpc } from "./ipc.mjs";

/** Route Plannotator's supported browser hook without changing its Ask AI bridge. */
export default function herdrPlannotator(pi: ExtensionAPI): void {
  let context: ExtensionContext | undefined;
  let router: ReturnType<typeof createReviewRouter> | undefined;
  let ipc: Awaited<ReturnType<typeof startBrowserIpc>> | undefined;
  const keys = ["PLANNOTATOR_BROWSER", "PLANNOTATOR_REMOTE", "HERDR_PLANNOTATOR_SESSION_ID", "HERDR_PLANNOTATOR_NODE", "HERDR_PLANNOTATOR_SOCKET", "HERDR_PLANNOTATOR_TOKEN"];
  const prior = new Map(keys.map(key => [key, process.env[key]]));
  const helper = path.join(path.dirname(fileURLToPath(import.meta.url)), process.platform === "win32" ? "open.cmd" : "open.mjs");
  const report = (error: unknown) => {
    console.error(error);
    context?.ui.notify(`Plannotator browser: ${error instanceof Error ? error.message : String(error)}`, "warning");
  };
  const requestScript = path.join(path.dirname(fileURLToPath(import.meta.url)), "request.mjs");

  // Herdr's tab bar asks for these: as if the person typed the command here.
  // Pi runs extension commands at once, even while the agent works.
  const requests = createRequestNamer();
  const runCommand = async (command: string) => {
    const name = command === "review" ? "plannotator-review" : "plannotator-last";
    if (!pi.getCommands().some(entry => entry.name === name)) throw new Error("Plannotator is not loaded in this Pi");
    requests.expect(command);
    void Promise.resolve(pi.sendUserMessage(`/${name}`, { expandPromptTemplates: true })).catch(report);
  };

  // The terminal label tells Herdr how to reach this Pi: request.mjs, run on
  // this machine, reads it back and checks that this Pi still runs there.
  const publish = async (session: string, server: { socket: string; token: string }) => {
    const terminal = process.env.HC_TERMINAL_ID;
    if (!terminal || !process.env.HC_TERMINAL_SOCKET) return;
    try {
      // Nested Pi processes inherit HC_*: only the terminal's own Pi publishes.
      const owner = await pi.exec("hc", ["get", terminal, "--json"], { timeout: 3000 });
      if (owner.code !== 0 || JSON.parse(owner.stdout).foreground?.pid !== process.pid) return;
      const label = { pid: process.pid, session, socket: server.socket, token: server.token, node: process.execPath, request: requestScript };
      const result = await pi.exec("hc", ["label", terminal, `herdr.plannotator=${JSON.stringify(label)}`], { timeout: 3000 });
      if (result.code !== 0) throw new Error(result.stderr.trim() || "hc label failed");
    } catch (error) {
      report(new Error(`Herdr cannot reach this session for annotations: ${error instanceof Error ? error.message : String(error)}`));
    }
  };

  pi.on("session_start", async (_event, ctx) => {
    const browser = prior.get("PLANNOTATOR_BROWSER");
    const ownHook = browser === helper && prior.get("HERDR_PLANNOTATOR_SOCKET") !== undefined;
    if (process.env.HERDR_PLANNOTATOR_MODE === "disabled" || (browser !== undefined && !ownHook)) return;
    // Node retains native ESM imports across Pi reloads; load the current browser hook.
    const module = new URL("./open.mjs", import.meta.url);
    module.searchParams.set("load", randomUUID());
    const { closeReview, openReview, resolveApp, runJson } = await importFresh(module.href) as typeof import("./open.mjs");
    const ipcModule = new URL("./ipc.mjs", import.meta.url);
    ipcModule.searchParams.set("load", randomUUID());
    const { startBrowserIpc } = await importFresh(ipcModule.href) as typeof import("./ipc.mjs");
    context = ctx;
    const session = ctx.sessionManager.getSessionId();
    router = createReviewRouter(session, { open: openReview, close: closeReview });
    ipc = await startBrowserIpc(session, router, report, runCommand);
    void publish(session, ipc);
    process.env.HERDR_PLANNOTATOR_SESSION_ID = session;
    process.env.HERDR_PLANNOTATOR_NODE = process.execPath;
    process.env.HERDR_PLANNOTATOR_SOCKET = ipc.socket;
    process.env.HERDR_PLANNOTATOR_TOKEN = ipc.token;
    process.env.PLANNOTATOR_BROWSER = helper;
    // WSL and its Windows host share localhost. An SSH transport into that WSL
    // core must not disable Plannotator's in-process Ask this session provider.
    if (prior.get("PLANNOTATOR_REMOTE") === undefined && process.env.HC_TERMINAL_ID && /microsoft/i.test(release())) {
      try {
        const api = await runJson(await resolveApp(), ["--browser-api"]);
        if (api.version === 1) process.env.PLANNOTATOR_REMOTE = "0";
      } catch { /* An unmodified/missing app keeps Plannotator's remote policy. */ }
    }
  });

  pi.events.on("plannotator:session-opened", event => router?.opened(requests.name(event)));
  pi.events.on("plannotator:session-closed", event => { void router?.closed(event).catch(report); });
  pi.on("session_shutdown", async () => {
    if (!ipc) return;
    try { await router?.shutdown(); }
    catch (error) { report(error); }
    finally {
      try { await ipc.stop(); }
      finally {
        for (const [key, value] of prior) {
          if (value === undefined) delete process.env[key]; else process.env[key] = value;
        }
        router = undefined; ipc = undefined; context = undefined;
      }
    }
  });
}
