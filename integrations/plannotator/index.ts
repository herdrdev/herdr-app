import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { release } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { closeReview, openReview, resolveApp, runJson } from "./open.mjs";
import { createReviewRouter } from "./router.mjs";
import { startBrowserIpc } from "./ipc.mjs";

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

  pi.on("session_start", async (_event, ctx) => {
    if (process.env.HERDR_PLANNOTATOR_MODE === "disabled" || prior.get("PLANNOTATOR_BROWSER") !== undefined) return;
    context = ctx;
    const session = ctx.sessionManager.getSessionId();
    router = createReviewRouter(session, { open: openReview, close: closeReview });
    ipc = await startBrowserIpc(session, router, report);
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

  pi.events.on("plannotator:session-opened", event => router?.opened(event));
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
