import { expect, test } from "bun:test";
import { isRemoteLoopbackUrl } from "./mod/policy.js";
for (const url of ["http://localhost:3000/a", "https://LOCALHOST/", "http://127.0.0.1:3000", "http://[::1]:3000",
  "http://localhost./", "http://127.0.0.2/", "http://127.1/", "http://user:pass@localhost:3000", "http://localhost:99999"]) {
  test("routes remote loopback " + url, () => {
    expect(isRemoteLoopbackUrl({ url, machine: "dev" })).toBe(true);
    expect(isRemoteLoopbackUrl({ url, machine: null })).toBe(false);
  });
}
for (const url of ["https://example.com", "http://localhost.evil/", "file:///tmp/file", "http://localhost@evil/",
  "http://[::]:3000", "http://10.0.0.1:3000", "ssh://localhost"]) {
  test("passes through ordinary link " + url, () => expect(isRemoteLoopbackUrl({ url, machine: "dev" })).toBe(false));
}
