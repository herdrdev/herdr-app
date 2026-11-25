import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input, Scrollbar, v_virtual_list } from "gpui-base";
import { spaces, panes, focused, exec, readTextFile, watchFolder, theme, settings, window } from "herdr";

function location() {
  const space = spaces().find((s) => s.active), f = focused();
  const pane = f?.pane ? panes().find(p => p.pane === f.pane) : null;
  const on = f?.pane === null ? (f.terminal?.includes("/") ? f.terminal.split("/")[0] : null) : space?.machine ?? null;
  return { cwd: pane?.cwd || f?.cwd || space?.folder, on };
}
const key = () => JSON.stringify(location());
// Reads write nothing under .git (no index refresh or lock, no fsmonitor cookie):
// the folder watch would see it, and every refresh would start the next one.
const READ = ["--no-optional-locks", "-c", "core.fsmonitor=false", "-c", "diff.autoRefreshIndex=false"];
const STATUS_AT_ONCE = 4;

async function worktree(cwd, on, child) {
  const args = ["rev-parse", "--is-inside-work-tree", "--show-cdup", "--show-toplevel"];
  if (child) args.unshift("-C", `./${child}`);
  const top = await exec("git", args, { cwd, on });
  if (top.code !== 0) return null;
  // A child must be at its own worktree root. core.worktree can point Git elsewhere.
  // Read the line ending from Git's fixed first field, not from the path.
  const probe = /^true(\r?\n)((?:\.\.\/)*)\1/.exec(top.stdout);
  if (!probe || (child && probe[2]) || !top.stdout.endsWith(probe[1])) return null;
  return top.stdout.slice(probe[0].length, -probe[1].length) || null;
}

async function candidates(cwd, on, current) {
  const options = { cwd, on };
  const powershell = async () => {
    const script = `
      $ErrorActionPreference = 'Stop'
      [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
      $names = @(Get-ChildItem -LiteralPath . -Directory -Force | Where-Object {
        $_.Name -ne '.git' -and $_.LinkType -notin @('SymbolicLink', 'Junction') -and
        (Test-Path -LiteralPath (Join-Path $_.FullName '.git'))
      } | ForEach-Object Name)
      ConvertTo-Json -InputObject $names -Compress`;
    const result = await exec("powershell", ["-NoProfile", "-Command", script], options);
    if (result.code !== 0) throw new Error(result.stderr || "Cannot list this folder");
    return JSON.parse(result.stdout);
  };
  if (!on && window()?.platform === "windows") return powershell();

  const script = `
    if ! [ -r . ] || ! [ -x . ]; then
      printf '%s\\n' 'Cannot list this folder' >&2
      exit 1
    fi
    for p in ./* ./.[!.]* ./..?*; do
      [ "$p" != ./.git ] && [ -d "$p" ] && [ ! -L "$p" ] || continue
      if [ -d "$p/.git" ] || [ -f "$p/.git" ]; then
        printf '%s\\0' "\${p#./}"
      fi
    done`;
  let result;
  try { result = await exec("sh", ["-c", script], options); }
  catch (e) {
    if (on && String(e).includes("cannot start sh: program not found") && current()) return powershell();
    throw e;
  }
  if (result.code !== 0) throw new Error(result.stderr || "Cannot list this folder");
  return result.stdout.split("\0").filter(Boolean);
}

async function discover(cwd, on, previous, current) {
  const top = await worktree(cwd, on);
  if (!current()) return null;
  if (top) return { grouped: false, roots: [{ root: top, name: top.slice(top.lastIndexOf("/") + 1) || top }] };
  const known = new Map();
  for (const repo of previous) known.set(repo.name, repo.root);
  const roots = [], seen = new Set();
  for (const name of (await candidates(cwd, on, current)).sort()) {
    if (!current()) return null;
    let root, probeError;
    try { root = await worktree(cwd, on, name); }
    catch (e) {
      // A probe that failed to run does not mean a listed checkout vanished.
      root = known.get(name);
      probeError = String(e);
    }
    if (!root || seen.has(root)) continue;
    seen.add(root);
    roots.push({ root, name, probeError });
  }
  return { grouped: true, roots };
}

const countLines = (text) => text ? text.split("\n").length - Number(text.endsWith("\n")) : 0;
const wideGlyph = /[\p{Ideographic}\p{Emoji_Presentation}\p{Extended_Pictographic}\u1100-\u115f\u2e80-\ua4cf\uac00-\ud7a3\ufe10-\ufe19\ufe30-\ufe6f\uff01-\uff60\uffe0-\uffe6]/gu;

// Keep room for spacing marks and give wide glyphs at least two columns.
function displayColumns(text) {
  let columns = text.length;
  for (const glyph of text.match(wideGlyph) ?? []) {
    if (glyph.length === 1) columns++;
  }
  return columns;
}

// The diff was laid out for 11px text: 20px rows, 7px characters.
const rowHeight = size => Math.ceil(size * 1.8), charWidth = size => size * 0.64;

// Text draws a tab as nothing: pad each to the next 4-column stop.
const untab = (text) => text.replace(/[^\t]*\t/g, (s) => s.slice(0, -1).padEnd((s.length - 1) - (s.length - 1) % 4 + 4));

// Numbers each diff line once, so a frame only draws the rows on screen.
function diffRows(lines) {
  let old = 0, next = 0;
  return lines.map((line) => {
    const match = /^@@ -(\d+)(?:,\d+)? \+(\d+)/.exec(line);
    if (match) { old = +match[1]; next = +match[2]; return { kind: "@", text: line }; }
    const kind = line[0];
    if (kind === "-") return { kind, number: old++, text: untab(line.slice(1)) || " " };
    if (kind === "+") return { kind, number: next++, text: untab(line.slice(1)) || " " };
    if (kind === " ") { old++; return { kind, number: next++, text: untab(line.slice(1)) || " " }; }
    return { kind: "", text: line };
  });
}

export default class Changes extends View {
  init(_props, cx) {
    this.rows = []; this.longest = 0; this.gutterChars = 4; this.handle = cx.focus_handle();
    this.input = InputState.new({ placeholder: "Commit message" });
    this.input.on("change", (_e, cx) => {
      const repo = this.active;
      if (!repo || !this.currentRepo(repo, this.context)) return;
      const hadMessage = Boolean(repo.message.trim());
      repo.message = this.input.value();
      if (hadMessage !== Boolean(repo.message.trim())) cx.notify();
    });
    this.input.on("submit", (_e, cx) => this.commit(this.active, cx));
    this.input.on("focus", (_e, cx) => { this.typing = true; cx.notify(); });
    this.input.on("blur", (_e, cx) => { this.typing = false; cx.notify(); });
    this.refresh(cx);
    cx.timer.every(10000, (cx) => this.refresh(cx));
  }

  watch(repo, cx) {
    if (this.watched?.repo === repo) return;
    this.watched?.pending?.cancel?.();
    this.watched = null;
    if (!repo) return;
    const watch = this.watched = { repo };
    cx.spawn(async (cx) => {
      try {
        while (this.watched === watch) {
          watch.pending = watchFolder(repo.root);
          await watch.pending;
          if (this.watched === watch) this.refresh(cx);
        }
      } catch {
        if (this.watched === watch) this.watched = null;
      }
    });
  }

  // One visit to a folder: results from an earlier visit to the same folder are stale too.
  current(context) {
    return this.context === context && key() === context.key;
  }

  currentRepo(repo, context) {
    return this.current(context) && context.repos.get(repo.root) === repo;
  }

  clearPreview() {
    this.previewRequest = null;
    this.setLines([]);
  }

  activate(repo) {
    if (this.active === repo) return false;
    this.active = repo;
    if (!repo) this.typing = false;
    this.clearPreview();
    this.input.set_value(repo?.message ?? "");
    return true;
  }

  setRepos(roots, context, cx) {
    const repos = new Map();
    for (const { root, name, probeError } of roots) {
      const repo = context.repos.get(root) ?? { root, files: [], expanded: true, open: null, closed: false, message: "" };
      repo.name = name;
      repo.probeError = probeError;
      repos.set(root, repo);
    }
    context.repos = repos;
    const active = repos.get(this.active?.root) ?? repos.values().next().value ?? null;
    if (this.activate(active)) this.preview(active, cx);
  }

  refresh(cx) {
    const at = location(), next = JSON.stringify(at), { cwd, on } = at;
    if (next !== this.context?.key) {
      this.watch(null, cx);
      this.context = { key: next, on, repos: new Map(), pending: new Set(), checking: 0 };
      this.activate(null);
      this.error = null;
      if (!cwd) cx.notify();
    }
    const context = this.context;
    if (!cwd || context.discovering) return;
    cx.spawn(async (cx) => {
      context.discovering = true;
      try {
        const previous = context.grouped ? context.repos.values() : [];
        const found = await discover(cwd, on, previous, () => this.current(context));
        if (!found || !this.current(context)) return;
        const { grouped, roots } = found;
        context.grouped = grouped;
        this.setRepos(roots, context, cx);
        this.error = roots.length ? null : "Not a git repository";
        this.watch(grouped || on ? null : this.active, cx);
        for (const repo of context.repos.values()) {
          if (repo.probeError || repo.loading || this.busy === repo) continue;
          repo.loading = true;
          context.pending.add(repo);
        }
      } catch (e) {
        if (this.current(context)) this.error = String(e);
      } finally {
        context.discovering = false;
        if (this.current(context)) cx.notify();
      }
      await this.checkRepos(context, cx);
    });
  }

  // Bound the Git work per visit without a GPUI task for each repo. A new
  // discovery can add to this queue while earlier status reads are pending.
  async checkRepos(context, cx) {
    const { pending } = context;
    await Promise.all(Array.from({ length: Math.min(STATUS_AT_ONCE - context.checking, pending.size) }, async () => {
      context.checking++;
      try {
        for (const repo of pending) {
          if (!this.current(context)) return;
          pending.delete(repo);
          if (context.repos.get(repo.root) === repo) await this.status(repo, context, cx);
        }
      } finally { context.checking--; }
    }));
  }

  async status(repo, context, cx) {
    const git = (args) => exec("git", [...READ, ...args], { cwd: repo.root, on: context.on });
    try {
      if (repo.probeError) return;
      const status = await git(["status", "--porcelain=v1", "-z", "--no-renames", "--untracked-files=all"]);
      if (status.code !== 0) throw new Error(status.stderr);
      if (!this.currentRepo(repo, context)) return;
      const head = await git(["rev-parse", "--verify", "HEAD"]);
      const stats = {};
      const results = head.code === 0 ? [await git(["diff", "HEAD", "--numstat", "-z", "--no-renames"])] : [await git(["diff", "--cached", "--numstat", "-z", "--no-renames"]), await git(["diff", "--numstat", "-z", "--no-renames"])];
      for (const result of results) {
        if (result.code !== 0) throw new Error(result.stderr);
        for (const entry of result.stdout.split("\0")) {
          const [a, d, ...parts] = entry.split("\t");
          if (!parts.length) continue;
          const path = parts.join("\t"), old = stats[path] ?? { add: 0, del: 0 };
          stats[path] = { add: old.add + (Number(a) || 0), del: old.del + (Number(d) || 0), binary: a === "-" };
        }
      }
      const files = [];
      const entries = status.stdout.split("\0").filter(Boolean);
      for (const entry of entries.slice(0, 1000)) {
        const xy = entry.slice(0, 2), path = entry.slice(3), fresh = xy === "??";
        const conflict = xy.includes("U") || xy === "AA" || xy === "DD";
        const s = conflict ? "U" : xy.includes("D") ? "D" : fresh || xy.includes("A") ? "A" : "M";
        const stat = stats[path] ?? { add: 0, del: 0 };
        files.push({ path, status: s, fresh, ...stat });
      }
      if (!this.currentRepo(repo, context)) return;
      repo.hasHead = head.code === 0; repo.files = files; repo.count = entries.length; repo.error = null;
      const previous = repo.open;
      if (previous && !files.some(f => f.path === previous)) repo.open = null;
      if (!repo.open && !repo.closed && files.length) repo.open = files[0].path;
      if (this.active === repo) {
        if (previous !== repo.open) this.clearPreview();
        this.preview(repo, cx);
      }
    } catch (e) {
      if (this.currentRepo(repo, context)) repo.error = String(e);
    } finally {
      repo.loading = false;
      if (this.currentRepo(repo, context)) cx.notify();
    }
  }

  preview(repo, cx) {
    const path = repo?.open, context = this.context;
    if (!path) return;
    const file = repo.files.find(f => f.path === path);
    if (!file) return;
    const { hasHead } = repo;
    const source = file.fresh || (!hasHead && file.status === "A") ? "file" : hasHead ? "HEAD" : "index";
    // Keep a slow preview only while status still calls for the same source.
    if (this.previewRequest?.source === source) return;
    if (this.previewRequest) this.clearPreview();
    const request = this.previewRequest = { source };
    try {
      cx.spawn(async (cx) => { if (await this.load(repo, file, context, request)) cx.notify(); });
    } catch (e) {
      this.previewRequest = null;
      throw e;
    }
  }

  async load(repo, file, context, request) {
    if (this.previewRequest !== request) return false;
    if (!this.currentRepo(repo, context)) {
      this.previewRequest = null;
      return false;
    }
    const { root } = repo, { on } = context, { path } = file, { source } = request;
    let lines, add;
    try {
      if (source === "file") {
        const text = await readTextFile(`${root}/${path}`, { on }).catch(() => null);
        if (text === null) lines = ["Cannot preview this file (binary, too large, or unreadable)"];
        else if (text.includes("\0")) lines = ["Binary file · no text diff"];
        else {
          add = countLines(text);
          lines = [`@@ -0,0 +1,${add} @@`, ...text.slice(0, 400000).replace(/\n$/, "").split("\n").slice(0, 1500).map(l => `+${l}`)];
        }
      } else {
        const r = await exec("git", [...READ, "diff", ...(source === "HEAD" ? ["HEAD"] : []), "--no-ext-diff", "--no-textconv", "--no-color", "--", path], { cwd: root, on });
        if (r.code !== 0) throw new Error(r.stderr);
        const all = r.stdout.replace(/\n$/, "").split("\n"), first = all.findIndex(l => l.startsWith("@@"));
        lines = first < 0 ? [file.binary ? "Binary file changed" : "No text diff available"] : all.slice(first);
      }
    } catch { lines = ["Cannot preview this diff (too large or unreadable)"]; }
    if (this.previewRequest !== request) return false;
    this.previewRequest = null;
    if (this.active !== repo || repo.open !== path || !this.currentRepo(repo, context)) return false;
    if (add !== undefined) repo.files.find(f => f.path === path).add = add;
    this.setLines(lines.slice(0, 1500).map(line => line.length > 4000 ? line.slice(0, 4000) + "… (line truncated)" : line));
    return true;
  }

  setLines(lines) {
    this.rows = diffRows(lines);
    this.longest = Math.max(0, ...this.rows.map(row => displayColumns(row.text)));
    this.gutterChars = Math.max(4, String(Math.max(0, ...this.rows.map(row => row.number ?? 0))).length);
  }

  pick(repo, path, cx) {
    if (!this.currentRepo(repo, this.context) || !repo.files.some(f => f.path === path)) return;
    const switched = this.activate(repo);
    repo.open = !switched && repo.open === path ? null : path;
    repo.closed = !repo.open;
    if (!switched) this.clearPreview();
    this.preview(repo, cx);
    cx.notify();
  }

  canCommit(repo) {
    return Boolean(repo && repo === this.active && !this.busy && !this.context.discovering && !this.error && !repo.loading && !repo.probeError && !repo.error && repo.message.trim());
  }

  commit(repo, cx) {
    const context = this.context;
    if (!this.canCommit(repo) || !this.currentRepo(repo, context)) return;
    const options = { cwd: repo.root, on: context.on }, message = repo.message.trim();
    this.busy = repo; repo.note = null;
    cx.spawn(async (cx) => {
      try {
        for (const args of [["add", "-A"], ["commit", "-m", message]]) {
          const r = await exec("git", args, options);
          if (r.code !== 0) throw new Error((r.stderr || r.stdout).trim());
        }
        if (this.currentRepo(repo, context)) {
          repo.message = "";
          if (this.active === repo) this.input.set_value("");
        }
      } catch (e) {
        if (this.currentRepo(repo, context)) repo.note = `Commit failed: ${e}`;
      } finally {
        this.busy = null;
        this.refresh(cx);
        cx.notify();
      }
    });
    cx.notify();
  }

  gutter(size) { return Math.ceil(this.gutterChars * charWidth(size)) + 2; }

  diffRow({ kind, number, text }, t, size) {
    const height = rowHeight(size);
    if (kind === "@") return div().w_full().px(12).h(height).flex_none().truncate().bg(t.raised).text_color(t.dim).child(text);
    return h_flex().w_full().h(height).flex_none().gap(10).when(kind === "+", (e) => e.bg(t.ansi2 + "18")).when(kind === "-", (e) => e.bg(t.ansi1 + "18"))
      .child(div().w(this.gutter(size)).flex_none().text_right().text_color(t.dim).child(number === undefined ? "" : String(number)))
      .child(div().flex_1().min_w(0).truncate().child(text));
  }

  toggleRepo(repo, cx) {
    if (!this.currentRepo(repo, this.context)) return;
    repo.expanded = !repo.expanded;
    if (this.activate(repo)) this.preview(repo, cx);
    cx.notify();
  }

  changeList(t, context, cwd) {
    const grouped = context?.grouped;
    const rows = [];
    let count = 0, add = 0, del = 0;
    const notice = (text, color = t.dim) => rows.push(h_flex().h(28).flex_none().items_center().pl(grouped ? 28 : 8).pr(8).text_size(11).text_color(color)
      .child(div().min_w(0).truncate().tooltip(text).child(text)));
    if (!context?.repos.size || this.error) notice((context && this.error) || (cwd ? "Loading changes…" : "No folder"));
    for (const repo of context?.repos.values() ?? []) {
      count += repo.count ?? 0;
      const expanded = !grouped || repo.expanded;
      if (grouped) {
        rows.push(h_flex().id(`changes-repo-${repo.root}`).h(28).flex_none().px(8).gap(8).items_center().rounded(7).cursor_pointer()
          .when(this.active === repo, (e) => e.bg(t.selected)).hover((s) => s.bg(t.hover)).on_click((_e, cx) => this.toggleRepo(repo, cx))
          .child(div().w(12).flex_none().text_color(t.dim).child(expanded ? "▾" : "▸"))
          .child(div().flex_1().min_w(0).truncate().tooltip(repo.root).font_semibold().child(repo.name))
          .child(div().flex_none().text_size(11).text_color(t.muted).child(`${repo.count ?? 0} files`)));
      }
      if (expanded) {
        const error = repo.probeError ?? repo.error;
        if (error) notice(error, t.ansi1);
        else if (!repo.files.length) notice(repo.loading ? "Loading changes…" : "No uncommitted changes");
        if (grouped && repo.files.length < repo.count) notice("Showing the first 1,000 changed files");
      }
      for (const f of repo.files) {
        add += f.add;
        del += f.del;
        if (!expanded) continue;
        const parts = f.path.split("/"), name = parts.pop();
        rows.push(h_flex().id(`change-${JSON.stringify([repo.root, f.path])}`).h(28).flex_none().pl(grouped ? 28 : 8).pr(8).gap(8).items_center().rounded(7).cursor_pointer()
          .when(this.active === repo && repo.open === f.path, (e) => e.bg(t.selected)).hover((s) => s.bg(t.hover)).on_click((_e, cx) => this.pick(repo, f.path, cx))
          .child(div().w(14).text_size(11).text_color(f.status === "A" ? t.ansi2 : f.status === "D" ? t.ansi1 : f.status === "U" ? t.ansi1 : t.blocked).child(f.status))
          .child(h_flex().flex_1().min_w(0).child(div().text_color(t.dim).truncate().child(parts.length ? parts.join("/") + "/" : "")).child(div().max_w(260).min_w(0).truncate().child(name)))
          .when(f.add > 0, (e) => e.child(div().text_size(11).text_color(t.ansi2).child(`+${f.add}`))).when(f.del > 0, (e) => e.child(div().text_size(11).text_color(t.ansi1).child(`−${f.del}`))));
      }
    }
    return { rows, count, add, del };
  }

  render(cx) {
    const at = location(), context = JSON.stringify(at) === this.context?.key ? this.context : null;
    if (!context && !this.refreshQueued) {
      // InputState and notify can only be updated outside the render phase.
      cx.timer.after(0, (cx) => {
        this.refreshQueued = false;
        if (key() !== this.context?.key) this.refresh(cx);
      });
      this.refreshQueued = true;
    }
    const t = theme(), size = settings().font_size, repo = context ? this.active : null, grouped = context?.grouped;
    const { rows, count, add, del } = this.changeList(t, context, at.cwd);
    const preview = context ? this.rows : [];
    const enabled = this.canCommit(repo);
    const committing = this.busy && this.busy === repo;
    const commitLabel = grouped && repo ? `${committing ? "Committing to" : "Commit to"} ${repo.name}${committing ? "…" : ""}` : this.busy ? "Committing…" : "Commit";
    return v_flex().id("changes").size_full().min_h(0).rounded(10).overflow_hidden().bg(t.background).border_1().border_color(t.border).text_color(t.foreground).text_size(13).track_focus(this.handle)
      .on_key_down((e, cx) => {
        if (e.key === "escape" && this.typing) { this.typing = false; this.handle.focus(); cx.stop_propagation(); cx.notify(); }
      })
      .child(h_flex().h(42).flex_none().px(14).gap(8).items_center().border_b(1).border_color(t.border)
        .child(div().font_semibold().child("Changes"))
        .child(div().px(5).py(1).rounded(4).border_1().border_color(t.border).text_size(10).text_color(t.muted).child(`${count} files`))
        .child(div().flex_1()).child(div().text_size(11).text_color(t.ansi2).child(`+${add}`)).child(div().text_size(11).text_color(t.ansi1).child(`−${del}`)))
      // overflow_y_scrollbar() wraps the list, so its flex sizing never reached the panel's layout.
      .child(div().flex_1().min_h(44).max_h(Math.max(44, rows.length * 28 + 12)).relative().border_b(1).border_color(t.border)
        .child(v_flex().id("changes-list").absolute().inset_0().overflow_y_scroll().p(6).children(rows))
        .child(Scrollbar.vertical("changes-list").absolute().inset_0()))
      .when(!grouped && repo?.files.length < repo?.count, el => el.child(div().px(12).py(4).text_size(11).text_color(t.dim).child("Showing the first 1,000 changed files")))
      .child(div().flex_1().min_h(0).relative().font_family(settings().font_family ?? "JetBrains Mono").text_size(size)
        .child(div().id("changes-diff-x").size_full().overflow_x_scrollbar()
          .child(div().w_full().min_w(Math.max(300, (context ? this.longest : 0) * charWidth(size) + this.gutter(size) + 20)).h_full().child(v_virtual_list("changes-diff", preview.length, rowHeight(size), (i) => String(i),
            (range) => preview.slice(range.start, range.end).map((row) => this.diffRow(row, t, size))).size_full())))
        .child(Scrollbar.vertical("changes-diff").viewport_from_layout().absolute().inset_0())
        .when(!repo?.open && repo?.files.length > 0, (e) => e.child(div().absolute().top(6).px(12).text_size(11).text_color(t.dim).child("Select a file to view its changes"))))
      .when(Boolean(repo?.note), (e) => e.child(div().px(10).pb(8).text_size(11).text_color(t.ansi1).child(repo.note)))
      .child(h_flex().flex_none().p(10).gap(8).border_t(1).border_color(t.border)
        .child(div().id("commit-message").flex_1().min_w(0).h(32).px(10).flex().items_center().rounded(8).border_1().border_color(repo && this.typing ? t.accent : t.border)
          .when(Boolean(repo), (e) => e.cursor_text().on_mouse_down("left", (_e, cx) => cx.stop_propagation()).child(Input.new(this.input).h_full().text_size(12).text_color(t.foreground))))
        .child(div().id("commit").h(32).min_w(0).px(14).flex().items_center().rounded(8).bg(enabled ? t.solid : t.raised).text_color(enabled ? t.solid_foreground : t.dim).text_size(12)
          .when(grouped, (e) => e.max_w("60%").tooltip(commitLabel)).when(enabled, (e) => e.cursor_pointer()).on_click((_e, cx) => this.commit(repo, cx))
          .child(div().min_w(0).truncate().child(commitLabel))));
  }
}
