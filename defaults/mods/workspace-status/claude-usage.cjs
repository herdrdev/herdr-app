const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { readSnapshot, writeSnapshot, readHookInput, summarizeLog } = require("./usage-hook.cjs");

const UUID = /^[a-f\d]{8}-(?:[a-f\d]{4}-){3}[a-f\d]{12}$/i;
const windowNames = [["five_hour", "5h"], ["seven_day", "7d"], ["seven_day_opus", "Opus 7d"], ["spend_limit", "Spend"]];

async function readJson(file) {
  try { return JSON.parse(await fs.promises.readFile(file, "utf8")); } catch { return null; }
}

function windows(limits, observedAt, source) {
  const result = [];
  for (const [key, label] of windowNames) {
    const entry = limits?.[key];
    const used = source === "hook" ? entry?.used_percentage : entry?.utilization;
    if (!Number.isFinite(used) || used < 0 || used > 100) continue;
    const resetAt = source === "hook" ? (Number.isFinite(entry.resets_at) ? entry.resets_at * 1000 : NaN) : Date.parse(entry.resets_at);
    result.push({ label, remaining: 100 - used, observedAt, ...(Number.isFinite(resetAt) && { resetAt }) });
  }
  return result;
}

async function defaultQuota(home) {
  const profile = await readJson(path.join(home, ".claude.json"));
  const cached = profile?.cachedUsageUtilization;
  if (!profile?.oauthAccount?.accountUuid || cached?.accountUuid !== profile.oauthAccount.accountUuid ||
      !Number.isFinite(cached.fetchedAtMs) || cached.fetchedAtMs <= 0) return [];
  return windows(cached.utilization, cached.fetchedAtMs, "cache");
}

// Called inside Claude's terminal. Only allowlisted status-line fields leave stdin.
async function publish(data, terminal, appHome) {
  if (!terminal || !UUID.test(data?.session_id)) return;
  const transcript = data.transcript_path;
  const id = data.session_id;
  const snapshot = { id, windows: windows(data.rate_limits, Date.now(), "hook") };
  if (typeof transcript === "string" && path.isAbsolute(transcript) && path.basename(transcript) === id + ".jsonl") {
    snapshot.transcript = transcript;
  }
  if (typeof data.model?.display_name === "string") snapshot.model = data.model.display_name;
  await writeSnapshot("claude", terminal, snapshot, appHome);
}

async function readSession(filename, id, appHome) {
  return summarizeLog("claude", filename, id, appHome, async lines => {
    const summary = { id, turns: 0 };
    const messages = new Map();
    const prompts = new Set();
    let matched = false;
    for await (const line of lines) {
      let row;
      try { row = JSON.parse(line); } catch { continue; }
      if (row?.sessionId !== id || row.isSidechain) continue;
      matched = true;
      const msg = row.message;
      if (row.type === "user" && !row.isMeta && !row.toolUseResult && !prompts.has(row.uuid)) {
        const content = msg?.content;
        if (typeof content === "string" || (Array.isArray(content) && content.some(b => b?.type === "text"))) {
          summary.turns++;
          if (row.uuid) prompts.add(row.uuid);
        }
      }
      if (row.type !== "assistant" || !msg || msg.model === "<synthetic>") continue;
      if (typeof msg.model === "string" && msg.model) summary.model = msg.model;
      if (!msg.usage) continue;
      const counts = {};
      for (const [field, key] of [["input_tokens", "inputTokens"], ["output_tokens", "outputTokens"],
        ["cache_creation_input_tokens", "cacheTokens"], ["cache_read_input_tokens", "cacheTokens"]]) {
        const n = msg.usage[field];
        if (Number.isSafeInteger(n) && n >= 0) counts[key] = (counts[key] || 0) + n;
      }
      // Streamed blocks can repeat a request with updated final usage.
      const key = msg.id ? JSON.stringify([msg.id, row.requestId]) : row.uuid;
      if (key && Object.keys(counts).length) messages.set(key, counts);
    }
    if (!matched) return null;
    for (const counts of messages.values()) {
      for (const [key, count] of Object.entries(counts)) {
        summary[key] = (summary[key] || 0) + count;
        summary.tokens = (summary.tokens || 0) + count;
      }
    }
    return summary;
  });
}

async function session(configDir, id, snapshot, appHome) {
  if (typeof id !== "string" || !UUID.test(id)) return null;
  if (snapshot?.transcript) {
    const current = await readSession(snapshot.transcript, id, appHome).catch(() => null);
    if (current) return current;
  } else {
    const projects = path.join(configDir, "projects");
    for (const dir of await fs.promises.readdir(projects, { withFileTypes: true }).catch(() => [])) {
      if (!dir.isDirectory()) continue;
      const current = await readSession(path.join(projects, dir.name, id + ".jsonl"), id, appHome).catch(() => null);
      if (current) return current;
    }
  }
  return snapshot ? { id, ...(snapshot.model && { model: snapshot.model }) } : null;
}

async function collect({ terminal, sessionId, home = os.homedir(), appHome,
  configDir = process.env.CLAUDE_CONFIG_DIR || path.join(home, ".claude") } = {}) {
  const snapshot = terminal ? await readSnapshot("claude", terminal, appHome) : null;
  const bound = snapshot && (!sessionId || snapshot.id === sessionId) ? snapshot : null;
  const id = sessionId || bound?.id;
  const accountWindows = !bound && path.resolve(configDir) === path.resolve(home, ".claude") ? defaultQuota(home) : [];
  const [usageWindows, current] = await Promise.all([
    bound ? bound.windows : accountWindows,
    id ? session(configDir, id, bound, appHome) : null,
  ]);
  return { windows: usageWindows || [], session: current };
}

if (require.main === module && process.argv[2] === "status-line") {
  (async () => {
    const passthrough = process.argv.includes("--passthrough");
    const data = await readHookInput(passthrough);
    if (process.env.HC_TERMINAL_ID) await publish(data, process.env.HC_TERMINAL_ID, path.resolve(__dirname, "../.."));
  })().catch(() => {}); // A status-line integration must let the user's own command run.
}

module.exports = { collect };
