// Worktree dialog: { space, action: "new" | "local" | "remote" | "open" | "remove" }
// or { folder, machine } for a repo without a space. Absent action opens Remote.
// { branch, tab?, argv? } places that branch at once.

import { View, div } from "gpui-kit";
import { v_flex, h_flex, InputState, Input, Scrollbar, Checkbox } from "gpui-base";
import { run, props, closeOverlay, theme, spaces, panes, exec, home } from "herdr";

// Where a space is now: its first terminal's live folder, else where it was made.
function place(s, all = panes()) {
  if (!s) return null;
  const tabs = new Set((s.tabs ?? []).map((c) => c.id));
  return all.find((p) => tabs.has(p.tab) && p.cwd)?.cwd || s.folder || null;
}

// Checkouts go to <ROOT>/<repo>/<branch, slashes as dashes>.
const ROOT = (home) => `${home}/.herdr/worktrees`;
// Home as ~ (git says C:/Users/x where Windows says C:\Users\x).
const tilde = (path, userHome) => {
  const [p, h] = [path, userHome].map((s) => s.replace(/\\/g, "/"));
  return p.startsWith(h) ? `~${p.slice(h.length)}` : path;
};
const slug = (branch) => branch.replace(/[^A-Za-z0-9._-]+/g, "-").replace(/^-+|-+$/g, "");
const ago = (s) => {
  const d = Date.now() / 1000 - s;
  return d < 3600 ? `${Math.max(1, Math.round(d / 60))}m` : d < 86400 ? `${Math.round(d / 3600)}h` : d < 86400 * 60 ? `${Math.round(d / 86400)}d` : `${Math.round(d / 86400 / 30)}mo`;
};

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

// A new space carries tab and argv when the caller set them. That includes a
// checkout that already exists and does not have a space yet.
function spaceNew(folder, on) {
  const p = props() ?? {};
  const space = { folder, machine: on ?? "" };
  if (p.tab !== undefined) space.tab = p.tab;
  if (p.argv !== undefined) space.argv = p.argv;
  run("space.new", space);
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
  const top = await git(folder, ["rev-parse", "--show-toplevel"], on);
  const common = await git(folder, ["rev-parse", "--path-format=absolute", "--git-common-dir"], on);
  if (!top.ok || !common.ok) return null;
  const main = common.out.endsWith("/.git") ? common.out.slice(0, -5) : common.out;
  return { top: top.out, main, name: main.split("/").pop(), linked: main !== top.out };
}

export default class Worktree extends View {
  init(_props, cx) {
    this.on = this.space()?.machine;
    this.userHome = this.on ? null : home();
    const action = props()?.action;
    this.source = action === "new" || action === "local" || action === "open" || action === "remove" ? action : "remote";
    this.query = { new: "", local: "", remote: "", open: "" };
    this.text = props()?.branch ?? "";
    if (this.source !== "remove") this.query[this.source] = this.text;
    this.list = undefined; // checkouts, once read
    this.branches = undefined;
    this.remotes = undefined;
    this.at = 0;
    this.inputFocused = true;
    this.mountInput(this.query[this.source] ?? "");
    this.handle = cx.focus_handle();
    this.handle.focus(); // the root is mounted from the first frame: keep the keyboard here while loading
    if (props()?.action !== "remove") this.input.focus();
    this.busy = false;
    this.error = "";
    this.repo = undefined; // undefined: looking, null: not in git
    this.dirty = false;
    this.unpushed = [];
    this.branch = "";
    this.deleteBranch = true;
    this.branchRefused = "";
    // null until Enter locks it. A focused checkbox toggles on that same Enter.
    this.branchChoice = null;
    this.branchKeyFlip = false;
    // Set for the Enter that confirms, so the checkbox's keyup click cannot paint a different box.
    this.branchHeld = false;
    cx.spawn(async (cx) => {
      try {
        if (this.on) {
          let result;
          try { result = await exec("sh", ["-c", 'printf %s "$HOME"'], { on: this.on }); } catch {}
          if (!result || result.code !== 0 || !result.stdout.trim()) result = await exec("powershell.exe", ["-NoProfile", "-Command", "[Console]::Write($HOME)"], { on: this.on });
          if (result.code !== 0 || !result.stdout.trim()) throw new Error("Could not find the remote home folder.");
          this.userHome = result.stdout.trim().replace(/\\/g, "/");
        }
        this.folder = props()?.folder || place(this.space()) || this.userHome;
        // The repo is set last: Enter does nothing until the warnings it consents to are known.
        const repo = await repoOf(this.folder, this.on);
        if (repo && props().action === "remove") {
          this.dirty = (await git(repo.top, ["--no-optional-locks", "-c", "core.fsmonitor=false", "status", "--porcelain"], this.on)).out !== "";
          this.unpushed = (await submodules(repo.top, this.on)).unpushed;
          const head = await git(repo.top, ["rev-parse", "--abbrev-ref", "HEAD"], this.on);
          // Detached HEAD is "HEAD": there is no local branch to delete.
          this.branch = head.ok && head.out && head.out !== "HEAD" ? head.out : "";
          if (!head.ok) this.error = head.err || "git rev-parse failed.";
          if (!this.branch) this.deleteBranch = false;
        }
        if (repo && props().action !== "remove") await this.loadLists(repo, cx);
        this.repo = repo;
        cx.notify();
        // The list (and with it the filter input) only mounts once loading is
        // done, so the keyboard must move there now — a focus call made while
        // loading lands on nothing and the dialog stays keyboard-dead.
        this.handle.focus();
        if (props().action !== "remove") cx.timer.after(100, () => this.input.focus());
        if (props().branch) await this.work(cx, true);
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

  go(cx) {
    if (this.busy || !this.repo) return;
    return cx.spawn(async (cx) => this.work(cx));
  }

  // immediate is only the opening {branch} resolution. A later Enter follows the source.
  async work(cx, immediate = false) {
    if (this.busy || !this.repo) return;
    this.busy = true;
    this.error = "";
    cx.notify();
    try {
      this.error = props().action === "remove"
        ? await this.remove()
        : immediate
          ? await this.resolveBranch((this.text ?? "").trim())
          : await this.create();
    } catch (error) { this.error = String(error); }
    this.busy = false;
    if (!this.error) this.close();
    cx.notify();
  }

  // JS InputState cannot change its placeholder, so each source gets a new one.
  mountInput(value) {
    this.input = InputState.new({ value: value ?? "", placeholder: this.placeholder() });
    this.input.on("change", (_e, cx) => {
      if (this.source !== "remove") this.query[this.source] = this.input.value();
      this.text = this.input.value();
      this.error = "";
      this.at = 0;
      cx.notify();
    });
    this.input.on("submit", (_e, cx) => this.go(cx));
    this.input.on("focus", (_e, cx) => { this.inputFocused = true; cx.notify(); });
    this.input.on("blur", (_e, cx) => { this.inputFocused = false; cx.notify(); });
  }

  // for-each-ref lines: "name\tunix", newest first.
  branchesFrom(out) {
    return out.split("\n").filter(Boolean).map((line) => {
      const [name, at] = line.split("\t");
      return { name, at: Number(at) || 0 };
    }).sort((a, b) => b.at - a.at);
  }

  // "origin/feature/name" → { remote, name }. A remote's HEAD is not a branch.
  remotesFrom(out) {
    return this.branchesFrom(out).flatMap((row) => {
      const slash = row.name.indexOf("/");
      if (slash <= 0) return [];
      const remote = row.name.slice(0, slash);
      const name = row.name.slice(slash + 1);
      if (name === "HEAD") return [];
      return [{ remote, name, at: row.at }];
    });
  }

  async loadLists(repo = this.repo, cx) {
    const format = ["for-each-ref", "--format=%(refname:short)%09%(committerdate:unix)"];
    this.list = await checkouts(repo, this.on);
    const heads = await git(repo.main, [...format, "refs/heads"], this.on);
    this.branches = this.branchesFrom(heads.ok ? heads.out : "");
    await this.refreshRemotes(repo, format, cx);
  }

  // refs/remotes is only the last fetch, so a branch created on the server
  // since then is missing. Ask every remote first. A failed fetch stays on
  // the Remote page; the list is whatever that fetch left behind.
  async refreshRemotes(repo, format, cx) {
    this.fetchError = "";
    this.loading = "Fetching remote branches…";
    cx?.notify();
    let fetched;
    try {
      fetched = await git(repo.main, ["fetch", "--all", "--prune", "--no-tags", "--no-recurse-submodules"], this.on, 120_000);
    } catch (error) {
      this.fetchError = error?.message || String(error);
    }
    if (fetched && !fetched.ok) this.fetchError = fetched.err || "git fetch failed.";
    this.loading = "";
    const remotes = await git(repo.main, [...format, "refs/remotes"], this.on);
    this.remotes = this.remotesFrom(remotes.ok ? remotes.out : "");
  }

  async localExists(name) {
    return (await git(this.repo.main, ["show-ref", "--verify", "--quiet", `refs/heads/${name}`], this.on)).ok;
  }

  treePath(branch) {
    return `${ROOT(this.userHome)}/${this.repo.name}/${slug(branch)}`;
  }

  async added(path, args) {
    const r = await git(this.repo.top, args, this.on, 600_000);
    if (!r.ok) return r.err || "git worktree add failed.";
    spaceNew(path, this.on);
    return "";
  }

  // "" or an error. A branch already in this.list opens that checkout.
  async place(kind, branch, remote) {
    const name = (branch ?? "").trim();
    if (!name) return "Type a branch name.";
    const checked = (this.list ?? []).find((c) => c.branch === name);
    if (kind !== "new" && checked) {
      this.open(checked);
      return "";
    }
    if (kind === "new") {
      if (await this.localExists(name)) return `Branch ${name} already exists.`;
      const path = this.treePath(name);
      return this.added(path, ["worktree", "add", "-b", name, path, "HEAD"]);
    }
    if (kind === "remote" && !(await this.localExists(name))) {
      const path = this.treePath(name);
      return this.added(path, ["worktree", "add", "--track", "-b", name, path, remote]);
    }
    const path = this.treePath(name);
    return this.added(path, ["worktree", "add", path, name]);
  }

  // {branch}: local, else the one remote with that short name, else from HEAD.
  async resolveBranch(name) {
    const branch = (name ?? "").trim();
    if (!branch) return "Type a branch name.";
    if ((this.list ?? []).some((c) => c.branch === branch) || await this.localExists(branch)) return this.place("local", branch);
    const hits = (this.remotes ?? []).filter((r) => r.name === branch);
    if (hits.length === 1) return this.place("remote", branch, `${hits[0].remote}/${hits[0].name}`);
    return this.place("new", branch);
  }

  placeholder() {
    if (this.source === "open") return "filter worktrees";
    if (this.source === "local" || this.source === "remote") return "filter branches";
    return "branch name";
  }

  // Enter follows the source. New never checks out an existing name.
  async create() {
    if (this.source === "open") {
      this.open(this.shown()[this.at]);
      return "";
    }
    if (this.source === "remote") {
      const row = this.shownRemotes()[this.at];
      if (!row) return "Pick a branch.";
      return this.place("remote", row.name, `${row.remote}/${row.name}`);
    }
    if (this.source === "local") {
      const row = this.shownBranches()[this.at];
      if (!row) return "Pick a branch.";
      return this.place("local", row.name);
    }
    return this.place("new", (this.query?.new ?? this.text ?? "").trim());
  }

  shownBranches() {
    const f = (this.query?.local ?? "").trim().toLowerCase();
    return (this.branches ?? []).filter((b) => !f || b.name.toLowerCase().includes(f));
  }

  shownRemotes() {
    const f = (this.query?.remote ?? "").trim().toLowerCase();
    return (this.remotes ?? []).filter((r) => !f || `${r.remote}/${r.name}`.toLowerCase().includes(f));
  }

  shown() {
    const f = (this.query?.open ?? "").trim().toLowerCase();
    return (this.list ?? []).filter((c) => !f || `${c.branch ?? ""} ${c.path}`.toLowerCase().includes(f));
  }

  selectSource(source, cx) {
    this.source = source;
    this.at = 0;
    this.error = "";
    const value = this.query[source] ?? "";
    this.text = value;
    this.mountInput(value);
    // The new field is not in the tree until the next frame.
    if (cx) cx.timer.after(100, () => this.input.focus());
  }

  open(c) {
    if (!c) return;
    this.close();
    const s = spaceOf(c.path, this.list, this.on, spaces(), panes());
    if (s) run("space.select", { id: s.id });
    else spaceNew(c.path, this.on);
  }

  async remove() {
    // -d already refused. The checkout is gone; this enter is only the -D question.
    if (this.branchRefused) {
      const deleted = await git(this.repo.main, ["branch", "-D", this.branchRefused], this.on);
      return deleted.ok ? "" : deleted.err || "git branch -D failed.";
    }
    // Locked by Enter before any await. A later click must not change which branch goes.
    const branch = this.branchChoice != null ? this.branchChoice : (this.deleteBranch ? (this.branch || "") : "");
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
    // Terminals first: a shell sitting in the folder keeps it busy on Windows.
    run("space.close", { id: s.id, confirmed: true });
    const force = this.dirty || mods.length > 0;
    const r = await git(this.repo.main, ["worktree", "remove", ...(force ? ["--force"] : []), this.repo.top], this.on, 600_000);
    if (!r.ok) return r.err || "git worktree remove failed.";
    if (!branch) return "";
    const deleted = await git(this.repo.main, ["branch", "-d", branch], this.on);
    if (deleted.ok) return "";
    // The directory is already gone. Show git's refusal and wait: Enter uses -D, Escape keeps the branch.
    this.branchRefused = branch;
    return deleted.err || "git branch -d failed.";
  }

  // Click and Space change the box. Enter confirms, so a toggle that arrived
  // with that Enter is ignored until the key comes back up.
  setBranchChecked(checked, cx) {
    if (this.branchHeld) return;
    this.deleteBranch = checked;
    this.branchKeyFlip = true;
    cx.notify();
    cx.timer.after(0, () => { this.branchKeyFlip = false; });
    // A click focuses the box. The keyboard belongs to the dialog: Enter removes.
    this.handle?.focus();
  }

  key(e, cx) {
    if (e.key === "escape") return this.close();
    if (this.source === "remove") {
      if (e.key === "enter") {
        // The box records Enter on keydown and clicks itself on keyup. That click
        // is dropped once focus has moved, because the generation no longer matches.
        if (this.branchKeyFlip) this.deleteBranch = !this.deleteBranch;
        this.branchKeyFlip = false;
        this.branchChoice = this.deleteBranch ? (this.branch || "") : "";
        this.branchHeld = true;
        this.handle?.focus();
        return this.go(cx);
      }
      return;
    }
    if (this.source === "local" || this.source === "remote" || this.source === "open") {
      const rows = this.source === "open" ? this.shown() : this.source === "local" ? this.shownBranches() : this.shownRemotes();
      const n = rows.length;
      if (e.key === "enter") {
        // The field's submit and this handler both see one Enter. go() marks
        // busy before its first await, so the second arrival does not open again.
        this.go(cx);
      } else if ((e.key === "down" || e.key === "up") && n) this.at = (this.at + (e.key === "down" ? 1 : n - 1)) % n;
      else return;
      cx.stop_propagation();
      return cx.notify();
    }
    if (e.key === "enter") return this.go(cx);
  }

  // A checkout already on screen. One spaces()/panes() pair per render.
  shownSpace(name, all, spaceList, paneList) {
    const checkout = (this.list ?? []).find((c) => c.branch === name);
    return checkout ? spaceOf(checkout.path, all, this.on, spaceList, paneList) : null;
  }

  placeRows(t, spaceList, paneList) {
    const open = this.source === "open";
    const rows = open ? this.shown() : this.source === "local" ? this.shownBranches() : this.shownRemotes();
    const all = this.list ?? [];
    const row = (item, i) => {
      const name = open ? item.branch : item.name;
      const label = open ? (item.branch ?? "detached") : this.source === "remote" ? `${item.remote}/${item.name}` : item.name;
      const there = open ? spaceOf(item.path, all, this.on, spaceList, paneList) : this.shownSpace(name, all, spaceList, paneList);
      const on = i === this.at;
      return v_flex()
        .id(`wt-${i}`)
        .flex_none()
        .h(open ? 44 : 32)
        .px(10)
        .justify_center()
        .rounded(7)
        .cursor_pointer()
        .when(on, (el) => el.bg(t.selected))
        .when(!on, (el) => el.hover((s) => s.bg(t.hover)))
        .on_click((_e, cx) => {
          this.at = i;
          if (open) this.open(item);
          else this.go(cx);
        })
        .child(
          h_flex()
            .gap(8)
            .items_center()
            .child(div().flex_1().min_w(0).truncate().text_color(t.foreground).child(label))
            .when(open && item.main, (el) => el.child(div().flex_none().text_size(11).text_color(t.dim).child("main checkout")))
            .when(Boolean(there), (el) => el.child(div().flex_none().text_size(11).text_color(t.accent).child("open")))
            .when(open && !there && item.at > 0, (el) => el.child(div().flex_none().text_size(11).text_color(t.dim).child(ago(item.at)))),
        )
        .when(open, (el) => el.child(div().truncate().text_size(11.5).text_color(t.dim).child(tilde(item.path, this.userHome ?? ""))));
    };
    const allRows = open ? all : this.source === "local" ? (this.branches ?? []) : (this.remotes ?? []);
    const query = (this.query?.[this.source] ?? "").trim();
    const empty = open
      ? "No matching worktrees."
      : allRows.length === 0 && !query
        ? this.source === "remote" ? "No remote branches." : "No local branches."
        : "No matching branches.";
    return { rows, row, empty };
  }

  // The selected row opens a checkout, or adds one.
  placeHint() {
    const rows = this.source === "open" ? this.shown() : this.source === "local" ? this.shownBranches() : this.shownRemotes();
    if (rows.length === 0) return "esc cancel";
    const item = rows[Math.min(this.at, rows.length - 1)];
    const name = this.source === "open" ? item.branch : item.name;
    const checked = this.source === "open" || (this.list ?? []).some((c) => c.branch === name);
    return checked ? "\u2191\u2193 choose \u00b7 \u21b5 open \u00b7 esc cancel" : "\u2191\u2193 choose \u00b7 \u21b5 checkout \u00b7 esc cancel";
  }

  render(cx) {
    const t = theme();
    if (this.source === "remove") return this.renderRemove(t);
    const r = this.repo;
    const home = r ? `${tilde(ROOT(this.userHome), this.userHome)}/${r.name}/` : "";
    const detail = r === undefined
      ? (this.loading || "\u2026")
      : r === null
        ? `${this.folder ? tilde(this.folder, this.userHome ?? "") : "This folder"} is not in a git repository.`
        : this.source === "new"
          ? `A new branch starting from here, checked out in ${home}.`
          : this.source === "open"
            ? `${(this.list ?? []).length} checkouts`
            : `A branch already checked out opens that space. Otherwise it is checked out in ${home}.`;
    const ready = Boolean(r);
    const spaceList = ready && this.source !== "new" ? spaces() : [];
    const paneList = ready && this.source !== "new" ? panes() : [];
    const listed = ready && this.source !== "new" ? this.placeRows(t, spaceList, paneList) : null;
    const hint = !ready ? "esc close" : this.source === "new" ? "\u21b5 create \u00b7 esc cancel" : this.placeHint();
    return v_flex()
      .id("worktree")
      .track_focus(this.handle)
      .on_key_down((e, cx) => this.key(e, cx))
      .size_full()
      .p(16)
      .gap(10)
      .bg(t.popover)
      .text_size(13)
      .child(div().font_semibold().text_color(t.foreground).child(`New worktree${r ? ` of ${r.name}` : ""}`))
      .child(h_flex().gap(4).children(["new", "local", "remote", "open"].map((source) =>
        div()
          .px(8)
          .py(3)
          .rounded(6)
          .cursor_pointer()
          .text_size(11)
          .text_color(this.source === source ? t.foreground : t.dim)
          .when(this.source === source, (el) => el.bg(t.selected))
          .on_click((_e, cx) => { this.selectSource(source, cx); cx.notify(); })
          .child(source === "new" ? "New" : source === "local" ? "Local" : source === "remote" ? "Remote" : "Open"),
      )))
      .child(div().text_color(t.muted).child(detail))
      .when(ready, (el) => el.child(
        Input.new(this.input).h(32).px(10).items_center().rounded(6).border(1).border_color(this.inputFocused ? t.accent : t.input).bg(t.background).text_color(t.foreground),
      ))
      .when(Boolean(listed), (el) => el.child(
        div().relative().flex_1().min_h(0)
          .child(v_flex().id("wt-list").size_full().overflow_y_scroll().children(listed.rows.map(listed.row)).when(listed.rows.length === 0, (box) => box.child(div().p(10).text_color(t.dim).child(listed.empty))))
          .child(Scrollbar.vertical("wt-list").absolute().inset_0()),
      ))
      .when(Boolean(this.error), (el) => el.child(div().text_color(t.blocked).child(this.error)))
      .when(this.source === "remote" && Boolean(this.fetchError), (el) => el.child(div().text_color(t.blocked).child(this.fetchError)))
      .when(this.source === "new", (el) => el.child(div().flex_1()))
      .child(h_flex().justify_end().text_size(11).text_color(t.dim).gap(6).when(this.busy, (el) => el.child(div().frames(["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"], 90))).child(this.busy ? "working\u2026" : hint));
  }

  renderRemove(t) {
    const s = this.space();
    const r = this.repo;
    const detail = r === undefined
      ? "\u2026"
      : r === null
        ? `${this.folder ? tilde(this.folder, this.userHome) : "This folder"} is not in a git repository.`
        : !r.linked
          ? "This is the repository's main checkout, not a worktree."
          : `Closes the space and deletes ${tilde(r.top, this.userHome)}.`;
    const blocked = !r || !r.linked;
    return v_flex()
      .id("worktree")
      .track_focus(this.handle)
      .on_key_down((e, cx) => this.key(e, cx))
      .on_key_up((e) => { if (e.key === "enter") this.branchHeld = false; })
      .size_full()
      .p(16)
      .gap(10)
      .bg(t.popover)
      .text_size(13)
      .child(div().font_semibold().text_color(t.foreground).child(`Remove worktree ${s?.name ?? ""}?`))
      .child(div().text_color(t.muted).child(detail))
      .when(Boolean(r?.linked && this.branch), (el) => el.child(
        // Checkbox draws nothing of its own. The box is the first child; the label is the second, so one click covers both.
        Checkbox.new("delete-branch").checked(Boolean(this.deleteBranch)).on_change((checked, cx) => {
          this.setBranchChecked(checked, cx);
        })
          .flex().items_center().gap(8).cursor_pointer()
          .child(div().size(14).flex_none().rounded(3).border(1).border_color(this.deleteBranch ? t.solid : t.input).bg(this.deleteBranch ? t.solid : t.background).flex().items_center().justify_center().text_size(10).font_semibold().text_color(t.solid_foreground).child(this.deleteBranch ? "\u2713" : ""))
          .child(div().text_color(t.foreground).child(`Delete local branch ${this.branch}`)),
      ))
      .when(this.dirty, (el) => el.child(div().text_color(t.blocked).child("It has uncommitted changes: they will be lost.")))
      .when(this.unpushed.length > 0, (el) => el.child(div().text_color(t.blocked).child(`Submodule ${this.unpushed.join(", ")} has commits that aren't pushed anywhere: they will be lost.`)))
      .when(Boolean(this.error), (el) => el.child(div().text_color(t.blocked).child(this.error)))
      .when(Boolean(this.branchRefused), (el) => el.child(div().text_color(t.foreground).child(`Delete local branch ${this.branchRefused} anyway?`)))
      .child(div().flex_1())
      .child(h_flex().justify_end().text_size(11).text_color(t.dim).gap(6).when(this.busy, (el) => el.child(div().frames(["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"], 90))).child(this.busy ? "working\u2026" : blocked ? "esc close" : this.branchRefused ? "\u21b5 delete anyway \u00b7 esc keep the branch" : "\u21b5 remove \u00b7 esc cancel"));
  }
}
