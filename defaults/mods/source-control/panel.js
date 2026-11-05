import { View, div } from "gpui-kit";
import { h_flex, v_flex, v_resizable, resizable_panel, InputState, Input, TextareaState, Textarea } from "gpui-base";
import { spaces, panes, focused, theme, settings, props, openOverlay, pickFolder, shared, setShared } from "herdr";
import { createGit } from "./git.js";
import { parseGraph, renderGraph } from "./graph.js";
import { DiffPanel } from "./diff-ui.js";
import { MergePanel } from "./merge-ui.js";

function activeLocation() {
  const space = spaces().find(s => s.active), focus = focused();
  const pane = panes().find(p => p.pane === focus?.pane);
  return { cwd: pane?.cwd || focus?.cwd || space?.folder, on: space?.machine ?? null };
}

export default class SourceControl extends View {
  init(p, cx) {
    p = p || props() || {};
    if (p.mode === "diff") { this.diffPanel = new DiffPanel(p, cx); return; }
    if (p.mode === "merge") { this.mergePanel = new MergePanel(p, cx); return; }
    this.limit = 200; this.history = []; this.collapsed = {}; this.message = "";
    this.input = TextareaState.new({ placeholder: "Message (Ctrl+Enter to commit)", rows: 2 });
    this.input.set_auto_grow(2, 4);
    this.input.on("change", (_e, cx) => { this.message = this.input.value(); cx.notify(); });
    this.input.on("submit", (event, cx) => {
      if (event.secondary && !event.shift) { if (this.dialog) this.submitDialog(cx); else this.commit({}, cx); }
    });
    this.handle = cx.focus_handle();
    this.refresh(cx);
    cx.timer.every(10000, cx => this.refresh(cx));
  }

  location() { return this.chosen || activeLocation(); }
  async close(cx) { if (this.diffPanel?.close) await this.diffPanel.close(cx); if (this.mergePanel?.close) await this.mergePanel.close(cx); }
  key() { return JSON.stringify(this.location()); }
  stamp() { return JSON.stringify({ head: this.data?.head, branch: this.data?.branch, operation: this.data?.operation }); }
  dialogCurrent(dialog) { return dialog.context === this.key() && dialog.stamp === this.stamp(); }
  ready() { return Boolean(this.data && !this.busy && !this.loading && !this.error && this.context === this.key()); }

  refresh(cx) {
    if (this.busy || this.loading) return;
    const context = this.key(), { cwd, on } = this.location();
    if (context !== this.context) {
      this.context = context; this.data = null; this.repo = null; this.history = []; this.dialog = null;
      this.note = null; this.error = null; this.limit = 200; this.selected = null;
      this.message = ""; this.input.set_value("");
    }
    if (!cwd) return;
    this.loading = true;
    const repo = createGit(cwd, on);
    cx.spawn(async cx => {
      try {
        const data = await repo.read(this.limit);
        if (context !== this.key()) return;
        this.data = data; this.repo = repo; this.history = parseGraph(data.graph); this.error = null;
      } catch (error) { if (context === this.key()) this.error = String(error); }
      finally { this.loading = false; cx.notify(); }
    });
  }

  perform(label, action, args, cx, after) {
    if (!this.ready() && action !== "init") return;
    if (this.busy || this.loading) return;
    const context = this.context, snapshot = this.data, repo = this.repo || createGit(this.location().cwd, this.location().on);
    this.busy = label; this.dialog = null; this.note = null;
    cx.spawn(async cx => {
      let completed = false;
      try {
        if (snapshot) {
          const current = await repo.read(1);
          if (current.head !== snapshot.head || current.branch?.name !== snapshot.branch?.name || current.operation !== snapshot.operation) throw new Error("The branch or Git operation changed. Refresh and try again.");
        }
        if (context !== this.key()) return;
        await repo.run(action, { branch: snapshot?.branch, ...args });
        completed = true;
        if (context === this.key() && (action === "commit" || action === "commitAll")) { this.message = ""; this.input.set_value(""); }
        if (after) {
          const current = await repo.read(1);
          if (context !== this.key()) return;
          if (current.branch?.name !== snapshot?.branch?.name || current.operation) throw new Error("The commit completed, but the branch or Git operation changed. Refresh before pushing or syncing.");
          await repo.run(after, { branch: current.branch });
        }
        if (context === this.key()) this.note = `${label} completed`;
      } catch (error) { if (context === this.key()) this.note = `${completed && after ? `${label} completed; ${after} failed` : `${label} failed`}\n${error}`; }
      finally { this.busy = null; this.refresh(cx); cx.notify(); }
    });
    cx.notify();
  }

  choose(title, items, cx) {
    if (this.busy) return;
    this.dialog = { title, items, context: this.key(), stamp: this.stamp() }; cx.notify();
  }

  form(title, fields, submit, cx, description = "", accept = "Confirm") {
    if (this.busy) return;
    const dialog = { title, description, accept, context: this.key(), stamp: this.stamp(), fields: fields.map(f => ({ ...f, input: InputState.new({ value: f.value || "", placeholder: f.placeholder || f.label }) })), submit };
    for (const field of dialog.fields) field.input.on("submit", (_e, cx) => this.submitDialog(cx));
    this.dialog = dialog; cx.notify();
    dialog.fields[0]?.input.focus();
  }

  confirm(title, description, submit, cx) { this.form(title, [], submit, cx, description, "Run"); }

  submitDialog(cx) {
    const d = this.dialog;
    if (!d?.submit || this.busy || this.loading) return;
    if (!this.dialogCurrent(d)) { this.dialog = null; this.note = "The repository state changed. Select the action again."; cx.notify(); return; }
    const values = Object.fromEntries(d.fields.map(f => [f.key, f.input.value().trim()]));
    if (d.fields.some(f => f.required !== false && !values[f.key])) { d.error = "Complete the required fields."; cx.notify(); return; }
    this.dialog = null; d.submit(values, cx); cx.notify();
  }

  primary() {
    const data = this.data, ready = this.ready();
    if (!data) return { label: "✓ Commit", enabled: false, action: "commit" };
    const groups = data.groups, changes = groups.staged.length + groups.changes.length;
    if (data.operation) return { label: `✓ Continue ${data.operation}`, enabled: ready && !groups.conflicts.length, action: "continue" };
    if (changes || groups.conflicts.length) return { label: "✓ Commit", enabled: ready && !!this.message.trim() && !groups.conflicts.length, action: "commit" };
    const branch = data.branch;
    if (data.head && branch && (!branch.upstream || branch.gone)) return { label: "↑ Publish Branch", enabled: ready && !!data.remotes.length, action: "publish" };
    if (branch?.behind) return { label: `↻ Sync Changes ↓${branch.behind} ↑${branch.ahead}`, enabled: ready, action: "sync" };
    if (branch?.ahead) return { label: `↑ Push ${branch.ahead}`, enabled: ready, action: "push" };
    return { label: "✓ Commit", enabled: false, action: "commit" };
  }

  primaryClick(cx) {
    const action = this.primary();
    if (!action.enabled) return;
    if (action.action === "commit") this.commit({}, cx);
    else if (action.action === "continue") this.perform("Continue", "continue", { operation: this.data.operation, message: this.message }, cx);
    else this.network(action.action, cx);
  }

  commit(options, cx) {
    if (!this.ready() || this.data.groups.conflicts.length) return;
    if (!this.message.trim()) { this.note = "Enter a commit message."; cx.notify(); return; }
    const action = options.all ? "commitAll" : "commit";
    const args = { message: this.message.trim(), amend: !!options.amend, signoff: !!options.signoff, signed: options.signed, allowEmpty: !!options.allowEmpty };
    if (!options.all && !options.amend && !options.allowEmpty && !this.data.groups.staged.length) {
      if (!this.data.groups.changes.length) return;
      this.confirm("Stage All & Commit", "There are no staged changes. Stage all changes and commit them?", (_v, cx) => this.perform("Commit", "commitAll", args, cx, options.after), cx); return;
    }
    if (options.amend) this.confirm("Amend Commit", "Replace the last commit with this message and the staged changes. This rewrites its history, including commits already shared.", (_v, cx) => this.perform("Amend", action, args, cx), cx);
    else this.perform("Commit", action, args, cx, options.after);
  }

  network(action, cx, extra = {}) {
    if (!this.ready()) return;
    if (action === "publish") {
      if (!this.data.branch) return;
      const branch = { ...this.data.branch };
      const publish = (remote, cx) => this.perform("Publish Branch", "publish", { remote, branch }, cx);
      if (this.data.remotes.length === 1) publish(this.data.remotes[0], cx);
      else this.choose("Publish to Remote", this.data.remotes.map(remote => ({ label: remote, run: cx => publish(remote, cx) })), cx);
    } else if (action === "sync") this.confirm("Sync Changes", "Pull the current branch, then push it. If conflicts occur, resolve them before continuing.", (_v, cx) => this.perform("Sync", "sync", extra, cx), cx);
    else this.perform(action[0].toUpperCase() + action.slice(1), action, extra, cx);
  }

  openDiff(file, group, cx, hash) {
    if (!this.ready()) return;
    openOverlay({ mod: "source-control", kind: "dialog", width: group === "conflicts" ? 1200 : 1100, height: group === "conflicts" ? 850 : 740, props: { mode: group === "conflicts" ? "merge" : "diff", root: this.data.root, on: this.location().on, file, group, hash } });
  }

  fileMenu(file, group, cx) {
    const items = [{ label: "Open Changes", run: cx => this.openDiff(file, group, cx) }];
    if (group === "staged") items.push({ label: "Unstage Changes", run: cx => this.perform("Unstage", "unstage", { file }, cx) });
    else items.push({ label: group === "conflicts" ? "Mark Resolved / Stage" : "Stage Changes", run: cx => this.perform("Stage", "stage", { file }, cx) });
    if (group === "conflicts") for (const side of ["ours", "theirs"]) items.push({ label: `Accept ${side === "ours" ? "Current" : "Incoming"} Version`, run: cx => this.confirm("Resolve Conflict", `${file.path}\nReplace the whole file with the ${side} version. Review it before staging.`, (_v, cx) => this.perform("Resolve", "resolve", { file, side }, cx), cx) });
    if (group === "changes") items.push({ label: "Discard Changes…", run: cx => this.discard(file, cx) });
    this.choose(file.path, items, cx);
  }

  discard(file, cx) {
    this.confirm("Discard Changes", `${file.path}\n${file.untracked ? "Delete this untracked file." : "Discard unstaged changes. Keep the staged content."}`, (_v, cx) => this.perform("Discard", "discard", { file }, cx), cx);
  }

  commitMenu(cx) {
    this.choose("Commit", [
      { label: "Commit Staged", run: cx => this.commit({}, cx) },
      { label: "Commit All", run: cx => this.commit({ all: true }, cx) },
      { label: "Commit & Push", run: cx => this.commit({ after: "push" }, cx) },
      { label: "Commit & Sync", run: cx => this.confirm("Commit & Sync", "Commit, then pull and push the current branch.", (_v, cx) => this.commit({ after: "sync" }, cx), cx) },
      { label: "Commit (Amend)…", run: cx => this.commit({ amend: true }, cx) },
      { label: "Commit (Sign Off)", run: cx => this.commit({ signoff: true }, cx) },
      { label: "Commit (Signed)", run: cx => this.commit({ signed: true }, cx) },
      { label: "Commit Empty", run: cx => this.commit({ allowEmpty: true }, cx) },
      { label: "Undo Last Commit…", run: cx => this.confirm("Undo Last Commit", "Undo the last commit and leave its changes staged.", (_v, cx) => this.perform("Undo Commit", "undoCommit", {}, cx), cx) },
    ], cx);
  }

  branchMenu(cx) {
    this.choose("Branches", [
      { label: "+ Create Branch…", run: cx => this.newBranch(null, cx) },
      ...this.data.refs.filter(r => r.type !== "tag").map(ref => ({ label: ref.name, hint: ref.type, run: cx => this.branchActions(ref, cx) })),
    ], cx);
  }

  newBranch(start, cx) {
    this.form("Create Branch", [{ key: "name", label: "Branch name" }], (v, cx) => this.perform("Create Branch", "branchCreate", { name: v.name, start: start || undefined, checkout: true }, cx), cx);
  }

  branchActions(ref, cx) {
    const items = [
      { label: "Checkout Branch", run: cx => this.perform("Checkout", "checkout", { ref: ref.ref }, cx) },
      { label: "Merge into Current Branch…", run: cx => this.confirm("Merge", `${ref.name} → ${this.data.branch?.name || "HEAD"}`, (_v, cx) => this.perform("Merge", "merge", { ref: ref.ref }, cx), cx) },
      { label: "Rebase Current Branch onto This…", run: cx => this.confirm("Rebase", `Replay commits from ${this.data.branch?.name || "HEAD"} onto ${ref.name}.`, (_v, cx) => this.perform("Rebase", "rebase", { ref: ref.ref }, cx), cx) },
      { label: "Create Branch from Here…", run: cx => this.newBranch(ref.ref, cx) },
    ];
    if (ref.type === "branch") items.push(
      { label: "Rename Branch…", run: cx => this.form("Rename Branch", [{ key: "name", label: "New name", value: ref.name }], (v, cx) => this.perform("Rename Branch", "branchRename", { name: ref.name, newName: v.name }, cx), cx) },
      { label: "Delete Branch…", run: cx => this.confirm("Delete Branch", `${ref.name}\nUnmerged branches will not be deleted.`, (_v, cx) => this.perform("Delete Branch", "branchDelete", { name: ref.name }, cx), cx) },
    );
    this.choose(ref.name, items, cx);
  }

  graphMenu(row, cx) {
    this.selected = row.hash;
    this.choose(row.subject, [
      { label: "Show Commit Changes", run: cx => this.showCommit(row, cx) },
      { label: "Create Branch from Commit…", run: cx => this.newBranch(row.hash, cx) },
      { label: "Create Tag…", run: cx => this.createTag(row.hash, cx) },
      { label: "Checkout Commit (Detached)…", run: cx => this.confirm("Checkout Commit", `${row.hash}\nSwitch to a detached HEAD, without checking out a branch.`, (_v, cx) => this.perform("Checkout", "checkout", { ref: row.hash, detach: true }, cx), cx) },
      { label: "Cherry-pick Commit…", run: cx => this.confirm("Cherry-pick", row.subject, (_v, cx) => this.perform("Cherry-pick", "cherryPick", { hash: row.hash }, cx), cx) },
      { label: "Revert Commit…", run: cx => this.confirm("Revert Commit", `${row.subject}\nCreate a new commit that reverses this change.`, (_v, cx) => this.perform("Revert", "revert", { hash: row.hash }, cx), cx) },
      { label: "Reset Current Branch to Commit…", run: cx => this.choose("Reset Branch", [
        { label: "Soft — keep changes staged", run: cx => this.resetTo(row, "soft", cx) },
        { label: "Mixed — keep working tree changes", run: cx => this.resetTo(row, "mixed", cx) },
        { label: "Hard — discard working tree changes…", run: cx => this.resetTo(row, "hard", cx) },
      ], cx) },
    ], cx);
  }

  resetTo(row, mode, cx) {
    this.confirm(`Reset (${mode})`, `${row.hash}\n${row.subject}\n${mode === "hard" ? "Discard staged and working tree changes, then move the branch." : "Move the branch to the selected commit."}`, (_v, cx) => this.perform("Reset", "reset", { hash: row.hash, mode }, cx), cx);
  }

  showCommit(row, cx) {
    if (!this.ready()) return;
    this.selected = row.hash;
    const context = this.context, repo = this.repo;
    this.busy = "Loading commit"; this.dialog = null;
    cx.spawn(async cx => {
      try {
        const detail = await repo.showCommit(row.hash);
        if (context !== this.key()) return;
        this.dialog = { title: row.subject, description: detail.text, context, stamp: this.stamp(), items: detail.files.map(file => ({ label: file.path, hint: file.status, run: cx => this.openDiff(file, "history", cx, row.hash) })) };
      } catch (error) { if (context === this.key()) this.note = String(error); }
      finally { this.busy = null; cx.notify(); }
    });
    cx.notify();
  }

  stashMenu(cx) {
    this.choose("Stashes", [
      { label: "+ Stash (including untracked)…", run: cx => this.form("Stash Changes", [{ key: "message", label: "Message", required: false }], (v, cx) => this.perform("Stash", "stashSave", { message: v.message, includeUntracked: true }, cx), cx) },
      ...this.data.stashes.map(stash => ({ label: stash.subject, hint: stash.ref, run: cx => this.choose(stash.ref, [
        { label: "Apply Stash", run: cx => this.perform("Apply Stash", "stashApply", { ref: stash.ref, hash: stash.hash }, cx) },
        { label: "Pop Stash", run: cx => this.perform("Pop Stash", "stashPop", { ref: stash.ref, hash: stash.hash }, cx) },
        { label: "Drop Stash…", run: cx => this.confirm("Drop Stash", `${stash.ref}\n${stash.subject}\nDelete the saved changes.`, (_v, cx) => this.perform("Drop Stash", "stashDrop", { ref: stash.ref, hash: stash.hash }, cx), cx) },
      ], cx) })),
    ], cx);
  }

  createTag(ref, cx) {
    this.form("Create Tag", [{ key: "name", label: "Tag name" }, { key: "message", label: "Annotation (optional)", required: false }], (v, cx) => this.perform("Create Tag", "tagCreate", { ...v, ref: ref || "HEAD" }, cx), cx);
  }

  tagsMenu(cx) {
    this.choose("Tags", [{ label: "+ Create Tag…", run: cx => this.createTag(null, cx) }, ...this.data.refs.filter(r => r.type === "tag").map(ref => ({ label: ref.name, run: cx => this.choose(ref.name, [
      { label: "Checkout Tag (Detached)", run: cx => this.perform("Checkout Tag", "checkout", { ref: ref.ref, detach: true }, cx) },
      { label: "Delete Local Tag…", run: cx => this.confirm("Delete Tag", ref.name, (_v, cx) => this.perform("Delete Tag", "tagDelete", { name: ref.name }, cx), cx) },
    ], cx) }))], cx);
  }

  worktreeMenu(cx) {
    this.choose("Worktrees", [
      { label: "+ Add Worktree…", run: cx => this.form("Add Worktree", [{ key: "path", label: "Absolute folder path" }, { key: "name", label: "New branch name" }], (v, cx) => this.perform("Add Worktree", "worktreeAdd", v, cx), cx) },
      ...this.data.worktrees.map(worktree => ({ label: worktree.path, hint: worktree.branch, run: cx => this.choose(worktree.path, [
        { label: "Show in Source Control", run: cx => { this.chosen = { cwd: worktree.path, on: this.location().on }; this.refresh(cx); cx.notify(); } },
        { label: "Remove Worktree…", run: cx => this.confirm("Remove Worktree", `${worktree.path}\nWorktrees with changes or locks will not be removed.`, (_v, cx) => this.perform("Remove Worktree", "worktreeRemove", { path: worktree.path }, cx), cx) },
      ], cx) })),
    ], cx);
  }

  remoteMenu(cx) {
    this.choose("Remote", [
      { label: "Fetch", run: cx => this.network("fetch", cx) },
      { label: "Fetch All (Prune)", run: cx => this.network("fetch", cx, { all: true, prune: true }) },
      { label: "Pull", run: cx => this.network("pull", cx) },
      { label: "Pull (Rebase)", run: cx => this.network("pull", cx, { rebase: true }) },
      { label: "Push", run: cx => this.network("push", cx) },
      { label: "Force Push (Lease)…", run: cx => this.confirm("Force Push with Lease", `${this.data.branch?.name} → ${this.data.branch?.upstream}\nReplace remote history only if the remote still matches its last observed commit.`, (_v, cx) => this.perform("Force Push", "forcePush", {}, cx), cx) },
      { label: "Push Tags…", run: cx => this.choose("Push Tags to Remote", this.data.remotes.map(remote => ({ label: remote, run: cx => this.confirm("Push Tags", `Push all local tags to ${remote}.`, (_v, cx) => this.perform("Push Tags", "pushTags", { remote }, cx), cx) })), cx) },
      { label: "Publish Branch…", run: cx => this.network("publish", cx) },
      { label: "Sync Changes…", run: cx => this.network("sync", cx) },
      { label: "Add Remote…", run: cx => this.form("Add Remote", [{ key: "name", label: "Name", value: "origin" }, { key: "url", label: "URL" }], (v, cx) => this.perform("Add Remote", "remoteAdd", v, cx), cx) },
      ...this.data.remotes.map(name => ({ label: `Remove Remote ${name}…`, run: cx => this.confirm("Remove Remote", `${name}\nRemove the local remote configuration and tracking refs. The server repository is kept.`, (_v, cx) => this.perform("Remove Remote", "remoteRemove", { name }, cx), cx) })),
    ], cx);
  }

  repositoryMenu(cx) {
    const choices = [{ label: "Follow Active Terminal", run: cx => { this.chosen = null; this.refresh(cx); cx.notify(); } }];
    const found = new Set();
    for (const space of spaces()) {
      const tabs = new Set(space.tabs.map(tab => tab.id));
      const folders = panes().filter(p => tabs.has(p.tab) && p.cwd).map(p => p.cwd);
      for (const cwd of folders.length ? folders : [space.folder]) {
        const id = `${space.machine}:${cwd}`;
        if (!cwd || found.has(id)) continue;
        found.add(id); choices.push({ label: `${space.name} · ${cwd}`, run: cx => { this.chosen = { cwd, on: space.machine ?? null }; this.refresh(cx); cx.notify(); } });
      }
    }
    choices.push({ label: "Open Local Folder…", run: cx => cx.spawn(async cx => { const cwd = await pickFolder({ prompt: "Source Control" }); if (cwd) { this.chosen = { cwd, on: null }; this.refresh(cx); cx.notify(); } }) });
    choices.push({ label: "Clone Repository…", run: cx => this.form("Clone Repository", [{ key: "url", label: "Repository URL" }, { key: "path", label: "Destination folder (absolute path)" }], (v, cx) => this.clone(v, cx), cx) });
    this.choose("Repositories", choices, cx);
  }

  clone(args, cx) {
    if (this.busy || this.loading) return;
    const context = this.key(), location = this.location();
    this.busy = "Cloning";
    cx.spawn(async cx => {
      try {
        await createGit(location.cwd, location.on).run("clone", args);
        if (context === this.key()) { this.chosen = { cwd: args.path, on: location.on }; this.note = "Clone completed"; }
      } catch (error) { if (context === this.key()) this.note = `Clone failed\n${error}`; }
      finally { this.busy = null; this.refresh(cx); cx.notify(); }
    });
    cx.notify();
  }

  moreMenu(cx) {
    if (!this.data) { this.repositoryMenu(cx); return; }
    const items = [
      { label: "Commit…", run: cx => this.commitMenu(cx) },
      { label: "Stage All Changes", run: cx => this.perform("Stage All", "stage", { all: true }, cx) },
      { label: "Unstage All Changes", run: cx => this.perform("Unstage All", "unstage", { all: true }, cx) },
      { label: "Remote / Fetch / Pull / Push…", run: cx => this.remoteMenu(cx) },
      { label: "Branches / Merge / Rebase…", run: cx => this.branchMenu(cx) },
      { label: "Stashes…", run: cx => this.stashMenu(cx) },
      { label: "Tags…", run: cx => this.tagsMenu(cx) },
      { label: "Worktrees…", run: cx => this.worktreeMenu(cx) },
      { label: "Repositories…", run: cx => this.repositoryMenu(cx) },
    ];
    if (this.data.operation) items.unshift({ label: `Abort ${this.data.operation}…`, run: cx => this.confirm("Abort Operation", `Abort the ${this.data.operation} operation.`, (_v, cx) => this.perform("Abort", "abort", { operation: this.data.operation }, cx), cx) });
    this.choose("Source Control", items, cx);
  }

  button(id, label, tip, action, enabled = true) {
    const t = theme();
    return div().id(id).h(24).min_w(22).px(4).flex_none().flex().items_center().justify_center().rounded(3).tooltip(tip).text_color(enabled ? t.muted : t.dim)
      .when(enabled, e => e.cursor_pointer().hover(s => s.bg(t.hover).text_color(t.foreground)))
      .on_click((_e, cx) => { cx.stop_propagation(); if (enabled) action(cx); }).child(label);
  }

  group(name, title, cx) {
    const t = theme(), files = this.data?.groups[name] || [], ready = this.ready();
    if (!files.length && name !== "changes") return div();
    return v_flex().flex_none().min_w(0)
      .child(h_flex().id(`group-${name}`).h(26).px(8).gap(5).items_center().cursor_pointer().hover(s => s.bg(t.hover))
        .on_click((_e, cx) => { this.collapsed[name] = !this.collapsed[name]; cx.notify(); })
        .child(div().text_color(t.muted).child(this.collapsed[name] ? "›" : "⌄"))
        .child(div().font_semibold().child(title))
        .child(div().rounded(4).bg(t.raised).px(5).text_size(10).child(String(files.length)))
        .child(div().flex_1())
        .when(name !== "conflicts", e => e.child(this.button(`all-${name}`, name === "staged" ? "−" : "+", name === "staged" ? "Unstage All" : "Stage All", cx => this.perform(name === "staged" ? "Unstage All" : "Stage All", name === "staged" ? "unstage" : "stage", { all: true }, cx), ready && files.length > 0))))
      .when(!this.collapsed[name], e => e.children(files.map(file => {
        const basename = file.path.split("/").pop(), directory = file.path.slice(0, -basename.length);
        return h_flex().id(`${name}:${file.path}`).h(26).flex_none().px(12).gap(5).items_center().cursor_pointer().hover(s => s.bg(t.hover)).tooltip(file.oldPath ? `${file.oldPath} → ${file.path}` : file.path)
          .on_click((_e, cx) => this.openDiff(file, name, cx))
          .on_mouse_down("right", (_e, cx) => { cx.stop_propagation(); this.fileMenu(file, name, cx); })
          .child(div().w(12).flex_none().text_color(name === "conflicts" ? t.ansi1 : file.status === "A" || file.untracked ? t.ansi2 : t.ansi3).child(file.status))
          .child(div().flex_1().min_w(0).truncate().child(basename))
          .child(div().max_w(85).truncate().text_size(10).text_color(t.dim).child(directory))
          .child(this.button(`stage:${name}:${file.path}`, name === "staged" ? "−" : "+", name === "staged" ? "Unstage" : "Stage", cx => this.perform(name === "staged" ? "Unstage" : "Stage", name === "staged" ? "unstage" : "stage", { file }, cx), ready))
          .when(name === "changes", e => e.child(this.button(`discard:${file.path}`, "↶", "Discard Changes", cx => this.discard(file, cx), ready)))
          .when(name === "conflicts", e => e.child(this.button(`resolve:${file.path}`, "…", "Resolve Conflict", cx => this.fileMenu(file, name, cx), ready)));
      })).when(!files.length, e => e.child(div().px(18).py(8).text_size(11).text_color(t.dim).child(this.loading && !this.data ? "Loading changes…" : "No changes"))));
  }

  renderDialog() {
    const d = this.dialog, t = theme();
    return v_flex().absolute().inset_0().p(12).bg(t.background).gap(10)
      .child(h_flex().gap(8).items_center().child(div().flex_1().min_w(0).font_semibold().child(d.title)).child(this.button("dialog-close", "×", "Close", cx => { this.dialog = null; cx.notify(); })))
      .when(Boolean(d.description), e => e.child(div().id("dialog-description").max_h(150).flex_none().overflow_y_scrollbar().text_size(11).text_color(t.muted).child(d.description)))
      .when(Boolean(d.items), e => e.child(v_flex().id("source-menu").flex_1().min_h(0).overflow_y_scrollbar().gap(2).children(d.items.map((item, index) =>
        h_flex().id(`menu-${index}`).min_h(32).flex_none().px(8).py(5).gap(8).items_center().rounded(4).cursor_pointer().hover(s => s.bg(t.hover))
          .on_click((_e, cx) => { if (this.busy || this.loading) return; if (!this.dialogCurrent(d)) { this.dialog = null; this.note = "The repository state changed. Select the action again."; cx.notify(); return; } this.dialog = null; item.run(cx); cx.notify(); })
          .child(div().flex_1().min_w(0).child(item.label)).when(!!item.hint, e => e.child(div().max_w(75).truncate().text_size(10).text_color(t.dim).child(item.hint)))))))
      .when(Boolean(d.fields), e => e.children(d.fields.map((field, index) => v_flex().gap(4).flex_none()
        .child(div().text_size(11).text_color(t.muted).child(field.label))
        .child(div().id(`form-${index}`).h(32).px(8).rounded(4).border_1().border_color(t.input).bg(t.surface).child(Input.new(field.input).h_full())))))
      .when(Boolean(d.error), e => e.child(div().text_color(t.ansi1).child(d.error)))
      .when(Boolean(d.submit), e => e.child(h_flex().gap(8).justify_end().child(this.button("cancel", "Cancel", "Cancel", cx => { this.dialog = null; cx.notify(); })).child(div().id("form-submit").px(14).py(7).rounded(4).bg(this.loading ? t.raised : t.accent).text_color(this.loading ? t.dim : t.accent_foreground).when(!this.loading, e => e.cursor_pointer()).on_click((_e, cx) => this.submitDialog(cx)).child(this.loading ? "Refreshing…" : d.accept))));
  }

  render(cx) {
    if (this.diffPanel) return this.diffPanel.render(cx);
    if (this.mergePanel) return this.mergePanel.render(cx);
    const space = spaces().find(s => s.active)?.id;
    if (this.space !== undefined && this.space !== space) { this.chosen = null; this.dialog = null; }
    this.space = space;
    const revision = shared("source-control.revision");
    if (revision !== this.revision && !this.loading && !this.busy) { this.revision = revision; this.refresh(cx); }
    if (this.context !== this.key()) this.refresh(cx);
    const t = theme(), primary = this.primary(), data = this.data;
    const branch = data?.branch, repoName = data?.root?.split(/[\\/]/).pop() || "Repository";
    const groupsHeight = ["conflicts", "staged", "changes"].reduce((height, name) => {
      const count = data?.groups[name].length || 0;
      if (!count && name !== "changes") return height;
      return height + 26 + (this.collapsed[name] ? 0 : count ? count * 26 : 38);
    }, 8);
    const upper = v_flex().id("source-control-upper").size_full().min_h(0).overflow_y_scrollbar()
      .child(h_flex().h(26).flex_none().px(10).gap(6).items_center()
        .child(div().id("repository-picker").flex_1().min_w(0).truncate().text_color(t.muted).cursor_pointer().tooltip(data?.root || this.location().cwd || "").on_click((_e, cx) => this.repositoryMenu(cx)).child(`▾ ${repoName}`))
        .child(div().id("branch-picker").max_w(170).truncate().text_color(t.ansi4).cursor_pointer().tooltip(branch?.upstream || "Select branch").on_click((_e, cx) => { if (this.ready()) this.branchMenu(cx); }).child(branch ? `⑂ ${branch.name}${branch.behind ? ` ↓${branch.behind}` : ""}${branch.ahead ? ` ↑${branch.ahead}` : ""}` : data?.head ? `Detached ${data.head.slice(0, 7)}` : "")))
      .when(Boolean(this.error), e => e.child(div().id("source-error").max_h(90).flex_none().overflow_y_scrollbar().px(12).py(8).text_color(t.ansi1).text_size(11).child(this.error)))
      .when(!data && !this.loading, e => e.child(this.button("init", "Initialize Repository", "Initialize Git in current folder", cx => this.confirm("Initialize Repository", this.location().cwd || "", (_v, cx) => this.perform("Initialize", "init", {}, cx), cx))))
      .child(v_flex().flex_none().px(12).py(8).gap(7)
        .child(div().id("commit-message").min_h(48).max_h(92).px(8).py(5).rounded(3).bg(t.surface).border_1().border_color(t.input).child(Textarea.new(this.input).w_full().text_size(12)))
        .child(h_flex().h(28).gap(1).child(div().id("primary-action").flex_1().flex().items_center().justify_center().rounded(3).bg(primary.enabled ? t.accent : t.raised).text_color(primary.enabled ? t.accent_foreground : t.dim)
          .when(primary.enabled, e => e.cursor_pointer()).on_click((_e, cx) => this.primaryClick(cx)).child(this.busy || primary.label))
          .child(div().id("commit-options").w(27).flex_none().flex().items_center().justify_center().bg(t.raised).rounded(3).cursor_pointer().on_click((_e, cx) => this.commitMenu(cx)).child("⌄"))))
      .when(Boolean(data?.operation), e => e.child(h_flex().px(12).gap(6).items_center().child(div().flex_1().text_color(t.ansi3).child(`${data.operation} in progress`)).child(this.button("abort", "Abort", "Abort operation", cx => this.confirm("Abort", data.operation, (_v, cx) => this.perform("Abort", "abort", { operation: data.operation }, cx), cx), this.ready()))))
      .child(v_flex().id("file-groups").flex_1().min_h(64).overflow_y_scrollbar().pb(8)
        .child(this.group("conflicts", "Merge Changes", cx)).child(this.group("staged", "Staged Changes", cx)).child(this.group("changes", "Changes", cx)))
      .when(Boolean(this.note), e => e.child(h_flex().px(10).py(4).gap(4).flex_none().items_start().child(div().id("operation-output").flex_1().min_w(0).max_h(100).overflow_y_scrollbar().text_size(11).text_color(t.muted).child(this.note)).child(this.button("dismiss-note", "×", "Dismiss", cx => { this.note = null; cx.notify(); }))));
    const graph = v_flex().size_full().min_h(0)
      .child(h_flex().h(30).flex_none().px(10).gap(6).items_center().border_t(1).border_color(t.border)
        .child(div().id("graph-heading").flex_1().min_w(0).cursor_pointer().on_click((_e, cx) => { this.collapsed.graph = !this.collapsed.graph; cx.notify(); }).child(`${this.collapsed.graph ? "›" : "⌄"} Graph`))
        .child(div().text_size(10).text_color(t.dim).child(String(this.history.filter(r => r.hash).length)))
        .child(this.button("fetch", "↓", "Fetch", cx => this.network("fetch", cx), this.ready() && !!data?.remotes.length))
        .child(this.button("push", "↑", "Push", cx => this.network("push", cx), this.ready() && !!branch?.upstream && !data?.operation))
        .child(this.button("graph-refresh", "↻", "Refresh Graph", cx => this.refresh(cx), !this.busy && !this.loading)))
      .when(!this.collapsed.graph, e => e.child(renderGraph(this.history, t, { selected: this.selected, onSelect: (row, cx) => this.showCommit(row, cx), onContext: (row, cx) => this.graphMenu(row, cx) }))
        .when(!this.history.length, e => e.child(div().px(12).py(8).text_color(t.dim).child(this.loading ? "Loading graph…" : "No commits")))
        .when(this.history.filter(r => r.hash).length >= this.limit, e => e.child(this.button("graph-more", "Load more…", "Load older commits", cx => { this.limit += 200; this.refresh(cx); }, !this.busy && !this.loading))));
    const savedHeight = shared("source-control.upper-height"), zoom = settings().zoom;
    // Native panel sizes are window pixels; ordinary element styles apply zoom themselves.
    // Reserve the 36px title, two borders, and at least 80px of Graph.
    const maxUpper = Math.max(160 * zoom, (this.panelHeight || 0) - 118 * zoom);
    const upperHeight = Math.min(maxUpper, (Number.isFinite(savedHeight) ? Math.max(160, savedHeight) : 135 + Math.min(285, groupsHeight)) * zoom);
    return v_flex().id("source-control").size_full().min_h(0).relative().rounded(10).overflow_hidden().bg(t.background).border_1().border_color(t.border).text_color(t.foreground).text_size(12).track_focus(this.handle)
      .on_size_change(({ height }, cx) => { this.panelHeight = height; cx.notify(); })
      .on_key_down((e, cx) => {
        if (e.key === "escape") { this.dialog = null; cx.notify(); }
      })
      .child(h_flex().h(36).flex_none().px(12).gap(5).items_center().child(div().font_semibold().child("Source Control")).child(div().flex_1())
        .child(this.button("refresh", "↻", "Refresh", cx => this.refresh(cx), !this.busy && !this.loading))
        .child(this.button("more", "…", "More Actions", cx => this.moreMenu(cx), !this.busy)))
      // Measure before creating native state so a saved height fits even in a smaller window.
      .when(this.panelHeight > 0, e => e.child(v_resizable("source-control-split").w_full().flex_1().min_h(0)
        .on_resize(([height], cx) => { setShared("source-control.upper-height", height / zoom); cx.notify(); })
        .child(resizable_panel().size(upperHeight).size_range(160 * zoom, maxUpper).flex_none().child(upper))
        .child(resizable_panel().size_range(80 * zoom).child(graph))))
      .when(Boolean(this.dialog), e => e.child(this.renderDialog()));
  }
}
