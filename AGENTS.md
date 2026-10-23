# Herdr app

The Herdr 2 desktop app (`herdr-app`): a native GPUI client of
[herdr-core](https://github.com/herdrdev/herdr-core). Can (`ogulcancelik`)
maintains it. His working notes live in `.brain/` (gitignored): if
`.brain/README.md` exists, read it first.

## Not released: no backward compatibility

Nobody depends on it yet. Until the first release, rename and reshape
freely: no aliases for old names, no migration code, no reading old labels,
files or formats "so existing setups keep working". Old-name aliases
already in the code can be deleted when you touch them. Remove this section
at release.

## Before you grow the app's core: ask Can

The app's core (the model, `spaces.json`, commands, what every mod gets)
stays small; everything else is mods. Before adding anything to it, answer
these three, then discuss with Can and wait for his go:

1. Does the app itself act on it (it changes what the app does, not only
   how something looks)?
2. Can only the app see it (e.g. a change that happens while no mod is
   watching)?
3. Can mods not do it correctly, even with care?

If all three are no, it belongs in mods (their shared storage, their own
code), even at the price of some repetition. Same for herdr-core: what the
app asks of it goes through Can too.

## The guide users' agents read

`defaults/AGENTS.md` is written into every user's `~/.config/herdr-app` on
every start: it's how their agents learn what the app can do. A change to
what users or agents can do (a command, a setting, a layout or mod
feature, a file) updates it in the same commit. `cargo test` checks that it
names every command and settings.json key; the rest is on you. Keep it a
map with rules and short examples: the `.d.ts` files are generated and are
the full API.

## What ships in the box

`defaults/` is built into the app: mods, `layouts/*.json`, `keys.json`
(Linux, Windows) and `keys.macos.json` (Mac). Mods update in the user's
`~/.config/herdr-app` on start if they didn't edit them (and go when no
longer shipped). Layouts update by merge and keep the user's edits
(`src/layout_sync.rs`). `keys.json` is installed only when missing, so a
changed default key never reaches existing users by itself.

## Building and testing

`just build`, `just test` (what CI runs, on Linux), `just check` (the tests).
`just install` uses the platform installer on Linux/macOS and
`cargo install` on Windows. Windows recipes use PowerShell by default;
`just --shell pwsh --shell-arg -Command <recipe>` uses pwsh.

Never run a bare `target/*/herdr-app` (no arguments) on a machine where you
also use the app without `HERDR_APP_HOME` pointing at a scratch home: it
opens a second window on your real setup. Subcommands and `--help` only
talk.

On Linux, test UI changes in the sandbox: `scripts/sandbox up`, then `do
<command> [json]`, `status`, `shot` (screenshot), `move|click|drag`
(pointer input, window px), `type`, `key <keysym> [Control_L ...]`, `hc
...`, `restart`, `down`. It has its own core, home and a headless display
(Weston + Xwayland) under `/var/tmp/herdr-app-sandbox`, so nothing pops up
on your screen. Never fake input on a real display. Run `scripts/sandbox
down` when done.

## Contributing

Work on a branch and open a pull request to `master`. CI must pass and Can
approves; merges are squashed. Commit messages: lowercase conventional
commits, single line, no AI co-author lines. Only Can releases (a `v*` tag).

## Only on Can's machines

Skip this section unless the checkout is Can's: `~/Projects/herdr-app` on
his Linux desktop or his Mac (`ssh mac`).

- Commit only when Can says go. Propose the message first.
- His terminals live in herdr-core, so quitting and reopening the app never
  kills them. Your own session usually runs inside his app. Never stop,
  signal or restart his app or his core (`hc server ...` without
  `HC_SESSION`). `herdr-app status` / `ls` against his window are fine.
- Can's Herdr is the main checkout's `target/release/herdr-app`
  (`~/.local/bin/herdr-app` and the GNOME app "Herdr", set up by
  `scripts/install-linux`). Never point that link anywhere else. When a
  change is ready, `cargo build --release` in the main checkout and tell
  him; he quits and reopens the app.
- Big work happens on a branch in a worktree
  (`~/Projects/herdr-app-worktrees/<name>`, its `target` a link into
  `/var/tmp`). Can tries a branch in **Herdr Dev**
  (`~/.local/bin/herdr-app-dev`, its own home `~/.config/herdr-app-dev` and
  core session `herdr-app-dev`): point its script at the branch's build.
  Never point it, or anything, at his `~/.config/herdr-app`. "Move my
  Herdr" means: bring the branch into the main checkout and build it there.
- When a change breaks his `~/.config/herdr-app` or his running terminals'
  labels, say so and fix his copy by hand with his OK. `keys.json` there is
  his: when you change a default key, ask whether to change his file too.
- On the Mac, run `scripts/install-macos` after syncing changes. It builds
  and installs a native executable inside `~/Applications/Herdr.app`, signs
  it locally and registers it; `~/.local/bin/herdr-app` points to it. Never
  replace it with a shell wrapper: macOS needs the bundle identity. The
  installer does not quit or reopen the app.
- Start sub-agents with herdr-core, not a harness's subagent tool (we
  dogfood `hc`): `hc run --bg --name <name> --cwd <dir> gpt-implementer
  "<task>"` (templates in `~/.config/herdr-core/templates`, `reviewer` for
  reviews), then `hc wait`, `hc read`, `hc send`.
