// Workspace spaces: stable colored avatars, worktrees under their checkout,
// grouped by machine when more than one is known. After a space's name, the repo
// its first terminal is in now, only where unusual: branch, ahead/behind. Agent state lives in the agents list, not here. Click to
// switch, right-click for its menu, drag to reorder.
// Folders (shared "space.folders", set by the space-folder dialog) gather their
// spaces under a header where the first of them sits; click it to fold. Drop a
// space on a header to put it in that folder, or on a space to join that space's
// folder (or leave its own, if that space has none). Folders nest the same way:
// drag a header onto another header, or onto a space to sit in that space's folder.
// Dropped on its own parent's header a folder goes up a level; dropped on the
// "Spaces" title, an empty part of the list or the drop zone shown while dragging,
// a folder or space goes to the top.

import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex } from "gpui-base";
import { spaces, machines, panes, run, openOverlay, exec, hc, theme, shared, setShared, watchFolder, window, layout } from "herdr";

function find(node, id) {
  if (node?.id === id) return node;
  for (const child of node?.children ?? []) {
    const found = find(child, id);
    if (found) return found;
  }
  return null;
}

// The sidebar's rail goes by its width, not a flag: dragging its edge narrows it too.
const narrow = () => (find(layout(), "sidebar")?.size?.px ?? Infinity) < 160;

// Where a space is now: its first terminal's live folder, else where it was made.
// `ps`: panes(), read once by the caller (each read copies every pane).
function place(s, ps) {
  const tabs = new Set((s.tabs ?? []).map((c) => c.id));
  return ps.find((p) => tabs.has(p.tab) && p.cwd)?.cwd || s.folder;
}
const rowHeight = ({ tree }) => (tree ? 28 : 30);
const where = (s, ps) => JSON.stringify([s.machine ?? null, place(s, ps)]);
const base = (path) => path.replace(/\/+$/, "").split("/").pop();
// Changes made outside Herdr (an editor's save, a commit in another app): looked
// for this often, in repos the list shows, while the window is in front.
const FALLBACK = 60000;

// The most urgent first, as a space's own status goes.
const URGENCY = ["blocked", "done", "working", "idle"];

// Puts space `id` in the folder named `name` ("" or null: in none). The same
// in the space-folder dialog.
function moveToFolder(id, name) {
  const list = (shared("space.folders") ?? []).map((f) => ({ ...f, ids: f.ids.filter((x) => x !== id) }));
  name = (name ?? "").trim();
  if (name) {
    const into = list.find((f) => f.name.toLowerCase() === name.toLowerCase());
    if (into) into.ids.push(id);
    else list.push({ name, collapsed: false, ids: [id], parent: null });
  }
  setShared("space.folders", prune(list));
}
const folderOf = (id) => (shared("space.folders") ?? []).find((f) => f.ids.includes(id))?.name ?? null;

// The folders holding `name`, innermost first: [name, its parent, ...].
function lineage(list, name) {
  const path = [];
  for (let at = name; at && !path.includes(at); at = list.find((f) => f.name === at)?.parent ?? null) path.push(at);
  return path;
}

// Puts folder `name` inside folder `parent` (null: at the top), never inside itself.
function nestFolder(name, parent) {
  const list = shared("space.folders") ?? [];
  if (name === parent || (parent && lineage(list, parent).includes(name))) return;
  setShared("space.folders", prune(list.map((f) => (f.name === name ? { ...f, parent } : f))));
}

// A folder stays while it holds a space or a folder that stays. The same in the
// space-folder dialog.
function prune(list) {
  for (;;) {
    const kept = list.filter((f) => f.ids.length || list.some((c) => c.parent === f.name));
    if (kept.length === list.length) return kept.map((f) => (f.parent && !kept.some((p) => p.name === f.parent) ? { ...f, parent: null } : f));
    list = kept;
  }
}

// What a stored color name looks like on an avatar (the same in every mod
// that draws space avatars): one strong color per hue, light and dark alike.
const MARK = { ansi6: "#22a99a", ansi3: "#e3814f", ansi4: "#5b8def", ansi5: "#8b7cf6", ansi2: "#3fae6a", ansi1: "#d9649a" };

// A space speaks up only when something in it needs you.
export function glyph(status) {
  return status === "blocked" || status === "done" ? "●" : "·";
}

export function tone(t, status) {
  return t[status] ?? t.unknown;
}

export default class Spaces extends View {
  init(_props, cx) {
    this.git = {}; // where() -> { name, branch, ahead, behind, repo, linked }, null: not a repo
    this.checked = {}; // where() -> when it was last looked at
    this.unsure = new Set(); // where()s git couldn't answer for (a machine away)
    this.pending = new Set(); // where()s to look at next
    this.busy = {}; // pane -> an agent working or a program in front of its shell
    this.ready = {}; // machine -> its layout was readable at the last look
    this.at = {}; // space -> its where() at the last look
    this.watching = new Set(); // local repos' git folders
    // Spaces made while this runs wait (at most a moment) for their git, so a
    // worktree appears under its repo instead of jumping there.
    this.shown = new Set(spaces().map((s) => s.id));
    this.born = {};
    this.pressed = null;
    this.over = null;
    this.pressedFolder = null; // a folder header being dragged
    this.start = null; // where the press began (window px)
    this.ghost = null; // where the dragged space's card follows the pointer (sidebar px)
    this.machines = [];
    this.folded = new Set(JSON.parse(localStorage.getItem("machines.folded") ?? "[]"));
    this.refreshMachines(cx);
    cx.timer.every(15000, cx => this.refreshMachines(cx));
    // `git status` walks the whole tree, on another machine a process there:
    // only when its answer could have changed (see `due`).
    cx.timer.every(250, (cx) => this.checkNew(cx));
  }

  checkNew(cx) {
    const now = Date.now();
    const sp = spaces();
    for (const s of sp) if (!this.shown.has(s.id)) this.born[s.id] ??= now;
    const due = this.due(now, sp, panes());
    if (due.size) this.refreshGit(due, cx);
    if (sp.some((s) => !this.shown.has(s.id))) cx.notify();
  }

  // Places whose git could have changed: new ones (a new space, a cd), where a
  // command or an agent's turn just ended (a checkout, a commit, edits), those on
  // a machine that is back, and repos in view whose fallback is due.
  due(now, sp, ps) {
    // Moved there: what it knew of that place may be old.
    const at = {};
    for (const s of sp) at[s.id] = where(s, ps);
    const keys = new Set(Object.values(at).filter((k) => JSON.parse(k)[1]));
    const due = new Set([...keys].filter((k) => !(k in this.git)));
    for (const [id, k] of Object.entries(at)) if (this.at[id] && this.at[id] !== k && keys.has(k)) due.add(k);
    this.at = at;
    const spaceOf = {};
    for (const s of sp) for (const t of s.tabs ?? []) spaceOf[t.id] = s;
    const busy = {};
    for (const p of ps) {
      if (!p.terminal || !spaceOf[p.tab]) continue;
      busy[p.pane] = p.status === "working" || !(p.program?.shell || p.program?.agent);
      if (this.busy[p.pane] && !busy[p.pane]) this.sameRepo(at[spaceOf[p.tab].id], keys).forEach((k) => due.add(k));
    }
    this.busy = busy;
    for (const m of machines()) {
      const ready = m.layout === "ready";
      if (ready && this.ready[m.name] === false) for (const k of keys) if (JSON.parse(k)[0] === m.name) due.add(k);
      this.ready[m.name] = ready;
    }
    if (window().active) {
      for (const k of keys) {
        const asks = this.git[k] || this.unsure.has(k);
        if (asks && !this.folded.has(JSON.parse(k)[0]) && now - (this.checked[k] ?? 0) > FALLBACK) due.add(k);
      }
    }
    return due;
  }

  // `key` and the other places in its repo (a commit there moves their ahead/behind).
  sameRepo(key, keys) {
    const repo = this.git[key]?.repo;
    return [key, ...(repo ? [...keys].filter((k) => this.git[k]?.repo === repo) : [])];
  }

  refreshMachines(cx) {
    cx.spawn(async cx => {
      try { const r = await hc(["machine", "ls", "--json"]); if (r.code === 0) this.machines = (JSON.parse(r.stdout).machines ?? []).filter((m) => !m.temporary); } catch {}
      cx.notify();
    });
  }

  // One look at a time, each place once however often it was asked for meanwhile.
  refreshGit(keys, cx) {
    for (const k of keys) this.pending.add(k);
    if (this.refreshing) return;
    this.refreshing = true;
    cx.spawn(async (cx) => {
      while (this.pending.size) {
        const keys = [...this.pending];
        this.pending.clear();
        for (const key of keys) await this.check(key, cx);
      }
      const used = new Set(Object.values(this.git).filter(Boolean).map((g) => JSON.parse(g.repo)).filter(([on]) => !on).map(([, repo]) => repo));
      for (const repo of this.watching) if (!used.has(repo)) this.watching.delete(repo);
      this.refreshing = false;
      cx.notify();
    });
  }

  // One place's git: branch, ahead/behind, its repo.
  async check(key, cx) {
    const [on, cwd] = JSON.parse(key);
    this.checked[key] = Date.now();
    this.unsure.delete(key);
    try {
      // No optional locks: a status that touched the index would wake the watch again.
      // Writes nothing under .git (index lock, fsmonitor cookie): watch() would wake on it.
      const r = await exec("git", ["--no-optional-locks", "-c", "core.fsmonitor=false", "status", "--porcelain=v2", "--branch"], { cwd, on });
      if (r.code !== 0) {
        // Only git's own "no" is remembered as no repo; anything else is asked again.
        if (!/not a git repository/i.test(r.stderr)) this.unsure.add(key);
        this.git[key] = null;
        return;
      }
      // The repo's own .git folder names it; a worktree's top differs from it.
      const top = await exec("git", ["rev-parse", "--show-toplevel"], { cwd, on });
      const common = await exec("git", ["rev-parse", "--path-format=absolute", "--git-common-dir"], { cwd, on });
      const repo = common.stdout.trim();
      const name = repo.endsWith("/.git") ? base(repo.slice(0, -5)) : base(repo).replace(/\.git$/, "");
      const info = { name, branch: null, ahead: 0, behind: 0, repo: JSON.stringify([on, repo]), linked: repo !== `${top.stdout.trim()}/.git` };
      for (const line of r.stdout.split("\n")) {
        if (line.startsWith("# branch.head ")) info.branch = line.slice(14);
        else if (line.startsWith("# branch.ab ")) {
          const [a, b] = line.slice(12).split(" ");
          info.ahead = Math.abs(parseInt(a, 10)) || 0;
          info.behind = Math.abs(parseInt(b, 10)) || 0;
        }
      }
      this.git[key] = info;
      if (!on) this.watch(repo, cx);
    } catch {
      // Couldn't ask (the machine is away): keep what it showed, look again later.
      this.git[key] ??= null;
      this.unsure.add(key);
    }
  }

  watch(repo, cx) {
    if (this.watching.has(repo)) return;
    this.watching.add(repo);
    cx.spawn(async (cx) => {
      try {
        while (this.watching.has(repo)) {
          await watchFolder(repo);
          const mine = JSON.stringify([null, repo]);
          if (this.watching.has(repo)) this.refreshGit(Object.keys(this.git).filter((k) => this.git[k]?.repo === mine), cx);
        }
      } catch { this.watching.delete(repo); }
    });
  }

  menu(w) {
    // A popup is as tall as its items; height is only the most.
    openOverlay({ mod: "menu", kind: "popup", width: 200, height: 260, props: { tab: "space", target: { id: w.id } } });
  }

  // Spaces in order, each worktree space moved under its main checkout's.
  ordered() {
    const sp = spaces(), ps = panes();
    const ready = (w) => this.shown.has(w.id) || where(w, ps) in this.git || Date.now() - (this.born[w.id] ??= Date.now()) > 1500;
    for (const w of sp) if (!this.shown.has(w.id) && ready(w)) this.shown.add(w.id);
    const list = sp.map((w, i) => ({ w, i, g: this.git[where(w, ps)] ?? null })).filter((r) => this.shown.has(r.w.id));
    const mains = new Set(list.filter((r) => r.g && !r.g.linked).map((r) => r.g.repo));
    const nested = (r) => r.g?.linked && mains.has(r.g.repo);
    const out = [];
    const placed = new Set(); // repos whose worktrees are listed (under the first of its spaces)
    for (const r of list) {
      if (nested(r)) continue;
      out.push({ ...r, tree: null });
      if (!r.g || r.g.linked || placed.has(r.g.repo)) continue;
      placed.add(r.g.repo);
      const kids = list.filter((k) => nested(k) && k.g.repo === r.g.repo);
      kids.forEach((k, n) => out.push({ ...k, tree: n === kids.length - 1 ? "└─" : "├─" }));
    }
    return out;
  }

  // The rows, under a header per machine once one isn't this computer.
  grouped(t, collapsed) {
    const rows = this.ordered();
    const draw = (r) => (collapsed ? this.compact(r, t) : this.row(r, t));
    const remote = [...new Set([...rows.map(r => r.w.machine).filter(Boolean), ...this.machines.map(m => m.name)])].sort();
    const loading = m => {
      const layout = machines().find(k => k.name === m)?.layout;
      return layout === "loading" || (!layout && this.machines.some(k => k.name === m && k.state === "connected"))
        || spaces().some(s => (s.machine ?? null) === m && !this.shown.has(s.id));
    };
    const loadingRow = m => div().id(`machine-loading-${m ?? "local"}`).h(28).flex_none().px(8).text_size(12).text_color(t.dim).child(collapsed ? "…" : "Loading…");
    if (!remote.length) return loading(null) && !rows.length ? [loadingRow(null)] : this.foldered(rows, t, collapsed, draw, null);
    const machineNames = [null, ...remote];
    return machineNames.flatMap((m) => {
      const mine = rows.filter((r) => (r.w.machine ?? null) === m);
      const machine = this.machines.find(k => k.name === m);
      const connected = !m || machine?.state === "connected";
      const folded = this.folded.has(m);
      const head = h_flex().id(`machine-${m ?? "local"}`).h(26).flex_none().px(collapsed ? 2 : 8).gap(7).items_center().text_size(11).text_color(t.dim)
        .cursor_pointer().hover(s => s.bg(t.hover))
        .on_click((_e, cx) => {
          folded ? this.folded.delete(m) : this.folded.add(m);
          localStorage.setItem("machines.folded", JSON.stringify([...this.folded]));
          cx.notify();
        })
        .child(svg(`mods/workspace-chrome/${m ? "server" : "monitor"}.svg`).size(13).flex_none())
        .when(!collapsed, el => el.child(div().flex_1().truncate().child(m ?? "This computer"))
          .when(!connected, el => el.child(div().flex_none().child((machine?.state ?? "asleep").replace(/_/g, " "))))
          .when(connected, el => el.child(div().id(`machine-add-${m ?? "local"}`).flex_none().px(4).rounded(4).hover(s => s.bg(t.hover).text_color(t.foreground))
            .on_click((_e, cx) => { cx.stop_propagation(); run("space.new", m ? { machine: m } : {}); }).child("+")))
          .child(div().size(6).rounded(3).flex_none().bg(connected ? t.ansi2 : t.dim))
          .child(svg(`mods/workspace-chrome/chevron-${folded ? "right" : "down"}.svg`).size(11).flex_none()));
      const empty = h_flex().id(`machine-new-${m ?? "local"}`).h(28).flex_none().px(8).gap(7).items_center().rounded(7)
        .text_size(12).text_color(t.dim).hover(s => s.bg(t.hover).text_color(t.foreground))
        .on_click(() => run("space.new", m ? { machine: m } : {}))
        .child(svg("mods/workspace-chrome/plus.svg").size(12)).when(!collapsed, el => el.child("New space there"));
      return [head, ...(folded ? [] : mine.length ? this.foldered(mine, t, collapsed, draw, m) : [loading(m) ? loadingRow(m) : empty])];
    });
  }

  // Rows with each folder's spaces (and their worktrees, and its folders) moved
  // under its header, where the first of them sits. A folder with spaces on two
  // machines has a header in each machine's block.
  foldered(rows, t, collapsed, draw, m) {
    const folders = shared("space.folders") ?? [];
    if (!folders.length) return rows.map(draw);
    // A worktree row goes with the space above it.
    const units = [];
    for (const r of rows) {
      if (r.tree && units.length) units[units.length - 1].rows.push(r);
      else units.push({ path: lineage(folders, folderOf(r.w.id)).reverse(), rows: [r] });
    }
    const out = [];
    const emit = (list, depth) => {
      const placed = new Set();
      for (const u of list) {
        const name = u.path[depth];
        if (!name) {
          for (const r of u.rows) out.push(collapsed || !depth ? draw(r) : div().id(`in-folder-${r.w.id}`).flex_none().pl(12 * depth).child(draw(r)));
          continue;
        }
        if (placed.has(name)) continue;
        placed.add(name);
        const f = folders.find((g) => g.name === name);
        const inside = list.filter((v) => v.path[depth] === name);
        const members = inside.flatMap((v) => v.rows);
        const status = URGENCY.find((s) => members.some((r) => r.w.status === s)) ?? "idle";
        out.push(this.folderHead(f, depth, members.length, status, t, collapsed, m));
        if (!f.collapsed) emit(inside, depth + 1);
      }
    };
    emit(units, 0);
    return out;
  }

  folderHead(f, depth, count, status, t, collapsed, m) {
    const toggle = (_e, cx) => {
      if (this.ghost) return; // the end of a drag, not a click
      const list = (shared("space.folders") ?? []).map((g) => (g.name === f.name ? { ...g, collapsed: !g.collapsed } : g));
      setShared("space.folders", list);
      cx.notify();
    };
    const dragging = this.pressed !== null || (this.pressedFolder !== null && this.pressedFolder !== f.name);
    return h_flex().id(`folder-${m ?? "local"}-${f.name}`).h(26).flex_none().pl(collapsed ? 2 : 8 + 12 * depth).pr(collapsed ? 2 : 8).gap(7).items_center()
      .rounded(7).text_size(12).text_color(t.muted).cursor_pointer().hover((s) => s.bg(t.hover))
      .when(collapsed, (el) => el.justify_center())
      .when(dragging, (el) => el.hover((s) => s.bg(t.selected).text_color(t.foreground)))
      .when(this.pressedFolder === f.name && Boolean(this.ghost), (el) => el.opacity(0.5))
      .on_mouse_down("left", (e) => {
        this.pressedFolder = f.name;
        this.start = e.position;
        this.ghost = null;
      })
      .on_mouse_move((_e, cx) => {
        if (this.over === null) return;
        this.over = null;
        cx.notify();
      })
      .on_mouse_up("left", (_e, cx) => {
        const space = this.pressed !== null ? spaces()[this.pressed]?.id : null;
        const folder = this.pressedFolder;
        this.pressed = null;
        this.over = null;
        this.pressedFolder = null;
        if (space) moveToFolder(space, f.name);
        else if (folder && folder !== f.name) {
          const list = shared("space.folders") ?? [];
          const parent = list.find((g) => g.name === folder)?.parent ?? null;
          nestFolder(folder, parent === f.name ? list.find((g) => g.name === f.name)?.parent ?? null : f.name);
        } else return;
        this.pressedFolder = null;
        this.ghost = null;
        cx.stop_propagation();
        cx.notify();
      })
      .on_click(toggle)
      .on_mouse_down("right", () => openOverlay({ mod: "menu", kind: "popup", width: 200, height: 260, props: { tab: "folder", target: { name: f.name } } }))
      .child(svg(`mods/workspace-chrome/chevron-${f.collapsed ? "right" : "down"}.svg`).size(11).flex_none())
      .when(!collapsed, (el) => el
        .child(div().flex_1().min_w(0).truncate().font_medium().child(f.name))
        .when(f.collapsed, (el) => el.child(div().flex_none().text_size(11).text_color(t.dim).child(String(count))))
        .child(div().flex_none().text_color(tone(t, status)).child(glyph(status))));
  }

  // Only what's unusual, after the name: "herdr \u00b7 spike \u21912 \u21931". The repo or
  // branch goes when the name already says it (a worktree is named after its branch),
  // and master/main go: nearly every space is on one.
  gitTail(g, tree, name, t) {
    const raw = g.branch ?? "detached";
    const same = (a) => a.toLowerCase() === name.toLowerCase();
    const branch = same(raw) || raw === "master" || raw === "main" ? "" : raw;
    const repo = tree || same(g.name) ? "" : g.name;
    const where = [repo, branch].filter(Boolean).join(" \u00b7 ");
    return h_flex()
      .flex_1()
      .min_w(0)
      .gap(5)
      .justify_end()
      .items_center()
      .text_size(11.5)
      .text_color(t.dim)
      .when(Boolean(where), (el) => el.child(div().min_w(0).truncate().child(where)))
      .when(g.ahead > 0, (el) => el.child(div().flex_none().text_color(t.ansi2).child(`\u2191${g.ahead}`)))
      .when(g.behind > 0, (el) => el.child(div().flex_none().text_color(t.ansi1).child(`\u2193${g.behind}`)));
  }

  // Closed spaces leave their folders (so a folder goes with its last space). Not
  // those on a machine whose spaces aren't known now: away, or still loading.
  forgetClosed(sp) {
    const list = shared("space.folders") ?? [];
    if (!list.length) return;
    const ids = new Set(sp.map((s) => s.id));
    const ready = new Set(machines().filter((k) => k.layout === "ready").map((k) => k.name ?? null));
    // A space on another machine has an id "machine/id".
    const gone = (id) => !ids.has(id) && ready.has(id.includes("/") ? id.slice(0, id.indexOf("/")) : null);
    if (list.some((f) => f.ids.some(gone))) setShared("space.folders", prune(list.map((f) => ({ ...f, ids: f.ids.filter((id) => !gone(id)) }))));
  }

  avatar(w, t) {
    const color = (shared("space.colors") ?? {})[w.id] ?? "ansi6";
    const fill = color.startsWith("#") ? color : (MARK[color] ?? t[color]);
    return h_flex().size(18).flex_none().rounded(5).bg(fill).items_center().justify_center().text_size(10.5).font_bold().text_color("#ffffff").child([...w.name][0]?.toUpperCase() ?? "?");
  }

  // Whether what's being dragged sits in a folder (so the top is somewhere to go).
  draggingNested() {
    if (!this.ghost) return false;
    if (this.pressedFolder !== null) return Boolean((shared("space.folders") ?? []).find((f) => f.name === this.pressedFolder)?.parent);
    const w = this.pressed !== null && spaces()[this.pressed];
    return Boolean(w && folderOf(w.id));
  }

  // A drop on the title, an empty part of the list or the drop zone: to the top.
  dropTop(cx) {
    const space = this.pressed !== null && this.ghost ? spaces()[this.pressed]?.id : null;
    if (this.pressedFolder !== null && this.ghost) nestFolder(this.pressedFolder, null);
    else if (space && folderOf(space)) moveToFolder(space, null);
    this.pressed = null;
    this.over = null;
    this.pressedFolder = null;
    cx.notify();
  }

  topZone(t) {
    return h_flex().id("drop-top").h(28).mt(4).flex_none().px(8).gap(7).items_center().justify_center().rounded(7)
      .border_1().border_dashed().border_color(t.accent).text_size(12).text_color(t.muted)
      .hover((s) => s.bg(t.selected).text_color(t.foreground))
      .child("Move to top level");
  }

  // The dragged space, beside the pointer (never under it: the drop goes to what's under it).
  dragCard(t) {
    if (!this.ghost) return null;
    const w = this.pressed !== null && spaces()[this.pressed];
    const folder = this.pressedFolder;
    if (!w && !folder) return null;
    return h_flex().id("drag-card").absolute().left(this.ghost.x + 14).top(this.ghost.y + 6)
      .h(28).px(8).gap(9).items_center().rounded(8).bg(t.raised).border_1().border_color(t.accent)
      .shadow_md().opacity(0.92).text_size(13).text_color(t.foreground)
      .child(w ? this.avatar(w, t) : svg("mods/workspace-chrome/folder.svg").size(14).flex_none().text_color(t.muted))
      .child(div().max_w(150).truncate().child(w ? w.name : folder));
  }

  row({ w, i, g, tree }, t) {
    const on = w.active;
    return v_flex()
      .relative()
      .id(`space-${w.id}`)
      .px(8)
      .h(rowHeight({ tree }))
      .flex_none()
      .justify_center()
      .rounded(8)
      .when(on, (el) => el.bg(t.selected))
      .when(!on, (el) => el.hover((s) => s.bg(t.hover)))
      .when(this.pressed === i && this.over !== i, (el) => el.opacity(0.5))
      .when(this.pressed !== null && this.over === i && this.pressed !== i, (el) => el.child(
        div().absolute().left(0).right(0).h(2).bg(t.accent)
          .when(this.pressed < i, line => line.bottom(0))
          .when(this.pressed > i, line => line.top(0)),
      ))
      .on_mouse_down("left", (e) => {
        this.pressed = i;
        this.over = i;
        this.start = e.position;
        this.ghost = null;
      })
      .on_mouse_move((_e, cx) => {
        const source = spaces()[this.pressed];
        const over = source && (source.machine ?? null) === (w.machine ?? null) ? i : null;
        if (this.pressed !== null && this.over !== over) {
          this.over = over;
          cx.notify();
        }
      })
      .on_mouse_up("left", (_e, cx) => {
        const from = this.pressed;
        const over = this.over;
        this.pressed = null;
        this.over = null;
        if (this.pressedFolder !== null) {
          nestFolder(this.pressedFolder, folderOf(w.id));
          this.pressedFolder = null;
        } else if (from !== null && over === i && from !== i) {
          const dragged = spaces()[from].id;
          run("space.move", { id: dragged, index: i + 1 });
          if (folderOf(dragged) !== folderOf(w.id)) moveToFolder(dragged, folderOf(w.id));
        } else if (from === i) run("space.select", { id: w.id });
        cx.notify();
      })
      .on_mouse_down("right", () => this.menu(w))
      .child(
        h_flex()
          .gap(9)
          .items_center()
          .when(Boolean(tree), el => el.pl(22).child(svg("mods/workspace-chrome/branch.svg").size(13).text_color(t.dim)))
          .when(!tree, el => el.child(this.avatar(w, t)))
          .child(div().flex_shrink(0).max_w(150).truncate().text_color(t.foreground).child(w.name))
          .child(g ? this.gitTail(g, tree, w.name, t) : div().flex_1())
          .when(w.status === "blocked" || w.status === "done", (el) => el.child(div().flex_none().text_color(tone(t, w.status)).child(glyph(w.status)))),
      );
  }

  compact({ w, i }, t) {
    return h_flex()
      .id(`space-${w.id}`)
      .h(26)
      .flex_none()
      .justify_center()
      .gap(4)
      .when(w.active, (el) => el.bg(t.raised))
      .hover((s) => s.bg(t.hover))
      .on_click(() => run("space.select", { id: w.id }))
      .child(div().text_color(w.active ? t.foreground : t.muted).child(String(i + 1)))
      .child(div().text_color(tone(t, w.status)).child(glyph(w.status)));
  }

  render() {
    const t = theme();
    const collapsed = narrow();
    const palette = ["ansi6", "ansi3", "ansi4", "ansi5", "ansi2", "ansi1"];
    const sp = spaces();
    const ids = new Set(sp.map(p => p.id));
    const kept = Object.fromEntries(Object.entries(shared("space.colors") ?? {}).filter(([id]) => ids.has(id)));
    let changed = Object.keys(kept).length !== Object.keys(shared("space.colors") ?? {}).length;
    for (const p of sp) {
      if (kept[p.id] || p.name === "~") continue;
      const uses = c => Object.values(kept).filter(v => v === c).length;
      kept[p.id] = palette.reduce((best, c) => uses(c) < uses(best) ? c : best);
      changed = true;
    }
    if (changed) setShared("space.colors", kept);
    this.forgetClosed(sp);
    if (this.pressed === null && this.pressedFolder === null) this.ghost = null;
    const card = this.dragCard(t);
    return v_flex().id("spaces-root").relative().size_full().min_h(0).bg(t.chrome).text_size(13)
      .on_mouse_move((e, cx) => {
        if ((this.pressed === null && this.pressedFolder === null) || !this.start) return;
        if (!this.ghost && Math.hypot(e.position.x - this.start.x, e.position.y - this.start.y) < 4) return;
        this.ghost = e.local_position;
        cx.notify();
      })
      .on_hover((hovered, cx) => {
        if (hovered || (this.pressed === null && this.pressedFolder === null)) return;
        this.pressed = null;
        this.over = null;
        this.pressedFolder = null;
        cx.notify();
      })
      .child(h_flex().id("spaces-title").h(31).pl(16).pr(10).items_center().justify_between().text_size(11.5).text_color(t.dim)
        .on_mouse_up("left", (_e, cx) => this.dropTop(cx))
        .when(!collapsed, el => el.child("Spaces"))
        .child(div().id("space-new").size(26).flex_none().flex().items_center().justify_center().rounded(6).hover(s => s.bg(t.hover).text_color(t.foreground))
          .on_click(() => run("space.new")).child(svg("mods/workspace-chrome/plus.svg").size(14))))
      .child(v_flex().flex_1().min_h(0).px(8).pb(14).overflow_y_scrollbar().mode("always")
        .on_mouse_up("left", (_e, cx) => this.dropTop(cx))
        .children(this.grouped(t, collapsed))
        .when(this.draggingNested(), (el) => el.child(this.topZone(t))))
      .when(Boolean(card), (el) => el.child(card));
  }
}
