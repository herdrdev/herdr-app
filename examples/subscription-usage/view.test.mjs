import { test } from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { providerOf, resetLabel } from "./mods/subscription-usage/quota.js";

const source = (await readFile(new URL("./mods/subscription-usage/main.js", import.meta.url), "utf8"))
  .replace(/^import .*;\r?\n/gm, "").replace("export default class SubscriptionUsage", "class SubscriptionUsage");
const CACHE = "subscription-usage.result";

function harness({ data, list = [], liveWindow = {}, execute = async () => ({ code: 1 }) } = {}) {
  const store = new Map([[CACHE, data]]), commands = [], closed = [], tasks = [], handlers = new Map(), timers = [];
  let now = 2000000000000, words = [];
  function element() {
    let id;
    const el = new Proxy({}, { get: (_, method) => (...args) => {
      if (method === "id") id = args[0];
      if (method === "on_click") handlers.set(id, args[0]);
      if (method === "child" && typeof args[0] === "string") words.push(args[0]);
      if (method === "when" && args[0]) args[1](el);
      return el;
    } });
    return el;
  }
  const keys = ["View", "div", "v_flex", "h_flex", "agents", "appHome", "exec", "theme", "shared", "setShared", "run", "props", "closeOverlay", "providerOf", "resetLabel", "Date", "window"];
  const values = [class {}, element, element, element, () => list, () => "/app home", execute,
    () => ({}), k => store.get(k), (k, v) => store.set(k, v), (...args) => commands.push(args),
    () => ({ overlay: "dialog-1" }), id => closed.push(id), providerOf, resetLabel, class extends Date { static now() { return now; } }, () => liveWindow];
  const Usage = new Function(...keys, `${source}; return SubscriptionUsage;`)(...values);
  const view = new Usage();
  const cx = { notify() {}, spawn(fn) { tasks.push(fn(cx)); }, timer: { every(ms, fn) { timers.push({ ms, fn }); } } };
  return { view, cx, store, tasks, commands, closed, handlers, timers,
    advance(ms) { now += ms; }, render() { words = []; view.render(); return words.join("\n"); } };
}

test("standalone checker cannot query providers or persist a busy lock without a window", () => {
  const h = harness({ liveWindow: null, execute: async () => assert.fail("must not run a helper") });
  h.view.init({}, h.cx);
  assert.equal(h.tasks.length, 0);
  assert.equal(h.store.has("subscription-usage.started"), false);
  assert.ok(h.render().includes("Not checked"));
});

test("render distinguishes zero, unavailable and stale quota; only local supported agents can be focused", () => {
  const data = {
    claude: { status: "ok", checkedAt: 2000000000000, windows: [{ label: "5 hours", remaining: 0, resetsAt: 2000000000001 }] },
    codex: { status: "unavailable", message: "Sign in to Codex", windows: [] },
  };
  const h = harness({ data, list: [
    { address: "t1", name: "Example task", status: "working", program: { key: "claude" } },
    { address: "remote/t2", name: "Remote task", program: { key: "claude" } },
    { address: "t3", name: "Other task", program: { key: "other" } },
  ] });
  let text = h.render();
  assert.ok(text.includes("0% left"));
  assert.ok(!text.includes("100% left"));
  assert.ok(text.includes("Sign in to Codex"));
  assert.ok(text.includes("account matching not verified"));
  assert.ok(text.includes("Example task"));
  assert.ok(!text.includes("Remote task"));
  assert.ok(!text.includes("Other task"));
  h.handlers.get("usage-agent-t1")();
  assert.deepEqual(h.commands, [["agent.focus", { terminal: "t1" }]]);
  assert.deepEqual(h.closed, ["dialog-1"]);
  h.advance(2);
  assert.ok(h.render().includes("0% left · stale"));
  assert.ok(h.render().includes("Reset due"));
});

test("refresh is shared, bounded by cooldown, clears failed data and rejects late replies", async () => {
  let resolveOld, calls = 0;
  const h = harness({ data: { claude: { status: "ok" } }, execute: async (_program, args) => {
    calls++;
    assert.deepEqual(args, ["/app home/mods/subscription-usage/query.mjs"]);
    if (calls === 1) return await new Promise(resolve => { resolveOld = resolve; });
    return { code: 1 };
  } });
  h.view.refresh(h.cx);
  h.view.refresh(h.cx);
  assert.equal(calls, 1);
  h.advance(36000);
  h.view.refresh(h.cx);
  await h.tasks[1];
  assert.equal(h.store.get(CACHE).claude.status, "unavailable");
  resolveOld({ code: 0, stdout: JSON.stringify({ claude: { status: "ok" }, codex: { status: "ok" } }) });
  await h.tasks[0];
  assert.equal(h.store.get(CACHE).claude.status, "unavailable");
  h.view.refresh(h.cx);
  assert.equal(calls, 2);
});

test("opening after an abandoned query retries the expired lock, then resumes two-minute polling", async () => {
  let calls = 0;
  const h = harness({ execute: async () => { calls++; return { code: 1 }; } });
  h.store.set("subscription-usage.started", 2000000000000);
  h.store.set("subscription-usage.attempt", 2000000000000);
  h.view.init({}, h.cx);
  assert.equal(calls, 0);
  assert.equal(h.timers[0].ms, 5000);
  h.advance(35000);
  h.timers[0].fn(h.cx);
  await h.tasks[0];
  assert.equal(calls, 1);
  h.advance(35000);
  h.timers[0].fn(h.cx);
  assert.equal(calls, 1);
  h.advance(85000);
  h.timers[0].fn(h.cx);
  await h.tasks[1];
  assert.equal(calls, 2);
});
