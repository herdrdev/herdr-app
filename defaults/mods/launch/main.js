// Launch: a webterminal-style launcher as a Herdr side panel (like Files), and Chat commands.
// One mod, three uses (props.mode):
//   "launch"   (default) programs with their flags. Entries of the same program fold under the
//              first one as variants ("+" starts it, the chevron shows the rest). "+" in the
//              header adds an entry.
//   "commands" chat commands: "/mcp", "/context"... Click one: it goes to the agent of the shown
//              tab (the focused one, else the one that changed last). "send" entries press Enter
//              (hc send); "type" entries only type, so you can finish the line.
//   "edit"     the dialog for either (add, change, remove).
// Right-click a row to edit it. Config: ~/.config/herdr-app/launch.json, this app's own
// (./launch.py reads, checks and writes it). Layouts put it in the "panel-body" stack as
// { id: "launch" } and { id: "commands", props: { mode: "commands" } }.

import { View, div, svg, image } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { exec, hc, agents, focused, panes, terminals, run, openOverlay, closeOverlay, props, appHome, theme, notify } from "herdr";

const HELPER = () => appHome() + "/mods/launch/launch.py";
const ICON = (n) => `mods/launch/icons/${n}.svg`;

async function helper(args) {
  const r = await exec("python3", [HELPER(), ...args]);
  const out = JSON.parse(r.stdout || "{}");
  if (out.error) throw new Error(out.error);
  return out;
}

function folder() {
  const f = focused();
  const p = f?.pane ? panes().find((x) => x.pane === f.pane) : null;
  return p?.cwd || f?.cwd || null;
}

function target() {
  const f = focused();
  const here = agents().filter((a) => a.tab === f?.tab);
  return here.find((a) => a.focused) ?? here.slice().sort((a, b) => b.seq - a.seq)[0] ?? null;
}

function cap(s) { return s ? s[0].toUpperCase() + s.slice(1) : s; }

function running() {
  const n = {};
  for (const t of terminals()) {
    if (t.state && t.state !== "live") continue;
    const cmd = String(t.argv?.[0] ?? "").split("/").pop();
    if (cmd) n[cmd] = (n[cmd] ?? 0) + 1;
  }
  return n;
}

export default class Launch extends View {
  init(_props, cx) {
    this.mode = props()?.mode ?? "launch";
    if (this.mode === "edit") return this.initEdit(cx);
    this.data = null; this.error = null; this.open = new Set();
    this.load(cx);
    cx.timer.every(4000, (cx) => this.load(cx));
  }

  load(cx) {
    cx.spawn(async (cx) => {
      try { this.data = await helper(["list"]); this.error = null; }
      catch (e) { this.error = String(e.message || e); }
      cx.notify();
    });
  }

  start(l) {
    const args = { folder: folder() ?? undefined, name: cap(l.label) };
    if (l.command !== "shell") {
      if (!l.argv) return notify({ title: `Can't start ${l.label}: check its flags`, level: "error" });
      args.argv = l.argv;
    }
    run("terminal.open", args);
  }

  send(cx, c) {
    const a = target();
    if (!a) return notify({ title: "No agent in this tab", body: "Start one from Launch first.", level: "warning" });
    if (c.submit) {
      cx.spawn(async () => {
        const r = await hc(["send", a.address, c.text]);
        if (r.code !== 0) notify({ title: `Couldn't send ${c.text}`, body: (r.stderr || r.stdout).trim().slice(0, 200), level: "error" });
      });
    } else {
      // Type without Enter: through the clipboard and the app's own paste, into the agent's pane.
      cx.write_to_clipboard(c.text);
      run("pane.focus", { pane: a.pane });
      run("terminal.paste");
    }
  }

  dialog(kind, item) {
    openOverlay({ mod: "launch", kind: "dialog", width: 440, height: kind === "chat" ? 330 : 380,
      props: { mode: "edit", kind, item: item ?? null } });
  }

  iconBtn(t, id, glyph, tip, onClick) {
    return div().id(id).size(26).flex_none().flex().items_center().justify_center().rounded(6).cursor_pointer()
      .text_size(15).text_color(t.muted).hover((s) => s.bg(t.hover).text_color(t.foreground))
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
      .on_click((_e, cx) => { cx.stop_propagation(); onClick(cx); })
      .tooltip(tip)
      .child(glyph);
  }

  icon(t, l, size) {
    const i = l.icon_info;
    return i.tint ? svg(i.file).size(size).flex_none().text_color(t.foreground) : image(i.file).w(size).h(size).flex_none();
  }

  row(t, id, { icon, title, sub, count, extra = [], onClick, onEdit, tip, depth = 0 }) {
    return h_flex().id(id).w_full().flex_none().h(24).pl(6 + depth * 18).pr(4).gap(8).items_center()
      .rounded(4).cursor_pointer().hover((s) => s.bg(t.hover))
      .on_click((_e, cx) => onClick(cx))
      .on_mouse_down("right", (_e, cx) => { cx.stop_propagation(); onEdit(); })
      .tooltip(tip)
      .when(Boolean(icon), (el) => el.child(icon))
      .child(div().flex_none().max_w(140).truncate().text_size(13).text_color(t.foreground).child(title))
      .child(div().flex_1().min_w(0).truncate().text_size(11.5).text_color(t.dim).child(sub))
      .when(count > 0, (el) => el.child(div().flex_none().text_size(11.5).text_color(t.dim).child(String(count))))
      .children(extra);
  }

  render(cx) {
    const t = theme();
    if (this.mode === "edit") return this.renderEdit(t);
    const d = this.data, n = running();
    const chat = this.mode === "commands";
    const sep = () => div().flex_none().h(4);
    const a = chat ? target() : null;

    const rows = [];
    if (d && !chat) {
      d.groups.forEach((g, gi) => {
        const main = g[0], key = main.command, open = this.open.has(key);
        if (gi) rows.push(sep());
        rows.push(this.row(t, `ln-${main.id}`, {
          icon: this.icon(t, main, 15), title: cap(main.label), sub: main.line, count: n[main.command] ?? 0,
          tip: main.line, onClick: () => this.start(main), onEdit: () => this.dialog("launcher", main),
          extra: g.length > 1 ? [
            this.iconBtn(t, `ln-start-${key}`, "+", `Start ${main.label}`, () => this.start(main)),
            this.iconBtn(t, `ln-fold-${key}`, open ? "⌃" : "⌄", open ? "Hide variants" : `${g.length - 1} more`,
              (cx) => { open ? this.open.delete(key) : this.open.add(key); cx.notify(); }),
          ] : [],
        }));
        if (open) rows.push(v_flex().flex_none().children(g.slice(1).map((v) =>
          this.row(t, `ln-${v.id}`, { title: cap(v.label), sub: v.line, tip: v.line, depth: 1,
            onClick: () => this.start(v), onEdit: () => this.dialog("launcher", v) }))));
      });
    }
    if (d && chat) {
      d.chat.forEach((c, i) => {
        if (i) rows.push(sep());
        rows.push(this.row(t, `ch-${c.id}`, {
          icon: div().w(15).flex_none().text_size(13).font_semibold().text_color(t.accent).child(c.text.trim().startsWith("/") ? "/" : "›"),
          title: c.label, sub: c.text.trim() + (c.submit ? "" : "  (types, no Enter)"),
          tip: a ? `Send to ${a.program?.name || a.title}` : "No agent in this tab",
          onClick: (cx) => this.send(cx, c), onEdit: () => this.dialog("chat", c),
        }));
      });
      if (!d.chat.length) rows.push(div().p(12).text_size(12).text_color(t.dim).child("No commands yet. + adds one, e.g. /mcp"));
    }

    const where = chat ? (a ? `→ ${a.program?.name || a.title}` : "no agent here") : (folder() ? `in ${folder().split("/").pop() || "/"}` : "");
    return v_flex().id(`launch-${this.mode}`).size_full().min_h(0).overflow_hidden().bg(t.background)
      .text_color(t.foreground).text_size(13)
      .child(h_flex().h(42).flex_none().px(14).gap(8).items_center().border_b(1).border_color(t.border)
        .child(div().font_semibold().child(chat ? "Chat commands" : "Launch"))
        .child(div().flex_1().min_w(0).truncate().text_size(11.5).text_color(chat && !a ? t.blocked : t.dim).child(where))
        .child(this.iconBtn(t, "ln-add", "+", chat ? "Add a chat command" : "Add a launcher", () => this.dialog(chat ? "chat" : "launcher"))))
      .child(v_flex().id(`launch-body-${this.mode}`).flex_1().min_h(0).p(6).gap(2).overflow_y_scroll()
        .when(Boolean(this.error), (el) => el.child(div().flex_none().m(6).p(8).rounded(6).bg(t.blocked + "22").text_size(12).text_color(t.blocked).child(this.error)))
        .when(!d && !this.error, (el) => el.child(div().p(12).text_size(12).text_color(t.dim).child("Loading…")))
        .children(rows))
      .child(div().flex_none().px(14).py(8).border_t(1).border_color(t.border).text_size(10.5).text_color(t.dim)
        .child(chat ? "Click to send to the tab's agent · right-click to edit" : "Click to start · right-click to edit · launch.json"));
  }

  // ---- edit dialog: props { kind: "launcher" | "chat", item | null } ----
  initEdit(_cx) {
    const p = props() ?? {};
    this.kind = p.kind; this.item = p.item; this.saveError = null;
    const i = this.item ?? {};
    this.submit = this.kind === "chat" ? (i.submit ?? true) : true;
    this.fields = this.kind === "chat" ? {
      label: InputState.new({ value: i.label ?? "", placeholder: "MCP servers" }),
      text: InputState.new({ value: i.text ?? "", placeholder: "/mcp" }),
    } : {
      label: InputState.new({ value: i.label ?? "", placeholder: "Claude (plan mode)" }),
      command: InputState.new({ value: i.command ?? "", placeholder: "claude, lazygit, /usr/bin/htop" }),
      args: InputState.new({ value: i.args ?? "", placeholder: "--permission-mode plan" }),
      icon: InputState.new({ value: i.icon ?? "", placeholder: "wt-claude, or blank to match the program" }),
    };
    (this.kind === "chat" ? this.fields.text : this.item ? this.fields.args : this.fields.label).focus();
    for (const f of Object.values(this.fields)) f.on("submit", (_e, cx) => this.save(cx));
  }

  save(cx) {
    const v = (k) => this.fields[k].value();
    const id = this.item?.id ?? "-";
    const args = this.kind === "chat" ? ["save-chat", id, v("label"), v("text"), this.submit ? "1" : "0"]
      : ["save-launcher", id, v("label"), v("command"), v("args"), v("icon")];
    this.run(cx, args);
  }

  run(cx, args) {
    cx.spawn(async (cx) => {
      try { const r = await helper(args); notify({ title: r.message, level: "success" }); closeOverlay(props().overlay); }
      catch (e) { this.saveError = String(e.message || e); cx.notify(); }
    });
  }

  field(t, label, state) {
    return v_flex().gap(4)
      .child(div().text_size(11).text_color(t.dim).child(label))
      .child(h_flex().h(32).px(8).rounded(6).border(1).border_color(t.border).bg(t.input).items_center()
        .child(Input.new(state).flex_1().h_full().text_size(13).text_color(t.foreground)));
  }

  renderEdit(t) {
    const btn = (id, label, kind, onClick) => div().id(id).px(12).h(28).flex().items_center().rounded(6).text_size(12.5).cursor_pointer()
      .bg(kind === "primary" ? t.accent : kind === "danger" ? t.blocked + "22" : t.raised)
      .text_color(kind === "primary" ? t.background : kind === "danger" ? t.blocked : t.foreground)
      .hover((s) => s.opacity(0.85)).on_click((_e, cx) => onClick(cx)).child(label);
    const chat = this.kind === "chat";
    const title = `${this.item ? "Edit" : "Add"} ${chat ? "chat command" : "launcher"}`;
    return v_flex().id("launch-edit").size_full().p(16).gap(10).bg(t.popover).text_color(t.foreground)
      .child(div().text_size(14).font_semibold().child(title))
      .when(chat, (el) => el
        .child(this.field(t, "Text to send", this.fields.text))
        .child(this.field(t, "Label", this.fields.label))
        .child(h_flex().id("le-submit").gap(8).items_center().text_size(12.5).cursor_pointer()
          .on_click((_e, cx) => { this.submit = !this.submit; cx.notify(); })
          .child(div().size(14).flex_none().rounded(3).border(1).border_color(t.border).bg(this.submit ? t.accent : t.background))
          .child("Press Enter after it (off: only type it)")))
      .when(!chat, (el) => el
        .child(this.field(t, "Label", this.fields.label))
        .child(this.field(t, "Program (same program = shown as its variant)", this.fields.command))
        .child(this.field(t, "Flags", this.fields.args))
        .child(this.field(t, "Icon", this.fields.icon)))
      .when(Boolean(this.saveError), (el) => el.child(div().text_size(12).text_color(t.blocked).child(this.saveError)))
      .child(div().flex_1())
      .child(h_flex().gap(8)
        .when(Boolean(this.item), (el) => el.child(btn("le-remove", "Remove", "danger",
          (cx) => this.run(cx, [chat ? "remove-chat" : "remove-launcher", this.item.id]))))
        .child(div().flex_1())
        .child(btn("le-cancel", "Cancel", "", () => closeOverlay(props().overlay)))
        .child(btn("le-save", this.item ? "Save" : "Add", "primary", (cx) => this.save(cx))));
  }
}
