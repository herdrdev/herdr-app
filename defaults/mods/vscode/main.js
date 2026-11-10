import { View, div } from "gpui-kit";
import { h_flex, v_flex } from "gpui-base";
import { NativeAppState, NativeApp } from "herdr-browser";
import { props, theme, focused, nextCommand } from "herdr";

// Proof of concept: VS Code's own window inside a Herdr tab. The editor runs
// as this tab's subprocess and ends when the tab closes.
export default class VsCode extends View {
  init(_props, cx) {
    this.folder = props()?.folder || focused()?.cwd || "";
    this.app = NativeAppState("vscode", this.folder);
    cx.spawn(async (cx) => {
      for (;;) {
        const command = await nextCommand();
        if (command === "focus") this.app.focus();
        cx.notify();
      }
    });
  }

  button(t, id, label, action) {
    return div().id(id).px(8).py(4).rounded(4).flex_none()
      .text_color(t.muted).cursor_pointer()
      .hover(s => s.bg(t.hover).text_color(t.foreground))
      .on_click(action).child(label);
  }

  render() {
    const t = theme();
    return v_flex().size_full().bg(t.surface).text_color(t.foreground).text_size(12)
      .child(h_flex().h(30).flex_none().px(8).gap(6).items_center().bg(t.chrome).border_b(1).border_color(t.border)
        .child(div().flex_none().text_color(t.foreground).child("VS Code"))
        .child(div().flex_1().min_w(0).truncate().text_color(t.muted).child(this.folder || "~"))
        .child(this.button(t, "restart", "Restart", (_event, cx) => { this.app.restart(); cx.notify(); })))
      .when(Boolean(this.app.error()), el => el.child(div().px(12).py(6).text_color(t.blocked).child(this.app.error())))
      .child(div().flex_1().min_h(0).child(NativeApp(this.app).size_full()));
  }
}
