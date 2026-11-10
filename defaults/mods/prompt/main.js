// A text prompt. Enter saves, Esc cancels.
import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { run, props, closeOverlay, theme, writeTextFile, exec, window, notify } from "herdr";

export default class Prompt extends View {
  init(_props, cx) {
    this.text = props()?.value ?? "";
    this.input = InputState.new({ value: this.text });
    if (this.text) this.input.set_selected_range({ start: 0, end: this.text.length });
    this.input.on("change", (_e, cx) => { this.text = this.input.value(); cx.notify(); });
    this.input.on("submit", () => this.submit());
    this.inputFocused = true;
    this.input.on("focus", (_e, cx) => { this.inputFocused = true; cx.notify(); });
    this.input.on("blur", (_e, cx) => { this.inputFocused = false; cx.notify(); });
    this.input.focus();
    cx.timer.after(600, () => this.input.focus());
    this.handle = cx.focus_handle();
  }

  submit() {
    const p = props();
    closeOverlay(p.overlay);
    if (!this.text) return;
    if (p.action === "create_file") {
      const full = p.target?.dir ? `${p.target.dir}/${this.text}` : this.text;
      writeTextFile(full, "", null).then(() => {
        notify({ title: "Created file", body: this.text, level: "success", timeout: 1500 });
      }).catch(e => {
        notify({ title: "Failed to create file", body: String(e), level: "error", timeout: 3000 });
      });
    } else if (p.action === "create_folder") {
      const full = p.target?.dir ? `${p.target.dir}/${this.text}` : this.text;
      if (!p.target?.on && window()?.platform === "windows") {
        const winPath = full.replace(/\//g, "\\");
        exec("powershell", ["-NoProfile", "-Command", `New-Item -ItemType Directory -LiteralPath '${winPath.replace(/'/g, "''")}' -Force`]).then(() => {
          notify({ title: "Created folder", body: this.text, level: "success", timeout: 1500 });
        }).catch(e => {
          notify({ title: "Failed to create folder", body: String(e), level: "error", timeout: 3000 });
        });
      } else {
        exec("mkdir", ["-p", full], { on: p.target?.on }).then(() => {
          notify({ title: "Created folder", body: this.text, level: "success", timeout: 1500 });
        }).catch(e => {
          notify({ title: "Failed to create folder", body: String(e), level: "error", timeout: 3000 });
        });
      }
    } else if (p.action === "rename") {
      const newPath = p.target?.parent ? `${p.target.parent}/${this.text}` : this.text;
      if (!p.target?.on && window()?.platform === "windows") {
        const oldWin = p.target?.oldPath?.replace(/\//g, "\\") ?? "";
        const newWin = newPath.replace(/\//g, "\\");
        exec("powershell", ["-NoProfile", "-Command", `Move-Item -LiteralPath '${oldWin.replace(/'/g, "''")}' -Destination '${newWin.replace(/'/g, "''")}' -Force`]).then(() => {
          notify({ title: "Renamed", body: `${p.value} → ${this.text}`, level: "success", timeout: 1500 });
        }).catch(e => {
          notify({ title: "Failed to rename", body: String(e), level: "error", timeout: 3000 });
        });
      } else {
        exec("mv", [p.target?.oldPath, newPath], { on: p.target?.on }).then(() => {
          notify({ title: "Renamed", body: `${p.value} → ${this.text}`, level: "success", timeout: 1500 });
        }).catch(e => {
          notify({ title: "Failed to rename", body: String(e), level: "error", timeout: 3000 });
        });
      }
    } else if (p.action === "find_in_folder") {
      const query = this.text;
      const folder = p.target?.dir || "";
      run("terminal.open", { folder });
      notify({ title: "Opened terminal for search", body: query, level: "info", timeout: 1500 });
    } else if (p.command?.name) {
      run(p.command.name, { ...(p.command.args ?? {}), name: this.text });
    }
  }

  key(e, cx) {
    if (e.key === "escape") { closeOverlay(props().overlay); cx.stop_propagation(); }
  }

  render() {
    const t = theme();
    const p = props() ?? {};
    const label = (text) => div().pt(16).pb(7).text_size(11.5).text_color(t.dim).child(text);
    const button = (id, text, primary, action) => h_flex().id(id).h(32).px(14).items_center().rounded(8)
      .border_1().border_color(primary ? t.solid : t.input).bg(primary ? t.solid : t.raised)
      .text_color(primary ? t.solid_foreground : t.foreground).text_size(12.5).font_medium().cursor_pointer()
      .hover(s => s.opacity(0.88)).on_click(action).child(text);
    return v_flex().id("prompt").track_focus(this.handle).on_key_down((e, cx) => this.key(e, cx))
      .w_full().min_h(0).p(18).bg(t.popover).text_color(t.foreground)
      .child(h_flex().items_center().child(div().flex_1().text_size(15).font_semibold().child((p.title ?? "").replace(/^./, (c) => c.toUpperCase())))
        .child(div().text_size(11).text_color(t.dim).child("Esc")))
      .child(label("Name"))
      .child(Input.new(this.input).h(36).px(10).rounded(9).border_1().border_color(this.inputFocused ? t.accent : t.input).bg(t.background).text_size(12.8).text_color(t.foreground))
      .when(Boolean(p.hint), el => el.child(div().pt(6).text_size(11.5).text_color(t.dim).child(p.hint)))
      .child(h_flex().pt(20).gap(8).justify_end()
        .child(button("pm-cancel", "Cancel", false, () => closeOverlay(p.overlay)))
        .child(button("pm-save", "Save", true, () => this.submit())));
  }
}
