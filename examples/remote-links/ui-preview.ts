#!/usr/bin/env bun
// Render each dialog state on a private display. No real transport or desktop input.
import { mkdtemp, mkdir, readFile, copyFile, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
const bin = join(import.meta.dir, "../../target/debug/herdr-app");
const root = await mkdtemp(join(tmpdir(), "herdr-link-preview-"));
const home = join(root, "home");
const env = { ...process.env, HERDR_APP_HOME: home, HC_SESSION: `herdr-link-preview-${process.pid}`,
  XDG_STATE_HOME: join(root, "state"), XDG_CONFIG_HOME: join(root, "config"), WAYLAND_DISPLAY: "",
  HERDR_TELEMETRY: "0", HERDR_APP_NO_SOUND: "1" };
let app: ReturnType<typeof Bun.spawn> | undefined;
let xvfb: ReturnType<typeof Bun.spawn> | undefined;
async function cmd(argv: string[]) {
  const p = Bun.spawn(argv, { env, stdout: "pipe", stderr: "pipe", stdin: "ignore" });
  const timer = setTimeout(() => p.kill(), 20000);
  try {
    const [code, out, err] = await Promise.all([p.exited, new Response(p.stdout).text(), new Response(p.stderr).text()]);
    if (code) throw new Error(`${argv.slice(0, 3).join(" ")}: ${err || out}`);
    return out;
  } finally { clearTimeout(timer); }
}
async function eventually(get: () => Promise<boolean>) {
  const until = Date.now() + 20000;
  while (Date.now() < until) { if (await get()) return; await Bun.sleep(100); }
  throw new Error("Private preview timed out");
}
try {
  for (const name of ["remote-links", "preview", "background"]) await mkdir(join(home, "mods", name), { recursive: true });
  await mkdir(join(home, "layouts"), { recursive: true });
  await writeFile(join(home, "settings.json"), JSON.stringify({ sounds: { enabled: false }, telemetry: false }));
  await writeFile(join(home, "shared.json"), JSON.stringify({ "frame.welcomed": true }));
  for (const file of ["main.js", "policy.js", "browser.svg", "spinner.svg", "mod.json"]) {
    await copyFile(join(import.meta.dir, "mod", file), join(home, "mods/remote-links", file));
    await copyFile(join(import.meta.dir, "mod", file), join(home, "mods/preview", file === "main.js" ? "ui-main.js" : file));
  }
  await writeFile(join(home, "mods/preview/main.js"), `import RemoteLinks from "./ui-main.js";
import { props } from "herdr";
export default class Preview extends RemoteLinks {
  init(p, cx) { super.init(p, cx); this.state = (props() || {}).state || "opening";
    this.session = this.state === "open" ? { port: 45678, terminal: null } : null;
    this.message = this.state === "error" ? "SSH connection failed: the remote machine is unreachable. Check its connection in Settings → Machines, then try again.\\n".repeat(4) : "";
  }
}`);
  await writeFile(join(home, "mods/background/mod.json"), '{"place":"none"}');
  await writeFile(join(home, "mods/background/main.js"), `import { View, div } from "gpui-kit";
import { checking, openOverlay, props, theme } from "herdr";
export default class Background extends View {
  init(p, cx) { if (checking()) return; openOverlay({ mod: "preview", kind: "dialog", width: 520, height: 368, props: props() }); }
  render() { return div().size_full().bg(theme().canvas); }
}`);
  const cases = [
    ["connecting", "opening", "graphite", "dev"], ["error", "error", "graphite", "dev"],
    ["ready", "ready", "graphite", "dev"], ["connected", "open", "graphite", "dev"], ["light", "opening", "paper", "dev"],
    ["long", "opening", "graphite", "a-very-long-remote-machine-name-".repeat(4)],
  ];
  for (const [name, state, theme, machine] of cases) await writeFile(join(home, "layouts", name + ".json"), JSON.stringify({
    id: "root", type: "block", theme, titlebar: "none", content: { mod: "background", props: {
      state, machine, terminal: machine + "/t7", url: "http://localhost:3000/dashboard" + (name === "long" ? "?search=" + "long-value-".repeat(40) : ""), activated: false,
    } },
  }));
  xvfb = Bun.spawn([process.env.HERDR_TEST_XVFB || "Xvfb", "-displayfd", "1", "-screen", "0", "1000x700x24", "-nolisten", "tcp"], { stdout: "pipe", stderr: "inherit" });
  const reader = (xvfb.stdout as ReadableStream<Uint8Array>).getReader();
  const display = new TextDecoder().decode((await reader.read()).value).trim(); reader.releaseLock();
  if (!/^\d+$/.test(display)) throw new Error("No private display");
  env.DISPLAY = ":" + display;
  for (const name of ["remote-links", "preview", "background"]) console.log(await cmd([bin, "check", join(home, "mods", name)]));
  const log = Bun.file(join(root, "app.log"));
  app = Bun.spawn([bin], { env, stdin: "ignore", stdout: log, stderr: log });
  await eventually(async () => { try { await cmd([bin, "status"]); return true; } catch { return false; } });
  for (const [name] of cases) {
    await cmd([bin, "do", "overlay.close"]).catch(() => {});
    await cmd([bin, "do", "layout.switch", JSON.stringify({ name })]);
    await eventually(async () => JSON.parse(await cmd([bin, "status"])).overlays.includes("preview"));
    // Let the next GPU frame finish after the IPC update (not an animation loop).
    await Bun.sleep(250);
    const status = JSON.parse(await cmd([bin, "status"]));
    if (status.errors.length) throw new Error(JSON.stringify(status.errors));
    await cmd([bin, "shot", join(root, name + ".png")]);
  }
  console.log("PASS: connecting, error, ready, connected, light and long-content states render without errors");
  console.log("Screenshots: " + root);
} catch (error) {
  console.error("Diagnostics: " + root);
  console.error(await readFile(join(root, "app.log"), "utf8").catch(() => ""));
  throw error;
} finally {
  app?.kill(); await app?.exited;
  await cmd(["hc", "server", "stop"]).catch(() => {});
  xvfb?.kill(); await xvfb?.exited;
}
