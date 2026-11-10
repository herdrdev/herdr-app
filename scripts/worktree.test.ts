import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const source = (await readFile(new URL("../defaults/mods/worktree/main.js", import.meta.url), "utf8"))
  .replace(/^import .*;\n/gm, "").replace("export default class Worktree", "class Worktree");

// Execute conditional builders and retain text, without native GPUI layout.
function element() {
  const node = { id: "", children: [] as any[] };
  const el: any = new Proxy(node, { get: (_, method) => {
    if (method === "node") return node;
    return (...args: any[]) => {
      if (method === "id") node.id = args[0];
      if (method === "child") node.children.push(args[0]?.node ?? args[0]);
      if (method === "children") node.children.push(...args[0].map((child: any) => child?.node ?? child));
      if (method === "when" && args[0]) args[1](el);
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
  const globals = {
    View: class {}, div: element, v_flex: element, h_flex: element,
    Input: { new: element }, Scrollbar: { vertical: element }, theme: () => ({}),
    spaces: () => { reads.spaces++; return structuredClone(spaceList); },
    panes: () => { reads.panes++; return structuredClone(paneList); },
    props: () => ({ overlay: "overlay" }), closeOverlay: () => {},
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
  return { worktree, reads, writes, destinations, executions, snapshot: (spaces: any[], panes: any[]) => { spaceList = spaces; paneList = panes; } };
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
