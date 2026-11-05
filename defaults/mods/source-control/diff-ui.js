import { div } from "gpui-kit";
import { h_flex, v_flex, Scrollbar, v_virtual_list } from "gpui-base";
import { appHome, writeTextFile, closeOverlay, setShared, theme } from "herdr";
import { unlink } from "fs/promises";
import { createGit } from "./git.js";
import { parseDiff, selectedPatch } from "./diff.js";

export class DiffPanel {
  constructor(props, cx) {
    this.props = props; this.repo = createGit(props.root, props.on);
    this.selected = new Set(); this.rows = []; this.patch = ""; this.width = 600;
    this.load(cx);
  }

  read() {
    return this.props.hash ? this.repo.commitDiff(this.props.hash, this.props.file) : this.repo.diff(this.props.file, this.props.group);
  }

  setPatch(patch) {
    this.patch = patch; this.diff = parseDiff(patch); this.selected.clear(); this.confirmDiscard = false;
    this.rows = this.diff.headers.map(text => ({ kind: "header", text }));
    for (const hunk of this.diff.hunks) {
      this.rows.push({ kind: "hunk", text: hunk.header, hunk });
      for (const line of hunk.lines) this.rows.push(line);
    }
    this.width = this.rows.reduce((width, row) => Math.max(width, row.text.length * 7.3 + 130), 600);
  }

  load(cx) {
    if (this.busy) return;
    this.busy = true; this.error = null; this.note = null; this.confirmDiscard = false;
    cx.spawn(async (cx) => {
      try { this.setPatch(await this.read()); }
      catch (error) { this.error = String(error); }
      finally { this.busy = false; cx.notify(); }
    });
    cx.notify();
  }

  restriction() {
    if (this.props.hash) return "Commit history is read-only.";
    if (this.props.on) return "Partial patches are not supported on remote machines. Stage or unstage the whole file.";
    if (this.props.group === "conflicts") return "Resolve the conflict, then stage the whole file.";
    return this.diff?.unsupported || "";
  }

  toggle(index, cx) {
    if (this.busy || this.error || this.restriction()) return;
    this.confirmDiscard = false;
    if (this.selected.has(index)) this.selected.delete(index); else this.selected.add(index);
    cx.notify();
  }

  toggleHunk(hunk, cx) {
    if (this.busy || this.error || this.restriction()) return;
    this.confirmDiscard = false;
    const indices = hunk.lines.filter(line => line.kind === "+" || line.kind === "-").map(line => line.index);
    const clear = indices.every(index => this.selected.has(index));
    for (const index of indices) { if (clear) this.selected.delete(index); else this.selected.add(index); }
    cx.notify();
  }

  requestDiscard(cx) {
    if (this.busy || this.error || this.restriction() || !this.selected.size || this.props.group !== "changes") return;
    this.confirmDiscard = true; cx.notify();
  }

  apply(cx, discard = false) {
    if (this.busy || this.error || this.restriction() || !this.selected.size) return;
    if (discard && (!this.confirmDiscard || this.props.group !== "changes")) return;
    this.busy = true; this.error = null; this.note = null; this.confirmDiscard = false;
    cx.spawn(async (cx) => {
      let patchFile, applied = false;
      try {
        const current = await this.read();
        if (current !== this.patch) {
          this.setPatch(current); this.note = "The diff changed and has been refreshed. Select the lines again.";
          return;
        }
        const patch = selectedPatch(this.diff, this.selected, discard || this.props.group === "staged");
        patchFile = `${appHome()}/data/source-control-${Date.now()}-${Math.random().toString(16).slice(2)}.patch`;
        this.patchFile = patchFile;
        await writeTextFile(patchFile, patch, null);
        await this.repo.run("applyPatch", { patchFile, cached: !discard });
        applied = true;
        setShared("source-control.revision", Date.now());
        this.setPatch(await this.read());
        this.note = discard ? "Discarded the selected working tree changes." : this.props.group === "staged" ? "Unstaged the selected changes." : "Staged the selected changes.";
      } catch (error) { this.error = `${applied ? "Changes were applied, but the diff could not be refreshed" : "Could not apply the changes"}: ${error}`; }
      finally {
        if (patchFile) await unlink(patchFile).catch(() => {});
        this.patchFile = null;
        this.busy = false; cx.notify();
      }
    });
    cx.notify();
  }

  async close() {
    if (this.patchFile) await unlink(this.patchFile).catch(() => {});
  }

  renderRow(row, index, t) {
    const mutable = !this.busy && !this.error && !this.restriction();
    if (row.kind === "header") return div().h(22).flex_none().px(12).text_color(t.dim).whitespace_nowrap().child(row.text);
    if (row.kind === "hunk") return h_flex().id(`diff-hunk-${index}`).h(22).flex_none().px(12).gap(12).items_center().bg(t.raised).text_color(t.accent)
      .when(mutable, e => e.cursor_pointer().tooltip("Select or clear all changed lines in this hunk").on_click((_event, cx) => this.toggleHunk(row.hunk, cx)))
      .child(div().flex_1().whitespace_nowrap().child(row.text))
      .when(mutable, e => e.child(div().text_size(10).child("Select hunk")));
    const changed = row.kind === "+" || row.kind === "-", selected = this.selected.has(row.index);
    return h_flex().id(`diff-line-${index}`).h(22).flex_none().items_center().gap(8).px(8)
      .bg(selected ? t.selected : row.kind === "+" ? t.ansi2 + "18" : row.kind === "-" ? t.ansi1 + "18" : t.background)
      .when(changed && mutable, e => e.cursor_pointer().hover(s => s.bg(t.hover)).tooltip("Select or clear this line").on_click((_event, cx) => this.toggle(row.index, cx)))
      .child(div().w(12).flex_none().text_color(t.accent).child(selected ? "✓" : ""))
      .child(div().w(38).flex_none().text_right().text_color(t.dim).child(row.oldLine == null ? "" : String(row.oldLine)))
      .child(div().w(38).flex_none().text_right().text_color(t.dim).child(row.newLine == null ? "" : String(row.newLine)))
      .child(div().w(10).flex_none().text_color(row.kind === "+" ? t.ansi2 : row.kind === "-" ? t.ansi1 : t.dim).child(row.kind))
      .child(div().whitespace_nowrap().child(row.text));
  }

  render() {
    const t = theme(), restriction = this.restriction(), enabled = !this.busy && !this.error && !restriction && this.selected.size > 0;
    const group = this.props.hash ? `Commit ${this.props.hash.slice(0, 8)}` : this.props.group === "staged" ? "Staged Changes" : this.props.group === "conflicts" ? "Merge Changes" : "Changes";
    return v_flex().size_full().min_h(0).min_w(0).bg(t.background).text_color(t.foreground).text_size(12)
      .child(h_flex().h(44).flex_none().px(14).gap(10).items_center().border_b(1).border_color(t.border)
        .child(div().font_semibold().child(group)).child(div().flex_1().min_w(0).truncate().tooltip(this.props.file.path).child(this.props.file.path))
        .child(div().id("diff-refresh").px(10).py(5).rounded(4).bg(t.raised).when(!this.busy, e => e.cursor_pointer()).on_click((_event, cx) => this.load(cx)).child("Refresh"))
        .when(Boolean(this.props.overlay), e => e.child(div().id("diff-close").px(10).py(5).rounded(4).when(!this.busy, e => e.cursor_pointer()).on_click(() => { if (!this.busy) closeOverlay(this.props.overlay); }).child("Close"))))
      .child(h_flex().h(42).flex_none().px(14).gap(12).items_center()
        .child(div().flex_1().text_color(t.muted).child(restriction || "Click changed lines or hunk headers to select them."))
        .when(!restriction, e => e.child(div().text_color(t.dim).child(`${this.selected.size} lines selected`))
          .when(this.props.group === "changes", e => e.child(div().id("diff-discard").px(10).py(6).rounded(4).text_color(enabled ? t.ansi1 : t.dim)
            .when(enabled, e => e.cursor_pointer()).on_click((_event, cx) => this.requestDiscard(cx)).child("Discard Selected…")))
          .child(div().id("diff-apply").px(12).py(6).rounded(4).bg(enabled ? t.accent : t.raised).text_color(enabled ? t.accent_foreground : t.dim)
            .when(enabled, e => e.cursor_pointer()).on_click((_event, cx) => this.apply(cx))
            .child(this.busy ? "Working…" : this.props.group === "staged" ? "Unstage Selected" : "Stage Selected"))))
      .when(this.confirmDiscard, e => e.child(h_flex().px(14).py(8).gap(10).items_center().bg(t.raised)
        .child(div().flex_1().child(`Discard working tree changes in ${this.selected.size} selected lines?`))
        .child(div().id("diff-discard-cancel").px(10).py(5).cursor_pointer().on_click((_event, cx) => { this.confirmDiscard = false; cx.notify(); }).child("Cancel"))
        .child(div().id("diff-discard-confirm").px(10).py(5).rounded(4).bg(t.ansi1).text_color(t.background).cursor_pointer().on_click((_event, cx) => this.apply(cx, true)).child("Discard Selected"))))
      .when(Boolean(this.error || this.note), e => e.child(div().px(14).py(7).text_color(this.error ? t.ansi1 : t.muted).child(this.error || this.note)))
      .child(div().flex_1().min_h(0).relative().font_family("JetBrains Mono").text_size(12)
        .child(div().id("source-diff-x").size_full().overflow_x_scrollbar()
          .child(div().min_w(this.width).w_full().h_full().child(v_virtual_list("source-diff-lines", this.rows.length, 22, index => String(index),
            range => this.rows.slice(range.start, range.end).map((row, offset) => this.renderRow(row, range.start + offset, t))).size_full())))
        .child(Scrollbar.vertical("source-diff-lines").viewport_from_layout().absolute().inset_0())
        .when(!this.rows.length, e => e.child(div().absolute().top(12).px(14).text_color(t.dim).child(this.busy ? "Loading diff…" : "No changes to display."))));
  }
}
