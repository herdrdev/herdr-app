# Herdr changelog

## [0.1.9] - 2026-10-30

### Fixed
- On macOS, updates keep Herdr's Apple signature again, so macOS stops forgetting the permissions you gave Herdr.
- Herdr no longer uses CPU in the background when the Changes or Files panel is open in a git repo.
- Mods use less CPU while they wait for work.

## [0.1.8] - 2026-10-30

### New
- **Option works as Alt on macOS.** Claude Code's Option shortcuts work, automatically on US keyboards. Change it with `option_as_alt` in settings.json.
- **Programs can copy to your clipboard,** also from other machines. Pi's or Claude Code's own mouse selection lands on your clipboard.
- **Your own commands in the + menu.** Entries in programs.json show up, shell functions included.

### Fixed
- Accent keys work on macOS and with ibus on Linux, and Caps Lock types capitals on macOS.
- @, [ and friends type on Slovenian, German and other Mac layouts in every program.
- Terminal colors show exactly as programs set them, so powerline prompts look right.
- Double-click selects a word, triple-click a line.
- Agents start from the + menu under fish, and one that fails at launch says why.
- On macOS, the window buttons stay clear of a narrow sidebar and the tabs.
- You can type a commit message in the Changes panel.
- Spaces named with emoji or accents work everywhere.
- The font list in Settings scrolls on its own.
- A downloaded update installs at the next start, even after a crash.

## [0.1.7] - 2026-10-29

### Fixed
- Herdr no longer freezes when a terminal shows an older Nerd Font icon it has no picture for, like some powerlevel10k battery icons. It draws them as text instead.

## [0.1.6] - 2026-10-29

### New
- **Coming from herdr?** The welcome says what carries over and gives you one line for your agent to bring your keybinds and settings across. Open it again with Show the welcome again in the command palette.
- **Updates at a glance.** Settings → Updates shows the app and the core on one row each, with What's new a click away.
- **herdr-core updates with Herdr.** After Herdr updates, its core moves to the latest version too. Your terminals keep running.
- **Screenshots in feedback.** Drop images straight onto the feedback form.

### Fixed
- Paste and copy in a peek go to the terminal you're peeking at, not the agent behind it.
- Scroll back through thousands of lines instead of a few hundred.
- Mod data, like topics and colors, can't get lost if Herdr closes in the middle of saving.
- Commands with an argument they don't know say so instead of acting on the current terminal.
- On Windows, file links at the end of a sentence open the right file.

## [0.1.5] - 2026-10-27
<!-- spotlight -->

### New
- **Drag panes where you want them.** Hover the top of a pane and grab the dots. Drop it on another pane to swap them, or near an edge to put it beside. A highlight shows where it lands.
  ![Dragging a pane beside another](https://media.herdr.dev/0.1.5/pane-drag.webp)
- **Ctrl-click a terminal to peek at it.** When an agent mentions another terminal, like t2, Ctrl-click (Cmd on macOS) its address to see it over your tab. Go to jumps there.
  ![Peeking at a terminal an agent mentioned](https://media.herdr.dev/0.1.5/peek.webp)
- **Agents can see the window.** `herdr-app shot` saves a picture of Herdr as it looks right now, so an agent building a mod can check its work. `herdr-app --skill` and `hc --skill` teach an agent the rest.
- **What changed, after every update.** Release notes like these, in Settings → Updates, including versions you skipped.

### Fixed
- nvim and other full-screen programs redraw right after you resize their pane.
- On Windows, AltGr and accent keys type the right characters, and Shift+Enter adds a new line in Claude Code.
- On Windows, new terminals start in PowerShell 7 when you have it, and Herdr follows their folder.
- On macOS, Herdr keeps the permissions you gave it across updates.
- Short menus open next to the pointer instead of near the top of the window.
- Removing a worktree asks once, then removes it.
- Mods can fetch from the web, know their size, and clean up when they close.

## [0.1.4] - 2026-10-23
<!-- spotlight -->

### New
- **See the agents that need you first.** Pick Priority beside the Agents title: agents waiting on you, then the ones that just finished, then the ones still working. Grouped by space is still there.
- **Pin the ones you're watching.** Hover an agent and click the pin. Pinned agents stay on top in both views until they end.
- **Fold what you don't need.** Click a space or section header to fold it; the header still shows how many agents are inside.

### Fixed
- Temporary agent sandboxes stay out of your spaces and machine lists.
- The agents list keeps its place when you scroll.
- An agent you're in moves to its new section right away.
- A pane closes when its terminal finishes instead of showing “has finished”.
- On macOS, the red close button hides Herdr instead of leaving it without a window.
- Mods reload cleanly when you save them, and tell you when a save didn't load.
