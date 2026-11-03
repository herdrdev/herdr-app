// A lazy file browser for the focused pane's folder, with bounded read-only previews.
import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex, Editor, EditorState } from "gpui-base";
import { spaces, panes, focused, exec, readTextFile, watchFolder, theme, settings, window, shared, setShared, props, openOverlay, closeOverlay, notify } from "herdr";
import { applications, openFile, pickApplication } from "./editor.js";

function location() {
  const space = spaces().find(s => s.active), f = focused();
  const pane = f?.pane ? panes().find(p => p.pane === f.pane) : null;
  const on = f?.pane === null ? (f.terminal?.includes("/") ? f.terminal.split("/")[0] : null) : space?.machine ?? null;
  return { cwd: pane?.cwd || f?.cwd || space?.folder, on, space: space?.id };
}
const key = () => JSON.stringify(location());
const LANG = { ts: "js", tsx: "js", mts: "js", cts: "js", jsx: "js", mjs: "js", toml: "yaml", jsonc: "json", svelte: "html", vue: "html" };
const PER_FOLDER = 500, TOTAL = 5000;
const fileType = path => path?.split(/[\\/]/).pop().match(/\.[^.]+$/)?.[0].toLowerCase() ?? "";

async function directory(path, on) {
  if (!on && window()?.platform === "windows") {
    // Windows PowerShell otherwise uses the console code page; exec decodes UTF-8.
    // Links keep directory attributes even when dangling; check their target.
    // Other reparse points (OneDrive's folders) have no LinkType and are folders.
    // PowerShell reads curly single quotes as quotes too, so they're doubled like '.
    const r = await exec("powershell", ["-NoProfile", "-Command", "[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); & { param($p) @(Get-ChildItem -LiteralPath $p -Force -ErrorAction Stop | Where-Object Name -ne '.git' | Select-Object -First 501 | ForEach-Object { @{ name=$_.Name; dir=($_.PSIsContainer -and (-not ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) -or -not $_.LinkType -or ($_.Target -and [IO.Directory]::Exists(([IO.Path]::Combine([IO.Path]::GetDirectoryName($_.FullName), $_.Target[0])))))) } }) | ConvertTo-Json -Compress } '" + path.replace(/['‘’‚‛]/g, "$&$&") + "'"]);
    if (r.code !== 0) throw new Error(r.stderr || "Cannot list this folder");
    const result = JSON.parse(r.stdout || "[]");
    return Array.isArray(result) ? result : [result];
  }
  const script = 'n=0; for p in "$1"/* "$1"/.[!.]* "$1"/..?*; do [ -e "$p" ] || [ -L "$p" ] || continue; name=${p##*/}; [ "$name" = .git ] && continue; if [ -d "$p" ]; then kind=d; else kind=f; fi; printf "%s\\0%s\\0" "$kind" "$name"; n=$((n+1)); [ "$n" -gt 500 ] && break; done; test -d "$1"';
  const r = await exec("sh", ["-c", script, "files", path], { on });
  if (r.code !== 0) throw new Error(r.stderr || "Cannot list this folder");
  const fields = r.stdout.split("\0"), out = [];
  for (let i = 0; i + 1 < fields.length; i += 2) out.push({ name: fields[i + 1], dir: fields[i] === "d" });
  return out;
}

export default class Files extends View {
  init(_props, cx) {
    if (props()?.openWith) {
      this.handle = cx.focus_handle(); this.handle.focus(); this.at = 0;
      this.loadApplications(props().file, cx);
      return;
    }
    this.saved = new Map();
    this.entries = new Map(); this.folders = new Set(); this.pending = new Set(); this.marks = {};
    this.refresh(cx);
    cx.timer.every(10000, cx => this.refresh(cx));
  }

  watch(root, cx) {
    if (this.on || this.watched === root) return;
    this.watched = root;
    const context = this.context;
    cx.spawn(async cx => {
      try { while (this.context === context) { await watchFolder(root); if (this.current(context)) this.refresh(cx); } }
      catch { if (this.context === context) this.watched = null; }
    });
  }

  current(context) {
    return this.context === context && key() === context.key;
  }

  async loadFolder(path, context) {
    const { on, cwd: root } = context;
    const entries = await directory(path ? `${root}/${path}` : root, on);
    if (!this.current(context) || (path && !this.folders.has(path))) return;
    const used = [...this.entries.entries()].filter(([p]) => p !== path).reduce((n, [, e]) => n + e.length, 0);
    this.entries.set(path, entries.slice(0, Math.max(0, Math.min(PER_FOLDER, TOTAL - used))).sort((a, b) => Number(b.dir) - Number(a.dir) || a.name.localeCompare(b.name)));
    if (entries.length > PER_FOLDER || used + entries.length > TOTAL) this.limited = true;
  }

  refresh(cx) {
    const { cwd, on } = location(), next = key();
    if (next !== this.context?.key) {
      if (this.context) this.saved.set(this.context.key, { folders: this.folders, file: this.file });
      const saved = this.saved.get(next);
      this.context = { key: next, cwd, on }; this.root = cwd; this.on = on; this.entries = new Map(); this.folders = new Set(saved?.folders);
      this.pending = new Set(); this.marks = {}; this.show(null); this.watched = null; this.error = null; this.limited = false;
      this.loading = false;
      if (saved?.file) this.show(saved.file, cx);
    }
    if (!cwd) return;
    if (this.file) this.loadFile(cx);
    if (this.loading) return;
    const context = this.context;
    this.loading = true;
    this.limited = false;
    cx.spawn(async cx => {
      try {
        for (const path of ["", ...this.folders]) {
          if (!this.current(context)) return;
          await this.loadFolder(path, context);
        }
        const top = await exec("git", ["rev-parse", "--show-toplevel"], { cwd, on }).catch(() => null);
        const marks = {};
        if (top?.code === 0) {
          // Writes nothing under .git (index lock, fsmonitor cookie): the folder watch would refresh again.
          const status = await exec("git", ["--no-optional-locks", "-c", "core.fsmonitor=false", "status", "--porcelain=v1", "-z", "--no-renames", "--untracked-files=all"], { cwd, on }).catch(() => null);
          if (status?.code === 0) for (const entry of status.stdout.split("\0").filter(Boolean).slice(0, TOTAL)) {
            const xy = entry.slice(0, 2);
            marks[`${top.stdout.trim()}/${entry.slice(3)}`] = xy === "??" || xy.includes("A") ? "A" : xy.includes("U") ? "U" : xy.includes("D") ? "D" : "M";
          }
        }
        if (!this.current(context)) return;
        this.marks = marks; this.error = null; this.watch(cwd, cx);
      } catch (e) { if (this.current(context)) this.error = String(e); }
      finally { if (this.context === context) this.loading = false; cx.notify(); }
    });
  }

  toggle(path, cx) {
    if (this.folders.has(path)) {
      for (const folder of [...this.folders]) if (folder === path || folder.startsWith(path + "/")) {
        this.folders.delete(folder);
        this.entries.delete(folder);
      }
    } else {
      this.folders.add(path);
      if (!this.entries.has(path) && !this.pending.has(path)) {
        const context = this.context;
        this.pending.add(path);
        cx.spawn(async cx => {
          try { await this.loadFolder(path, context); }
          catch (e) { if (this.current(context)) this.error = String(e); }
          finally { if (this.context === context) this.pending.delete(path); cx.notify(); }
        });
      }
    }
    cx.notify();
  }

  show(path, cx) {
    this.code?.release(); this.code = null; this.file = path; this.fileError = null;
    this.text = null; this.selection = {}; this.reading = null;
    this.applications = null;
    if (path && cx) {
      this.loadFile(cx);
      if (!this.on) this.loadApplications(`${this.root}/${path}`, cx);
    }
  }

  loadFile(cx) {
    if (this.reading) return;
    const context = this.context, path = this.file, root = this.root, on = this.on, selection = this.selection;
    const request = this.reading = {};
    cx.spawn(async cx => {
      try {
        const text = await readTextFile(`${root}/${path}`, { on }).catch(() => {
          throw new Error("Cannot preview this file (binary, too large, or unreadable)");
        });
        if (this.selection !== selection || !this.current(context)) return;
        if (text === null) throw new Error("This file is no longer available");
        if (text.includes("\0")) throw new Error("Binary file · no text preview");
        const value = text.length > 400000 ? text.slice(0, 400000) + "\n… (file truncated)" : text;
        if (value === this.text) return;
        const t = theme(), ext = path.includes(".") ? path.split(".").pop() : "txt";
        if (this.code) this.code.set_value(value);
        else this.code = EditorState.new({ language: LANG[ext] ?? ext, readonly: true,
          value,
          font_family: settings().font_family, font_size: 12,
          syntax: { keyword: t.ansi5, string: t.ansi2, comment: t.dim, function: t.ansi4, type: t.ansi3, number: t.ansi1, constant: t.ansi6 } });
        this.text = value; this.fileError = null;
      } catch (e) {
        if (this.selection === selection && this.current(context)) {
          this.code?.release(); this.code = null; this.text = null; this.fileError = e.message;
        }
      } finally { if (this.reading === request) this.reading = null; cx.notify(); }
    });
  }

  loadApplications(path, cx) {
    const selection = this.selection;
    this.appsLoading = true; this.appsError = null;
    cx.spawn(async cx => {
      try {
        const result = await applications(path);
        if (this.selection === selection) this.applications = result;
      } catch (e) { if (this.selection === selection) this.appsError = String(e.message ?? e); }
      finally { if (this.selection === selection) this.appsLoading = false; cx.notify(); }
    });
  }

  chooseApplication() {
    if (!this.file || this.on) return;
    openOverlay({ mod: "files", kind: "popup", width: 280, height: 350, props: { openWith: true, file: `${this.root}/${this.file}` } });
  }

  openInApplication(app, cx, remember = false, other = false) {
    if (this.opening || this.on) return;
    const p = props(), path = p?.openWith ? p.file : this.file && `${this.root}/${this.file}`;
    if (!path) return;
    this.opening = true;
    cx.spawn(async cx => {
      try {
        if (other) { app = await pickApplication(cx); if (!app) return; }
        await openFile(path, app);
        if (remember) {
          const choices = { ...shared("files.openWith") };
          if (app) choices[fileType(path)] = app; else delete choices[fileType(path)];
          setShared("files.openWith", choices);
        }
        if (p?.openWith) closeOverlay(p.overlay);
      } catch (e) { notify({ title: "Could not open file", body: String(e.message ?? e), level: "error" }); }
      finally { this.opening = false; cx.notify(); }
    });
  }

  applicationMenu(t) {
    const p = props(), selected = shared("files.openWith")?.[fileType(p.file)];
    const apps = this.applications?.apps ?? [];
    const choices = [{ name: this.applications?.defaultName ? `${this.applications.defaultName} (system default)` : "System default", app: null },
      ...apps.filter(app => app.id !== this.applications.defaultId).map(app => ({ name: app.name, app }))];
    if (selected && !apps.some(app => app.id === selected.id)) choices.push({ name: selected.name, app: selected });
    choices.push({ name: "Other…", app: null, other: true });
    this.at = Math.min(this.at, choices.length - 1);
    const choose = (choice, cx) => this.openInApplication(choice.app, cx, true, choice.other);
    const row = (choice, i) => h_flex().id(`files-application-${i}`).h(30).flex_none().px(8).gap(8).items_center().rounded(5)
      .bg(i === this.at ? t.hover : t.popover).cursor_pointer().hover(s => s.bg(t.hover)).opacity(this.opening ? 0.5 : 1)
      .on_click((_e, cx) => choose(choice, cx))
      .child(div().w(14).flex_none().child(!choice.other && (choice.app ? selected?.id === choice.app.id : !selected) ? "✓" : ""))
      .child(div().flex_1().min_w(0).truncate().child(choice.name));
    return v_flex().id("files-open-with").w_full().h(350).p(6).gap(4).bg(t.popover).text_color(t.foreground).text_size(13)
      .track_focus(this.handle).on_key_down((e, cx) => {
        if (e.key === "escape") closeOverlay(p.overlay);
        else if (e.key === "up") this.at = (this.at + choices.length - 1) % choices.length;
        else if (e.key === "down") this.at = (this.at + 1) % choices.length;
        else if (e.key === "enter") choose(choices[this.at], cx);
        else return;
        cx.stop_propagation(); cx.notify();
      })
      .child(div().px(8).py(4).font_semibold().child("Open with"))
      .child(v_flex().id("files-applications").flex_1().min_h(0).overflow_y_scrollbar()
        .when(this.appsLoading || Boolean(this.appsError), el => el.child(div().p(8).text_size(11).text_color(t.dim).child(this.appsError || "Finding applications…")))
        .children(choices.slice(0, -1).map(row)))
      .child(row(choices[choices.length - 1], choices.length - 1))
      .child(div().px(8).py(4).text_size(11).text_color(t.dim).child(`Remembers your choice for ${fileType(p.file) || "extensionless"} files in Herdr.`));
  }

  rows(prefix, depth, out, t) {
    if (out.length >= TOTAL || depth > 40) return out;
    for (const entry of this.entries.get(prefix) ?? []) {
      if (out.length >= TOTAL) break;
      const path = prefix ? `${prefix}/${entry.name}` : entry.name, open = this.folders.has(path), mark = this.marks[`${this.root}/${path}`];
      out.push(h_flex().id(`file-${path}`).h(28).flex_none().pl(8 + depth * 18).pr(8).gap(7).items_center().rounded(7).cursor_pointer().hover(s => s.bg(t.hover))
        .on_click((_e, cx) => entry.dir ? this.toggle(path, cx) : this.show(path, cx))
        .child(entry.dir ? svg(`mods/files/chevron-${open ? "down" : "right"}.svg`).size(12).flex_none().text_color(t.dim) : div().w(12).flex_none())
        .child(svg(`mods/files/${entry.dir ? "folder" : "file"}.svg`).size(14).flex_none().text_color(t.muted))
        .child(div().flex_1().min_w(0).truncate().child(entry.name))
        .when(Boolean(mark), el => el.child(div().text_size(11).text_color(mark === "A" ? t.ansi2 : mark === "D" || mark === "U" ? t.ansi1 : t.blocked).child(mark))));
      if (entry.dir && open) this.rows(path, depth + 1, out, t);
    }
    return out;
  }

  render(cx) {
    if (props()?.openWith) return this.applicationMenu(theme());
    if (key() !== this.context?.key) this.refresh(cx);
    const t = theme(), app = shared("files.openWith")?.[fileType(this.file)], appName = app?.name ?? this.applications?.defaultName;
    return v_flex().id("files").size_full().min_h(0).rounded(10).overflow_hidden().bg(t.background).border_1().border_color(t.border).text_color(t.foreground).text_size(13)
      .child(h_flex().h(42).flex_none().px(14).gap(8).items_center().border_b(1).border_color(t.border)
        .when(Boolean(this.file), el => el.child(div().id("files-back").w(20).cursor_pointer().text_color(t.muted).on_click((_e, cx) => { this.show(null); cx.notify(); }).child("←")))
        .child(div().font_semibold().child("Files"))
        .child(div().flex_1().min_w(0).truncate().text_right().text_size(12).text_color(t.dim).child(this.file ?? this.root?.split(/[\\/]/).pop() ?? ""))
        .when(Boolean(this.file) && !this.on, el => el.child(h_flex().flex_none().gap(2).text_size(11).text_color(t.muted)
          .child(div().id("files-open-editor").px(5).py(3).rounded(5).cursor_pointer().hover(s => s.bg(t.hover).text_color(t.foreground))
            .max_w(140).truncate().tooltip(appName ? `Open in ${appName}` : "Open with the system's default app").on_click((_e, cx) => this.openInApplication(app ?? null, cx))
            .child(this.opening ? "Opening…" : appName ? `Open in ${appName}` : "Open"))
          .child(div().id("files-choose-application").px(3).py(3).rounded(5).cursor_pointer().tooltip("Open with…").hover(s => s.bg(t.hover).text_color(t.foreground))
            .on_click(() => this.chooseApplication()).child(svg("mods/files/chevron-down.svg").size(12))))))
      .when(!this.file, el => el.child(v_flex().id("files-tree").flex_1().min_h(0).overflow_y_scrollbar().p(6).children(this.rows("", 0, [], t))
        .when(Boolean(this.error) || !this.entries.get("")?.length, el => el.child(div().p(8).text_size(12).text_color(t.dim).child(this.error || (this.loading ? "Loading files…" : this.root ? "No files" : "No folder"))))
        .when(this.limited, el => el.child(div().p(8).text_size(11).text_color(t.dim).child("File list limited to 500 entries per folder and 5,000 total")))))
      .when(Boolean(this.file), el => el.child(this.code ? Editor.new(this.code).flex_1().min_h(0) : div().p(12).text_color(t.dim).child(this.fileError || "Loading file…")));
  }
}
