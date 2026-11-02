# Shipped beside herdr-app.exe. Dot-source only to test the functions in isolation.
param(
    [ValidateSet('Offer', 'Enable', 'Disable', 'Repair')][string]$Action = 'Offer',
    [string]$InstallRoot = $PSScriptRoot
)

function Test-HerdrExplorerPlatform {
    return [Environment]::OSVersion.Version.Build -ge 22000
}

function Get-HerdrExplorerChoice {
    $key = 'HKCU:\Software\Herdr\Explorer'
    if (Test-Path $key) { return (Get-ItemProperty -LiteralPath $key -Name Choice -ErrorAction SilentlyContinue).Choice }
}

function Set-HerdrExplorerChoice($Choice) {
    $key = 'HKCU:\Software\Herdr\Explorer'
    New-Item -Path $key -Force | Out-Null
    Set-ItemProperty -LiteralPath $key -Name Choice -Value $Choice
}

function Get-HerdrExplorerPackage {
    Get-AppxPackage -Name Herdr.Explorer -ErrorAction Stop
}

function Get-HerdrExplorerExternalLocation($PackageFullName) {
    $manager = [Windows.Management.Deployment.PackageManager, Windows.Management.Deployment, ContentType = WindowsRuntime]::new()
    $package = $manager.FindPackageForUser('', $PackageFullName)
    return $package.EffectiveExternalLocation.Path
}

function Test-HerdrExplorerSignature($Path) {
    $signature = Get-AuthenticodeSignature -LiteralPath $Path
    if ($signature.Status -ne 'Valid') {
        throw "Explorer integration requires a trusted signed release ($Path`: $($signature.Status))."
    }
}

function Confirm-HerdrExplorer {
    $text = 'Add Open in Herdr to the File Explorer folder context menu?'
    # Yes/No, question icon; a timeout leaves the choice unset.
    return (New-Object -ComObject WScript.Shell).Popup($text, 60, 'Herdr', 36)
}

function Install-HerdrExplorer($InstallRoot) {
    $InstallRoot = [IO.Path]::GetFullPath($InstallRoot).TrimEnd('\')
    if ((Split-Path $InstallRoot -Leaf) -ne 'herdr-app') { throw 'Explorer integration requires an installation directory named herdr-app.' }
    $payload = Join-Path $InstallRoot 'shell'
    $versionText = (Get-Content -Raw -LiteralPath (Join-Path $payload 'version.txt')).Trim()
    if ($versionText -notmatch '^\d+\.\d+\.\d+\.\d+$') { throw 'Invalid Explorer package version.' }
    $version = [version]$versionText
    $packagePath = Join-Path $payload 'Herdr.Explorer.msix'
    $dll = Join-Path $payload 'herdr-explorer.dll'
    Test-HerdrExplorerSignature $packagePath
    Test-HerdrExplorerSignature $dll
    Test-HerdrExplorerSignature (Join-Path $InstallRoot 'herdr-app.exe')
    $publisher = (Get-AuthenticodeSignature -LiteralPath $packagePath).SignerCertificate.Subject
    if (-not $publisher) { throw 'Cannot determine the Explorer package signing publisher.' }
    $programs = Split-Path $InstallRoot -Parent
    $extensions = Join-Path $programs 'herdr-app-shell'
    $destination = Join-Path $extensions $versionText
    New-Item -ItemType Directory -Path $destination -Force | Out-Null
    foreach ($name in @('herdr-explorer.dll', 'logo.png')) {
        $source = Join-Path $payload $name
        $target = Join-Path $destination $name
        if (Test-Path -LiteralPath $target) {
            if ((Get-FileHash -LiteralPath $source).Hash -ne (Get-FileHash -LiteralPath $target).Hash) {
                throw "Explorer files changed without a package version bump ($versionText)."
            }
        } else { Copy-Item -LiteralPath $source -Destination $target }
    }
    $installed = @(Get-HerdrExplorerPackage)
    if ($installed.Count -gt 1) { throw 'Multiple Herdr Explorer package identities are registered. Disable the integration before reinstalling.' }
    if ($installed.Count -eq 1) {
        if ($installed[0].Publisher -ne $publisher) {
            throw 'A Herdr Explorer package from a different publisher is registered. Disable the integration, then enable it again.'
        }
        try { $external = Get-HerdrExplorerExternalLocation $installed[0].PackageFullName }
        catch { throw "Cannot verify the registered Explorer location. Disable the integration, then enable it again. $_" }
        if (-not $external -or [IO.Path]::GetFullPath($external).TrimEnd('\') -ne $programs) {
            throw 'Herdr Explorer is registered for a different installation location. Disable the integration, then enable it again.'
        }
    }
    if ($installed.Count -eq 1 -and [version]$installed[0].Version -gt $version) {
        throw 'A newer Herdr Explorer package is registered. Disable it before installing an older version.'
    }
    # The app's stable install path does not change across updates. Re-registering
    # the same version is unnecessary and may fail while a shell host uses it.
    if ($installed.Count -eq 0 -or [version]$installed[0].Version -ne $version -or $installed[0].Status -ne 'Ok') {
        Add-AppxPackage -Path $packagePath -ExternalLocation $programs -DeferRegistrationWhenPackagesAreInUse -ErrorAction Stop
        $installed = @(Get-HerdrExplorerPackage)
    }
    # A deferred registration still needs its old external files until Windows switches it.
    if ($installed.Count -ne 1 -or [version]$installed[0].Version -ne $version -or $installed[0].Status -ne 'Ok') { return }
    # Loaded DLLs can remain until a later update; never stop their shell host.
    foreach ($old in Get-ChildItem -LiteralPath $extensions -Directory) {
        if ($old.Name -notmatch '^\d+\.\d+\.\d+\.\d+$' -or [version]$old.Name -ge $version) { continue }
        foreach ($name in @('herdr-explorer.dll', 'logo.png')) {
            Remove-Item -LiteralPath (Join-Path $old.FullName $name) -Force -ErrorAction SilentlyContinue
        }
        if (@(Get-ChildItem -LiteralPath $old.FullName -Force).Count -eq 0) {
            Remove-Item -LiteralPath $old.FullName -ErrorAction SilentlyContinue
        }
    }
}

function Remove-HerdrExplorer {
    foreach ($package in @(Get-HerdrExplorerPackage)) {
        Remove-AppxPackage -Package $package.PackageFullName -ErrorAction Stop
    }
}

function Invoke-HerdrExplorer($Action, $InstallRoot) {
    if ($Action -eq 'Disable') {
        # Preserve the opt-out even if Windows cannot unregister immediately.
        Set-HerdrExplorerChoice 'disabled'
        Remove-HerdrExplorer
        return
    }
    if (-not (Test-HerdrExplorerPlatform)) {
        if ($Action -eq 'Offer') { return }
        throw 'The modern Explorer command requires Windows 11.'
    }
    $choice = Get-HerdrExplorerChoice
    if ($Action -eq 'Offer') {
        if ($choice -eq 'disabled') { return }
        $packagePath = Join-Path $InstallRoot 'shell\Herdr.Explorer.msix'
        if (-not (Test-Path -LiteralPath $packagePath)) { return }
        # Source builds may replace a signed release while preserving its choice.
        try { Test-HerdrExplorerSignature $packagePath } catch { Write-Warning $_; return }
        if ($choice -ne 'enabled') {
            $answer = Confirm-HerdrExplorer
            if ($answer -eq 7) { Set-HerdrExplorerChoice 'disabled'; return }
            if ($answer -ne 6) { return }
        }
    } elseif ($Action -eq 'Repair' -and $choice -ne 'enabled') { return }
    Install-HerdrExplorer $InstallRoot
    Set-HerdrExplorerChoice 'enabled'
    Write-Host 'Open in Herdr is enabled. Windows may need a sign-out before the menu refreshes.'
}

function Enter-HerdrExplorerMutex($Mutex, $Action) {
    $timeout = if ($Action -eq 'Offer') { 0 } else { 15000 }
    try { $entered = $Mutex.WaitOne($timeout) }
    catch [Threading.AbandonedMutexException] { return $true }
    if (-not $entered -and $Action -ne 'Offer') {
        throw 'Another Herdr Explorer setup is running. Wait for it to finish, then retry this action.'
    }
    return $entered
}

if ($MyInvocation.InvocationName -eq '.') { return }
$ErrorActionPreference = 'Stop'
$mutex = $null
$locked = $false
try {
    $sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
    $mutex = New-Object Threading.Mutex($false, "Local\Herdr.Explorer.$sid")
    $locked = Enter-HerdrExplorerMutex $mutex $Action
    if (-not $locked) { return }
    Invoke-HerdrExplorer $Action $InstallRoot
} catch {
    Write-Warning "Herdr Explorer integration: $_"
    if ($Action -eq 'Offer' -and [Environment]::UserInteractive) {
        (New-Object -ComObject WScript.Shell).Popup("Could not enable Open in Herdr. Your app and terminals are unchanged.`n`n$_", 30, 'Herdr', 48) | Out-Null
    }
    exit 1
} finally {
    if ($locked) { $mutex.ReleaseMutex() }
    if ($mutex) { $mutex.Dispose() }
}
