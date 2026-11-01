import { test } from "node:test";
import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { PassThrough, Writable } from "node:stream";
import { quotaWindow, claudeQuota, codexQuota, providerOf, resetLabel } from "./mods/subscription-usage/quota.js";
import { readClaude, readCodex } from "./mods/subscription-usage/query.mjs";

test("unknown percentages never become full allowance; zero and exhaustion stay distinct", () => {
  for (const value of [null, undefined, "0", NaN, Infinity, -1, 101]) assert.equal(quotaWindow("test", value, null), null);
  assert.equal(quotaWindow("test", 0, null)?.remaining, 100);
  assert.equal(quotaWindow("test", 100, null)?.remaining, 0);
  assert.deepEqual(claudeQuota({ five_hour: {}, seven_day: null }), []);
  assert.deepEqual(codexQuota({}), []);
});

test("reset units, model windows, and multiple Codex buckets", () => {
  const at = Date.parse("2026-11-02T12:00:00Z");
  for (const reset of [at, at / 1000, "2026-11-02T12:00:00Z"]) assert.equal(quotaWindow("x", 20, reset)?.resetsAt, at);
  assert.equal(quotaWindow("x", 20, "invalid")?.resetsAt, null);
  assert.deepEqual(claudeQuota({ five_hour: { utilization: 20 }, seven_day_sonnet: { utilization: 50 } }).map(w => w.label), ["5 hours", "7 days · sonnet"]);
  const bucket = { primary: { usedPercent: 80, windowDurationMins: 300, resetsAt: at / 1000 } };
  const result = codexQuota({ rateLimits: bucket, rateLimitsByLimitId: { codex: bucket, other: { secondary: { usedPercent: 0, windowDurationMins: 10080 } } } });
  assert.deepEqual(result.map(w => w.remaining), [20, 100]);
  assert.deepEqual(result.map(w => w.label), ["codex · 5 hours", "other · 7 days"]);
});

test("remote or unsupported agents are not counted against local quota; reset does not refill", () => {
  assert.equal(providerOf({ address: "vps/t1", program: { key: "claude" } }), null);
  assert.equal(providerOf({ address: "t1", program: { key: "other" } }), null);
  assert.equal(providerOf({ address: "t1", program: { key: "codex" } }), "codex");
  const now = 2000000000000;
  assert.ok(resetLabel(now, now).includes("refresh"));
});

test("Claude reads the selected credential root and emits no credentials/raw errors", async () => {
  let path, options;
  const result = await readClaude({ env: { CLAUDE_CONFIG_DIR: "/custom/account" }, platform: "linux",
    read: async p => { path = p; return JSON.stringify({ claudeAiOauth: { accessToken: "secret-canary", refreshToken: "refresh-canary" } }); },
    request: async (url, o) => { assert.equal(url, "https://api.anthropic.com/api/oauth/usage"); options = o; return { ok: true, json: async () => ({ five_hour: { utilization: 42 } }) }; },
  });
  assert.equal(path.replaceAll("\\", "/"), "/custom/account/.credentials.json");
  assert.equal(options.headers.Authorization, "Bearer secret-canary");
  assert.equal(options.redirect, "error");
  assert.ok(!JSON.stringify(result).includes("canary"));
  assert.equal(result.windows[0].remaining, 58);
  for (const status of [401, 403, 429, 500]) {
    const failed = await readClaude({ read: async () => '{"claudeAiOauth":{"accessToken":"secret-canary"}}', request: async () => ({ ok: false, status, json: async () => ({ error: "secret-canary" }) }) });
    assert.equal(failed.status, "unavailable");
    assert.deepEqual(failed.windows, []);
    assert.ok(!JSON.stringify(failed).includes("canary"));
  }
});

test("Claude missing login, malformed JSON and network errors are unknown, without refresh", async () => {
  for (const raw of ["{}", "broken", '{"claudeAiOauth":{"accessToken":"test"}}']) {
    const result = await readClaude({ platform: "linux", read: async () => raw, request: async () => { throw new Error("secret-canary"); } });
    assert.equal(result.status, "unavailable");
    assert.ok(!JSON.stringify(result).includes("canary"));
  }
});

test("macOS uses only the default keychain service, never guessing custom account services", async () => {
  let calls = 0;
  const options = { platform: "darwin", env: {}, read: async () => { throw new Error("missing file"); },
    keychain(program, args, opts, callback) {
      calls++;
      assert.equal(program, "security");
      assert.deepEqual(args, ["find-generic-password", "-s", "Claude Code-credentials", "-w"]);
      assert.equal(opts.timeout, 10000);
      callback(null, '{"claudeAiOauth":{"accessToken":"keychain-canary"}}');
    },
    request: async () => ({ ok: true, json: async () => ({ five_hour: { utilization: 10 } }) }),
  };
  const result = await readClaude(options);
  assert.equal(result.windows[0].remaining, 90);
  assert.ok(!JSON.stringify(result).includes("canary"));
  assert.equal((await readClaude({ ...options, env: { CLAUDE_CONFIG_DIR: "/custom" } })).status, "unavailable");
  assert.equal(calls, 1);
});

function rpcFixture(respond) {
  const child = new EventEmitter();
  const sent = [];
  child.pid = undefined;
  child.stdout = new PassThrough(); child.stderr = new PassThrough();
  child.stdin = new Writable({ write(chunk, _encoding, callback) {
    const message = JSON.parse(chunk.toString()); sent.push(message);
    queueMicrotask(() => respond(message, value => child.stdout.write(JSON.stringify(value) + "\n")));
    callback();
  } });
  return { child, sent };
}

test("Codex initializes before reading limits, ignores notifications, never starts a turn", async () => {
  const { child, sent } = rpcFixture((message, reply) => {
    if (message.id === 1) { reply({ method: "notice" }); reply({ id: 1, result: {} }); }
    if (message.id === 2) reply({ id: 2, result: { rateLimits: { primary: { usedPercent: 27, windowDurationMins: 300 } } } });
  });
  const result = await readCodex({ start: () => child, platform: "linux", deadline: 2000 });
  assert.equal(result.windows[0].remaining, 73);
  assert.deepEqual(sent.map(m => m.method), ["initialize", "initialized", "account/rateLimits/read"]);
  assert.equal(child.stdin.writableEnded, true);
});

test("Codex failure and timeout are bounded and do not expose raw server errors", async () => {
  const failed = rpcFixture((message, reply) => reply({ id: message.id, error: { message: "secret-canary" } }));
  const error = await readCodex({ start: () => failed.child, platform: "linux", deadline: 100 });
  assert.equal(error.status, "unavailable"); assert.ok(!JSON.stringify(error).includes("canary"));
  const stalled = rpcFixture(() => {});
  const timeout = await readCodex({ start: () => stalled.child, platform: "linux", deadline: 10 });
  assert.equal(timeout.status, "unavailable"); assert.ok(timeout.message.includes("timed out"));
  assert.equal(stalled.child.stdin.writableEnded, true);
});

test("Windows launches the npm command wrapper with fixed arguments and hidden pipes", async () => {
  const fixture = rpcFixture((message, reply) => reply({ id: message.id, error: { message: "not signed in" } }));
  let launch;
  const result = await readCodex({ platform: "win32", start: (...args) => { launch = args; return fixture.child; } });
  assert.equal(result.status, "unavailable");
  assert.equal(launch[0], process.env.ComSpec || "cmd.exe");
  assert.deepEqual(launch[1], ["/d", "/s", "/c", "codex app-server"]);
  assert.deepEqual(launch[2], { windowsHide: true, stdio: ["pipe", "pipe", "pipe"] });
});
