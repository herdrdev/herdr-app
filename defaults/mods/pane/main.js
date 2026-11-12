// A pane's header, for layouts that frame their panes:
//   { "tab": "active", "pane": { "header": { "content": { "mod": "pane" } } } }
// Shows the program's icon (the app's program table), the agent's name or
// the terminal's title (with active session title), the folder, and the agent's status;
// buttons split, zoom and close this pane. Props from the app: { pane, terminal }.

import { View, div, svg, image } from "gpui-kit";
import { h_flex } from "gpui-base";
import { panes, props, run, theme, home, layout, hc } from "herdr";

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

function stripAnsi(str) {
  if (!str) return "";
  return str.replace(/\x1B(?:[@-Z\\-_]|\[[0-?]*[ -/]*[@-~])/g, "");
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

function isGenericTitle(title, p) {
  if (!title) return true;
  const t = title.trim().toLowerCase();
  const progName = (p?.program?.name || "").toLowerCase();
  const progKey = (p?.program?.key || "").toLowerCase();
  const defaults = [
    "", "bash", "zsh", "sh", "fish", "powershell", "pwsh", "cmd", "shell",
    "terminal", "program", progName, progKey
  ];
  return defaults.includes(t);
}

function extractAgentTitle(agentType, rawScreen) {
  if (!rawScreen) return null;
  const screen = stripAnsi(rawScreen);
  const type = (agentType || "").toLowerCase();
  const rawLines = screen.split("\n");

  // 1. OMP
  if (type.includes("omp")) {
    for (let i = rawLines.length - 1; i >= 0; i--) {
      const line = rawLines[i].trim();
      const m = line.match(/(?:[\p{Emoji}\uFE0F\s]*)\s*([A-Z0-9_-]{2,20}:\s*[^|\n\r]{3,80})/u);
      if (m && !line.includes("π >") && !line.startsWith("├") && !line.startsWith("│") && !line.startsWith("╰")) {
        const label = m[1].split(":")[0].trim();
        if (label.endsWith("_TOKEN") || label.endsWith("_KEY") || label.endsWith("_PATH") || label.endsWith("_URL")) continue;
        return m[1].trim();
      }
    }
  }

  // 2. OpenCode
  if (type.includes("opencode")) {
    for (let i = 0; i < Math.min(3, rawLines.length); i++) {
      const line = rawLines[i].trim();
      if (line.endsWith("+")) {
        const tabs = line.split(/\s{2,}/).map(s => s.trim()).filter(s => s && s !== "+");
        if (tabs.length > 0) {
          return tabs[tabs.length - 1];
        }
      }
    }
  }

  // 3. Claude Code
  if (type.includes("claude")) {
    for (let i = rawLines.length - 1; i >= 0; i--) {
      const line = rawLines[i].trim();
      const m = line.match(/(?:recap:\s*(?:\[PELICAN\]\s*)?|●\s*\[PELICAN\]\s*)([^\n\r]{5,80})/);
      if (m) {
        let t = m[1].replace(/^[A-Za-z0-9_-]+:\s*/, "").trim();
        t = t.split(".")[0].trim();
        if (t.length > 45) t = t.slice(0, 42) + "...";
        return t;
      }
    }
  }

  // 4. Pi
  if (type.includes("pi")) {
    for (let i = rawLines.length - 1; i >= 0; i--) {
      const line = rawLines[i].trim();
      if (line.startsWith("π ") && line.includes(":")) {
        const parts = line.split(":");
        return parts.slice(1).join(":").trim();
      }
    }
  }

  // 5. Fallback for any agent status/task line
  for (let i = rawLines.length - 1; i >= 0; i--) {
    const line = rawLines[i].trim();
    if (line.startsWith("⚠") || line.startsWith("!") || line.toLowerCase().includes("warning")) continue;
    const m = line.match(/(?:[\p{Emoji}\uFE0F\s]*)\s*([A-Z0-9_-]{2,20}:\s*[^|\n\r]{3,80})/u);
    if (m && !line.includes("π >") && !line.startsWith("├") && !line.startsWith("│") && !line.startsWith("╰")) {
      const label = m[1].split(":")[0].trim();
      if (label.endsWith("_TOKEN") || label.endsWith("_KEY") || label.endsWith("_PATH") || label.endsWith("_URL")) continue;
      return m[1].trim();
    }
  }

  return null;
}

const terminalTitleCache = new Map();
const inFlightTerminals = new Set();

function resolveTerminalTitle(p, cx) {
  if (!p || !p.terminal) return;
  const terminal = p.terminal;
  const cached = terminalTitleCache.get(terminal);
  const now = Date.now();
  if (cached && (now - cached.time < 3500)) return;
  if (inFlightTerminals.has(terminal)) return;

  inFlightTerminals.add(terminal);

  cx.spawn(async (cx) => {
    try {
      const res = await hc(["read", terminal]).catch(() => null);
      if (res && res.code === 0 && res.stdout) {
        const agentType = p.program?.key || p.program?.name || "";
        const title = extractAgentTitle(agentType, res.stdout);
        if (title) {
          terminalTitleCache.set(terminal, { title, time: Date.now() });
          cx.notify();
        }
      }
    } catch (_) {
      // background scan fail-safe
    } finally {
      inFlightTerminals.delete(terminal);
    }
  });
}

function formatTitle(p, extraTitle) {
  const progName = p.program?.name || p.title || "shell";
  const progKey = p.program?.key || "";
  const prefix = agentPrefix(progName, progKey);

  // 1. If we have a resolved live session title from the terminal stream
  if (extraTitle && extraTitle.trim()) {
    let clean = extraTitle.trim();
    if (clean.includes(" | ")) {
      return clean;
    }
    // Remove leading spinners or emoji if prefix is already adding agent name
    clean = clean
      .replace(/^[π\s>⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏•·\u2800-\u28FF]+/u, "")
      .replace(/^[\p{Emoji}\uFE0F\s]+/u, "")
      .trim();
    if (clean && !isGenericTitle(clean, p)) {
      return `${prefix} | ${clean}`;
    }
  }

  // 2. If terminal title is already explicitly formatted (e.g. "OC | ...", "OMP | ...")
  const termTitle = (p.title || "").trim();
  if (termTitle.includes(" | ")) {
    return termTitle;
  }

  // 3. Fallback to meaningful p.title if not generic
  if (termTitle && !isGenericTitle(termTitle, p)) {
    return `${prefix} | ${termTitle}`;
  }

  return progName;
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

    const cached = p.terminal ? terminalTitleCache.get(p.terminal) : null;
    resolveTerminalTitle(p, cx);

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
