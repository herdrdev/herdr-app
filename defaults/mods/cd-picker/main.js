// Go to a folder without typing cd. Opens over the focused terminal with its
// folder's contents; the keys are the ones shells, readline and fzf taught:
//   type            search (dot folders show once the search starts with ".")
//   name/  ../  /  ~/   walk a path as after `cd ` (a pasted path works too)
//   ↑ ↓  Ctrl+P/N  Ctrl+K/J  PgUp/PgDn   move
//   Tab  →          look inside          (→ and ← while the search is empty)
//   ⌫  ←  Shift+Tab go up
//   Enter           cd there             Cmd/Ctrl+Enter  cd here
//   Ctrl+U          clear the search     Esc  Ctrl+C  Ctrl+G  Cmd+G  close
// With the mouse: click a folder to look inside, the breadcrumb to go back,
// "Go here" to cd. It only types into a shell waiting at its prompt: never
// into vim, an agent or a running command.
// Opened by keys.json's "terminal" section: Cmd+G on macOS, Ctrl+Shift+G elsewhere.

import { View, div, svg } from "gpui-kit";
import { v_flex, h_flex, InputState, Input, v_virtual_list, VirtualListScrollHandle } from "gpui-base";
import { focused, hc, exec, home, theme, props, closeOverlay, window } from "herdr";

// A leading space keeps the cd out of history in fish, and in zsh / bash with
// HIST_IGNORE_SPACE / HISTCONTROL=ignorespace; elsewhere it's harmless.
const KEEP_OUT_OF_HISTORY = true;
// The list keeps one height while you browse, so the dialog doesn't jump.
const ROW = 34, VISIBLE_ROWS = 9, PER_FOLDER = 500, PAGE = VISIBLE_ROWS - 1;

export default class CdPicker extends View {
  init(_props, cx) {
    this.filter = "";
    this.at = null;       // the highlighted row; null: pick the default
    this.dir = null;      // the folder shown
    this.entries = null;  // its contents; null while loading
    this.problem = null;  // { title, detail }: why nothing can be typed into this terminal
    this.error = null;    // why the folder didn't list, or the cd failed
    this.busy = false;
    this.good = null;     // { dir, entries }: the last folder that listed
    this.nav = 0;         // counts every move (typed, clicked, keyed), so only the newest one lands
    this.handle = cx.focus_handle();
    this.scroll = VirtualListScrollHandle.new();
    this.input = InputState.new({ placeholder: "Search, or type a path…" });
    this.input.on("change", (_e, cx) => this.typed(cx));
    this.input.focus();
    // The overlay frame takes the keyboard first; take it back once it's up.
    cx.timer.after(600, () => this.input.focus());

    this.target = target();
    if (!this.target) {
      this.problem = { title: "No terminal focused", detail: "Click a terminal, then try again." };
      return;
    }
    cx.spawn(async (cx) => {
      try {
        const now = await inspect(this.target.address);
        this.problem = now.problem;
        this.shell = now.shell;
        this.start = now.cwd;
        if (!this.problem) {
          // Anything typed while the terminal was being read still counts:
          // a search stays a search, a path is walked once there's a folder.
          const early = this.filter;
          await this.load(now.cwd, early);
          if (splitPath(early, now.cwd) !== null) await this.walk(early);
        }
      } catch (e) {
        this.problem = { title: "Can't read this terminal", detail: e.message ?? String(e) };
      }
      cx.notify();
    });
  }

  // Shows `dir`'s contents (Tab, Shift+Tab, Backspace, clicks, typed paths come here),
  // searching it for `filter`.
  browse(dir, cx, filter = "") {
    cx.spawn(async (cx) => {
      await this.load(dir, filter);
      cx.notify();
    });
  }

  async load(dir, filter = "") {
    const turn = ++this.nav;
    // Going up lands on the folder you came out of, as in ranger or lf; it
    // stays in the list even if it's a dot folder.
    this.cameFrom = this.dir === null ? null : childToward(dir, this.dir);
    this.reveal = this.cameFrom;
    this.dir = dir;
    this.entries = null;
    this.error = null;
    this.at = null;
    this.setSearch(filter);
    try {
      const list = await directory(dir, this.target.on);
      // A newer move may have happened while this one listed (even to this same folder).
      if (turn !== this.nav) return;
      this.entries = sortEntries(list);
      this.good = { dir, entries: this.entries };
    } catch (e) {
      if (turn !== this.nav) return;
      // A typo in a typed path (or a folder you can't read) keeps you in the
      // last folder that listed, never in one that doesn't exist.
      if (this.good) Object.assign(this, this.good);
      else this.entries = [];
      this.error = `Can't open “${crumbName(dir)}”`;
    }
  }

  // Sets the search box without it reading as typing.
  setSearch(text) {
    this.filter = text;
    if (this.input.value() === text) return;
    this.settingSearch = true;
    this.input.set_value(text);
    this.settingSearch = false;
  }

  rows(search = this.filter) {
    const f = search.toLowerCase();
    const out = [];
    const up = parent(this.dir);
    if (!f && up) out.push({ kind: "up", name: "..", path: up });
    // Like `cd <Tab>`: dot folders stay out of the way until the search starts with ".".
    const dots = f.startsWith(".");
    const all = this.entries ?? [];
    const shown = (e) => dots || !e.name.startsWith(".") || e.name === this.reveal;
    const hits = all.filter((e) => shown(e) && (!f || e.name.toLowerCase().includes(f)));
    // Searching: names that start with it first, folders before files throughout.
    if (f) hits.sort((a, b) => b.dir - a.dir || b.name.toLowerCase().startsWith(f) - a.name.toLowerCase().startsWith(f));
    for (const e of hits) out.push({ kind: e.dir ? "dir" : "file", name: e.name, path: join(this.dir, e.name) });
    return out;
  }

  // Dot entries kept out of the list (none once the search starts with ".").
  hiddenCount() {
    if (this.filter.startsWith(".")) return 0;
    return (this.entries ?? []).filter((e) => e.name.startsWith(".") && e.name !== this.reveal).length;
  }

  render() {
    const t = theme();
    // Nothing to highlight until the folder has listed, so the highlight is chosen from what's in it.
    const rows = this.dir === null || this.entries === null ? [] : this.rows();
    const picks = rows.map((r, i) => (pickable(r) ? i : -1)).filter((i) => i >= 0);
    this.highlight(rows, picks);

    return v_flex()
      .id("cd-picker")
      .track_focus(this.handle)
      .w_full()
      .bg(t.popover)
      .text_size(13)
      .on_key_down((e, cx) => this.key(e, rows, picks, cx))
      .child(this.search(t))
      .child(this.problem ? this.blocked(t) : this.list(t, rows))
      .child(this.footer(t));
  }

  // Keeps the highlight on a row you can act on: after going up, the folder
  // you came out of; otherwise the first folder (none in a folder without
  // folders, so Enter there means "here", not "up"). Always scrolled into view.
  highlight(rows, picks) {
    if (this.at !== null && picks.includes(this.at)) return;
    const back = this.cameFrom && !this.filter ? rows.findIndex((r) => r.kind === "dir" && r.name === this.cameFrom) : -1;
    if (back >= 0) this.cameFrom = null;
    this.at = back >= 0 ? back : firstPick(rows);
    if (this.at >= 0) this.scroll.scroll_to_item(this.at, back >= 0 ? "center" : "top");
  }

  // "Go here" makes sense once you've moved to a folder that listed.
  canGoHere() {
    return !this.problem && this.entries !== null && this.dir !== null && this.dir !== this.start;
  }

  search(t) {
    return h_flex()
      .h(48)
      .flex_none()
      .px(16)
      .gap(10)
      .items_center()
      .border_b(1)
      .border_color(t.border)
      .child(svg("mods/cd-picker/search.svg").size(15).flex_none().text_color(t.dim))
      .child(Input.new(this.input).flex_1().h_full().text_size(14).text_color(t.foreground))
      // How to see what's left out: type "." (as `cd .<Tab>` would).
      .when(!this.problem && this.hiddenCount() > 0, (el) => el.child(hint(t, ".", `${this.hiddenCount()} hidden`)))
      .when(!!this.target?.on, (el) => el.child(div().text_size(11).text_color(t.dim).child(`on ${this.target.on}`)));
  }

  // Why this terminal can't take a cd, in place of the list.
  blocked(t) {
    return v_flex()
      .h(VISIBLE_ROWS * ROW + 12)
      .items_center()
      .justify_center()
      .gap(6)
      .px(32)
      .child(div().text_size(14).font_semibold().text_color(t.foreground).child(this.problem.title))
      .child(div().text_size(12.5).text_color(t.dim).child(this.problem.detail));
  }

  list(t, rows) {
    const box = v_flex().h(VISIBLE_ROWS * ROW + 12).min_h(0).p(6);
    const quiet = (text) => box.items_center().justify_center().child(div().text_size(12.5).text_color(t.dim).child(text));
    if (this.dir === null || this.entries === null) return quiet("Loading…");
    if (!rows.length) return quiet(this.filter ? `No match for “${this.filter}”` : this.error ?? "Empty folder");
    // Only the way up is left: say why under it.
    const onlyUp = rows.every((r) => r.kind === "up");
    return box
      .child(this.virtualRows(t, rows))
      .when(onlyUp, (el) => el.child(div().pt(24).text_size(12.5).text_color(t.dim).text_center().child(this.error ?? "Empty folder")));
  }

  virtualRows(t, rows) {
    return v_virtual_list("cd-rows", rows.length, ROW, (i) => rowKey(rows[i]),
      (range) => rows.slice(range.start, range.end).map((r, j) => this.row(t, r, range.start + j)))
      .w_full()
      .h(Math.min(rows.length, VISIBLE_ROWS) * ROW)
      .track_scroll(this.scroll)
      // One handler for the whole list (rows can't have their own): a click
      // looks inside a folder, or goes up.
      .on_item_click((key, cx) => {
        const r = rows.find((x) => rowKey(x) === key);
        if (r && pickable(r)) this.browse(r.path, cx);
      });
  }

  row(t, r, i) {
    const on = i === this.at;
    const hidden = r.name.startsWith(".") && r.kind !== "up";
    const tone = r.kind === "file" ? t.dim : hidden ? t.muted : t.foreground;
    return h_flex()
      .id(`cd-${i}`)
      .w_full()
      .h(ROW)
      .px(10)
      .gap(10)
      .items_center()
      .rounded(8)
      .when(on, (el) => el.bg(t.hover))
      .when(r.kind !== "file", (el) => el.cursor_pointer().hover((s) => s.bg(t.hover)))
      .child(svg(`mods/cd-picker/${r.kind === "up" ? "up" : r.kind === "file" ? "file" : "folder"}.svg`)
        .size(15).flex_none().text_color(r.kind === "dir" && !hidden ? t.accent : t.dim))
      .child(div().flex_1().min_w(0).truncate().text_color(tone)
        .child(r.kind === "up" ? `Up to ${crumbName(r.path)}` : r.name))
      // Keys where the action is: the highlighted row says what Tab and Enter do.
      .when(on, (el) => el.child(h_flex().gap(10).flex_none()
        .child(hint(t, "⇥", "open"))
        .child(hint(t, "↵", "go"))))
      .when(!on && r.kind === "up", (el) => el.child(kbd(t, "⌫")));
  }

  footer(t) {
    const mac = window()?.platform === "macos";
    return h_flex()
      .h(46)
      .flex_none()
      .px(12)
      .gap(10)
      .items_center()
      .border_t(1)
      .border_color(t.border)
      .text_size(12)
      .child(this.error && this.entries?.length
        ? div().flex_1().min_w(0).truncate().text_color(t.blocked).child(this.error)
        : this.breadcrumb(t))
      .child(this.canGoHere()
        ? h_flex().id("cd-go").h(28).pl(12).pr(6).gap(8).items_center().rounded(7).bg(t.solid).text_color(t.solid_foreground)
            .cursor_pointer().hover((s) => s.opacity(0.88)).on_click((_e, cx) => this.go(this.dir, cx))
            .child(this.busy ? "Going…" : "Go here")
            .child(kbd(t, mac ? "⌘↵" : "Ctrl ↵", true))
        : hint(t, "esc", "close"));
  }

  // Where you are, each part a way back to it.
  breadcrumb(t) {
    const bar = h_flex().flex_1().min_w(0).gap(4).items_center().text_color(t.dim);
    if (this.dir === null) return bar;
    const parts = crumbs(this.dir, this.target.on);
    // Deep paths keep their last three parts, and ~ when it's where they start.
    const more = { label: "…", path: null };
    const shown = parts.length <= 4 ? parts : parts[0].label === "~" ? [parts[0], more, ...parts.slice(-3)] : [more, ...parts.slice(-3)];
    shown.forEach((p, i) => {
      const last = i === shown.length - 1;
      if (i) bar.child(div().text_color(t.dim).child("›"));
      bar.child(div().id(`crumb-${i}`).px(4).py(1).rounded(5).truncate()
        .text_color(last ? t.foreground : t.dim)
        .when(last, (d) => d.font_semibold())
        .when(!last && p.path !== null, (d) => d.cursor_pointer().hover((s) => s.bg(t.hover).text_color(t.foreground))
          .on_click((_e, cx) => this.browse(p.path, cx)))
        .child(p.label));
    });
    return bar;
  }

  // Typing searches; a "/" in it walks a path the way `cd` reads one:
  // "src/" looks inside, "../" goes up, "/" starts at the root, "~/" at home.
  typed(cx) {
    if (this.settingSearch) return;
    const v = this.input.value();
    this.error = null;
    if (this.dir === null || splitPath(v, this.dir) === null) {
      this.filter = v;
      this.at = null;
      cx.notify();
      return;
    }
    cx.spawn(async (cx) => {
      await this.walk(v);
      cx.notify();
    });
  }

  // Goes where a typed path leads and searches its last part: "src/co" opens
  // src and searches "co".
  async walk(text) {
    const [head, rest] = splitPath(text, this.dir);
    const turn = ++this.nav;
    try {
      const dir = await this.resolve(head);
      // Moved on while a remote home was looked up (typed on, clicked, a key): that wins.
      if (turn !== this.nav || this.input.value() !== text) return;
      await this.load(dir, rest);
    } catch (e) {
      if (turn !== this.nav) return;
      // A folder still loading when this started was dropped: show the last good one.
      if (this.entries === null && this.good) Object.assign(this, this.good);
      this.setSearch(rest);
      this.error = e.message ?? String(e);
    }
  }

  async resolve(head) {
    const win = windowsPath(this.dir);
    const drive = win ? head.match(/^([A-Za-z]:)[\\/]/) : null;
    const share = win ? head.match(/^[\\/]{2}([^\\/]+)[\\/]([^\\/]+)(?:[\\/]|$)/) : null;
    let dir = this.dir, path = head;
    if (share) {
      dir = `\\\\${share[1]}\\${share[2]}`;
      path = head.slice(share[0].length);
    } else if (drive) {
      dir = `${drive[1]}\\`;
      path = head.slice(3);
    } else if (head.startsWith("/") || (win && head.startsWith("\\"))) {
      while (parent(dir) !== null) dir = parent(dir);
      path = head.slice(1);
    } else if (head.startsWith("~/") || (win && head.startsWith("~\\"))) {
      dir = await homeOf(this.target.on);
      path = head.slice(2);
    }
    for (const part of path.split(win ? /[\\/]/ : "/")) {
      if (!part || part === ".") continue;
      if (part === "..") dir = parent(dir) ?? dir;
      // A name in the folder on screen may be a search: "sr/" opens what it picked.
      else dir = join(dir, dir === this.dir ? this.pick(part) : part);
    }
    return dir;
  }

  // The folder a typed name means here: exactly that, else the highlighted one.
  pick(name) {
    const dirs = (this.entries ?? []).filter((e) => e.dir);
    const exact = dirs.find((e) => e.name === name) ?? dirs.find((e) => e.name.toLowerCase() === name.toLowerCase());
    if (exact) return exact.name;
    // The row highlighted while searching for it (its first pick, if typed faster than a redraw).
    const rows = this.rows(name);
    const shown = name === this.filter && this.at !== null && rows[this.at] && pickable(rows[this.at]);
    const sel = rows[shown ? this.at : firstPick(rows)];
    return sel?.kind === "dir" ? sel.name : name;
  }

  // The keys terminal people already know: readline and fzf to move and cancel,
  // shell completion and file managers (ranger, lf, yazi) to walk folders.
  key(e, rows, picks, cx) {
    const k = e.keystroke, sel = rows[this.at], empty = !this.filter;
    if (["escape", "ctrl-c", "ctrl-g", "cmd-g"].includes(k)) {
      // Not while a cd is being typed: it finishes, then the dialog closes itself.
      if (!this.busy) this.dismiss();
      cx.stop_propagation();
      return;
    }
    if (this.problem || this.busy || this.dir === null) return;
    const step = { down: 1, "ctrl-n": 1, "ctrl-j": 1, up: -1, "ctrl-p": -1, "ctrl-k": -1, pagedown: PAGE, pageup: -PAGE }[k];
    if (step) {
      this.move(step, picks);
    } else if (k === "tab" || (k === "right" && empty)) {
      if (sel && pickable(sel)) this.browse(sel.path, cx);
    } else if (k === "shift-tab" || ((k === "left" || k === "backspace") && empty)) {
      const up = parent(this.dir);
      if (up) this.browse(up, cx);
    } else if (k === "ctrl-u") {
      this.setSearch("");
      this.at = null;
    } else if (k === "cmd-enter" || k === "ctrl-enter") {
      if (this.canGoHere()) this.go(this.dir, cx);
    } else if (k === "enter") {
      // Nothing highlighted in a folder without folders: Enter means here
      // (not after a search that matched nothing).
      if (sel && pickable(sel)) this.go(sel.path, cx);
      else if (empty && this.canGoHere()) this.go(this.dir, cx);
    } else return;
    cx.stop_propagation();
    cx.notify();
  }

  // One row at a time wraps around; a page stops at the ends.
  move(step, picks) {
    if (!picks.length) return;
    const i = Math.max(0, picks.indexOf(this.at)), n = picks.length;
    const j = Math.abs(step) === 1 ? (i + step + n) % n : Math.max(0, Math.min(n - 1, i + step));
    this.at = picks[j];
    this.scroll.scroll_to_item(this.at);
  }

  // Types the cd into the terminal, then closes. Stays open with the reason if it can't.
  go(path, cx) {
    if (this.busy) return;
    this.busy = true;
    this.error = null;
    const { address } = this.target;
    cx.spawn(async (cx) => {
      try {
        // Still a shell at its prompt? Something may have started since the dialog opened.
        const now = await inspect(address);
        if (now.problem) throw new Error(now.problem.title);
        await typeLine(address, cdLine(now.shell, path));
        this.dismiss();
      } catch (e) {
        this.busy = false;
        this.error = `Couldn't cd: ${e.message ?? e}`;
        cx.notify();
      }
    });
  }

  dismiss() {
    closeOverlay(props().overlay);
  }
}

// The terminal the dialog opened over (the one its pane shows for now, if
// swapped), and the machine it runs on: remote addresses are `machine/t7`.
function target() {
  const f = focused();
  const address = f?.shown ?? f?.terminal;
  if (!address) return null;
  return { address, on: address.includes("/") ? address.split("/")[0] : null };
}

// What runs in front in the terminal, from the core: a shell waiting at its
// prompt is the only thing it is safe to type a cd into. A shell's name isn't
// enough: a bash script waiting on `read`, or the subshell zsh forks for `( … )`,
// is "bash"/"zsh" in front too, but not at a prompt.
async function inspect(address) {
  const r = await hc(["get", address, "--json"]);
  if (r.code !== 0) throw new Error(r.stderr.trim() || `hc get ${address} failed`);
  const rec = JSON.parse(r.stdout);
  const fg = rec.foreground;
  const name = programName(fg?.name ?? fg?.argv?.[0] ?? "");
  const shell = SHELLS[name] ?? null;
  const cwd = fg?.cwd ?? rec.cwd ?? null;
  const atPrompt = "Go to folder works at a shell prompt.";
  let problem = null;
  if (rec.state !== "live") problem = { title: "This terminal has ended", detail: "Open a new one to change folders." };
  else if (!fg || !name) problem = { title: "Can't tell what's running here", detail: atPrompt };
  else if (!shell) problem = { title: `${name} is running`, detail: atPrompt };
  else if (!atItsPrompt(fg, rec)) {
    const script = (fg.argv ?? []).slice(1).find((a) => !a.startsWith("-"));
    problem = { title: script ? `${crumbName(script)} is running` : "A command is running", detail: atPrompt };
  } else if (!cwd) problem = { title: "Can't tell where this terminal is", detail: atPrompt };
  return { problem, shell, name, cwd };
}

// The terminal's own shell is in front (same process), or one you started by
// hand inside it (`bash`, `zsh -l`). Either way it must be interactive: only
// options, no script, no -c command. And not a fork of the terminal's shell
// (same command, other process). What it can't see: a `read` typed at that
// prompt, or a half-finished `for …; do` waiting for more lines.
function atItsPrompt(fg, rec) {
  const interactive = (argv) => (argv ?? []).slice(1).every((a) => a.startsWith("-") && !/^-[^-]*c/i.test(a) && a !== "--command");
  if (fg.pid === rec.pid) return interactive(rec.argv ?? fg.argv);
  return interactive(fg.argv) && JSON.stringify(fg.argv) !== JSON.stringify(rec.argv);
}

const SHELLS = {
  sh: "posix", bash: "posix", zsh: "posix", dash: "posix", ksh: "posix", mksh: "posix",
  oksh: "posix", ash: "posix", yash: "posix", tcsh: "csh", csh: "csh",
  fish: "fish", nu: "nu", elvish: "elvish", pwsh: "pwsh", powershell: "pwsh", cmd: "cmd",
};

// "/bin/zsh", "-zsh" (a login shell), "pwsh.exe" → "zsh", "zsh", "pwsh".
function programName(s) {
  return s.split(/[\\/]/).pop().replace(/^-/, "").replace(/\.exe$/i, "").toLowerCase();
}

// Names and whether each is a folder, here or on the terminal's machine
// (`on`): folders first, at most PER_FOLDER in all, so a folder full of files
// still shows every subfolder.
async function directory(path, on) {
  if (!on && window()?.platform === "windows") {
    // -Command joins its arguments into code, so the path goes in quoted, never as
    // its own argument; UTF-8 out so names like "José" survive.
    const r = await exec("powershell", ["-NoProfile", "-Command", `[Console]::OutputEncoding = [Text.Encoding]::UTF8; & { param($p) @(Get-ChildItem -LiteralPath $p -Force -ErrorAction Stop | Sort-Object { -not $_.PSIsContainer } | Select-Object -First ${PER_FOLDER} | ForEach-Object { @{ name=$_.Name; dir=$_.PSIsContainer } }) | ConvertTo-Json -Compress } ${psQuote(path)}`]);
    if (r.code !== 0) throw new Error(r.stderr.trim() || "Can't list this folder");
    const result = JSON.parse(r.stdout || "[]");
    return Array.isArray(result) ? result : [result];
  }
  // Symlinks to folders count as folders: cd follows them. A folder you can't
  // read fails here rather than looking empty.
  const script = `n=0; for want in d f; do for p in "$1"/* "$1"/.[!.]* "$1"/..?*; do [ -e "$p" ] || [ -L "$p" ] || continue; if [ -d "$p" ]; then kind=d; else kind=f; fi; [ "$kind" = "$want" ] || continue; printf "%s\\0%s\\0" "$kind" "\${p##*/}"; n=$((n+1)); [ "$n" -ge ${PER_FOLDER} ] && exit 0; done; done; [ -d "$1" ] && [ -r "$1" ] && [ -x "$1" ]`;
  const r = await exec("sh", ["-c", script, "cd-picker", path], { on });
  if (r.code !== 0) throw new Error(r.stderr.trim() || "Can't list this folder");
  const fields = r.stdout.split("\0"), out = [];
  for (let i = 0; i + 1 < fields.length; i += 2) out.push({ name: fields[i + 1], dir: fields[i] === "d" });
  return out;
}

// Folders first, hidden ones after the rest, then by name.
function sortEntries(list) {
  const hidden = (e) => e.name.startsWith(".");
  return list.sort((a, b) => b.dir - a.dir || hidden(a) - hidden(b) || a.name.localeCompare(b.name, undefined, { sensitivity: "base" }));
}

// Windows paths use backslashes; everything else, and remote machines, slashes.
const windowsPath = (p) => /^[A-Za-z]:\\/.test(p) || p.startsWith("\\\\");

function parent(dir) {
  if (windowsPath(dir)) {
    const trimmed = dir.replace(/\\+$/, "");
    const i = trimmed.lastIndexOf("\\");
    // The top is a drive (C:) or a network share (\\server\share).
    if (i < 0 || /^[A-Za-z]:$/.test(trimmed) || /^\\\\[^\\]+\\[^\\]+$/.test(trimmed)) return null;
    const up = trimmed.slice(0, i);
    return /^[A-Za-z]:$/.test(up) ? `${up}\\` : up;
  }
  if (dir === "/") return null;
  const trimmed = dir.replace(/\/+$/, "");
  const i = trimmed.lastIndexOf("/");
  return i <= 0 ? "/" : trimmed.slice(0, i);
}

function join(dir, name) {
  const sep = windowsPath(dir) ? "\\" : "/";
  return dir.endsWith(sep) ? dir + name : dir + sep + name;
}

// A typed path's folder part and its last part: "src/co" → ["src/", "co"];
// null for a plain search. Windows folders take "\" as well as "/".
function splitPath(text, dir) {
  const win = dir !== null && windowsPath(dir);
  const cut = Math.max(text.lastIndexOf("/"), win ? text.lastIndexOf("\\") : -1);
  return cut < 0 ? null : [text.slice(0, cut + 1), text.slice(cut + 1)];
}

// Rows you can highlight, open and cd into: folders and the way up.
function pickable(r) {
  return r.kind === "dir" || r.kind === "up";
}

// The child of `dir` on the way to `from`: childToward("/a", "/a/b/c") → "b";
// null when `from` isn't inside `dir`.
function childToward(dir, from) {
  const sep = windowsPath(dir) ? "\\" : "/";
  const prefix = dir.endsWith(sep) ? dir : dir + sep;
  if (!from.startsWith(prefix)) return null;
  return from.slice(prefix.length).split(sep)[0] || null;
}

// Where the highlight starts: the first folder, so Tab and Enter go somewhere
// at once; -1 (nothing) when there are none.
function firstPick(rows) {
  return rows.findIndex((r) => r.kind === "dir");
}

// A row's identity in the list, and what a click on it reports.
function rowKey(r) {
  return `${r.kind}:${r.name}`;
}

// A folder's own name: "/a/b" → "b", "/" → "/", "C:\" → "C:".
function crumbName(path) {
  const trimmed = path.replace(/[\\/]+$/, "");
  return trimmed.split(/[\\/]/).pop() || path;
}

// A key as the app draws it: a small raised cap.
function kbd(t, key, onSolid = false) {
  return div().h(18).min_w(18).px(5).flex().items_center().justify_center().rounded(5)
    .text_size(10.5).font_semibold()
    // On the button: its text color, faint, so it reads on light and dark themes.
    .bg(onSolid && /^#[0-9a-f]{6}$/i.test(t.solid_foreground) ? `${t.solid_foreground}2e` : t.raised)
    .text_color(onSolid ? t.solid_foreground : t.muted)
    .child(key);
}

// A key and what it does: "⇥ open".
function hint(t, key, what) {
  return h_flex().gap(5).items_center().text_size(11.5).text_color(t.dim).child(kbd(t, key)).child(what);
}

// The path as breadcrumb parts, each with where it leads; home shows as ~
// on this computer (a remote home isn't known here).
function crumbs(dir, on) {
  const h = on ? null : home();
  const out = [];
  for (let p = dir; p !== null; p = parent(p)) {
    if (h && p === h) {
      out.push({ label: "~", path: p });
      break;
    }
    out.push({ label: crumbName(p), path: p });
  }
  return out.reverse();
}

// The home folder where the terminal runs: asked of that machine once (one
// question in flight per machine, however fast you type "~/").
const homes = new Map();
async function homeOf(on) {
  if (!on) return home();
  if (!homes.has(on)) {
    homes.set(on, exec("sh", ["-c", 'printf %s "$HOME"'], { on }).then((r) => {
      if (r.code !== 0 || !r.stdout) throw new Error(`Can't find the home folder on ${on}`);
      return r.stdout;
    }));
  }
  try {
    return await homes.get(on);
  } catch (e) {
    homes.delete(on);
    throw e;
  }
}

// Types the line into the terminal. Ctrl+C first drops whatever is half-typed,
// in emacs and vi modes alike, and the line follows at once. One process does
// both where there's a sh, so the dialog closing midway can't leave the prompt
// cleared and the cd not typed.
async function typeLine(address, line) {
  if (window()?.platform === "windows") {
    for (const args of [["keys", address, "ctrl+c"], ["send", address, line]]) {
      const r = await hc(args);
      if (r.code !== 0) throw new Error(r.stderr.trim() || `hc ${args[0]} failed`);
    }
    return;
  }
  const r = await exec("sh", ["-c", 'hc keys "$1" ctrl+c && hc send "$1" "$2"', "cd-picker", address, line]);
  if (r.code !== 0) throw new Error(r.stderr.trim() || "hc couldn't type into the terminal");
}

// The cd for this shell, with the path quoted the way that shell reads it.
// Throws for a name it can't type safely.
function cdLine(shell, path) {
  // A control character (newline, Esc, Ctrl+U…) would be typed as a key, not as text.
  if (/[\x00-\x1f\x7f]/.test(path)) throw new Error("this folder's name has control characters");
  const lead = KEEP_OUT_OF_HISTORY ? " " : "";
  switch (shell) {
    case "fish": return `${lead}cd '${path.replace(/[\\']/g, "\\$&")}'`;
    // A raw string ends at ' plus its #s: use more #s than any run in the name.
    case "nu": {
      const longest = Math.max(0, ...[...path.matchAll(/'(#*)/g)].map((m) => m[1].length));
      const hashes = "#".repeat(longest + 1);
      return `${lead}cd r${hashes}'${path}'${hashes}`;
    }
    case "elvish": return `${lead}cd '${path.replaceAll("'", "''")}'`;
    case "pwsh": return `${lead}Set-Location -LiteralPath ${psQuote(path)}`;
    // cmd expands %VAR% (and !VAR! with delayed expansion) even inside quotes,
    // and has no way to escape a quote.
    case "cmd":
      if (/[%!"]/.test(path)) throw new Error("cmd can't take %, ! or \" in a folder name");
      return `${lead}cd /d "${path}"`;
    // csh and tcsh expand ! (history) even inside single quotes.
    case "csh": return `${lead}cd '${path.replaceAll("'", `'\\''`).replaceAll("!", "\\!")}'`;
    default: return `${lead}cd '${path.replaceAll("'", `'\\''`)}'`;
  }
}

// A PowerShell single-quoted string. PowerShell reads ‘ ’ ‚ ‛ as single quotes
// too, so each of them is doubled like ' (as Herdr's own powershell_line does).
function psQuote(s) {
  return `'${s.replace(/['‘’‚‛]/g, "$&$&")}'`;
}
