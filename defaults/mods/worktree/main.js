import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input, Scrollbar } from "gpui-base";
import { run, props, closeOverlay, theme, spaces, panes, exec, home, appHome, readTextFile, window } from "herdr";

// Where a space is now: its first terminal's live folder, else where it was made.
function place(s, all = panes()) {
  if (!s) return null;
  const tabs = new Set((s.tabs ?? []).map((c) => c.id));
  return all.find((p) => tabs.has(p.tab) && p.cwd)?.cwd || s.folder || null;
}

// The mod owns this preference; paths belong to the space's machine, not the app.
async function root(userHome) {
  const raw = await readTextFile(`${appHome()}/settings.json`);
  const directory = (raw === null ? {} : JSON.parse(raw))?.worktrees?.directory ?? "~/.herdr/worktrees";
  if (typeof directory !== "string" || !directory.trim()) throw new Error("worktrees.directory must be a non-empty path.");
  let path = directory.replace(/\\/g, "/");
  if (path === "~" || path.startsWith("~/")) path = userHome.replace(/\\/g, "/").replace(/\/$/, "") + path.slice(1);
  if (!path.startsWith("/") && !/^[A-Za-z]:\//.test(path)) throw new Error("worktrees.directory must be an absolute path or start with ~/.");
  return path.replace(/\/+$/, "");
}
// Home as ~ (git says C:/Users/x where Windows says C:\Users\x).
const tilde = (path, userHome) => {
  const [p, h] = [path, userHome].map((s) => s.replace(/\\/g, "/"));
  return p.startsWith(h) ? `~${p.slice(h.length)}` : path;
};
const slug = (branch) => {
  const normalized = branch.normalize("NFC");
  const name = normalized.replace(/[^\p{L}\p{M}\p{N}._-]+/gu, "-").replace(/^-+|-+$/g, "");
  if (name) return name;
  // Hash the original name so symbol-only branches do not share a folder.
  let hash = 2166136261;
  for (const char of normalized) hash = Math.imul(hash ^ char.codePointAt(0), 16777619);
  return `branch-${(hash >>> 0).toString(16).padStart(8, "0")}`;
};
const ago = (s) => {
  const d = Date.now() / 1000 - s;
  return d < 3600 ? `${Math.max(1, Math.round(d / 60))}m` : d < 86400 ? `${Math.round(d / 3600)}h` : d < 86400 * 60 ? `${Math.round(d / 86400)}d` : `${Math.round(d / 86400 / 30)}mo`;
};

const JJ = ["--no-pager", "--color=never"];
const JJ_READ = [...JJ, "--ignore-working-copy", "--at-op=@"];
const checkoutPath = (path) => /^[A-Za-z]:[\\/]|^\\\\/.test(path) ? path.replace(/\\/g, "/") : path;

async function jj(cwd, args, on, timeout) {
  const result = await exec("jj", args, { cwd, on, timeout });
  if (result.code !== 0) throw new Error((result.stderr || result.stdout).trim());
  return result.stdout;
}

async function jjRepo(folder, on) {
  const result = await exec("jj", [...JJ_READ, "workspace", "root"], { cwd: folder, on }).catch(() => null);
  if (!result || /there is no jj repo/i.test(result.stderr)) return null;
  if (result.code !== 0) throw new Error((result.stderr || result.stdout).trim());
  const top = checkoutPath(result.stdout.replace(/\r?\n$/, ""));
  if (!/^(?:\/|[A-Za-z]:\/)/.test(top)) return null;
  const pointer = await readTextFile(`${top}/.jj/repo`, { on }).catch(() => null);
  const main = pointer ? checkoutPath(pointer).replace(/\/\.jj\/repo$/, "") : top;
  return { jj: true, top, name: main.split("/").at(-1) };
}

async function jjWorkspaces(repo, on) {
  const template = 'json(name) ++ "\\t" ++ json(root) ++ "\\t" ++ json(target.current_working_copy()) ++ "\\t" ++ json(target.change_id().shortest(8)) ++ "\\t" ++ json(target.local_bookmarks().map(|b| b.name())) ++ "\\n"';
  const out = await jj(repo.top, [...JJ_READ, "workspace", "list", "-T", template], on);
  const list = out.trimEnd().split("\n").filter(Boolean).map((line) => {
    const [workspace, root, current, id, bookmarks] = line.split("\t").map(JSON.parse);
    const revision = id.prefix + id.rest;
    return { workspace, path: root && checkoutPath(root), current, revision, bookmarks };
  });
  // Older workspaces may have no recorded path. The current one is identifiable
  // by its working-copy change if no other workspace points to it.
  const current = list.filter((c) => c.current);
  if (current.length === 1) current[0].path = repo.top;
  return list.filter((c) => c.path).sort((a, b) => (b.path === repo.top) - (a.path === repo.top));
}

async function jjBookmarks(repo, on) {
  const out = await jj(repo.top, [...JJ_READ, "bookmark", "list", "-T", 'if(!remote, json(name) ++ "\\t" ++ name ++ "\\n")'], on);
  return out.trimEnd().split("\n").filter(Boolean).map((line) => {
    const [label, revision] = line.split("\t");
    return { label: JSON.parse(label), revision };
  });
}

// Every checkout of a repo (`git worktree list`), the main one first, then
// the most recently committed: { path, branch, main, at }.
async function checkouts(repo, on) {
  const list = await git(repo.main, ["worktree", "list", "--porcelain"], on);
  if (!list.ok) return [];
  const refs = await git(repo.main, ["for-each-ref", "--format=%(refname:short)%09%(committerdate:unix)", "refs/heads"], on);
  const when = new Map(refs.out.split("\n").map((l) => l.split("\t")).map(([b, at]) => [b, Number(at) || 0]));
  const out = [];
  for (const block of list.out.split(/\n\n+/)) {
    const lines = block.split("\n");
    const path = lines.find((l) => l.startsWith("worktree "))?.slice(9);
    if (!path || lines.some((l) => l === "bare" || l.startsWith("prunable"))) continue;
    const branch = lines.find((l) => l.startsWith("branch "))?.slice(7).replace(/^refs\/heads\//, "") ?? null;
    out.push({ path, branch, main: out.length === 0, at: branch ? (when.get(branch) ?? 0) : 0 });
  }
  return out.sort((a, b) => b.main - a.main || b.at - a.at);
}

// The space showing a checkout: its first terminal is in it (the deepest checkout wins).
function spaceOf(path, all, on, spaceList, paneList) {
  const inside = (p, dir) => p === dir || p.startsWith(dir + "/");
  return spaceList.find((s) => (s.machine ?? null) === (on ?? null) && (() => {
    const at = place(s, paneList)?.replace(/\\/g, "/");
    if (!at || !inside(at, path)) return false;
    return !all.some((c) => c.path !== path && c.path.length > path.length && inside(at, c.path));
  })());
}

async function git(cwd, args, on, timeout) {
  const r = await exec("git", args, { cwd, on, ...(timeout === undefined ? {} : { timeout }) });
  return { ok: r.code === 0, out: (r.stdout ?? "").trim(), err: (r.stderr ?? r.stdout ?? "").trim() };
}

// The submodule repos git keeps for a checkout (nested ones too, checked out
// or not): { name, dir, pinned }. `config` is the args that read its
// .gitmodules; `pinned` is the commit its parent records ("" if none).
async function modules(gitDir, config, on, out = [], parent = "") {
  const keys = await git(gitDir, ["--git-dir", gitDir, "config", ...config, "-z", "--get-regexp", "^submodule\\..*\\.path$"], on);
  if (!keys.ok) return out; // no .gitmodules, or no submodules in it
  // -z: "key\nvalue" records ended by NUL, so names and paths may hold spaces.
  for (const record of keys.out.split("\0").filter(Boolean)) {
    const at = record.indexOf("\n");
    const name = record.slice("submodule.".length, at - ".path".length);
    const dir = `${gitDir}/modules/${name}`;
    if (!(await git(gitDir, ["--git-dir", dir, "rev-parse", "--git-dir"], on)).ok) continue; // never checked out
    const pinned = await git(gitDir, ["--git-dir", gitDir, "rev-parse", `HEAD:${record.slice(at + 1)}`], on);
    // Names are only unique within their parent: nested ones carry its name.
    out.push({ name: parent + name, dir, pinned: pinned.ok ? pinned.out : "" });
    if ((await git(gitDir, ["--git-dir", dir, "cat-file", "-e", "HEAD:.gitmodules"], on)).ok) await modules(dir, ["--blob", "HEAD:.gitmodules"], on, out, `${parent + name}/`);
  }
  return out;
}

// The worktree's submodules, and the names of those with commits that exist
// nowhere else: not on a remote, and not the commit the parent pins (git
// fetches that by hash, so it is often on no remote branch here).
async function submodules(top, on) {
  const gitDir = await git(top, ["rev-parse", "--absolute-git-dir"], on);
  if (!gitDir.ok) throw new Error(gitDir.err || "git rev-parse failed.");
  const mods = await modules(gitDir.out, ["--file", `${top}/.gitmodules`], on);
  const unpushed = [];
  for (const m of mods) {
    const local = await git(gitDir.out, ["--git-dir", m.dir, "rev-list", "-n1", "--all", "--not", "--remotes", ...(m.pinned ? [m.pinned] : [])], on);
    if (!local.ok) throw new Error(local.err || `Couldn't check submodule ${m.name}.`);
    if (local.out) unpushed.push(m.name);
  }
  return { mods, unpushed };
}

// The repo a folder belongs to: its main checkout and name, and whether
// the folder is itself a linked worktree. null outside git.
async function repoOf(folder, on) {
  const jujutsu = await jjRepo(folder, on);
  if (jujutsu) return jujutsu;
  const top = await git(folder, ["rev-parse", "--show-toplevel"], on);
  const common = await git(folder, ["rev-parse", "--path-format=absolute", "--git-common-dir"], on);
  if (!top.ok || !common.ok) return null;
  const main = common.out.endsWith("/.git") ? common.out.slice(0, -5) : common.out;
  // A bare repo hidden beside its worktrees (<project>/.bare) is named after its project.
  const parts = main.split("/"), name = parts.at(-1) === ".bare" ? parts.at(-2) : parts.at(-1);
  return { top: top.out, main, name, linked: main !== top.out };
}

export default class Worktree extends View {
  init(_props, cx) {
    this.on = this.space()?.machine;
    this.userHome = this.on ? null : home();
    this.opening = props()?.action === "open";
    this.revising = props()?.action === "revision";
    this.text = props()?.branch ?? "";
    this.bookmarks = [];
    this.list = undefined; // open: the checkouts, once read
    this.at = 0;
    this.input = InputState.new({ value: this.text, placeholder: this.opening ? "Filter by name or path" : "Branch name" });
    this.input.on("change", (_e, cx) => { this.text = this.input.value(); this.error = ""; this.at = 0; cx.notify(); });
    this.input.on("submit", (_e, cx) => this.go(cx));
    this.inputFocused = true;
    this.input.on("focus", (_e, cx) => { this.inputFocused = true; cx.notify(); });
    this.input.on("blur", (_e, cx) => { this.inputFocused = false; cx.notify(); });
    this.handle = cx.focus_handle();
    this.handle.focus(); // the root is mounted from the first frame: keep the keyboard here while loading
    if (props()?.action !== "remove") this.input.focus();
    this.busy = false;
    this.error = "";
    this.repo = undefined; // undefined: looking, null: not in git
    this.dirty = false;
    this.unpushed = [];
    cx.spawn(async (cx) => {
      try {
        if (this.on) {
          let result;
          try { result = await exec("sh", ["-c", 'printf %s "$HOME"'], { on: this.on }); } catch {}
          if (!result || result.code !== 0 || !result.stdout.trim()) result = await exec("powershell.exe", ["-NoProfile", "-Command", "[Console]::Write($HOME)"], { on: this.on });
          if (result.code !== 0 || !result.stdout.trim()) throw new Error("Could not find the remote home folder.");
          this.userHome = result.stdout.trim().replace(/\\/g, "/");
        }
        if (!this.opening && !this.revising && props()?.action !== "remove") this.root = await root(this.userHome);
        this.folder = props()?.folder || place(this.space()) || this.userHome;
        // The repo is set last: Enter does nothing until the warnings it consents to are known.
        const repo = await repoOf(this.folder, this.on);
        if (repo?.jj && props().action === "remove") throw new Error("Use jj workspace remove for this workspace.");
        if (repo && !repo.jj && this.revising) throw new Error("Changing revisions requires a jj workspace.");
        if (repo && props().action === "remove") {
          this.dirty = (await git(repo.top, ["--no-optional-locks", "-c", "core.fsmonitor=false", "status", "--porcelain"], this.on)).out !== "";
          this.unpushed = (await submodules(repo.top, this.on)).unpushed;
        }
        if (repo && this.opening) this.list = await (repo.jj ? jjWorkspaces(repo, this.on) : checkouts(repo, this.on));
        if (repo?.jj && !this.opening) {
          const name = props()?.name ?? "";
          const revision = props()?.revision ?? "";
          this.setupJjInputs(name, revision);
          if (!name || this.revising) {
            this.bookmarks = await jjBookmarks(repo, this.on).catch(() => []);
          }
        }
        this.repo = repo;
        cx.notify();
        // The list (and with it the filter input) only mounts once loading is
        // done, so the keyboard must move there now — a focus call made while
        // loading lands on nothing and the dialog stays keyboard-dead.
        this.handle.focus();
        if (props().action !== "remove") cx.timer.after(100, () => (this.revising ? this.revision : this.input)?.focus());
        if (repo?.jj ? props().action === "new" && props().name : props().branch) await this.work(cx);
      } catch (error) { this.repo = null; this.error = String(error); cx.notify(); }
    });
  }

  space() {
    const p = props() ?? {};
    return spaces().find((s) => s.id === p.space) ?? (p.folder ? { folder: p.folder, machine: p.machine ?? null, name: "" } : undefined);
  }

  close() {
    closeOverlay(props().overlay);
  }

  setupJjInputs(name, revision) {
    this.input = InputState.new({ value: name, placeholder: "Workspace name" });
    this.revision = InputState.new({ value: revision, placeholder: this.revising ? "Bookmark, change ID, or revset" : "Bookmark, change ID, or revset (default @)" });
    for (const input of [this.input, this.revision]) {
      input.on("change", (_e, cx) => { this.error = ""; cx.notify(); });
      input.on("submit", (_e, cx) => this.go(cx, this.revising ? "new" : undefined));
    }
  }

  go(cx, change) {
    if (this.busy || !this.repo) return;
    cx.spawn(async (cx) => this.work(cx, change));
  }

  async work(cx, change) {
    const remove = props()?.action === "remove";
    if (this.busy || !this.repo || (remove && !this.repo.linked)) return;
    this.busy = true;
    this.error = "";
    cx.notify();
    try {
      if (change) this.error = await this.changeRevision(change);
      else this.error = remove ? await this.remove() : await this.create();
    }
    catch (error) { this.error = String(error); }
    this.busy = false;
    if (!this.error) this.close();
    cx.notify();
  }

  async create() {
    if (this.repo.jj) return this.createWorkspace();
    const branch = this.text.trim();
    if (!branch) return "Type a branch name.";
    const path = `${this.root}/${this.repo.name}/${slug(branch)}`;
    const exists = (await git(this.repo.main, ["show-ref", "--verify", "--quiet", `refs/heads/${branch}`], this.on)).ok;
    const add = exists ? ["worktree", "add", path, branch] : ["worktree", "add", "-b", branch, path, "HEAD"];
    const r = await git(this.repo.top, add, this.on, 600_000);
    if (!r.ok) return r.err || "git worktree add failed.";
    const p = props();
    // Leave spaces unnamed so they follow the checkout folder and nest under the repo.
    run("space.new", { folder: path, machine: this.on ?? "", tab: p.tab, argv: p.argv });
    return "";
  }

  async createWorkspace() {
    const name = this.input.value().trim();
    if (!name) return "Type a workspace name.";
    const folder = slug(name);
    if (folder === "." || folder === "..") return "Choose a workspace name other than ‘.’ or ‘..’.";
    const parent = `${this.root}/${this.repo.name}`;
    const path = `${parent}/${folder}`;
    const { top } = this.repo;
    const on = this.on;
    const revision = this.revision.value().trim() || "@";
    // Older jj versions can leave a partly created directory for an invalid revset.
    if (!await jj(top, [...JJ_READ, "log", "--no-graph", "--limit=1", "-r", revision, "-T", "commit_id"], on)) return "The revset did not match any changes.";
    // jj workspace add expects its destination's parent directory to exist.
    const windows = /^(?:[A-Za-z]:\/|\/\/)/.test(top);
    const literal = `'${parent.replace(/['‘’‚‛]/g, "$&$&")}'`;
    const mkdir = windows
      ? await exec("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", `[void][IO.Directory]::CreateDirectory(${literal})`], { cwd: top, on })
      : await exec("mkdir", ["-p", "--", parent], { cwd: top, on });
    if (mkdir.code !== 0) throw new Error((mkdir.stderr || mkdir.stdout).trim());
    await jj(top, [...JJ, "workspace", "add", `--name=${name}`, `--revision=${revision}`, "--", path], on, 600_000);
    const { tab, argv } = props();
    run("space.new", { folder: path, machine: on ?? "", tab, argv });
    return "";
  }

  async changeRevision(action) {
    const revision = this.revision.value().trim();
    if (!revision) return "Type a bookmark, change ID, or revset.";
    await jj(this.repo.top, [...JJ, action, "--", revision], this.on, 600_000);
    return "";
  }

  shown() {
    const f = this.text.trim().toLowerCase();
    return (this.list ?? []).filter((c) => !f || [c.branch, c.workspace, c.revision, ...(c.bookmarks ?? []), c.path].join(" ").toLowerCase().includes(f));
  }

  open(c) {
    if (!c) return;
    this.close();
    const s = spaceOf(c.path, this.list, this.on, spaces(), panes());
    if (s) run("space.select", { id: s.id });
    else run("space.new", { folder: c.path, machine: this.on ?? "" });
  }

  async remove() {
    const s = this.space();
    const status = await git(this.repo.top, ["--no-optional-locks", "-c", "core.fsmonitor=false", "status", "--porcelain"], this.on);
    if (!status.ok) return status.err || "git status failed.";
    // git refuses a worktree with submodules unless forced, because their
    // history goes with it. The popup already showed what's at risk; only
    // risk that appeared since it opened gets one more warning.
    const { mods, unpushed } = await submodules(this.repo.top, this.on);
    const changed = (status.out && !this.dirty) || unpushed.some((name) => !this.unpushed.includes(name));
    this.dirty ||= status.out !== "";
    this.unpushed = [...new Set([...this.unpushed, ...unpushed])];
    if (changed) return "It changed since this opened. Enter again removes it anyway.";
    const list = await git(this.repo.main, ["-c", "core.quotePath=false", "worktree", "list", "--porcelain"], this.on);
    if (!list.ok) return list.err || "git worktree list failed.";
    const checkout = list.out.split(/\n\n+/).map((record) => record.split("\n"))
      .find(([path]) => path === `worktree ${this.repo.top}`);
    if (!checkout) return "Git could not find this worktree.";
    const lock = checkout.find((field) => field === "locked" || field.startsWith("locked "));
    if (lock) return lock === "locked" ? "This worktree is locked." : `This worktree is locked: ${lock.slice(7)}`;

    // Git on a remote Windows machine reports a drive-letter or UNC path.
    const windows = this.on ? /^(?:[A-Za-z]:\/|\/\/)/.test(this.repo.top) : window().platform === "windows";
    const closeSpace = () => { if (s?.id) run("space.close", { id: s.id, confirmed: true }); };
    // A shell sitting in the folder can keep it busy on Windows.
    if (windows) closeSpace();
    const force = this.dirty || mods.length > 0;
    const r = await git(this.repo.main, ["worktree", "remove", ...(force ? ["--force"] : []), this.repo.top], this.on, 600_000);
    if (!r.ok) return r.err || "git worktree remove failed.";
    if (!windows) closeSpace();
    return "";
  }

  key(e, cx) {
    if (e.key === "escape") return this.close();
    if (this.opening) {
      const n = this.shown().length;
      if (e.key === "enter") this.open(this.shown()[this.at]);
      else if ((e.key === "down" || e.key === "up") && n) this.at = (this.at + (e.key === "down" ? 1 : n - 1)) % n;
      else return;
      cx.stop_propagation();
      return cx.notify();
    }
    if (e.key === "enter" && props()?.action === "remove") return this.go(cx);
  }

  renderOpen(t) {
    const r = this.repo;
    const rows = this.shown();
    const all = this.list ?? [];
    const spaceList = spaces();
    const paneList = panes();
    const row = (c, i) => {
      const on = i === this.at;
      const there = spaceOf(c.path, all, this.on, spaceList, paneList);
      return v_flex()
        .id(`wt-${i}`)
        .flex_none()
        .h(44)
        .px(10)
        .justify_center()
        .rounded(7)
        .cursor_pointer()
        .when(on, (el) => el.bg(t.selected))
        .when(!on, (el) => el.hover((s) => s.bg(t.hover)))
        .on_click(() => this.open(c))
        .child(
          h_flex()
            .gap(8)
            .items_center()
            .child(div().flex_1().min_w(0).truncate().text_color(t.foreground).child(c.workspace ?? c.branch ?? "detached"))
            .when(Boolean(c.revision), (el) => el.child(div().min_w(0).truncate().text_size(11).text_color(t.dim).child([c.revision, ...(c.bookmarks ?? [])].join(" · "))))
            .when(c.main, (el) => el.child(div().flex_none().text_size(11).text_color(t.dim).child("main checkout")))
            .when(Boolean(there), (el) => el.child(div().flex_none().text_size(11).text_color(t.accent).child("open")))
            .when(!there && c.at > 0, (el) => el.child(div().flex_none().text_size(11).text_color(t.dim).child(ago(c.at)))),
        )
        .child(div().truncate().text_size(11.5).text_color(t.dim).child(tilde(c.path, this.userHome ?? "")));
    };
    const count = `${rows.length === all.length ? all.length : `${rows.length}/${all.length}`} ${r?.jj ? "workspaces" : "checkouts"}`;
    return v_flex()
      .id("worktree")
      .track_focus(this.handle)
      .on_key_down((e, cx) => this.key(e, cx))
      .size_full()
      .p(16)
      .gap(10)
      .bg(t.popover)
      .text_size(13)
      .child(h_flex().child(div().flex_1().font_semibold().text_color(t.foreground).child(`Open ${r?.jj ? "jj workspace" : "worktree"}${r ? ` of ${r.name}` : ""}`)).child(div().text_size(11).text_color(t.dim).child(r ? count : "")))
      .when(r === undefined || (r && this.list === undefined), (el) => el.child(div().text_color(t.muted).child("\u2026")))
      .when(r === null, (el) => el.child(div().text_color(t.muted).child(this.error || `${this.folder ? tilde(this.folder, this.userHome ?? "") : "This folder"} is outside a Git or jj repository.`)))
      .when(Boolean(r && this.list), (el) =>
        el
          .child(Input.new(this.input).h(32).px(10).items_center().rounded(6).border(1).border_color(this.inputFocused ? t.accent : t.input).bg(t.background).text_color(t.foreground))
          .child(
            div()
              .relative()
              .flex_1()
              .min_h(0)
              .child(
                v_flex()
                  .id("wt-list")
                  .size_full()
                  .overflow_y_scroll()
                  .children(rows.map(row))
                  .when(rows.length === 0, (el) => el.child(div().p(10).text_color(t.dim).child(r.jj ? "No matching workspaces." : "No matching worktrees."))),
              )
              .child(Scrollbar.vertical("wt-list").absolute().inset_0()),
          ),
      )
      .child(h_flex().justify_end().text_size(11).text_color(t.dim).child("\u2191\u2193 choose \u00b7 \u21b5 open \u00b7 esc close"));
  }

  renderJj(t) {
    const field = (input) => Input.new(input).h(32).px(10).items_center().rounded(6).border(1).border_color(t.input).bg(t.background).text_color(t.foreground);
    const button = (label, click) => h_flex().id(label).h(30).px(10).items_center().rounded(6).bg(t.raised)
      .text_color(this.busy ? t.dim : t.foreground).when(!this.busy, el => el.cursor_pointer().hover(s => s.bg(t.hover)))
      .on_click(click).child(label);
    return v_flex().id("worktree").track_focus(this.handle).on_key_down((e, cx) => this.key(e, cx)).size_full()
      .p(16).gap(10).bg(t.popover).text_size(13)
      .child(div().font_semibold().text_color(t.foreground).child(this.revising ? "Change revision" : `New jj workspace of ${this.repo.name}`))
      .child(div().text_color(t.muted).child(this.revising
        ? "Start a new change on the selected revision or make it your working-copy change."
        : `Creates a workspace under ${tilde(this.root, this.userHome)}/${this.repo.name}/ with a new change on the selected revision.`))
      .when(!this.revising, el => el.child(field(this.input)))
      .child(field(this.revision))
      .when(this.bookmarks.length > 0, el => el.child(v_flex().id("jj-bookmarks").flex_1().min_h(0).overflow_y_scroll().gap(3)
        .child(div().text_color(t.dim).text_size(11).child("Bookmarks"))
        .children(this.bookmarks.map(({ label, revision }) => div().id(`bookmark-${label}`).p(5).flex_none().rounded(6).text_color(t.foreground)
          .cursor_pointer().hover(s => s.bg(t.hover)).on_click((_e, cx) => { this.revision.set_value(revision); cx.notify(); }).child(label)))))
      .when(Boolean(this.error), el => el.child(div().text_color(t.blocked).child(this.error)))
      .when(this.bookmarks.length === 0, el => el.child(div().flex_1()))
      .child(h_flex().gap(8).flex_none().justify_end()
        .when(this.busy, el => el.child(div().text_color(t.dim).child("Working…")))
        .when(this.revising, el => el.child(button("Edit change", (_e, cx) => this.go(cx, "edit"))))
        .child(button(this.revising ? "New change on revision" : "Create workspace", (_e, cx) => this.go(cx, this.revising ? "new" : undefined))));
  }

  render(cx) {
    const t = theme();
    if (this.opening) return this.renderOpen(t);
    if (this.repo?.jj) return this.renderJj(t);
    const remove = props()?.action === "remove";
    const s = this.space();
    const r = this.repo;
    const title = this.revising ? "Change revision" : remove ? `Remove worktree ${s?.name ?? ""}?` : `New worktree${r ? ` of ${r.name}` : ""}`;
    const detail =
      r === undefined
        ? "…"
        : r === null
          ? this.error ? "" : `${this.folder ? tilde(this.folder, this.userHome) : "This folder"} is outside a Git or jj repository.`
          : remove
            ? !r.linked
              ? "This is the repository's main checkout, not a worktree."
              : `Closes the space and deletes ${tilde(r.top, this.userHome)}. The branch stays.`
            : `A new space on a checkout in ${tilde(this.root, this.userHome)}/${r.name}/. A branch that exists is checked out; a new one starts from here.`;
    const blocked = !r || (remove && !r.linked);
    return v_flex()
      .id("worktree")
      .track_focus(this.handle)
      .on_key_down((e, cx) => this.key(e, cx))
      .size_full()
      .p(16)
      .gap(10)
      .bg(t.popover)
      .text_size(13)
      .child(div().font_semibold().text_color(t.foreground).child(title))
      .child(div().text_color(t.muted).child(detail))
      .when(remove && this.dirty, (el) => el.child(div().text_color(t.blocked).child("It has uncommitted changes: they will be lost.")))
      .when(remove && this.unpushed.length > 0, (el) => el.child(div().text_color(t.blocked).child(`Submodule ${this.unpushed.join(", ")} has commits that aren't pushed anywhere: they will be lost.`)))
      .when(!remove && !blocked, (el) =>
        el.child(
          Input.new(this.input)
            .h(32)
            .px(10)
            .items_center()
            .rounded(6)
            .border(1)
            .border_color(this.inputFocused ? t.accent : t.input)
            .bg(t.background)
            .text_color(t.foreground),
        ),
      )
      .when(Boolean(this.error), (el) => el.child(div().text_color(t.blocked).child(this.error)))
      .child(div().flex_1())
      .child(
        h_flex()
          .justify_end()
          .text_size(11)
          .text_color(t.dim)
          .gap(6)
          .when(this.busy, (el) => el.child(div().frames(["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"], 90)))
          .child(this.busy ? "working…" : blocked ? "esc close" : remove ? "↵ remove · esc cancel" : "↵ create · esc cancel"),
      );
  }
}
