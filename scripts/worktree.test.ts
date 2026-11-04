import { test, expect } from "bun:test";
import { mkdtemp, mkdir, writeFile, readFile, readdir, realpath, rm, chmod } from "node:fs/promises";
import { existsSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";

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

function harness() {
  let spaceList: any[] = [];
  let paneList: any[] = [];
  const reads = { spaces: 0, panes: 0 };
  const writes: unknown[] = [];
  const globals = {
    View: class {}, div: element, v_flex: element, h_flex: element,
    Input: { new: element }, Scrollbar: { vertical: element }, theme: () => ({}),
    spaces: () => { reads.spaces++; return structuredClone(spaceList); },
    panes: () => { reads.panes++; return structuredClone(paneList); },
    props: () => ({ overlay: "overlay" }), closeOverlay: () => {},
    run: (...args: unknown[]) => writes.push(args),
  };
  const Worktree = new Function(...Object.keys(globals), `${source}; return Worktree;`)(...Object.values(globals));
  const worktree = new Worktree();
  Object.assign(worktree, { opening: true, text: "", at: 0, repo: { name: "repo" }, userHome: "/home/user" });
  return { worktree, reads, writes, snapshot: (spaces: any[], panes: any[]) => { spaceList = spaces; paneList = panes; } };
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

const part = (from: string, to: string) => source.slice(source.indexOf(from), source.indexOf(to));
const realRm = Bun.which("rm");

// What each exec saw: whether the checkout was still there, and what was next to it.
const calls: { argv: string[]; there: boolean; beside: string[] }[] = [];
let at = { top: "", bin: "" };
const removeAside = new Function("exec", `${part("async function git(", "\n// The submodule repos")}\n${part("async function removeAside(", "\n// The repo a folder")}; return removeAside;`)(
  async (command: string, args: string[] = [], options?: { cwd?: string }) => {
    calls.push({ argv: [command, ...args], there: existsSync(at.top), beside: readdirSync(dirname(at.top)) });
    const p = Bun.spawn([command, ...args], { cwd: options?.cwd, stdout: "pipe", stderr: "pipe", env: { ...process.env, PATH: `${at.bin}:${process.env.PATH}` } });
    const [code, stdout, stderr] = await Promise.all([p.exited, new Response(p.stdout).text(), new Response(p.stderr).text()]);
    return { code, stdout, stderr };
  },
);

function git(cwd: string, ...args: string[]) {
  const p = Bun.spawnSync(["git", "-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null", ...args], { cwd });
  if (p.exitCode !== 0) throw new Error(`git ${args.join(" ")}: ${p.stderr}`);
  return p.stdout.toString().trim();
}

// A repo with one linked worktree holding ignored dependencies, and a slow rm
// on PATH: if the background delete kept exec's pipes open, exec would wait for it.
async function checkout() {
  const root = await realpath(await mkdtemp(join(process.platform === "linux" ? "/var/tmp" : tmpdir(), "herdr-worktree-")));
  const main = join(root, "repo"), top = join(root, "feat"), bin = join(root, "bin");
  await mkdir(main);
  git(main, "init", "-q");
  await writeFile(join(main, ".gitignore"), "node_modules/\n");
  git(main, "add", ".gitignore");
  git(main, "commit", "-qm", "init");
  git(main, "worktree", "add", "-q", "-b", "feat", top, "HEAD");
  for (let i = 0; i < 20; i++) {
    await mkdir(join(top, "node_modules", `pkg${i}`), { recursive: true });
    await writeFile(join(top, "node_modules", `pkg${i}`, "index.js"), "");
  }
  await mkdir(bin);
  await writeFile(join(bin, "rm"), `#!/bin/sh\ntouch "${root}/slow-rm-ran"\nsleep 3\nexec "${realRm}" "$@"\n`);
  await chmod(join(bin, "rm"), 0o755);
  at = { top, bin };
  calls.length = 0;
  return { root, main, top };
}

const checkouts = (main: string) => git(main, "worktree", "list", "--porcelain").split("\n").filter((l) => l.startsWith("worktree "));
const leftovers = async (root: string) => (await readdir(root)).filter((name) => name.startsWith("feat."));

test.skipIf(process.platform === "win32")("Remove moves the checkout aside first, so git only forgets it and the files go in the background", async () => {
  const { root, main, top } = await checkout();
  try {
    const started = performance.now();
    expect(await removeAside(main, top, ["worktree", "remove", top])).toBe(true);
    expect(performance.now() - started).toBeLessThan(2000);
    expect(calls.find((c) => c.argv.join(" ").includes("worktree remove"))?.there).toBe(false);
    // Next to itself, so a rename on the same disk rather than a copy.
    expect(calls.find((c) => c.argv.join(" ").includes("rm -rf"))?.beside.filter((name) => name.startsWith("feat.removing-"))).toHaveLength(1);
    expect(checkouts(main)).toEqual([`worktree ${main}`]);
    expect(existsSync(top)).toBe(false);
    for (let i = 0; i < 100 && (await leftovers(root)).length; i++) await Bun.sleep(100);
    expect(await leftovers(root)).toEqual([]);
    expect(existsSync(join(root, "slow-rm-ran"))).toBe(true);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
}, 15000);

test.skipIf(process.platform === "win32")("Without --force, what git would refuse is left for the plain remove", async () => {
  const { root, main, top } = await checkout();
  try {
    await writeFile(join(top, "notes.md"), "draft");
    expect(await removeAside(main, top, ["worktree", "remove", top])).toBe(false);
    expect(existsSync(join(top, "notes.md"))).toBe(true);
    await rm(join(top, "notes.md"));
    // A repo checked out inside, with history of its own: git won't remove it unforced.
    await mkdir(join(top, "lib"));
    git(join(top, "lib"), "init", "-q");
    git(join(top, "lib"), "commit", "-q", "--allow-empty", "-m", "local only");
    git(top, "add", "lib");
    git(top, "commit", "-qm", "add lib");
    expect(await removeAside(main, top, ["worktree", "remove", top])).toBe(false);
    expect(existsSync(join(top, "lib", ".git"))).toBe(true);
    expect(checkouts(main)).toHaveLength(2);
    expect(await leftovers(root)).toEqual([]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test.skipIf(process.platform === "win32")("The main checkout is never moved, even for a moment", async () => {
  const { root, main } = await checkout();
  try {
    at.top = main;
    expect(await removeAside(main, main, ["worktree", "remove", main])).toBe(false);
    expect(calls.every((c) => c.there)).toBe(true);
    expect((await readdir(root)).filter((name) => name.startsWith("repo."))).toEqual([]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test.skipIf(process.platform === "win32")("A worktree git won't remove is put back where it was", async () => {
  const { root, main, top } = await checkout();
  try {
    git(main, "worktree", "lock", "--reason", "on a usb disk", top);
    expect(await removeAside(main, top, ["worktree", "remove", "--force", top])).toBe(false);
    expect(existsSync(join(top, "node_modules", "pkg0", "index.js"))).toBe(true);
    expect(git(top, "rev-parse", "--abbrev-ref", "HEAD")).toBe("feat");
    expect(checkouts(main)).toHaveLength(2);
    expect(await leftovers(root)).toEqual([]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
