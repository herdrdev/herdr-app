import { View, div } from "gpui-kit";
import { v_flex } from "gpui-base";
import { NativeAppState, NativeApp } from "herdr-browser";
import { props, theme, focused, nextCommand, shared, setShared } from "herdr";

// The person's own VS Code in this tab, on props.uri (a folder URI such as
// vscode-remote://wsl+Ubuntu/root/app) or props.folder (a local path).
// Closing the tab closes that VS Code window. shared("vscode.tabs") maps each
// open project URI to its tab, so "Open in VS Code" can select it again.
export default class VsCode extends View {
  init(_props, cx) {
    this.target = props()?.uri || props()?.folder || focused()?.cwd || "";
    this.app = NativeAppState("vscode", this.target);
    // A tab starts its mod when it is first shown, so the focused tab is this one.
    const tab = focused()?.tab;
    if (tab && this.target) setShared("vscode.tabs", { ...(shared("vscode.tabs") || {}), [this.target]: tab });
    cx.spawn(async (cx) => {
      for (;;) {
        const command = await nextCommand();
        if (command === "focus") this.app.focus();
        cx.notify();
      }
    });
  }

  render() {
    const t = theme();
    const error = this.app.error();
    // The window covers this pane once it is shown; until then, or after it
    // closes, the pane says why it is empty.
    const message = v_flex().absolute().size_full().items_center().justify_center().gap(10)
      .text_size(13).text_color(error ? t.blocked : t.muted)
      .child(div().child(error || this.app.status()))
      .when(Boolean(error) || this.app.status() === "Closed.", el => el.child(
        div().id("reopen").px(10).py(5).rounded(5).bg(t.hover).text_color(t.foreground).cursor_pointer()
          .on_click((_event, cx) => { this.app.reopen(); cx.notify(); })
          .child("Open again")));
    return div().relative().size_full().bg(t.surface)
      .child(message)
      .child(NativeApp(this.app).size_full());
  }
}
