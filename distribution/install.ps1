# Installs the Herdr app on Windows, and herdr-core if there's none yet:
#
#   powershell -c "irm https://releases.herdr.dev/app/install.ps1 | iex"
#
# The app into %LOCALAPPDATA%\Programs\herdr-app (added to your PATH for
# `herdr-app`), Herdr in the Start menu. The core, with its own installer,
# only if none is there yet; the app keeps it current after that. Running it
# again updates the app (an open one keeps running until it quits). No admin
# needed. HERDR_RELEASES_URL points elsewhere (tests).

# Its own scope: under `irm | iex` it would otherwise set the caller's variables.
& {
$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

# throw, not exit: under `irm | iex`, exit would close the person's window.
function Fail($why) { throw "herdr: $why" }

$releases = if ($env:HERDR_RELEASES_URL) { $env:HERDR_RELEASES_URL.TrimEnd("/") } else { "https://releases.herdr.dev" }
$dir = Join-Path $env:LOCALAPPDATA "Programs\herdr-app"
$arch = if ($env:PROCESSOR_ARCHITEW6432) { $env:PROCESSOR_ARCHITEW6432 } else { $env:PROCESSOR_ARCHITECTURE }
if ($arch -ne "AMD64") { Fail "no build for $arch Windows yet (x64 only)" }
$target = "windows-x86_64"
# Windows' own tar: Git Bash's, often first on PATH, takes C:\ for a remote host.
$tar = Join-Path $env:SystemRoot "System32\tar.exe"
if (-not (Get-Command curl.exe -ErrorAction SilentlyContinue) -or -not (Test-Path $tar)) {
    Fail "needs curl.exe and tar.exe (Windows 10 1803 or newer)"
}

$tmp = Join-Path ([IO.Path]::GetTempPath()) "herdr-install-$PID"
New-Item -ItemType Directory -Force $tmp | Out-Null
try {
    curl.exe -fsSL "$releases/app/latest.json" -o "$tmp\latest.json"
    if ($LASTEXITCODE) { Fail "cannot reach $releases" }
    $m = Get-Content -Raw "$tmp\latest.json" | ConvertFrom-Json
    $t = $m.targets.$target
    if (-not $t.url -or -not $t.sha256) { Fail "no build for $target yet" }

    # Tells the release server's download counter a new install from a
    # reinstall; the file is the same either way. Only plain URLs on the
    # release server in use are marked: one with a query or fragment (a
    # mirror's signed URL) is fetched exactly as the manifest gives it.
    $src = if (Test-Path $dir) { "reinstall" } else { "install" }
    $plain = $t.url.StartsWith("$releases/", [StringComparison]::Ordinal) -and $t.url.IndexOfAny([char[]]"?#") -lt 0
    $download = if ($plain) { "$($t.url)?src=$src" } else { $t.url }

    Write-Host "Installing Herdr $($m.version) for $target..."
    curl.exe -fsSL $download -o "$tmp\app.tar.gz"
    if ($LASTEXITCODE) { Fail "download failed: $($t.url)" }
    # .NET, not Get-FileHash: that one fails when PowerShell 7 starts 5.1.
    $stream = [IO.File]::OpenRead("$tmp\app.tar.gz")
    try { $got = -join ([Security.Cryptography.SHA256]::Create().ComputeHash($stream) | ForEach-Object { $_.ToString("x2") }) } finally { $stream.Close() }
    if ($got -ne $t.sha256) { Fail "the download is damaged (checksum mismatch); try again" }
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
    $raw = [string]$key.GetValue("Path", "", [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    $listed = $raw -split ";" | Where-Object { $_.Trim().TrimEnd("\") -ieq $dir.TrimEnd("\") }
    if (-not $listed) {
        $kind = if ($key.GetValueNames() -contains "Path") { $key.GetValueKind("Path") } else { [Microsoft.Win32.RegistryValueKind]::ExpandString }
        $key.SetValue("Path", $(if ($raw) { "$($raw.TrimEnd(';'));$dir" } else { $dir }), $kind)
        # Setting any user variable tells running programs the environment changed.
        [Environment]::SetEnvironmentVariable("HERDR_INSTALLING", "1", "User")
        [Environment]::SetEnvironmentVariable("HERDR_INSTALLING", $null, "User")
    }
    $key.Close()
    if (($env:Path -split ";") -notcontains $dir) { $env:Path += ";$dir" }

    # The core: installed only if there's none yet. One that's already there
    # is left alone; the app updates it.
    $core = Join-Path $env:LOCALAPPDATA "Programs\herdr-core\herdr-core.exe"
    if (-not (Test-Path $core) -and -not (Get-Command herdr-core -ErrorAction SilentlyContinue)) {
        try {
            $env:HERDR_RELEASES_URL = $releases
            Invoke-RestMethod "$releases/core/install.ps1" | Invoke-Expression
        } catch {
            Write-Host "herdr-core didn't install; Herdr will offer it when it opens."
        }
    }

    Write-Host "Herdr is installed. Open it from the Start menu, or run: herdr-app"
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
}
