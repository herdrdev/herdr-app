// Right-click menu for a file or folder (opened by the Files panel with
// props { path: absolute path, dir: bool }). "Add to chat" types `@path ` into the
// agent of the shown tab (the focused one, else the one that changed last) without
// pressing Enter: the text goes through the clipboard and the app's own terminal.paste,
// so it lands as a paste in Claude Code, Codex, OpenCode, Pi and other @-mention chats.
// Paths inside the agent's folder are relative; folders end with "/"; spaces get quotes.

import { View, div } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { agents, panes, focused, props, run, closeOverlay, notify, theme } from "herdr";

function target() {
  const f = focused();
  const here = agents().filter((a) => a.tab === f?.tab);
  const pool = here.length ? here : agents();
  return pool.find((a) => a.focused) ?? pool.slice().sort((a, b) => b.seq - a.seq)[0] ?? null;
}

function relative(path, base) {
  if (!base) return path;
  const b = base.endsWith("/") ? base : base + "/";
  return path === base ? "." : path.startsWith(b) ? path.slice(b.length) : path;
}

function mention(path, dir) {
  const p = dir && !path.endsWith("/") ? path + "/" : path;
  return /\s/.test(p) ? `@"${p}" ` : `@${p} `;
}

export default class FileMenu extends View {
  close() { closeOverlay(props().overlay); }

  item(t, id, label, hint, action) {
    return h_flex().id(id).h(30).px(10).gap(8).items_center().rounded(6).text_size(13).cursor_pointer()
      .text_color(t.foreground).hover((s) => s.bg(t.hover))
      .on_click((_e, cx) => { action(cx); this.close(); })
      .child(div().flex_1().child(label))
      .when(Boolean(hint), (el) => el.child(div().max_w(110).truncate().text_size(11).text_color(t.dim).child(hint)));
  }

  render(cx) {
    const t = theme();
    const { path, dir } = props() ?? {};
    const a = target();
    const cwd = a ? panes().find((p) => p.pane === a.pane)?.cwd : null;
    const name = a ? (a.program?.name || a.app || a.title) : null;
    const pane = panes().find((p) => p.pane === focused()?.pane);
    return v_flex().id("file-menu").size_full().p(5).gap(1).bg(t.popover)
      .child(this.item(t, "fm-chat", "Add to chat", name ?? "no agent", (cx) => {
        if (!a) return notify({ title: "No agent in this tab to add it to", level: "warning" });
        cx.write_to_clipboard(mention(relative(path, cwd), dir));
        run("pane.focus", { pane: a.pane });
        run("terminal.paste");
      }))
      .child(this.item(t, "fm-copy", "Copy path", null, (cx) => cx.write_to_clipboard(path)))
      .child(this.item(t, "fm-rel", "Copy relative path", null, (cx) => cx.write_to_clipboard(relative(path, pane?.cwd ?? cwd))));
  }
}
