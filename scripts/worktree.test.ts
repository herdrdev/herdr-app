import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const source = (await readFile(new URL("../defaults/mods/worktree/main.js", import.meta.url), "utf8"))
  .replace(/^import .*;\n/gm, "").replace("export default class Worktree", "class Worktree");

// Execute conditional builders and retain text, without native GPUI layout.
function element() {
  const node = { id: "", children: [] as any[], handlers: {} as Record<string, Function> };
  const el: any = new Proxy(node, { get: (_, method) => {
    if (method === "node") return node;
    return (...args: any[]) => {
      if (method === "id") node.id = args[0];
      if (method === "child") node.children.push(args[0]?.node ?? args[0]);
      if (method === "children") node.children.push(...args[0].map((child: any) => child?.node ?? child));
      if (method === "when" && args[0]) args[1](el);
      if (method === "on_key_up" || method === "on_key_down" || method === "on_change") node.handlers[method] = args[0];
      return el;
    };
  } });
  return el;
}
function textOf(node: any): string {
  return typeof node === "string" ? node : (node?.children ?? []).map(textOf).join(" ");
}
function rowsOf(node: any): any[] {
  return node?.id?.match(/^wt-\d+$/) ? [node] : (node?.children ?? []).flatMap(rowsOf);
}

const absent = async (_c: unknown, args: string[]) => ({ code: args[0] === "show-ref" ? 1 : 0, stdout: "", stderr: "" });

function harness(focusedMachine: string | null = null, execImpl?: (command: string, args: string[], options: any) => Promise<any>) {
  let spaceList: any[] = [];
  let paneList: any[] = [];
  const reads = { spaces: 0, panes: 0 };
  const writes: unknown[] = [];
  const destinations: (string | null)[] = [];
  const executions: any[] = [];
  const propsValue: any = { overlay: "overlay" };
  const globals = {
    View: class {}, div: element, v_flex: element, h_flex: element,
    Checkbox: { new: () => element() },
    Input: { new: element },
    InputState: { new: (opts: any = {}) => ({ placeholder: opts.placeholder ?? "", _value: opts.value ?? "", value() { return this._value; }, set_value(v: string) { this._value = v; }, on() { return true; }, focus() {} }) },
    Scrollbar: { vertical: element }, theme: () => ({}),
    spaces: () => { reads.spaces++; return structuredClone(spaceList); },
    panes: () => { reads.panes++; return structuredClone(paneList); },
    props: () => propsValue, closeOverlay: () => {},
    run: (command: string, args: any) => {
      writes.push([command, args]);
      if (command === "space.new") destinations.push(args.machine === "" ? null : args.machine ?? focusedMachine);
    },
    exec: async (command: string, args: string[], options: any) => {
      executions.push({ command, args, ...options });
      return execImpl ? execImpl(command, args, options) : { code: 0, stdout: "", stderr: "" };
    },
  };
  const Worktree = new Function(...Object.keys(globals), `${source}; return Worktree;`)(...Object.values(globals));
  const worktree = new Worktree();
  Object.assign(worktree, { source: "remote", opening: false, text: "", at: 0, repo: { name: "repo" }, userHome: "/home/user", query: { new: "", local: "", remote: "", open: "" } });
  return { worktree, reads, writes, destinations, executions, props: propsValue, snapshot: (spaces: any[], panes: any[]) => { spaceList = spaces; paneList = panes; } };
}

test("open render reads spaces and panes once and refreshes both on the next render", () => {
  const { worktree, reads, snapshot } = harness();
  worktree.source = "open";
  worktree.list = Array.from({ length: 80 }, (_, i) => ({ path: `/repo/wt-${i}`, branch: `branch-${i}`, at: 0 }));
  const spaces = worktree.list.map((c: any, i: number) => ({ id: `s${i}`, tabs: [{ id: `tab${i}` }] }));
  const panes = worktree.list.map((c: any, i: number) => ({ tab: `tab${i}`, cwd: `${c.path}/src` }));
  snapshot(spaces, panes);
  const first = rowsOf(worktree.render({}).node);
  expect(reads).toEqual({ spaces: 1, panes: 1 });
  expect(first).toHaveLength(80);
  for (let i = 0; i < first.length; i++) {
    expect(textOf(first[i])).toContain(`branch-${i}`);
    expect(textOf(first[i])).toContain(" open ");
  }
  snapshot(spaces.slice(0, -1), panes.map((p: any, i: number) => i === 0 ? { ...p, cwd: "/elsewhere" } : p));
  const second = rowsOf(worktree.render({}).node);
  expect(reads).toEqual({ spaces: 2, panes: 2 });
  expect(textOf(second[0])).not.toContain(" open ");
  expect(textOf(second[79])).not.toContain(" open ");
  expect(textOf(second[1])).toContain(" open ");
});

test("open clicks read fresh snapshots once and select an existing space or create one", () => {
  const { worktree, reads, writes, snapshot } = harness();
  worktree.source = "open";
  const checkout = { path: "/repo/worktree", branch: "branch", at: 0 };
  worktree.list = [checkout];
  snapshot([], []);
  worktree.render({});
  snapshot([{ id: "existing", tabs: [{ id: "tab" }] }], [{ tab: "tab", cwd: checkout.path }]);
  worktree.open(checkout);
  expect(reads).toEqual({ spaces: 2, panes: 2 });
  expect(writes).toEqual([["space.select", { id: "existing" }]]);
  snapshot([], []);
  worktree.open(checkout);
  expect(reads).toEqual({ spaces: 3, panes: 3 });
  expect(writes[1]).toEqual(["space.new", { folder: checkout.path, machine: "" }]);
  worktree.open(undefined);
  expect(reads).toEqual({ spaces: 3, panes: 3 });
});

for (const on of [undefined, null, "server"]) {
  for (const focused of [null, "server"]) {
    test(`create and open stay on ${on ?? "local"} while ${focused ?? "local"} is focused`, async () => {
      const { worktree, destinations, executions } = harness(focused, absent);
      Object.assign(worktree, {
        on, text: "feature", repo: { name: "repo", main: "/repo", top: "/repo" }, list: [],
      });
      expect(await worktree.place("new", "feature")).toBe("");
      expect(executions).toHaveLength(2);
      expect(executions.every(e => (e.on ?? null) === (on ?? null))).toBe(true);
      expect(executions[1].args).toEqual(["worktree", "add", "-b", "feature", "/home/user/.herdr/worktrees/repo/feature", "HEAD"]);
      worktree.open({ path: "/repo/existing" });
      expect(destinations).toEqual([on ?? null, on ?? null]);
    });
  }
}

test("space matching keeps deepest checkout, machine, live-folder and fallback rules", () => {
  const { worktree, reads, writes, snapshot } = harness();
  const paths = ["C:/repo", "C:/repo/nested", "C:/repo/fallback", "C:/repo-other"];
  worktree.list = paths.map(path => ({ path }));
  snapshot([
    { id: "remote", machine: "server", folder: paths[0] },
    { id: "nested", folder: paths[0], tabs: [{ id: "tab" }] },
    { id: "fallback", folder: paths[2] },
    { id: "other", folder: paths[3] },
  ], [
    { tab: "tab" },
    { tab: "tab", cwd: "C:\\repo\\nested\\src" },
    { tab: "tab", cwd: paths[0] },
  ]);
  for (const checkout of worktree.list) worktree.open(checkout);
  expect(writes).toEqual([
    ["space.new", { folder: paths[0], machine: "" }],
    ["space.select", { id: "nested" }],
    ["space.select", { id: "fallback" }],
    ["space.select", { id: "other" }],
  ]);
  worktree.on = "server";
  worktree.open(worktree.list[0]);
  expect(writes[4]).toEqual(["space.select", { id: "remote" }]);
  expect(reads).toEqual({ spaces: 5, panes: 5 });
});

test("new refuses an existing local name", async () => {
  const { worktree, executions } = harness();
  Object.assign(worktree, { repo: { name: "repo", main: "/repo", top: "/repo" }, list: [] });
  expect(await worktree.place("new", "feature")).toBe("Branch feature already exists.");
  expect(executions.some((e) => e.args[0] === "worktree")).toBe(false);
});

test("new with a blank name asks for a branch name", async () => {
  const { worktree, executions } = harness();
  Object.assign(worktree, { repo: { name: "repo", main: "/repo", top: "/repo" }, list: [] });
  expect(await worktree.place("new", "  ")).toBe("Type a branch name.");
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

test("remote filter is case-insensitive and reports no matches", () => {
  const { worktree } = harness();
  Object.assign(worktree, {
    source: "remote", repo: { name: "repo" }, userHome: "/home/user",
    remotes: [{ remote: "origin", name: "feature/foo", at: 1 }],
    list: [], query: { new: "", local: "", remote: "FOO", open: "" }, at: 0,
  });
  expect(textOf(worktree.render({}).node)).toContain("origin/feature/foo");
  worktree.query.remote = "zzz";
  const missed = textOf(worktree.render({}).node);
  expect(missed).not.toContain("origin/feature/foo");
  expect(missed).toContain("No matching branches.");
});

test("an empty remote list says there are no remote branches", () => {
  const { worktree } = harness();
  Object.assign(worktree, {
    source: "remote", repo: { name: "repo" }, userHome: "/home/user",
    remotes: [], list: [], query: { new: "", local: "", remote: "", open: "" }, at: 0,
  });
  const text = textOf(worktree.render({}).node);
  expect(text).toContain("No remote branches.");
  expect(text).toContain("esc cancel");
  expect(text).not.toContain("↵ checkout");
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

function removal(execImpl: (command: string, args: string[]) => Promise<any>) {
  const harnessed = harness(null, async (_c, args) => {
    if (args.includes("status")) return { code: 0, stdout: "", stderr: "" };
    if (args.includes("--absolute-git-dir")) return { code: 0, stdout: "/repo/.git", stderr: "" };
    if (args.includes("config")) return { code: 1, stdout: "", stderr: "" };
    return execImpl(args[0] === "git" ? args[0] : "git", args);
  });
  harnessed.props.space = "s1";
  harnessed.props.action = "remove";
  harnessed.snapshot([{ id: "s1", name: "Feature", tabs: [{ id: "t" }] }], []);
  Object.assign(harnessed.worktree, {
    on: "server", dirty: false, unpushed: [],
    repo: { name: "repo", main: "/repo", top: "/wt", linked: true },
    branch: "feature/foo", deleteBranch: true, userHome: "/home/user", source: "remove",
  });
  return harnessed;
}

test("remove deletes the local branch after the worktree when the box is checked", async () => {
  const { worktree, writes, executions } = removal(async (_c, args) => {
    if (args[0] === "worktree" || args[0] === "branch") return { code: 0, stdout: "", stderr: "" };
    return { code: 0, stdout: "", stderr: "" };
  });
  const shown = textOf(worktree.render({}).node);
  expect(shown).toContain("Delete local branch feature/foo");
  expect(shown).toContain("\u2713");
  expect(await worktree.remove()).toBe("");
  expect(writes).toEqual([["space.close", { id: "s1", confirmed: true }]]);
  const removed = executions.find((e) => e.args[0] === "worktree");
  const deleted = executions.find((e) => e.args[0] === "branch");
  expect(removed.args).toEqual(["worktree", "remove", "/wt"]);
  expect(removed).toMatchObject({ cwd: "/repo", on: "server" });
  expect(deleted.args).toEqual(["branch", "-d", "feature/foo"]);
  expect(deleted.cwd).toBe("/repo");
  expect(executions.indexOf(removed)).toBeLessThan(executions.indexOf(deleted));
});

test("enter keeps a checked box that the same key just toggled off", async () => {
  const { worktree, executions } = removal(async () => ({ code: 0, stdout: "", stderr: "" }));
  worktree.deleteBranch = false;
  worktree.branchKeyFlip = true;
  await worktree.key({ key: "enter" }, { spawn: (fn) => fn({ notify() {} }), notify() {}, stop_propagation() {} });
  expect(worktree.deleteBranch).toBe(true);
  expect(worktree.branchHeld).toBe(true);
  expect(executions.find((e) => e.args[0] === "branch")?.args).toEqual(["branch", "-d", "feature/foo"]);
});

test("enter moves focus off the checkbox so keyup cannot uncheck it", async () => {
  const { worktree, executions } = removal(async () => ({ code: 0, stdout: "", stderr: "" }));
  let focused = 0;
  worktree.handle = { focus() { focused += 1; } };
  worktree.deleteBranch = true;
  await worktree.key({ key: "enter" }, { spawn: (fn) => fn({ notify() {} }), notify() {}, stop_propagation() {} });
  expect(focused).toBe(1);
  expect(worktree.deleteBranch).toBe(true);
  expect(worktree.branchHeld).toBe(true);
  let painted = 0;
  worktree.setBranchChecked(false, { notify() { painted += 1; }, timer: { after() {} } });
  expect(worktree.deleteBranch).toBe(true);
  expect(painted).toBe(0);
  expect(executions.find((e) => e.args[0] === "branch")?.args).toEqual(["branch", "-d", "feature/foo"]);
});

test("keyup of the confirming enter lets the box be toggled again", () => {
  const { worktree } = removal(async () => ({ code: 0, stdout: "", stderr: "" }));
  worktree.branchHeld = true;
  worktree.deleteBranch = true;
  worktree.handle = { focus() {} };
  const node = worktree.render({}).node;
  node.handlers.on_key_up({ key: "enter" });
  expect(worktree.branchHeld).toBe(false);
  let painted = 0;
  worktree.setBranchChecked(false, { notify() { painted += 1; }, timer: { after() {} } });
  expect(worktree.deleteBranch).toBe(false);
  expect(painted).toBe(1);
});

test("enter after the box has settled does not put the check back", async () => {
  const { worktree, executions } = removal(async () => ({ code: 0, stdout: "", stderr: "" }));
  worktree.deleteBranch = false;
  worktree.branchKeyFlip = false;
  await worktree.key({ key: "enter" }, { spawn: (fn) => fn({ notify() {} }), notify() {}, stop_propagation() {} });
  expect(executions.some((e) => e.args[0] === "branch")).toBe(false);
});

test("remove keeps the local branch when the box is unchecked", async () => {
  const { worktree, executions } = removal(async () => ({ code: 0, stdout: "", stderr: "" }));
  worktree.deleteBranch = false;
  expect(textOf(worktree.render({}).node)).not.toContain("\u2713");
  expect(await worktree.remove()).toBe("");
  expect(executions.some((e) => e.args[0] === "branch")).toBe(false);
});

test("a failed worktree remove does not delete the branch", async () => {
  const { worktree, executions } = removal(async (_c, args) => (
    args[0] === "worktree" ? { code: 128, stdout: "", stderr: "fatal: worktree locked" } : { code: 0, stdout: "", stderr: "" }
  ));
  expect(await worktree.remove()).toBe("fatal: worktree locked");
  expect(executions.some((e) => e.args[0] === "branch")).toBe(false);
});

test("a refused branch -d stays on screen and the next enter uses -D", async () => {
  const calls: string[][] = [];
  const { worktree, writes, executions } = removal(async (_c, args) => {
    if (args[0] === "branch") calls.push(args);
    if (args[0] === "branch" && args[1] === "-d") return { code: 1, stdout: "", stderr: "error: not fully merged" };
    return { code: 0, stdout: "", stderr: "" };
  });
  expect(await worktree.remove()).toBe("error: not fully merged");
  expect(worktree.branchRefused).toBe("feature/foo");
  worktree.error = "error: not fully merged";
  const text = textOf(worktree.render({}).node);
  expect(text).toContain("error: not fully merged");
  expect(text).toContain("Delete local branch feature/foo anyway?");
  expect(text).toContain("↵ delete anyway · esc keep the branch");
  expect(await worktree.remove()).toBe("");
  expect(calls).toEqual([["branch", "-d", "feature/foo"], ["branch", "-D", "feature/foo"]]);
  expect(writes).toEqual([["space.close", { id: "s1", confirmed: true }]]);
  expect(executions.filter((e) => e.args[0] === "worktree")).toHaveLength(1);
});

test("a refused -D stays on screen and does not remove the worktree again", async () => {
  const { worktree, executions } = removal(async (_c, args) => (
    args[0] === "branch" ? { code: 1, stdout: "", stderr: args[1] === "-d" ? "error: not fully merged" : "error: cannot force delete" } : { code: 0, stdout: "", stderr: "" }
  ));
  expect(await worktree.remove()).toBe("error: not fully merged");
  expect(await worktree.remove()).toBe("error: cannot force delete");
  expect(executions.filter((e) => e.args[0] === "worktree")).toHaveLength(1);
  expect(executions.filter((e) => e.args[0] === "branch").map((e) => e.args[1])).toEqual(["-d", "-D"]);
});

test("a detached checkout has no delete-branch box", () => {
  const { worktree } = harness();
  Object.assign(worktree, {
    source: "remove", userHome: "/home/user", branch: "", deleteBranch: false, dirty: false, unpushed: [],
    repo: { name: "repo", main: "/repo", top: "/wt", linked: true },
  });
  expect(textOf(worktree.render({}).node)).not.toContain("Delete local branch");
});

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

test("loadLists drops origin/HEAD and keeps nested remote names", async () => {
  const { worktree, executions } = harness(null, async (_c, args) => {
    const ref = args.at(-1);
    if (ref === "refs/remotes") return { code: 0, stdout: "origin/HEAD\t1\norigin/feature/foo\t2\n", stderr: "" };
    if (ref === "refs/heads") return { code: 0, stdout: "feature/foo\t3\n", stderr: "" };
    return { code: 0, stdout: "", stderr: "" };
  });
  const notes: string[] = [];
  Object.assign(worktree, { on: "server", repo: { name: "repo", main: "/repo", top: "/repo" } });
  await worktree.loadLists(worktree.repo, { notify() { notes.push(worktree.loading); } });
  const fetchAt = executions.findIndex((e) => e.args[0] === "fetch");
  const remotesAt = executions.findIndex((e) => e.args.at(-1) === "refs/remotes");
  expect(fetchAt).toBeGreaterThanOrEqual(0);
  expect(fetchAt).toBeLessThan(remotesAt);
  expect(executions[fetchAt]).toMatchObject({
    command: "git",
    args: ["fetch", "--all", "--prune", "--no-tags", "--no-recurse-submodules"],
    cwd: "/repo",
    on: "server",
    timeout: 120000,
  });
  expect(notes).toContain("Fetching remote branches…");
  expect(worktree.loading).toBe("");
  expect(worktree.fetchError).toBe("");
  expect(worktree.remotes).toEqual([{ remote: "origin", name: "feature/foo", at: 2 }]);
  expect(worktree.branches).toEqual([{ name: "feature/foo", at: 3 }]);
});

test("a failed fetch stays visible and the remote-tracking refs are still listed", async () => {
  const { worktree } = harness(null, async (_c, args) => {
    if (args[0] === "fetch") return { code: 128, stdout: "", stderr: "could not resolve host" };
    if (args.at(-1) === "refs/remotes") return { code: 0, stdout: "origin/old\t4\n", stderr: "" };
    return { code: 0, stdout: "", stderr: "" };
  });
  Object.assign(worktree, { repo: { name: "repo", main: "/repo", top: "/repo" }, source: "remote", userHome: "/home/user" });
  await worktree.loadLists();
  expect(worktree.fetchError).toBe("could not resolve host");
  expect(worktree.remotes).toEqual([{ remote: "origin", name: "old", at: 4 }]);
  expect(textOf(worktree.render({}).node)).toContain("could not resolve host");
  worktree.selectSource("local");
  expect(textOf(worktree.render({}).node)).not.toContain("could not resolve host");
});

test("a fetch that cannot start does not discard the dialog's remote list", async () => {
  const { worktree } = harness(null, async (_c, args) => {
    if (args[0] === "fetch") throw new Error("timed out");
    if (args.at(-1) === "refs/remotes") return { code: 0, stdout: "origin/old\t4\n", stderr: "" };
    return { code: 0, stdout: "", stderr: "" };
  });
  Object.assign(worktree, { repo: { name: "repo", main: "/repo", top: "/repo" } });
  await worktree.loadLists();
  expect(worktree.fetchError).toBe("timed out");
  expect(worktree.remotes).toEqual([{ remote: "origin", name: "old", at: 4 }]);
});

// Enter is delivered twice: the field submits, and the key bubbles to the dialog.
const fieldCx = () => ({ notify() {}, stop_propagation() {}, spawn(body: any) { return body(fieldCx()); } });

test("enter on an open row creates one space when the field also submits", async () => {
  const { worktree, writes } = harness();
  Object.assign(worktree, {
    source: "open", repo: { name: "repo" }, busy: false, at: 0,
    list: [{ path: "/repo", branch: "main", at: 0 }],
  });
  const cx = fieldCx();
  worktree.key({ key: "enter" }, cx);
  await worktree.go(cx);
  expect(writes.filter((w) => w[0] === "space.new")).toEqual([["space.new", { folder: "/repo", machine: "" }]]);
});

test("switching source replaces the placeholder", () => {
  const { worktree } = harness();
  worktree.query = { new: "mine", local: "loc", remote: "", open: "op" };
  worktree.selectSource("new");
  expect(worktree.input.placeholder).toBe("branch name");
  expect(worktree.input.value()).toBe("mine");
  worktree.selectSource("open");
  expect(worktree.input.placeholder).toBe("filter worktrees");
  expect(worktree.input.value()).toBe("op");
  worktree.selectSource("local");
  expect(worktree.input.placeholder).toBe("filter branches");
});

test("a later enter follows the source after an immediate branch", async () => {
  const { worktree, executions, props } = harness(null, absent);
  props.branch = "feature";
  Object.assign(worktree, {
    source: "new", repo: { name: "repo", main: "/repo", top: "/repo" }, busy: false,
    list: [], remotes: [{ remote: "origin", name: "feature", at: 1 }],
    text: "feature", query: { new: "feature", local: "", remote: "", open: "" },
  });
  await worktree.work({ notify() {} });
  expect(executions.at(-1).args).toEqual(["worktree", "add", "-b", "feature", "/home/user/.herdr/worktrees/repo/feature", "HEAD"]);
});

test("the opening branch still resolves before later enters follow the source", async () => {
  const { worktree, executions, props } = harness(null, absent);
  props.branch = "feature";
  Object.assign(worktree, {
    source: "new", repo: { name: "repo", main: "/repo", top: "/repo" }, busy: false,
    list: [], remotes: [{ remote: "origin", name: "feature", at: 1 }], text: "feature",
  });
  await worktree.work({ notify() {} }, true);
  expect(executions.at(-1).args).toEqual(["worktree", "add", "--track", "-b", "feature", "/home/user/.herdr/worktrees/repo/feature", "origin/feature"]);
});

test("a checkout with no space keeps the caller's tab and argv", () => {
  const { worktree, writes, props, snapshot } = harness();
  props.argv = ["claude"];
  props.tab = { name: "agent" };
  worktree.list = [{ path: "/repo", branch: "main" }];
  worktree.open(worktree.list[0]);
  expect(writes).toEqual([["space.new", { folder: "/repo", machine: "", tab: { name: "agent" }, argv: ["claude"] }]]);
  snapshot([{ id: "existing", tabs: [{ id: "tab" }] }], [{ tab: "tab", cwd: "/repo" }]);
  worktree.open(worktree.list[0]);
  expect(writes[1]).toEqual(["space.select", { id: "existing" }]);
});
