// A live, rendered view of one markdown file beside the terminals: an agent's
// plan, a README. Props: { file } (an absolute path on this computer).
// ponytail: local files only; pass props.on to readTextFile (as Files does) for other machines.
import { View, div } from "gpui-kit";
import { v_flex, h_flex, TextView } from "gpui-base";
import { props, readTextFile, watchFolder, theme } from "herdr";

export default class Markdown extends View {
  init(_props, cx) {
    this.file = props()?.file;
    this.text = null;
    this.error = this.file ? null : "No file";
    if (!this.file) return;
    const folder = this.file.replace(/[\\/][^\\/]*$/, "") || "/";
    cx.spawn(async cx => {
      for (;;) {
        try {
          this.text = await readTextFile(this.file);
          this.error = this.text === null ? "This file is no longer available" : null;
        } catch { this.text = null; this.error = "Cannot read this file (too large or unreadable)"; }
        cx.notify();
        // ponytail: any change in the folder re-reads the file; filter the batch if a busy folder costs too much.
        try { await watchFolder(folder); } catch { await cx.sleep(2000); }
      }
    });
  }

  render() {
    const t = theme();
    return v_flex().id("markdown").size_full().min_h(0).bg(t.background).text_color(t.foreground).text_size(13)
      .child(h_flex().h(32).flex_none().px(14).items_center().border_b(1).border_color(t.border)
        .child(div().flex_1().min_w(0).truncate().text_size(12).text_color(t.muted).child(this.file?.split(/[\\/]/).pop() ?? "")))
      .child(this.text !== null
        ? TextView.markdown("markdown-text", this.text).selectable().scrollable().flex_1().min_h(0).p(16)
        : div().p(16).text_color(t.dim).child(this.error || "Loading…"));
  }
}
