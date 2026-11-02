$ErrorActionPreference = "Stop"
. "$PSScriptRoot\install-windows.ps1"

function Assert($Condition, $Message) {
    if (-not $Condition) { throw $Message }
}

$tmp = Join-Path ([IO.Path]::GetTempPath()) "herdr-install-test-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    $binary = Join-Path $tmp "build.exe"
    $icon = Join-Path $tmp "icon.svg"
    $destination = Join-Path $tmp "Programs\herdr-app"
    Set-Content -LiteralPath $binary -Value "first build"
    Set-Content -LiteralPath $icon -Value "icon"
    Install-HerdrAppFiles $binary $icon $destination
    Assert ((Get-Content "$destination\herdr-app.exe") -eq "first build") "First install lost the binary"
    Assert ((Get-Content "$destination\herdr-app.svg") -eq "icon") "First install lost the icon"
    Assert (Test-Path "$destination\windows-explorer.ps1") "Installer omitted the Explorer helper"

    $payload = Join-Path $tmp 'shell'
    New-Item -ItemType Directory -Path $payload | Out-Null
    foreach ($name in @('herdr-explorer.dll', 'Herdr.Explorer.msix', 'logo.png', 'version.txt')) {
        Set-Content -LiteralPath (Join-Path $payload $name) -Value $name
    }
    Install-HerdrAppFiles $binary $icon $destination $payload
    Assert ((Get-Content "$destination\shell\herdr-explorer.dll") -eq 'herdr-explorer.dll') "Installer omitted the Explorer payload"

    # A shell host can keep this sibling DLL loaded across app replacements.
    $extension = Join-Path $tmp 'Programs\herdr-app-shell\1.0.0.0'
    New-Item -ItemType Directory -Path $extension -Force | Out-Null
    $extensionDll = Join-Path $extension 'herdr-explorer.dll'
    Set-Content -LiteralPath $extensionDll -Value 'loaded extension'
    $loaded = [IO.File]::Open($extensionDll, 'Open', 'Read', 'Read')

    Set-Content -LiteralPath $binary -Value "new build"
    Install-HerdrAppFiles $binary $icon $destination
    Assert ((Get-Content "$destination\herdr-app.exe") -eq "new build") "Replacement kept the old binary"
    Assert ((Get-ChildItem (Split-Path $destination)).Count -eq 2) "Replacement left temporary files"
    Assert ((Get-Content $extensionDll) -eq 'loaded extension') "App replacement modified the loaded shell extension"
    $loaded.Dispose()

    # Fail the promotion, then the rollback too, using real moves otherwise.
    function Rename-Item {
        param($LiteralPath, $NewName)
        $script:moves++
        if ($script:failMoves -contains $script:moves) { throw "injected move failure $script:moves" }
        Microsoft.PowerShell.Management\Rename-Item -LiteralPath $LiteralPath -NewName $NewName
    }
    foreach ($failures in @(@(2), @(2, 3))) {
        $script:moves = 0
        $script:failMoves = $failures
        $errorText = $null
        try { Install-HerdrAppFiles $binary $icon $destination }
        catch { $errorText = $_.ToString() }
        Assert ($null -ne $errorText) "Failed installation reported success"
        if ($failures.Count -eq 1) {
            Assert ((Get-Content "$destination\herdr-app.exe") -eq "new build") "Rollback lost the previous build"
        } else {
            $backup = Get-ChildItem (Split-Path $destination) -Directory -Filter "herdr-app.old-*"
            Assert ($backup.Count -eq 1) "Failed rollback lost the backup"
            Assert ($errorText.Contains($backup.FullName)) "Failed rollback did not report the backup path"
            Assert ((Get-Content "$($backup.FullName)\herdr-app.exe") -eq "new build") "Failed rollback damaged the backup"
        }
        Assert (-not (Get-ChildItem (Split-Path $destination) -Directory -Filter "herdr-app.install-*")) "Failed install left staging files"
    }
    Write-Host "Installer file replacement and rollback checks passed."
} finally {
    if ($loaded) { $loaded.Dispose() }
    Remove-Item Function:Rename-Item -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $tmp -Recurse -Force
}
