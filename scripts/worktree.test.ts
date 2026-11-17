import { test, expect } from "bun:test";
import { readFile, mkdtemp } from "node:fs/promises";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";

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

type Reply = { code: number; stdout?: string; stderr?: string };
function harness(focusedMachine: string | null = null, raw: string | null = null, files: Record<string, string> = {}, extra: Record<string, unknown> = {}, reply: (command: string, args: string[]) => Reply | undefined = () => undefined) {
  let spaceList: any[] = [];
  let paneList: any[] = [];
  const reads = { spaces: 0, panes: 0 };
  const writes: unknown[] = [];
  const destinations: (string | null)[] = [];
  const executions: any[] = [];
  const globals = {
    View: class {}, div: element, v_flex: element, h_flex: element,
    Input: { new: element }, Scrollbar: { vertical: element }, theme: () => ({}),
    appHome: () => "/app", readTextFile: async (path: string) => path === "/app/settings.json" ? raw : files[path] ?? null,
    spaces: () => { reads.spaces++; return structuredClone(spaceList); },
    panes: () => { reads.panes++; return structuredClone(paneList); },
    props: () => ({ overlay: "overlay", ...extra }), closeOverlay: () => {},
    run: (command: string, args: any) => {
      writes.push([command, args]);
      if (command === "space.new") destinations.push(args.machine === "" ? null : args.machine ?? focusedMachine);
    },
    exec: async (command: string, args: string[], options: any) => {
      executions.push({ command, args, ...options });
      return { stdout: "", stderr: "", ...(reply(command, args) ?? { code: 0 }) };
    },
  };
  const { Worktree, root, plan } = new Function(...Object.keys(globals), `${source}; return { Worktree, root, plan };`)(...Object.values(globals));
  const worktree = new Worktree();
  Object.assign(worktree, { opening: true, text: "", at: 0, repo: { name: "repo" }, userHome: "/home/user", root: "/home/user/.herdr/worktrees" });
  return { worktree, root, plan, reads, writes, destinations, executions, snapshot: (spaces: unknown[], panes: unknown[]) => { spaceList = spaces; paneList = panes; } };
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

for (const [branch, folder] of [
  ["음악수행", "음악수행"], ["쪼지학", "쪼지학"],
  ["हिन्दी", "हिन्दी"], ["हिंदी", "हिंदी"],
  ["का", "का"], ["कि", "कि"],
  ["বাংলা", "বাংলা"],
  ["feature/हिन्दी", "feature-हिन्दी"],
  ["feature/음악수행-１２٣", "feature-음악수행-１２٣"],
  ["음악수행".normalize("NFD"), "음악수행"],
  ["feature/ASCII_1.2--fix", "feature-ASCII_1.2--fix"],
]) {
  test(`branch ${branch} uses folder ${folder}`, async () => {
    const { worktree, executions, writes } = harness();
    Object.assign(worktree, { text: branch, repo: { name: "repo", main: "/repo", top: "/repo" } });
    expect(await worktree.create()).toBe("");
    const path = `/home/user/.herdr/worktrees/repo/${folder}`;
    expect(executions[1].args).toEqual(["worktree", "add", path, branch]);
    expect(writes[0][1].folder).toBe(path);
  });
}

test("symbol-only branches get stable distinct non-empty folders", async () => {
  const folders: string[] = [];
  for (const branch of ["🎵", "🎶", "🎵"]) {
    const { worktree, executions } = harness();
    Object.assign(worktree, { text: branch, repo: { name: "repo", main: "/repo", top: "/repo" } });
    expect(await worktree.create()).toBe("");
    const path = executions[1].args[2];
    expect(path).toMatch(/\/repo\/branch-[0-9a-f]{8}$/);
    folders.push(path);
  }
  expect(folders[0]).toBe(folders[2]);
  expect(folders[0]).not.toBe(folders[1]);
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

const repo = { name: "repo", main: "/home/user/code/repo", top: "/home/user/code/repo" };
const shared = (settings: unknown) => ({ [`${repo.main}/.herdr/worktree.json`]: JSON.stringify(settings) });
const sh = (argv: string[], cwd: string) => spawnSync(argv[0], argv.slice(1), { cwd, encoding: "utf8", env: { ...process.env, SHELL: "/bin/sh" } });

test("a repo's .herdr/worktree.json, else the person's worktrees.repos entry, chooses where its checkouts go", async () => {
  for (const [settings, files, expected] of [
    [null, {}, "/home/user/.herdr/worktrees/repo"],
    [null, shared({ directory: ".claude/worktrees" }), "/home/user/code/repo/.claude/worktrees"],
    [null, shared({ directory: "../repo.worktrees/" }), "/home/user/code/repo.worktrees"],
    [null, shared({ directory: "~/wt/repo" }), "/home/user/wt/repo"],
    [{ worktrees: { directory: "/srv/all", repos: { "~/code/repo": { directory: ".worktrees" } } } }, shared({ directory: ".claude/worktrees" }), "/home/user/code/repo/.claude/worktrees"],
    [{ worktrees: { directory: "/srv/all", repos: { "~/code/repo": { directory: ".worktrees" } } } }, shared({ setup: "make" }), "/home/user/code/repo/.worktrees"],
    [{ worktrees: { directory: "/srv/all", repos: { "~/code/repo": { directory: ".worktrees" } } } }, {}, "/home/user/code/repo/.worktrees"],
    [{ worktrees: { repos: { "/home/user/code/other": { directory: "/elsewhere" } } } }, shared({ directory: ".claude/worktrees" }), "/home/user/code/repo/.claude/worktrees"],
    [{ worktrees: { directory: "/srv/all", repos: { "/home/user/code/repo/": { setup: "make" } } } }, {}, "/srv/all/repo"],
  ] as const) {
    const { worktree, plan, executions, writes } = harness(null, settings && JSON.stringify(settings), files as Record<string, string>);
    Object.assign(worktree, { opening: false, text: "feature/new", repo, plan: await plan(repo, "/home/user", null, true) });
    expect(await worktree.create()).toBe("");
    expect(executions[1].args).toEqual(["worktree", "add", `${expected}/feature-new`, "feature/new"]);
    expect(writes[0][1].folder).toBe(`${expected}/feature-new`);
    expect(textOf(worktree.render({}).node)).toContain(`in ${expected.replace("/home/user", "~")}/.`);
  }
});

test("setup runs the person's commands, then the repo's, in the new terminal before its program", async () => {
  const dir = await mkdtemp(`${tmpdir()}/worktree-setup-`);
  const settings = JSON.stringify({ worktrees: { setup: "echo mine >> log" } });
  const files = shared({ setup: 'echo "repo $HERDR_WORKTREE_BRANCH $HERDR_WORKTREE_PATH $HERDR_WORKTREE_REPO" >> log\nexport FROM_SETUP=yes\nfalse' });
  const { worktree, plan, writes } = harness(null, settings, files, { argv: ["/bin/sh", "-c", 'echo "end $FROM_SETUP" >> log'] });
  Object.assign(worktree, { opening: false, text: "feature/new", repo, plan: await plan(repo, "/home/user", null, true) });
  expect(textOf(worktree.render({}).node)).toContain("Then runs echo mine >> log, then echo \"repo $HERDR_WORKTREE_BRANCH");
  expect(await worktree.create()).toBe("");
  const { argv, folder } = writes[0][1];
  expect(folder).toBe("/home/user/.herdr/worktrees/repo/feature-new");
  expect(argv.slice(3, 7)).toEqual(["sh", repo.main, folder, "feature/new"]);
  const ran = sh(argv, dir);
  expect(ran.status).toBe(0);
  expect(ran.stdout).toContain("herdr: setup step failed (exit 1): echo \"repo");
  expect(await readFile(`${dir}/log`, "utf8")).toBe(`mine\nrepo feature/new ${folder} ${repo.main}\nend yes\n`);
  const bare = sh(argv.slice(0, 7), dir);
  expect(bare.status).toBe(0);
  expect(await readFile(`${dir}/log`, "utf8")).toBe(`mine\nrepo feature/new ${folder} ${repo.main}\nend yes\nmine\nrepo feature/new ${folder} ${repo.main}\n`);
});

test("without setup the first terminal runs what it was given, and Windows skips the scripts", async () => {
  const none = harness(null, null, {}, { argv: ["claude"] });
  Object.assign(none.worktree, { opening: false, text: "x", repo, plan: await none.plan(repo, "/home/user", null, true) });
  expect(await none.worktree.create()).toBe("");
  expect(none.writes[0][1].argv).toEqual(["claude"]);
  const win = harness(null, JSON.stringify({ worktrees: { setup: "make" } }), {}, { argv: ["claude"] });
  Object.assign(win.worktree, { opening: false, text: "x", userHome: "C:\\Users\\user", repo, plan: await win.plan(repo, "C:\\Users\\user", null, true) });
  expect(textOf(win.worktree.render({}).node)).toContain("Setup scripts need sh, so they don't run on Windows.");
  expect(await win.worktree.create()).toBe("");
  expect(win.writes[0][1].argv).toEqual(["claude"]);
});

test("teardown runs the repo's commands, then the person's, and a failure keeps the checkout until Enter again", async () => {
  const linked = { ...repo, top: "/home/user/code/repo/.claude/worktrees/feature-new", linked: true };
  const settings = JSON.stringify({ worktrees: { teardown: "echo mine" } });
  let fail = true;
  const { worktree, plan, writes, executions, snapshot } = harness(null, settings, shared({ teardown: "docker compose down" }), { action: "remove", space: "s1" },
    (command) => command === "/bin/sh" && fail ? { code: 3, stdout: "a\nb", stderr: "compose: no such service" } : undefined);
  snapshot([{ id: "s1", name: "feature-new", tabs: [] }], []);
  Object.assign(worktree, { opening: false, repo: linked, dirty: false, unpushed: [], plan: await plan(linked, "/home/user", null, false) });
  expect(textOf(worktree.render({}).node)).toContain("First runs docker compose down, then echo mine in the checkout.");
  const error = await worktree.remove();
  expect(error).toContain("Teardown failed (exit 3), nothing was removed.");
  expect(error).toContain("compose: no such service");
  expect(error).toContain("Enter again removes it without the teardown.");
  expect(writes).toEqual([]);
  expect(executions.some((e) => e.args.includes("remove"))).toBe(false);
  const run = executions.find((e) => e.command === "/bin/sh");
  expect(run.cwd).toBe(linked.top);
  expect(run.timeout).toBe(600_000);
  const dir = await mkdtemp(`${tmpdir()}/worktree-teardown-`);
  const real = sh(["/bin/sh", run.args[0], run.args[1].replace("docker compose down", "echo repo >> log; exit 4"), ...run.args.slice(2)], dir);
  expect(real.status).toBe(4);
  expect(await readFile(`${dir}/log`, "utf8")).toBe("repo\n");
  fail = false;
  const before = executions.length;
  expect(await worktree.remove()).toBe("");
  expect(executions.slice(before).some((e) => e.command === "/bin/sh")).toBe(false);
  expect(writes).toEqual([["space.close", { id: "s1", confirmed: true }]]);
  expect(executions.at(-1).args).toEqual(["worktree", "remove", linked.top]);
});

test("a teardown that can't start or times out still lets Enter again remove the checkout", async () => {
  const linked = { ...repo, top: "/home/user/code/repo/.claude/worktrees/x", linked: true };
  const { worktree, plan, writes, snapshot } = harness(null, null, shared({ teardown: "sleep 9999" }), { action: "remove", space: "s1" },
    (command) => { if (command === "/bin/sh") throw new Error("timed out"); return undefined; });
  snapshot([{ id: "s1", name: "x", tabs: [] }], []);
  Object.assign(worktree, { opening: false, repo: linked, dirty: false, unpushed: [], plan: await plan(linked, "/home/user", null, false) });
  expect(await worktree.remove()).toBe("Teardown failed, nothing was removed.\ntimed out\nEnter again removes it without the teardown.");
  expect(await worktree.remove()).toBe("");
  expect(writes).toEqual([["space.close", { id: "s1", confirmed: true }]]);
});

test("broken repo and person settings say what is wrong instead of creating elsewhere", async () => {
  for (const [settings, files, message] of [
    [null, { [`${repo.main}/.herdr/worktree.json`]: "{" }, ".herdr/worktree.json:"],
    [null, { [`${repo.main}/.herdr/worktree.json`]: "[]" }, ".herdr/worktree.json must be a JSON object."],
    [null, shared({ directory: "" }), ".herdr/worktree.json directory must be a non-empty path."],
    [null, shared({ setup: 42 }), ".herdr/worktree.json setup must be a command."],
    [{ worktrees: { setup: ["make"] } }, {}, "worktrees.setup must be a command."],
    [{ worktrees: { repos: [] } }, {}, "worktrees.repos must be an object."],
    [{ worktrees: { repos: { "code/repo": {} } } }, {}, "worktrees.repos key code/repo must be an absolute path"],
    [{ worktrees: { repos: { "~/code/repo": ".worktrees" } } }, {}, 'worktrees.repos["~/code/repo"] must be an object.'],
    [{ worktrees: { repos: { "~/code/repo": { directory: "" } } } }, shared({ setup: "make" }), 'worktrees.repos["~/code/repo"] directory must be a non-empty path.'],
    [{ worktrees: { repos: { "~/code/repo": { directory: ".ok" } } } }, shared({ directory: "" }), ".herdr/worktree.json directory must be a non-empty path."],
  ] as const) {
    const { plan } = harness(null, settings && JSON.stringify(settings), files as Record<string, string>);
    await expect(plan(repo, "/home/user", null, true)).rejects.toThrow(message);
  }
});
