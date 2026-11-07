import { div } from "gpui-kit";
import { h_flex, v_flex, Editor, EditorState } from "gpui-base";
import { exec, readTextFile, writeTextFile, closeOverlay, setShared, theme, settings } from "herdr";
import { createGit } from "./git.js";

export class MergePanel {
  constructor(props, cx) {
    this.props = props; this.repo = createGit(props.root, props.on); this.editors = {};
    this.load(cx);
  }

  close() {
    this.closed = true;
    for (const editor of Object.values(this.editors)) editor.release();
    this.editors = {};
  }

  load(cx) {
    if (this.busy) return;
    this.busy = true; this.loaded = false; this.error = null; this.note = null; this.restriction = "";
    cx.spawn(async cx => {
      try {
        const { root, on, file } = this.props, path = file.path;
        if (!path || /^(?:[a-z]:|[\\/])/i.test(path) || path.split(/[\\/]/).includes("..") || path.includes("\0")) throw new Error("Only files inside the repository can be opened.");
        this.path = `${root.replace(/[\\/]$/, "")}/${path}`;
        const git = args => exec("git", ["--no-pager", "--literal-pathspecs", ...args], { cwd: root, on });
        const [index, base, current, incoming, original, head] = await Promise.all([
          git(["ls-files", "--unmerged", "-z", "--", path]),
          ...[1, 2, 3].map(stage => git(["show", "--no-ext-diff", "--no-textconv", `:${stage}:${path}`])),
          readTextFile(this.path, { on }),
          git(["rev-parse", "--verify", "--quiet", "HEAD"]),
        ]);
        if (this.closed) return;
        if (index.code !== 0) throw new Error(index.stderr || "Could not read conflict information.");
        if (![0, 1].includes(head.code)) throw new Error(head.stderr || "Could not read HEAD.");
        this.indexSnapshot = index.stdout; this.headSnapshot = head.code === 0 ? head.stdout.trim() : null;
        const entries = index.stdout.split("\0").filter(Boolean);
        this.sides = { base: base.code === 0 ? base.stdout : null, current: current.code === 0 ? current.stdout : null, incoming: incoming.code === 0 ? incoming.stdout : null };
        if (!entries.length) this.restriction = "This file is no longer conflicted. Check it in Changes.";
        else if (entries.some(entry => !/^100(?:644|755) /.test(entry))) this.restriction = "Resolve directory, symlink, or submodule conflicts at the file level, then stage them.";
        else if (this.sides.current === null || this.sides.incoming === null || original === null) this.restriction = "One side deleted this file. Choose a version or delete it, then stage the resolution.";
        else if ([original, ...Object.values(this.sides)].some(text => text?.includes("\0"))) this.restriction = "Binary files cannot be merged as text. Choose a version at the file level, then stage it.";
        else if ([original, ...Object.values(this.sides)].some(text => text && text.length > 400000)) this.restriction = "Resolve large files in an external editor, then stage them.";
        if (on) this.restriction = "Remote files cannot be saved here. Resolve them in a remote editor, then stage them.";
        this.original = original;
        this.close(); this.closed = false;
        const t = theme(), ext = path.split(".").pop();
        const options = { language: ext, font_family: settings().font_family, font_size: 12, soft_wrap: false, syntax: { keyword: t.ansi5, string: t.ansi2, comment: t.dim, function: t.ansi4, type: t.ansi3, number: t.ansi1 } };
        for (const side of ["base", "current", "incoming"]) this.editors[side] = EditorState.new({ ...options, readonly: true, value: (this.sides[side] ?? "The file does not exist on this side.").slice(0, 400000) });
        this.editors.result = EditorState.new({ ...options, readonly: Boolean(this.restriction), value: (original ?? "").slice(0, 400000) });
        this.editors.result.on("change", (_event, cx) => cx.notify());
        this.loaded = true;
      } catch (error) { this.error = String(error); }
      finally { this.busy = false; cx.notify(); }
    });
    cx.notify();
  }

  accept(side, cx) {
    if (this.busy || !this.loaded || this.restriction || !this.editors.result || this.sides[side] === null) return;
    this.editors.result.set_value(this.sides[side]); this.note = "Updated Result. The file will change when you save."; cx.notify();
  }

  async verifyConflict() {
    const { root, on, file } = this.props;
    const git = args => exec("git", ["--no-pager", "--literal-pathspecs", ...args], { cwd: root, on });
    const [index, head] = await Promise.all([git(["ls-files", "--unmerged", "-z", "--", file.path]), git(["rev-parse", "--verify", "--quiet", "HEAD"])]);
    if (index.code !== 0 || ![0, 1].includes(head.code)) throw new Error(index.stderr || head.stderr || "Could not verify the conflict state.");
    if (index.stdout !== this.indexSnapshot || (head.code === 0 ? head.stdout.trim() : null) !== this.headSnapshot) throw new Error("HEAD or the conflict index changed. Reload before resolving the conflict.");
  }

  save(stage, cx) {
    if (this.busy || !this.loaded || this.restriction || !this.editors.result || this.props.on) return;
    const result = this.editors.result.value();
    if (stage && /^(?:<{7,}|={7,}|>{7,}|\|{7,})(?: .*)?\r?$/m.test(result)) {
      this.error = "Conflict markers remain. Resolve them in Result before Save & Stage."; cx.notify(); return;
    }
    this.busy = true; this.error = null; this.note = null;
    cx.spawn(async cx => {
      let saved = false;
      try {
        await this.verifyConflict();
        await writeTextFile(this.path, result, this.original);
        this.original = result; saved = true;
        setShared("source-control.revision", Date.now());
        if (stage) { await this.verifyConflict(); await this.repo.run("stage", { file: this.props.file }); }
        if (stage) setShared("source-control.revision", Date.now());
        this.note = stage ? "Saved and staged the result." : "Saved the result. Use Save & Stage to finish resolving the conflict.";
        if (stage) this.restriction = "Staged. Close this window to review it in Source Control.";
      } catch (error) { this.error = saved ? `The file was saved, but staging failed: ${error}` : `Could not save. Reload if the file changed externally: ${error}`; }
      finally { this.busy = false; cx.notify(); }
    });
    cx.notify();
  }

  render() {
    const t = theme(), enabled = Boolean(this.loaded && this.editors.result && !this.busy && !this.restriction);
    const button = (id, label, action, active = true) => div().id(id).h(28).px(10).flex().items_center().rounded(4).bg(active ? t.raised : t.surface).text_color(active ? t.foreground : t.dim)
      .when(active, e => e.cursor_pointer().hover(s => s.bg(t.hover))).on_click((_event, cx) => { if (active) action(cx); }).child(label);
    const editor = (name, label) => v_flex().flex_1().min_w(0).min_h(0).border_1().border_color(t.border)
      .child(div().h(28).flex_none().px(10).flex().items_center().bg(t.raised).font_semibold().child(label))
      .child(this.editors[name] ? Editor.new(this.editors[name]).flex_1().min_h(0) : div().p(12).text_color(t.dim).child("Loading…"));
    return v_flex().w_full().h_full().min_h(0).bg(t.background).text_color(t.foreground).text_size(12)
      .child(h_flex().h(44).flex_none().px(14).gap(8).items_center().border_b(1).border_color(t.border)
        .child(div().font_semibold().child("Merge Editor"))
        .child(div().flex_1().min_w(0).truncate().text_color(t.muted).child(this.props.file.path))
        .child(button("merge-base", this.showBase ? "Hide Base" : "Show Base", cx => { this.showBase = !this.showBase; cx.notify(); }))
        .child(button("merge-reload", "Reload", cx => this.load(cx), !this.busy).tooltip("Discard unsaved edits and reload the file."))
        .child(button("merge-close", "Close", () => closeOverlay(this.props.overlay), !this.busy)))
      .when(Boolean(this.restriction), e => e.child(div().px(14).py(8).flex_none().text_color(t.muted).child(this.restriction)))
      .child(h_flex().h(250).flex_none().min_w(0).px(10).pt(8).gap(8)
        .when(this.showBase, e => e.child(editor("base", "Base")))
        .child(editor("current", "Current · ours" )).child(editor("incoming", "Incoming · theirs")))
      .child(h_flex().h(38).flex_none().px(10).gap(8).items_center()
        .child(div().flex_1().font_semibold().child("Result"))
        .child(button("merge-current", "Accept Current", cx => this.accept("current", cx), enabled))
        .child(button("merge-incoming", "Accept Incoming", cx => this.accept("incoming", cx), enabled)))
      .child(div().flex_1().min_h(0).px(10).child(this.editors.result ? Editor.new(this.editors.result).size_full() : div().p(12).text_color(t.dim).child("Loading…")))
      .when(Boolean(this.error || this.note), e => e.child(div().id("merge-message").max_h(72).flex_none().overflow_y_scrollbar().px(14).py(8).text_color(this.error ? t.ansi1 : t.ansi2).child(this.error || this.note)))
      .child(h_flex().h(48).flex_none().px(14).gap(8).items_center().border_t(1).border_color(t.border)
        .child(div().flex_1().text_color(t.dim).child(this.busy ? "Working…" : this.editors.result && this.editors.result.value() !== this.original ? "Unsaved edits" : ""))
        .child(button("merge-save", "Save Result", cx => this.save(false, cx), enabled))
        .child(button("merge-stage", "Save & Stage", cx => this.save(true, cx), enabled)));
  }
}
