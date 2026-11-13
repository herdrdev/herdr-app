import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const source = (await readFile(new URL("../defaults/frame/settings/main.js", import.meta.url), "utf8"))
  .replace(/^import .*;\n/gm, "").replace("export default class Settings", "class Settings");

// The window's settings mod, without a window or a core: reads and writes go to
// this harness, and toasts are collected instead of shown.
function harness(raw: string | null = null, writeError?: string, readError?: string) {
  const writes: unknown[][] = [], commands: unknown[][] = [], toasts: unknown[] = [], pending: Promise<unknown>[] = [];
  const Settings = new Function("View", "window", "appHome", "readTextFile", "writeTextFile", "exec", "notify", `${source}; return Settings;`)(
    class {}, () => ({ platform: "windows" }), () => "C:/fixture/app",
    async () => { if (readError) throw new Error(readError); return raw; },
    async (...args: unknown[]) => { writes.push(args); if (writeError) throw new Error(writeError); raw = args[1] as string; },
    async (command: string, args: string[]) => { commands.push([command, args]); return { code: 0, stdout: '{"machines":[]}', stderr: "" }; },
    (toast: unknown) => { toasts.push(toast); },
  );
  const view = new Settings();
  Object.assign(view, { person: {}, raw: null, handle: { focus() {} } });
  const cx = { notify() {}, spawn(fn: (cx: unknown) => Promise<unknown>) { pending.push(fn(cx)); } };
  return { view, cx, writes, commands, toasts, async settle() { for (let i = 0; i < pending.length; i++) await pending[i]; } };
}

test("a change writes settings.json, keeping what else it holds", async () => {
  const h = harness('{"font_size": 15}');
  h.view.change((q: Record<string, unknown>) => { q.wsl = "Ubuntu"; }, h.cx);
  await h.settle();
  expect(JSON.parse(String(h.writes.at(-1)![1]))).toEqual({ font_size: 15, wsl: "Ubuntu" });
  expect(h.view.person).toEqual({ font_size: 15, wsl: "Ubuntu" });
  expect(h.toasts).toEqual([]);
});

test("a broken settings.json is never written over, and says so", async () => {
  for (const raw of ['{"font_size": 15, "theme": "x",}', "null", "[]", "42", "{broken"]) {
    const h = harness(raw);
    h.view.change((q: Record<string, unknown>) => { q.wsl = "Ubuntu"; }, h.cx);
    await h.settle();
    expect(h.writes).toHaveLength(0);
    expect(h.toasts).toEqual([{ title: "settings.json has errors: fix it first", level: "error" }]);
  }
});

test("a write that keeps failing is reported, not dropped", async () => {
  const h = harness("{}", "disk full");
  h.view.change((q: Record<string, unknown>) => { q.wsl = "Ubuntu"; }, h.cx);
  await h.settle();
  expect(h.writes).toHaveLength(3);
  expect(h.toasts).toEqual([{ title: "Not saved to settings.json", body: "disk full", level: "error" }]);
  expect(h.view.person.wsl).toBeUndefined();
});

test("a settings.json that can't be read, or an edit that throws, says why", async () => {
  const unread = harness("{}", undefined, "permission denied");
  unread.view.change((q: Record<string, unknown>) => { q.wsl = "Ubuntu"; }, unread.cx);
  await unread.settle();
  expect(unread.writes).toHaveLength(0);
  expect(unread.toasts).toEqual([{ title: "Cannot read settings.json", body: "permission denied", level: "error" }]);

  const thrown = harness("{}");
  thrown.view.change(() => { throw new Error("no such section"); }, thrown.cx);
  await thrown.settle();
  expect(thrown.writes).toHaveLength(0);
  expect(thrown.toasts).toEqual([{ title: "Cannot read settings.json", body: "no such section", level: "error" }]);
});

test("Settings surfaces distro list failures and recovers without losing the selected distro", async () => {
  let result: { code: number; stdout: string; stderr: string } | Error = new Error("wsl.exe unavailable");
  const Settings = new Function("View", "window", "exec", "hc", `${source}; return Settings;`)(
    class {}, () => ({ platform: "windows" }),
    async () => { if (result instanceof Error) throw result; return result; },
    async () => ({ code: 0, stdout: '{"machines":[]}', stderr: "" }),
  );
  const view = new Settings();
  view.person = { wsl: "Ubuntu" };
  view.wslDistros = [];
  await view.loadMachines();
  expect(view.wslListError).toContain("Couldn't list WSL distros: Error: wsl.exe unavailable");
  expect(view.person.wsl).toBe("Ubuntu");
  result = { code: 1, stdout: "", stderr: "WSL is starting" };
  await view.loadMachines();
  expect(view.wslListError).toContain("WSL is starting");
  result = { code: 0, stdout: "Ubuntu\r\n日本語\r\nubuntu\r\ndocker-desktop\r\n", stderr: "" };
  await view.loadMachines();
  expect(view.wslListError).toBeNull();
  expect(view.wslDistros).toEqual(["Ubuntu", "日本語"]);
});

test("Settings WSL Restart runs app.restart without requesting an update", () => {
  const button = source.slice(source.indexOf("const restartBtn ="), source.indexOf("const statusDot ="));
  const click = button.match(/\.on_click\((.*)\)\n/)?.[1];
  expect(click).toBeDefined();
  const commands: string[] = [];
  const onClick = new Function("run", `return (${click});`)((name: string) => commands.push(name));
  onClick();
  expect(commands).toEqual(["app.restart"]);
});

test("Settings saves the chosen distro at once and keeps what else settings.json holds", async () => {
  const h = harness('{"font":"Geist","core":"/home/Name With Space/hc"}');
  await h.view.load();
  h.view.selectWsl("Ubuntu", h.cx);
  await h.settle();
  expect(h.view.person).toEqual({ font: "Geist", core: "/home/Name With Space/hc", wsl: "Ubuntu" });
  expect(JSON.parse(String(h.writes.at(-1)![1]))).toEqual({ font: "Geist", core: "/home/Name With Space/hc", wsl: "Ubuntu" });
});

test("Settings saves Windows as no distro", async () => {
  const h = harness('{"font":"Geist","wsl":"Ubuntu"}');
  await h.view.load();
  h.view.selectWsl(null, h.cx);
  await h.settle();
  expect(h.view.person).toEqual({ font: "Geist" });
  expect(JSON.parse(String(h.writes.at(-1)![1]))).toEqual({ font: "Geist" });
});
