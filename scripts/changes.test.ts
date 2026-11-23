import { test, expect } from "bun:test";
import { readFileSync } from "node:fs";
import { mkdir, mkdtemp, readFile, realpath, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";

const source = await readFile(new URL("../defaults/mods/changes/main.js", import.meta.url), "utf8");
const rowsSource = source.slice(source.indexOf("const untab ="), source.indexOf("\nexport default class"));
const diffRows = new Function(`${rowsSource}; return diffRows;`)();

test("Changes keeps tab indentation, to the next 4-column stop", () => {
  const rows = diffRows(["@@ -1,2 +1,2 @@", "-\tif x {", "+\t\tif x {", " a\tb", "+    spaces"]);
  expect(rows.map((r: { text: string }) => r.text)).toEqual(["@@ -1,2 +1,2 @@", "    if x {", "        if x {", "a   b", "    spaces"]);
});

// A timer turn runs every promise continuation that is ready.
const settled = () => new Promise(resolve => setTimeout(resolve));
const held = () => { let release!: () => void; const wait = new Promise<void>(resolve => { release = resolve; }); return { wait, release }; };
const never = new Promise<void>(() => {});
type Answer<T> = T & { wait?: Promise<void> };

type ExecOptions = { cwd: string; on: string | null };
type ExecResult = { code: number; stdout: string; stderr: string };
type FileStatus = Answer<{ files: string[] } | { stderr: string }>;
type FileRead = Answer<{ text: string }>;
type Handler = (event: unknown, cx: unknown) => void;
type Node = { id?: string; minWidth?: number; background?: string; click?: Handler; children: (Node | string)[] };
type Element = { node: Node; id: (id: string) => Element; children: (children: (Element | string)[]) => Element };

// Run the panel's render and event handlers without a native GPUI window.
function element() {
  const node: Node = { children: [] };
  const el: Element = new Proxy({
    node,
    id(id: string) { node.id = id; return el; },
    min_w(width: number) { node.minWidth = width; return el; },
    bg(color: string) { node.background = color; return el; },
    on_click(handler: Handler) { node.click = handler; return el; },
    child(child: Element | string) { node.children.push(typeof child === "string" ? child : child.node); return el; },
    children(children: (Element | string)[]) {
      node.children.push(...children.map(child => typeof child === "string" ? child : child.node));
      return el;
    },
    when(condition: unknown, branch: (el: Element) => Element) { return condition ? branch(el) : el; },
  }, { get: (target, method) => Reflect.has(target, method) ? Reflect.get(target, method) : () => el });
  return el;
}

const nodes = (node: Node | string): Node[] => typeof node === "string" ? [] : [node, ...node.children.flatMap(nodes)];
const textOf = (node: Node | string): string => typeof node === "string" ? node : node.children.map(textOf).join("");
const ok = (stdout = ""): ExecResult => ({ code: 0, stdout, stderr: "" });
const error = (stderr: string): ExecResult => ({ code: 128, stdout: "", stderr });
const untracked = (files: string[]) => ok(files.map(f => `?? ${f}\0`).join(""));
const modSource = source.replace(/^import .*;\n/gm, "").replace("export default class Changes", "class Changes");

async function mount(at: { cwd: string; on: string | null }, host: {
  exec: (command: string, args: string[], options: ExecOptions) => Promise<ExecResult>;
  readTextFile: (path: string, options: { on: string | null }) => Promise<string | null>;
  window?: () => { platform: string };
  watchFolder?: (path: string) => Promise<unknown>;
}) {
  let message = "", tick!: (cx: unknown) => void;
  const events = new Map<string, Handler>();
  const globals = {
    View: class {}, div: element, v_flex: element, h_flex: element,
    Input: { new: element }, Scrollbar: { vertical: element },
    v_virtual_list: (id: string, length: number, _height: number, _key: unknown, rows: (range: { start: number; end: number }) => Element[]) =>
      element().id(id).children(rows({ start: 0, end: length })),
    theme: () => ({ selected: "selected", raised: "raised", ansi1: "#ff0000", ansi2: "#00ff00" }),
    settings: () => ({ font_size: 11 }),
    InputState: { new: () => ({
      on: (event: string, handler: Handler) => events.set(event, handler),
      value: () => message,
      set_value: (next: string) => { message = next; },
    }) },
    spaces: () => [{ active: true, folder: at.cwd, machine: at.on }], panes: () => [], focused: () => null,
    watchFolder: () => never, window: () => ({ platform: "macos" }),
    ...host,
  };
  const Changes = new Function(...Object.keys(globals), `${modSource}; return Changes;`)(...Object.values(globals));
  const view = new Changes();
  const cx = {
    focus_handle: () => ({}), timer: { every(_ms: number, handler: typeof tick) { tick = handler; } },
    notify() {}, spawn(task: (cx: unknown) => Promise<unknown>) { void task(cx); },
  };
  const render = (): Node => view.render(cx).node;
  const byId = (id: string) => nodes(render()).find(node => node.id === id)!;
  const listRows = (prefix: string) => nodes(byId("changes-list")).filter(node => node.id?.startsWith(prefix));
  const refresh = async () => { tick(cx); await settled(); };
  view.init({}, cx);
  await settled();
  return {
    render, byId, refresh,
    show: async (cwd: string, on = at.on) => { at.cwd = cwd; at.on = on; render(); await settled(); },
    repoNames: () => listRows("changes-repo-").map(node => textOf(node.children[1])),
    fileRows: () => listRows("change-"),
    paths: () => listRows("change-").map(node => textOf(node.children[1])),
    preview: () => byId("changes-diff").children.map(row => typeof row === "string" ? row : textOf(row.children.at(-1)!)),
    message: () => message,
    type: (next: string) => { message = next; events.get("change")!(null, cx); },
    submit: async () => { events.get("submit")!(null, cx); await settled(); },
    click: async (node: Node) => { node.click!(null, cx); await settled(); },
  };
}

// Status and file reads consume queued answers when supplied; otherwise each repo has one untracked file.
async function panel(statuses: Record<string, FileStatus[]> = {}, reads: Record<string, FileRead[]> = {}, options: {
  cwd?: string;
  on?: string;
  missingSh?: boolean;
  listError?: string;
  holdAdd?: Promise<void>;
  commitError?: string;
} = {}) {
  const commands: (ExecOptions & { command: string; args: string[] })[] = [];
  const fileReads: { path: string; on: string | null }[] = [];
  const watched: string[] = [];
  const h = await mount({ cwd: options.cwd ?? "/repo-a", on: options.on ?? null }, {
    async exec(command, args, at) {
      const { cwd } = at;
      commands.push({ command, args, ...at });
      if (command === "sh" || command === "powershell") {
        if (command === "sh" && options.missingSh) throw new Error("cannot start sh: program not found");
        if (options.listError) return error(options.listError);
        const names = cwd === "/" ? ["repo-b", "repo-a"] : [];
        return ok(command === "sh" ? names.map(name => `${name}\0`).join("") : JSON.stringify(names));
      }
      const repo = args[0] === "-C" ? `/${args[1].slice(2)}` : /^\/repo-[ab]/.exec(cwd)?.[0];
      if (!repo) return error("not a git repository");
      if (args.includes("--show-toplevel")) return ok(`${repo}\n`);
      if (args.includes("status")) {
        const answer = statuses[repo]?.shift() ?? { files: [options.cwd === "/" ? "same.txt" : `${repo.slice(-1)}.txt`] };
        await answer.wait;
        return "stderr" in answer ? error(answer.stderr) : untracked(answer.files);
      }
      if (args[0] === "add") await options.holdAdd;
      if (args[0] === "commit" && options.commitError) return error(options.commitError);
      return args.includes("HEAD") ? error("no HEAD") : ok();
    },
    async readTextFile(path, { on }) {
      fileReads.push({ path, on });
      const answer = reads[path]?.shift() ?? { text: "text\n" };
      await answer.wait;
      return answer.text;
    },
    watchFolder(path) { watched.push(path); return never; },
  });
  return { ...h, commands, fileReads, watched };
}

test.each([
  ["Bengali spacing marks", "বাংলা ".repeat(8) + "END", 400],
  ["tab-indented CJK glyphs", "\t" + "界".repeat(20) + "END", 375],
])("Changes leaves horizontal room for %s", async (_name, text, pixels) => {
  const h = await panel({}, { "/repo-a/a.txt": [{ text }] });
  expect((h.byId("changes-diff-x").children[0] as Node).minWidth).toBeGreaterThanOrEqual(pixels);
});

test("Changes follows a space switch while the last repo's preview never finishes", async () => {
  const h = await panel({}, { "/repo-a/a.txt": [{ text: "", wait: never }] });
  expect(h.paths()).toEqual(["a.txt"]);
  await h.show("/repo-b");
  expect(h.paths()).toEqual(["b.txt"]);
  await h.show("/plain");
  expect(h.paths()).toEqual([]);
  expect(textOf(h.byId("changes-list"))).toBe("Not a git repository");
});

test("Changes runs one load at a time in a repo, even when the one it left finishes late", async () => {
  const preview = held(), status = held();
  const h = await panel({ "/repo-b": [{ files: ["b.txt"], wait: status.wait }] }, { "/repo-a/a.txt": [{ text: "text\n", wait: preview.wait }] });
  await h.show("/repo-b");
  preview.release();
  await settled();
  await h.refresh();
  expect(h.commands.filter(c => c.args.includes("status")).map(c => c.cwd)).toEqual(["/repo-a", "/repo-b"]);
  status.release();
  await settled();
  expect(h.paths()).toEqual(["b.txt"]);
});

test("Changes keeps the newer list when a load from an earlier visit to the repo finishes last", async () => {
  const first = held();
  const h = await panel({ "/repo-a": [{ files: ["old.txt"], wait: first.wait }, { files: ["new.txt"] }, { files: ["updated.txt"] }] });
  await h.show("/repo-b");
  await h.show("/repo-a");
  expect(h.paths()).toEqual(["new.txt"]);
  first.release();
  await settled();
  expect(h.paths()).toEqual(["new.txt"]);
  await h.refresh();
  expect(h.paths()).toEqual(["updated.txt"]);
});

test("Changes keeps the newer preview when one from an earlier visit to the repo finishes last", async () => {
  const first = held();
  const h = await panel({}, { "/repo-a/a.txt": [{ text: "old\n", wait: first.wait }, { text: "new\n" }] });
  await h.show("/repo-b");
  await h.show("/repo-a");
  expect(h.preview()).toEqual(["@@ -0,0 +1,1 @@", "new"]);
  first.release();
  await settled();
  expect(h.preview()).toEqual(["@@ -0,0 +1,1 @@", "new"]);
});

test("Changes refreshes the file list while its first preview is still pending", async () => {
  const h = await panel({ "/repo-a": [{ files: ["a.txt"] }, { files: ["next.txt"] }] }, { "/repo-a/a.txt": [{ text: "", wait: never }] });
  await h.refresh();
  expect(h.paths()).toEqual(["next.txt"]);
});

test("Changes shows both checkouts and switches between same-named files", async () => {
  const first = held();
  const h = await panel({}, {
    "/repo-a/same.txt": [{ text: "obsolete A\n", wait: first.wait }, { text: "from A\n" }],
    "/repo-b/same.txt": [{ text: "from B\n" }],
  }, { cwd: "/" });
  expect(h.paths()).toEqual(["same.txt", "same.txt"]);
  expect(textOf(h.byId("changes-repo-/repo-a"))).toContain("repo-a1 files");
  expect(textOf(h.byId("changes-repo-/repo-b"))).toContain("repo-b1 files");
  expect(textOf(h.render())).toContain("Changes2 files");
  expect(new Set(h.fileRows().map(node => node.id)).size).toBe(2);

  h.type("draft A");
  await h.click(h.fileRows()[1]);
  expect(textOf(h.byId("commit"))).toBe("Commit to repo-b");
  expect(h.byId("changes-repo-/repo-b").background).toBe("selected");
  expect(h.message()).toBe("");
  expect(h.preview().at(-1)).toBe("from B");
  h.type("draft B");
  first.release();
  await settled();
  expect(h.preview().at(-1)).toBe("from B");

  await h.click(h.byId("changes-repo-/repo-a"));
  expect(h.paths()).toEqual(["same.txt"]);
  expect(h.message()).toBe("draft A");
  expect(h.preview().at(-1)).toBe("from A");
  await h.click(h.byId("changes-repo-/repo-a"));
  expect(h.paths()).toEqual(["same.txt", "same.txt"]);
  await h.click(h.fileRows()[1]);
  expect(h.message()).toBe("draft B");
  await h.click(h.fileRows()[1]);
  await h.refresh();
  expect(h.preview()).toEqual([]);
});

test.each(["success", "failure"])("Changes keeps drafts in their own checkouts after a remote commit: %s", async (outcome) => {
  const add = held(), on = "render-host", commitError = outcome === "failure" ? "Hook declined the commit" : undefined;
  const h = await panel({}, {}, { cwd: "/", on, holdAdd: add.wait, commitError });
  h.type("  draft A  ");
  await h.submit();
  await h.click(h.fileRows()[1]);
  h.type("draft B");
  await h.click(h.byId("commit"));
  add.release();
  await settled();

  expect(h.commands.filter(c => ["add", "commit"].includes(c.args[0])).map(c => [c.cwd, c.on, c.args])).toEqual([
    ["/repo-a", on, ["add", "-A"]],
    ["/repo-a", on, ["commit", "-m", "draft A"]],
  ]);
  expect(h.message()).toBe("draft B");
  expect(textOf(h.render())).not.toContain("Commit failed");
  await h.click(h.byId("changes-repo-/repo-a"));
  expect(h.message()).toBe(commitError ? "  draft A  " : "");
  if (commitError) expect(textOf(h.render())).toContain(`Commit failed: Error: ${commitError}`);
});

test("Changes keeps healthy groups usable when another checkout fails", async () => {
  const status = held();
  const h = await panel({
    "/repo-a": [{ stderr: "status unavailable", wait: status.wait }, { stderr: "status unavailable" }],
    "/repo-b": [{ files: [] }, { files: ["same.txt"] }],
  }, {}, { cwd: "/" });
  h.type("draft A");
  await h.submit();
  expect(h.commands.some(c => c.args[0] === "add")).toBe(false);
  expect(textOf(h.byId("changes-list"))).toContain("No uncommitted changes");
  status.release();
  await settled();
  expect(textOf(h.byId("changes-list"))).toContain("status unavailable");
  await h.click(h.byId("commit"));
  expect(h.commands.some(c => c.args[0] === "add")).toBe(false);

  await h.click(h.byId("changes-repo-/repo-b"));
  await h.refresh();
  expect(textOf(h.byId("changes-repo-/repo-b"))).toContain("repo-b1 files");
  expect(h.paths()).not.toContain("same.txt");
  h.type("draft B");
  await h.submit();
  expect(h.commands.filter(c => c.args[0] === "commit").map(c => c.cwd)).toEqual(["/repo-b"]);
  expect(h.watched).toEqual([]);
});

test.each([false, true])("Changes discovers and previews on the focused machine (PowerShell fallback: %s)", async (missingSh) => {
  const on = "remote";
  const h = await panel({}, {}, { cwd: "/", on, missingSh });
  expect(h.repoNames()).toEqual(["repo-a", "repo-b"]);
  expect(h.commands.filter(c => c.command !== "git").map(c => c.command)).toEqual(missingSh ? ["sh", "powershell"] : ["sh"]);
  expect(h.commands.filter(c => c.args.includes("status")).map(c => c.cwd)).toEqual(["/repo-a", "/repo-b"]);
  expect(h.commands.every(c => c.on === on)).toBe(true);
  expect(h.fileReads).toEqual([{ path: "/repo-a/same.txt", on }]);
  await h.show("/repo-a", on);
  expect(h.paths()).toEqual(["same.txt"]);
  expect(h.watched).toEqual([]);
});

test("Changes distinguishes listing errors from an empty folder", async () => {
  const h = await panel({}, {}, { cwd: "/", listError: "Cannot open directory" });
  expect(textOf(h.byId("changes-list"))).toContain("Cannot open directory");
  await h.show("");
  expect(textOf(h.byId("changes-list"))).toBe("No folder");
});

test("Changes discovers native Git checkouts and commits only in the selected one", async () => {
  const folder = await realpath(await mkdtemp(join(tmpdir(), "herdr-changes-")));
  try {
    const win = process.platform === "win32";
    const env = { ...process.env };
    for (const key of Object.keys(env)) if (key.startsWith("GIT_")) delete env[key];
    Object.assign(env, {
      GIT_CONFIG_NOSYSTEM: "1", GIT_CONFIG_GLOBAL: join(folder, "gitconfig"), GIT_CEILING_DIRECTORIES: dirname(folder),
      GIT_AUTHOR_NAME: "Changes Test", GIT_AUTHOR_EMAIL: "changes@example.test",
      GIT_COMMITTER_NAME: "Changes Test", GIT_COMMITTER_EMAIL: "changes@example.test",
    });
    function run(command: string, args: string[], cwd: string): ExecResult {
      const result = Bun.spawnSync([command, ...args], { cwd, env });
      return { code: result.exitCode, stdout: result.stdout.toString(), stderr: result.stderr.toString() };
    }
    function checked(cwd: string, command: string, ...args: string[]) {
      const result = run(command, args, cwd);
      if (result.code !== 0) throw new Error(`${command} ${args.join(" ")}: ${result.stderr || result.stdout}`);
      return result.stdout.replace(/\r?\n$/, "");
    }
    const clean = join(folder, "api"), dirty = join(folder, "web café"), linked = join(folder, ".linked");
    for (const cwd of [clean, dirty]) {
      await mkdir(cwd);
      checked(cwd, "git", "init", "-q");
      checked(cwd, "git", "commit", "--allow-empty", "-qm", "base");
    }
    const plain = join(folder, "plain");
    await mkdir(plain);
    const bogus = join(folder, "bogus");
    await mkdir(bogus);
    await writeFile(join(bogus, ".git"), "not a gitdir");
    checked(plain, "git", "init", "-q", "too-deep");
    checked(folder, "git", "init", "--bare", "-q", "bare.git");
    await symlink(dirty, join(folder, "directory-alias"), win ? "junction" : "dir");
    await writeFile(join(dirty, "same.txt"), "dirty sibling\n");
    await writeFile(join(dirty, "staged.txt"), "staged sibling\n");
    checked(dirty, "git", "add", "staged.txt");

    const h = await mount({ cwd: folder, on: null }, {
      async exec(command, args, { cwd }) { return run(command, args, cwd); },
      async readTextFile(path) { return readFileSync(path, "utf8"); },
      window: () => ({ platform: win ? "windows" : process.platform }),
    });
    expect(h.repoNames()).toEqual(["api", "web café"]);
    expect(textOf(h.byId("changes-list"))).toContain("No uncommitted changes");
    expect(h.paths()).toContain("same.txt");

    checked(clean, "git", "worktree", "add", "-qb", "linked", linked);
    if (win) checked(folder, "attrib", "+H", ".linked");
    await h.refresh();
    expect(h.repoNames()).toEqual([".linked", "api", "web café"]);
    expect(textOf(h.byId("commit"))).toBe("Commit to api");
    checked(clean, "git", "worktree", "remove", linked);
    await h.refresh();
    expect(h.repoNames()).toEqual(["api", "web café"]);

    const siblingState = () => [
      checked(dirty, "git", "rev-parse", "HEAD"),
      checked(dirty, "git", "ls-files", "--stage"),
      checked(dirty, "git", "status", "--porcelain=v1", "-z"),
      readFileSync(join(dirty, "same.txt"), "utf8"),
    ];
    const untouched = siblingState();
    await writeFile(join(clean, "same.txt"), "committed in api\n");
    await h.refresh();
    expect(h.preview().at(-1)).toBe("committed in api");
    h.type("Only in api");
    await h.click(h.byId("commit"));
    expect(checked(clean, "git", "show", "HEAD:same.txt")).toBe("committed in api");
    expect(checked(clean, "git", "log", "-1", "--format=%s")).toBe("Only in api");
    expect(siblingState()).toEqual(untouched);

    const sub = join(clean, "sub");
    await mkdir(sub);
    checked(sub, "git", "init", "-q", "nested");
    await writeFile(join(clean, "outer.txt"), "enclosing repo\n");
    await h.show(sub);
    expect(h.repoNames()).toEqual([]);
    expect(h.paths()).toContain("outer.txt");
    expect(textOf(h.byId("commit"))).toBe("Commit");
  } finally {
    await rm(folder, { recursive: true, force: true });
  }
});
