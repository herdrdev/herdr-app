// Puts a space, or a folder, in a folder of the spaces sidebar. Enter saves, Esc
// cancels. props { space } or { folder }. An empty name puts it at the top; a new
// name makes a folder there. Empty folders go away.
// Folders live in shared("space.folders"): [{ name, collapsed, ids }].
import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input } from "gpui-base";
import { props, closeOverlay, theme, shared, setShared } from "herdr";

const folders = () => shared("space.folders") ?? [];

function moveToFolder(space, name) {
  const list = folders().map((f) => ({ ...f, ids: f.ids.filter((id) => id !== space) }));
  name = name.trim();
  if (name) {
    const into = list.find((f) => f.name.toLowerCase() === name.toLowerCase());
    if (into) into.ids.push(space);
    else list.push({ name, collapsed: false, ids: [space] });
  }
  setShared("space.folders", prune(list));
}

// The folders holding `name`, innermost first: [name, its parent, ...].
function lineage(list, name) {
  const path = [];
  for (let at = name; at && !path.includes(at); at = list.find((f) => f.name === at)?.parent ?? null) path.push(at);
  return path;
}

// Puts folder `name` in the folder named `into` ("" puts it at the top), never inside itself.
function moveFolder(name, into) {
  const list = folders().map((f) => ({ ...f }));
  const self = list.find((f) => f.name === name);
  if (!self) return;
  into = into.trim();
  let target = into ? list.find((f) => f.name.toLowerCase() === into.toLowerCase()) : null;
  if (into && !target) list.push((target = { name: into, collapsed: false, ids: [], parent: self.parent ?? null }));
  if (target && lineage(list, target.name).includes(name)) return;
  self.parent = target?.name ?? null;
  setShared("space.folders", prune(list));
}

// A folder stays while it holds a space or a folder that stays.
function prune(list) {
  for (;;) {
    const kept = list.filter((f) => f.ids.length || list.some((c) => c.parent === f.name));
    if (kept.length === list.length) return kept.map((f) => (f.parent && !kept.some((p) => p.name === f.parent) ? { ...f, parent: null } : f));
    list = kept;
  }
}

export default class SpaceFolder extends View {
  init(_props, cx) {
    const p = props() ?? {};
    this.text = (p.folder
      ? folders().find((f) => f.name === p.folder)?.parent
      : folders().find((f) => f.ids.includes(p.space))?.name) ?? "";
    this.input = InputState.new({ value: this.text });
    if (this.text) this.input.set_selected_range({ start: 0, end: this.text.length });
    this.input.on("change", (_e, cx) => { this.text = this.input.value(); cx.notify(); });
    this.input.on("submit", () => this.submit(this.text));
    this.inputFocused = true;
    this.input.on("focus", (_e, cx) => { this.inputFocused = true; cx.notify(); });
    this.input.on("blur", (_e, cx) => { this.inputFocused = false; cx.notify(); });
    this.input.focus();
    cx.timer.after(600, () => this.input.focus());
    this.handle = cx.focus_handle();
  }

  submit(name) {
    const p = props();
    closeOverlay(p.overlay);
    if (p.folder) moveFolder(p.folder, name);
    else moveToFolder(p.space, name);
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
    // A folder can't go into itself or a folder inside it.
    const existing = folders().filter((f) => !p.folder || !lineage(folders(), f.name).includes(p.folder));
    return v_flex().id("space-folder").track_focus(this.handle).on_key_down((e, cx) => this.key(e, cx))
      .w_full().min_h(0).p(18).bg(t.popover).text_color(t.foreground)
      .child(h_flex().items_center().child(div().flex_1().text_size(15).font_semibold().child(p.folder ? `Move folder ${p.folder}` : "Move to folder"))
        .child(div().text_size(11).text_color(t.dim).child("Esc")))
      .child(label("Folder"))
      .child(Input.new(this.input).h(36).px(10).rounded(9).border_1().border_color(this.inputFocused ? t.accent : t.input).bg(t.background).text_size(12.8).text_color(t.foreground))
      .child(div().pt(6).text_size(11.5).text_color(t.dim).child(p.folder ? "A new name makes a folder. Empty moves it to the top." : "A new name makes a folder. Empty takes the space out."))
      .when(existing.length > 0, el => el.child(h_flex().pt(12).gap(6).flex_wrap()
        .children(existing.map((f, i) => div().id(`sf-${i}`).px(9).py(4).rounded(7).bg(t.raised).text_size(12)
          .cursor_pointer().hover(s => s.bg(t.hover)).on_click(() => this.submit(f.name)).child(f.name)))))
      .child(h_flex().pt(20).gap(8).justify_end()
        .child(button("sf-cancel", "Cancel", false, () => closeOverlay(p.overlay)))
        .child(button("sf-save", "Save", true, () => this.submit(this.text))));
  }
}
