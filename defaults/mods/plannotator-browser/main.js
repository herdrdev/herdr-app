import { View, div } from "gpui-kit";
import { h_flex, v_flex } from "gpui-base";
import { BrowserState, Browser } from "herdr-browser";
import { props, theme } from "herdr";

export default class PlannotatorBrowser extends View {
  init() {
    const url = props()?.url;
    this.browser = url ? BrowserState(url) : null;
  }

  button(t, id, label, action) {
    return div().id(id).px(8).py(4).rounded(4).flex_none()
      .text_color(t.muted).cursor_pointer()
      .hover(s => s.bg(t.hover).text_color(t.foreground))
      .on_click(action).child(label);
  }

  render() {
    const t = theme();
    if (!this.browser) return div().size_full().p(16).text_color(t.muted).child("Open a Plannotator review from an agent to show it here.");
    return v_flex().size_full().bg(t.surface).text_color(t.foreground).text_size(12)
      .child(h_flex().h(38).flex_none().px(8).gap(6).items_center().bg(t.chrome).border_b(1).border_color(t.border)
        .child(this.button(t, "back", "Back", () => this.browser.back()))
        .child(this.button(t, "reload", "Reload", () => this.browser.reload()))
        .child(this.button(t, "external", "Open externally", (_event, cx) => cx.open_url(this.browser.url())))
        .child(div().flex_1().min_w(0).truncate().text_color(t.muted).child(props()?.url || "Plannotator"))
        .child(div().flex_none().text_color(t.dim).child(props()?.origin || "")))
      .when(Boolean(this.browser.error()), el => el.child(div().px(12).py(6).text_color(t.blocked).child(this.browser.error())))
      .child(div().flex_1().min_h(0).child(Browser(this.browser).size_full()));
  }
}
