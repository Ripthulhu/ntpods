<#
    ZIP installer for Windows 11 22H2 or newer. Uses the same setup functions as
    NTPods.msi: copy the apps to Program Files and register startup entries.
    USB dongles use Windows' built-in drivers. -Drivers additionally signs and
    installs the direct Bluetooth drivers and configures the PC's Apple identity.
    The distribution includes all inputs; development tools are not required.

    For -Drivers, use a test PC configured beforehand:
      1. Save the BitLocker recovery key.
      2. Disable Secure Boot if it blocks test signing.
      3. Run bcdedit /set testsigning on, then reboot.

    Usage: powershell -ExecutionPolicy Bypass -File .\install.ps1
    -Drivers opts into direct Bluetooth support. Existing drivers are otherwise left alone.
    -Yes skips the driver installation's BitLocker recovery-key confirmation.
#>
param([switch]$Drivers, [switch]$Yes, [ValidateSet('auto', 'acx')][string]$MicDriver = 'acx')
$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
. (Join-Path $here 'setup-common.ps1')
. (Join-Path $here 'app-transaction.ps1')
$dest = Join-Path $env:ProgramFiles 'NTPods'
$sid = Get-CurrentUserSid

# Validate before changing certificates or drivers.
if (-not (Test-Admin)) { throw 'Run this from an ADMINISTRATOR PowerShell.' }
Assert-UserSid $sid
$existingInstall = Get-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\NTPods' -ErrorAction SilentlyContinue
if ($existingInstall.InstallKind -eq 'msi') { throw 'Update an MSI installation with the MSI package, not the ZIP installer.' }
$ownsDrivers = $Drivers -or (Test-NTPodsZipDriverOwnership $existingInstall)
if ($Drivers) {
Write-Warning 'Direct Bluetooth drivers require Test Mode, which weakens Windows driver-signature checks. These experimental drivers are not Microsoft production-signed. Use a dedicated test PC. USB dongles do not require Test Mode.'
$MicDriver = Resolve-MicDriver $MicDriver
$bitlocker = Test-BitLocker
if (-not (Test-TestMode)) {
    $secureBoot = switch (Test-SecureBoot) {
        $true { 'Secure Boot is ON on this PC, so turn it off first.' }
        $false { 'Secure Boot is already off.' }
        default { "Couldn't read the Secure Boot state." }
    }
    $bl = if ($bitlocker) { "BitLocker is ON for $env:SystemDrive. $script:BitLockerAdvice" } else { 'BitLocker is off for the system drive.' }
    throw @"
Test Mode is not active, so Windows would refuse to load the NTPods drivers.
  1. $bl
  2. Disable Secure Boot in your firmware/BIOS (while it is on, bcdedit refuses).
     $secureBoot
  3. In an admin PowerShell:  bcdedit /set testsigning on
  4. Reboot ("Test Mode" shows in the bottom-right corner), then run this again.
"@
}
# Later changes to Test Mode or Secure Boot can also trigger BitLocker recovery.
if ($bitlocker) {
    Write-Host "BitLocker is ON for $env:SystemDrive.`n$script:BitLockerAdvice" -ForegroundColor Yellow
    if (-not $Yes) {
        $a = Read-Host 'Is your BitLocker recovery key saved somewhere other than this PC? (y/N)'
        if ($a -notmatch '^(y|yes|s|sim)$') { throw 'Stopped: save the BitLocker recovery key first, then run this again.' }
    }
}
}

$driverFiles = @(
    'driver\NTPodsAAP.sys', 'driver\ntpodsaap.cat', 'driver\NTPodsAAP.inf',
    'driver-mic-acx\AudioCodec.sys', 'driver-mic-acx\audiocodec.cat', 'driver-mic-acx\AudioCodec.inf',
    'driver-audio\NTPodsAudio.sys', 'driver-audio\ntpodsaudio.cat', 'driver-audio\NTPodsAudio.inf',
    'tools\devcon.exe', 'tools\audio-binding.exe'
)
$required = @(
    'audio-driver.ps1', 'ntpodsd.exe', 'avcodec-61.dll', 'avutil-59.dll', 'swresample-5.dll',
    'ntpods-aac-encoder.exe', 'ntpods-eld-encoder.exe', 'ntpods-fdk-q31.dll',
    'winui\ntpods-winui.exe', 'fix-driver.ps1', 'rename-mic.ps1', 'bluetooth-host-identity.ps1'
)
if ($Drivers) { $required += $driverFiles }
$required = $required | ForEach-Object { Join-Path $here $_ }
$missing = @($required | Where-Object { -not (Test-Path $_) })
if ($missing) {
    throw "This install folder is incomplete. Download the release again. Missing:`n  " + ($missing -join "`n  ")
}

# Validate protected destinations before driver changes.
if (Test-Path -LiteralPath $dest) { Assert-ProtectedInstallPath $dest }
Stop-NTPods
$setupDir = Initialize-SetupDirectory
Remove-SetupLeftovers $env:LOCALAPPDATA

$work = Join-Path $setupDir "drivers-$([guid]::NewGuid())"
$transactionId = [guid]::NewGuid().ToString()
try {
Prepare-NTPodsAppTransaction $here $transactionId
if ($Drivers) {
try { Install-NTPodsDrivers $here $work $MicDriver $sid $transactionId }
finally {
    if ([IO.Path]::GetFullPath($work).StartsWith($setupDir + '\', [StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }
}
}

Write-Host "==> Installing the apps in $dest"
Install-NTPodsAppTransaction $transactionId

if ($Drivers) { Register-NTPodsTasks $dest $sid }
New-Item -Path 'HKLM:\SOFTWARE\NTPods' -Force | Out-Null
Set-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\NTPods' -Name InstalledUserSid -Value $sid
Set-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\NTPods' -Name InstallKind -Value 'zip'
Set-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\NTPods' -Name DirectBluetoothDrivers -Value ([int]$ownsDrivers) -Type DWord

# The daemon needs the user's audio session. These Run values also back the
# app's Start with Windows setting; --tray keeps the window hidden at login.
Write-Host '==> Adding the daemon + WinUI app to startup...'
$startup = Get-StartupDir $env:APPDATA
foreach ($l in 'NTPods Daemon.lnk', 'NTPods.lnk') {   # older installs used shortcuts
    Remove-Item (Join-Path $startup $l) -Force -ErrorAction SilentlyContinue
}
Set-RunValues $sid (Join-Path $dest 'ntpodsd.exe') (Join-Path $dest 'winui\ntpods-winui.exe')
if ($Drivers) { & (Join-Path $dest 'bluetooth-host-identity.ps1') -Action Install -TransactionId $transactionId }
} catch {
    $failure = $_
    $recoveryErrors = @()
    if ($Drivers) {
    try { & (Join-Path $here 'bluetooth-host-identity.ps1') -Action Rollback -TransactionId $transactionId }
    catch { Write-Warning "Bluetooth identity recovery failed: $_ The original backup was retained." }
    try { Restore-NTPodsDriverCheckpoint $here $transactionId }
    catch { $recoveryErrors += "Driver recovery failed: $_ Backup retained at $(Get-DriverCheckpointPath)." }
    }
    try { Restore-NTPodsAppTransaction $transactionId }
    catch { $recoveryErrors += "App recovery failed: $_ Backup retained at $(Get-NTPodsAppTransactionPath)." }
    if ($recoveryErrors.Count) { throw "Installation failed: $failure $($recoveryErrors -join ' ')" }
    throw $failure
}
if ($Drivers) {
try { & (Join-Path $dest 'bluetooth-host-identity.ps1') -Action Commit -TransactionId $transactionId }
catch { Write-Warning "Installation completed, but Bluetooth identity transaction cleanup failed: $_" }
try { Complete-NTPodsDriverTransaction $transactionId }
catch { Write-Warning "Installation completed, but backup cleanup failed at $(Get-DriverCheckpointPath). Remaining backup files may be incomplete. $_" }
}
try { Complete-NTPodsAppTransaction $transactionId }
catch { Write-Warning "Installation completed, but app backup cleanup failed at $(Get-NTPodsAppTransactionPath). $_" }

Write-Host ''
if ($Drivers) {
    Write-Host '==> Done. Restart Windows to finish the direct Bluetooth driver install.' -ForegroundColor Green
} else {
    Write-Host '==> Done. The app is installed; existing drivers and Bluetooth identity were left unchanged.' -ForegroundColor Green
}
