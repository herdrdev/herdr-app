import { expect, test } from "bun:test";
import { readFile } from "node:fs/promises";

// Open repository: pick a local git repository, then open or create its space.
// New space, including its key, stays `space.new`.

type Exec = (command: string, args: string[], options: { cwd?: string }) => Promise<{ code: number; stdout: string; stderr: string }>;

async function openRepository(host: {
  pickFolder: (options: { prompt: string }) => Promise<string | null>;
  exec: Exec;
  spaces: () => any[];
  panes: () => any[];
  run: (command: string, args: any) => void;
}) {
  const source = await readFile(new URL("../defaults/mods/space-new/main.js", import.meta.url), "utf8");
  const body = source.replace(/^import .*;\n/gm, "").split("export default class")[0].replace("export async function openRepository", "async function openRepository");
  return new Function(`${body}; return openRepository;`)()(host);
}

function world(roots: Record<string, string>, picked: string | null, spaces: any[] = [], panes: any[] = []) {
  const writes: unknown[][] = [];
  const prompts: string[] = [];
  const execs: { cwd?: string; args: string[] }[] = [];
  const host = {
    writes,
    async pickFolder(options: { prompt: string }) {
      prompts.push(options.prompt);
      return picked;
    },
    async exec(command: string, args: string[], options: { cwd?: string }) {
      execs.push({ cwd: options.cwd, args });
      expect(command).toBe("git");
      expect(args).toEqual(["rev-parse", "--show-toplevel"]);
      const cwd = (options.cwd ?? "").replace(/\\/g, "/").replace(/\/+$/, "");
      const root = roots[cwd];
      return root === undefined
        ? { code: 128, stdout: "", stderr: "not a git repository" }
        : { code: 0, stdout: `${root}\n`, stderr: "" };
    },
    spaces: () => spaces,
    panes: () => panes,
    run(command: string, args: any) { writes.push([command, args]); },
  };
  return { host, writes, prompts, execs };
}

test("cancelling the folder picker creates nothing", async () => {
  const { host, writes, prompts, execs } = world({}, null);
  expect(await openRepository(host)).toBe("");
  expect(prompts).toEqual(["Choose a git repository"]);
  expect(execs).toEqual([]);
  expect(writes).toEqual([]);
});

test("a folder that is not a git repository is refused", async () => {
  const { host, writes } = world({}, "/tmp/notes");
  expect(await openRepository(host)).toBe("That folder is not a git repository.");
  expect(writes).toEqual([]);
});

test("a directory inside a repository creates a space at that repository on this computer", async () => {
  const { host, writes } = world({ "/repo/src": "/repo" }, "/repo/src");
  expect(await openRepository(host)).toBe("");
  expect(writes).toEqual([["space.new", { folder: "/repo", machine: "" }]]);
});

test("an existing local space in that repository is shown", async () => {
  const spaces = [{ id: "sp1", machine: null, folder: "/elsewhere", tabs: [{ id: "tb1" }] }];
  const panes = [{ tab: "tb1", cwd: "/repo/crates" }];
  const { host, writes } = world({ "/repo/src": "/repo", "/repo/crates": "/repo" }, "/repo/src", spaces, panes);
  expect(await openRepository(host)).toBe("");
  expect(writes).toEqual([["space.select", { id: "sp1" }]]);
});

test("a space made in the repository is shown even after its terminal leaves", async () => {
  const spaces = [{ id: "sp1", machine: null, folder: "/repo", tabs: [{ id: "tb1" }] }];
  const panes = [{ tab: "tb1", cwd: "/tmp" }];
  const { host, writes } = world({ "/repo/src": "/repo", "/repo": "/repo" }, "/repo/src", spaces, panes);
  expect(await openRepository(host)).toBe("");
  expect(writes).toEqual([["space.select", { id: "sp1" }]]);
});

test("a worktree checkout and a remote space are not that repository", async () => {
  const spaces = [
    { id: "remote", machine: "vps", folder: "/repo", tabs: [{ id: "rt" }] },
    { id: "linked", machine: null, folder: "/home/user/.herdr/worktrees/repo/feature", tabs: [{ id: "lt" }] },
  ];
  const panes = [
    { tab: "rt", cwd: "/repo" },
    { tab: "lt", cwd: "/home/user/.herdr/worktrees/repo/feature" },
  ];
  const roots = {
    "/repo": "/repo",
    "/home/user/.herdr/worktrees/repo/feature": "/home/user/.herdr/worktrees/repo/feature",
  };
  const { host, writes } = world(roots, "/repo", spaces, panes);
  expect(await openRepository(host)).toBe("");
  expect(writes).toEqual([["space.new", { folder: "/repo", machine: "" }]]);
});

test("windows paths compare after slash normalization", async () => {
  const spaces = [{ id: "sp1", machine: null, folder: "C:\\repo", tabs: [{ id: "tb1" }] }];
  const panes = [{ tab: "tb1", cwd: "C:\\repo\\src" }];
  const { host, writes } = world({ "C:/repo/src": "C:/repo" }, "C:\\repo\\src", spaces, panes);
  expect(await openRepository(host)).toBe("");
  expect(writes).toEqual([["space.select", { id: "sp1" }]]);
});

test("a live space in the repository wins over one that only started there", async () => {
  const spaces = [
    { id: "left", machine: null, folder: "/repo", tabs: [{ id: "left-tab" }] },
    { id: "here", machine: null, folder: "/other", tabs: [{ id: "here-tab" }] },
  ];
  const panes = [
    { tab: "left-tab", cwd: "/tmp" },
    { tab: "here-tab", cwd: "/repo" },
  ];
  const { host, writes } = world({ "/repo": "/repo" }, "/repo", spaces, panes);
  expect(await openRepository(host)).toBe("");
  expect(writes).toEqual([["space.select", { id: "here" }]]);
});

test("git failing to run is not reported as an ordinary refusal", async () => {
  const host = {
    pickFolder: async () => "/repo",
    exec: async () => { throw new Error("git is not installed"); },
    spaces: () => [],
    panes: () => [],
    run() {},
  };
  await expect(openRepository(host)).rejects.toThrow("git is not installed");
});

test("Open repository is its own action and New space stays space.new", async () => {
  const read = (path: string) => readFile(new URL(path, import.meta.url), "utf8");
  const [menu, plus, spaces, picker, palette, keys, mac] = await Promise.all([
    read("../defaults/mods/menu/main.js"),
    read("../defaults/mods/new-menu/main.js"),
    read("../defaults/mods/spaces/main.js"),
    read("../defaults/mods/picker/main.js"),
    read("../defaults/frame/palette/main.js"),
    read("../defaults/keys.json"),
    read("../defaults/keys.macos.json"),
  ]);
  for (const source of [menu, plus, spaces, palette]) {
    expect(source).toContain('mod.open", { name: "space-new" }');
    expect(source).toContain("Open repository");
  }
  expect(menu).toContain('["New space", "space.new", {}]');
  expect(plus).toContain('run("space.new")');
  expect(spaces).toContain('run("space.new", { machine: m ?? "" })');
  expect(spaces).toContain('on_click(() => run("space.new"))');
  expect(picker).toContain('run("space.new", { machine: r.machine ?? "" })');
  expect(picker).not.toContain("space-new");
  expect(palette).toContain('["space.new", {}, "Space", "New space"');
  expect(keys).toContain('"ctrl-shift-n": "space.new"');
  expect(mac).toContain('"cmd-n": "space.new"');
  expect(keys).not.toContain("space-new");
  expect(mac).not.toContain("space-new");
});
