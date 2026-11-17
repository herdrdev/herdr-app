#!/usr/bin/env bun
/** Opt-in real SSH + headless Chrome test. Creates and removes only its own jobs. */
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { createConnection } from "node:net";
import { locations, open, rpc, sshTarget } from "./mod/runner";

const machine = process.argv[2];
if (!machine) throw new Error("Usage: bun smoke.ts SAVED_SSH_MACHINE");
const root = await mkdtemp(join(tmpdir(), "herdr-remote-links-smoke-"));
let fixture = "";
let brokerName = "";
async function cmd(argv: string[]) {
  const p = Bun.spawn(argv, { stdout: "pipe", stderr: "pipe", stdin: "ignore" });
  const timer = setTimeout(() => p.kill(), 30000);
  try {
    const [code, out, err] = await Promise.all([p.exited, new Response(p.stdout).text(), new Response(p.stderr).text()]);
    if (code) throw new Error(`${argv[0]} failed (${code}): ${err || out}`);
    return out;
  } finally { clearTimeout(timer); }
}
async function eventually<T>(get: () => Promise<T | undefined>, description: string): Promise<T> {
  const until = Date.now() + 15000;
  while (Date.now() < until) {
    const value = await get();
    if (value !== undefined) return value;
    await Bun.sleep(100);
  }
  throw new Error("Timed out: " + description);
}
async function assertClosed(port: number) {
  await new Promise<void>((resolve, reject) => {
    const socket = createConnection({ host: "127.0.0.1", port });
    socket.setTimeout(1000);
    socket.once("connect", () => { socket.destroy(); reject(new Error(`Listener ${port} leaked`)); });
    socket.once("error", error => (error as NodeJS.ErrnoException).code === "ECONNREFUSED" ? resolve() : reject(error));
    socket.once("timeout", () => { socket.destroy(); reject(new Error("Listener check timed out")); });
  });
}
async function cdpText(webSocketDebuggerUrl: string, expression = "document.body?.innerText"): Promise<string> {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(webSocketDebuggerUrl);
    const timer = setTimeout(() => { ws.close(); reject(new Error("CDP timed out")); }, 3000);
    ws.onopen = () => ws.send(JSON.stringify({ id: 1, method: "Runtime.evaluate", params: { expression, returnByValue: true, awaitPromise: true } }));
    ws.onerror = error => { clearTimeout(timer); ws.close(); reject(error); };
    ws.onmessage = event => {
      const msg = JSON.parse(String(event.data));
      if (msg.id !== 1) return;
      clearTimeout(timer);
      ws.close();
      resolve(msg.result?.result?.value ?? "");
    };
  });
}

try {
  const saved = JSON.parse(await cmd(["hc", "machine", "ls", "--json"]));
  const target = sshTarget(saved.machines, machine);
  const perl = `$|=1; my @s=map { IO::Socket::INET->new(LocalAddr=>"127.0.0.1",LocalPort=>0,Listen=>10,ReuseAddr=>1) or die $! } 1..2;
print "REMOTE_LINKS_PORTS ",join(" ",map { $_->sockport } @s),"\\n";
my $sel=IO::Select->new(@s); while(1){ for my $s ($sel->can_read){ my $c=$s->accept; my $line=<$c>; my ($path)=$line=~m{^GET (\\S+)};
my $key=""; while(my $h=<$c>){last if $h eq "\\r\\n"; $key=$1 if $h=~/Sec-WebSocket-Key: (\\S+)/i} if($path eq "/ws"){ my $accept=sha1_base64($key."258EAFA5-E914-47DA-95CA-C5AB0DC85B11")."="; my $msg="HERDR_REMOTE_WS_OK"; print $c "HTTP/1.1 101 Switching Protocols\\r\\nUpgrade: websocket\\r\\nConnection: Upgrade\\r\\nSec-WebSocket-Accept: $accept\\r\\n\\r\\n",chr(129),chr(length($msg)),$msg; } elsif($path=~m{^/redirect}){ print $c "HTTP/1.1 302 Found\\r\\nLocation: http://localhost:",$s[1]->sockport,"/second?kept=yes\\r\\nContent-Length: 0\\r\\nConnection: close\\r\\n\\r\\n" }
else { my $body="<html><body>HERDR_REMOTE_SOCKS_OK $path</body></html>"; print $c "HTTP/1.1 200 OK\\r\\nContent-Type: text/html\\r\\nContent-Length: ",length($body),"\\r\\nConnection: close\\r\\n\\r\\n",$body } close $c } }`;
  const launch = JSON.parse(await cmd(["hc", "run", "--on", machine, "--bg", "--no-parent", "--no-notify", "--name", `rl-probe-${process.pid}`, "--",
    "perl", "-MIO::Socket::INET", "-MIO::Select", "-MDigest::SHA=sha1_base64", "-e", perl]));
  fixture = launch.address;
  const ports = await eventually(async () => {
    const text = await cmd(["hc", "read", fixture, "--lines", "12"]);
    const match = text.match(/REMOTE_LINKS_PORTS (\d+) (\d+)/);
    return match ? [Number(match[1]), Number(match[2])] : undefined;
  }, "remote HTTP fixture ready");
  await mkdir(join(root, "runtime"), { mode: 0o700 });
  await writeFile(join(root, "chrome"), '#!/bin/sh\nexec google-chrome-stable --headless --remote-debugging-port=0 --disable-dev-shm-usage "$@"\n', { mode: 0o700 });
  // Isolate only OUR files, not XDG_RUNTIME_DIR: SSH ProxyCommand authentication
  // services (e.g. mint) still need the real desktop runtime directory.
  Object.assign(process.env, { HERDR_REMOTE_LINKS_RUNTIME: join(root, "runtime"), HERDR_APP_HOME: join(root, "app"), HERDR_REMOTE_LINKS_CACHE: join(root, "profiles"), HERDR_REMOTE_LINKS_CHROME: join(root, "chrome") });
  const loc = locations(machine);
  brokerName = loc.name;
  const initial = { machine, terminal: fixture, url: `http://127.0.0.1:${ports[0]}/redirect?original=1#fragment` };
  // Exercise the real CLI launch path, including hc tracking and app-home propagation.
  const session = await open(initial);
  const job = JSON.parse(await cmd(["hc", "get", session.terminal, "--json"]));
  if (job.labels?.["remote-links.origin"] !== fixture) throw new Error("Origin ownership label missing");
  console.log("PASS: local hc helper tracks its remote originating terminal");
  console.log("PASS: lazy SOCKS + headless local Chrome startup");
  const debugPort = await eventually(async () => {
    try { return Number((await readFile(join(loc.profile, "DevToolsActivePort"), "utf8")).split("\n")[0]) || undefined; }
    catch { return undefined; }
  }, "Chrome debugging port");
  async function expectPage(url: string, text: string) {
    await eventually(async () => {
      const tabs = await (await fetch(`http://127.0.0.1:${debugPort}/json/list`)).json() as any[];
      const tab = tabs.find(t => t.type === "page" && t.url === url);
      if (!tab) return undefined;
      return (await cdpText(tab.webSocketDebuggerUrl)).includes(text) ? true : undefined;
    }, "remote content in Chrome: " + url);
  }
  // Browsers inherit the original fragment when a redirect omits its own.
  await expectPage(`http://localhost:${ports[1]}/second?kept=yes#fragment`, "HERDR_REMOTE_SOCKS_OK /second?kept=yes");
  console.log("PASS: redirect to another remote localhost port (no per-port forwards)");
  const urls = [`http://127.0.0.1:${ports[0]}/one?kept=1#fragment`, `http://localhost:${ports[1]}/two?kept=2`];
  const sessions = await Promise.all(urls.map(url => rpc(loc.socket, { ...initial, url })));
  if (sessions.some(s => s?.port !== session.port || s?.profile !== session.profile)) throw new Error("Did not reuse browser/SOCKS");
  for (const [i, url] of urls.entries()) await expectPage(url, i ? "/two?kept=2" : "/one?kept=1");
  console.log("PASS: concurrent repeat clicks reuse the same SOCKS/profile; queries and fragments preserved");
  const tabs = await (await fetch(`http://127.0.0.1:${debugPort}/json/list`)).json() as any[];
  const tab = tabs.find(t => t.type === "page" && t.url === urls[0]);
  const expression = `new Promise(resolve => { const ws = new WebSocket('ws://localhost:${ports[1]}/ws'); ws.onmessage = event => { resolve(event.data); ws.close(); }; ws.onerror = () => resolve('ERROR'); setTimeout(() => resolve('TIMEOUT'), 2000); })`;
  if (await cdpText(tab.webSocketDebuggerUrl, expression) !== "HERDR_REMOTE_WS_OK") throw new Error("Remote WebSocket did not use SOCKS");
  console.log("PASS: page WebSocket to another remote loopback port uses SOCKS");
  const prefix = "Herdr · " + machine;
  await eventually(async () => {
    const current = await (await fetch(`http://127.0.0.1:${debugPort}/json/list`)).json() as any[];
    return urls.every(url => current.some(t => t.url === url && t.title.startsWith(prefix))) ? true : undefined;
  }, "browser titles branded on all newly opened tabs");
  await cdpText(tab.webSocketDebuggerUrl, "document.title = 'Reactive app title'");
  await eventually(async () => {
    const current = await (await fetch(`http://127.0.0.1:${debugPort}/json/list`)).json() as any[];
    return current.some(t => t.url === urls[0] && t.title === prefix + " — Reactive app title") ? true : undefined;
  }, "browser title prefix survives page title changes");
  console.log("PASS: machine-prefixed titles on multiple tabs survive reactive page title changes");
  await cmd(["hc", "kill", session.terminal]);
  await cmd(["hc", "wait", session.terminal, "--timeout", "10"]);
  await cmd(["hc", "rm", session.terminal]);
  brokerName = "";
  if (await rpc(loc.socket, initial)) throw new Error("Broker socket leaked after shutdown");
  await assertClosed(session.port);
  await assertClosed(debugPort);
  console.log("PASS: broker shutdown removes local listener and closes its dedicated browser");
} catch (error) {
  if (brokerName) console.error(await cmd(["hc", "read", brokerName, "--lines", "40"]).catch(() => "No broker diagnostics"));
  throw error;
} finally {
  if (brokerName) {
    await cmd(["hc", "kill", brokerName]).catch(() => {});
    await cmd(["hc", "wait", brokerName, "--timeout", "10"]).catch(() => {});
    await cmd(["hc", "rm", brokerName]).catch(() => {});
  }
  if (fixture) {
    await cmd(["hc", "kill", fixture]).catch(() => {});
    await cmd(["hc", "rm", fixture]).catch(() => {});
  }
  await rm(root, { recursive: true, force: true });
}
