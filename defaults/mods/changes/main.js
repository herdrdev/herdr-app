import { View, div, svg } from "gpui-kit";
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
const countLines = (text) => text ? text.split("\n").length - Number(text.endsWith("\n")) : 0;

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

const repoCache = new Map();

// Discovers git repositories: either the folder itself or nested repositories inside it.
async function findGitRepos(root, on) {
  if (!root) return [];
  const cacheKey = `${on || 'local'}:${root}`;
  const hit = repoCache.get(cacheKey);
  if (hit && (Date.now() - hit.time < 60000)) {
    return hit.repos;
  }

  // 1. Check if the root folder itself is a git repository or inside one
  const top = await exec("git", ["rev-parse", "--show-toplevel"], { cwd: root, on }).catch(() => null);
  if (top && top.code === 0 && top.stdout && top.stdout.trim()) {
    const repos = [top.stdout.trim().replace(/\\/g, "/")];
    repoCache.set(cacheKey, { repos, time: Date.now() });
    return repos;
  }

  // 2. Discover nested git repositories up to depth 2 or 3
  let repos = [];
  if (!on && window()?.platform === "windows") {
    try {
      const script = "[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); & { param($p) @(Get-ChildItem -LiteralPath $p -Depth 2 -Directory -Hidden -Filter '.git' -ErrorAction SilentlyContinue | Where-Object { $_.FullName -notmatch '[\\\\/](\\.cache|node_modules|vendor)[\\\\/]' } | ForEach-Object { $_.Parent.FullName.Replace('\\', '/') }) | ConvertTo-Json -Compress } '" + root.replace(/['‘’‚‛]/g, "$&$&") + "'";
      const r = await exec("powershell", ["-NoProfile", "-Command", script]).catch(() => null);
      if (r && r.code === 0 && r.stdout && r.stdout.trim()) {
        const parsed = JSON.parse(r.stdout.trim());
        const list = Array.isArray(parsed) ? parsed : [parsed];
        repos = [...new Set(list.filter(Boolean))];
      }
    } catch (_) {}
  } else {
    try {
      const script = 'find "$1" -maxdepth 3 -name .git -type d -not -path "*/.*/*" -not -path "*/node_modules/*" 2>/dev/null | while read -r p; do dirname "$p"; done';
      const r = await exec("sh", ["-c", script, "repos", root], { on }).catch(() => null);
      if (r && r.code === 0 && r.stdout && r.stdout.trim()) {
        const list = r.stdout.trim().split("\n").map(s => s.trim()).filter(Boolean);
        repos = [...new Set(list)];
      }
    } catch (_) {}
  }

  repoCache.set(cacheKey, { repos, time: Date.now() });
  return repos;
}

async function loadRepoChanges(repoRoot, on) {
  const git = (args) => exec("git", [...READ, ...args], { cwd: repoRoot, on });
  const status = await git(["status", "--porcelain=v1", "-z", "--no-renames", "--untracked-files=all"]).catch(() => null);
  if (!status || status.code !== 0) return null;

  const entries = status.stdout.split("\0").filter(Boolean);
  const name = repoRoot.split("/").pop() || repoRoot;

  // Fast path for clean repositories: skip diffs completely to save SSH/CPU cycles
  if (entries.length === 0) {
    return {
      root: repoRoot,
      name,
      branch: "",
      hasHead: true,
      files: [],
      limited: false,
      add: 0,
      del: 0,
    };
  }

  const head = await git(["rev-parse", "--verify", "HEAD"]).catch(() => null);
  const branchRes = await git(["rev-parse", "--abbrev-ref", "HEAD"]).catch(() => null);
  const branch = branchRes?.code === 0 ? branchRes.stdout.trim() : "";

  const stats = {};
  const results = (head && head.code === 0)
    ? [await git(["diff", "HEAD", "--numstat", "-z", "--no-renames"]).catch(() => null)]
    : [await git(["diff", "--cached", "--numstat", "-z", "--no-renames"]).catch(() => null), await git(["diff", "--numstat", "-z", "--no-renames"]).catch(() => null)];

  for (const result of results) {
    if (!result || result.code !== 0) continue;
    for (const entry of result.stdout.split("\0")) {
      const [a, d, ...parts] = entry.split("\t");
      if (!parts.length) continue;
      const path = parts.join("\t"), old = stats[path] ?? { add: 0, del: 0 };
      stats[path] = { add: old.add + (Number(a) || 0), del: old.del + (Number(d) || 0), binary: a === "-" };
    }
  }

  const files = [];
  for (const entry of entries.slice(0, 1000)) {
    const xy = entry.slice(0, 2), path = entry.slice(3), fresh = xy === "??";
    const conflict = xy.includes("U") || xy === "AA" || xy === "DD";
    const s = conflict ? "U" : xy.includes("D") ? "D" : fresh || xy.includes("A") ? "A" : "M";
    const stat = stats[path] ?? { add: 0, del: 0 };
    files.push({ repo: repoRoot, repoName: name, branch, path, status: s, fresh, ...stat });
  }

  return {
    root: repoRoot,
    name,
    branch,
    hasHead: head?.code === 0,
    files,
    limited: entries.length > 1000,
    add: files.reduce((n, f) => n + f.add, 0),
    del: files.reduce((n, f) => n + f.del, 0),
  };
}

export default class Changes extends View {
  init(_props, cx) {
    this.repos = [];
    this.allFiles = [];
    this.collapsedRepos = new Set();
    this.userToggled = new Set();
    this.selectedRepo = null;
    this.rows = [];
    this.diffWidth = 0;
    this.message = "";
    this.handle = cx.focus_handle();
    this.input = InputState.new({ placeholder: "Commit message" });
    this.input.on("change", (_e, cx) => {
      const hadMessage = Boolean(this.message.trim());
      this.message = this.input.value();
      if (hadMessage !== Boolean(this.message.trim())) cx.notify();
    });
    this.input.on("submit", (_e, cx) => this.commit(cx));
    this.input.on("focus", (_e, cx) => { this.typing = true; cx.notify(); });
    this.input.on("blur", (_e, cx) => { this.typing = false; cx.notify(); });
    this.refresh(cx);
    cx.timer.every(25000, (cx) => this.refresh(cx));
  }

  watch(root, cx) {
    if (this.on || this.watched === root) return;
    this.watched = root;
    cx.spawn(async (cx) => {
      try {
        while (this.watched === root) {
          await watchFolder(root);
          if (this.watched === root) this.refresh(cx);
        }
      } catch { this.watched = null; }
    });
  }

  getTargetRepo() {
    if (this.selectedRepo) {
      const found = this.repos.find(r => r.root === this.selectedRepo);
      if (found) return found;
    }
    if (this.openFile?.repo) {
      const found = this.repos.find(r => r.root === this.openFile.repo);
      if (found) return found;
    }
    return this.repos.find(r => r.files.length > 0) || this.repos[0] || null;
  }

  refresh(cx) {
    if (this.loading || this.busy) return;
    const { cwd, on } = location(), context = key();
    if (context !== this.context) {
      this.context = context; this.root = null; this.repos = []; this.allFiles = []; this.open = null; this.openFile = null;
      this.selectedRepo = null; this.collapsedRepos = new Set(); this.userToggled = new Set();
      this.setLines([]); this.message = ""; this.input.set_value(""); this.error = null; this.note = null; this.watched = null; this.closed = false; this.limited = false;
    }
    if (!cwd) return;
    this.loading = true;
    cx.spawn(async (cx) => {
      try {
        const repoRoots = await findGitRepos(cwd, on);
        if (key() !== context) return;

        if (!repoRoots.length) {
          this.root = null; this.repos = []; this.allFiles = []; this.setLines([]); this.open = null; this.openFile = null;
          this.error = "Not a git repository (no git projects found)";
          return;
        }

        const loaded = [];
        if (on) {
          for (const r of repoRoots) {
            const res = await loadRepoChanges(r, on);
            if (res) loaded.push(res);
          }
        } else {
          const results = await Promise.all(repoRoots.map(r => loadRepoChanges(r, on)));
          loaded.push(...results.filter(Boolean));
        }
        if (key() !== context) return;

        this.repos = loaded;
        this.allFiles = loaded.flatMap(r => r.files);
        this.limited = loaded.some(r => r.limited);
        this.on = on;
        this.error = null;
        this.root = loaded[0]?.root || cwd;

        // Auto-expand repos with changes, collapse clean ones unless user toggled
        for (const r of loaded) {
          if (!this.userToggled.has(r.root)) {
            if (r.files.length === 0) {
              this.collapsedRepos.add(r.root);
            } else {
              this.collapsedRepos.delete(r.root);
            }
          }
        }

        this.watch(cwd, cx);

        // Keep current diff open if file still exists
        if (this.open && !this.allFiles.some(f => `${f.repo}/${f.path}` === this.open)) {
          this.open = null; this.openFile = null; this.setLines([]);
        }
        if (this.openFile) {
          try { await this.load(this.openFile, context); }
          catch { if (key() === context) this.setLines(["Cannot preview this diff (too large or unreadable)"]); }
        }
      } catch (e) {
        if (key() === context) this.error = String(e);
      } finally {
        this.loading = false; cx.notify();
      }
    });
  }

  async load(file, context) {
    if (!file) return;
    const root = file.repo, on = this.on;
    const repo = this.repos.find(r => r.root === root);
    let lines;
    if (file.fresh || (!repo?.hasHead && file.status === "A")) {
      const text = await readTextFile(`${root}/${file.path}`, { on }).catch(() => null);
      if (text === null) lines = ["Cannot preview this file (binary, too large, or unreadable)"];
      else if (text.includes("\0")) lines = ["Binary file · no text diff"];
      else {
        file.add = countLines(text);
        lines = [`@@ -0,0 +1,${file.add} @@`, ...text.slice(0, 400000).replace(/\n$/, "").split("\n").slice(0, 1500).map(l => `+${l}`)];
      }
    } else {
      const r = await exec("git", [...READ, "diff", ...(repo?.hasHead ? ["HEAD"] : []), "--no-ext-diff", "--no-textconv", "--no-color", "--", file.path], { cwd: root, on });
      if (r.code !== 0) throw new Error(r.stderr);
      const all = r.stdout.replace(/\n$/, "").split("\n"), first = all.findIndex((l) => l.startsWith("@@"));
      lines = first < 0 ? [file.binary ? "Binary file changed" : "No text diff available"] : all.slice(first);
    }
    if (key() === context && this.open === `${file.repo}/${file.path}`) {
      this.setLines(lines.slice(0, 1500).map(line => line.length > 4000 ? line.slice(0, 4000) + "… (line truncated)" : line));
    }
  }

  setLines(lines) {
    this.rows = diffRows(lines);
    this.diffWidth = Math.max(300, ...lines.map(line => line.length * 7 + 50));
  }

  pick(file, cx) {
    const fileKey = `${file.repo}/${file.path}`;
    if (this.open === fileKey) {
      this.open = null;
      this.openFile = null;
      this.setLines([]);
      this.closed = true;
    } else {
      this.open = fileKey;
      this.openFile = file;
      this.selectedRepo = file.repo;
      this.closed = false;
      this.setLines([]);
      this.note = null;
      const context = this.context;
      cx.spawn(async (cx) => {
        try { await this.load(file, context); }
        catch (e) {
          if (key() === context && this.open === fileKey) {
            this.note = `Cannot preview this diff: ${e}`;
          }
        }
        cx.notify();
      });
    }
    cx.notify();
  }

  commit(cx) {
    const target = this.getTargetRepo();
    if (!this.message.trim() || !target || this.busy || this.loading || this.context !== key()) return;
    const root = target.root, on = this.on, context = this.context, message = this.message.trim();
    this.busy = true; this.note = null;
    cx.spawn(async (cx) => {
      try {
        for (const args of [["add", "-A"], ["commit", "-m", message]]) {
          const r = await exec("git", args, { cwd: root, on });
          if (r.code !== 0) throw new Error((r.stderr || r.stdout).trim());
        }
        if (key() === context) { this.message = ""; this.input.set_value(""); }
      } catch (e) { if (key() === context) this.note = `Commit failed (${target.name}): ${e}`; }
      finally { this.busy = false; this.refresh(cx); cx.notify(); }
    });
    cx.notify();
  }

  diffRow({ kind, number, text }, t) {
    if (kind === "@") return div().w_full().px(12).h(20).flex_none().truncate().bg(t.raised).text_color(t.dim).child(text);
    return h_flex().w_full().h(20).flex_none().gap(10).when(kind === "+", (e) => e.bg(t.ansi2 + "18")).when(kind === "-", (e) => e.bg(t.ansi1 + "18"))
      .child(div().w(30).flex_none().text_right().text_color(t.dim).child(number === undefined ? "" : String(number)))
      .child(div().flex_1().min_w(0).truncate().child(text));
  }

  render(cx) {
    if (key() !== this.context) this.refresh(cx);
    const t = theme();
    const totalFiles = this.allFiles.length;
    const totalAdd = this.repos.reduce((n, r) => n + r.add, 0);
    const totalDel = this.repos.reduce((n, r) => n + r.del, 0);
    const targetRepo = this.getTargetRepo();
    const enabled = Boolean(this.message.trim() && targetRepo && !this.busy && !this.loading);
    const hasDiff = Boolean(this.open && this.rows.length);

    return v_flex().id("changes").size_full().min_h(0).rounded(10).overflow_hidden().bg(t.background).border_1().border_color(t.border).text_color(t.foreground).text_size(13).track_focus(this.handle)
      .on_key_down((e, cx) => {
        if (e.key === "escape" && this.typing) { this.typing = false; this.handle.focus(); cx.stop_propagation(); cx.notify(); }
      })
      // Header
      .child(h_flex().h(42).flex_none().px(14).gap(8).items_center().border_b(1).border_color(t.border)
        .child(div().font_semibold().child("Changes"))
        .child(div().px(5).py(1).rounded(4).border_1().border_color(t.border).text_size(10).text_color(t.muted).child(`${totalFiles} files`))
        .when(this.repos.length > 1, el => el.child(div().px(5).py(1).rounded(4).bg(t.raised).text_size(10).text_color(t.dim).child(`${this.repos.length} repos`)))
        .child(div().flex_1())
        .when(this.repos.length > 1, el => el.child(
          div().id("toggle-all-repos").px(6).py(2).rounded(4).bg(t.raised).hover(h => h.bg(t.hover)).cursor_pointer().text_size(10).text_color(t.dim)
            .on_click((_e, cx) => {
              const allCollapsed = this.repos.every(r => this.collapsedRepos.has(r.root));
              if (allCollapsed) {
                this.collapsedRepos.clear();
              } else {
                this.repos.forEach(r => this.collapsedRepos.add(r.root));
              }
              this.repos.forEach(r => this.userToggled.add(r.root));
              cx.notify();
            })
            .child(this.repos.every(r => this.collapsedRepos.has(r.root)) ? "Expand all" : "Collapse all")
        ))
        .child(div().text_size(11).text_color(t.ansi2).child(`+${totalAdd}`))
        .child(div().text_size(11).text_color(t.ansi1).child(`−${totalDel}`)))

      // Top-down Accordion Tree: Repositories and their changed files
      .child(
        div()
          .when(!hasDiff, el => el.flex_1().min_h(0))
          .when(hasDiff, el => el.flex_none().h(220).border_b(1).border_color(t.border))
          .relative()
          .child(
            v_flex()
              .id("changes-tree")
              .size_full()
              .overflow_y_scroll()
              .p(6)
              .gap(2)
              .children(
                this.repos.map((repo) => {
                  const isCollapsed = this.collapsedRepos.has(repo.root);
                  const isTarget = targetRepo?.root === repo.root;

                  return v_flex().id(`repo-group-${repo.name}`).flex_none().child(
                    // Repository Header Row
                    h_flex()
                      .id(`repo-header-${repo.name}`)
                      .h(28)
                      .flex_none()
                      .px(6)
                      .gap(6)
                      .items_center()
                      .rounded(6)
                      .cursor_pointer()
                      .bg(t.raised)
                      .hover(s => s.bg(t.hover))
                      .when(isTarget, s => s.border_1().border_color(t.accent))
                      .on_click((_e, cx) => {
                        this.selectedRepo = repo.root;
                        if (this.collapsedRepos.has(repo.root)) {
                          this.collapsedRepos.delete(repo.root);
                        } else {
                          this.collapsedRepos.add(repo.root);
                        }
                        this.userToggled.add(repo.root);
                        cx.notify();
                      })
                      .child(
                        svg(isCollapsed ? "mods/workspace-chrome/chevron-right.svg" : "mods/workspace-chrome/chevron-down.svg")
                          .size(11)
                          .flex_none()
                          .text_color(t.dim)
                      )
                      .child(div().font_semibold().text_size(12).text_color(t.foreground).truncate().child(repo.name))
                      .when(Boolean(repo.branch), el => el.child(
                        div().px(5).py(1).rounded(3).bg(t.background).border_1().border_color(t.border).text_size(10).text_color(t.dim).truncate().child(repo.branch)
                      ))
                      .child(div().flex_1())
                      .when(repo.add > 0, el => el.child(div().text_size(11).text_color(t.ansi2).child(`+${repo.add}`)))
                      .when(repo.del > 0, el => el.child(div().text_size(11).text_color(t.ansi1).child(`−${repo.del}`)))
                      .child(
                        div().px(5).py(1).rounded(4).border_1().border_color(t.border).text_size(10)
                          .text_color(repo.files.length ? t.foreground : t.muted)
                          .child(`${repo.files.length}`)
                      )
                  )
                  // Expanded files under this repository
                  .when(!isCollapsed, el => el.child(
                    v_flex().id(`repo-files-${repo.name}`).gap(1).pt(2).pb(4).children(
                      repo.files.length === 0
                        ? [div().pl(24).py(4).text_size(11).text_color(t.dim).child("No uncommitted changes")]
                        : repo.files.map((f) => {
                            const fileKey = `${f.repo}/${f.path}`;
                            const parts = f.path.split("/"), name = parts.pop();
                            const isSelected = this.open === fileKey;

                            return h_flex()
                              .id(`change-${fileKey}`)
                              .h(26)
                              .flex_none()
                              .pl(20)
                              .pr(6)
                              .gap(6)
                              .items_center()
                              .rounded(5)
                              .cursor_pointer()
                              .when(isSelected, s => s.bg(t.selected))
                              .hover(s => s.bg(t.hover))
                              .on_click((_e, cx) => {
                                this.selectedRepo = repo.root;
                                this.pick(f, cx);
                              })
                              .child(
                                div().w(14).text_size(11).font_medium()
                                  .text_color(f.status === "A" ? t.ansi2 : f.status === "D" ? t.ansi1 : f.status === "U" ? t.ansi1 : t.blocked)
                                  .child(f.status)
                              )
                              .child(
                                h_flex().flex_1().min_w(0)
                                  .child(div().text_color(t.dim).truncate().child(parts.length ? parts.join("/") + "/" : ""))
                                  .child(div().max_w(260).min_w(0).truncate().child(name))
                              )
                              .when(f.add > 0, s => s.child(div().text_size(11).text_color(t.ansi2).child(`+${f.add}`)))
                              .when(f.del > 0, s => s.child(div().text_size(11).text_color(t.ansi1).child(`−${f.del}`)));
                          })
                    )
                  ));
                })
              )
              .when(!this.repos.length, el => el.child(
                div().p(12).text_color(t.dim).child(
                  this.error || (this.loading ? "Loading changes…" : "No git repository found")
                )
              ))
          )
          .child(Scrollbar.vertical("changes-tree").absolute().inset_0())
      )

      .when(this.limited, el => el.child(div().px(12).py(4).text_size(11).text_color(t.dim).child("Showing the first 1,000 changed files per repo")))

      // Diff viewer (shown when a file is selected)
      .when(hasDiff, el => el.child(
        div().flex_1().min_h(120).relative().font_family(settings().font_family ?? "JetBrains Mono").text_size(11)
          .child(
            div().id("changes-diff-x").size_full().overflow_x_scrollbar()
              .child(
                div().w_full().min_w(this.diffWidth).h_full()
                  .child(v_virtual_list("changes-diff", this.rows.length, 20, (i) => String(i),
                    (range) => this.rows.slice(range.start, range.end).map((row) => this.diffRow(row, t))).size_full())
              )
          )
          .child(Scrollbar.vertical("changes-diff").viewport_from_layout().absolute().inset_0())
      ))

      .when(Boolean(this.note), (e) => e.child(div().px(10).pb(8).text_size(11).text_color(t.ansi1).child(this.note)))

      // Bottom commit bar
      .child(
        h_flex().flex_none().p(10).gap(8).border_t(1).border_color(t.border)
          .child(
            div().id("commit-message").flex_1().min_w(0).h(32).px(10).flex().items_center().rounded(8).border_1()
              .border_color(this.typing ? t.accent : t.border).cursor_text()
              .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
              .child(Input.new(this.input).h_full().text_size(12).text_color(t.foreground))
          )
          .child(
            div().id("commit").h(32).px(14).flex().items_center().rounded(8)
              .bg(enabled ? t.solid : t.raised)
              .text_color(enabled ? t.solid_foreground : t.dim)
              .text_size(12)
              .when(enabled, (e) => e.cursor_pointer())
              .on_click((_e, cx) => this.commit(cx))
              .child(this.busy ? "Committing…" : `Commit${targetRepo ? ` (${targetRepo.name})` : ""}`)
          )
      );
  }
}
