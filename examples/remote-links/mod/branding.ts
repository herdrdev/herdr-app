/** Cosmetic branding for this dedicated browser only; no polling or extensions. */
export function titleBrandingSource(machine: string): string {
  const prefix = JSON.stringify(`Herdr · ${machine}`);
  return `(() => {
    const key = Symbol.for('herdr.remote-browser.title');
    if (globalThis[key]) return;
    globalThis[key] = true;
    const prefix = ${prefix};
    const update = () => {
      const title = document.title;
      if (title === prefix || title.startsWith(prefix + ' — ')) return;
      document.title = title ? prefix + ' — ' + title : prefix;
    };
    const start = () => {
      update();
      new MutationObserver(update).observe(document.head, { childList: true, subtree: true, characterData: true });
    };
    if (document.head) start();
    else document.addEventListener('DOMContentLoaded', start, { once: true });
  })()`;
}

export async function installBrowserBranding(endpoint: string, machine: string): Promise<() => void> {
  const version = await (await fetch(endpoint + "/json/version", { signal: AbortSignal.timeout(3000) })).json() as { webSocketDebuggerUrl: string };
  const url = new URL(version.webSocketDebuggerUrl);
  const expected = new URL(endpoint);
  if (url.protocol !== "ws:" || url.hostname !== "127.0.0.1" || url.port !== expected.port)
    throw new Error("Chrome branding endpoint is not the private local browser");
  const ws = new WebSocket(url.href);
  let nextId = 0;
  const pending = new Map<number, { resolve: (v: any) => void; reject: (e: Error) => void; timer: ReturnType<typeof setTimeout> }>();
  const send = (method: string, params: object, sessionId?: string): Promise<any> => new Promise((resolve, reject) => {
    const id = ++nextId;
    const timer = setTimeout(() => { pending.delete(id); reject(new Error("Chrome branding request timed out")); }, 3000);
    pending.set(id, { resolve, reject, timer });
    try { ws.send(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) })); }
    catch (error) { clearTimeout(timer); pending.delete(id); reject(error as Error); }
  });
  const cleanup = () => {
    for (const p of pending.values()) { clearTimeout(p.timer); p.reject(new Error("Browser branding closed")); }
    pending.clear();
  };
  ws.addEventListener("close", cleanup);
  ws.addEventListener("message", event => {
    const message = JSON.parse(String(event.data));
    if (message.id) {
      const p = pending.get(message.id);
      if (!p) return;
      clearTimeout(p.timer);
      pending.delete(message.id);
      if (message.error) p.reject(new Error(message.error.message)); else p.resolve(message.result);
    } else if (message.method === "Target.attachedToTarget") {
      if (message.params.targetInfo.type !== "page") return;
      const sessionId = message.params.sessionId;
      const source = titleBrandingSource(machine);
      // Install for every future document in the tab, then brand its current document.
      // Auto-attach also catches browser-created tabs and popups, not only our open RPC.
      (async () => {
        await send("Page.enable", {}, sessionId);
        await send("Page.addScriptToEvaluateOnNewDocument", { source }, sessionId);
        await send("Runtime.evaluate", { expression: source }, sessionId);
      })().catch(error => console.warn("Could not brand browser tab:", String(error)));
    }
  });
  try {
    await new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => { ws.close(); reject(new Error("Chrome branding connection timed out")); }, 3000);
      ws.addEventListener("open", () => { clearTimeout(timer); resolve(); }, { once: true });
      ws.addEventListener("error", () => { clearTimeout(timer); reject(new Error("Chrome branding connection failed")); }, { once: true });
    });
    await send("Target.setAutoAttach", { autoAttach: true, waitForDebuggerOnStart: false, flatten: true });
  } catch (error) { ws.close(); cleanup(); throw error; }
  return () => { ws.close(); cleanup(); };
}
