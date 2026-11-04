# herdr-app: drive the person's window

Herdr shows spaces, tabs and panes. herdr-core (`hc`) runs their terminals
and keeps them alive when the window closes. Use `herdr-app` only to change
what the person sees. For starting, reading, messaging, waiting on or cleaning
up agents and programs, read `hc --skill` and use hc.
Your terminal address is `$HC_TERMINAL_ID`; it is not a pane ID.

## Inspect before acting

```sh
herdr-app ls          # spaces > tabs > terminals, including pane IDs
herdr-app status      # current layout, focus, overlays, zoom and file/mod errors
herdr-app commands    # available commands, arguments and descriptions
```

`ls` and `status` talk to the running window, not a new app. `commands`
prints the built-in command list even without an open window. Do not launch bare
`herdr-app` to inspect it: that opens or raises the window. Do not stop,
signal or restart the person's app or core. A window closing does not kill
its terminals.

`app.check_updates` checks for newer releases. Settings reports "Up to date"
only after a successful check; on failure, check the connection and retry.
Package-managed cores are checked but updated through their package manager.
Dismiss a core update error with its × or `overlay.close {id:"core-update-error"}`;
it stays dismissed until the error changes or the update recovers.

## Show something when asked

New spaces and tabs are shown immediately: only create them when asked.
To start one the person will look at later without moving them there, pass
`--focus false`:

```sh
herdr-app do space.new --name eng-123 --folder ~/code/app/eng-123 --argv '["claude"]' --task "Implement ENG-123" --focus false
```

Pick an explicit, useful name for every new space and tab; don't rely on
default names. Use IDs from `ls` rather than guessing or acting on the wrong
focused pane.

```sh
herdr-app do space.new --name api --folder /path/to/api
herdr-app do tab.new --name "fix login" --argv '["claude"]' --task "Look at the login bug"
herdr-app do pane.split --direction right --pane p12
herdr-app do agent.focus --terminal t7
```

For a remote space:

```sh
herdr-app do space.new --name "remote api" --machine vps --folder /srv/api
```

The folder is on that machine; tabs and splits in the space stay there.
New-space keys and clicks follow the focused space's machine. Script commands
without `--machine` still use this computer; `--machine ''` explicitly chooses it.
The agent menu checks and starts agents in the remote Unix user's
login/interactive shell (sh, bash, zsh, dash, ksh or fish), including its
configured PATH. Other shells keep the sh launcher.
`terminal.open --folder /path --place down` starts the split in that folder,
including when `--argv` supplies a program. Without `--folder`, it follows the
focused terminal's folder. If an inherited local folder was deleted, new spaces,
tabs and splits retry in that machine's home. An explicit invalid folder
still fails.
`hc machine ls` lists machines. Use `hc run --on vps ...`, `hc exec --on vps ...`
and `hc file ... --on vps ...` for remote work, not your local file tools.

`do` accepts `--key value` arguments (JSON for arrays/objects) or one JSON
object. Unknown argument names fail and list valid names. Target IDs (`id`,
`pane`, `space`, `target`, `terminal`) must be strings; other non-null types fail before
the current-target default can apply. Null keeps its omitted-argument meaning.
Use `commands` to confirm the command's exact shape:

```sh
herdr-app do pane.split '{"direction":"right","pane":"p12"}'
```

`pane.swap {pane?, direction | target}` trades the places of `pane` (default:
the focused one) and its neighbor in a direction, or `target`, another pane of
the same tab. Give `direction` or `target`, not both. Focus stays with the same pane:

```sh
herdr-app do pane.swap '{"pane":"p12","target":"p14"}'
```

`terminal.select_all {terminal?}` selects a terminal's whole screen and scrollback,
ready for `terminal.copy`. Without a target it follows keyboard focus. In vim and
other alternate-screen apps, it selects only that screen. Cmd+A runs it on macOS;
Linux and Windows have no default, so Ctrl+A stays with the shell.

`terminal.select {terminal?, at?: {row, col} | "cursor", mode?: "cell" | "word" | "line"}`
replaces the selection, at the program cursor by default. Cells are zero-based; rows
count from the oldest retained history through the active screen. Word and line
match double/triple click. `terminal.adjust_selection {terminal?, direction}` moves
the inclusive moving end and follows it in the view: left, right, up, down, page_up,
page_down, home, end, line_start, line_end. It fails without a selection.
`terminal.clear_selection {terminal?}` clears it; `terminal.copy` copies the focused terminal's selection.
These add no default keys or built-in copy mode. Mods read the inclusive selection
endpoints from `focused().selection` or `terminals()`; null means none (or no
attached view). Use `selection.end` as a copy-mode cursor. Selection facts update
when the selection changes, without refreshing readers on ordinary terminal output.

For an unavailable remote tab or space, use `tab.close {id, local: true}` or
`space.close {id, local: true}` to close it only on this screen. This never
kills terminals or edits the remote layout, even if the machine reconnects
before the command runs. It returns `{ "ok": true, "local": true }` and stays
closed across reconnects and restarts. `local` must be a boolean and requires
a remote target. Ordinary connected close ends the remaining visible terminals;
already dismissed work stays running. Restore that work first to end it here.
`pane.close {pane, local: true}` supports only the last pane, cascading to
its tab/space; for other panes close the tab explicitly.
Keep the qualified tab/space ID from `ls` before closing. To restore it, run
`tab.select {id}` or `space.select {id}` after its remote layout is available;
restoration combines the last fetched authoritative hidden content with pending
local edits and does not force a remote read. Reconnect, attention, index and
next/previous selection never restore it automatically. Downgrading to an
older app may lose these private exclusions.
If a competing write removes an unacknowledged view from the authoritative
layout, its terminal keeps running and the private exclusion prevents silent
re-adoption. Saved-ID selection cannot recreate a view absent from that layout;
it can restore the view only once the authoritative layout contains it again.

`terminal.search` opens a find bar in the focused terminal. It searches that
terminal's screen and scrollback; Enter and Shift-Enter move between matches,
Escape closes the bar while its find field has focus, and each terminal keeps its own query (reopening selects it, so typing replaces it).
Clicking the terminal while the bar is open puts typing, including Escape, back in the program; the bar and its matches stay.

It prints the created IDs (for example `{ "pane": "...", "terminal": "..." }`)
or `{ "error": "..." }` and exits 1. Read the result, then inspect `ls` or
`status` to confirm. Commands invoked by an agent never open a missing-argument
dialog: supply all required arguments. A successful launch reply means the
terminal was created, not that its program started correctly. If it exits
with a nonzero code within two seconds, the window shows an error toast with
the exit code and last output lines.

A terminal started with hc opts into the window with `herdr.space=<id>`.
`herdr.tab=<id>` chooses its tab; without it, Herdr creates a tab in that space.
Unlabeled workers stay hidden. Do not add labels just to run a background
worker. Use app commands when the
user wants to see a new tab or split, hc for orchestration that need not change
the screen.

## Customize layouts, keys, or mods

First read `~/.config/herdr-app/AGENTS.md` (or `$HERDR_APP_HOME/AGENTS.md` if
set). It is the installed guide to settings, layouts, commands and mods.
Read the generated `.d.ts` files beside it for full API details; do not edit
those files or the generated guide. The app rewrites them on start.

- `terminal_line_height` (default 1, 0.8..2) adjusts terminal rows;
  `terminal_cell_width` (default 1, 0.8..2) adjusts terminal columns;
  `terminal_scroll_speed` (default 1, 0.1..5) adjusts wheel and trackpad
  scrolling, including mouse reports. Set them in settings.json or the
  layout's look; the layout wins.
- Home defaults to `~/.config/herdr-app`; `HERDR_APP_HOME` selects another.
- `worktrees.directory` in settings.json sets the shipped New worktree destination,
  e.g. `{"worktrees":{"directory":"~/Developer/worktrees"}}`. Default:
  `~/.herdr/worktrees`; checkouts use `<directory>/<repo>/<branch>` (slashes in
  branches become dashes). Use an absolute path or `~` / `~/...`, resolved on the
  space's machine. Read on dialog open; existing checkouts stay put. No clones
  needed, and the app does not read the TUI's config.toml.
- Mods live in `mods/<name>/` with `main.js` and optional `mod.json`.
  They are trusted JavaScript with command and network access.
  `"focusable": true` in `mod.json` lets `pane.go` move onto a copy; it receives
  `"focus"` from `nextCommand()` and focuses its own keyboard handle.
- A mod's `watchFolder(dir)` promise has `cancel()`; cancel it when the folder
  is no longer shown. It resolves with `[]` and releases only that call.
- Shipped mods belong to Herdr: duplicate with `mod.clone` before editing.
  Shipped layouts likewise use `layout.clone` for a personal copy.
  Discover required arguments with `herdr-app commands`.
- Save to reload. A broken save keeps the old version running and reports
  an error in `status`; a still-working screen is not proof the edit worked.
- Make only the requested change. Don't touch unrelated mods or settings.
- Use theme colors. Do not retain a mod's `cx` across callbacks; each
  callback receives its own. Native animations, not redraw timers, keep
  the window cheap to run. Root `on_size_change((size, cx) => …)` reports
  logical pixels after layout, including resizes without pointer movement.
  Optional `View.close(cx)` gets one bounded final call; start cleanup there,
  not in an earlier task. The installed guide explains its 250 ms quit budget
  and which cleanup commands go out before shutdown.

After every mod change:

```sh
herdr-app check ~/.config/herdr-app/mods/<name>
herdr-app status
herdr-app shot /tmp/mod.png   # then look at the picture
```

`check` loads the mod and renders it once. It exits nonzero with the exact
error. Its shared state starts empty and stays in memory; it leaves shared.json
untouched. Its `init()` still runs: import `checking` from `"herdr"` and guard
process side effects with `if (checking()) return;`. It is true throughout a
check and false in a live window; `exec`, `hc` and `writeTextFile` still run
and warn during checks. Never call a mod done without a passing check.
Then confirm it is shown correctly in the running window with `shot`; don't claim a visual
check you haven't made.

`herdr-app shot [file.png]` saves the window's current frame and prints its
absolute path (default: `herdr-shot.png` in your current folder). It is the
app's own rendering at full pixel size: terminals, images, overlays and
floats included, even when the window is covered or minimized. No screen
capture or permissions. It needs a running window that has drawn a frame;
`HERDR_APP_HOME` selects the window, as for `status`.

## Problems and safety

If Herdr misbehaves, `herdr-app feedback "<what happened>"` drafts a report.
The person reviews and sends it; it does not silently send feedback.

Never experiment with pointer/key automation on the person's real display.
When developing the app on Linux, its `scripts/sandbox` provides an isolated
home, core and display: `up`, `do`, `status`, `shot`, then `down` when done.
Never stop a server or session you didn't start.
