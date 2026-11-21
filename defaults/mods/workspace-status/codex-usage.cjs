const { spawn } = require("node:child_process");
const fs = require("node:fs/promises");
const { createInterface } = require("node:readline");

const uuid = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i;

function quotaWindows(result) {
  const groups = Object.entries(result?.rateLimitsByLimitId ?? {});
  if (!groups.length && result?.rateLimits) groups.push(["codex", result.rateLimits]);
  const windows = [];
  for (const [id, group] of groups) {
    for (const key of ["primary", "secondary"]) {
      const window = group?.[key];
      if (!Number.isFinite(window?.usedPercent)) continue;
      const mins = window.windowDurationMins;
      let period = key;
      if (Number.isFinite(mins) && mins > 0) {
        if (mins % 1440 === 0) period = `${mins / 1440}d`;
        else if (mins % 60 === 0) period = `${mins / 60}h`;
        else period = `${mins}m`;
      }
      windows.push({
        label: id === "codex" ? period : `${group.limitName || id} ${period}`,
        remaining: Math.max(0, Math.min(100, 100 - window.usedPercent)),
        ...(Number.isFinite(window.resetsAt) && { resetAt: window.resetsAt * 1000 }),
      });
    }
  }
  return windows;
}

// A token_count contains a cumulative snapshot. Bound disk I/O on every poll
// even when one thread contains months of tool output.
async function readSession(thread, id) {
  if (!thread || thread.id !== id) return null;
  const session = { id };
  if (thread.model) session.model = thread.model;
  if (!thread.path) return session;
  const file = await fs.open(thread.path, "r").catch(() => null);
  if (!file) return session;
  try {
    const stat = await file.stat();
    if (!stat.isFile()) return session;
    const size = Math.min(stat.size, 2 * 1024 * 1024);
    const buffer = Buffer.alloc(size);
    const { bytesRead } = await file.read(buffer, 0, size, stat.size - size);
    const lines = buffer.toString("utf8", 0, bytesRead).split("\n");
    if (size < stat.size) lines.shift();
    const turns = new Set();
    for (const line of lines) {
      if (!line.includes('"token_count"') && !line.includes('"turn_context"') && !line.includes('"token_usage_record"')) continue;
      let event;
      try { event = JSON.parse(line); } catch { continue; }
      const payload = event?.payload;
      if (event?.type === "turn_context" && payload?.turn_id) turns.add(payload.turn_id);
      const usage = event?.type === "token_usage_record" ? payload?.thread_token_usage
        : event?.type === "event_msg" && payload?.type === "token_count" ? payload.info?.total_token_usage : null;
      if (usage) {
        for (const [field, key] of Object.entries({
          total_tokens: "tokens", input_tokens: "inputTokens",
          output_tokens: "outputTokens", cached_input_tokens: "cacheTokens",
        })) {
          if (Number.isSafeInteger(usage[field]) && usage[field] >= 0) session[key] = usage[field];
        }
      }
    }
    // A tail cannot tell how many earlier turns were omitted.
    if (size === stat.size && turns.size) session.turns = turns.size;
  } finally {
    await file.close();
  }
  return session;
}

async function collect({ sessionId } = {}) {
  const id = typeof sessionId === "string" && uuid.test(sessionId) ? sessionId : null;
  const child = spawn("codex", ["app-server"], {
    stdio: ["pipe", "pipe", "ignore"], windowsHide: true,
  });
  const lines = createInterface({ input: child.stdout });
  let thread;
  let limits;
  let receivedQuota = false;
  let receivedThread = !id;
  let initialized = false;

  // The provider owns this child. Never attach to, stop or mutate a running Codex session.
  await new Promise(resolve => {
    const timer = setTimeout(finish, 8000);
    function finish() {
      clearTimeout(timer);
      resolve();
    }
    function send(message) {
      child.stdin.write(JSON.stringify(message) + "\n");
    }
    child.once("error", finish);
    child.once("close", finish);
    child.stdin.on("error", finish);
    lines.on("line", line => {
      let msg;
      try { msg = JSON.parse(line); } catch { return; }
      if (msg?.id === 1 && !initialized) {
        if (msg.error) return finish();
        initialized = true;
        send({ method: "initialized" });
        send({ id: 2, method: "account/rateLimits/read", params: { excludeResetCreditDetails: true } });
        if (id) send({ id: 3, method: "thread/read", params: { threadId: id, includeTurns: false } });
      } else if (msg?.id === 2) {
        limits = msg.result;
        receivedQuota = true;
      } else if (msg?.id === 3) {
        thread = msg.result?.thread;
        receivedThread = true;
      }
      if (receivedQuota && receivedThread) finish();
    });
    send({ id: 1, method: "initialize", params: {
      clientInfo: { name: "herdr_workspace_status", title: "Herdr", version: "1" }, capabilities: null,
    } });
  });
  lines.close();
  child.stdin.destroy();
  child.kill();
  return { windows: quotaWindows(limits), session: await readSession(thread, id) };
}

module.exports = { collect, quotaWindows, readSession };
