import { test, expect } from "bun:test";
import { readFile, mkdtemp, mkdir, writeFile, chmod, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

const source = (await readFile(new URL("../defaults/frame/settings/main.js", import.meta.url), "utf8"))
  .replace(/^import .*;\n/gm, "").replace("export default class Settings", "class Settings");

const coreProbe = await readFile(new URL("../defaults/frame/settings/find-core.sh", import.meta.url), "utf8");

function harness(raw: string | null = null, writeError?: string, probeError = "", probe: string | null = coreProbe) {
  const writes: unknown[][] = [], commands: unknown[][] = [], pending: Promise<unknown>[] = [];
  const Settings = new Function("View", "window", "appHome", "readTextFile", "writeTextFile", "exec", `${source}; return Settings;`)(
    class {}, () => ({ platform: "windows" }), () => "C:/fixture/app",
    async (path: string) => path === "C:/fixture/app/frame/settings/find-core.sh" ? probe : raw,
    async (...args: unknown[]) => { if (writeError) throw new Error(writeError); writes.push(args); raw = args[1] as string; },
    async (command: string, args: string[]) => { commands.push([command, args]); return { code: args[1] === "Missing" ? 3 : args[1] === "Removed" ? 1 : 0, stdout: "", stderr: probeError }; },
  );
  const view = new Settings();
  Object.assign(view, { person: {}, raw: null, handle: { focus() {} } });
  const cx = { notify() {}, spawn(fn: (cx: unknown) => Promise<unknown>) { pending.push(fn(cx)); } };
  return { view, cx, writes, commands, async settle() { for (let i = 0; i < pending.length; i++) await pending[i]; } };
}

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

test("Settings refuses to overwrite invalid JSON and reports write failures", async () => {
  for (const raw of ["{broken", "null", "[]", "42"]) {
    const h = harness(raw);
    h.view.change((p: { wsl: string }) => { p.wsl = "Ubuntu"; }, h.cx);
    await h.settle();
    expect(h.writes).toHaveLength(0);
    expect(h.view.saveError).toContain("Couldn't save settings");
  }
  const h = harness('{"font":"Geist"}', "disk full");
  h.view.change((p: { wsl: string }) => { p.wsl = "Ubuntu"; }, h.cx);
  await h.settle();
  expect(h.view.saveError).toContain("disk full");
  expect(h.view.person.wsl).toBeUndefined();
});

test("Settings verifies hc before saving a distro and retains unrelated keys", async () => {
  const h = harness('{"font":"Geist","core":"/home/Name With Space/hc"}');
  await h.view.load();
  h.view.selectWsl("Missing", h.cx);
  await h.settle();
  expect(h.writes).toHaveLength(0);
  expect(h.view.wslError).toContain("herdr-core (hc) is missing in Missing");
  expect(h.view.wslError).toContain("Install it in this distribution");
  h.view.selectWsl("Ubuntu", h.cx);
  await h.settle();
  expect(h.view.person).toEqual({ font: "Geist", core: "/home/Name With Space/hc", wsl: "Ubuntu" });
  const argv = h.commands.at(-1)![1] as string[];
  expect(argv.slice(0, 7)).toEqual(["-d", "Ubuntu", "--cd", "~", "-e", "sh", "-c"]);
  expect(argv[7]).toBe(coreProbe);
  expect(argv.at(-1)).toBe("/home/Name With Space/hc");
  expect(h.view.wslChecking).toBeNull();
});

test("Settings refuses to save when the shared WSL probe is missing or empty", async () => {
  for (const probe of [null, "", " \r\n"]) {
    const h = harness(null, undefined, "", probe);
    h.view.selectWsl("Ubuntu", h.cx);
    await h.settle();
    expect(h.commands).toHaveLength(0);
    expect(h.writes).toHaveLength(0);
    expect(h.view.wslError).toContain("Couldn't read the WSL core probe");
    expect(h.view.wslChecking).toBeNull();
  }
});

test("Settings preserves WSL startup errors instead of claiming hc is missing", async () => {
  const h = harness(null, undefined, "There is no distribution with the supplied name.\r\n");
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
  expect(h.commands).toHaveLength(1);
  expect(h.view.person.wsl).toBe("Ubuntu");
});

test("Settings uses the shared WSL probe for literal configured and home paths", async () => {
  const h = harness();
  h.view.selectWsl("Ubuntu", h.cx);
  await h.settle();
  const probe = (h.commands[0][1] as string[])[7];
  expect(probe).toBe(coreProbe);
  expect(probe).not.toContain("\r");
  if (process.platform === "win32") return;
  const root = await mkdtemp(join(tmpdir(), "herdr-settings-probe-"));
  try {
    const home = join(root, "O'Brien [literal] home"), core = join(home, ".local/bin/hc");
    await mkdir(join(home, ".local/bin"), { recursive: true });
    await writeFile(core, "#!/bin/sh\nprintf 'hc 0.1.0\\n'\n");
    await chmod(core, 0o755);
    for (const configured of [core, "~/.local/bin/hc", ""]) {
      const p = Bun.spawn(["/bin/sh", "-c", probe, "sh", configured], {
        env: { HOME: home, SHELL: "/bin/false", PATH: "/nonexistent" }, stdout: "pipe", stderr: "pipe",
      });
      expect(await p.exited).toBe(0);
      expect((await new Response(p.stdout).text()).replace(/\n$/, "")).toBe(core);
    }
    await rm(core);
    const missing = Bun.spawn(["/bin/sh", "-c", probe, "sh", ""], {
      env: { HOME: home, SHELL: "/bin/false", PATH: "/nonexistent" }, stdout: "pipe", stderr: "pipe",
    });
    expect(await missing.exited).toBe(3);
  } finally { await rm(root, { recursive: true, force: true }); }
});
