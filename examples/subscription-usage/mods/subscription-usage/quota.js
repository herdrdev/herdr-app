// Provider data is untrusted: absent or malformed percentages are unknown, not zero.
export function quotaWindow(label, used, reset) {
  if (typeof used !== "number" || !Number.isFinite(used) || used < 0 || used > 100) return null;
  let resetsAt = null;
  if (typeof reset === "number" && Number.isFinite(reset) && reset > 0) resetsAt = reset < 1e11 ? reset * 1000 : reset;
  if (typeof reset === "string" && reset.trim()) resetsAt = Date.parse(reset);
  if (!Number.isFinite(resetsAt) || resetsAt > 8.64e15) resetsAt = null;
  return { label, used, remaining: 100 - used, resetsAt };
}

export function claudeQuota(data) {
  const windows = [];
  for (const [key, value] of Object.entries(data ?? {})) {
    if (!value || !(key === "five_hour" || key.startsWith("seven_day"))) continue;
    const label = key === "five_hour" ? "5 hours" : key === "seven_day" ? "7 days" : `7 days · ${key.slice(10).replace(/_/g, " ")}`;
    const window = quotaWindow(label, value.utilization ?? value.used_percentage, value.resets_at);
    if (window) windows.push(window);
  }
  return windows;
}

export function codexQuota(data) {
  const buckets = data?.rateLimitsByLimitId;
  const entries = buckets && Object.keys(buckets).length ? Object.entries(buckets) : [["codex", data?.rateLimits]];
  return entries.flatMap(([id, bucket]) => ["primary", "secondary"].flatMap(key => {
    const value = bucket?.[key];
    if (!value) return [];
    const mins = value.windowDurationMins;
    const period = typeof mins !== "number" || !Number.isFinite(mins) || mins <= 0 ? key
      : mins % 1440 === 0 ? `${mins / 1440} days` : mins % 60 === 0 ? `${mins / 60} hours` : `${mins} minutes`;
    const name = bucket.limitName || id;
    const window = quotaWindow(`${name} · ${period}`, value.usedPercent, value.resetsAt);
    return window ? [window] : [];
  }));
}

export function providerOf(agent) {
  if (agent.address?.includes("/")) return null;
  const key = agent.program?.key?.toLowerCase();
  return key === "claude" || key === "codex" ? key : null;
}

export function resetLabel(at, now = Date.now()) {
  if (at === null) return "Reset time unavailable";
  if (at <= now) return "Reset due · refresh to check";
  const minutes = Math.ceil((at - now) / 60000);
  const relative = minutes < 60 ? `${minutes}m` : minutes < 1440 ? `${Math.floor(minutes / 60)}h ${minutes % 60}m` : `${Math.floor(minutes / 1440)}d ${Math.floor(minutes % 1440 / 60)}h`;
  return `Resets in ${relative} · ${new Date(at).toLocaleString()}`;
}
