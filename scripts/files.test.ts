import { test, expect } from "bun:test";
import { readFile, mkdtemp, mkdir, writeFile, rm, symlink, unlink, access } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { randomBytes } from "node:crypto";
import { applications as macApplications, picker as macPicker } from "../defaults/mods/files/open-with-macos.js";
import { associations as winAssociations, picker as winPicker } from "../defaults/mods/files/open-with-windows.js";

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

// Three PowerShell launches can exceed Bun's 5 s default on a cold Windows runner.
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
}, 20000);

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

const editorSource = (await readFile(new URL("../defaults/mods/files/editor.js", import.meta.url), "utf8"))
  .replace(/^import .*;\r?\n/gm, "").replace(/^export /gm, "");
const linuxSource = (await readFile(new URL("../defaults/mods/files/open-with-linux.js", import.meta.url), "utf8"))
  .replace(/^import .*;\r?\n/gm, "").replace(/^export /gm, "");
function launcher(platform: string, exec: (...args: any[]) => any, home = tmpdir()) {
  const linux = new Function("exec", "readTextFile", `${linuxSource}; return { applications, picker };`)(exec,
    async (path: string) => { try { return await readFile(path, "utf8"); } catch (e: any) { if (e.code === "ENOENT") return null; throw e; } });
  const values = { exec, window: () => ({ platform }), appHome: () => home,
    mkdir, exists: async (path: string) => { try { await access(path); return true; } catch (e: any) { if (e.code === "ENOENT") return false; throw e; } },
    readFile, unlink, randomBytes, Buffer, macos: { applications: macApplications, picker: macPicker },
    windows: { associations: winAssociations, picker: winPicker }, linux };
  return new Function(...Object.keys(values), `${editorSource}; return { applications, openFile, pickApplication };`)(...Object.values(values));
}

test("default and custom editor launches keep application and document paths literal", async () => {
  for (const platform of ["macos", "windows", "linux"]) {
    const calls: any[][] = [];
    const { openFile } = launcher(platform, async (...args) => { calls.push(args); return { code: 0, stderr: "" }; });
    const path = platform === "windows" ? "C:\\Test Folder\\中文 O'Brien ‘notes’ #100%.md" : "/tmp/Test Folder/中文 O'Brien ‘notes’ #100%?.md";
    const custom = platform === "windows" ? "C:\\Test Apps\\O’Brien Editor.exe" : "/tmp/Test Apps/O'Brien Editor.app";
    await openFile(path);
    await openFile(path, { id: custom, kind: "executable" });
    expect(calls).toHaveLength(2);
    for (const [i, call] of calls.entries()) {
      if (platform === "macos") {
        expect(call).toEqual(["open", [...(i ? ["-a", custom] : []), "--", path]]);
      } else if (platform === "linux") {
        expect(call[0]).toBe("sh");
        expect(call[1].slice(-2)).toEqual([i ? custom : "xdg-open", path]);
      } else {
        expect(call[0]).toBe("powershell");
        // Exercise the real PowerShell parser without changing file associations.
        if (process.platform === "win32") {
          const script = 'function Start-Process { param([string]$FilePath, [string]$ArgumentList) [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); ConvertTo-Json -Compress @{ file = $FilePath; args = $ArgumentList } }; ' + call[1].at(-1);
          const result = await run("powershell", ["-NoProfile", "-NonInteractive", "-Command", script]);
          expect(result.code).toBe(0);
          expect(JSON.parse(result.stdout)).toEqual({ file: i ? custom : path, args: i ? '"' + path + '"' : "" });
        }
      }
    }
  }
});

test("editor launch reports failed launches", async () => {
  const { openFile } = launcher("macos", async () => ({ code: 1, stderr: "Application not found\n" }));
  await expect(openFile("/tmp/file.md", { id: "/missing/Editor.app", kind: "app" })).rejects.toThrow("Application not found");
});

// Like the other mod tests, drive the shipped view and its event handlers.
class Element {
  nodes: any[] = [];
  calls: Record<string, any[]> = {};
  constructor() {
    const proxy: Element = new Proxy(this, {
      get: (target, key: string) => key in target ? (target as any)[key] : (...args: any[]) => {
        if (key === "child") target.nodes.push(args[0]);
        else if (key === "children") target.nodes.push(...args[0]);
        else if (key === "when") { if (args[0]) args[1](proxy); }
        else target.calls[key] = args;
        return proxy;
      },
    });
    return proxy;
  }
}
function find(root: Element, id: string): Element | undefined {
  if (root?.calls?.id?.[0] === id) return root;
  for (const child of root?.nodes ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
}
function filesHarness() {
  const jobs: Promise<any>[] = [], opens: any[] = [], overlays: any[] = [], notices: any[] = [], closed: any[] = [];
  const data = new Map<string, any>();
  const state = { folder: "/project", machine: null as string | null, props: {} as any,
    read: async (_path: string): Promise<string | null> => "original", launch: async () => {},
    apps: { defaultName: "Reader", defaultId: "/apps/Reader", apps: [{ id: "/apps/Reader", name: "Reader", kind: "app" }, { id: "/apps/Editor", name: "Editor", kind: "app" }] },
    picked: null as any };
  const element = () => new Element();
  const values = {
    View: class {}, div: element, svg: element, v_flex: element, h_flex: element,
    Editor: { new: (code: any) => element().child(code.value()) },
    EditorState: { new: ({ value }: { value: string }) => ({ value: () => value, updates: 0, released: false,
      set_value(next: string) { value = next; this.updates++; }, release() { this.released = true; } }) },
    applications: async () => state.apps, pickApplication: async () => state.picked,
    openFile: async (path: string, app: any) => { opens.push([path, app]); await state.launch(); },
    theme: () => ({}), settings: () => ({}), window: () => ({ platform: "linux" }),
    spaces: () => [{ id: "space", active: true, folder: state.folder, machine: state.machine }], focused: () => null, panes: () => [],
    readTextFile: (path: string) => state.read(path), exec: async () => ({ code: 0, stdout: "", stderr: "" }),
    watchFolder: async () => { throw new Error("watch unavailable; use timer"); },
    props: () => state.props, shared: (k: string) => data.get(k), setShared: (k: string, v: any) => data.set(k, v),
    openOverlay: (p: any) => overlays.push(p), closeOverlay: (id: string) => closed.push(id), notify: (p: any) => notices.push(p),
  };
  const Files = new Function(...Object.keys(values), source.replace(/^import .*;\r?\n/gm, "").replace("export default class Files", "return class Files"))(...Object.values(values));
  const cx = { notify() {}, stop_propagation() {}, focus_handle: () => ({ focus() {} }), timer: { every() {} }, spawn(f: any) { jobs.push(f(cx)); } };
  const view = new Files(); view.init({}, cx);
  const drain = async () => { while (jobs.length) await Promise.all(jobs.splice(0)); };
  const click = (id: string) => { const el = find(view.render(cx), id); expect(el).toBeDefined(); el!.calls.on_click[0]({}, cx); };
  return { view, state, cx, drain, click, data, opens, overlays, notices, closed, Files };
}

test("Files follows the system default, offers associated apps, and remembers choices per file type", async () => {
  const h = filesHarness(); await h.drain();
  h.view.show("README.md", h.cx); await h.drain();
  expect(find(h.view.render(h.cx), "files-open-editor")!.nodes).toContain("Open in Reader");
  h.click("files-open-editor"); await h.drain();
  expect(h.opens).toEqual([["/project/README.md", null]]);
  h.click("files-choose-application");
  expect(h.overlays[0].props).toEqual({ openWith: true, file: "/project/README.md" });
  h.state.props = { ...h.overlays[0].props, overlay: "chooser" };
  const menu = new h.Files(); menu.init({}, h.cx); await h.drain();
  find(menu.render(h.cx), "files-application-1")!.calls.on_click[0]({}, h.cx); await h.drain();
  expect(h.data.get("files.openWith")[".md"]).toEqual(h.state.apps.apps[1]);
  expect(h.closed).toEqual(["chooser"]);
  h.state.props = {};
  h.click("files-open-editor"); await h.drain();
  expect(h.opens.at(-1)).toEqual(["/project/README.md", h.state.apps.apps[1]]);
  h.view.show("notes.txt", h.cx); await h.drain();
  h.click("files-open-editor"); await h.drain();
  expect(h.opens.at(-1)).toEqual(["/project/notes.txt", null]);
  h.state.props = { openWith: true, file: "/project/README.md", overlay: "chooser" };
  find(menu.render(h.cx), "files-application-0")!.calls.on_click[0]({}, h.cx); await h.drain();
  expect(h.data.get("files.openWith")[".md"]).toBeUndefined();
});

test("Other uses the application picker; cancelling or a failed launch never changes the preference", async () => {
  const h = filesHarness(); await h.drain();
  h.state.props = { openWith: true, file: "/project/README.md", overlay: "chooser" };
  const menu = new h.Files(); menu.init({}, h.cx); await h.drain();
  const other = () => find(menu.render(h.cx), "files-application-2")!.calls.on_click[0]({}, h.cx);
  other(); await h.drain();
  expect(h.opens).toHaveLength(0); expect(h.closed).toHaveLength(0);
  h.state.picked = { id: "/apps/Custom.app", name: "Custom", kind: "app" };
  h.state.launch = async () => { throw new Error("App missing"); };
  other(); await h.drain();
  expect(h.notices[0].body).toBe("App missing"); expect(h.data.has("files.openWith")).toBe(false);
  h.state.launch = async () => {};
  other(); await h.drain();
  expect(h.data.get("files.openWith")[".md"]).toEqual(h.state.picked);
});

test("Files never launches local editors for a remote path", async () => {
  const h = filesHarness(); await h.drain();
  h.state.machine = "remote"; h.view.refresh(h.cx); await h.drain();
  h.view.show("README.md", h.cx); await h.drain();
  expect(find(h.view.render(h.cx), "files-open-editor")).toBeUndefined();
  h.view.chooseApplication(); h.view.openInApplication(null, h.cx); await h.drain();
  expect(h.opens).toHaveLength(0); expect(h.overlays).toHaveLength(0);
});

test("Files refreshes saved content, preserves unchanged views, and recovers after deletion", async () => {
  const h = filesHarness(); await h.drain();
  h.view.show("README.md", h.cx); await h.drain();
  const code = h.view.code;
  h.view.refresh(h.cx); await h.drain();
  expect(h.view.code).toBe(code); expect(code.updates).toBe(0);
  h.state.read = async () => "edited outside Herdr";
  h.view.refresh(h.cx); await h.drain();
  expect(h.view.code).toBe(code); expect(code.value()).toBe("edited outside Herdr");
  expect(code.updates).toBe(1);
  h.state.read = async () => null;
  h.view.refresh(h.cx); await h.drain();
  expect(h.view.code).toBeNull(); expect(code.released).toBe(true);
  expect(h.view.fileError).toContain("no longer available");
  h.state.read = async () => "restored";
  h.view.refresh(h.cx); await h.drain();
  expect(h.view.code.value()).toBe("restored"); expect(h.view.fileError).toBeNull();
});

test("Files discards delayed reads even when the same file is reopened", async () => {
  const h = filesHarness(); await h.drain();
  let finish!: (s: string) => void;
  h.state.read = () => new Promise(resolve => { finish = resolve; });
  h.view.show("README.md", h.cx);
  h.view.show(null);
  h.state.read = async () => "new content";
  h.view.show("README.md", h.cx);
  finish("stale content"); await h.drain();
  expect(h.view.code.value()).toBe("new content");
  h.state.read = () => new Promise(resolve => { finish = resolve; });
  h.view.loadFile(h.cx);
  h.state.folder = "/other-project";
  finish("old project content"); await h.drain();
  expect(h.view.code.value()).toBe("new content");
});


async function run(command: string, args: string[], env = process.env) {
  const proc = Bun.spawn([command, ...args], { env, stdout: "pipe", stderr: "pipe", windowsHide: true });
  const [code, stdout, stderr] = await Promise.all([proc.exited, new Response(proc.stdout).text(), new Response(proc.stderr).text()]);
  return { code, stdout, stderr };
}
async function eventuallyFile(path: string) {
  const deadline = Date.now() + 10000;
  while (Date.now() < deadline) {
    try { return await readFile(path, "utf8"); } catch (e: any) { if (e.code !== "ENOENT") throw e; }
    await Bun.sleep(25);
  }
  throw new Error(`Timed out waiting for ${path}`);
}

test.skipIf(process.platform !== "darwin")("macOS opens the exact document in an application without an editor CLI", async () => {
  const root = await mkdtemp(join(tmpdir(), "herdr-open-"));
  try {
    const received = join(root, "received.txt"), file = join(root, "中文 O'Brien ‘notes’ #100%?.md"), app = join(root, "Editor.app");
    await writeFile(file, "test document");
    const script = join(root, "editor.applescript");
    // A windowless fixture app receives a real macOS open-document event, then quits.
    await writeFile(script, `on open docs\n  set targetPath to POSIX path of (item 1 of docs)\n  do shell script "/usr/bin/printf %s " & quoted form of targetPath & " > " & quoted form of "${received}"\n  quit\nend open\n`);
    const built = await run("osacompile", ["-o", app, script]);
    expect(built.code).toBe(0);
    const { openFile } = launcher("macos", run);
    await openFile(file, { id: app, kind: "executable" });
    // macOS may canonicalize /var to /private/var in the event's path.
    expect((await eventuallyFile(received)).replace(/^\/private\/var\//, "/var/")).toBe(file.replace(/^\/private\/var\//, "/var/"));
  } finally { await rm(root, { recursive: true, force: true }); }
}, 20000);

test.skipIf(process.platform !== "linux")("Linux detaches default and custom openers, preserves literal paths, and reports missing executables", async () => {
  const root = await mkdtemp(join(tmpdir(), "herdr-open-"));
  try {
    const received = join(root, "received"), finished = join(root, "finished"), custom = join(root, "My Editor's executable");
    const script = '#!/bin/sh\nprintf "%s" "$1" > "$HERDR_TEST_RECEIVED"\nsleep 0.2\nprintf done > "$HERDR_TEST_FINISHED"\n';
    await writeFile(join(root, "xdg-open"), script, { mode: 0o755 });
    await writeFile(custom, script, { mode: 0o755 });
    const env = { ...process.env, PATH: `${root}:${process.env.PATH}`, HERDR_TEST_RECEIVED: received, HERDR_TEST_FINISHED: finished };
    const { openFile } = launcher("linux", (command, args) => run(command, args, env));
    const path = "/tmp/中文 'notes' $(literal) #100%?.md";
    for (const editor of ["", custom]) {
      await openFile(path, editor ? { id: editor, kind: "executable" } : null);
      expect(await eventuallyFile(received)).toBe(path);
      expect(await eventuallyFile(finished)).toBe("done");
      await rm(received); await rm(finished);
    }
    await expect(openFile(path, { id: join(root, "missing"), kind: "executable" })).rejects.toThrow("Cannot find executable");
  } finally { await rm(root, { recursive: true, force: true }); }
});

test.skipIf(process.platform !== "win32")("Windows delivers a document path intact to a custom editor executable", async () => {
  const root = await mkdtemp(join(tmpdir(), "herdr-open-"));
  try {
    const file = join(root, "中文 O'Brien ‘notes’ $folder #100%.md"), app = join(root, "My Editor.exe");
    await writeFile(file, "test document");
    const code = 'public class Editor { public static void Main(string[] args) { System.IO.File.WriteAllText(args[0] + ".opened", args[0]); } }';
    const quote = (s: string) => "'" + s.replace(/['‘’‚‛]/g, "$&$&") + "'";
    const built = await run("powershell", ["-NoProfile", "-NonInteractive", "-Command", `$ErrorActionPreference = 'Stop'; Add-Type -TypeDefinition ${quote(code)} -OutputAssembly ${quote(app)} -OutputType WindowsApplication`]);
    expect(built.code).toBe(0);
    const { openFile } = launcher("windows", run);
    await openFile(file, { id: app, kind: "executable" });
    expect(await eventuallyFile(file + ".opened")).toBe(file);
  } finally { await rm(root, { recursive: true, force: true }); }
}, 20000);


test.skipIf(!["darwin", "win32"].includes(process.platform))("application choices come from the native file association service", async () => {
  const root = await mkdtemp(join(tmpdir(), "herdr-associations-"));
  try {
    const file = join(root, "中文 O'Brien #100%.txt"); await writeFile(file, "text");
    const { applications } = launcher(process.platform === "darwin" ? "macos" : "windows", run);
    const result = await applications(file);
    expect(Array.isArray(result.apps)).toBe(true);
    expect(result.apps.length).toBeGreaterThan(0);
    for (const app of result.apps) { expect(typeof app.id).toBe("string"); expect(typeof app.name).toBe("string"); }
    expect(result.defaultName === null || typeof result.defaultName === "string").toBe(true);
  } finally { await rm(root, { recursive: true, force: true }); }
}, 20000);

test("the application picker handles a selection, cancellation, and a picker failure", async () => {
  const root = await mkdtemp(join(tmpdir(), "herdr-picker-"));
  try {
    for (const result of ["/Applications/Other Editor.app\n", "\n", new Error("Picker unavailable")]) {
      const { pickApplication } = launcher("macos", async (_command, args) => {
        const output = args[3];
        if (result instanceof Error) await writeFile(output + ".err", result.message);
        else await writeFile(output, result);
        return { code: 0, stdout: "", stderr: "" };
      }, root);
      const picked = pickApplication({ sleep: () => Promise.resolve() });
      if (result instanceof Error) await expect(picked).rejects.toThrow("Picker unavailable");
      else expect(await picked).toEqual(result === "\n" ? null : { id: "/Applications/Other Editor.app", name: "Other Editor", kind: "app" });
    }
    expect(await import("node:fs/promises").then(fs => fs.readdir(join(root, "data/files")))).toEqual([]);
  } finally { await rm(root, { recursive: true, force: true }); }
});
