# VS Code inside a Herdr tab (Windows)

The `vscode` mod shows the person's own VS Code window in a tab. Herdr asks
VS Code for a new window on a folder and adopts it: the same profile, settings,
extensions and sign-in as any other VS Code window.

```sh
herdr-app do tab.new '{"name":"VS Code","mod":"vscode","props":{"uri":"vscode-remote://wsl+Ubuntu/root/app"}}'
```

`props.uri` is a VS Code folder URI; `props.folder` a local folder. Without
either, the tab opens the focused terminal's folder. The tab records itself in
`shared("vscode.tabs")` by URI, so a launcher can select an open project's tab
instead of opening another.

## How it works (`src/browser/embed.rs`)

- **Window.** Herdr runs `Code.exe --new-window` on the target and adopts the
  new top-level window of VS Code's process. When VS Code already shows that
  folder, it is adopted instead, found as the foreground window or by its title.
  A pane whose mod restarts takes the window over from its earlier copy.
- **Container.** The window becomes a borderless child of a Herdr-owned container
  window. The container follows the pane and cuts away only the window frame
  outside its client area; everything VS Code draws stays. A move, resize or
  minimize from inside is undone within a quarter second.
- **Overlays.** Toasts, popups and dialogs mark their area with
  `browser::cover()`. Native views (VS Code and the browser) cut those areas
  out of themselves with a window region and stay visible around them.
- **Keyboard focus.** The Herdr window is subclassed: a click on an adopted
  window gives it focus, a click on Herdr's own surface takes it back, and
  reactivating Herdr returns focus to the adopted window that had it. The
  `focus` mod command gives a shown VS Code tab the keyboard.
- **Closing.** Closing the tab gives the window back to the desktop and closes it
  as its own close button would; unsaved changes prompt there. Quitting Herdr
  closes adopted windows the same way. VS Code itself keeps running.
- Native calls run outside App updates and without borrowed state: SetFocus and
  SetWindowPos deliver messages to Herdr's own window at once.

## Mod hints

`"frameless": true` in `mod.json` drops the layout's pane header for that mod.

## Known limits

- Herdr shortcuts do not reach Herdr while VS Code has keyboard focus.
- Not yet handled: drag and drop between Herdr and VS Code, several Herdr
  windows, DPI changes while moving between monitors.
- Linux and macOS report that embedding is unsupported.
