# Removes what scripts/install-windows.ps1 added for the current Windows user.
param([switch]$Purge)

# Keeps the PATH segments the installer would not have matched. Returns $null
# when nothing is left, which tells the caller to delete the value.
function Remove-PathEntry {
    param([string]$Raw, [string]$Entry)
    $rest = @($Raw -split ";" | Where-Object { $_.Trim().TrimEnd("\") -ine $Entry.TrimEnd("\") })
    if ($rest.Count -eq 0) {
        return $null
    }
    return ($rest -join ";")
}

# Removes the install folder and the herdr-app.old-* backups the installer
# keeps while an old Herdr still runs. Returns the paths it removed.
function Remove-HerdrAppFiles {
    param([string]$Destination)
    $Destination = [IO.Path]::GetFullPath($Destination)
    $removed = @()
    if (Test-Path -LiteralPath $Destination) {
        Remove-Item -LiteralPath $Destination -Recurse -Force
        $removed += $Destination
    }
    $parent = Split-Path $Destination
    if (Test-Path -LiteralPath $parent) {
        Get-ChildItem -LiteralPath $parent -Directory -Filter "herdr-app.old-*" | ForEach-Object {
            Remove-Item -LiteralPath $_.FullName -Recurse -Force
            $removed += $_.FullName
        }
    }
    return $removed
}

# Dot-sourcing exposes only the file operations for isolated uninstaller tests.
if ($MyInvocation.InvocationName -eq '.') { return }
$ErrorActionPreference = "Stop"
if ($env:OS -ne "Windows_NT") { throw "This uninstaller requires Windows." }

if (Get-Process -Name herdr-app -ErrorAction SilentlyContinue) {
    throw "Herdr is still running. Quit it, then re-run this uninstaller."
}

$dir = Join-Path $env:LOCALAPPDATA "Programs\herdr-app"
$menu = Join-Path ([Environment]::GetFolderPath("Programs")) "Herdr.lnk"
$config = $env:HERDR_APP_HOME
if ([string]::IsNullOrEmpty($config)) {
    $config = $env:XDG_CONFIG_HOME
    if ([string]::IsNullOrEmpty($config)) {
        $config = Join-Path $env:USERPROFILE ".config"
    }
    $config = Join-Path $config "herdr-app"
}

$key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Environment")
try {
    $raw = [string]$key.GetValue("Path", "", [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
} finally {
    $key.Close()
}
$listed = @($raw -split ";" | Where-Object { $_.Trim().TrimEnd("\") -ieq $dir.TrimEnd("\") })
$stale = @()
$parent = Split-Path $dir
if (Test-Path -LiteralPath $parent) {
    $stale = Get-ChildItem -LiteralPath $parent -Directory -Filter "herdr-app.old-*" | ForEach-Object { $_.FullName }
}

$removals = @()
if (Test-Path -LiteralPath $dir) { $removals += "Install folder: $dir" }
if (Test-Path -LiteralPath $menu) { $removals += "Start menu shortcut: $menu" }
if ($listed.Count -gt 0) { $removals += "Path entry: $dir" }
$stale | ForEach-Object { $removals += "Leftover backup: $_" }
if ($Purge -and (Test-Path -LiteralPath $config)) { $removals += "App data, including layouts and mods: $config" }

if ($removals.Count -eq 0) {
    Write-Host "Nothing to remove: the Herdr install is already gone."
    exit 0
}

Write-Host "This will remove:"
$removals | ForEach-Object { Write-Host "  $_" }
if ($Purge) { Write-Host "  --purge deletes the app data; it cannot be restored." }
$answer = Read-Host "Remove all of it (y/N)"
if ($answer -ine "y") {
    Write-Host "Aborted, nothing was removed."
    exit 1
}

if ($listed.Count -gt 0) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Environment", $true)
    try {
        # Re-read after the prompt so a concurrent Path edit is not lost.
        $raw = [string]$key.GetValue("Path", "", [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        $listed = @($raw -split ";" | Where-Object { $_.Trim().TrimEnd("\") -ieq $dir.TrimEnd("\") })
        if ($listed.Count -gt 0) {
            # Keep the literal %VARIABLES% and the existing registry value type in PATH.
            $kind = if ($key.GetValueNames() -contains "Path") { $key.GetValueKind("Path") } else { [Microsoft.Win32.RegistryValueKind]::ExpandString }
            $new = Remove-PathEntry $raw $dir
            if ($null -eq $new) {
                $key.DeleteValue("Path")
            } else {
                $key.SetValue("Path", $new, $kind)
            }
            # Broadcast the change so new consoles see the updated Path.
            [Environment]::SetEnvironmentVariable("HERDR_INSTALLING", "1", "User")
            [Environment]::SetEnvironmentVariable("HERDR_INSTALLING", $null, "User")
        }
    } finally {
        $key.Close()
    }
}
if (Test-Path -LiteralPath $menu) {
    $link = (New-Object -ComObject WScript.Shell).CreateShortcut($menu)
    if ($link.TargetPath -ieq (Join-Path $dir "herdr-app.exe")) {
        Remove-Item -LiteralPath $menu
        Write-Host "Removed $menu"
    } else {
        Write-Warning "$menu points to $($link.TargetPath), not this install. Left in place."
    }
}
Remove-HerdrAppFiles $dir | ForEach-Object { Write-Host "Removed $_" }
if ($Purge -and (Test-Path -LiteralPath $config)) {
    Remove-Item -LiteralPath $config -Recurse -Force
    Write-Host "Removed $config"
}
Write-Host "Uninstalled herdr-app. Open a new terminal for the Path change to apply."
