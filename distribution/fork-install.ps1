# Installs or updates the fork's Windows build from its latest GitHub release:
#
#   powershell -c "irm https://github.com/alexshpunt/herdr-app/releases/latest/download/install.ps1 | iex"
#
# The app goes into %LOCALAPPDATA%\Programs\herdr-app (added to your PATH for
# `herdr-app`), Herdr into the Start menu. herdr-core is installed from the
# upstream installer only if none is there yet. Running it again updates the
# app; an open one keeps running until it quits. No admin needed. The build
# does not update itself: run this again for a newer one. HERDR_FORK_RELEASE_URL
# points elsewhere (tests).

# Its own scope: under `irm | iex` it would otherwise set the caller's variables.
& {
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

# throw, not exit: under `irm | iex`, exit would close the person's window.
function Fail($why) { throw "herdr: $why" }

$base = if ($env:HERDR_FORK_RELEASE_URL) { $env:HERDR_FORK_RELEASE_URL.TrimEnd("/") } else { "https://github.com/alexshpunt/herdr-app/releases/latest/download" }
$dir = Join-Path $env:LOCALAPPDATA "Programs\herdr-app"
$arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
if ($arch -ne "AMD64") { Fail "no build for $arch Windows (x64 only)" }
$name = "herdr-app-windows-x86_64.tar.gz"
# Windows' own tar: Git Bash's, often first on PATH, takes C:\ for a remote host.
$tar = Join-Path $env:SystemRoot "System32\tar.exe"
if (-not (Get-Command curl.exe -ErrorAction SilentlyContinue) -or -not (Test-Path $tar)) {
    Fail "needs curl.exe and tar.exe (Windows 10 1803 or newer)"
}

$tmp = Join-Path ([IO.Path]::GetTempPath()) "herdr-install-$PID"
New-Item -ItemType Directory -Force $tmp | Out-Null
try {
    Write-Host "Downloading the latest Herdr fork build..."
    curl.exe -fsSL "$base/$name.sha256" -o "$tmp\app.sha256"
    if ($LASTEXITCODE) { Fail "cannot download $base/$name.sha256" }
    $want = ((Get-Content -Raw "$tmp\app.sha256") -split "\s+")[0].ToLower()
    curl.exe -fsSL "$base/$name" -o "$tmp\app.tar.gz"
    if ($LASTEXITCODE) { Fail "download failed: $base/$name" }
    # .NET, not Get-FileHash: that one fails when PowerShell 7 starts 5.1.
    $stream = [IO.File]::OpenRead("$tmp\app.tar.gz")
    try { $got = -join ([Security.Cryptography.SHA256]::Create().ComputeHash($stream) | ForEach-Object { $_.ToString("x2") }) } finally { $stream.Close() }
    if ($got -ne $want) { Fail "the download is damaged (checksum mismatch); try again" }
    & $tar -xzf "$tmp\app.tar.gz" -C $tmp
    if ($LASTEXITCODE -or -not (Test-Path "$tmp\herdr-app\herdr-app.exe")) { Fail "the download holds no complete app" }

    # A running app's folder can be renamed, not deleted: the old one goes
    # now if it can, else on the next install.
    New-Item -ItemType Directory -Force (Split-Path $dir) | Out-Null
    Remove-Item -Recurse -Force "$dir.old" -ErrorAction SilentlyContinue
    if (Test-Path $dir) { Rename-Item $dir "$dir.old" }
    Move-Item "$tmp\herdr-app" $dir
    Remove-Item -Recurse -Force "$dir.old" -ErrorAction SilentlyContinue

    $exe = Join-Path $dir "herdr-app.exe"
    $menu = Join-Path ([Environment]::GetFolderPath("Programs")) "Herdr.lnk"
    $link = (New-Object -ComObject WScript.Shell).CreateShortcut($menu)
    $link.TargetPath = $exe
    $link.WorkingDirectory = $env:USERPROFILE
    $link.Description = "Terminals and coding agents that keep running"
    $link.Save()

    # The raw user PATH, so %VARIABLES% in it stay unexpanded and keep their type.
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Environment", $true)
    try {
        $raw = [string]$key.GetValue("Path", "", [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
        $listed = $raw -split ";" | Where-Object { $_.Trim().TrimEnd("\") -ieq $dir.TrimEnd("\") }
        if (-not $listed) {
            $kind = if ($key.GetValueNames() -contains "Path") { $key.GetValueKind("Path") } else { [Microsoft.Win32.RegistryValueKind]::ExpandString }
            $key.SetValue("Path", $(if ($raw) { "$($raw.TrimEnd(';'));$dir" } else { $dir }), $kind)
            # Setting any user variable tells running programs the environment changed.
            [Environment]::SetEnvironmentVariable("HERDR_INSTALLING", "1", "User")
            [Environment]::SetEnvironmentVariable("HERDR_INSTALLING", $null, "User")
        }
    } finally {
        $key.Close()
    }
    if (($env:Path -split ";") -notcontains $dir) { $env:Path += ";$dir" }

    # The core is upstream's own release; installed only if there is none yet.
    # One that's already there is left alone; the app updates it.
    $core = Join-Path $env:LOCALAPPDATA "Programs\herdr-core\herdr-core.exe"
    if (-not (Test-Path $core) -and -not (Get-Command herdr-core -ErrorAction SilentlyContinue)) {
        try {
            Invoke-RestMethod "https://releases.herdr.dev/core/install.ps1" | Invoke-Expression
        } catch {
            Write-Host "herdr-core didn't install; Herdr will offer it when it opens."
        }
    }

    Write-Host "Herdr is installed. Quit and reopen it to use the new build, or run: herdr-app"
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
}
