import { View, div } from "gpui-kit";
import { h_flex, v_flex, InputState, Input } from "gpui-base";
import { BrowserState, Browser } from "herdr-browser";
import { props, theme } from "herdr";

// Shared by Linear and other website tabs. The existing mod name is retained
// so saved Linear tabs continue to open.
export default class WebsiteBrowser extends View {
  init() {
    const url = props()?.url;
    this.browser = url ? BrowserState(url) : null;
    this.input = InputState.new({ placeholder: "https://example.com" });
    this.input.on("submit", (_event, cx) => this.open(cx));
    if (!this.browser) this.input.focus();
  }

  open(cx) {
    try {
      const url = this.input.value().trim();
      if (!url) return;
      this.browser = BrowserState(url);
      this.inputError = false;
    } catch {
      this.inputError = true;
    }
    cx.notify();
  }

  button(t, id, label, action) {
    return div().id(id).px(8).py(4).rounded(4).flex_none()
      .text_color(t.muted).cursor_pointer()
      .hover(s => s.bg(t.hover).text_color(t.foreground))
      .on_click(action).child(label);
  }

  render() {
    const t = theme();
    if (!this.browser) return v_flex().size_full().p(16).gap(10).bg(t.surface).text_color(t.foreground)
      .child(div().child("Enter a website URL"))
      .child(h_flex().gap(8).items_center()
        .child(Input.new(this.input).h(36).flex_1().px(10).rounded(9).border_1().border_color(t.input).bg(t.background).text_color(t.foreground))
        .child(this.button(t, "open", "Open", (_event, cx) => this.open(cx))))
      .when(this.inputError, el => el.child(div().text_color(t.blocked).child("Enter a valid HTTP or HTTPS URL without credentials.")));
    return v_flex().size_full().bg(t.surface).text_color(t.foreground).text_size(12)
      .child(h_flex().h(38).flex_none().px(8).gap(6).items_center().bg(t.chrome).border_b(1).border_color(t.border)
        .child(this.button(t, "back", "Back", () => this.browser.back()))
        .child(this.button(t, "reload", "Reload", () => this.browser.reload()))
        .child(this.button(t, "external", "Open externally", (_event, cx) => cx.open_url(this.browser.url())))
        .child(div().flex_1().min_w(0)))
      .when(props()?.linked === false, el => el.child(div().px(12).py(6).text_color(t.muted).child("No Linear project found for this folder. Opening Linear instead.")))
      .when(Boolean(this.browser.error()), el => el.child(div().px(12).py(6).text_color(t.blocked).child(this.browser.error())))
      .child(div().flex_1().min_h(0).child(Browser(this.browser).size_full()));
  }
}
