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

test("SFTP file and directory rows use supported element methods", () => {
  const supported = new Set(["id", "h", "w", "px", "py", "mx", "mb", "p", "gap", "items_center", "justify_center", "rounded", "cursor_pointer", "hover", "tooltip", "on_mouse_down", "on_click", "child", "size", "text_color", "flex_none", "flex_1", "min_w", "truncate", "text_size", "border_1", "border_color", "bg"]);
  const element = () => {
    const el = {};
    for (const method of supported) el[method] = () => el;
    el.when = (condition, fn) => condition ? fn(el) : el;
    return el;
  };
  const classSource = sftpSource.slice(sftpSource.indexOf("export class SftpBrowser")).replace("export class SftpBrowser", "class SftpBrowser");
  const Browser = new Function("v_flex", "h_flex", "div", "svg", "formatBytes", `${classSource}\nreturn SftpBrowser;`)(element, element, element, element, helpers.formatBytes);
  const browser = Object.create(Browser.prototype);
  const theme = {};
  for (const entry of [{ path: "/file", name: "file", size: 10, isDirectory: false }, { path: "/dir", name: "dir", isDirectory: true }]) {
    browser.selectedPath = entry.path;
    browser.menuEntry = entry;
    assert.doesNotThrow(() => browser.renderRow(entry, theme));
  }
});

test("remote preview reads the selected machine and ignores stale results", async () => {
  let resolveRead;
  const readTextFile = (path, options) => {
    assert.equal(path, "/notes.txt");
    assert.equal(options.on, "server");
    return new Promise(resolve => { resolveRead = resolve; });
  };
  const classSource = sftpSource.slice(sftpSource.indexOf("export class SftpBrowser")).replace("export class SftpBrowser", "class SftpBrowser");
  const EditorState = { new: options => ({ options, release() {} }) };
  const Browser = new Function("readTextFile", "EditorState", "settings", "PREVIEW_LANG", `${classSource}\nreturn SftpBrowser;`)(readTextFile, EditorState, () => ({}), {});
  const browser = new Browser();
  browser.machineId = "server";
  let pending;
  const cx = { notify() {}, spawn(fn) { pending = fn(cx); } };
  browser.openPreview({ path: "/notes.txt", name: "notes.txt", size: 4 }, {}, cx);
  resolveRead("hello");
  await pending;
  assert.equal(browser.previewCode.options.value, "hello");
  assert.equal(browser.previewCode.options.readonly, true);
  browser.openPreview({ path: "/notes.txt", name: "notes.txt", size: 4 }, {}, cx);
  browser.closePreview();
  resolveRead("stale");
  await pending;
  assert.equal(browser.preview, null);
  assert.equal(browser.previewCode, null);
});
