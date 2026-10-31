// Skills & MCP: webterminal-adw's "Add Skills / Add MCP Servers" as a side panel like Files.
// Layouts put it in the "panel-body" stack as { id: "skills" }. Project = the focused pane's
// folder (follows it like Files). Lists what the project already has (removable) and what
// cc-switch offers (tick, then Add). Each card's copy icon copies its folder or config file
// (cc-switch MCP servers live in its database, so theirs copies the server's JSON).
// Logic lives in webterminal-adw's agent_setup.py, reached through ./cli.py.

import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { exec, focused, panes, appHome, home, theme, notify, props, closeOverlay } from "herdr";

const CLI = () => appHome() + "/mods/agent-setup/cli.py";
const ICON = (n) => `mods/agent-setup/${n}.svg`;

function short(p) {
  const h = home();
  return p === h ? "~" : p.startsWith(h + "/") ? "~" + p.slice(h.length) : p;
}

function folder() {
  const f = focused();
  const p = f?.pane ? panes().find((x) => x.pane === f.pane) : null;
  return p?.cwd || f?.cwd || null;
}

export default class AgentSetup extends View {
  init(_props, cx) {
    // It used to open as a dialog; as an overlay now (an old key or button), close it.
    if (props()?.overlay) { closeOverlay(props().overlay); return; }
    this.project = null;
    this.kind = "skills";
    this.data = null;
    this.error = null;
    this.busy = false;
    this.always = true;
    this.selected = new Set();
    this.filter = "";
    this.copied = null;
    this.input = InputState.new({ placeholder: "Filter" });
    this.input.on("change", (_e, cx) => { this.filter = this.input.value().toLowerCase(); cx.notify(); });
    this.follow(cx);
    cx.timer.every(1500, (cx) => this.follow(cx));
  }

  // Like Files: when the focused folder changes, show that project.
  follow(cx) {
    const cwd = folder();
    if (!cwd || cwd === this.project) return;
    this.project = cwd; this.data = null; this.selected.clear(); this.error = null;
    this.load(cx);
  }

  async call(cx, args) {
    this.busy = true; cx.notify();
    try {
      const r = await exec("python3", [CLI(), ...args]);
      const out = JSON.parse(r.stdout || "{}");
      if (out.error) throw new Error(out.error);
      return out;
    } catch (e) {
      this.error = String(e.message || e);
      return null;
    } finally {
      this.busy = false; cx.notify();
    }
  }

  load(cx) {
    const project = this.project;
    cx.spawn(async (cx) => {
      const d = await this.call(cx, ["list", project]);
      if (d && project === this.project) { this.data = d; this.error = d.warning; }
      cx.notify();
    });
  }

  act(cx, args) {
    cx.spawn(async (cx) => {
      const r = await this.call(cx, args);
      if (r) { notify({ title: r.message, level: "success" }); this.selected.clear(); }
      this.load(cx);
    });
  }

  copy(cx, id, text, kind) {
    cx.write_to_clipboard(text);
    this.copied = id; cx.notify();
    notify({ title: kind === "json" ? "Copied the server's JSON" : `Copied ${short(text)}`, level: "info" });
    cx.timer.after(1200, (cx) => { if (this.copied === id) { this.copied = null; cx.notify(); } });
  }

  tabBtn(t, id, label) {
    const on = this.kind === id;
    return div().id(`as-tab-${id}`).flex_1().h(26).flex().items_center().justify_center().rounded(6).text_size(12)
      .cursor_pointer().bg(on ? t.selected : t.chrome).text_color(on ? t.foreground : t.muted)
      .hover((s) => s.bg(t.hover))
      .on_click((_e, cx) => { this.kind = id; this.selected.clear(); cx.notify(); })
      .child(label);
  }

  button(t, id, label, onClick, primary = false, enabled = true) {
    return div().id(id).px(10).h(26).flex().items_center().rounded(6).text_size(12).flex_none()
      .bg(primary && enabled ? t.accent : t.raised).text_color(primary && enabled ? t.background : enabled ? t.foreground : t.dim)
      .when(enabled, (el) => el.cursor_pointer().hover((s) => s.opacity(0.85))
        .on_click((_e, cx) => { cx.stop_propagation(); onClick(cx); }))
      .child(label);
  }

  copyBtn(t, id, text, kind) {
    if (!text) return div();
    const done = this.copied === id;
    return div().id(`as-copy-${id}`).p(4).rounded(5).flex_none().cursor_pointer()
      .text_color(done ? t.done : t.dim).hover((s) => s.bg(t.hover).text_color(t.foreground))
      .on_click((_e, cx) => { cx.stop_propagation(); this.copy(cx, id, text, kind); })
      .child(done ? div().text_size(12).child("✓") : svg(ICON("copy")).size(14));
  }

  card(t, id, title, sub, right) {
    return h_flex().id(id).w_full().flex_none().gap(6).pl(10).pr(6).py(6).items_center()
      .border_b(1).border_color(t.border)
      .child(v_flex().flex_1().min_w(0)
        .child(div().truncate().text_size(12.5).text_color(t.foreground).child(title))
        .when(Boolean(sub), (el) => el.child(div().truncate().text_size(11).text_color(t.dim).child(sub))))
      .children(right);
  }

  section(t, title) {
    return div().flex_none().pt(10).pb(4).text_size(10.5).font_semibold().text_color(t.dim).child(title);
  }

  box(t) {
    return v_flex().w_full().flex_none().rounded(8).border(1).border_color(t.border).bg(t.surface).overflow_hidden();
  }

  render(cx) {
    const t = theme();
    if (props()?.overlay) return div();
    const d = this.data;
    const mcp = this.kind === "mcp";
    const installed = d ? (mcp ? d.installed.mcp : d.installed.skills) : [];
    const have = new Set(installed.map((x) => x.id));
    const avail = d ? (mcp ? d.mcp : d.skills).filter((x) =>
      !this.filter || [x.id, x.name, x.description, x.summary].join(" ").toLowerCase().includes(this.filter)) : [];

    const inst = this.box(t).children(installed.length ? installed.map((x) => this.card(t, `as-i-${x.id}`,
      x.name || x.id, mcp ? `${x.summary} · ${x.sources.join(", ")}` : x.description, [
        this.copyBtn(t, `i-${this.kind}-${x.id}`, x.copy),
        this.button(t, `as-rm-${x.id}`, "Remove", (cx) => this.act(cx, mcp ? ["rm-mcp", this.project, x.id] : ["rm-skill", this.project, x.path])),
      ]))
      : [div().w_full().min_h(32).p(10).text_size(12).text_color(t.dim).child(d ? `No ${mcp ? "MCP servers" : "skills"} in this project` : "Loading…")]);

    const list = this.box(t).children(avail.map((x) => {
      const on = this.selected.has(x.id);
      return this.card(t, `as-a-${x.id}`, x.name || x.id, (mcp ? x.summary : x.description) + (have.has(x.id) ? "  (installed)" : ""), [
        this.copyBtn(t, `a-${this.kind}-${x.id}`, x.copy, x.copy_kind),
        div().size(16).flex_none().rounded(4).border(1).border_color(on ? t.accent : t.border).bg(on ? t.accent : t.background)
          .flex().items_center().justify_center().text_size(11).text_color(t.background).child(on ? "✓" : ""),
      ]).cursor_pointer().hover((s) => s.bg(t.hover))
        .on_click((_e, cx) => { on ? this.selected.delete(x.id) : this.selected.add(x.id); cx.notify(); });
    }));

    const add = (cx) => this.act(cx, mcp
      ? ["add-mcp", this.project, this.always ? "1" : "0", ...this.selected]
      : ["add-skills", this.project, ...this.selected]);

    return v_flex().id("agent-setup").size_full().min_h(0).rounded(10).overflow_hidden().bg(t.background)
      .border_1().border_color(t.border).text_color(t.foreground).text_size(13)
      .child(h_flex().h(42).flex_none().px(14).gap(8).items_center().border_b(1).border_color(t.border)
        .child(div().font_semibold().child("Skills & MCP"))
        .child(div().flex_1().min_w(0).truncate().text_right().text_size(12).text_color(t.dim).child(this.project ? short(this.project) : "")))
      .child(h_flex().flex_none().px(10).pt(10).gap(4)
        .child(this.tabBtn(t, "skills", "Skills"))
        .child(this.tabBtn(t, "mcp", "MCP servers")))
      .child(v_flex().id("as-body").flex_1().min_h(0).px(10).pb(10).overflow_y_scroll()
        .when(!this.project, (el) => el.child(div().pt(10).text_size(12).text_color(t.dim).child("Focus a pane to pick its project")))
        .when(Boolean(this.error), (el) => el.child(div().flex_none().mt(10).p(8).rounded(6).bg(t.blocked + "22").text_size(12).text_color(t.blocked).child(this.error)))
        .child(this.section(t, "IN THIS PROJECT"))
        .child(inst)
        .child(this.section(t, "FROM CC-SWITCH"))
        .when(mcp, (el) => el.child(h_flex().id("as-always").flex_none().mb(6).gap(8).items_center().text_size(12).cursor_pointer()
          .on_click((_e, cx) => { this.always = !this.always; cx.notify(); })
          .child(div().size(14).flex_none().rounded(3).border(1).border_color(t.border).bg(this.always ? t.accent : t.background))
          .child(div().flex_1().min_w(0).truncate().child("Always load tools (Claude; off defers via ToolSearch)"))))
        .child(h_flex().flex_none().mb(6).h(30).px(8).rounded(6).border(1).border_color(t.border).bg(t.input).items_center()
          .child(Input.new(this.input).flex_1().h_full().text_size(12.5).text_color(t.foreground)))
        .child(list))
      .child(h_flex().flex_none().h(44).px(10).gap(8).items_center().border_t(1).border_color(t.border)
        .child(div().flex_1().text_size(11.5).text_color(t.dim).child(this.selected.size ? `${this.selected.size} selected` : "Tick items to add them"))
        .when(this.selected.size > 0, (el) => el.child(this.button(t, "as-clear", "Clear", (cx) => { this.selected.clear(); cx.notify(); })))
        .child(this.button(t, "as-add", this.busy ? "Working…" : "Add", add, true, !this.busy && this.selected.size > 0 && Boolean(this.project))));
  }
}
