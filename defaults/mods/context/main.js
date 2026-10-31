// Context: Claude Code's /context breakdown for the focused pane's folder, live.
// A side panel like Files: layouts put it in the "panel-body" stack as { id: "context" }.
// Data from ./context.py: overhead per category from `claude -p /context` (cached 10 min),
// the live total from the newest session transcript there. Refreshes every 5 s.
// Click the header to fold it, a list row (MCP tools, Memory files, ...) to open it,
// the ↻ to re-measure the overhead now.

import { View, div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { exec, focused, panes, appHome, theme, props, closeOverlay } from "herdr";

const COLORS = { "Messages": "#4c8df6", "MCP tools": "#e8743b", "System tools": "#19a99a", "Skills": "#d9a21b" };
const DIM = ["System prompt", "MCP server instructions", "Memory files", "Custom agents"];
const LISTS = [["MCP Tools", "MCP tools"], ["Memory Files", "Memory files"], ["Custom Agents", "Custom agents"], ["Skills", "Skills"]];

function k(n) {
  if (n >= 1e6) return `${+(n / 1e6).toFixed(1)}M`;
  if (n >= 1e3) return `${+(n / 1e3).toFixed(1)}k`;
  return String(n);
}

function folder() {
  const f = focused();
  const p = f?.pane ? panes().find((x) => x.pane === f.pane) : null;
  return p?.cwd || f?.cwd || null;
}

export default class Context extends View {
  init(_props, cx) {
    // Context used to be a floating window (kept across starts); as an overlay now, close it.
    if (props()?.overlay) { closeOverlay(props().overlay); return; }
    this.data = null; this.error = null; this.folded = false; this.open = new Set(); this.cwd = null;
    this.load(cx);
    cx.timer.every(5000, (cx) => this.load(cx));
  }

  load(cx, refresh = false) {
    const cwd = folder();
    if (!cwd || this.busy) return;
    if (cwd !== this.cwd) { this.cwd = cwd; this.data = null; }
    this.busy = true;
    cx.spawn(async (cx) => {
      try {
        const r = await exec("python3", [appHome() + "/mods/context/context.py", cwd, ...(refresh ? ["--refresh"] : [])]);
        const d = JSON.parse(r.stdout || "{}");
        if (d.error) throw new Error(d.error);
        if (cwd === this.cwd) { this.data = d; this.error = null; }
      } catch (e) { this.error = String(e.message || e); }
      this.busy = false; cx.notify();
    });
  }

  color(t, name) {
    if (COLORS[name]) return COLORS[name];
    if (name === "Free space") return t.raised;
    if (name === "Autocompact buffer") return t.muted;
    return t.dim;
  }

  row(t, name, tokens, pct, color) {
    return h_flex().h(24).gap(10).items_center().text_size(13)
      .child(div().size(10).flex_none().rounded(2).bg(color))
      .child(div().flex_1().min_w(0).truncate().text_color(t.muted).child(name))
      .child(div().w(56).flex_none().text_right().text_color(t.foreground).child(k(tokens)))
      .child(div().w(52).flex_none().text_right().text_color(t.foreground).child(pct));
  }

  render(cx) {
    const t = theme();
    const d = this.data;
    const head = h_flex().id("ctx-head").h(42).flex_none().px(14).gap(8).items_center().border_b(1).border_color(t.border)
      .child(div().font_semibold().child("Context"))
      .child(div().id("ctx-fold").text_size(10).text_color(t.dim).cursor_pointer()
        .on_click((_e, cx) => { this.folded = !this.folded; cx.notify(); }).child(this.folded ? "▸" : "▾"))
      .child(div().flex_1())
      .child(div().id("ctx-refresh").px(4).rounded(4).text_size(12).text_color(t.dim).hover((s) => s.bg(t.hover))
        .on_click((_e, cx) => { cx.stop_propagation(); this.load(cx, true); }).child(this.busy ? "…" : "↻"));

    const body = v_flex().id("ctx-body").flex_1().min_h(0).p(14).gap(10).overflow_y_scroll();
    const frame = (inner) => v_flex().id("context").size_full().min_h(0).rounded(10).overflow_hidden().bg(t.background)
      .border_1().border_color(t.border).text_color(t.foreground).text_size(13).child(head).when(Boolean(inner), (el) => el.child(inner));

    if (this.folded) return frame(null);
    if (!d) return frame(body.child(div().text_size(12).text_color(this.error ? t.blocked : t.dim)
      .child(this.error || (this.cwd ? "Measuring… (first look at a folder takes a few seconds)" : "Focus a pane to see its folder's context"))));

    const pct = (n) => `${((n / d.window) * 100).toFixed(1)}%`;
    const bar = h_flex().w_full().h(6).flex_none().rounded(3).overflow_hidden().bg(t.raised)
      .children(d.rows.filter((r) => r.name !== "Free space" && r.tokens > 0).map((r) =>
        div().h_full().w(`${Math.max(0.4, (r.tokens / d.window) * 100).toFixed(2)}%`).bg(this.color(t, r.name))));

    const lists = LISTS.filter(([key]) => d.lists[key]).map(([key, label]) => {
      const open = this.open.has(key), items = d.lists[key];
      return v_flex()
        .child(h_flex().id(`ctx-l-${key}`).h(24).gap(8).items_center().cursor_pointer().text_size(13)
          .on_click((_e, cx) => { open ? this.open.delete(key) : this.open.add(key); cx.notify(); })
          .child(div().w(10).text_size(10).text_color(t.dim).child(open ? "▾" : "›"))
          .child(div().flex_1().text_color(t.muted).child(label))
          .child(div().text_color(t.dim).child(String(items.length))))
        .when(open, (el) => el.children(items.map((it, i) => h_flex().id(`ctx-i-${key}-${i}`).h(20).pl(18).gap(8).items_center().text_size(11.5)
          .child(div().flex_1().min_w(0).truncate().text_color(t.dim).child(it.name))
          .child(div().flex_none().text_color(t.dim).child(k(it.tokens))))));
    });

    return frame(body
      .child(h_flex().flex_none().items_end()
        .child(div().text_size(16).font_semibold().child(`${Math.round((d.used / d.window) * 100)}% used`))
        .child(div().flex_1())
        .child(div().text_size(13).text_color(t.muted).child(`${k(d.used)} / ${k(d.window)}`)))
      .child(bar)
      .child(v_flex().flex_none().children(d.rows.map((r) => this.row(t, r.name, r.tokens, pct(r.tokens), this.color(t, r.name)))))
      .when(d.deferred.length > 0, (el) => el.child(v_flex().flex_none().pt(4).children(d.deferred.map((r) => this.row(t, r.name, r.tokens, "—", t.dim)))))
      .child(v_flex().flex_none().pt(4).children(lists))
      .child(div().text_size(10.5).text_color(t.dim).child(
        (d.live ? `Live: ${d.model} session ${String(d.session).slice(0, 8)}` : "No session yet: overhead only") +
        " · categories are Claude Code's estimate")));
  }
}
