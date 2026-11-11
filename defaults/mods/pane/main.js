// A pane's header, for layouts that frame their panes:
//   { "tab": "active", "pane": { "header": { "content": { "mod": "pane" } } } }
// Shows the program's icon (the app's program table), the agent's name or
// the terminal's title (with active session title), the folder, and the agent's status;
// buttons split, zoom and close this pane. Props from the app: { pane, terminal }.

import { View, div, svg, image } from "gpui-kit";
import { h_flex } from "gpui-base";
import { panes, props, run, theme, home, layout, agents, terminals } from "herdr";

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

function getLiveTitle(p) {
  if (!p) return "";
  // 1. Check pane.title
  if (p.title && p.title.trim()) {
    const t = p.title.trim();
    if (!isGenericTitle(t, p)) return t;
  }
  // 2. Check agents()
  try {
    const agList = typeof agents === "function" ? agents() : [];
    const ag = agList.find(a => a.pane === p.pane || (p.terminal && a.address === p.terminal));
    if (ag?.title && ag.title.trim()) {
      const t = ag.title.trim();
      if (!isGenericTitle(t, p)) return t;
    }
  } catch (_) {}
  // 3. Check terminals()
  try {
    if (p.terminal && typeof terminals === "function") {
      const termList = terminals();
      const term = termList.find(t => t.address === p.terminal || t.address.endsWith("/" + p.terminal.split("/").pop()));
      if (term?.title && term.title.trim()) {
        const t = term.title.trim();
        if (!isGenericTitle(t, p)) return t;
      }
    }
  } catch (_) {}
  return p.title || "";
}

function formatTitle(p) {
  const progName = p.program?.name || p.title || "shell";
  const progKey = p.program?.key || "";
  const prefix = agentPrefix(progName, progKey);

  const raw = getLiveTitle(p);
  if (!raw || isGenericTitle(raw, p)) {
    return progName;
  }

  const trimmed = raw.trim();

  // If already formatted like "PREFIX | Title" (e.g. "OC | ...", "OMP | ...")
  if (trimmed.includes(" | ")) {
    return trimmed;
  }

  // Clean OMP / CLI status spinners and prompt symbols:
  // e.g. "π > 💻 OPEN: Chỉnh hình trekker và màu BHD" -> "OPEN: Chỉnh hình trekker và màu BHD"
  // e.g. "π ⠧ 💻 OPEN: Tích hợp CMS vào menubar admin" -> "OPEN: Tích hợp CMS vào menubar admin"
  // e.g. "π ⠇ 🖼️ PHOTO: Tái hiện Intro Grid Motion Transition" -> "PHOTO: Tái hiện Intro Grid Motion Transition"
  // e.g. "π > 📋 ENUM: Deploy Enum dropdown fix" -> "ENUM: Deploy Enum dropdown fix"
  let clean = trimmed
    .replace(/^[π\s>⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏•·\u2800-\u28FF]+/u, "")
    .replace(/^[\p{Emoji}\uFE0F\s]+/u, "")
    .trim();

  if (!clean || isGenericTitle(clean, p)) {
    return progName;
  }

  return `${prefix} | ${clean}`;
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

    const name = formatTitle(p);
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
