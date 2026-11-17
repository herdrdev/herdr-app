import { afterAll, describe, expect, test } from "bun:test";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { chromeArgs, chromeEndpoint, locations, openChromeTab, rpc, sshArgs, sshTarget, validateOrigin } from "./mod/runner";

const origin = (url = "http://localhost:3000/a?x=1#fragment") => ({ url, terminal: "dev/t7", machine: "dev" });
const root = await mkdtemp(join(tmpdir(), "herdr-remote-links-test-"));
afterAll(() => rm(root, { recursive: true, force: true }));

describe("link routing", () => {
  for (const host of ["localhost", "LOCALHOST", "localhost.", "127.0.0.1", "127.0.0.2", "[::1]"]) {
    test("accepts remote loopback " + host, () => {
      const o = origin("https://" + host + ":4443/a?next=http://localhost:8000#b");
      expect(validateOrigin(o)).toEqual(o);
    });
  }
  for (const url of ["http://localhost.evil.test:3000", "http://example.com", "file:///etc/passwd", "ssh://localhost",
    "http://localhost@evil.test", "http://user:pass@localhost/", "http://[::]:3000", "http://0.0.0.0:3000",
    "http://localhost:99999", "http://localhost/\n", "--proxy-server=direct://"]) {
    test("rejects " + JSON.stringify(url), () => expect(() => validateOrigin(origin(url))).toThrow());
  }
  test("never takes machine from focused terminal", () => {
    expect(() => validateOrigin({ ...origin(), machine: "other" })).toThrow();
    expect(() => validateOrigin({ ...origin(), terminal: "t7" })).toThrow();
    expect(() => validateOrigin({ ...origin(), terminal: "dev/" })).toThrow();
    expect(() => validateOrigin({ ...origin(), machine: null })).toThrow();
  });
});

describe("transport policy", () => {
  test("uses saved SSH target, not display name", () => {
    expect(sshTarget([{ name: "dev", target: "dev-tobi", via: false }], "dev")).toBe("dev-tobi");
  });
  test("refuses unsupported transports, temporary machines and option injection", () => {
    for (const m of [{ name: "dev", target: "dev", via: true }, { name: "dev", target: "-oProxyCommand=evil" },
      { name: "dev", target: "dev", temporary: true }, { name: "dev", target: "dev host" }])
      expect(() => sshTarget([m], "dev")).toThrow();
  });
  test("binds only local loopback, verifies forwarding, keeps failures bounded", () => {
    const args = sshArgs("dev-tobi", 9051, "/private/master");
    expect(args).toContain("127.0.0.1:9051");
    expect(args).toContain("ExitOnForwardFailure=yes");
    expect(args).toContain("BatchMode=yes");
    expect(args).toContain("ControlPersist=no");
    expect(args.slice(-2)).toEqual(["--", "dev-tobi"]);
  });
  test("Chrome preserves original URL and cannot bypass loopback proxy", () => {
    const url = origin().url;
    const args = chromeArgs(9051, "/profile with spaces", url, "dev");
    expect(args).toContain("--proxy-server=socks5://127.0.0.1:9051");
    expect(args).toContain("--proxy-bypass-list=<-loopback>");
    expect(args).toContain("--disable-quic");
    expect(args).toContain("--force-webrtc-ip-handling-policy=disable_non_proxied_udp");
    expect(args).toContain("--user-data-dir=/profile with spaces");
    expect(args.at(-1)).toBe(url);
  });
  test("profiles are isolated by machine AND app home", () => {
    expect(locations("dev", "/one").profile).not.toBe(locations("other", "/one").profile);
    expect(locations("dev", "/one").profile).not.toBe(locations("dev", "/two").profile);
    expect(locations("dev", "/one").socket.length).toBeLessThan(104);
    expect(locations("dev", "/one").name.length).toBeLessThanOrEqual(32);
  });
});

describe("dedicated browser control", () => {
  test("opens an exact URL only in the identified browser", async () => {
    const profile = join(root, "profile");
    await mkdir(profile);
    let opened = "";
    const server = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch(request) {
      if (new URL(request.url).pathname === "/json/version")
        return Response.json({ webSocketDebuggerUrl: `ws://127.0.0.1:${server.port}/devtools/browser/test-id` });
      expect(request.method).toBe("PUT");
      opened = decodeURIComponent(new URL(request.url).search.slice(1));
      return Response.json({ id: "new-tab" });
    } });
    try {
      await writeFile(join(profile, "DevToolsActivePort"), `${server.port}\n/devtools/browser/test-id\n`);
      await openChromeTab(profile, origin().url);
      expect(opened).toBe(origin().url);
    } finally { server.stop(true); }
  });
  test("refuses a stale endpoint identifying a different browser", async () => {
    const profile = join(root, "stale-profile");
    await mkdir(profile);
    const server = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch() {
      return Response.json({ webSocketDebuggerUrl: `ws://127.0.0.1:${server.port}/devtools/browser/someone-else` });
    } });
    try {
      await writeFile(join(profile, "DevToolsActivePort"), `${server.port}\n/devtools/browser/expected\n`);
      await expect(chromeEndpoint(profile)).rejects.toThrow("different browser");
    } finally { server.stop(true); }
  });
  test("refuses an invalid control port", async () => {
    const profile = join(root, "invalid-profile");
    await mkdir(profile);
    await writeFile(join(profile, "DevToolsActivePort"), "99999\n/devtools/browser/id\n");
    await expect(chromeEndpoint(profile)).rejects.toThrow("Invalid Chrome");
  });
});

describe("local broker protocol", () => {
  test("an absent socket requests a lazy launch", async () => {
    expect(await rpc(join(root, "missing.sock"), origin())).toBeNull();
  });
  test("passes the exact source and returns tracked ownership", async () => {
    const socket = join(root, "echo.sock");
    const server = Bun.serve({ unix: socket, async fetch(request) {
      expect(await request.json()).toEqual(origin());
      return Response.json({ machine: "dev", terminal: "t20", owners: ["dev/t7"], port: 9051, profile: "/p" });
    } });
    try {
      expect(await rpc(socket, origin())).toEqual({ machine: "dev", terminal: "t20", owners: ["dev/t7"], port: 9051, profile: "/p" });
    } finally { server.stop(true); }
  });
  test("a refused launch never falls back to the ordinary local browser", async () => {
    const socket = join(root, "fail.sock");
    const server = Bun.serve({ unix: socket, fetch() { return Response.json({ error: "SSH login needed" }, { status: 400 }); } });
    try { await expect(rpc(socket, origin())).rejects.toThrow("SSH login needed"); }
    finally { server.stop(true); }
  });
});
