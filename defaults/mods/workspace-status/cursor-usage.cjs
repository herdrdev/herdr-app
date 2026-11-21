const fs = require("node:fs/promises");
const path = require("node:path");
const os = require("node:os");
const { readSnapshot, writeSnapshot, readHookInput, summarizeLog } = require("./usage-hook.cjs");

const UUID = /^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/i;

async function publish(data, terminal, appHome) {
  const id = data?.conversation_id;
  if (!terminal || typeof id !== "string" || !UUID.test(id)) return;
  const session = { id };
  if (typeof data.model === "string") session.model = data.model.slice(0, 120);
  await writeSnapshot("cursor", terminal, session, appHome);
}

function windowsFrom(summary, observedAt) {
  const plan = summary?.individualUsage?.plan;
  const windows = [];
  const resetAt = Date.parse(summary?.billingCycleEnd);
  for (const [label, field] of [["Plan", "totalPercentUsed"], ["Auto", "autoPercentUsed"], ["API", "apiPercentUsed"]]) {
    const used = plan?.[field];
    if (Number.isFinite(used) && used >= 0) {
      windows.push({ label, remaining: Math.max(0, 100 - used), observedAt, ...(Number.isFinite(resetAt) && { resetAt }) });
    }
  }
  return windows;
}

async function quota() {
  const file = process.env.HERDR_CURSOR_USAGE_FILE;
  if (file) {
    const stat = await fs.stat(file);
    if (!stat.isFile() || stat.size > 1024 * 1024) return [];
    return windowsFrom(JSON.parse(await fs.readFile(file, "utf8")), stat.mtimeMs);
  }
  const cookie = process.env.HERDR_CURSOR_USAGE_COOKIE;
  if (!cookie || !/^WorkosCursorSessionToken=[^;\s]+$/.test(cookie)) return [];
  // The opt-in session cookie is only for cursor.com's own usage dashboard.
  const response = await fetch("https://cursor.com/api/usage-summary", {
    headers: { Accept: "application/json", Cookie: cookie },
    redirect: "error", signal: AbortSignal.timeout(6000),
  });
  if (!response.ok) return [];
  return windowsFrom(await response.json(), Date.now());
}

async function countTurns(lines) {
  let turns = 0;
  let matched = false;
  for await (const line of lines) {
    let row;
    try { row = JSON.parse(line); } catch { continue; }
    if (row?.role === "user") turns++;
    if (row?.role === "user" || row?.role === "assistant") matched = true;
  }
  return matched ? { turns } : null;
}

async function readSession(terminal, id, home, appHome) {
  if (!terminal) return null;
  const snapshot = await readSnapshot("cursor", terminal, appHome);
  id ||= snapshot?.id;
  if (typeof id !== "string" || !UUID.test(id)) return null;
  const bound = snapshot?.id === id ? snapshot : null;
  const root = path.join(home, ".cursor", "projects");
  for (const dir of await fs.readdir(root, { withFileTypes: true }).catch(() => [])) {
    if (!dir.isDirectory()) continue;
    const file = path.join(root, dir.name, "agent-transcripts", id, id + ".jsonl");
    const stats = await summarizeLog("cursor", file, id, appHome, countTurns).catch(() => null);
    if (stats) return { id, ...bound, ...stats };
  }
  return bound;
}

async function collect({ terminal, sessionId, home = os.homedir(), appHome } = {}) {
  const [windows, session] = await Promise.all([
    quota().catch(() => []),
    readSession(terminal, sessionId, home, appHome),
  ]);
  return { windows, session };
}

module.exports = { collect, publish };
if (require.main === module) {
  readHookInput().then(data => publish(data, process.env.HC_TERMINAL_ID, path.resolve(__dirname, "../..")))
    .catch(() => {}).finally(() => process.stdout.write("{}\n"));
}
