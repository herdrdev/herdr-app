# Worktree Dialog Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the separate New worktree and Open worktree dialogs with one four-source dialog that checks out an existing branch, opens its space when it is already checked out, or creates a branch from HEAD.

**Architecture:** `defaults/mods/worktree/main.js` gains `loadLists`, `place`, and `resolveBranch`. The dialog's `source` is `new`, `local`, `remote`, or `open`. Menus open it with `action: "remote"`. Remove stays a separate small dialog.

**Tech Stack:** gpui-kit mod JavaScript, Bun tests in `scripts/worktree.test.ts`.

**Spec:** `docs/superpowers/specs/2026-11-10-worktree-dialog-design.md`

## Global Constraints

- Four sources, labels `New`, `Local`, `Remote`, `Open`. Menu entry passes `action: "remote"`. `action: "new" | "local" | "open"` still select those sources. Absent or unknown action (other than `remove`) is Remote.
- Checkout the branch itself. A branch already in `this.list` opens that checkout's space. Never pass `--force`.
- New only creates: `git worktree add -b <name> <path> HEAD`. An existing local branch returns `Branch <name> already exists.` and does not add.
- Local: `git worktree add <path> <branch>` when not checked out.
- Remote display name is `<remote>/<short>`. Short name is everything after the first `/`. Remote names contain no `/`. Skip refs whose short name is `HEAD`.
- If that local branch exists, a remote row uses it and does not retarget it. Otherwise `git worktree add --track -b <short> <path> <remote>/<short>`.
- `{branch}` with no chosen row uses `resolveBranch`: local rule, else the single matching remote, else New's create-from-HEAD rule.
- Path is `` `${ROOT(userHome)}/${repo.name}/${slug(branch)}` ``. `slug` stays as it is. `space.new` gets `tab` and `argv` only when a worktree was just added and those props are not `undefined`. Opening an existing checkout does not pass them.
- Dialog title stays `New worktree of <repo>`. Menu label stays `New worktree…`. Both non-remove dialogs are 560×480. Remove keeps its current size.
- Click switches source. Arrow left/right stay in the input. Lists load together before `this.repo` is set.
- No Rust changes. No Remove behavior changes. No custom path, no space name field.
- UI copy is English, matching the rest of the app.
- Commit messages: lowercase conventional commits, single line, no AI co-author trailer.

## Review Focus

- `origin/feature/conditional-mods` must create local branch `feature/conditional-mods`, not `feature`. Task 1 tests `place("remote", "feature/conditional-mods", "origin/feature/conditional-mods")`.
- Local `feature` already exists; choosing `upstream/feature` must run `git worktree add <path> feature` and must not pass `upstream/feature`. Task 1.
- A branch checked out on the main worktree, with no space yet, must `space.new` that main path and must not `worktree add`. Task 1.
- `{branch: "feature"}` with `origin/feature` and `upstream/feature` and no local branch must `git worktree add -b feature <path> HEAD`, not `--track`. Task 1.
- New with a name that exists only on a remote must still `add -b` from HEAD. `origin/HEAD` must not become a remote row. Task 1.

---

### Task 1: Place a branch

**Files:**
- Modify: `defaults/mods/worktree/main.js`
- Test: `scripts/worktree.test.ts`

**Interfaces:**
- Consumes: existing `git`, `slug`, `ROOT`, `open`, `checkouts`, `repo`, `on`, `userHome`, `list`.
- Produces:
  - `loadLists()` sets `this.list`, `this.branches` (`{name, at}`), `this.remotes` (`{remote, name, at}`).
  - `branchesFrom(out: string) -> {name, at}[]`, sorted by `at` descending.
  - `remotesFrom(out: string) -> {remote, name, at}[]`, sorted by `at` descending, `HEAD` removed.
  - `async place(kind: "new" | "local" | "remote", branch: string, remote?: string) -> Promise<string>`. `""` on success, otherwise the error to show.
  - `async resolveBranch(name: string) -> Promise<string>` for `{branch}`.
  - Checked-out means `this.list` has `branch === name` (main checkout included). Success on that row calls `open` and does not add.
  - New worktree `run("space.new", { folder, machine: this.on ?? "", ...tab/argv when defined })`.

- [ ] **Step 1: Extend the harness and write failing tests**

Change `harness` so the second argument is an exec implementation. Default remains `{ code: 0, stdout: "", stderr: "" }`. Record every call as today.

`show-ref` is the only command that may return code 1. `worktree add` returns code 0, or `place` would treat success as failure.

```javascript
const absent = async (_c, args) => ({ code: args[0] === "show-ref" ? 1 : 0, stdout: "", stderr: "" });

test("new refuses an existing local name", async () => {
  const { worktree, executions } = harness();
  Object.assign(worktree, { repo: { name: "repo", main: "/repo", top: "/repo" }, list: [] });
  expect(await worktree.place("new", "feature")).toBe("Branch feature already exists.");
  expect(executions.some((e) => e.args[0] === "worktree")).toBe(false);
});

test("new from HEAD uses -b when the local name is absent", async () => {
  const { worktree, executions, writes } = harness(null, absent);
  Object.assign(worktree, { repo: { name: "repo", main: "/repo", top: "/repo" }, on: "server", list: [] });
  expect(await worktree.place("new", "feature/foo")).toBe("");
  expect(executions.at(-1).args).toEqual(["worktree", "add", "-b", "feature/foo", "/home/user/.herdr/worktrees/repo/feature-foo", "HEAD"]);
  expect(executions.at(-1).on).toBe("server");
  expect(writes.at(-1)).toEqual(["space.new", { folder: "/home/user/.herdr/worktrees/repo/feature-foo", machine: "server" }]);
});

test("remote short name keeps slashes and tracks that remote", async () => {
  const { worktree, executions } = harness(null, absent);
  Object.assign(worktree, { repo: { name: "repo", main: "/repo", top: "/repo" }, list: [] });
  expect(await worktree.place("remote", "feature/conditional-mods", "origin/feature/conditional-mods")).toBe("");
  expect(executions.at(-1).args).toEqual(["worktree", "add", "--track", "-b", "feature/conditional-mods", "/home/user/.herdr/worktrees/repo/feature-conditional-mods", "origin/feature/conditional-mods"]);
});

test("an existing local branch wins over the clicked remote", async () => {
  const { worktree, executions } = harness(null, async () => ({ code: 0, stdout: "", stderr: "" }));
  Object.assign(worktree, { repo: { name: "repo", main: "/repo", top: "/repo" }, list: [] });
  expect(await worktree.place("remote", "feature", "upstream/feature")).toBe("");
  expect(executions.at(-1).args).toEqual(["worktree", "add", "/home/user/.herdr/worktrees/repo/feature", "feature"]);
});

test("a branch checked out on the main worktree opens that path", async () => {
  const { worktree, executions, writes } = harness();
  Object.assign(worktree, {
    repo: { name: "repo", main: "/repo", top: "/repo" },
    list: [{ path: "/repo", branch: "main", main: true, at: 0 }],
  });
  expect(await worktree.place("local", "main")).toBe("");
  expect(executions.some((e) => e.args[0] === "worktree")).toBe(false);
  expect(writes.at(-1)).toEqual(["space.new", { folder: "/repo", machine: "" }]);
});

test("resolveBranch creates from HEAD when two remotes share the name", async () => {
  const { worktree, executions } = harness(null, absent);
  Object.assign(worktree, {
    repo: { name: "repo", main: "/repo", top: "/repo" }, list: [],
    remotes: [{ remote: "origin", name: "feature", at: 2 }, { remote: "upstream", name: "feature", at: 1 }],
  });
  expect(await worktree.resolveBranch("feature")).toBe("");
  expect(executions.at(-1).args).toEqual(["worktree", "add", "-b", "feature", "/home/user/.herdr/worktrees/repo/feature", "HEAD"]);
});

test("resolveBranch tracks the only remote and opens an existing checkout", async () => {
  const created = harness(null, absent);
  Object.assign(created.worktree, {
    repo: { name: "repo", main: "/repo", top: "/repo" }, list: [],
    remotes: [{ remote: "origin", name: "feature", at: 1 }],
  });
  expect(await created.worktree.resolveBranch("feature")).toBe("");
  expect(created.executions.at(-1).args[1]).toBe("add");
  expect(created.executions.at(-1).args).toContain("origin/feature");

  const opened = harness();
  Object.assign(opened.worktree, {
    repo: { name: "repo", main: "/repo", top: "/repo" },
    list: [{ path: "/repo/wt", branch: "feature", at: 1 }],
    remotes: [{ remote: "origin", name: "feature", at: 1 }],
  });
  opened.snapshot([{ id: "s", tabs: [{ id: "t" }] }], [{ tab: "t", cwd: "/repo/wt" }]);
  expect(await opened.worktree.resolveBranch("feature")).toBe("");
  expect(opened.writes).toEqual([["space.select", { id: "s" }]]);
});

test("loadLists drops origin/HEAD and keeps nested remote names", async () => {
  const { worktree } = harness(null, async (_c, args) => {
    const ref = args.at(-1);
    if (ref === "refs/remotes") return { code: 0, stdout: "origin/HEAD\t1\norigin/feature/foo\t2\n", stderr: "" };
    if (ref === "refs/heads") return { code: 0, stdout: "feature/foo\t3\n", stderr: "" };
    return { code: 0, stdout: "", stderr: "" };
  });
  Object.assign(worktree, { repo: { name: "repo", main: "/repo", top: "/repo" } });
  await worktree.loadLists();
  expect(worktree.remotes).toEqual([{ remote: "origin", name: "feature/foo", at: 2 }]);
  expect(worktree.branches).toEqual([{ name: "feature/foo", at: 3 }]);
});
```

Update the existing machine-routing loop to pass `absent`, call `place("new", "feature")`, and expect `["worktree", "add", "-b", "feature", "/home/user/.herdr/worktrees/repo/feature", "HEAD"]` plus the same `on` and the same two destinations after `open`. Do not leave that loop on `create()`: today's `create()` checks out a name when `show-ref` succeeds.

Empty `place("new", "  ")` returns `Type a branch name.` and does not exec `worktree`.

- [ ] **Step 2: Run the new tests**

Run: `bun test scripts/worktree.test.ts`

Expected: FAIL because `place`, `resolveBranch`, or `loadLists` is missing.

- [ ] **Step 3: Implement placement in `defaults/mods/worktree/main.js`**

`localExists(name)` is `git show-ref --verify --quiet refs/heads/<name>` against `repo.main`.

`place` follows the Global Constraints. `git worktree add` keeps cwd `repo.top`, timeout `600_000`, and `on`. Failure returns `r.err || "git worktree add failed."`.

`loadLists` uses the existing `checkouts` plus:

- `for-each-ref --format=%(refname:short)%09%(committerdate:unix) refs/heads`
- the same format for `refs/remotes`

`init` awaits `loadLists` whenever the action is not `remove`, still assigning `this.repo` last. `{branch}` still calls `resolveBranch` after that load, not `place("new")` blindly.

- [ ] **Step 4: Re-run the tests**

Run: `bun test scripts/worktree.test.ts`

Expected: PASS, including the previous open-list and machine tests.

- [ ] **Step 5: Commit**

```bash
git add defaults/mods/worktree/main.js scripts/worktree.test.ts
git commit -m "feat: place a worktree on an existing branch"
```

---

### Task 2: Four-source dialog

**Files:**
- Modify: `defaults/mods/worktree/main.js`
- Test: `scripts/worktree.test.ts`

**Interfaces:**
- Consumes: Task 1 `place`, `branches`, `remotes`, `list`, `open`.
- Produces:
  - `this.source` from `props().action`: `new`, `local`, `open`, or else `remote`. `remove` unchanged.
  - `this.query` is `{ new, local, remote, open }`, each a string. The input shows `this.query[this.source]`.
  - `shownBranches()`, `shownRemotes()`, `shown()` (open, unchanged filter).
  - Enter on New calls `place("new", query.new.trim())`. Enter or click on a local or remote row calls `place` with that row. Open's enter and click stay on `open`.
  - Switching source sets `at` to 0 and does not clear the other queries.

- [ ] **Step 1: Write failing render tests**

Harness default becomes `source: "remote"`. Existing open-list tests set `source: "open"` and still find `wt-N` rows, branch names, and `open`.

```javascript
test("remote is the default source and shows full ref names", () => {
  const { worktree } = harness();
  Object.assign(worktree, {
    source: "remote", repo: { name: "repo" }, userHome: "/home/user",
    remotes: [{ remote: "origin", name: "feature/foo", at: 1 }],
    list: [], query: { new: "", local: "", remote: "", open: "" }, at: 0,
  });
  const text = textOf(worktree.render({}).node);
  expect(text).toContain("New worktree of repo");
  expect(text).toContain("origin/feature/foo");
  expect(text).toContain("A branch already checked out opens that space.");
  expect(text).toContain("↵ checkout");
});

test("open badge appears only when a space shows that checkout", () => {
  const { worktree, snapshot } = harness();
  const checkout = { path: "/repo/wt", branch: "feature", at: 1 };
  Object.assign(worktree, {
    source: "local", repo: { name: "repo" }, userHome: "/home/user", on: null,
    branches: [{ name: "feature", at: 1 }, { name: "other", at: 0 }],
    list: [checkout, { path: "/repo/bare", branch: "other", at: 0 }],
    query: { new: "", local: "", remote: "", open: "" }, at: 0,
  });
  snapshot([{ id: "s", tabs: [{ id: "t" }] }], [{ tab: "t", cwd: "/repo/wt" }]);
  const text = textOf(worktree.render({}).node);
  expect(text).toContain("feature");
  expect(text).toContain("open");
  const other = rowsOf(worktree.render({}).node).find((row) => textOf(row).includes("other"));
  expect(textOf(other)).not.toContain("open");
  expect(text).toContain("↵ open");
});

test("new source explains a branch from here", () => {
  const { worktree } = harness();
  Object.assign(worktree, {
    source: "new", repo: { name: "repo" }, userHome: "/home/user",
    query: { new: "", local: "", remote: "", open: "" },
  });
  const text = textOf(worktree.render({}).node);
  expect(text).toContain("A new branch starting from here, checked out in ~/.herdr/worktrees/repo/.");
  expect(text).toContain("↵ create · esc cancel");
  expect(text).not.toContain("origin/");
});
```

Exact detail strings:

- New: `A new branch starting from here, checked out in ${tilde(ROOT(home), home)}/${repo}/.`
- Local and Remote: `A branch already checked out opens that space. Otherwise it is checked out in ${tilde(ROOT(home), home)}/${repo}/.`
- Placeholders: New `branch name`, Local and Remote `filter branches`, Open `filter worktrees`.
- Local and Remote filters match today's open list: case-insensitive `includes` on the displayed name (`feature` matches `origin/feature/foo`). A filter that excludes every row says `No matching branches.`
- Footer: New `↵ create · esc cancel`. Selected row that will open: `↑↓ choose · ↵ open · esc cancel`. Selected row that will add: `↑↓ choose · ↵ checkout · esc cancel`. No rows: `esc cancel`.
- Empty copy: `No remote branches.` / `No local branches.` when the unfiltered list is empty. `No matching branches.` when a filter excludes them. Open keeps `No matching worktrees.`

- [ ] **Step 2: Run the tests**

Run: `bun test scripts/worktree.test.ts`

Expected: FAIL on the new source assertions.

- [ ] **Step 3: Render the four sources**

One dialog shell for every non-remove source: title, four source labels, detail, input, list (except New), error, footer. Open no longer uses the title `Open worktree`. `render` branches on `source === "open"` only for the list contents, not a second dialog.

`init` sets `source` and `query`, focuses the input for every non-remove source, and still focuses the root while loading. Map the input `change` onto `this.query[this.source]` only.

`key`: Esc closes. Up/down/enter apply to local, remote, and open. Enter on New calls `go`. Left and right are untouched.

- [ ] **Step 4: Re-run the tests**

Run: `bun test scripts/worktree.test.ts`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add defaults/mods/worktree/main.js scripts/worktree.test.ts
git commit -m "feat: show four sources in the worktree dialog"
```

---

### Task 3: One menu entry and the agent guide

**Files:**
- Modify: `defaults/mods/menu/main.js` (the space items and the overlay size near the `worktree` command)
- Modify: `defaults/mods/new-menu/main.js` (the two worktree rows)
- Modify: `defaults/AGENTS.md` (the `worktree` mod paragraph)
- Test: `scripts/worktree.test.ts`

**Interfaces:**
- Consumes: dialog `action` values from Task 2.
- Produces: one menu row, `New worktree…` in the + menu and `New worktree` in the space menu, both `action: "remote"`, both overlays 560×480. Remove stays, and its overlay stays 440×260.

- [ ] **Step 1: Write a failing source test**

```javascript
test("menus and the guide expose one worktree dialog", async () => {
  const menu = await readFile(new URL("../defaults/mods/menu/main.js", import.meta.url), "utf8");
  const plus = await readFile(new URL("../defaults/mods/new-menu/main.js", import.meta.url), "utf8");
  const guide = await readFile(new URL("../defaults/AGENTS.md", import.meta.url), "utf8");
  expect(menu.match(/New worktree/g)).toHaveLength(1);
  expect(menu).not.toContain("Open worktree");
  expect(menu).toContain('action: "remote"');
  expect(plus).not.toContain("worktree-open");
  expect(plus).toContain('action: "remote"');
  expect(plus).toContain("width: 560, height: 480");
  expect(guide).toContain('action: "new" | "local" | "remote" | "open" | "remove"');
  expect(guide).not.toContain("Open worktree");
});
```

- [ ] **Step 2: Run it**

Run: `bun test scripts/worktree.test.ts`

Expected: FAIL because both menus still contain Open worktree.

- [ ] **Step 3: Update the menus and `defaults/AGENTS.md`**

Space menu: a git repo gets `New worktree` with `action: "remote"` only. Linked worktrees still get Remove worktree.

+ menu: delete the Open worktree row. The remaining row opens 560×480 with `action: "remote"`.

Space-menu overlay: `action === "remove"` keeps 440×260. Every other worktree action uses 560×480.

Replace the worktree sentence in `defaults/AGENTS.md` with: the space menu and the + menu offer New worktree (four sources: New, Local, Remote, Open; the menu opens Remote; a branch already checked out opens its space) and Remove worktree. Props are `{space, folder?, action: "new" | "local" | "remote" | "open" | "remove"}`, or `{branch, tab?, argv?}` to place that branch at once. Checkouts still go to `~/.herdr/worktrees/<repo>/<branch>`.

- [ ] **Step 4: Re-run the tests**

Run: `bun test scripts/worktree.test.ts`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add defaults/mods/menu/main.js defaults/mods/new-menu/main.js defaults/AGENTS.md scripts/worktree.test.ts
git commit -m "feat: open the worktree dialog from one menu entry"
```
