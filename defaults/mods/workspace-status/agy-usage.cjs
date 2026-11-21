const path = require("node:path");
const os = require("node:os");
const { readSnapshot, writeSnapshot, readHookInput, summarizeLog } = require("./usage-hook.cjs");

const UUID = /^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/i;

async function publish(data, terminal, appHome) {
  if (!terminal || typeof data?.session_id !== "string" || !UUID.test(data.session_id)) return;
  const session = { id: data.session_id };
  if (typeof data.model?.display_name === "string") session.model = data.model.display_name.slice(0, 120);
  const observedAt = Date.now();
  const windows = [];
  for (const [label, quota] of Object.entries(data.quota || {})) {
    const fraction = quota?.remaining_fraction;
    if (!Number.isFinite(fraction) || fraction < 0 || fraction > 1) continue;
    const resetAt = Date.parse(quota.reset_time);
    windows.push({ label, remaining: fraction * 100, observedAt, ...(Number.isFinite(resetAt) && { resetAt }) });
  }
  await writeSnapshot("agy", terminal, { session, windows }, appHome);
  await writeSnapshot("agy", null, { windows }, appHome);
}

async function summarize(lines) {
  const steps = new Map();
  for await (const line of lines) {
    let row;
    try { row = JSON.parse(line); } catch { continue; }
    if (!Number.isSafeInteger(row?.step_index)) continue;
    if (row.type === "USER_INPUT") steps.set(row.step_index, { turn: true });
    if (row.type === "PLANNER_RESPONSE" || row.type === "CHECKPOINT") {
      steps.set(row.step_index, {
        input: row.input_tokens, output: row.output_tokens, cache: row.cache_read_tokens ?? 0,
      });
    }
  }
  if (!steps.size) return null;
  let turns = 0, inputTokens = 0, outputTokens = 0, cacheTokens = 0;
  let complete = true, responses = 0;
  for (const step of steps.values()) {
    if (step.turn) { turns++; continue; }
    responses++;
    if (![step.input, step.output, step.cache].every(n => Number.isSafeInteger(n) && n >= 0)) {
      complete = false;
      continue;
    }
    inputTokens += step.input;
    outputTokens += step.output;
    cacheTokens += step.cache;
  }
  return { turns, ...(responses && complete ? { inputTokens, outputTokens, cacheTokens, tokens: inputTokens + outputTokens + cacheTokens } : {}) };
}

async function collect({ terminal, sessionId, home = os.homedir(), appHome } = {}) {
  const data = await readSnapshot("agy", terminal || null, appHome);
  const binding = data?.session;
  const id = sessionId || binding?.id;
  const bound = binding && id === binding.id ? binding : null;
  const windows = (bound ? data : await readSnapshot("agy", null, appHome))?.windows || [];
  if (!terminal || typeof id !== "string" || !UUID.test(id)) return { windows, session: null };
  for (const dir of ["antigravity-cli", "antigravity"]) {
    const file = path.join(home, ".gemini", dir, "brain", id, ".system_generated", "logs", "transcript.jsonl");
    const stats = await summarizeLog("agy", file, id, appHome, summarize).catch(() => null);
    if (stats) return { windows, session: { id, ...bound, ...stats } };
  }
  return { windows, session: bound };
}

module.exports = { collect, publish };
if (require.main === module) {
  readHookInput(process.argv.includes("--passthrough")).then(data => publish(data, process.env.HC_TERMINAL_ID, path.resolve(__dirname, "../.."))).catch(() => {});
}
