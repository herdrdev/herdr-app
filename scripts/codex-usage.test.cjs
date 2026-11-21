const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const { quotaWindows, readSession } = require("../defaults/mods/workspace-status/codex-usage.cjs");

test("Codex preserves exhausted windows, meter names, and backend periods", () => {
  const primary = { usedPercent: 100, windowDurationMins: 300, resetsAt: 1000 };
  const regular = { primary, secondary: { usedPercent: 23, windowDurationMins: 10080 } };
  const result = { rateLimits: regular, rateLimitsByLimitId: {
    codex: regular, extra: { limitName: "Reserve", primary: { ...primary, usedPercent: 0 } },
  } };
  assert.deepEqual(quotaWindows(result), [
    { label: "5h", remaining: 0, resetAt: 1000000 },
    { label: "7d", remaining: 77 },
    { label: "Reserve 5h", remaining: 100, resetAt: 1000000 },
  ]);
  assert.deepEqual(quotaWindows({ rateLimits: regular }), quotaWindows(result).slice(0, 2));
  assert.deepEqual(quotaWindows({ rateLimits: { primary: {} } }), []);
});

test("Codex reads cumulative counters only for the bound thread", async t => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "codex-usage-"));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const thread = { id: "thread", model: "gpt-example", path: path.join(dir, "rollout.jsonl") };
  const context = { type: "turn_context", payload: { turn_id: "one" } };
  const usage = total_tokens => ({ type: "event_msg", payload: { type: "token_count",
    info: { total_token_usage: { total_tokens } } } });
  const lines = [context, usage(10), context, usage(24)].map(row => JSON.stringify(row)).join("\n");
  await fs.writeFile(thread.path, lines + "\n{");
  assert.equal(await readSession(thread, "another"), null);
  assert.deepEqual(await readSession(thread, "thread"), { id: "thread", model: "gpt-example", turns: 1, tokens: 24 });
  await fs.writeFile(thread.path, "x".repeat(3 * 1024 * 1024) + "\n" + lines + "\n");
  assert.deepEqual(await readSession(thread, "thread"), { id: "thread", model: "gpt-example", tokens: 24 });
});

test("the collector negotiates stdio and keeps session metrics when quota fails", {
  skip: process.platform === "win32" && "the fixture uses a Unix executable script",
}, async t => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), "codex-rpc-"));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const id = "01010101-0101-0101-0101-010101010101";
  const fake = `#!/usr/bin/env node
const { createInterface } = require("node:readline");
let ready = false;
createInterface({input: process.stdin}).on("line", line => {
    const packet = JSON.parse(line);
    if (packet.method === "initialized") { ready = true; return; }
    let result;
    if (packet.method === "initialize") result = {};
    else if (ready && packet.method === "thread/read") {
      result = {thread: {id: packet.params.threadId, model: "fixture-model"}};
    }
    console.log(JSON.stringify({ id: packet.id, result, error: result ? undefined : {code: -1} }));
});
`;
  await fs.writeFile(path.join(dir, "codex"), fake, { mode: 0o700 });
  const { promisify } = require("node:util");
  const exec = promisify(require("node:child_process").execFile);
  const helper = path.resolve(__dirname, "../defaults/mods/workspace-status/usage.cjs");
  const { stdout } = await exec(process.execPath, [helper, "read", "codex", "t1", id],
    { env: { ...process.env, PATH: dir + path.delimiter + process.env.PATH } });
  assert.deepEqual(JSON.parse(stdout), { windows: [], session: { id, model: "fixture-model" } });
});
