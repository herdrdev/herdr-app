import { View, div } from "gpui-kit";
import { v_flex } from "gpui-base";
import { NativeAppState, NativeApp } from "herdr-browser";
import { props, theme, focused, nextCommand } from "herdr";

// The Unity Editor in this tab, on the Unity project at props.folder (a local
// path) or props.uri (a file:// URI), in the editor version the project
// names. An editor that already has the project open moves into the tab.
// Closing the tab closes the editor, as its own close button does.
export default class UnityEditor extends View {
  init(_props, cx) {
    this.target = props()?.folder || props()?.uri || focused()?.cwd || "";
    this.app = NativeAppState("unity", this.target);
    cx.spawn(async (cx) => {
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

  render() {
    const t = theme();
    const error = this.app.error();
    // The window covers this pane once it is shown; until then, or after it
    // closes, the pane says why it is empty. Unity shows its own progress
    // while it opens the project.
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
