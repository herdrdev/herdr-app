import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const source = (await readFile(new URL("../defaults/mods/worktree/main.js", import.meta.url), "utf8"))
  .replace(/^import .*;\r?\n/gm, "").replace("export default class Worktree", "class Worktree");

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

function harness(focusedMachine: string | null = null, raw: string | null = null) {
  let spaceList: any[] = [];
  let paneList: any[] = [];
  const reads = { spaces: 0, panes: 0 };
  const writes: unknown[] = [];
  const destinations: (string | null)[] = [];
  const executions: any[] = [];
  const globals = {
    View: class {}, div: element, v_flex: element, h_flex: element,
    Input: { new: element }, Scrollbar: { vertical: element }, theme: () => ({}),
    appHome: () => "/app", readTextFile: async () => raw,
    spaces: () => { reads.spaces++; return structuredClone(spaceList); },
    panes: () => { reads.panes++; return structuredClone(paneList); },
    props: () => ({ overlay: "overlay" }), closeOverlay: () => {},
    run: (command: string, args: any) => {
      writes.push([command, args]);
      if (command === "space.new") destinations.push(args.machine === "" ? null : args.machine ?? focusedMachine);
    },
    exec: async (command: string, args: string[], options: any) => {
      executions.push({ command, args, ...options });
      return { code: 0, stdout: "", stderr: "" };
    },
  };
  const { Worktree, root } = new Function(...Object.keys(globals), `${source}; return { Worktree, root };`)(...Object.values(globals));
  const worktree = new Worktree();
  Object.assign(worktree, { opening: true, text: "", at: 0, repo: { name: "repo" }, userHome: "/home/user", root: "/home/user/.herdr/worktrees" });
  return { worktree, root, reads, writes, destinations, executions, snapshot: (spaces: unknown[], panes: unknown[]) => { spaceList = spaces; paneList = panes; } };
}

test("open render reads spaces and panes once and refreshes both on the next render", () => {
  const { worktree, reads, snapshot } = harness();
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
      const { worktree, destinations, executions } = harness(focused);
      Object.assign(worktree, {
        on, text: "feature", repo: { name: "repo", main: "/repo", top: "/repo" }, list: [],
      });
      expect(await worktree.create()).toBe("");
      expect(executions).toHaveLength(2);
      expect(executions.every(e => (e.on ?? null) === (on ?? null))).toBe(true);
      expect(executions[1].args).toEqual(["worktree", "add", "/home/user/.herdr/worktrees/repo/feature", "feature"]);
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

test("configured roots drive both the checkout and its space, expanding the target home", async () => {
  for (const [directory, userHome, on, expected] of [
    [undefined, "/home/user", null, "/home/user/.herdr/worktrees"],
    ["~/Developer/worktrees", "/home/remote", "server", "/home/remote/Developer/worktrees"],
    ["/srv/worktrees/", "/home/remote", "server", "/srv/worktrees"],
    ["C:\\Developer\\Work Trees\\", "C:\\Users\\user", null, "C:/Developer/Work Trees"],
    ["~\\Developer\\worktrees", "C:\\Users\\user", "server", "C:/Users/user/Developer/worktrees"],
    ["~", "/home/user", null, "/home/user"],
    ["/", "/home/user", null, ""],
  ] as const) {
    const { worktree, root, executions, writes } = harness(null, JSON.stringify({ worktrees: { directory } }));
    Object.assign(worktree, { opening: false, on, userHome, text: "feature/new", repo: { name: "repo", main: "/repo", top: "/repo" }, root: await root(userHome) });
    const folder = `${expected}/repo/feature-new`;
    expect(await worktree.create()).toBe("");
    expect(executions[1].args).toEqual(["worktree", "add", folder, "feature/new"]);
    expect(executions[1].on).toBe(on);
    expect(writes).toEqual([["space.new", { folder, machine: on ?? "", tab: undefined, argv: undefined }]]);
    expect(textOf(worktree.render({}).node)).toContain(`${expected.replace(userHome.replace(/\\/g, "/"), "~")}/repo/`);
  }
});

test("missing settings keep the old location, while invalid directories cannot create elsewhere", async () => {
  expect(await harness().root("/home/user")).toBe("/home/user/.herdr/worktrees");
  for (const directory of ["", "   ", 42, {}, "relative/worktrees", "C:relative"]) {
    const { root } = harness(null, JSON.stringify({ worktrees: { directory } }));
    await expect(root("/home/user")).rejects.toThrow("worktrees.directory");
  }
  await expect(harness(null, "{").root("/home/user")).rejects.toThrow();
});
