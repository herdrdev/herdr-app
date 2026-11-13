import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";

const source = (await readFile(new URL("../defaults/frame/settings/main.js", import.meta.url), "utf8"))
  .replace(/^import .*;\n/gm, "").replace("export default class Settings", "class Settings");

type Found = { path: string } | { missing: true };
type Lookup = (options: { distro?: string; core?: string | null }) => Promise<Found>;

// The window's settings mod, without a window or a core: reads and writes go to
// this harness, the core search is a stub, and toasts are collected instead of shown.
function harness(raw: string | null = null, writeError?: string, find: Lookup = async () => ({ path: "/home/test/.local/bin/hc" }), readError?: string) {
  const writes: unknown[][] = [], commands: unknown[][] = [], lookups: { distro?: string; core?: string | null }[] = [], toasts: unknown[] = [], pending: Promise<unknown>[] = [];
  const Settings = new Function("View", "window", "appHome", "readTextFile", "writeTextFile", "exec", "findCore", "notify", `${source}; return Settings;`)(
    class {}, () => ({ platform: "windows" }), () => "C:/fixture/app",
    async () => { if (readError) throw new Error(readError); return raw; },
    async (...args: unknown[]) => { writes.push(args); if (writeError) throw new Error(writeError); raw = args[1] as string; },
    async (command: string, args: string[]) => { commands.push([command, args]); return { code: 0, stdout: '{"machines":[]}', stderr: "" }; },
    async (options: { distro?: string; core?: string | null }) => { lookups.push(options); return find(options); },
    (toast: unknown) => { toasts.push(toast); },
  );
  const view = new Settings();
  Object.assign(view, { person: {}, raw: null, handle: { focus() {} } });
  const cx = { notify() {}, spawn(fn: (cx: unknown) => Promise<unknown>) { pending.push(fn(cx)); } };
  return { view, cx, writes, commands, lookups, toasts, async settle() { for (let i = 0; i < pending.length; i++) await pending[i]; } };
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
  const unread = harness("{}", undefined, undefined, "permission denied");
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

test("Settings verifies the core before saving a distro and retains unrelated keys", async () => {
  const h = harness('{"font":"Geist","core":"/home/Name With Space/hc"}', undefined, async ({ distro }) => (
    distro === "Missing" ? { missing: true } : { path: "/home/Name With Space/.local/bin/hc" }
  ));
  await h.view.load();
  h.view.selectWsl("Missing", h.cx);
  await h.settle();
  expect(h.writes).toHaveLength(0);
  expect(h.view.wslError).toContain("herdr-core (hc) is missing in Missing");
  expect(h.view.wslError).toContain("Install it in this distribution");
  h.view.selectWsl("Ubuntu", h.cx);
  await h.settle();
  expect(h.view.person).toEqual({ font: "Geist", core: "/home/Name With Space/hc", wsl: "Ubuntu" });
  expect(h.lookups.at(-1)).toEqual({ distro: "Ubuntu", core: "/home/Name With Space/hc" });
  expect(h.view.wslChecking).toBeNull();
});

test("Settings reports a distro the core search can't reach and writes nothing", async () => {
  const h = harness(null, undefined, async () => {
    throw new Error("Cannot reach WSL (Removed): There is no distribution with the supplied name.");
  });
  h.view.selectWsl("Removed", h.cx);
  await h.settle();
  expect(h.writes).toHaveLength(0);
  expect(h.view.wslError).toContain("Couldn't reach WSL (Removed)");
  expect(h.view.wslError).toContain("There is no distribution with the supplied name.");
  expect(h.view.wslError).not.toContain("Install herdr-core");
  expect(h.view.wslChecking).toBeNull();
});

test("Settings cannot start a second distro check while one is pending", async () => {
  const h = harness();
  h.view.selectWsl("Ubuntu", h.cx);
  h.view.selectWsl("Debian", h.cx);
  await h.settle();
  expect(h.lookups).toHaveLength(1);
  expect(h.view.person.wsl).toBe("Ubuntu");
});
