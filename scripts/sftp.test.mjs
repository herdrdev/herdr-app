import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";

const sftpSource = await readFile(new URL("../defaults/mods/files/sftp.js", import.meta.url), "utf8");
const mainSource = await readFile(new URL("../defaults/mods/files/main.js", import.meta.url), "utf8");

// Extract pure helper functions from sftp.js without requiring gpui/herdr runtime modules.
const helpersSource = sftpSource
  .slice(sftpSource.indexOf("export function normalizePosixPath("), sftpSource.indexOf("export class SftpBrowser"))
  .replaceAll("export function ", "function ");

const helpers = new Function(
  `${helpersSource}\nreturn { normalizePosixPath, joinPosixPath, parentPosixPath, breadcrumbSegments, sortSftpEntries, filterSftpEntries, formatBytes, formatSftpError };`
)();

test("normalizePosixPath converts Windows backslashes and resolves dot segments", () => {
  assert.equal(helpers.normalizePosixPath("\\var\\log\\nginx\\"), "/var/log/nginx");
  assert.equal(helpers.normalizePosixPath("///home/user/../admin/./app//"), "/home/admin/app");
  assert.equal(helpers.normalizePosixPath("\\"), "/");
  assert.equal(helpers.normalizePosixPath(""), "/");
});

test("joinPosixPath and parentPosixPath enforce POSIX separators", () => {
  assert.equal(helpers.joinPosixPath("/home/user", "app.log"), "/home/user/app.log");
  assert.equal(helpers.joinPosixPath("\\", "etc"), "/etc");
  assert.throws(() => helpers.joinPosixPath("/home/user", "../etc"));
  assert.throws(() => helpers.joinPosixPath("/home/user", "a\\b"));
  assert.throws(() => helpers.joinPosixPath("/home/user", "a/b"));
  assert.equal(helpers.parentPosixPath("/home/user/app"), "/home/user");
  assert.equal(helpers.parentPosixPath("/home"), "/");
  assert.equal(helpers.parentPosixPath("/"), "/");
});

test("breadcrumbSegments builds clickable POSIX segments", () => {
  assert.deepEqual(helpers.breadcrumbSegments("/"), [{ label: "/", path: "/" }]);
  assert.deepEqual(helpers.breadcrumbSegments("/var/www/html"), [
    { label: "/", path: "/" },
    { label: "var", path: "/var" },
    { label: "www", path: "/var/www" },
    { label: "html", path: "/var/www/html" },
  ]);
});

test("filterSftpEntries hides dotfiles when disabled and sorts folders before files", () => {
  const entries = [
    { name: "z.txt", isDirectory: false },
    { name: ".env", isDirectory: false },
    { name: "src", isDirectory: true },
    { name: ".git", isDirectory: true },
    { name: "alpha", isDirectory: true },
    { name: "a.txt", isDirectory: false },
  ];
  assert.deepEqual(
    helpers.filterSftpEntries(entries, false).map(e => e.name),
    ["alpha", "src", "a.txt", "z.txt"]
  );
  assert.deepEqual(
    helpers.filterSftpEntries(entries, true).map(e => e.name),
    [".git", "alpha", "src", ".env", "a.txt", "z.txt"]
  );
});

test("formatSftpError highlights EACCES and ENOENT cleanly", () => {
  assert.match(helpers.formatSftpError(new Error("EACCES: Permission denied (/root)")), /Permission denied \(EACCES\)/);
  assert.match(helpers.formatSftpError(new Error("ENOENT: No such file")), /Path not found/);
});

test("main.js preserves directory() slice boundary for scripts/files.test.ts", () => {
  const start = mainSource.indexOf("async function directory(");
  const end = mainSource.indexOf("\nexport default class");
  assert.ok(start >= 0 && end > start, "directory() slice markers must stay intact");
  const slice = mainSource.slice(start, end);
  assert.ok(!slice.includes("SftpBrowser"), "directory() slice must stay clean");
});
