#!/usr/bin/env bun
// Private Xvfb/core/app test. Never synthesizes input on the real desktop.
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";

const bin = join(import.meta.dir, "../../target/debug/herdr-app");
const root = await mkdtemp(join(tmpdir(), "herdr-link-ui-"));
const home = join(root, "home");
const env = { ...process.env, HERDR_APP_HOME: home, HC_SESSION: `herdr-link-ui-${process.pid}`,
  XDG_STATE_HOME: join(root, "state"), XDG_CONFIG_HOME: join(root, "config"),
  WAYLAND_DISPLAY: "", HERDR_APP_NO_SOUND: "1", HERDR_TELEMETRY: "0" };
let xvfb: ReturnType<typeof Bun.spawn> | undefined;
let app: ReturnType<typeof Bun.spawn> | undefined;
async function cmd(argv: string[]) {
  const p = Bun.spawn(argv, { env, stdout: "pipe", stderr: "pipe", stdin: "ignore" });
  const timer = setTimeout(() => p.kill(), 30000);
  try {
    const [code, stdout, stderr] = await Promise.all([p.exited, new Response(p.stdout).text(), new Response(p.stderr).text()]);
    if (code) throw new Error(`${argv.slice(0, 3).join(" ")} failed: ${stderr || stdout}`);
    return stdout;
  } finally { clearTimeout(timer); }
}
async function eventually(get: () => Promise<boolean>, what: string) {
  const until = Date.now() + 20000;
  while (Date.now() < until) { if (await get()) return; await Bun.sleep(100); }
  throw new Error("Timed out: " + what);
}
try {
  await mkdir(join(home, "mods/capture-link"), { recursive: true });
  await mkdir(join(home, "layouts"), { recursive: true });
  await writeFile(join(home, "settings.json"), JSON.stringify({ sounds: { enabled: false }, telemetry: false }));
  // Suppress the first-run welcome in this private fixture. status.overlays lists
  // mod names, not overlay IDs, so those names must not be used as close targets.
  await writeFile(join(home, "shared.json"), JSON.stringify({ "frame.welcomed": true }));
  await writeFile(join(home, "layouts/hook-test.json"), JSON.stringify({ id: "root", type: "block", titlebar: "none",
    terminal_link_handler: "capture-link", terminal_padding: [8, 8], content: { tab: "active" } }));
  await writeFile(join(home, "mods/capture-link/mod.json"), '{"place":"none"}');
  await writeFile(join(home, "mods/capture-link/main.js"), `import { View, div } from "gpui-kit";
import { props, checking, appHome, writeTextFile, closeOverlay } from "herdr";
export default class Capture extends View {
  init(_p, cx) { if (checking()) return; const p = props(); cx.spawn(async () => {
    await writeTextFile(appHome() + "/captured.json", JSON.stringify(p), null); closeOverlay(p.overlay);
  }); }
  render() { return div().child("Captured terminal link"); }
}`);
  const xvfbPath = process.env.HERDR_TEST_XVFB ?? "Xvfb";
  xvfb = Bun.spawn([xvfbPath, "-displayfd", "1", "-screen", "0", "1400x900x24", "-nolisten", "tcp"], { stdout: "pipe", stderr: "inherit" });
  const reader = (xvfb.stdout as ReadableStream<Uint8Array>).getReader();
  const display = new TextDecoder().decode((await reader.read()).value).trim();
  reader.releaseLock();
  if (!/^\d+$/.test(display)) throw new Error("No private X display");
  env.DISPLAY = ":" + display;
  const log = Bun.file(join(root, "app.log"));
  app = Bun.spawn([bin], { env, stdin: "ignore", stdout: log, stderr: log });
  await eventually(async () => { try { await cmd([bin, "status"]); return true; } catch { return false; } }, "private app ready");
  await cmd([bin, "do", "layout.switch", JSON.stringify({ name: "hook-test" })]);
  const url = "http://127.0.0.1:31415/secret?x=1#b";
  const launched = JSON.parse(await cmd([bin, "do", "space.new", JSON.stringify({ name: "link-test", argv: ["sh", "-c",
    `printf '\\033[2J\\033[H\\033]8;;${url}\\033\\\\CLICK-ME\\033]8;;\\033\\\\\\n'; exec cat`] })]));
  await eventually(async () => (await cmd(["hc", "read", launched.terminal, "--lines", "4"])).includes("CLICK-ME"), "link rendered");
  // The interactive launcher may print a greeting; the fixture clears it. The
  // first-run welcome can also arrive asynchronously after the app answers IPC.
  await eventually(async () => {
    const current = JSON.parse(await cmd([bin, "status"]));
    return !current.overlays?.length;
  }, "first-run overlays dismissed");
  await cmd([bin, "shot", join(root, "before.png")]);
  const input = `from Xlib import X, XK, display\nfrom Xlib.ext import xtest\nd=display.Display()\nr=d.screen().root\nwindows=[w for w in r.query_tree().children if w.get_wm_name()=="Herdr"]\nassert windows, "private Herdr window missing"\nw=windows[0]\np=r.translate_coords(w, 28, 14)\nkey=d.keysym_to_keycode(XK.string_to_keysym("Control_L"))\nxtest.fake_input(d,X.MotionNotify,x=p.x,y=p.y)\nxtest.fake_input(d,X.KeyPress,key)\nxtest.fake_input(d,X.ButtonPress,1)\nxtest.fake_input(d,X.ButtonRelease,1)\nxtest.fake_input(d,X.KeyRelease,key)\nd.sync()\n`;
  await cmd(["uv", "run", "--no-project", "--with", "python-xlib", "python", "-c", input]);
  await eventually(async () => { try { return !!(await readFile(join(home, "captured.json"), "utf8")); } catch { return false; } }, "native URL event reached mod");
  const captured = JSON.parse(await readFile(join(home, "captured.json"), "utf8"));
  if (captured.url !== url || captured.terminal !== launched.terminal || captured.machine !== null || !captured.activated)
    throw new Error("Clicked source/OSC8 target mismatch: " + JSON.stringify(captured));
  const status = JSON.parse(await cmd([bin, "status"]));
  if (status.errors.length) throw new Error(JSON.stringify(status.errors));
  console.log("PASS: native Ctrl-click on an OSC8 link delivers the actual URL and clicked source to the personal mod");
  console.log("PASS: private app has no mod/file errors; real desktop/core untouched");
} catch (error) {
  console.error("Artifacts retained at " + root);
  console.error(await Bun.file(join(root, "app.log")).text().catch(() => ""));
  throw error;
} finally {
  app?.kill();
  await app?.exited;
  await cmd(["hc", "server", "stop"]).catch(() => {});
  xvfb?.kill();
  await xvfb?.exited;
  // Keep diagnostic screenshots/logs; the private core and display are stopped.
}
