# Builds an unsigned DLL and sparse package; signing and registration belong to the installer/release.
param(
    [string]$Publisher = $env:HERDR_WINDOWS_PUBLISHER,
    [string]$Version,
    [string]$OutputDirectory,
    [switch]$PackageOnly,
    [switch]$SkipTests
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $root 'target\windows-shell' }
$OutputDirectory = [IO.Path]::GetFullPath($OutputDirectory)
if (-not $Version) {
    $cargo = Get-Content -Raw (Join-Path $root 'Cargo.toml')
    $Version = [regex]::Match($cargo, '(?m)^version = "(\d+\.\d+\.\d+)"').Groups[1].Value + '.0'
}
if ($Version -notmatch '^\d+\.\d+\.\d+\.\d+$') { throw 'Version must have four numeric components.' }
if (-not $Publisher) {
    $Publisher = 'CN=Herdr Development'
    Write-Warning 'Building unsigned development artifacts. Production requires HERDR_WINDOWS_PUBLISHER and a matching trusted signature.'
}
New-Item -ItemType Directory -Force $OutputDirectory | Out-Null
$sdk = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
$makeappx = Get-ChildItem $sdk -Directory | Where-Object { $_.Name -match '^10\.' } |
    Sort-Object { [version]$_.Name } -Descending |
    ForEach-Object { Join-Path $_.FullName 'x64\makeappx.exe' } |
    Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
if (-not $makeappx) { throw 'Windows SDK makeappx.exe is required.' }
if (-not $PackageOnly) {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    $vs = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if (-not $vs) { throw 'Visual Studio C++ build tools are required.' }
    $devcmd = Join-Path $vs 'Common7\Tools\VsDevCmd.bat'
    # A temporary batch keeps MSVC's environment local to this build process.
    $batch = Join-Path $OutputDirectory 'build.cmd'
    $source = Join-Path $root 'windows\shell.cpp'
    $definition = Join-Path $root 'windows\shell.def'
    $testSource = Join-Path $root 'windows\shell.test.cpp'
    $lines = @('@echo off', "call `"$devcmd`" -no_logo -arch=x64 -host_arch=x64", 'if errorlevel 1 exit /b 1',
        "cl /nologo /std:c++17 /EHsc /W4 /WX /MT /utf-8 /DUNICODE /D_UNICODE /LD `"$source`" /Fe:herdr-explorer.dll /link /DEF:`"$definition`" ole32.lib shell32.lib shlwapi.lib uuid.lib", 'if errorlevel 1 exit /b 1')
    if (-not $SkipTests) {
        $lines += @("cl /nologo /std:c++17 /EHsc /W4 /WX /MT /utf-8 /DUNICODE /D_UNICODE `"$testSource`" /Fe:shell-test.exe /link ole32.lib shell32.lib shlwapi.lib uuid.lib", 'if errorlevel 1 exit /b 1', 'shell-test.exe', 'if errorlevel 1 exit /b 1')
    }
    [IO.File]::WriteAllLines($batch, $lines, [Text.UTF8Encoding]::new($false))
    Push-Location $OutputDirectory
    try {
        & cmd.exe /d /c $batch
        if ($LASTEXITCODE) { throw "Shell extension build/tests failed ($LASTEXITCODE)." }
    } finally { Pop-Location }
}
if (-not (Test-Path (Join-Path $OutputDirectory 'herdr-explorer.dll'))) { throw 'Build the DLL before packaging.' }
Copy-Item (Join-Path $root 'assets\icons\herdr-ui-icon-clean.png') (Join-Path $OutputDirectory 'logo.png') -Force
$manifest = (Get-Content -Raw (Join-Path $root 'windows\AppxManifest.xml.in')).Replace('@VERSION@', $Version).Replace('@PUBLISHER@', [Security.SecurityElement]::Escape($Publisher))
# Keep package inputs separate: DLL/resources are external and deployed by the installer.
$packageRoot = Join-Path $OutputDirectory 'identity'
New-Item -ItemType Directory -Force $packageRoot | Out-Null
[IO.File]::WriteAllText((Join-Path $packageRoot 'AppxManifest.xml'), $manifest, [Text.UTF8Encoding]::new($false))
Copy-Item (Join-Path $packageRoot 'AppxManifest.xml') (Join-Path $OutputDirectory 'AppxManifest.xml') -Force
[IO.File]::WriteAllText((Join-Path $OutputDirectory 'version.txt'), $Version, [Text.UTF8Encoding]::new($false))
& $makeappx pack /o /nv /d $packageRoot /p (Join-Path $OutputDirectory 'Herdr.Explorer.msix')
if ($LASTEXITCODE) { throw "Sparse package build failed ($LASTEXITCODE)." }
Write-Host "Unsigned shell artifacts: $OutputDirectory (version $Version, publisher $Publisher)"
