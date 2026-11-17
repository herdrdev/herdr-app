#!/usr/bin/env bun
/** Standalone prototype: all subprocesses run on the herdr-app computer. */
import { createHash } from "node:crypto";
import { installBrowserBranding } from "./branding";
import { chmod, lstat, mkdir, readFile, readlink, unlink } from "node:fs/promises";
import { createServer } from "node:net";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";

export type Origin = { url: string; terminal: string; machine: string };
export type Machine = { name: string; target: string; via?: boolean; temporary?: boolean };
export type Session = { machine: string; terminal: string; owners: string[]; port: number; profile: string };

export function validateOrigin(value: unknown): Origin {
  const v = value as Origin;
  if (!v || typeof v.url !== "string" || typeof v.machine !== "string" || typeof v.terminal !== "string")
    throw new Error("Expected {url, terminal, machine}");
  if (!v.machine || v.machine.includes("/") || !v.terminal.startsWith(`${v.machine}/`) || v.terminal.length === v.machine.length + 1)
    throw new Error("The terminal must belong to the supplied remote machine");
  if ([v.url, v.machine, v.terminal].some(s => /[\x00-\x1f\x7f]/.test(s)) || v.url.length > 8192)
    throw new Error("Invalid link or terminal address");
  const url = new URL(v.url);
  const loopback = ["localhost", "localhost.", "[::1]"].includes(url.hostname) || /^127\./.test(url.hostname);
  if (!["http:", "https:"].includes(url.protocol) || !loopback || url.username || url.password)
    throw new Error("Only remote HTTP(S) loopback links without credentials are supported");
  // Keep the original origin, path, query and fragment. Never remap its port.
  return { url: v.url, terminal: v.terminal, machine: v.machine };
}

export function sshTarget(machines: Machine[], machine: string): string {
  const m = machines.find(m => m.name === machine && !m.temporary);
  if (!m) throw new Error(`No saved machine named ${machine}`);
  if (m.via) throw new Error("This prototype needs an SSH machine; custom --via transports need native core forwarding");
  if (!m.target || m.target.startsWith("-") || /[\s\x00-\x1f\x7f]/.test(m.target))
    throw new Error("The saved machine has an invalid SSH target");
  return m.target;
}

export function sshArgs(target: string, port: number, control: string): string[] {
  return ["-N", "-M", "-S", control, "-D", `127.0.0.1:${port}`,
    "-o", "ExitOnForwardFailure=yes", "-o", "ControlPersist=no", "-o", "BatchMode=yes",
    "-o", "ConnectTimeout=8", "-o", "ServerAliveInterval=30", "-o", "ServerAliveCountMax=3", "--", target];
}

export function chromeArgs(port: number, profile: string, url: string, machine: string): string[] {
  return [`--proxy-server=socks5://127.0.0.1:${port}`, "--proxy-bypass-list=<-loopback>",
    "--force-webrtc-ip-handling-policy=disable_non_proxied_udp", "--disable-quic",
    `--user-data-dir=${profile}`, `--class=herdr-remote-${machine.replace(/[^A-Za-z0-9_-]/g, "_")}`,
    `--window-name=Herdr · ${machine}`, "--remote-debugging-address=127.0.0.1", "--remote-debugging-port=0",
    "--disable-background-mode", "--disable-sync", "--no-first-run", "--no-default-browser-check", url];
}

async function privateDir(dir: string) {
  await mkdir(dir, { recursive: true, mode: 0o700 });
  const s = await lstat(dir);
  if (!s.isDirectory() || s.isSymbolicLink() || (process.getuid && s.uid !== process.getuid()))
    throw new Error(`Unsafe private directory: ${dir}`);
  await chmod(dir, 0o700);
}

export function locations(machine: string, scope = process.env.HERDR_APP_HOME ?? join(homedir(), ".config/herdr-app")) {
  const hash = createHash("sha256").update(`${scope}\0${machine}`).digest("hex").slice(0, 20);
  const runtime = process.env.HERDR_REMOTE_LINKS_RUNTIME ?? join(process.env.XDG_RUNTIME_DIR ?? tmpdir(), `herdr-remote-links-${process.getuid?.() ?? "user"}`);
  const cache = process.env.HERDR_REMOTE_LINKS_CACHE ?? join(homedir(), ".cache/herdr-remote-links");
  return { runtime, socket: join(runtime, `${hash}.sock`), control: join(runtime, `${hash}.ssh`),
    profile: join(cache, hash), name: `remote-links-${hash.slice(0, 18)}` };
}

async function command(argv: string[]) {
  const child = Bun.spawn(argv, { stdin: "ignore", stdout: "pipe", stderr: "pipe" });
  const timer = setTimeout(() => child.kill(), 30000);
  try {
    const [code, stdout, stderr] = await Promise.all([child.exited, new Response(child.stdout).text(), new Response(child.stderr).text()]);
    if (code !== 0) throw new Error(`${argv[0]} failed (${code}): ${stderr.trim() || stdout.trim()}`);
    return stdout;
  } finally { clearTimeout(timer); }
}

export async function chromeEndpoint(profile: string): Promise<string> {
  const [port, socketPath] = (await readFile(join(profile, "DevToolsActivePort"), "utf8")).trim().split("\n");
  if (!/^\d+$/.test(port) || Number(port) < 1 || Number(port) > 65535 || !socketPath?.startsWith("/devtools/browser/"))
    throw new Error("Invalid Chrome control endpoint");
  const endpoint = `http://127.0.0.1:${port}`;
  const version = await (await fetch(endpoint + "/json/version", { signal: AbortSignal.timeout(1000) })).json() as { webSocketDebuggerUrl?: string };
  if (!version.webSocketDebuggerUrl || new URL(version.webSocketDebuggerUrl).pathname !== socketPath)
    throw new Error("Chrome control endpoint belongs to a different browser");
  return endpoint;
}

export async function openChromeTab(profile: string, url: string) {
  const endpoint = await chromeEndpoint(profile);
  const response = await fetch(endpoint + "/json/new?" + encodeURIComponent(url), { method: "PUT", signal: AbortSignal.timeout(3000) });
  if (!response.ok) throw new Error("Chrome refused to open the link");
}

async function assertProfileAvailable(profile: string) {
  let lock: string;
  try { lock = await readlink(join(profile, "SingletonLock")); }
  catch (error) { if ((error as NodeJS.ErrnoException).code === "ENOENT") return; throw error; }
  const pid = Number(lock.split("-").at(-1));
  if (!Number.isInteger(pid) || pid <= 0) throw new Error("Unrecognized Chrome profile lock");
  try { process.kill(pid, 0); }
  catch (error) { if ((error as NodeJS.ErrnoException).code === "ESRCH") return; throw error; }
  throw new Error("This machine's Chrome profile is already in use; close that browser first");
}

async function availablePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const server = createServer();
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      if (!address || typeof address === "string") return reject(new Error("No local TCP port"));
      server.close(error => error ? reject(error) : resolve(address.port));
    });
  });
}

async function chromeBinary(): Promise<string> {
  // Overrides are useful for offscreen testing, not taken from terminal text.
  if (process.env.HERDR_REMOTE_LINKS_CHROME) return process.env.HERDR_REMOTE_LINKS_CHROME;
  const candidates = process.platform === "darwin" ? [
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
  ] : ["google-chrome-stable", "google-chrome", "chromium", "chromium-browser"];
  for (const path of candidates) if (Bun.which(path)) return path;
  throw new Error("Chrome or Chromium is not installed on the herdr-app computer");
}

export async function rpc(socket: string, origin: Origin): Promise<Session | null> {
  let response: Response;
  try {
    response = await fetch("http://localhost/open", { unix: socket, method: "POST", body: JSON.stringify(origin), signal: AbortSignal.timeout(20000) });
  } catch (error) {
    // Only an absent/refusing socket means 'start a broker'. Never mask startup failures.
    if (["ENOENT", "ECONNREFUSED", "FailedToOpenSocket"].includes((error as NodeJS.ErrnoException).code ?? "")) return null;
    throw error;
  }
  const data = await response.json() as Session & { error?: string };
  if (!response.ok) throw new Error(data.error ?? "Remote browser failed");
  return data;
}

export async function open(origin: Origin): Promise<Session> {
  origin = validateOrigin(origin);
  const loc = locations(origin.machine);
  await privateDir(loc.runtime);
  const existing = await rpc(loc.socket, origin);
  if (existing) return existing;
  const saved = JSON.parse(await command(["hc", "machine", "ls", "--json"]));
  const target = sshTarget(saved.machines, origin.machine);
  // A core parent cannot name a terminal on another machine. Preserve explicit ownership
  // as a label rather than falsely adopting this local job into a remote machine's tab.
  let launchError: unknown;
  try {
    await command(["hc", "run", "--bg", "--json", "--no-parent", "--no-notify", "--name", loc.name,
      "--label", `remote-links.origin=${origin.terminal}`, "--label", `remote-links.machine=${origin.machine}`,
      "--", process.execPath, import.meta.path, "serve", origin.machine, target, JSON.stringify({ ...loc, chrome: await chromeBinary() })]);
  } catch (error) { launchError = error; } // Concurrent launches use hc's unique live name.
  const until = Date.now() + 10000;
  while (Date.now() < until) {
    const session = await rpc(loc.socket, origin);
    if (session) return session;
    await Bun.sleep(100);
  }
  throw new Error(`Remote browser broker did not start. ${launchError ?? `Inspect hc get ${loc.name}`}`);
}

export async function serve(machine: string, target: string, loc: ReturnType<typeof locations> & { chrome?: string } = locations(machine)) {
  await privateDir(loc.runtime);
  await privateDir(loc.profile);
  // Refuse a duplicate direct 'serve' invocation; hc run normally arbitrates startup.
  const probe = validateOrigin({ machine, terminal: `${machine}/probe`, url: "http://localhost/" });
  let connected = false;
  try {
    const res = await fetch("http://localhost/health", { unix: loc.socket, signal: AbortSignal.timeout(1000) });
    connected = res.ok;
  } catch (error) {
    if (!["ENOENT", "ECONNREFUSED", "FailedToOpenSocket"].includes((error as NodeJS.ErrnoException).code ?? "")) throw error;
  }
  if (connected) throw new Error("A broker is already running for this machine");
  await unlink(loc.socket).catch(error => { if (error.code !== "ENOENT") throw error; });
  await unlink(loc.control).catch(error => { if (error.code !== "ENOENT") throw error; });
  const browser = loc.chrome ?? await chromeBinary();
  let tunnel: ReturnType<typeof Bun.spawn> | undefined;
  let chrome: ReturnType<typeof Bun.spawn> | undefined;
  let port = 0;
  let closing = false;
  let closeBranding: (() => void) | undefined;
  let ready = false;
  const owners = new Set<string>();
  let queue = Promise.resolve();
  let errors = "";

  async function stop() {
    if (closing) return;
    closing = true;
    closeBranding?.();
    server.stop();
    tunnel?.kill();
    chrome?.kill();
    await Promise.all([tunnel?.exited, chrome?.exited]);
    await unlink(loc.socket).catch(() => {});
    await unlink(loc.control).catch(() => {});
    process.exit(0);
  }

  async function connect() {
    port = await availablePort();
    tunnel = Bun.spawn(["ssh", ...sshArgs(target, port, loc.control)], { stdin: "ignore", stdout: "ignore", stderr: "pipe" });
    const thisTunnel = tunnel;
    (async () => {
      const reader = (thisTunnel.stderr as ReadableStream<Uint8Array>).getReader();
      const decoder = new TextDecoder();
      for (;;) {
        const part = await reader.read();
        if (part.done) break;
        errors = (errors + decoder.decode(part.value)).slice(-4096);
      }
    })().catch(() => {});
    const deadline = Date.now() + 10000;
    while (Date.now() < deadline && thisTunnel.exitCode === null) {
      try {
        // Checking our private SSH master proves OUR forwarding succeeded. Merely
        // connecting to the port could mistake another process for our proxy.
        await command(["ssh", "-S", loc.control, "-O", "check", "--", target]);
        return;
      } catch { await Bun.sleep(100); }
    }
    thisTunnel.kill();
    await thisTunnel.exited;
    tunnel = undefined;
    throw new Error(`SSH SOCKS connection failed: ${errors.trim() || "startup timed out"}. Check SSH login in a terminal.`);
  }

  async function visit(origin: Origin): Promise<Session> {
    if (closing) throw new Error("Remote browser is shutting down; retry the click");
    if (origin.machine !== machine) throw new Error("This broker belongs to a different machine");
    if (!ready) {
      await assertProfileAvailable(loc.profile);
      await unlink(join(loc.profile, "DevToolsActivePort")).catch(error => { if (error.code !== "ENOENT") throw error; });
      await connect();
      chrome = Bun.spawn([browser, ...chromeArgs(port, loc.profile, origin.url, machine)], { stdin: "ignore", stdout: "ignore", stderr: "ignore" });
      // A second Chrome process must never take over a profile from another broker.
      // Profiles are private to this app-home/machine, and server startup is serialized.
      const deadline = Date.now() + 10000;
      let browserReady = false;
      while (Date.now() < deadline && chrome.exitCode === null) {
        try { await chromeEndpoint(loc.profile); browserReady = true; break; }
        catch { await Bun.sleep(100); }
      }
      if (!browserReady || chrome.exitCode !== null) {
        chrome.kill();
        await chrome.exited;
        tunnel?.kill();
        await tunnel?.exited;
        tunnel = undefined;
        throw new Error("Chrome exited during startup; inspect its dedicated profile lock");
      }
      try { closeBranding = await installBrowserBranding(await chromeEndpoint(loc.profile), machine); }
      catch (error) { console.warn("Browser title branding unavailable:", String(error)); }
      ready = true;
      chrome.exited.then(stop);
      tunnel!.exited.then(() => { console.error("SSH connection ended; closing its dedicated browser"); return stop(); });
      console.log(`SOCKS ready: ${machine}, local port ${port}. Close this browser to stop it.`);
    } else {
      // Reuse the browser over its private loopback control endpoint. This also
      // works headlessly and never risks a second process ignoring proxy flags.
      await openChromeTab(loc.profile, origin.url);
    }
    owners.add(origin.terminal);
    return { machine, terminal: process.env.HC_TERMINAL_ID ?? "", owners: [...owners], port, profile: loc.profile };
  }

  const server = Bun.serve({ unix: loc.socket, maxRequestBodySize: 16384, async fetch(request) {
    if (new URL(request.url).pathname === "/health") return Response.json({ machine });
    if (request.method !== "POST" || new URL(request.url).pathname !== "/open") return new Response("Not found", { status: 404 });
    try {
      const origin = validateOrigin(await request.json());
      const task = queue.then(() => visit(origin));
      queue = task.then(() => {}, () => {});
      return Response.json(await task);
    } catch (error) { return Response.json({ error: String(error) }, { status: 400 }); }
  } });
  await chmod(loc.socket, 0o600);
  process.on("SIGTERM", stop);
  process.on("SIGINT", stop);
  process.on("SIGHUP", stop);
  // If the launch has no request (e.g. the caller dies), don't leak an idle helper.
  const idle = setTimeout(() => { if (!ready) void stop(); }, 30000);
  idle.unref();
  console.log(`Remote browser broker ready for ${probe.machine}`);
}

if (import.meta.main) {
  try {
    const [action, ...args] = process.argv.slice(2);
    if (action === "open" && args.length === 3) console.log(JSON.stringify(await open({ machine: args[0], terminal: args[1], url: args[2] })));
    else if (action === "serve" && (args.length === 2 || args.length === 3)) await serve(args[0], args[1], args[2] ? JSON.parse(args[2]) : undefined);
    else throw new Error("Usage: bun runner.ts open MACHINE MACHINE/TERMINAL URL | serve MACHINE SSH_TARGET");
  } catch (error) { console.error(String(error)); process.exitCode = 1; }
}
