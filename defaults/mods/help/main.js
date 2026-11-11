import { t as tr } from "herdr";
// The keybind list (Herdr's prefix+?), grouped, filterable by typing.
// Reads keys.json through keys(); edit keys.json to change any of them.

import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { keys, props, closeOverlay, theme } from "herdr";

const GROUPS = [
  ["spaces", /^space\./],
  ["tabs", /^tab\./],
  ["panes", /^pane\./],
  ["agents", /^agent\./],
  ["terminal", /^terminal\.|^send:/],
  ["window", /./],
];

const LABELS = {
  "app.feedback": "command-app-feedback",
  "app.check_updates": "command-app-check-updates",
  "app.update_core": "command-app-update-core",
  "app.update": "command-app-update",
  "terminal.paste_image": "command-terminal-paste-image",
  "terminal.scroll": "command-terminal-scroll",
  "app.palette": "command-palette",
  "layout.toggle": "command-layout-toggle",
  "tab.select": "command-tab-select",
  "space.select": "command-space-select",
  "mod.open": "command-mod-open",
  "tab.new": "palette-new-tab",
  "tab.rename": "palette-rename-tab",
  "tab.close": "palette-close-tab",
  "tab.next": "palette-next-tab",
  "tab.prev": "palette-previous-tab",
  "pane.close": "palette-close-pane",
  "pane.zoom": "palette-zoom-pane",
  "space.new": "palette-new-space",
  "space.rename": "palette-rename-space",
  "space.close": "palette-close-space",
  "space.next": "palette-next-space",
  "space.prev": "palette-previous-space",
  "layout.switch": "palette-next-layout",
  "layout.reset": "palette-reset-layout",
  "layout.clone": "palette-duplicate-layout",
  "mod.clone": "palette-duplicate-mod",
  "terminal.open": "palette-new-terminal",
  "terminal.copy": "palette-copy",
  "terminal.paste": "palette-paste",
  "agent.next": "palette-next-agent",
  "agent.prev": "palette-previous-agent",
  "window.minimize": "palette-minimize-window",
  "window.maximize": "palette-maximize-window",
  "window.close": "palette-close-window",
  "app.settings": "palette-settings",
  "app.help": "palette-keys",
  "app.welcome": "palette-welcome",
  "app.whats_new": "palette-what-s-new",
  "app.reset": "palette-reset-to-the-default-screen",
  "app.zoom_in": "palette-zoom-in",
  "app.zoom_out": "palette-zoom-out",
  "app.zoom_reset": "palette-reset-zoom",
  "app.quit": "palette-quit-herdr",
  "terminal.search": "command-terminal-search"
};

function label(action) {
  if (action.startsWith("send:")) return tr("help-type", { text: JSON.stringify(action.slice(5)) });
  const [name, arg] = action.split(":");
  if (name === "pane.split" && ["right", "down"].includes(arg)) return tr(`palette-split-${arg}`);
  if (["pane.go", "pane.swap"].includes(name) && ["left", "right", "up", "down"].includes(arg)) return tr(name === "pane.go" ? "palette-focus-pane" : "palette-swap-pane", { d: tr(`common-direction-${arg}`) });
  const words = LABELS[name] ? tr(LABELS[name]) : name.replace(/[._]/g, " ");
  return arg ? `${words} ${arg}` : words;
}

export default class Help extends View {
  init(_props, cx) {
    this.filter = "";
    this.input = InputState.new({ placeholder: tr("help-type-to-filter") });
    this.input.on("change", (_e, cx) => { this.filter = this.input.value(); cx.notify(); });
    this.input.focus();
    cx.timer.after(600, () => this.input.focus());
    this.handle = cx.focus_handle();
  }

  render(cx) {
    const t = theme();
    const f = this.filter.toLowerCase();
    const all = keys().filter((k) => !f || k.keys.includes(f) || label(k.action).toLowerCase().includes(f) || k.action.toLowerCase().includes(f));
    const seen = new Set();
    const groups = GROUPS.map(([name, re]) => {
      const rows = all.filter((k) => !seen.has(k) && re.test(k.action) && (seen.add(k) || true));
      return [name, rows];
    }).filter(([, rows]) => rows.length > 0);
    return v_flex()
      .id("help")
      .track_focus(this.handle)
      .on_key_down((e, cx) => {
        if (e.key === "escape") return closeOverlay(props().overlay);
      })
      .size_full()
      .min_h(0)
      .p(18)
      .gap(14)
      .bg(t.popover)
      .text_size(12)
      .child(
        h_flex()
          .h(22)
          .items_center()
          .justify_between()
          .child(div().text_size(15).font_semibold().text_color(t.foreground).child(tr("help-keybinds")))
          .child(div().id("help-esc").px(6).py(2).rounded(5).text_size(11).text_color(t.dim).cursor_pointer().hover((s) => s.bg(t.hover).text_color(t.foreground)).on_click(() => closeOverlay(props().overlay)).child(tr("help-esc"))),
      )
      .child(Input.new(this.input).h(36).flex_none().px(10).rounded(9).border_1().border_color(t.accent).bg(t.background).text_size(12.8).text_color(t.foreground))
      .child(
        v_flex()
          .flex_1()
          .min_h(0)
          .overflow_y_scrollbar()
          .py(2)
          .gap(10)
          .children(
            groups.map(([name, rows]) =>
              v_flex()
                .gap(2)
                .child(div().text_size(11.5).font_medium().text_color(t.dim).child(tr(`help-group-${name}`)))
                .children(
                  rows.map((k) =>
                    h_flex()
                      .h(32)
                      .px(8)
                      .rounded(7)
                      .items_center()
                      .child(div().w(190).text_color(t.accent).child(k.keys + (k.section !== "global" ? `  (${k.section})` : "")))
                      .child(div().text_color(t.foreground).child(label(k.action))),
                  ),
                ),
            ),
          ),
      );
  }
}
