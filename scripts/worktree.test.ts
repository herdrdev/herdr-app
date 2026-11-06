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

function harness(options: { props?: Record<string, unknown>; exec?: (...args: any[]) => any } = {}) {
  let spaceList: any[] = [];
  let paneList: any[] = [];
  const reads = { spaces: 0, panes: 0 };
  const writes: unknown[] = [];
  const closed: unknown[] = [];
  const notes: unknown[] = [];
  const globals = {
    View: class {}, div: element, v_flex: element, h_flex: element,
    Input: { new: element }, Scrollbar: { vertical: element }, theme: () => ({}),
    spaces: () => { reads.spaces++; return structuredClone(spaceList); },
    panes: () => { reads.panes++; return structuredClone(paneList); },
    props: () => ({ overlay: "overlay", ...(options.props ?? {}) }), closeOverlay: (id: unknown) => closed.push(id),
    run: (...args: unknown[]) => writes.push(args), notify: (note: unknown) => notes.push(note),
    home: () => "/home/user",
    exec: options.exec ?? (async () => ({ code: 0, stdout: "", stderr: "" })),
  };
  const Worktree = new Function(...Object.keys(globals), `${source}; return Worktree;`)(...Object.values(globals));
  const worktree = new Worktree();
  Object.assign(worktree, { opening: true, text: "", at: 0, repo: { name: "repo" }, userHome: "/home/user" });
  return { worktree, reads, writes, closed, notes, snapshot: (spaces: any[], panes: any[]) => { spaceList = spaces; paneList = panes; } };
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
  expect(writes[1]).toEqual(["space.new", { folder: checkout.path, machine: undefined }]);
  worktree.open(undefined);
  expect(reads).toEqual({ spaces: 3, panes: 3 });
});

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
    ["space.new", { folder: paths[0], machine: undefined }],
    ["space.select", { id: "nested" }],
    ["space.select", { id: "fallback" }],
    ["space.select", { id: "other" }],
  ]);
  worktree.on = "server";
  worktree.open(worktree.list[0]);
  expect(writes[4]).toEqual(["space.select", { id: "remote" }]);
  expect(reads).toEqual({ spaces: 5, panes: 5 });
});

test("remove closes the dialog and finishes the checkout without the dialog's copy", async () => {
  const calls: string[][] = [];
  const exec = async (_cmd: string, args: string[]) => {
    calls.push(args);
    if (args[0] === "rev-parse" && args[1] === "--absolute-git-dir") return { code: 0, stdout: "/repo/.git", stderr: "" };
    if (args.includes("--get-regexp")) return { code: 1, stdout: "", stderr: "" };
    return { code: 0, stdout: "", stderr: "" };
  };
  const { worktree, writes, closed, notes, snapshot } = harness({ props: { action: "remove", space: "s1" }, exec });
  worktree.repo = { top: "/repo/wt", main: "/repo", name: "repo", linked: true };
  worktree.dirty = false;
  worktree.unpushed = [];
  snapshot([{ id: "s1", folder: "/repo/wt", tabs: [] }], []);

  const spawned: { fn: () => Promise<void>; opts: unknown }[] = [];
  const cx = { notify() {}, spawn(fn: () => Promise<void>, opts: unknown) { spawned.push({ fn, opts }); return { cancel() {} }; } };
  await worktree.work(cx);

  expect(closed).toEqual(["overlay"]);
  expect(spawned).toHaveLength(1);
  expect(spawned[0].opts).toEqual({ owner: null });
  expect(writes).toEqual([]);

  await spawned[0].fn();
  expect(writes).toEqual([["space.close", { id: "s1", confirmed: true }]]);
  expect(calls.some((args) => args.includes("worktree") && args.includes("remove"))).toBe(true);
  expect(notes).toEqual([]);
});

test("a failed remove reports a toast after the dialog is gone", async () => {
  const exec = async (_cmd: string, args: string[]) => {
    if (args[0] === "rev-parse" && args[1] === "--absolute-git-dir") return { code: 0, stdout: "/repo/.git", stderr: "" };
    if (args.includes("--get-regexp")) return { code: 1, stdout: "", stderr: "" };
    if (args.includes("worktree") && args.includes("remove")) return { code: 1, stdout: "", stderr: "folder is busy" };
    return { code: 0, stdout: "", stderr: "" };
  };
  const { worktree, closed, notes, snapshot } = harness({ props: { action: "remove", space: "s1" }, exec });
  worktree.repo = { top: "/repo/wt", main: "/repo", name: "repo", linked: true };
  worktree.dirty = false;
  worktree.unpushed = [];
  snapshot([{ id: "s1", folder: "/repo/wt", tabs: [] }], []);

  const spawned: (() => Promise<void>)[] = [];
  const cx = { notify() {}, spawn(fn: () => Promise<void>) { spawned.push(fn); return { cancel() {} }; } };
  await worktree.work(cx);
  await spawned[0]();

  expect(closed).toEqual(["overlay"]);
  expect(notes).toEqual([{ title: "Remove worktree failed", body: "folder is busy", level: "error" }]);
});
