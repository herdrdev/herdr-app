import { View, div } from "gpui-kit";
import { v_flex } from "gpui-base";
import { NativeAppState, NativeApp } from "herdr-browser";
import { props, theme, focused, nextCommand, panes, shared, setShared } from "herdr";

// The person's Fork (git client) in this tab, on props.uri (a folder URI such
// as vscode-remote://wsl+Ubuntu/root/app) or props.folder (a local path).
// Each tab has a Fork process of its own with only this repository; closing
// the tab closes it. shared("fork.tabs") maps each open project URI to its tab.
export default class ForkApp extends View {
  init(_props, cx) {
    this.target = props()?.uri || props()?.folder || focused()?.cwd || "";
    this.app = NativeAppState("fork", this.target);
    this.register();
    cx.spawn(async (cx) => {
      // The new tab can join panes() only after this init.
      this.register();
      for (;;) {
        const command = await nextCommand();
        if (command === "focus") this.app.focus();
        cx.notify();
      }
    });
    // The pane's message follows the window's state while it is not shown.
    cx.spawn(async (cx) => {
      let last = "";
      for (;;) {
        await cx.sleep(500);
        const now = `${this.app.status()}|${this.app.error()}`;
        if (now !== last) { last = now; cx.notify(); }
      }
    });
  }

  // This tab is the one Fork tab no other open project has claimed.
  register() {
    if (!this.target) return;
    const tabs = shared("fork.tabs") || {};
    const open = new Set(panes().filter((p) => p.mod === "fork").map((p) => p.tab));
    if (open.has(tabs[this.target])) return;
    const claimed = new Set(Object.entries(tabs).filter(([uri]) => uri !== this.target).map(([, tab]) => tab));
    const mine = [...open].filter((tab) => !claimed.has(tab));
    if (mine.length === 1) setShared("fork.tabs", { ...tabs, [this.target]: mine[0] });
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
