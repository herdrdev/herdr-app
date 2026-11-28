// Quick notes: each note is a checklist. The list is the history, newest edit
// first, and a note opens to tick its tasks off. Kept in this mod's localStorage.
import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex, InputState, Input, Checkbox, Scrollbar } from "gpui-base";
import { theme } from "herdr";

const STORE = "notes";
const UNTITLED = "Untitled";
const ICON = {
  check: "mods/workspace-chrome/check.svg",
  x: "mods/workspace-chrome/x.svg",
  plus: "mods/workspace-chrome/plus.svg",
  back: "mods/workspace-chrome/chevron-left.svg",
  pen: "mods/workspace-chrome/square-pen.svg",
};

const now = () => Date.now();
const uid = () => now().toString(36) + "-" + Math.random().toString(36).slice(2, 8);

// Short and readable in a 368px panel; the exact date for anything older.
function ago(ts) {
  const s = Math.max(0, Math.round((now() - ts) / 1000));
  if (s < 45) return "just now";
  if (s < 3600) return `${Math.max(1, Math.round(s / 60))}m ago`;
  if (s < 86400) return `${Math.round(s / 3600)}h ago`;
  if (s < 604800) return `${Math.round(s / 86400)}d ago`;
  const d = new Date(ts);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

const stop = (_e, cx) => cx.stop_propagation();

export default class Notes extends View {
  init(_props, cx) {
    this.handle = cx.focus_handle();
    this.notes = [];
    this.openId = null;
    this.editing = null;
    this.focused = null;
    this.load();
    // Inputs are host calls, so they belong here and not in render().
    this.titleInput = this.makeInput("Note title", {
      submit: (cx) => this.finishTitle(cx, true),
      blur: (cx) => this.finishTitle(cx, false),
    });
    this.itemInput = this.makeInput("Task", { submit: (cx) => this.finishItem(cx), blur: (cx) => this.finishItem(cx) });
    this.addInput = this.makeInput("Add a task, then Enter", { submit: (cx) => this.addTask(cx) });
  }

  // One input per thing being typed in. Blur commits the ones that edit a value;
  // the add input has nothing to commit.
  makeInput(placeholder, { submit, blur }) {
    const state = InputState.new({ placeholder });
    if (submit) state.on("submit", (_e, cx) => submit(cx));
    state.on("focus", (_e, cx) => { this.focused = state; cx.notify(); });
    state.on("blur", (_e, cx) => {
      if (this.focused === state) this.focused = null;
      if (blur) blur(cx);
      else cx.notify();
    });
    return state;
  }

  load() {
    let raw = null;
    try { raw = JSON.parse(localStorage.getItem(STORE) ?? "[]"); } catch { raw = null; }
    this.notes = (Array.isArray(raw) ? raw : []).flatMap((n) => {
      if (!n || typeof n !== "object") return [];
      const items = (Array.isArray(n.items) ? n.items : []).flatMap((i) => {
        if (!i || typeof i !== "object") return [];
        return [{ id: String(i.id ?? uid()), text: String(i.text ?? ""), done: Boolean(i.done) }];
      });
      const created = Number(n.created) || now();
      return [{ id: String(n.id ?? uid()), title: String(n.title || UNTITLED), items, created, updated: Number(n.updated) || created }];
    });
    this.order();
  }

  save() {
    try { localStorage.setItem(STORE, JSON.stringify(this.notes)); } catch { /* a full disk keeps the panel usable */ }
  }

  order() {
    this.notes.sort((a, b) => b.updated - a.updated);
  }

  openNote() {
    return this.notes.find((n) => n.id === this.openId) ?? null;
  }

  touch(note) {
    note.updated = now();
    this.order();
    this.save();
  }

  // A new note opens on its title, so naming one is typing and nothing else.
  create(cx) {
    const note = { id: uid(), title: UNTITLED, items: [], created: now(), updated: now() };
    this.notes.unshift(note);
    this.openId = note.id;
    this.save();
    this.editTitle(note, cx);
  }

  editTitle(note, cx) {
    this.editing = { kind: "title" };
    this.titleInput.set_value(note.title);
    this.titleInput.set_selected_range({ start: 0, end: note.title.length });
    this.titleInput.focus();
    // The input mounts on the next frame; focus again once it is there.
    cx.timer.after(30, () => this.titleInput.focus());
    cx.notify();
  }

  finishTitle(cx, typed) {
    if (this.editing?.kind !== "title") return;
    this.editing = null;
    const note = this.openNote(), title = this.titleInput.value().trim();
    if (note && title && title !== note.title) { note.title = title; this.touch(note); }
    else if (note && !title) this.titleInput.set_value(note.title);
    // Naming a new note leads straight into its first task.
    if (typed && note && !note.items.length) this.addInput.focus();
    cx.notify();
  }

  editItem(item, cx) {
    this.editing = { kind: "item", id: item.id };
    this.itemInput.set_value(item.text);
    this.itemInput.set_selected_range({ start: 0, end: item.text.length });
    this.itemInput.focus();
    cx.timer.after(30, () => this.itemInput.focus());
    cx.notify();
  }

  finishItem(cx) {
    if (this.editing?.kind !== "item") return;
    const id = this.editing.id, note = this.openNote();
    const item = note?.items.find((i) => i.id === id), text = this.itemInput.value().trim();
    this.editing = null;
    // An emptied task keeps its old text: deleting is the × on its row.
    if (item && text && text !== item.text) { item.text = text; this.touch(note); }
    cx.notify();
  }

  addTask(cx) {
    const note = this.openNote(), text = this.addInput.value().trim();
    if (!note || !text) return;
    note.items.push({ id: uid(), text, done: false });
    this.addInput.set_value("");
    this.addInput.focus();
    this.touch(note);
    cx.notify();
  }

  toggle(note, item, cx) {
    item.done = !item.done;
    this.touch(note);
    cx.notify();
  }

  removeItem(note, item, cx) {
    note.items = note.items.filter((i) => i !== item);
    this.touch(note);
    cx.notify();
  }

  removeNote(note, cx) {
    this.notes = this.notes.filter((n) => n !== note);
    if (this.openId === note.id) this.openId = null;
    this.editing = null;
    this.save();
    cx.notify();
  }

  async close() {
    this.editing = null;
  }

  iconButton(t, id, icon, label, action) {
    return div().id(id).size(22).flex_none().flex().items_center().justify_center().rounded(6)
      .text_color(t.muted).hover((s) => s.bg(t.hover).text_color(t.foreground))
      .on_mouse_down("left", stop).on_click(action).accessibility_label(label)
      .child(svg(icon).size(13));
  }

  header(t, note, cx) {
    return h_flex().h(42).flex_none().px(10).gap(6).items_center().border_b(1).border_color(t.border)
      .when(Boolean(note), (el) => el.child(this.iconButton(t, "notes-back", ICON.back, "All notes", (_e, cx) => {
        this.editing = null;
        this.openId = null;
        cx.notify();
      })))
      .child(div().font_semibold().child("Notes"))
      .when(!note && this.notes.length > 0, (el) => el.child(div().px(5).py(1).rounded(4)
        .border_1().border_color(t.border).text_size(10).text_color(t.muted).child(String(this.notes.length))))
      .child(div().flex_1())
      .child(this.iconButton(t, "notes-new", ICON.plus, "New note", (_e, cx) => this.create(cx)))
      .when(Boolean(note), (el) => el.child(this.iconButton(t, "notes-delete", ICON.x, "Delete note", (_e, cx) => this.removeNote(note, cx))));
  }

  empty(t, cx) {
    return v_flex().flex_1().min_h(0).items_center().justify_center().gap(6).p(24)
      .child(div().text_color(t.muted).child("No notes yet"))
      .child(div().text_size(11.5).text_center().text_color(t.dim).child("Write down what you owe yourself, then tick it off."))
      .child(div().id("notes-first").mt(6).h(28).px(12).flex().items_center().rounded(8).cursor_pointer()
        .bg(t.solid).text_color(t.solid_foreground).text_size(12).hover((s) => s.opacity(0.88))
        .on_click((_e, cx) => this.create(cx)).child("New note"));
  }

  row(t, note, cx) {
    const total = note.items.length, done = note.items.filter((i) => i.done).length, complete = total > 0 && done === total;
    return h_flex().id(`notes-row-${note.id}`).w_full().px(9).py(8).gap(8).items_start().rounded(8)
      .cursor_pointer().hover((s) => s.bg(t.hover))
      .on_mouse_down("left", stop)
      .on_click((_e, cx) => { this.openId = note.id; this.editing = null; cx.notify(); })
      .child(v_flex().flex_1().min_w(0).gap(3)
        .child(h_flex().gap(6).items_center()
          .when(complete, (el) => el.child(svg(ICON.check).size(11).flex_none().text_color(t.done)))
          .child(div().flex_1().min_w(0).truncate().text_size(12.8).font_medium()
            .text_color(complete ? t.muted : t.foreground).child(note.title)))
        .child(h_flex().gap(5).text_size(11).text_color(t.dim)
          .child(div().child(total ? `${done}/${total} done` : "empty"))
          .child(div().child("·"))
          .child(div().child(ago(note.updated)))))
      .child(div().id(`notes-row-del-${note.id}`).flex_none().size(20).flex().items_center().justify_center().rounded(5)
        .text_color(t.dim).hover((s) => s.bg(t.raised).text_color(t.foreground))
        .on_mouse_down("left", stop)
        .on_click((_e, cx) => { cx.stop_propagation(); this.removeNote(note, cx); })
        .child(svg(ICON.x).size(12)));
  }

  list(t, cx) {
    if (!this.notes.length) return this.empty(t, cx);
    return div().flex_1().min_h(0).relative()
      .child(v_flex().id("notes-list").absolute().inset_0().overflow_y_scroll().p(6).gap(2)
        .children(this.notes.map((note) => this.row(t, note, cx))))
      .child(Scrollbar.vertical("notes-list").absolute().inset_0());
  }

  mark(t, done) {
    return div().size(16).flex().items_center().justify_center().rounded(4)
      .border_1().border_color(done ? t.accent : t.input)
      .when(done, (el) => el.bg(t.accent))
      .when(done, (el) => el.child(svg(ICON.check).size(10).text_color(t.accent_foreground)));
  }

  task(t, note, item, cx) {
    const editing = this.editing?.kind === "item" && this.editing.id === item.id;
    return h_flex().id(`notes-task-${item.id}`).w_full().px(9).py(6).gap(8).items_start().rounded(7)
      .hover((s) => s.bg(t.hover))
      .child(Checkbox.new(`notes-check-${item.id}`).checked(item.done).flex_none().mt(1)
        .cursor_pointer().accessibility_label(item.text || "Task")
        .on_click((_e, cx) => this.toggle(note, item, cx))
        .child(this.mark(t, item.done)))
      .when(editing, (el) => el.child(div().flex_1().min_w(0).h(26).px(6).flex().items_center().rounded(6)
        .border_1().border_color(t.accent).bg(t.raised).on_mouse_down("left", stop)
        .child(Input.new(this.itemInput).h_full().text_size(12.2).text_color(t.foreground))))
      .when(!editing, (el) => el.child(div().id(`notes-task-text-${item.id}`).flex_1().min_w(0)
        .text_size(12.5).cursor_text().text_color(item.done ? t.muted : t.foreground)
        .when(item.done, (e) => e.line_through())
        .on_click((_e, cx) => this.editItem(item, cx)).child(item.text || " ")))
      .child(div().id(`notes-task-del-${item.id}`).flex_none().size(18).flex().items_center().justify_center()
        .rounded(5).text_color(t.dim).hover((s) => s.bg(t.raised).text_color(t.foreground))
        .on_mouse_down("left", stop)
        .on_click((_e, cx) => this.removeItem(note, item, cx))
        .child(svg(ICON.x).size(11)));
  }

  editor(t, note, cx) {
    const total = note.items.length, done = note.items.filter((i) => i.done).length;
    const editingTitle = this.editing?.kind === "title";
    return v_flex().flex_1().min_h(0)
      .child(v_flex().flex_none().px(12).pt(10).pb(9).gap(6).border_b(1).border_color(t.border)
        .when(editingTitle, (el) => el.child(div().w_full().h(30).px(8).flex().items_center().rounded(7)
          .border_1().border_color(t.accent).bg(t.raised).on_mouse_down("left", stop)
          .child(Input.new(this.titleInput).h_full().text_size(13).text_color(t.foreground))))
        .when(!editingTitle, (el) => el.child(h_flex().id("notes-title").w_full().gap(6).items_start()
          .rounded(6).cursor_text().hover((s) => s.bg(t.hover))
          .on_click((_e, cx) => this.editTitle(note, cx))
          .child(div().flex_1().min_w(0).text_size(14).font_semibold().child(note.title || UNTITLED))
          .child(svg(ICON.pen).size(12).flex_none().mt(2).text_color(t.dim))))
        .child(h_flex().gap(5).text_size(11).text_color(t.dim)
          .child(div().child(total ? `${done} of ${total} done` : "no tasks yet"))
          .child(div().child("·"))
          .child(div().child(`edited ${ago(note.updated)}`)))
        .when(total > 0, (el) => el.child(div().h(3).w_full().rounded(2).bg(t.raised)
          .child(div().h(3).rounded(2).bg(t.accent).w(`${Math.round((done / total) * 100)}%`)))))
      .child(div().flex_1().min_h(0).relative()
        .child(v_flex().id("notes-tasks").absolute().inset_0().overflow_y_scroll().p(5).gap(1)
          .children(note.items.map((item) => this.task(t, note, item, cx)))
          .when(!note.items.length, (el) => el.child(div().p(8).text_size(11.5).text_color(t.dim)
            .child("No tasks yet. Type one below and press Enter."))))
        .child(Scrollbar.vertical("notes-tasks").absolute().inset_0()));
  }

  addRow(t) {
    const focused = this.focused === this.addInput;
    return h_flex().flex_none().p(10).gap(8).border_t(1).border_color(t.border)
      .child(div().id("notes-add").flex_1().min_w(0).h(32).px(10).flex().items_center().rounded(8)
        .border_1().border_color(focused ? t.accent : t.border).cursor_text().on_mouse_down("left", stop)
        .child(Input.new(this.addInput).h_full().text_size(12).text_color(t.foreground)))
      .child(div().id("notes-add-go").h(32).px(12).flex().items_center().rounded(8).bg(t.raised)
        .text_color(t.foreground).text_size(12).cursor_pointer().hover((s) => s.bg(t.hover))
        .on_mouse_down("left", stop).on_click((_e, cx) => this.addTask(cx)).child("Add"));
  }

  render(cx) {
    const t = theme(), note = this.openNote();
    return v_flex().id("notes").size_full().min_h(0).rounded(10).overflow_hidden()
      .bg(t.background).border_1().border_color(t.border).text_color(t.foreground).text_size(13)
      .track_focus(this.handle)
      .on_key_down((e, cx) => {
        if (e.key !== "escape") return;
        if (this.editing) { this.editing = null; this.handle.focus(); }
        else if (this.openId) this.openId = null;
        else return;
        cx.stop_propagation();
        cx.notify();
      })
      .child(this.header(t, note, cx))
      .child(note ? this.editor(t, note, cx) : this.list(t, cx))
      .when(Boolean(note), (el) => el.child(this.addRow(t)));
  }
}
