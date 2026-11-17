import { test, expect } from "bun:test";
import { readFile, mkdtemp, mkdir, writeFile, rm, symlink } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const source = await readFile(new URL("../defaults/mods/files/main.js", import.meta.url), "utf8");
const directorySource = source.slice(source.indexOf("async function directory("), source.indexOf("\nexport default class"));
const loadDirectory = (exec: Function, platform: string) => new Function("exec", "window", `${directorySource}; return directory;`)(exec, () => ({ platform }));
const directory = loadDirectory(
  async (command: string, args: string[]) => {
    // Hidden like the app's spawns: a visible console would give PowerShell UTF-8 and hide code page bugs.
    const p = Bun.spawn([command, ...args], { stdout: "pipe", stderr: "pipe", windowsHide: true });
    const [code, stdout, stderr] = await Promise.all([p.exited, new Response(p.stdout).text(), new Response(p.stderr).text()]);
    return { code, stdout, stderr };
  },
  process.platform === "win32" ? "windows" : process.platform,
);

test("Files retries a remote Windows directory with PowerShell on that machine", async () => {
  const calls: { command: string; args: string[]; on: string }[] = [];
  const list = loadDirectory(async (command: string, args: string[], options: { on: string }) => {
    calls.push({ command, args, on: options.on });
    if (command === "sh") throw new Error("herdr.exec: cannot start sh: program not found");
    return { code: 0, stdout: '{"name":"中文.txt","dir":false}', stderr: "" };
  }, "linux");
  const path = "C:/O’Brien's $folder";
  expect(await list(path, "winvm")).toEqual([{ name: "中文.txt", dir: false }]);
  expect(calls.map(c => [c.command, c.on])).toEqual([["sh", "winvm"], ["powershell", "winvm"]]);
  expect(calls[0].args.at(-1)).toBe(path);
  expect(calls[1].args.at(-1)).toContain("'C:/O’’Brien''s $folder'");
  expect(calls[1].args.at(-1)).toContain("[Console]::OutputEncoding");
});

test("Files does not mask remote command failures with PowerShell", async () => {
  for (const failure of [new Error("connection lost"), new Error("cannot start sh: permission denied"), { code: 1, stdout: "", stderr: "folder denied" }]) {
    const calls: string[] = [];
    const list = loadDirectory(async (command: string) => {
      calls.push(command);
      if (failure instanceof Error) throw failure;
      return failure;
    }, "linux");
    await expect(list("/folder", "linuxvm")).rejects.toThrow(failure instanceof Error ? failure.message : failure.stderr);
    expect(calls).toEqual(["sh"]);
  }
});

test("Files keeps remote POSIX directory listings on sh", async () => {
  const calls: string[] = [];
  const list = loadDirectory(async (command: string) => {
    calls.push(command);
    return { code: 0, stdout: "d\0folder\0f\0file.txt\0", stderr: "" };
  }, "windows");
  expect(await list("/folder", "linuxvm")).toEqual([{ name: "folder", dir: true }, { name: "file.txt", dir: false }]);
  expect(calls).toEqual(["sh"]);
});

test("Files lists literal folder names, not shell expressions", async () => {
  const root = await mkdtemp(join(process.platform === "linux" ? "/var/tmp" : tmpdir(), "herdr-files-"));
  try {
    for (const name of ["Test Folder (ACME)", "O'Brien $folder; (ACME)", "O’Brien ‘quoted’ ‚low‛"]) {
      const folder = join(root, name);
      await mkdir(join(folder, "child"), { recursive: true });
      await writeFile(join(folder, "hello.txt"), "hello");
      const entries = await directory(folder, null);
      expect(entries.sort((a: { name: string }, b: { name: string }) => a.name.localeCompare(b.name))).toEqual([
        { name: "child", dir: true }, { name: "hello.txt", dir: false },
      ]);
    }
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("Files preserves Unicode names when listing and expanding folders", async () => {
  const root = await mkdtemp(join(tmpdir(), "herdr-files-unicode-"));
  try {
    const folder = "中文目录 O'Brien (测试)", file = "项目说明.txt", child = "子文件.txt";
    await mkdir(join(root, folder));
    await writeFile(join(root, file), "hello");
    await writeFile(join(root, folder, child), "hello");
    const entries = await directory(root, null);
    expect(entries).toHaveLength(2);
    expect(entries).toContainEqual({ name: folder, dir: true });
    expect(entries).toContainEqual({ name: file, dir: false });
    expect(await directory(join(root, folder), null)).toEqual([{ name: child, dir: false }]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("Files follows directory links one level at a time", async () => {
  const root = await mkdtemp(join(tmpdir(), "herdr-files-links-"));
  try {
    const target = join(root, "versions");
    await mkdir(target);
    for (let minor = 1; minor <= 9; minor++) {
      await mkdir(join(target, `0.${minor}.0`));
    }
    await writeFile(join(target, "0.9.0", "STORY.md"), "story preview");
    await mkdir(join(root, ".hidden"));
    await writeFile(join(root, "ordinary.txt"), "ordinary");
    await symlink("versions", join(root, ".kb"), "dir");
    if (process.platform === "win32") await symlink(target, join(root, "junction"), "junction");
    await symlink(join(root, "ordinary.txt"), join(root, "file-link"), "file");
    await symlink(join(root, "missing"), join(root, "dangling"), "dir");
    // A cycle must be listed without recursively walking it.
    await symlink(target, join(target, "loop"), "dir");

    const entries = await directory(root, null);
    expect(entries).toHaveLength(process.platform === "win32" ? 7 : 6);
    if (process.platform === "win32") expect(entries).toContainEqual({ name: "junction", dir: true });
    expect(entries).toContainEqual({ name: ".kb", dir: true });
    expect(entries).toContainEqual({ name: "versions", dir: true });
    expect(entries).toContainEqual({ name: ".hidden", dir: true });
    expect(entries).toContainEqual({ name: "ordinary.txt", dir: false });
    expect(entries).toContainEqual({ name: "file-link", dir: false });
    expect(entries).toContainEqual({ name: "dangling", dir: false });
    const versions = await directory(join(root, ".kb"), null);
    expect(versions).toHaveLength(10);
    for (let minor = 1; minor <= 9; minor++) {
      expect(versions).toContainEqual({ name: `0.${minor}.0`, dir: true });
    }
    expect(versions).toContainEqual({ name: "loop", dir: true });
    expect(await directory(join(root, ".kb", "0.9.0"), null)).toEqual([{ name: "STORY.md", dir: false }]);
    expect(await readFile(join(root, ".kb", "0.9.0", "STORY.md"), "utf8")).toBe("story preview");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

type Watch = Promise<string[]> & { root: string; cancel(): void; change(): void };
// A timer turn runs every promise continuation that is ready.
const settled = () => new Promise(resolve => setTimeout(resolve));

// The panel over empty folders outside any repo. Folder watches stay pending
// like the app's: until they are cancelled or their folder changes.
async function panel() {
  const at = { cwd: "/folder-0", machine: null as string | null };
  const watching = new Set<Watch>();
  const count = { listed: 0 };
  const Files = new Function("View", "spaces", "panes", "focused", "exec", "watchFolder", "window",
    `${source.replace(/^import .*;\n/gm, "").replace("export default class Files", "class Files")}; return Files;`)(
    class {},
    () => [{ id: "space", active: true, folder: at.cwd, machine: at.machine }], () => [], () => null,
    async (command: string) => {
      if (command !== "sh") return { code: 128, stdout: "", stderr: "not a git repository" };
      count.listed++;
      return { code: 0, stdout: "", stderr: "" };
    },
    (root: string) => {
      let settle!: (paths: string[]) => void;
      const watch: Watch = Object.assign(new Promise<string[]>(resolve => { settle = resolve; }), {
        root,
        cancel() { if (watching.delete(watch)) settle([]); },
        change() { if (watching.delete(watch)) settle([root]); },
      });
      watching.add(watch);
      return watch;
    },
    () => ({ platform: "linux" }),
  );
  const view = new Files();
  const cx = { timer: { every() {} }, notify() {}, spawn(task: (cx: unknown) => Promise<unknown>) { task(cx); } };
  const show = async (cwd: string, machine: string | null = null) => {
    Object.assign(at, { cwd, machine });
    view.refresh(cx);
    await settled();
  };
  view.init({}, cx);
  await settled();
  return { watching, count, show, roots: () => [...watching].map(watch => watch.root) };
}

test("Files holds one folder watch while the focused pane moves through 75 quiet folders", async () => {
  const h = await panel();
  for (let i = 0; i < 75; i++) {
    await h.show(`/folder-${i}`);
    expect(h.roots()).toEqual([`/folder-${i}`]);
  }
});

test("Files watches a folder it comes back to once, and lists it again when it changes", async () => {
  const h = await panel();
  await h.show("/folder-1");
  await h.show("/folder-0");
  expect(h.roots()).toEqual(["/folder-0"]);
  const listed = h.count.listed;
  [...h.watching][0].change();
  await settled();
  expect(h.count.listed).toBe(listed + 1);
  expect(h.roots()).toEqual(["/folder-0"]);
});

test("Files releases its watch when the pane moves to a remote folder", async () => {
  const h = await panel();
  // A remote space's folder is polled, not watched.
  await h.show("/folder-0", "vm");
  expect(h.roots()).toEqual([]);
});
