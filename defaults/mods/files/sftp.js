import { div, svg } from "gpui-kit";
import { v_flex, h_flex, Input, InputState } from "gpui-base";
import {
  sftpResolveInitialPath,
  sftpListDir,
  sftpDownloadFile,
  sftpPickAndUploadFiles,
  sftpUploadFiles,
  sftpCreateDirectory,
  sftpRenameEntry,
  sftpDeleteEntry,
  notify,
} from "herdr";

export function normalizePosixPath(raw) {
  const slash = String(raw || "/").replace(/\\/g, "/");
  const parts = [];
  for (const seg of slash.split("/")) {
    if (!seg || seg === ".") continue;
    if (seg === "..") parts.pop();
    else parts.push(seg);
  }
  return parts.length ? "/" + parts.join("/") : "/";
}

export function joinPosixPath(dir, name) {
  if (!name || name === "." || name === ".." || name.includes("/") || name.includes("\\") || name.includes("\0")) {
    throw new Error(`Invalid remote entry name: ${name}`);
  }
  const base = normalizePosixPath(dir);
  return base === "/" ? `/${name}` : `${base}/${name}`;
}

export function parentPosixPath(path) {
  const clean = normalizePosixPath(path);
  if (clean === "/") return "/";
  const idx = clean.lastIndexOf("/");
  return idx <= 0 ? "/" : clean.slice(0, idx);
}

export function breadcrumbSegments(path) {
  const clean = normalizePosixPath(path);
  const segs = [{ label: "/", path: "/" }];
  if (clean === "/") return segs;
  let acc = "";
  for (const part of clean.slice(1).split("/")) {
    acc += `/${part}`;
    segs.push({ label: part, path: acc });
  }
  return segs;
}

export function sortSftpEntries(entries) {
  return [...entries].sort((a, b) => {
    const dirDiff = Number(Boolean(b.isDirectory)) - Number(Boolean(a.isDirectory));
    if (dirDiff !== 0) return dirDiff;
    return a.name.localeCompare(b.name, undefined, { sensitivity: "base" }) || a.name.localeCompare(b.name);
  });
}

export function filterSftpEntries(entries, showHidden) {
  const visible = showHidden ? entries : entries.filter(e => !e.name.startsWith("."));
  return sortSftpEntries(visible);
}

export function formatBytes(bytes) {
  if (bytes === null || bytes === undefined) return "";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let idx = 0;
  while (value >= 1024 && idx < units.length - 1) {
    value /= 1024;
    idx++;
  }
  return `${value >= 10 ? value.toFixed(0) : value.toFixed(1)} ${units[idx]}`;
}

export function formatSftpError(err) {
  const msg = String(err?.message ?? err ?? "Unknown SFTP error");
  if (msg.includes("EACCES") || /permission denied/i.test(msg)) {
    return `Permission denied (EACCES): ${msg.replace(/^EACCES:\s*/i, "")}`;
  }
  if (msg.includes("ENOENT") || /no such file/i.test(msg)) {
    return `Path not found: ${msg.replace(/^ENOENT:\s*/i, "")}`;
  }
  return msg;
}

export class SftpBrowser {
  constructor() {
    this.machineId = null;
    this.workspacePath = null;
    this.path = "/";
    this.entries = [];
    this.loading = false;
    this.busyText = null;
    this.error = null;
    this.notice = null;
    this.showHidden = false;
    this.selectedPath = null;
    this.menuEntry = null;
    this.prompt = null; // { kind: "mkdir" | "rename", entry?, input, value, focused }
    this.confirmDelete = null; // SftpEntry
    this.requestSeq = 0;
  }

  syncMachine(machineId, workspacePath, activeMode, cx) {
    const changed = machineId !== this.machineId;
    this.workspacePath = workspacePath || null;
    if (!machineId) {
      this.machineId = null;
      this.path = "/";
      this.entries = [];
      this.error = null;
      this.notice = null;
      this.menuEntry = null;
      this.prompt = null;
      this.confirmDelete = null;
      return;
    }
    if (changed) {
      this.machineId = machineId;
      this.path = "/";
      this.entries = [];
      this.error = null;
      this.notice = null;
      this.selectedPath = null;
      this.menuEntry = null;
      this.prompt = null;
      this.confirmDelete = null;
      if (activeMode) this.openInitial(cx);
    }
  }

  openInitial(cx) {
    if (!this.machineId) return;
    const machineId = this.machineId;
    const seq = ++this.requestSeq;
    this.loading = true;
    this.error = null;
    this.menuEntry = null;
    cx?.notify();
    cx.spawn(async cx => {
      try {
        const initial = await sftpResolveInitialPath(machineId, this.workspacePath);
        if (this.machineId !== machineId || this.requestSeq !== seq) return;
        const clean = normalizePosixPath(initial || "/");
        const entries = await sftpListDir(machineId, clean);
        if (this.machineId !== machineId || this.requestSeq !== seq) return;
        this.path = clean;
        this.entries = sortSftpEntries(entries);
        this.error = null;
      } catch (e) {
        if (this.machineId === machineId && this.requestSeq === seq) this.error = formatSftpError(e);
      } finally {
        if (this.machineId === machineId && this.requestSeq === seq) this.loading = false;
        cx.notify();
      }
    });
  }

  navigate(targetPath, cx) {
    if (!this.machineId) return;
    const machineId = this.machineId;
    const clean = normalizePosixPath(targetPath);
    const seq = ++this.requestSeq;
    this.loading = true;
    this.error = null;
    this.menuEntry = null;
    this.confirmDelete = null;
    cx.notify();
    cx.spawn(async cx => {
      try {
        const entries = await sftpListDir(machineId, clean);
        if (this.machineId !== machineId || this.requestSeq !== seq) return;
        this.path = clean;
        this.entries = sortSftpEntries(entries);
        this.selectedPath = null;
        this.error = null;
      } catch (e) {
        if (this.machineId === machineId && this.requestSeq === seq) this.error = formatSftpError(e);
      } finally {
        if (this.machineId === machineId && this.requestSeq === seq) this.loading = false;
        cx.notify();
      }
    });
  }

  refresh(cx) {
    if (!this.machineId) return;
    if (!this.entries.length && this.path === "/") this.openInitial(cx);
    else this.navigate(this.path, cx);
  }

  goUp(cx) {
    if (this.path === "/") return;
    this.navigate(parentPosixPath(this.path), cx);
  }

  toggleHidden(cx) {
    this.showHidden = !this.showHidden;
    this.menuEntry = null;
    cx.notify();
  }

  flashNotice(text, cx) {
    this.notice = text;
    cx.notify();
    cx.timer.after(3500, cx => {
      if (this.notice === text) {
        this.notice = null;
        cx.notify();
      }
    });
  }

  download(entry, cx) {
    if (!this.machineId || entry.isDirectory) return;
    const machineId = this.machineId;
    const remotePath = normalizePosixPath(entry.path);
    this.menuEntry = null;
    this.error = null;
    this.busyText = `Downloading ${entry.name}…`;
    cx.notify();
    cx.spawn(async cx => {
      try {
        const res = await sftpDownloadFile(machineId, remotePath);
        if (!res.canceled) {
          const msg = `Downloaded ${entry.name} (${formatBytes(res.bytes ?? 0)})`;
          this.flashNotice(msg, cx);
          notify({ title: "SFTP Download", body: `${entry.name} saved to ${res.localPath}`, level: "success" });
        }
      } catch (e) {
        this.error = formatSftpError(e);
        notify({ title: "SFTP Download Failed", body: this.error, level: "error" });
      } finally {
        this.busyText = null;
        cx.notify();
      }
    });
  }

  pickAndUpload(cx) {
    if (!this.machineId) return;
    const machineId = this.machineId;
    const targetDir = this.path;
    this.menuEntry = null;
    this.error = null;
    this.busyText = "Preparing upload…";
    cx.notify();
    cx.spawn(async cx => {
      try {
        const res = await sftpPickAndUploadFiles(machineId, targetDir);
        if (!res.canceled) {
          const count = res.uploaded?.length ?? 0;
          this.flashNotice(`Uploaded ${count} file${count === 1 ? "" : "s"}`, cx);
          notify({ title: "SFTP Upload", body: `Uploaded ${count} file${count === 1 ? "" : "s"} to ${targetDir}`, level: "success" });
          await this.reloadQuiet(machineId, targetDir);
        }
      } catch (e) {
        this.error = formatSftpError(e);
        notify({ title: "SFTP Upload Failed", body: this.error, level: "error" });
      } finally {
        this.busyText = null;
        cx.notify();
      }
    });
  }

  uploadDroppedPaths(localPaths, cx) {
    if (!this.machineId || !localPaths?.length) return;
    const machineId = this.machineId;
    const targetDir = this.path;
    this.menuEntry = null;
    this.error = null;
    this.busyText = `Uploading ${localPaths.length} dropped item${localPaths.length === 1 ? "" : "s"}…`;
    cx.notify();
    cx.spawn(async cx => {
      try {
        const res = await sftpUploadFiles(machineId, localPaths, targetDir);
        const count = res.uploaded?.length ?? 0;
        this.flashNotice(`Uploaded ${count} file${count === 1 ? "" : "s"}`, cx);
        notify({ title: "SFTP Upload", body: `Uploaded ${count} file${count === 1 ? "" : "s"} to ${targetDir}`, level: "success" });
        await this.reloadQuiet(machineId, targetDir);
      } catch (e) {
        this.error = formatSftpError(e);
        notify({ title: "SFTP Upload Failed", body: this.error, level: "error" });
      } finally {
        this.busyText = null;
        cx.notify();
      }
    });
  }

  async reloadQuiet(machineId, dir) {
    if (this.machineId !== machineId || this.path !== dir) return;
    const entries = await sftpListDir(machineId, dir);
    if (this.machineId === machineId && this.path === dir) {
      this.entries = sortSftpEntries(entries);
    }
  }

  openPrompt(kind, entry, cx) {
    this.menuEntry = null;
    this.confirmDelete = null;
    const initial = kind === "rename" && entry ? entry.name : "";
    const input = InputState.new({ value: initial });
    if (initial) input.set_selected_range({ start: 0, end: initial.length });
    const prompt = { kind, entry, input, value: initial, focused: true };
    input.on("change", (_e, cx) => { prompt.value = input.value(); cx.notify(); });
    input.on("submit", (_e, cx) => this.submitPrompt(cx));
    input.on("focus", (_e, cx) => { prompt.focused = true; cx.notify(); });
    input.on("blur", (_e, cx) => { prompt.focused = false; cx.notify(); });
    input.focus();
    this.prompt = prompt;
    cx.notify();
  }

  cancelPrompt(cx) {
    this.prompt = null;
    cx.notify();
  }

  submitPrompt(cx) {
    if (!this.prompt || !this.machineId) return;
    const name = this.prompt.value.trim();
    if (!name) return;
    const { kind, entry } = this.prompt;
    const machineId = this.machineId;
    const dir = this.path;
    let targetPath;
    try {
      targetPath = joinPosixPath(dir, name);
    } catch (e) {
      this.error = formatSftpError(e);
      cx.notify();
      return;
    }
    this.prompt = null;
    this.error = null;
    this.busyText = kind === "mkdir" ? `Creating ${name}…` : `Renaming to ${name}…`;
    cx.notify();
    cx.spawn(async cx => {
      try {
        if (kind === "mkdir") {
          await sftpCreateDirectory(machineId, targetPath);
          this.flashNotice(`Created folder ${name}`, cx);
        } else if (kind === "rename" && entry) {
          const oldPath = normalizePosixPath(entry.path);
          if (oldPath !== targetPath) {
            await sftpRenameEntry(machineId, oldPath, targetPath);
            this.flashNotice(`Renamed to ${name}`, cx);
          }
        }
        await this.reloadQuiet(machineId, dir);
      } catch (e) {
        this.error = formatSftpError(e);
      } finally {
        this.busyText = null;
        cx.notify();
      }
    });
  }

  requestDelete(entry, cx) {
    this.menuEntry = null;
    this.prompt = null;
    this.confirmDelete = entry;
    cx.notify();
  }

  performDelete(cx) {
    if (!this.confirmDelete || !this.machineId) return;
    const entry = this.confirmDelete;
    const machineId = this.machineId;
    const dir = this.path;
    this.confirmDelete = null;
    this.error = null;
    this.busyText = `Deleting ${entry.name}…`;
    cx.notify();
    cx.spawn(async cx => {
      try {
        await sftpDeleteEntry(machineId, normalizePosixPath(entry.path), Boolean(entry.isDirectory && !entry.isSymlink));
        this.flashNotice(`Deleted ${entry.name}`, cx);
        await this.reloadQuiet(machineId, dir);
      } catch (e) {
        this.error = formatSftpError(e);
      } finally {
        this.busyText = null;
        cx.notify();
      }
    });
  }

  copyRemotePath(entry, cx) {
    this.menuEntry = null;
    const path = normalizePosixPath(entry.path);
    cx.write_to_clipboard(path);
    this.flashNotice(`Copied ${path}`, cx);
  }

  renderToolbar(t, cx) {
    const iconBtn = (id, icon, tip, disabled, action) =>
      h_flex()
        .id(id)
        .w(26)
        .h(26)
        .items_center()
        .justify_center()
        .rounded(6)
        .when(!disabled, el => el.cursor_pointer().hover(s => s.bg(t.hover)).on_click((_e, cx) => action(cx)))
        .when(disabled, el => el.opacity(0.35))
        .tooltip(tip)
        .child(svg(`mods/files/${icon}.svg`).size(13).text_color(t.muted));

    return h_flex()
      .h(34)
      .flex_none()
      .px(8)
      .gap(4)
      .items_center()
      .border_b(1)
      .border_color(t.border)
      .bg(t.surface)
      .child(iconBtn("sftp-up", "arrow-up", "Up (..)", this.path === "/" || this.loading, cx => this.goUp(cx)))
      .child(iconBtn("sftp-refresh", "refresh", "Refresh", this.loading, cx => this.refresh(cx)))
      .child(
        h_flex()
          .id("sftp-dotfiles")
          .h(24)
          .px(7)
          .gap(4)
          .items_center()
          .rounded(6)
          .cursor_pointer()
          .border_1()
          .border_color(this.showHidden ? t.accent : t.border)
          .when(this.showHidden, el => el.bg(t.selected))
          .hover(s => s.bg(t.hover))
          .tooltip(this.showHidden ? "Hide dotfiles" : "Show dotfiles")
          .on_click((_e, cx) => this.toggleHidden(cx))
          .child(svg(`mods/files/${this.showHidden ? "eye" : "eye-off"}.svg`).size(12).text_color(this.showHidden ? t.foreground : t.dim))
          .child(div().text_size(11).text_color(this.showHidden ? t.foreground : t.dim).child(".*"))
      )
      .child(div().flex_1())
      .child(
        h_flex()
          .id("sftp-upload")
          .h(24)
          .px(8)
          .gap(5)
          .items_center()
          .rounded(6)
          .cursor_pointer()
          .border_1()
          .border_color(t.border)
          .hover(s => s.bg(t.hover))
          .tooltip("Upload file(s)")
          .on_click((_e, cx) => this.pickAndUpload(cx))
          .child(svg("mods/files/upload.svg").size(12).text_color(t.muted))
          .child(div().text_size(11).text_color(t.foreground).child("Upload"))
      )
      .child(
        h_flex()
          .id("sftp-mkdir")
          .h(24)
          .px(8)
          .gap(5)
          .items_center()
          .rounded(6)
          .cursor_pointer()
          .border_1()
          .border_color(t.border)
          .hover(s => s.bg(t.hover))
          .tooltip("New folder")
          .on_click((_e, cx) => this.openPrompt("mkdir", null, cx))
          .child(svg("mods/files/folder-plus.svg").size(12).text_color(t.muted))
          .child(div().text_size(11).text_color(t.foreground).child("New"))
      );
  }

  renderBreadcrumbs(t) {
    const segs = breadcrumbSegments(this.path);
    return h_flex()
      .h(28)
      .flex_none()
      .px(10)
      .gap(2)
      .items_center()
      .overflow_x_scroll()
      .border_b(1)
      .border_color(t.border)
      .children(
        segs.flatMap((seg, i) => {
          const last = i === segs.length - 1;
          const items = [
            div()
              .id(`sftp-bc-${i}`)
              .px(5)
              .py(2)
              .rounded(4)
              .text_size(11.5)
              .cursor_pointer()
              .text_color(last ? t.foreground : t.dim)
              .when(last, el => el.font_medium())
              .hover(s => s.bg(t.hover).text_color(t.foreground))
              .on_click((_e, cx) => this.navigate(seg.path, cx))
              .child(seg.label),
          ];
          if (i > 0 && !last) {
            items.push(div().text_size(11).text_color(t.dim).child("/"));
          }
          return items;
        })
      );
  }

  renderBanners(t, cx) {
    const out = [];
    if (this.error) {
      out.push(
        h_flex()
          .id("sftp-error-banner")
          .flex_none()
          .px(10)
          .py(7)
          .gap(8)
          .items_center()
          .border_b(1)
          .border_color(t.ansi1)
          .bg(t.surface)
          .child(div().flex_1().min_w(0).text_size(11.5).text_color(t.ansi1).child(this.error))
          .child(
            div()
              .id("sftp-error-retry")
              .px(6)
              .py(2)
              .rounded(4)
              .cursor_pointer()
              .text_size(11)
              .text_color(t.foreground)
              .bg(t.raised)
              .hover(s => s.bg(t.hover))
              .on_click((_e, cx) => this.refresh(cx))
              .child("Retry")
          )
          .child(
            div()
              .id("sftp-error-dismiss")
              .px(5)
              .py(2)
              .rounded(4)
              .cursor_pointer()
              .text_size(12)
              .text_color(t.dim)
              .hover(s => s.bg(t.hover))
              .on_click((_e, cx) => { this.error = null; cx.notify(); })
              .child("×")
          )
      );
    }
    if (this.busyText || this.notice) {
      out.push(
        h_flex()
          .flex_none()
          .px(10)
          .py(5)
          .gap(8)
          .items_center()
          .border_b(1)
          .border_color(t.border)
          .bg(t.surface)
          .child(div().flex_1().min_w(0).truncate().text_size(11.5).text_color(this.busyText ? t.accent : t.ansi2).child(this.busyText || this.notice))
      );
    }
    if (this.prompt) {
      const label = this.prompt.kind === "mkdir" ? "New folder name" : `Rename ${this.prompt.entry?.name ?? ""}`;
      out.push(
        v_flex()
          .flex_none()
          .p(8)
          .gap(6)
          .border_b(1)
          .border_color(t.border)
          .bg(t.surface)
          .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
          .child(div().text_size(11).text_color(t.dim).child(label))
          .child(
            h_flex()
              .gap(6)
              .items_center()
              .child(
                div()
                  .flex_1()
                  .min_w(0)
                  .h(28)
                  .px(8)
                  .flex()
                  .items_center()
                  .rounded(6)
                  .border_1()
                  .border_color(this.prompt.focused ? t.accent : t.input)
                  .bg(t.background)
                  .child(Input.new(this.prompt.input).h_full().text_size(12).text_color(t.foreground))
              )
              .child(
                div()
                  .id("sftp-prompt-ok")
                  .px(9)
                  .h(28)
                  .flex()
                  .items_center()
                  .rounded(6)
                  .cursor_pointer()
                  .bg(t.solid)
                  .text_color(t.solid_foreground)
                  .text_size(11.5)
                  .font_medium()
                  .on_click((_e, cx) => this.submitPrompt(cx))
                  .child("Save")
              )
              .child(
                div()
                  .id("sftp-prompt-cancel")
                  .px(8)
                  .h(28)
                  .flex()
                  .items_center()
                  .rounded(6)
                  .cursor_pointer()
                  .bg(t.raised)
                  .text_color(t.foreground)
                  .text_size(11.5)
                  .on_click((_e, cx) => this.cancelPrompt(cx))
                  .child("Cancel")
              )
          )
      );
    }
    if (this.confirmDelete) {
      const target = this.confirmDelete;
      out.push(
        h_flex()
          .flex_none()
          .px(10)
          .py(7)
          .gap(8)
          .items_center()
          .border_b(1)
          .border_color(t.ansi1)
          .bg(t.surface)
          .child(
            div()
              .flex_1()
              .min_w(0)
              .truncate()
              .text_size(11.5)
              .text_color(t.foreground)
              .child(`Delete ${target.isDirectory ? "folder" : "file"} "${target.name}"?`)
          )
          .child(
            div()
              .id("sftp-delete-confirm")
              .px(8)
              .py(3)
              .rounded(5)
              .cursor_pointer()
              .bg(t.ansi1)
              .text_color(t.solid_foreground)
              .text_size(11)
              .font_medium()
              .on_click((_e, cx) => this.performDelete(cx))
              .child("Delete")
          )
          .child(
            div()
              .id("sftp-delete-cancel")
              .px(8)
              .py(3)
              .rounded(5)
              .cursor_pointer()
              .bg(t.raised)
              .text_color(t.foreground)
              .text_size(11)
              .on_click((_e, cx) => { this.confirmDelete = null; cx.notify(); })
              .child("Cancel")
          )
      );
    }
    return out;
  }

  renderContextMenu(entry, t) {
    const item = (id, icon, label, danger, action) =>
      h_flex()
        .id(id)
        .h(26)
        .px(8)
        .gap(7)
        .items_center()
        .rounded(5)
        .cursor_pointer()
        .hover(s => s.bg(t.hover))
        .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
        .on_click((_e, cx) => { cx.stop_propagation(); action(cx); })
        .child(svg(`mods/files/${icon}.svg`).size(12).text_color(danger ? t.ansi1 : t.muted))
        .child(div().text_size(12).text_color(danger ? t.ansi1 : t.foreground).child(label));

    return v_flex()
      .id(`sftp-menu-${entry.path}`)
      .mx(6)
      .mb(4)
      .p(4)
      .gap(2)
      .rounded(7)
      .border_1()
      .border_color(t.border)
      .bg(t.popover)
      .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
      .when(!entry.isDirectory, el => el.child(item(`sftp-act-dl-${entry.path}`, "download", "Download", false, cx => this.download(entry, cx))))
      .child(item(`sftp-act-rn-${entry.path}`, "pencil", "Rename", false, cx => this.openPrompt("rename", entry, cx)))
      .child(item(`sftp-act-cp-${entry.path}`, "copy", "Copy Remote Path", false, cx => this.copyRemotePath(entry, cx)))
      .child(item(`sftp-act-rm-${entry.path}`, "trash", "Delete", true, cx => this.requestDelete(entry, cx)));
  }

  renderRow(entry, t) {
    const selected = this.selectedPath === entry.path;
    const menuOpen = this.menuEntry?.path === entry.path;
    const rowIcon = (id, icon, tip, danger, action) =>
      h_flex()
        .id(id)
        .w(22)
        .h(22)
        .items_center()
        .justify_center()
        .rounded(5)
        .cursor_pointer()
        .hover(s => s.bg(t.raised))
        .tooltip(tip)
        .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
        .on_click((_e, cx) => { cx.stop_propagation(); action(cx); })
        .child(svg(`mods/files/${icon}.svg`).size(12).text_color(danger ? t.ansi1 : t.dim));

    return v_flex()
      .key(entry.path)
      .child(
        h_flex()
          .id(`sftp-row-${entry.path}`)
          .h(28)
          .flex_none()
          .px(8)
          .gap(7)
          .items_center()
          .rounded(7)
          .cursor_pointer()
          .when(selected, el => el.bg(t.selected))
          .hover(s => s.bg(t.hover))
          .on_mouse_down("right", (_e, cx) => {
            cx.stop_propagation();
            this.selectedPath = entry.path;
            this.menuEntry = menuOpen ? null : entry;
            cx.notify();
          })
          .on_click((_e, cx) => {
            if (entry.isDirectory) {
              this.navigate(entry.path, cx);
            } else {
              this.selectedPath = entry.path;
              this.menuEntry = null;
              cx.notify();
            }
          })
          .child(svg(`mods/files/${entry.isDirectory ? "folder" : "file"}.svg`).size(14).flex_none().text_color(entry.isDirectory ? t.accent : t.muted))
          .child(
            div()
              .flex_1()
              .min_w(0)
              .truncate()
              .text_color(t.foreground)
              .child(entry.isSymlink ? `${entry.name} ↗` : entry.name)
          )
          .when(!entry.isDirectory, el =>
            el.child(div().flex_none().text_size(11).text_color(t.dim).child(formatBytes(entry.size)))
          )
          .when(selected, el =>
            el.child(
              h_flex()
                .flex_none()
                .gap(2)
                .items_center()
                .when(!entry.isDirectory, el => el.child(rowIcon(`sftp-dl-${entry.path}`, "download", "Download", false, cx => this.download(entry, cx))))
                .child(rowIcon(`sftp-rn-${entry.path}`, "pencil", "Rename", false, cx => this.openPrompt("rename", entry, cx)))
                .child(rowIcon(`sftp-cp-${entry.path}`, "copy", "Copy Remote Path", false, cx => this.copyRemotePath(entry, cx)))
                .child(rowIcon(`sftp-rm-${entry.path}`, "trash", "Delete", true, cx => this.requestDelete(entry, cx)))
            )
          )
          .child(
            div()
              .id(`sftp-more-${entry.path}`)
              .px(4)
              .py(2)
              .rounded(4)
              .text_size(12)
              .text_color(t.dim)
              .cursor_pointer()
              .hover(s => s.bg(t.raised).text_color(t.foreground))
              .on_mouse_down("left", (_e, cx) => cx.stop_propagation())
              .on_click((_e, cx) => {
                cx.stop_propagation();
                this.selectedPath = entry.path;
                this.menuEntry = menuOpen ? null : entry;
                cx.notify();
              })
              .child("⋯")
          )
      )
      .when(menuOpen, el => el.child(this.renderContextMenu(entry, t)));
  }

  render(t, cx) {
    const visible = filterSftpEntries(this.entries, this.showHidden);
    return v_flex()
      .id("sftp-browser")
      .flex_1()
      .min_h(0)
      .child(this.renderToolbar(t, cx))
      .child(this.renderBreadcrumbs(t))
      .children(this.renderBanners(t, cx))
      .child(
        v_flex()
          .id("sftp-drop-zone")
          .flex_1()
          .min_h(0)
          .overflow_y_scrollbar()
          .p(6)
          .rounded(8)
          .border_1()
          .border_color(t.background)
          .drag_over(s => s.bg(t.hover).border_color(t.accent))
          .on_drop((event, cx) => {
            cx.stop_propagation();
            this.uploadDroppedPaths(event.paths ?? [], cx);
          })
          .on_click((_e, cx) => {
            if (this.menuEntry) {
              this.menuEntry = null;
              cx.notify();
            }
          })
          .children(visible.map(entry => this.renderRow(entry, t)))
          .when(!visible.length, el =>
            el.child(
              div()
                .p(12)
                .text_size(12)
                .text_color(t.dim)
                .child(this.loading ? "Connecting via SFTP…" : "Empty directory · Drop files here to upload")
            )
          )
      );
  }
}
