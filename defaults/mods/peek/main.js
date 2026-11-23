// The bar on top of a peeked agent (an overlay the agents list opens for a
// sub-agent, or a terminal address link): who it is and how it's doing, ↗ to
// go to its pane (or, without one, keep it as a tab in a space on its machine:
// the shown one, else its nearest ancestor's, else the first), ✕ to close.
// Props: { terminal, overlay }.

import { View, div, image, svg } from "gpui-kit";
import { h_flex } from "gpui-base";
import { agents, terminals, panes, spaces, run, theme, props, closeOverlay } from "herdr";

const SPIN = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
// The icon from the app's program table: a one-color glyph or a picture.
function icon(p, size, color) {
  if (!p?.icon) return null;
  return p.tint ? svg(p.icon).size(size).flex_none().text_color(color) : image(p.icon).w(size).h(size).flex_none();
}
const WORDS = { working: "working", blocked: "needs you", done: "finished", idle: "idle" };

export default class Peek extends View {
  init(_props, cx) {
  }

  term() {
    return terminals().find((t) => t.address === props()?.terminal);
  }

  status() {
    const addr = props()?.terminal;
    return agents().find((a) => a.address === addr)?.status ?? this.term()?.status ?? null;
  }

  // A pane stays on its terminal's machine (terminal.keep refuses another one's space).
  keepSpace(terminal, parent) {
    const machine = terminal?.includes("/") ? terminal.split("/")[0] : null;
    const all = spaces();
    const shown = all.find((s) => s.active) ?? all[0];
    if ((shown?.machine ?? null) === machine) return shown;
    const there = all.filter((s) => (s.machine ?? null) === machine);
    if (there.length < 2) return there[0];
    const allPanes = panes();
    const allTerminals = terminals();
    const seen = new Set();
    while (parent && !seen.has(parent)) {
      seen.add(parent);
      const tab = allPanes.find((p) => p.terminal === parent)?.tab;
      const space = there.find((s) => s.tabs.some((c) => c.id === tab));
      if (space) return space;
      parent = allTerminals.find((t) => t.address === parent)?.parent;
    }
    return there[0];
  }

  button(t, id, label, onClick) {
    return div()
      .id(id)
      .px(8)
      .py(2)
      .rounded(5)
      .text_color(t.muted)
      .cursor_pointer()
      .hover((s) => s.bg(t.hover).text_color(t.foreground))
      .on_click(onClick)
      .child(label);
  }

  render() {
    const t = theme();
    const { terminal, overlay } = props() ?? {};
    const k = this.term();
    const st = this.status();
    const pic = icon(k?.program, 14, t.foreground);
    const parent = k?.parent ? terminals().find((p) => p.address === k.parent) : null;
    const home = panes().some((p) => p.terminal === terminal);
    const space = home ? null : this.keepSpace(terminal, k?.parent);
    const glyph = st === "working" ? SPIN[0] : st === "idle" ? "○" : st ? "●" : "·";
    return h_flex()
      .size_full()
      .px(12)
      .gap(8)
      .items_center()
      .bg(t.surface)
      .border_b(1)
      .border_color(t.border)
      .text_size(12)
      .child(div().w(12).flex_none().text_color(t[st] ?? t.dim).when(st === "working", (el) => el.frames(SPIN, 90)).child(glyph))
      .when(Boolean(pic), (el) => el.child(pic))
      .child(div().truncate().font_semibold().text_color(t.foreground).child(k?.name || k?.template || k?.program?.name || terminal || ""))
      .when(Boolean(st), (el) => el.child(div().flex_none().text_color(t[st] ?? t.dim).child(WORDS[st] ?? st)))
      .when(Boolean(parent), (el) => el.child(div().truncate().text_color(t.dim).child(`from ${parent.name || parent.address}`)))
      .child(div().flex_1())
      .when(home || Boolean(space), (el) =>
        el.child(
          this.button(t, "keep", home ? "↗ go to" : "↗ keep", () => {
            if (space && !space.active) run("space.select", { id: space.id });
            run(home ? "pane.focus" : "terminal.keep", { terminal });
            closeOverlay(overlay);
          }),
        ),
      )
      .child(this.button(t, "close", "✕", () => closeOverlay(overlay)));
  }
}
