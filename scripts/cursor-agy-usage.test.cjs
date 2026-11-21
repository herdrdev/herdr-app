const { test } = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const agy = require("../defaults/mods/workspace-status/agy-usage.cjs");
const cursor = require("../defaults/mods/workspace-status/cursor-usage.cjs");

const first = "49f35021-99a9-454a-a029-8eeddde44c6b";
const second = "f5655cd7-6208-47a3-a710-c6a003e86538";

async function fixture(t) {
  const home = await fs.mkdtemp(path.join(os.tmpdir(), "herdr-usage-"));
  t.after(() => fs.rm(home, { recursive: true, force: true }));
  return { home, appHome: path.join(home, "app") };
}

async function writeLines(file, lines) {
  await fs.mkdir(path.dirname(file), { recursive: true });
  await fs.writeFile(file, lines.map(row => JSON.stringify(row)).join("\n") + "\n");
}

test("Agy reads only the bound terminal's own turns and preserves an exhausted subscription", async t => {
  const env = await fixture(t);
  const file = path.join(env.home, ".gemini/antigravity-cli/brain", first, ".system_generated/logs/transcript.jsonl");
  const lines = [{ type: "USER_INPUT", step_index: 0 }, { type: "PLANNER_RESPONSE", step_index: 1, input_tokens: 50, cache_read_tokens: 120, output_tokens: 7 }];
  await writeLines(file, [...lines, lines[1], { type: "CHECKPOINT", step_index: 2, input_tokens: 13, output_tokens: 6 }]);
  await agy.publish({ session_id: first, model: { display_name: "Gemini" }, quota: {
    weekly: { remaining_fraction: 0, reset_time: "2027-04-12T11:12:00Z" }, unknown: {},
  } }, "t1", env.appHome);
  const focused = await agy.collect({ ...env, terminal: "t1" });
  assert.deepEqual(focused.session, { id: first, model: "Gemini", turns: 1, inputTokens: 63, cacheTokens: 120, outputTokens: 13, tokens: 196 });
  assert.equal(focused.windows.length, 1);
  assert.equal(focused.windows[0].remaining, 0);
  assert.equal(focused.windows[0].resetAt, Date.parse("2027-04-12T11:12:00Z"));
  assert.deepEqual(await agy.collect({ ...env, terminal: "t2" }), { session: null, windows: focused.windows });
  assert.deepEqual(await agy.collect({ ...env, terminal: null }), { session: null, windows: focused.windows });
  assert.equal((await agy.collect({ ...env, terminal: "t1", sessionId: second })).session, null);
  await writeLines(file, [...lines, { type: "PLANNER_RESPONSE", step_index: 3 }]);
  assert.equal((await agy.collect({ ...env, terminal: "t1" })).session.tokens, undefined);
});

test("Cursor uses exact conversation ID across two chats and reports only measured plan windows", async t => {
  const env = await fixture(t);
  const billing = path.join(env.home, "usage.json");
  const old = process.env.HERDR_CURSOR_USAGE_FILE;
  process.env.HERDR_CURSOR_USAGE_FILE = billing;
  t.after(() => {
    if (old === undefined) delete process.env.HERDR_CURSOR_USAGE_FILE;
    else process.env.HERDR_CURSOR_USAGE_FILE = old;
  });
  await fs.writeFile(billing, JSON.stringify({ billingCycleEnd: "2027-01-01", individualUsage: { plan: { totalPercentUsed: 25, autoPercentUsed: 105, apiPercentUsed: null } } }));
  const root = path.join(env.home, ".cursor/projects/repo/agent-transcripts");
  await writeLines(path.join(root, first, first + ".jsonl"), [{ role: "user" }, { role: "assistant" }]);
  await writeLines(path.join(root, second, second + ".jsonl"), [{ role: "user" }, { role: "assistant" }, { role: "user" }]);
  await cursor.publish({ conversation_id: first, model: "model-a", user_email: "ignored" }, "t1", env.appHome);
  const a = await cursor.collect({ ...env, terminal: "t1" });
  assert.deepEqual(a.session, { id: first, model: "model-a", turns: 1 });
  assert.deepEqual(a.windows.map(w => [w.label, w.remaining]), [["Plan", 75], ["Auto", 0]]);
  const b = await cursor.collect({ ...env, terminal: "t1", sessionId: second });
  assert.deepEqual(b.session, { id: second, turns: 2 });
  assert.equal((await cursor.collect({ ...env, terminal: "t2" })).session, null);
  assert.equal((await cursor.collect({ ...env, terminal: "t1", sessionId: "../escape" })).session, null);
  await fs.writeFile(billing, "broken");
  assert.deepEqual((await cursor.collect({ ...env, terminal: "t1" })).windows, []);
});

test("Cursor sends an explicitly configured session cookie only to the usage endpoint", async t => {
  const env = await fixture(t);
  const previous = { file: process.env.HERDR_CURSOR_USAGE_FILE, cookie: process.env.HERDR_CURSOR_USAGE_COOKIE };
  t.after(() => {
    for (const [name, value] of Object.entries({ HERDR_CURSOR_USAGE_FILE: previous.file, HERDR_CURSOR_USAGE_COOKIE: previous.cookie })) {
      if (value === undefined) delete process.env[name];
      else process.env[name] = value;
    }
  });
  delete process.env.HERDR_CURSOR_USAGE_FILE;
  process.env.HERDR_CURSOR_USAGE_COOKIE = "WorkosCursorSessionToken=fixture";
  const mock = t.mock.method(globalThis, "fetch", async () => Response.json({ individualUsage: { plan: { totalPercentUsed: 35 } } }));
  const result = await cursor.collect(env);
  assert.equal(result.windows[0].remaining, 65);
  const [url, options] = mock.mock.calls[0].arguments;
  assert.equal(url, "https://cursor.com/api/usage-summary");
  assert.equal(options.headers.Cookie, "WorkosCursorSessionToken=fixture");
  assert.equal(options.redirect, "error");
});

test("installed Agy status line publishes into its own app home and preserves an existing renderer's stdin", async t => {
  const { execFileSync } = require("node:child_process");
  const env = await fixture(t);
  const mod = path.join(env.appHome, "mods", "workspace-status");
  await fs.mkdir(mod, { recursive: true });
  for (const file of ["agy-usage.cjs", "usage-hook.cjs", "usage.cjs"]) {
    await fs.copyFile(path.join(__dirname, "../defaults/mods/workspace-status", file), path.join(mod, file));
  }
  const options = { env: { ...process.env, HC_TERMINAL_ID: "t1", HOME: env.home, HERDR_APP_HOME: path.join(env.home, "other-home") }, encoding: "utf8" };
  const input = JSON.stringify({ session_id: first, model: { display_name: "Gemini" }, quota: { week: { remaining_fraction: 0.75 } } });
  assert.equal(execFileSync(process.execPath, [path.join(mod, "agy-usage.cjs"), "--passthrough"], { ...options, input }), input);
  const raw = execFileSync(process.execPath, [path.join(mod, "usage.cjs"), "read", "agy", "t1"], options);
  assert.equal(JSON.parse(raw).windows[0].remaining, 75);
  assert.equal(JSON.parse(raw).session.id, first);
});
