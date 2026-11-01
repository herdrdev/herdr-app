import { test, expect } from "bun:test";
import { readFile, mkdtemp, mkdir, writeFile, rm, symlink } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const source = await readFile(new URL("../defaults/mods/files/main.js", import.meta.url), "utf8");
const directorySource = source.slice(source.indexOf("async function directory("), source.indexOf("\nexport default class"));
const directory = new Function("exec", "window", `${directorySource}; return directory;`)(
  async (command: string, args: string[]) => {
    // Hidden like the app's spawns: a visible console would give PowerShell UTF-8 and hide code page bugs.
    const p = Bun.spawn([command, ...args], { stdout: "pipe", stderr: "pipe", windowsHide: true });
    const [code, stdout, stderr] = await Promise.all([p.exited, new Response(p.stdout).text(), new Response(p.stderr).text()]);
    return { code, stdout, stderr };
  },
  () => ({ platform: process.platform === "win32" ? "windows" : process.platform }),
);

test("Files lists literal folder names, not shell expressions", async () => {
  const root = await mkdtemp(join(process.platform === "linux" ? "/var/tmp" : tmpdir(), "herdr-files-"));
  try {
    for (const name of ["Test Folder (ACME)", "O'Brien $folder; (ACME)"]) {
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

function fileRows(root: string, on: string | null) {
  const build = () => {
    const calls: Record<string, unknown[]> = {};
    const element: any = new Proxy({ calls }, {
      get(target, name: string) {
        if (name === "calls") return target.calls;
        return (...args: unknown[]) => {
          calls[name] = args;
          if (name === "when" && args[0]) (args[1] as Function)(element);
          return element;
        };
      },
    });
    return element;
  };
  const Files = new Function("View", "div", "svg", "h_flex",
    source.replace(/^import .*;$/gm, "").replace("export default class", "class") + "; return Files;",
  )(class {}, build, build, build);
  const files = new Files();
  files.root = root; files.on = on; files.marks = {};
  files.entries = new Map([
    ["", [{ name: "中文 O'Brien", dir: true }, { name: "hello world.txt", dir: false }]],
    ["中文 O'Brien", [{ name: "子文件.txt", dir: false }]],
  ]);
  files.folders = new Set(["中文 O'Brien"]);
  return { files, rows: files.rows("", 0, [], {}) };
}

test("Files makes folders and nested files draggable with their source machine", () => {
  for (const [root, on] of [["/work", null], ["/remote/work", "server"], ["C:/Project", null]] as const) {
    const { rows } = fileRows(root, on);
    expect(rows.map((row: any) => row.calls.drag_paths)).toEqual([
      [[`${root}/中文 O'Brien`], { on }],
      [[`${root}/中文 O'Brien/子文件.txt`], { on }],
      [[`${root}/hello world.txt`], { on }],
    ]);
  }
});

test("Files retains folder expansion and file preview click handlers", () => {
  const { files, rows } = fileRows("/work", null), clicks: unknown[] = [];
  files.toggle = (path: string) => clicks.push(["toggle", path]);
  files.show = (path: string) => clicks.push(["show", path]);
  rows.forEach((row: any) => row.calls.on_click[0]({}, {}));
  expect(clicks).toEqual([["toggle", "中文 O'Brien"], ["show", "中文 O'Brien/子文件.txt"], ["show", "hello world.txt"]]);
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
