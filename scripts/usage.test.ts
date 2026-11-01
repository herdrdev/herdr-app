import { test, expect } from "bun:test";
import { EventEmitter } from "node:events";
import { PassThrough, Writable } from "node:stream";
import { quotaWindow, claudeQuota, codexQuota, providerOf, resetLabel } from "../defaults/mods/subscription-usage/quota.js";
import { readClaude, readCodex } from "../defaults/mods/subscription-usage/query.mjs";

test("unknown percentages never become full allowance; zero and exhaustion stay distinct", () => {
  for (const value of [null, undefined, "0", NaN, Infinity, -1, 101]) expect(quotaWindow("test", value, null)).toBeNull();
  expect(quotaWindow("test", 0, null)?.remaining).toBe(100);
  expect(quotaWindow("test", 100, null)?.remaining).toBe(0);
  expect(claudeQuota({ five_hour: {}, seven_day: null })).toEqual([]);
  expect(codexQuota({})).toEqual([]);
});

test("reset units, model windows, and multiple Codex buckets", () => {
  const at = Date.parse("2026-11-02T12:00:00Z");
  for (const reset of [at, at / 1000, "2026-11-02T12:00:00Z"]) expect(quotaWindow("x", 20, reset)?.resetsAt).toBe(at);
  expect(quotaWindow("x", 20, "invalid")?.resetsAt).toBeNull();
  expect(claudeQuota({ five_hour: { utilization: 20 }, seven_day_sonnet: { utilization: 50 } }).map(w => w.label)).toEqual(["5 hours", "7 days · sonnet"]);
  const bucket = { primary: { usedPercent: 80, windowDurationMins: 300, resetsAt: at / 1000 } };
  const result = codexQuota({ rateLimits: bucket, rateLimitsByLimitId: { codex: bucket, other: { secondary: { usedPercent: 0, windowDurationMins: 10080 } } } });
  expect(result.map(w => w.remaining)).toEqual([20, 100]);
  expect(result.map(w => w.label)).toEqual(["codex · 5 hours", "other · 7 days"]);
});

test("remote or unsupported agents are not counted against local quota; reset does not refill", () => {
  expect(providerOf({ address: "vps/t1", program: { key: "claude" } })).toBeNull();
  expect(providerOf({ address: "t1", program: { key: "other" } })).toBeNull();
  expect(providerOf({ address: "t1", program: { key: "codex" } })).toBe("codex");
  const now = 2000000000000;
  expect(resetLabel(now, now)).toContain("refresh");
});

test("Claude reads the selected credential root and emits no credentials/raw errors", async () => {
  let path, options;
  const result = await readClaude({ env: { CLAUDE_CONFIG_DIR: "/custom/account" }, platform: "linux",
    read: async p => { path = p; return JSON.stringify({ claudeAiOauth: { accessToken: "secret-canary", refreshToken: "refresh-canary" } }); },
    request: async (url, o) => { expect(url).toBe("https://api.anthropic.com/api/oauth/usage"); options = o; return { ok: true, json: async () => ({ five_hour: { utilization: 42 } }) }; },
  });
  expect(path.replaceAll("\\", "/")).toBe("/custom/account/.credentials.json");
  expect(options.headers.Authorization).toBe("Bearer secret-canary");
  expect(options.redirect).toBe("error");
  expect(JSON.stringify(result)).not.toContain("canary");
  expect(result.windows[0].remaining).toBe(58);
  for (const status of [401, 403, 429, 500]) {
    const failed = await readClaude({ read: async () => '{"claudeAiOauth":{"accessToken":"secret-canary"}}', request: async () => ({ ok: false, status, json: async () => ({ error: "secret-canary" }) }) });
    expect(failed.status).toBe("unavailable");
    expect(failed.windows).toEqual([]);
    expect(JSON.stringify(failed)).not.toContain("canary");
  }
});

test("Claude missing login, malformed JSON and network errors are unknown, without refresh", async () => {
  for (const raw of ["{}", "broken", '{"claudeAiOauth":{"accessToken":"test"}}']) {
    const result = await readClaude({ platform: "linux", read: async () => raw, request: async () => { throw new Error("secret-canary"); } });
    expect(result.status).toBe("unavailable");
    expect(JSON.stringify(result)).not.toContain("canary");
  }
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
  expect(result.windows[0].remaining).toBe(73);
  expect(sent.map(m => m.method)).toEqual(["initialize", "initialized", "account/rateLimits/read"]);
  expect(child.stdin.writableEnded).toBe(true);
});

test("Codex failure and timeout are bounded and do not expose raw server errors", async () => {
  const failed = rpcFixture((message, reply) => reply({ id: message.id, error: { message: "secret-canary" } }));
  const error = await readCodex({ start: () => failed.child, platform: "linux", deadline: 100 });
  expect(error.status).toBe("unavailable"); expect(JSON.stringify(error)).not.toContain("canary");
  const stalled = rpcFixture(() => {});
  const timeout = await readCodex({ start: () => stalled.child, platform: "linux", deadline: 10 });
  expect(timeout.status).toBe("unavailable"); expect(timeout.message).toContain("timed out");
  expect(stalled.child.stdin.writableEnded).toBe(true);
});
