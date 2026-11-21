const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const test = require("node:test");
const { collect } = require("../defaults/mods/workspace-status/claude-usage.cjs");

test("Claude quota belongs to the signed-in account, including exhausted windows", async t => {
  const home = await fs.mkdtemp(path.join(os.tmpdir(), "claude-quota-"));
  t.after(() => fs.rm(home, { recursive: true, force: true }));
  const observedAt = Date.now();
  const resets = new Date(observedAt + 60000);
  const profile = { oauthAccount: { accountUuid: "one" }, cachedUsageUtilization: {
    accountUuid: "two", fetchedAtMs: observedAt,
    utilization: { five_hour: { utilization: 100, resets_at: resets }, seven_day: {} },
  } };
  const save = () => fs.writeFile(path.join(home, ".claude.json"), JSON.stringify(profile));
  await save();
  assert.deepEqual((await collect({ home })).windows, []);
  profile.oauthAccount.accountUuid = "two";
  await save();
  assert.deepEqual((await collect({ home })).windows, [{ label: "5h", remaining: 0, observedAt, resetAt: resets.getTime() }]);
});

test("Claude reports only the bound session, counting repeated streaming blocks once", async t => {
  const home = await fs.mkdtemp(path.join(os.tmpdir(), "claude-session-"));
  t.after(() => fs.rm(home, { recursive: true, force: true }));
  const configDir = path.join(home, ".claude");
  const dir = path.join(configDir, "projects", "one");
  await fs.mkdir(dir, { recursive: true });
  const id = "f4643693-dece-4243-bb3c-29f873cfab49";
  const user = { sessionId: id, type: "user", uuid: "u", message: { content: [{ type: "text", text: "hello" }] } };
  const assistant = { sessionId: id, type: "assistant", requestId: "r", message: {
    id: "m", model: "example", usage: { input_tokens: 10, output_tokens: 2, cache_read_input_tokens: 3 }
  } };
  const rows = [user, user, { ...user, uuid: "tool", message: { content: [{ type: "tool_result" }] } },
    assistant, assistant, { ...assistant, sessionId: "unrelated" }, { ...assistant, isSidechain: true }];
  await fs.writeFile(path.join(dir, id + ".jsonl"), rows.map(r => JSON.stringify(r)).join("\n") + "\n{");
  assert.deepEqual((await collect({ home, configDir, appHome: home, sessionId: id })).session, { id, turns: 1, model: "example", inputTokens: 10, outputTokens: 2, cacheTokens: 3, tokens: 15 });
  assert.equal((await collect({ home, configDir })).session, null);
  assert.equal((await collect({ home, configDir, appHome: home, sessionId: "../escape" })).session, null);
});

test("Claude status line forwards JSON intact and associates custom profiles only with their terminal", async t => {
  const { spawnSync } = require("node:child_process");
  const home = await fs.mkdtemp(path.join(os.tmpdir(), "claude-status-line-"));
  t.after(() => fs.rm(home, { recursive: true, force: true }));
  const mod = path.join(home, "mods", "workspace-status");
  await fs.mkdir(mod, { recursive: true });
  for (const name of ["claude-usage.cjs", "usage-hook.cjs"]) {
    await fs.copyFile(require.resolve("../defaults/mods/workspace-status/" + name), path.join(mod, name));
  }
  const helper = path.join(mod, "claude-usage.cjs");
  const configDir = path.join(home, "alt");
  await fs.mkdir(configDir, { recursive: true });
  const id = "f4643693-dece-4243-bb3c-29f873cfab49";
  const filename = path.join(configDir, id + ".jsonl");
  const event = { sessionId: id, type: "assistant", message: { id: "msg1", model: "test", usage: { output_tokens: 3 } } };
  await fs.writeFile(filename, JSON.stringify(event) + "\n");
  const line = { session_id: id, model: { display_name: "Test" }, transcript_path: filename,
    rate_limits: { five_hour: { used_percentage: 27.5, resets_at: 1795281577 } } };
  const input = JSON.stringify(line, null, 2) + "\n";
  const child = spawnSync(process.execPath, [helper, "status-line", "--passthrough"], {
    input, encoding: "utf8", env: { ...process.env, HERDR_APP_HOME: home, HC_TERMINAL_ID: "t1" },
  });
  assert.equal(child.status, 0);
  assert.equal(child.stdout, input);
  const options = { home, appHome: home, configDir, terminal: "t1" };
  const result = await collect(options);
  assert.equal(result.windows.length, 1);
  assert.equal(result.windows[0].remaining, 72.5);
  assert.equal(result.windows[0].resetAt, 1795281577000);
  assert.equal(result.session.tokens, 3);
  assert.deepEqual(await collect({ ...options, terminal: "t2" }), { windows: [], session: null });

  // The summary survives collector process restarts and changes when the transcript grows.
  assert.deepEqual(await collect(options), result);
  event.message.id = "msg2";
  await fs.appendFile(filename, JSON.stringify(event) + "\n");
  assert.equal((await collect(options)).session.outputTokens, 6);
});
