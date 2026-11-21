import { div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { focused, terminals, appHome, exec, shared, setShared, openOverlay, closeOverlay, theme } from "herdr";

export const names = { codex: "Codex", cursor: "Cursor", agy: "Agy", claude: "Claude Code" };
const preference = "workspace.usage.providers";
const resumeFlags = { codex: ["resume"], cursor: ["--resume"], agy: ["--conversation"], claude: ["--resume", "--session-id"] };
const UUID = /^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/i;

function selection() {
  const terminal = focused()?.terminal;
  if (!terminal || terminal.includes("/")) return {};
  const t = terminals().find(row => row.address === terminal);
  const provider = t?.program?.key === "antigravity" ? "agy" : t?.program?.key;
  if (!Object.prototype.hasOwnProperty.call(names, provider)) return {};
  const foreground = t.foreground?.argv;
  const argv = Array.isArray(foreground) && foreground.length ? foreground : t.argv;
  const args = Array.isArray(argv) ? argv : [];
  let explicit;
  for (const [index, arg] of args.entries()) {
    const equal = typeof arg === "string" && arg.startsWith("--") ? arg.indexOf("=") : -1;
    const flag = equal === -1 ? arg : arg.slice(0, equal);
    if (!resumeFlags[provider].includes(flag)) continue;
    explicit = equal === -1 ? args[index + 1] : arg.slice(equal + 1);
    break;
  }
  const binding = t.labels?.["workspace.usage"];
  const prefix = provider + "/";
  const bound = binding?.startsWith(prefix) ? binding.slice(prefix.length) : "";
  const id = [explicit, bound].find(id => typeof id === "string" && UUID.test(id)) ?? "";
  return { provider, terminal, id };
}

function enabled() {
  const value = shared(preference);
  return Array.isArray(value) ? value.filter(key => Object.prototype.hasOwnProperty.call(names, key)) : [];
}

function keyFor(provider, selected) {
  return provider === selected.provider ? JSON.stringify([selected.terminal, selected.id]) : "";
}

function selectedSession(rows, selected, active) {
  if (!active.includes(selected.provider)) return null;
  const row = rows[selected.provider];
  return row?.key === keyFor(selected.provider, selected) ? row.session : null;
}

function reading(w) {
  return Math.round(w.remaining) + "%" + (w.resetAt <= Date.now() || Date.now() - w.observedAt > 900000 ? " (stale)" : "");
}

function sessionText(s) {
  if (!s) return "Session unavailable";
  const parts = [s.model, Number.isFinite(s.turns) && s.turns + (s.turns === 1 ? " turn" : " turns"),
    Number.isFinite(s.tokens) && s.tokens.toLocaleString() + " tokens"].filter(Boolean);
  return parts.join(" · ") || "Session " + String(s.id).slice(0, 8);
}

export class Usage {
  constructor() {
    this.rows = {};
    this.busy = new Set();
  }

  refresh(cx) {
    const active = enabled();
    if (!active.length) { this.rows = {}; return; }
    const selected = selection();
    for (const provider of Object.keys(names)) {
      if (!active.includes(provider)) {
        delete this.rows[provider];
        continue;
      }
      const key = keyFor(provider, selected);
      const previous = this.rows[provider];
      if (this.busy.has(provider) || (previous?.key === key && Date.now() - previous.at < 60000)) continue;
      this.busy.add(provider);
      cx.spawn(async cx => {
        let value;
        try {
          const args = [appHome() + "/mods/workspace-status/usage.cjs", "read", provider];
          if (key) args.push(selected.terminal, selected.id);
          const result = await exec("node", args, { timeout: 22000 });
          if (result.code !== 0) throw Error();
          value = JSON.parse(result.stdout);
        } catch {
          value = { windows: [], error: "Collector unavailable. Check Node.js and provider setup." };
        } finally {
          this.busy.delete(provider);
        }
        if (!enabled().includes(provider)) return;
        this.rows[provider] = { ...value, at: Date.now(), key };
        cx.notify();
      });
    }
  }

  button(compact) {
    const selected = selection();
    const active = enabled();
    const session = selectedSession(this.rows, selected, active);
    const labels = [];
    if (session) labels.push(sessionText(session));
    for (const id of active) {
      const row = this.rows[id];
      const first = row?.key === keyFor(id, selected) && row.windows?.[0];
      if (first) labels.push(names[id] + " " + first.label + " " + reading(first) + " left");
    }
    const text = compact ? (labels[session ? 1 : 0] || labels[0] || "Usage") : (labels.join("   ·   ") || "Usage");
    return div().id("workspace-usage").cursor_pointer().overflow_hidden().whitespace_nowrap()
      .tooltip("Session and local subscription usage").on_click(() => openOverlay({
        mod: "workspace-status", kind: "dialog", width: 460, height: 350,
        props: { details: true, rows: this.rows, selected },
      })).child(text);
  }
}

export function details({ rows = {}, selected = {}, overlay }) {
  const t = theme();
  const active = enabled();
  const session = selectedSession(rows, selected, active);
  const line = (text) => div().text_size(12).text_color(t.muted).child(text);
  const content = v_flex().gap(8).p(16).text_color(t.foreground)
    .child(div().font_semibold().child("Current session"))
    .child(line(selected.provider ? names[selected.provider] + " · " + sessionText(session) : "No local agent focused"));
  if (session) {
    for (const [field, label] of [["inputTokens", "Input"], ["outputTokens", "Output"], ["cacheTokens", "Cached"]]) {
      if (Number.isFinite(session[field])) content.child(line(label + " · " + session[field].toLocaleString()));
    }
  }
  content.child(div().pt(8).font_semibold().child("Local subscriptions"));
  for (const [id, name] of Object.entries(names)) {
    const on = active.includes(id);
    content.child(h_flex().items_center().child(div().flex_1().child(name))
      .child(div().id("enable-" + id).cursor_pointer().text_color(t.accent)
        .on_click(() => {
          setShared(preference, on ? active.filter(p => p !== id) : [...active, id]);
          closeOverlay(overlay);
        })
        .child(on ? "Disable" : "Enable")));
    if (!on) continue;
    const row = rows[id]?.key === keyFor(id, selected) ? rows[id] : null;
    if (!row?.windows?.length) content.child(line(row?.error || (row ? "Quota unavailable" : "Waiting for usage data")));
    for (const w of row?.windows || []) {
      const reset = w.resetAt ? " · resets " + new Date(w.resetAt).toLocaleString() : "";
      content.child(line(w.label + " · " + reading(w) + " remaining" + reset));
    }
  }
  return v_flex().size_full().bg(t.popover).child(v_flex().id("usage-scroll").flex_1().min_h(0).overflow_y_scroll().child(content));
}
