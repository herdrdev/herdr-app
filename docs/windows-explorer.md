# Open folders in Herdr

Run `herdr-app open "D:\Project\my app"` to start or raise Herdr and open a new
local space in that folder. The terminal uses herdr-core's shell configuration
in `config.toml`, including Git Bash. No app shell or machine setting is needed.
It opens locally even when a remote space is selected. Each invocation creates
a new space; this is a visible action, not a way to run background work.

## Windows 11 Explorer

Signed Windows releases can add **Open in Herdr** to File Explorer's main
context menu when you right-click a folder or a folder background. Installation
or the first launch of a new release offers to enable it. Declining is remembered.
The app remains usable if you decline or Windows blocks package registration.

To change the choice later, run the installed helper in PowerShell:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$env:LOCALAPPDATA\Programs\herdr-app\windows-explorer.ps1" -Action Enable
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$env:LOCALAPPDATA\Programs\herdr-app\windows-explorer.ps1" -Action Disable
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$env:LOCALAPPDATA\Programs\herdr-app\windows-explorer.ps1" -Action Repair
```

`Enable` opts in, `Disable` removes the integration, and `Repair` restores an
enabled integration's registration. The choice is stored in
`HKCU\Software\Herdr\Explorer`; updates preserve a refusal. A signed package
trusted by Windows is required. Source builds
without the signed package do not provide this menu action. Windows 10 can run
Herdr and use the CLI; this integration targets the Windows 11 main menu.
Virtual folders and unsupported selections do not offer the action.

## Verification

Test with an isolated app home and core session, keeping the person's app and
core running. Check cold and warm launches, a selected remote space still
opening locally, and the shell selected by the core. Include paths with spaces,
Unicode, ampersands and a drive root. Invalid folders must report an error.
Opening waits up to 30 seconds for the local layout. After a missing or uncertain
reply, inspect the window before retrying: a terminal may already have started.

Check both folder and background menus on an interactive Windows 11 desktop.
Verify enabling, declining, disabling, repairing and an app update while Explorer
has loaded the extension. Compilation and headless CI cannot establish that the
action appears correctly in the main menu. Do not stop or restart Explorer as
part of testing.

[Back to Herdr](../README.md)
