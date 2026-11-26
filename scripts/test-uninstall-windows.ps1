$ErrorActionPreference = "Stop"
. "$PSScriptRoot\uninstall-windows.ps1"

function Assert($Condition, $Message) {
    if (-not $Condition) { throw $Message }
}

# Remove-PathEntry undoes exactly what the installer's Path check matches.
Assert ((Remove-PathEntry "a;b;c" "b") -eq "a;c") "Middle entry left the wrong Path"
Assert ((Remove-PathEntry "b;c" "b") -eq "c") "First entry left the wrong Path"
Assert ((Remove-PathEntry "a;b" "a") -eq "b") "Last entry left the wrong Path"
Assert ((Remove-PathEntry "a" "a") -eq $null) "Only entry should clear the value"
Assert ((Remove-PathEntry "a;b" "z") -eq "a;b") "Unrelated Path was modified"
Assert ((Remove-PathEntry "C:\Tools" "c:\tools") -eq $null) "Comparison must be case-insensitive"
Assert ((Remove-PathEntry "a;b\" "b") -eq "a") "Trailing backslash must match"
Assert ((Remove-PathEntry "a;b;" "b") -eq "a;") "Other segments must survive verbatim"
Assert ((Remove-PathEntry "" "b") -eq "") "Empty Path must stay empty"

$tmp = Join-Path ([IO.Path]::GetTempPath()) "herdr-uninstall-test-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path "$tmp\Programs\herdr-app" | Out-Null
try {
    Set-Content -LiteralPath "$tmp\Programs\herdr-app\herdr-app.exe" -Value "binary"
    New-Item -ItemType Directory -Path "$tmp\Programs\herdr-app.old-deadbeef" | Out-Null
    Set-Content -LiteralPath "$tmp\Programs\herdr-app.old-deadbeef\herdr-app.exe" -Value "old build"

    $removed = @(Remove-HerdrAppFiles "$tmp\Programs\herdr-app")
    Assert (-not (Test-Path "$tmp\Programs\herdr-app")) "Install folder survived"
    Assert (-not (Test-Path "$tmp\Programs\herdr-app.old-deadbeef")) "Leftover backup survived"
    Assert ($removed.Count -eq 2) "Expected two removals, got $($removed.Count)"
    Assert ((Get-ChildItem $tmp\Programs).Count -eq 0) "Removal left files behind"

    # A missing install is a no-op, but orphaned backups still get cleaned.
    New-Item -ItemType Directory -Path "$tmp\Programs\herdr-app.old-cafe" | Out-Null
    $stale = @(Remove-HerdrAppFiles "$tmp\Programs\herdr-app")
    Assert ($stale.Count -eq 1) "Orphaned backup was not cleaned"
    Assert (-not (Test-Path "$tmp\Programs\herdr-app.old-cafe")) "Orphaned backup survived"
    $nothing = @(Remove-HerdrAppFiles "$tmp\Programs\herdr-app")
    Assert ($nothing.Count -eq 0) "Clean state reported removals"
    Write-Host "Uninstaller Path entry and file removal checks passed."
} finally {
    Remove-Item -LiteralPath $tmp -Recurse -Force
}
