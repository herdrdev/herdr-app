// Context menus (Herdr's items). Opened as a popup with
// props { tab: "pane" | "tab" | "space" | "global" | "file", target }.
// ↑/↓ move, Enter runs, Esc closes. An item is [label, command, args];
// a command "worktree" opens the worktree dialog instead.

import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { run, props, closeOverlay, openOverlay, theme, keys, terminals, spaces, panes, exec, window, notify } from "herdr";

// Global in-memory clipboard for file operations (Cut/Copy/Paste)
let fileClipboard = null;

// Where a space is now: its first terminal's live folder, else where it was made.
function place(s) {
  const tabs = new Set((s.tabs ?? []).map((c) => c.id));
  return panes().find((p) => tabs.has(p.tab) && p.cwd)?.cwd || s.folder;
}

// A space's checkout: "main" (a repo's own), "linked" (a worktree), or null.
async function checkout(folder, on) {
  const top = await exec("git", ["rev-parse", "--show-toplevel"], { cwd: folder, on });
  const common = await exec("git", ["rev-parse", "--path-format=absolute", "--git-common-dir"], { cwd: folder, on });
  if (top.code !== 0 || common.code !== 0) return null;
  return common.stdout.trim() === `${top.stdout.trim()}/.git` ? "main" : "linked";
}

function items(p, git) {
  p = p ?? {};
  const tg = p.target ?? {};
  switch (p.tab) {
    case "file": {
      return [
        ["New File...", "file.new_file", tg],
        ["New Folder...", "file.new_folder", tg],
        ["Open in Integrated Terminal", "file.open_terminal", tg],
        ["Find in Folder...", "file.find_in_folder", tg],
        ["Cut", "file.cut", tg],
        ["Copy", "file.copy", tg],
        ["Paste", "file.paste", tg],
        ["Copy Path", "file.copy_absolute", tg],
        ["Copy Relative Path", "file.copy_relative", tg],
        ["Rename...", "file.rename", tg],
        ["Delete Permanently", "file.delete", tg],
      ];
    }
    case "pane": {
      // A kept sub-agent can go back under its parent, still running.
      const all = terminals();
      const term = all.find((t) => t.address === tg.terminal);
      const parent = term?.parent && all.find((t) => t.address === term.parent);
      // Once they go to the program, Shift+right-click still opens this menu.
      const toProgram = term?.labels?.["herdr.right-click"] === "program";
      return [
        ["Split right", "pane.split", { direction: "right", pane: tg.pane }],
        ["Split down", "pane.split", { direction: "down", pane: tg.pane }],
        ["Zoom", "pane.zoom", { pane: tg.pane }],
        ...(term
          ? [
              toProgram
                ? ["Use Herdr right-click menu", "terminal.right_click", { terminal: term.address, to: "menu" }]
                : ["Send right-clicks to pane", "terminal.right_click", { terminal: term.address, to: "program" }],
            ]
          : []),
        ...(parent ? [[`Send back to ${parent.name || parent.address}`, "terminal.park", { terminal: term.address }]] : []),
        ["Close pane", "pane.close", { pane: tg.pane }],
      ];
    }
    case "tab":
      return [
        ["Rename", "tab.rename", { id: tg.id }],
        ["New tab", "tab.new", { space: tg.space }],
        ["Close", "tab.close", { id: tg.id }],
      ];
    case "space":
      return [
        ["Rename", "space.rename", { id: tg.id }],
        ...(git ? [["New worktree", "worktree", { space: tg.id, action: "new" }], ["Open worktree", "worktree", { space: tg.id, action: "open" }]] : []),
        ...(git === "linked" ? [["Remove worktree", "worktree", { space: tg.id, action: "remove" }]] : []),
        ["Close", "space.close", { id: tg.id }],
      ];
    default:
      return [
        ["New space", "space.new", {}],
        ["Keybinds", "app.help", {}],
        ["Theme: graphite", "app.theme", { name: "graphite" }],
        ["Theme: ink", "app.theme", { name: "ink" }],
        ["Theme: moss", "app.theme", { name: "moss" }],
        ["Theme: atom one dark", "app.theme", { name: "atom-one-dark" }],
      ];
  }
}

function menuIcon(command) {
  if (command === "file.new_file") return "mods/workspace-chrome/plus.svg";
  if (command === "file.new_folder") return "mods/workspace-chrome/folder-plus.svg";
  if (command === "file.open_terminal") return "mods/workspace-chrome/terminal.svg";
  if (command === "file.find_in_folder") return "mods/workspace-chrome/search.svg";
  if (command === "file.cut") return "mods/workspace-chrome/square.svg";
  if (command === "file.copy") return "mods/workspace-chrome/files.svg";
  if (command === "file.paste") return "mods/workspace-chrome/file-text.svg";
  if (command === "file.copy_absolute" || command === "file.copy_relative") return "mods/workspace-chrome/files.svg";
  if (command === "file.rename") return "mods/workspace-chrome/square-pen.svg";
  if (command === "file.delete") return "mods/workspace-chrome/x.svg";

  const name = command.endsWith(".close") ? "x" : command.endsWith(".rename") ? "square-pen" : command.endsWith(".new") ? "plus" : command === "worktree" ? "git-branch" : command === "app.help" ? "terminal" : command === "app.theme" ? "settings" : command === "pane.split" ? "panel-right" : command === "pane.zoom" ? "square" : "terminal";
  return `mods/workspace-chrome/${name}.svg`;
}

// The key bound to a command, for the hint on the right.
function shortcut(name, args) {
  if (name === "file.find_in_folder") return "Shift+Alt+F";
  if (name === "file.cut") return "Ctrl+X";
  if (name === "file.copy") return "Ctrl+C";
  if (name === "file.paste") return "Ctrl+V";
  if (name === "file.copy_absolute") return "Shift+Alt+C";
  if (name === "file.copy_relative") return "Ctrl+K Ctrl+Shift+C";
  if (name === "file.rename") return "F2";
  if (name === "file.delete") return "Del";

  const value = args && Object.values(args)[0];
  const k = keys().find((b) => b.section === "global" && (b.action === name || b.action === `${name}:${value}`));
  return k ? k.keys : "";
}

function isDivider(list, i) {
  const it = list[i];
  const prev = list[i - 1];
  if (!it || !prev) return false;
  const cmd = it[1];
  const prevCmd = prev[1];
  if (cmd === "file.find_in_folder") return true;
  if (cmd === "file.cut") return true;
  if (cmd === "file.copy_absolute") return true;
  if (cmd === "file.rename") return true;
  if (cmd.endsWith(".close") || cmd === "app.help" || (cmd === "app.theme" && prevCmd !== "app.theme")) return true;
  return false;
}

export default class Menu extends View {
  init(_props, cx) {
    this.at = 0;
    this.git = null;
    this.handle = cx.focus_handle();
    this.handle.focus();
    const p = props();
    const space = p?.tab === "space" && spaces().find((s) => s.id === p.target?.id);
    const folder = space && place(space);
    if (folder)
      cx.spawn(async (cx) => {
        this.git = await checkout(folder, space.machine).catch(() => null);
        cx.notify();
      });
  }

  choose(i, cx) {
    const it = items(props(), this.git)[i];
    if (!it) return;
    closeOverlay(props()?.overlay);
    const [_, cmd, tg] = it;

    if (cmd === "worktree") {
      openOverlay({ mod: "worktree", kind: "dialog", ...(tg.action === "open" ? { width: 560, height: 480 } : { width: 440, height: 260 }), props: tg });
    } else if (cmd === "file.copy_relative") {
      cx.write_to_clipboard(tg.path || "");
      notify({ title: "Copied relative path", body: tg.path || "", level: "success", timeout: 1500 });
    } else if (cmd === "file.copy_absolute") {
      const full = tg.root ? (tg.path ? `${tg.root}/${tg.path}` : tg.root) : (tg.path || "");
      const formatted = (!tg.on && window()?.platform === "windows") ? full.replace(/\//g, "\\") : full;
      cx.write_to_clipboard(formatted);
      notify({ title: "Copied path", body: formatted, level: "success", timeout: 1500 });
    } else if (cmd === "file.open_terminal") {
      const full = tg.root ? (tg.path ? `${tg.root}/${tg.path}` : tg.root) : (tg.path || "");
      const folder = tg.dir ? full : (full.includes("/") ? full.substring(0, full.lastIndexOf("/")) : full);
      run("terminal.open", { folder });
    } else if (cmd === "file.find_in_folder") {
      const targetDir = tg.dir ? (tg.root ? (tg.path ? `${tg.root}/${tg.path}` : tg.root) : tg.path) : (tg.root ? (tg.path?.includes("/") ? `${tg.root}/${tg.path.substring(0, tg.path.lastIndexOf("/"))}` : tg.root) : (tg.path?.includes("/") ? tg.path.substring(0, tg.path.lastIndexOf("/")) : tg.root));
      openOverlay({
        mod: "prompt",
        kind: "dialog",
        width: 380,
        props: {
          title: "Find in Folder",
          hint: "Search query",
          action: "find_in_folder",
          target: { dir: targetDir, on: tg.on }
        }
      });
    } else if (cmd === "file.cut") {
      const full = tg.root ? (tg.path ? `${tg.root}/${tg.path}` : tg.root) : (tg.path || "");
      fileClipboard = { action: "cut", full, path: tg.path, name: tg.name, dir: tg.dir, on: tg.on };
      notify({ title: "Cut", body: tg.name || tg.path, level: "info", timeout: 1500 });
    } else if (cmd === "file.copy") {
      const full = tg.root ? (tg.path ? `${tg.root}/${tg.path}` : tg.root) : (tg.path || "");
      fileClipboard = { action: "copy", full, path: tg.path, name: tg.name, dir: tg.dir, on: tg.on };
      cx.write_to_clipboard(full);
      notify({ title: "Copied to clipboard", body: tg.name || tg.path, level: "info", timeout: 1500 });
    } else if (cmd === "file.paste") {
      if (!fileClipboard) {
        notify({ title: "Clipboard is empty", level: "warning", timeout: 1500 });
        return;
      }
      const destBase = tg.dir ? (tg.root ? (tg.path ? `${tg.root}/${tg.path}` : tg.root) : tg.path) : (tg.root ? (tg.path?.includes("/") ? `${tg.root}/${tg.path.substring(0, tg.path.lastIndexOf("/"))}` : tg.root) : (tg.path?.includes("/") ? tg.path.substring(0, tg.path.lastIndexOf("/")) : tg.root));
      const src = fileClipboard.full;
      const dest = `${destBase}/${fileClipboard.name}`;
      cx.spawn(async () => {
        try {
          if (!tg.on && window()?.platform === "windows") {
            const srcWin = src.replace(/\//g, "\\");
            const destWin = dest.replace(/\//g, "\\");
            if (fileClipboard.action === "cut") {
              await exec("powershell", ["-NoProfile", "-Command", `Move-Item -LiteralPath '${srcWin.replace(/'/g, "''")}' -Destination '${destWin.replace(/'/g, "''")}' -Force`]);
              fileClipboard = null;
            } else {
              await exec("powershell", ["-NoProfile", "-Command", `Copy-Item -LiteralPath '${srcWin.replace(/'/g, "''")}' -Destination '${destWin.replace(/'/g, "''")}' -Recurse -Force`]);
            }
          } else {
            if (fileClipboard.action === "cut") {
              await exec("mv", [src, dest], { on: tg.on });
              fileClipboard = null;
            } else {
              await exec("cp", ["-r", src, dest], { on: tg.on });
            }
          }
          notify({ title: "Pasted", body: dest, level: "success", timeout: 1500 });
        } catch (e) {
          notify({ title: "Paste failed", body: String(e), level: "error", timeout: 3000 });
        }
      });
    } else if (cmd === "file.new_file") {
      const baseDir = tg.dir ? (tg.root ? (tg.path ? `${tg.root}/${tg.path}` : tg.root) : tg.path) : (tg.root ? (tg.path?.includes("/") ? `${tg.root}/${tg.path.substring(0, tg.path.lastIndexOf("/"))}` : tg.root) : (tg.path?.includes("/") ? tg.path.substring(0, tg.path.lastIndexOf("/")) : ""));
      openOverlay({
        mod: "prompt",
        kind: "dialog",
        width: 380,
        props: {
          title: "New File",
          hint: "Enter file name",
          action: "create_file",
          target: { dir: baseDir, on: tg.on }
        }
      });
    } else if (cmd === "file.new_folder") {
      const baseDir = tg.dir ? (tg.root ? (tg.path ? `${tg.root}/${tg.path}` : tg.root) : tg.path) : (tg.root ? (tg.path?.includes("/") ? `${tg.root}/${tg.path.substring(0, tg.path.lastIndexOf("/"))}` : tg.root) : (tg.path?.includes("/") ? tg.path.substring(0, tg.path.lastIndexOf("/")) : ""));
      openOverlay({
        mod: "prompt",
        kind: "dialog",
        width: 380,
        props: {
          title: "New Folder",
          hint: "Enter folder name",
          action: "create_folder",
          target: { dir: baseDir, on: tg.on }
        }
      });
    } else if (cmd === "file.rename") {
      const full = tg.root ? (tg.path ? `${tg.root}/${tg.path}` : tg.root) : (tg.path || "");
      const parent = full.includes("/") ? full.substring(0, full.lastIndexOf("/")) : "";
      openOverlay({
        mod: "prompt",
        kind: "dialog",
        width: 380,
        props: {
          title: `Rename ${tg.dir ? "Folder" : "File"}`,
          value: tg.name || "",
          hint: "Enter new name",
          action: "rename",
          target: { oldPath: full, parent, on: tg.on }
        }
      });
    } else if (cmd === "file.delete") {
      const full = tg.root ? (tg.path ? `${tg.root}/${tg.path}` : tg.root) : (tg.path || "");
      cx.spawn(async () => {
        try {
          if (!tg.on && window()?.platform === "windows") {
            const winPath = full.replace(/\//g, "\\");
            await exec("powershell", ["-NoProfile", "-Command", `Remove-Item -LiteralPath '${winPath.replace(/'/g, "''")}' -Recurse -Force`]);
          } else {
            await exec("rm", ["-rf", full], { on: tg.on });
          }
          notify({ title: "Deleted permanently", body: tg.name || full, level: "info", timeout: 2000 });
        } catch (e) {
          notify({ title: "Delete failed", body: String(e), level: "error", timeout: 3000 });
        }
      });
    } else {
      run(cmd, tg);
    }
  }

  render(cx) {
    const t = theme();
    const list = items(props(), this.git);
    return v_flex()
      .id("menu")
      .track_focus(this.handle)
      .w_full()
      .min_h(0)
      .p(5)
      .bg(t.popover)
      .text_size(12.8)
      .on_key_down((e, cx) => {
        if (e.key === "down") this.at = (this.at + 1) % list.length;
        else if (e.key === "up") this.at = (this.at + list.length - 1) % list.length;
        else if (e.key === "enter") this.choose(this.at, cx);
        else if (e.key === "escape") closeOverlay(props().overlay);
        else return;
        cx.notify();
      })
      .children(
        list.map((it, i) =>
          v_flex()
            .when(i > 0 && isDivider(list, i), el => el.child(div().mx(7).my(4).h(1).bg(t.border)))
            .child(h_flex()
            .id(`mi-${i}`)
            .h(30)
            .px(8)
            .gap(9)
            .rounded(6)
            .items_center()
            .justify_between()
            .text_color(t.foreground)
            .when(i === this.at, (el) => el.bg(t.selected))
            .hover((s) => s.bg(t.hover))
            .on_click((_e, cx) => this.choose(i, cx))
            .child(svg(menuIcon(it[1])).size(14).flex_none().text_color(t.muted))
            .child(div().flex_1().child(it[0]))
            .child(div().text_size(11).text_color(t.dim).child(shortcut(it[1], it[2])))),
        ),
      );
  }
}
