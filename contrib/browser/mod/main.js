import { View, div } from 'gpui-kit';
import { v_flex, h_flex, InputState, Input } from 'gpui-base';
import { spaces, theme, exec, closeOverlay, props } from 'herdr';
import { command } from './config.js';

export default class BrowserLauncher extends View {
  init(_props, cx) {
    this.url = InputState.new({ value: 'about:blank', placeholder: 'URL or localhost:3000' });
    this.url.on('submit', (_event, cx) => this.open(cx));
    this.url.focus();
    this.error = '';
    this.busy = false;
  }

  open(cx) {
    if (this.busy) return;
    const space = spaces().find(space => space.active);
    if (!space) { this.error = 'Open a space before starting the browser.'; cx.notify(); return; }
    if (space.machine) { this.error = 'Choose a local space. For remote networking, use the companion’s SSH option.'; cx.notify(); return; }
    this.busy = true;
    this.error = '';
    const url = this.url.value().trim() || 'about:blank';
    cx.notify();
    cx.spawn(async cx => {
      try {
        const response = await exec(command[0], [...command.slice(1), 'open', '--space', space.id, '--url', url, '--focus']);
        if (response.code !== 0) throw new Error(response.stderr.trim() || 'Browser could not start. Check that terminal-browser is installed.');
        closeOverlay(props().overlay);
      } catch (error) { this.error = String(error.message ?? error); }
      this.busy = false;
      cx.notify();
    });
  }

  render() {
    const t = theme();
    return v_flex().w_full().min_h(0).p(18).gap(12).bg(t.popover).text_color(t.foreground)
      .child(div().text_size(16).font_semibold().child('Open browser'))
      .child(div().text_size(12.5).text_color(t.muted).child('Chromium in a new Herdr tab.'))
      .child(Input.new(this.url).h(36).px(10).rounded(8).border_1().border_color(t.input).bg(t.background).text_size(13).text_color(t.foreground))
      .when(Boolean(this.error), el => el.child(div().text_size(12).text_color(t.foreground).child(this.error)))
      .child(h_flex().justify_end().child(div().id('open-browser').h(32).px(14).flex().items_center().rounded(8)
        .bg(t.solid).text_color(t.solid_foreground).text_size(12.5).font_medium()
        .when(!this.busy, el => el.cursor_pointer().hover(s => s.opacity(0.88)))
        .on_click((_event, cx) => this.open(cx)).child(this.busy ? 'Opening…' : 'Open browser')));
  }
}
