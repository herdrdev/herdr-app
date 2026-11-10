// A pane's header, for layouts that frame their panes:
//   { "tab": "active", "pane": { "header": { "content": { "mod": "pane" } } } }
// Shows the program's icon (the app's program table), the agent's name or
// the terminal's title (with active session title), the folder, and the agent's status;
// buttons split, zoom and close this pane. Props from the app: { pane, terminal }.

import { View, div, svg, image } from "gpui-kit";
import { h_flex } from "gpui-base";
import { panes, props, run, theme, home, exec, spaces, layout } from "herdr";

// Match the first visible tab frame, including the active child of a stack.
function paneFrame(node) {
  if (!node || node.hidden) return null;
  if (node.content?.tab && node.content.pane) return node.content.pane;
  let children = node.children ?? [];
  if (node.type === "stack") {
    const visible = children.filter(child => !child.hidden);
    children = [visible.find(child => child.id === node.active) ?? visible[0]];
  }
  for (const child of children) {
    const frame = paneFrame(child);
    if (frame) return frame;
  }
  return null;
}

function icon(p, size, color) {
  if (!p?.icon) return null;
  return p.tint ? svg(p.icon).size(size).flex_none().text_color(color) : image(p.icon).w(size).h(size).flex_none();
}

function folder(cwd) {
  if (!cwd) return "";
  const h = home();
  return cwd === h ? "~" : cwd.startsWith(h + "/") ? "~" + cwd.slice(h.length) : cwd;
}

function agentPrefix(progName, progKey) {
  const k = (progKey || "").toLowerCase();
  const n = (progName || "").toLowerCase();
  if (k === "opencode" || n.includes("opencode")) return "OC";
  if (k === "omp" || n === "omp") return "OMP";
  if (k === "claude" || n.includes("claude")) return "Claude";
  if (k === "codex" || n.includes("codex")) return "Codex";
  if (k === "antigravity" || n.includes("antigravity") || n === "agy") return "AGY";
  if (k === "gemini" || n.includes("gemini")) return "Gemini";
  if (k === "pi" || n === "pi") return "Pi";
  if (k === "cursor" || n.includes("cursor")) return "Cursor";
  return progName || "Agent";
}

function formatTitle(p, extraTitle) {
  const progName = p.program?.name || p.title || "shell";
  const progKey = p.program?.key || "";
  const termTitle = (p.title || "").trim();
  const prefix = agentPrefix(progName, progKey);

  // 1. If terminal title already has formatted session (e.g. "OC | ...", "OMP | ...")
  if (termTitle.includes(" | ")) {
    return termTitle;
  }

  // 2. If we resolved a session title from agent history
  if (extraTitle && extraTitle.trim()) {
    const clean = extraTitle
      .replace(/^[\s\S]*?(THREAD|SERVER):\s*/i, "")
      .replace(/^[\p{Emoji}\uFE0F\s]+/u, "")
      .trim();
    if (clean) {
      return `${prefix} | ${clean}`;
    }
  }

  // 3. If terminal title is a meaningful custom title (not generic shell/program)
  const isDefault = [
    progName.toLowerCase(),
    progKey.toLowerCase(),
    "bash", "zsh", "sh", "fish", "powershell", "pwsh", "cmd", "shell"
  ].includes(termTitle.toLowerCase());

  if (termTitle && !isDefault) {
    return `${prefix} | ${termTitle}`;
  }

  return progName;
}

const sessionCache = new Map();
const inFlight = new Set();

function targetMachine(p) {
  if (p?.terminal?.includes("/")) return p.terminal.split("/")[0];
  const space = spaces().find(s => s.active);
  return space?.machine ?? null;
}

function resolveSessionTitle(p, cx) {
  if (!p || !p.pane || !p.cwd) return;
  const prog = (p.program?.name || p.program?.key || p.title || "").toLowerCase();
  const on = targetMachine(p);
  const cacheKey = `${p.pane}:${on || 'local'}:${p.cwd}`;

  const cached = sessionCache.get(cacheKey);
  if (cached && (Date.now() - cached.time < 5000)) return;
  if (inFlight.has(cacheKey)) return;

  inFlight.add(cacheKey);

  cx.spawn(async (cx) => {
    try {
      let title = null;
      if (prog.includes("omp")) {
        const py = `import sqlite3, os
p = os.path.expanduser('~/.omp/agent/history.db')
cwd = ${JSON.stringify(p.cwd)}
if os.path.exists(p):
    try:
        con = sqlite3.connect(p)
        r = con.execute("SELECT title FROM session_titles WHERE session_id IN (SELECT session_id FROM history WHERE cwd=? ORDER BY created_at DESC LIMIT 1)", [cwd]).fetchone()
        if not r:
            r = con.execute("SELECT title FROM session_titles ORDER BY rowid DESC LIMIT 1").fetchone()
        if r and r[0]:
            print(r[0])
    except Exception:
        pass
`;
        const res = await exec("python3", ["-c", py], { on }).catch(() => null);
        if (res && res.code === 0 && res.stdout && res.stdout.trim()) {
          title = res.stdout.trim().split("\n")[0];
        }
      } else if (prog.includes("claude")) {
        const py = `import json, os
p = os.path.expanduser('~/.claude/history.jsonl')
cwd = ${JSON.stringify(p.cwd)}
if os.path.exists(p):
    try:
        lines = open(p, 'r', encoding='utf-8', errors='ignore').readlines()
        for line in reversed(lines):
            try:
                data = json.loads(line)
                if data.get('project') == cwd and data.get('display'):
                    d = data['display'].strip()
                    if not d.startswith('/'):
                        print(d)
                        break
            except Exception:
                continue
    except Exception:
        pass
`;
        const res = await exec("python3", ["-c", py], { on }).catch(() => null);
        if (res && res.code === 0 && res.stdout && res.stdout.trim()) {
          title = res.stdout.trim().split("\n")[0];
        }
      }

      if (title) {
        sessionCache.set(cacheKey, { title, time: Date.now() });
        cx.notify();
      }
    } catch (_) {
      // Ignore background detection errors
    } finally {
      inFlight.delete(cacheKey);
    }
  });
}

export default class Pane extends View {
  init() {
    this.hovered = false;
  }

  me() {
    return panes().find((p) => p.pane === props()?.pane);
  }

  button(t, id, icon, command, args) {
    return div()
      .id(id)
      .p(3)
      .rounded(4)
      .text_color(t.dim)
      .hover((s) => s.bg(t.hover).text_color(t.foreground))
      .on_click((_e, cx) => {
        cx.stop_propagation();
        run(command, args);
      })
      .child(svg(`mods/pane/${icon}.svg`).size(14));
  }

  render(cx) {
    const t = theme();
    const p = this.me();
    if (!p) return div();

    const on = targetMachine(p);
    const cacheKey = `${p.pane}:${on || 'local'}:${p.cwd}`;
    const cached = sessionCache.get(cacheKey);

    if (!p.title || !p.title.includes(" | ")) {
      resolveSessionTitle(p, cx);
    }

    const name = formatTitle(p, cached?.title);
    const pic = icon(p.program, 14, t.dim);
    const st = p.status;
    const pane = p.pane;
    const bottom = paneFrame(layout())?.header?.position === "bottom";

    return h_flex()
      .id(`pane-head-${pane}`)
      .size_full()
      .px(10)
      .gap(8)
      .items_center()
      .bg(t.background)
      .when(bottom, el => el.border_t(1))
      .when(!bottom, el => el.border_b(1))
      .border_color(t.border)
      .text_size(12)
      .on_hover((hovered, cx) => { this.hovered = hovered; cx.notify(); })
      .on_click(() => run("pane.focus", { pane }))
      .when(Boolean(pic), (el) => el.child(pic))
      .child(div().flex_none().truncate().max_w(380).font_medium().text_color(t.foreground).child(name))
      .child(div().flex_1().truncate().text_color(t.dim).child(folder(p.cwd)))
      .when(st === "working", el => el.child(svg("mods/workspace-chrome/spinner.svg").size(12).flex_none().text_color(t.working).spin(900)))
      .when(st === "blocked", el => el.child(div().flex_none().px(6).h(18).flex().items_center().rounded(5).bg(t.blocked + "26").text_size(10.5).font_semibold().text_color(t.blocked).child("Needs you")))
      .when(st === "done", el => el.child(div().size(7).flex_none().rounded(4).bg(t.done)))
      .when(this.hovered, el => el
        .child(this.button(t, `ph-r-${pane}`, "split-right", "pane.split", { direction: "right", pane }))
        .child(this.button(t, `ph-d-${pane}`, "split-down", "pane.split", { direction: "down", pane }))
        .child(this.button(t, `ph-z-${pane}`, p.zoomed ? "unzoom" : "zoom", "pane.zoom", { pane }))
        .child(this.button(t, `ph-x-${pane}`, "close", "pane.close", { pane })));
  }
}
