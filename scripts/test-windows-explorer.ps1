# Standalone PowerShell 5.1 checks; no package/registry/certificate changes.
$ErrorActionPreference = 'Stop'
. "$PSScriptRoot\windows-explorer.ps1"
$realInstall = ${function:Install-HerdrExplorer}
$realRemove = ${function:Remove-HerdrExplorer}
$realSignature = ${function:Test-HerdrExplorerSignature}

function Assert($Condition, $Message) {
    if (-not $Condition) { throw $Message }
}

function Assert-Throws($Body, $Message) {
    $failed = $false
    try { & $Body } catch { $failed = $true }
    Assert $failed $Message
}

# Keep every filesystem operation in a private home, including the sibling DLL deployment.
$temporary = Join-Path ([IO.Path]::GetTempPath()) "herdr-explorer-test-$([guid]::NewGuid().ToString('N'))"
$app = Join-Path $temporary 'Programs\herdr-app'
$payload = Join-Path $app 'shell'
New-Item -ItemType Directory -Force $payload | Out-Null
foreach ($name in @('Herdr.Explorer.msix', 'herdr-explorer.dll', 'logo.png')) {
    [IO.File]::WriteAllText((Join-Path $payload $name), "fixture $name")
}
[IO.File]::WriteAllText((Join-Path $payload 'version.txt'), '1.0.0.0')
[IO.File]::WriteAllText((Join-Path $app 'herdr-app.exe'), 'fixture app')

# Exercise the real functions while substituting OS state and package deployment.
function Test-HerdrExplorerPlatform { return $script:supported }
function Get-HerdrExplorerChoice { return $script:choice }
function Set-HerdrExplorerChoice($Choice) { $script:choice = $Choice }
function Confirm-HerdrExplorer { $script:prompts++; return $script:answer }
function Get-HerdrExplorerPackage { if ($script:package) { return $script:package } }
function Get-HerdrExplorerExternalLocation($PackageFullName) { return $script:externalLocation }
function Get-AuthenticodeSignature($LiteralPath) { return @{ Status = $script:signatureStatus; SignerCertificate = @{ Subject = 'CN=Herdr Test' } } }
function Add-AppxPackage($Path, $ExternalLocation, $ErrorAction, [switch]$DeferRegistrationWhenPackagesAreInUse) {
    $script:registrations++
    Assert ($ExternalLocation -eq (Split-Path $app -Parent)) 'ExternalLocation must be the Programs parent'
    Assert ($Path -eq (Join-Path $payload 'Herdr.Explorer.msix')) 'Registration must use the validated payload'
    Assert $DeferRegistrationWhenPackagesAreInUse 'Updates must defer registration instead of stopping a shell host'
    if ($script:deploymentFailure) { throw 'injected deployment failure' }
    if ($script:deferDeployment) { return }
    $script:package = @{ Version = '1.0.0.0'; Status = 'Ok'; PackageFullName = 'Herdr.Explorer.test'; Publisher = 'CN=Herdr Test' }
    $script:externalLocation = $ExternalLocation
}
function Remove-AppxPackage($Package, $ErrorAction) {
    $script:unregistrations++
    Assert ($Package -eq 'Herdr.Explorer.test') 'Unregister must target only the Herdr package'
    if ($script:removeFailure) { throw 'injected unregister failure' }
    $script:package = $null
}

try {
$script:signatureStatus = 'NotSigned'
Assert-Throws { & $realSignature 'unused' } 'Unsigned artifacts must be rejected'
$script:signatureStatus = 'Valid'
& $realSignature 'unused'
$script:signatureCalls = @()
function Test-HerdrExplorerSignature($Path) {
    $script:signatureCalls += $Path
    if ($script:signatureFailure) { throw 'injected untrusted signature' }
}
$script:registrations = 0
$script:unregistrations = 0
$script:package = $null
& $realInstall $app
$deployed = Join-Path (Split-Path $app -Parent) 'herdr-app-shell\1.0.0.0\herdr-explorer.dll'
Assert ((Get-FileHash -LiteralPath $deployed).Hash -eq (Get-FileHash -LiteralPath (Join-Path $payload 'herdr-explorer.dll')).Hash) 'Deployment must preserve DLL content'
Assert ($script:registrations -eq 1 -and $script:signatureCalls.Count -eq 3) 'Installation must validate app, DLL and package before registering'
$loaded = [IO.File]::Open($deployed, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
try {
    & $realInstall $app
    Assert ($script:registrations -eq 1) 'An unchanged loaded DLL/package must not be overwritten or registered again'
} finally { $loaded.Dispose() }
[IO.File]::WriteAllText((Join-Path $payload 'herdr-explorer.dll'), 'changed without version bump')
Assert-Throws { & $realInstall $app } 'Changed content at the same version must be rejected'
Assert ($script:registrations -eq 1) 'Rejected content must not change package registration'
[IO.File]::WriteAllText((Join-Path $payload 'herdr-explorer.dll'), 'fixture herdr-explorer.dll')
$script:signatureFailure = $true
Assert-Throws { & $realInstall $app } 'A signature failure must stop installation'
Assert ($script:registrations -eq 1) 'Untrusted payload must not register'
$script:signatureFailure = $false
$script:package = $null
$script:deploymentFailure = $true
Assert-Throws { & $realInstall $app } 'Windows deployment errors must reach the caller'
$script:deploymentFailure = $false
$script:package = @{ Version = '2.0.0.0'; Status = 'Ok'; PackageFullName = 'Herdr.Explorer.test'; Publisher = 'CN=Herdr Test' }
Assert-Throws { & $realInstall $app } 'Package downgrade must be explicit'
$script:package = @{ Version = '1.0.0.0'; Status = 'Ok'; PackageFullName = 'Herdr.Explorer.test'; Publisher = 'CN=Other' }
Assert-Throws { & $realInstall $app } 'A different signing publisher must require explicit removal'
$script:package.Publisher = 'CN=Herdr Test'
$script:externalLocation = Join-Path $temporary 'different-programs'
Assert-Throws { & $realInstall $app } 'The same version registered at a different location must require explicit removal'
$script:externalLocation = Split-Path $app -Parent
$outputs = @(& $realInstall $app)
Assert ($outputs.Count -eq 0) 'Successful installation must not leak helper outputs'
$oldFolder = Join-Path (Split-Path $app -Parent) 'herdr-app-shell\0.9.0.0'
New-Item -ItemType Directory -Force $oldFolder | Out-Null
$oldDll = Join-Path $oldFolder 'herdr-explorer.dll'
[IO.File]::WriteAllText($oldDll, 'old loaded extension')
$script:package.Version = '0.9.0.0'
$script:deferDeployment = $true
$outputs = @(& $realInstall $app)
Assert ($outputs.Count -eq 0) 'Deferred installation must not leak helper outputs'
Assert ($script:package.Version -eq '0.9.0.0' -and (Test-Path -LiteralPath $oldDll)) 'Deferred registration must keep the old registered extension files'
$script:deferDeployment = $false
# Windows completes the pending registration once the host releases the package.
$script:package.Version = '1.0.0.0'
$loaded = [IO.File]::Open($oldDll, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
try {
    & $realInstall $app
    Assert (Test-Path -LiteralPath $oldDll) 'A loaded old DLL must survive best-effort cleanup'
} finally { $loaded.Dispose() }
& $realInstall $app
Assert (-not (Test-Path -LiteralPath $oldFolder)) 'Unlocked old extension files must be cleaned up on later repair'
& $realRemove
Assert ($script:unregistrations -eq 1 -and $null -eq $script:package) 'Remove must unregister its package'
[IO.File]::WriteAllText((Join-Path $payload 'version.txt'), '..\escape')
Assert-Throws { & $realInstall $app } 'Malformed version must not escape the deployment directory'
[IO.File]::WriteAllText((Join-Path $payload 'version.txt'), '1.0.0.0')

# The remaining cases exercise consent/update policy without filesystem effects.
function Install-HerdrExplorer($InstallRoot) {
    $script:installs++
    if ($script:installFailure) { throw 'injected registration failure' }
}
function Remove-HerdrExplorer {
    $script:removals++
    if ($script:removeFailure) { throw 'injected unregister failure' }
}

function Reset-Scenario($Choice, $Consent) {
    $script:choice = $Choice
    $script:answer = if ($Consent) { 6 } else { 7 }
    $script:supported = $true
    $script:prompts = 0
    $script:installs = 0
    $script:removals = 0
    $script:installFailure = $false
    $script:removeFailure = $false
}

Reset-Scenario $null $false
Invoke-HerdrExplorer Offer $app
Assert ($script:choice -eq 'disabled' -and $script:prompts -eq 1 -and $script:installs -eq 0) 'Declining the offer must persist without installation'
Invoke-HerdrExplorer Offer $app
Invoke-HerdrExplorer Repair $app
Assert ($script:prompts -eq 1 -and $script:installs -eq 0) 'Install/update must preserve a previous decline'

Reset-Scenario $null $true
Invoke-HerdrExplorer Offer $app
Assert ($script:choice -eq 'enabled' -and $script:prompts -eq 1 -and $script:installs -eq 1) 'Accepting the offer must install and remember consent'
Invoke-HerdrExplorer Repair $app
Assert ($script:prompts -eq 1 -and $script:installs -eq 2) 'Repair must preserve consent without another prompt'
Invoke-HerdrExplorer Disable $app
Assert ($script:choice -eq 'disabled' -and $script:removals -eq 1) 'Disable must unregister and preserve the decision'

Reset-Scenario 'disabled' $false
Invoke-HerdrExplorer Enable $app
Assert ($script:choice -eq 'enabled' -and $script:installs -eq 1) 'Explicit enable must override a previous decline'
$script:installFailure = $true
Assert-Throws { Invoke-HerdrExplorer Repair $app } 'Registration failure must reach the caller'
Assert ($script:choice -eq 'enabled') 'A failed repair must not forget consent'
$script:removeFailure = $true
Assert-Throws { Invoke-HerdrExplorer Disable $app } 'Unregister failure must reach the caller'
Assert ($script:choice -eq 'disabled') 'A failed unregister must still preserve opt-out'

Reset-Scenario $null $true
$script:answer = -1
Invoke-HerdrExplorer Offer $app
Assert ($null -eq $script:choice -and $script:installs -eq 0) 'Offer timeout must leave the choice unset'
Reset-Scenario $null $true
$script:signatureFailure = $true
Invoke-HerdrExplorer Offer $app -WarningAction SilentlyContinue
Assert ($script:prompts -eq 0 -and $null -eq $script:choice -and $script:installs -eq 0) 'Unsigned builds must not offer installation'
$script:signatureFailure = $false
Reset-Scenario 'enabled' $false
$script:signatureFailure = $true
Invoke-HerdrExplorer Offer $app
Assert ($script:choice -eq 'enabled' -and $script:prompts -eq 0 -and $script:installs -eq 0) 'An unsigned source build must preserve consent without failed repair or another popup'
$script:signatureFailure = $false
Remove-Item -LiteralPath (Join-Path $payload 'Herdr.Explorer.msix')
Invoke-HerdrExplorer Offer $app
Assert ($script:choice -eq 'enabled' -and $script:installs -eq 0) 'A build without integration assets must preserve consent without repair'
[IO.File]::WriteAllText((Join-Path $payload 'Herdr.Explorer.msix'), 'fixture Herdr.Explorer.msix')
Reset-Scenario $null $true
$script:supported = $false
Invoke-HerdrExplorer Offer $app
Assert ($script:prompts -eq 0 -and $script:installs -eq 0) 'Unsupported Windows must not offer modern integration'
Assert-Throws { Invoke-HerdrExplorer Enable $app } 'Explicit enable must explain unsupported Windows'

$mutex = New-Object PSObject
$mutex | Add-Member ScriptMethod WaitOne {
    param($Timeout)
    $script:mutexTimeout = $Timeout
    return $script:mutexAvailable
}
$script:mutexAvailable = $false
Assert (-not (Enter-HerdrExplorerMutex $mutex Offer)) 'A busy background offer may be skipped'
Assert ($script:mutexTimeout -eq 0) 'Background offers must not wait for another setup'
Assert-Throws { Enter-HerdrExplorerMutex $mutex Disable } 'An explicit action must report that setup is busy'
Assert ($script:mutexTimeout -eq 15000) 'Explicit actions must wait for a bounded interval'
$script:mutexAvailable = $true
Assert (Enter-HerdrExplorerMutex $mutex Enable) 'Explicit setup must continue when the mutex becomes available'

Write-Host 'Explorer deployment, signatures, consent and update checks passed.'
} finally {
    Remove-Item -LiteralPath $temporary -Recurse -Force
}
