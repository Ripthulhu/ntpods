<#
    ZIP installer for Windows 11 22H2 or newer. Uses the same setup functions as
    NTPods.msi: sign and install the AAP, ACX microphone and playback drivers;
    copy the apps to Program Files; register helper tasks and startup entries.
    The distribution includes all inputs; development tools are not required.

    Use a test PC. Before running as administrator:
      1. Save the BitLocker recovery key.
      2. Disable Secure Boot if it blocks test signing.
      3. Run bcdedit /set testsigning on, then reboot.

    Usage: powershell -ExecutionPolicy Bypass -File .\install.ps1
    -Yes skips the BitLocker recovery-key confirmation for unattended installs.
#>
param([switch]$Yes, [ValidateSet('auto', 'acx')][string]$MicDriver = 'acx')
$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
. (Join-Path $here 'setup-common.ps1')
$dest = Join-Path $env:ProgramFiles 'NTPods'
$sid = Get-CurrentUserSid

# Validate before changing certificates or drivers.
Write-Warning 'NTPods requires Test Mode, which weakens Windows driver-signature checks. These experimental drivers are not Microsoft production-signed. Do not enable Test Mode on your everyday or work PC for NTPods; use a dedicated test PC.'
if (-not (Test-Admin)) { throw 'Run this from an ADMINISTRATOR PowerShell.' }
Assert-UserSid $sid
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

$required = @(
    'driver\NTPodsAAP.sys', 'driver\ntpodsaap.cat', 'driver\NTPodsAAP.inf',
    'driver-mic-acx\AudioCodec.sys', 'driver-mic-acx\audiocodec.cat', 'driver-mic-acx\AudioCodec.inf',
    'driver-audio\NTPodsAudio.sys', 'driver-audio\ntpodsaudio.cat', 'driver-audio\NTPodsAudio.inf',
    'tools\devcon.exe', 'tools\audio-binding.exe', 'audio-driver.ps1', 'ntpodsd.exe', 'avcodec-61.dll', 'avutil-59.dll', 'swresample-5.dll',
    'ntpods-aac-encoder.exe', 'ntpods-eld-encoder.exe', 'fdk-aac.dll',
    'winui\ntpods-winui.exe', 'fix-driver.ps1', 'rename-mic.ps1', 'bluetooth-host-identity.ps1'
) | ForEach-Object { Join-Path $here $_ }
$missing = @($required | Where-Object { -not (Test-Path $_) })
if ($missing) {
    throw "This install folder is incomplete. Download the release again. Missing:`n  " + ($missing -join "`n  ")
}

# Validate protected destinations before driver changes.
New-Item -ItemType Directory -Force -Path $dest | Out-Null
Assert-ProtectedInstallPath $dest
Stop-NTPods
$setupDir = Initialize-SetupDirectory
Remove-SetupLeftovers $env:LOCALAPPDATA

$work = Join-Path $setupDir "drivers-$([guid]::NewGuid())"
$transactionId = [guid]::NewGuid().ToString()
try {
try { Install-NTPodsDrivers $here $work $MicDriver $sid $transactionId }
finally {
    if ([IO.Path]::GetFullPath($work).StartsWith($setupDir + '\', [StringComparison]::OrdinalIgnoreCase)) {
        Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Write-Host "==> Copying the apps to $dest"
Assert-ProtectedInstallPath $dest
foreach ($f in 'ntpodsd.exe', 'avcodec-61.dll', 'avutil-59.dll', 'swresample-5.dll', 'ntpods-aac-encoder.exe', 'ntpods-eld-encoder.exe', 'fdk-aac.dll', 'fix-driver.ps1', 'rename-mic.ps1', 'setup-common.ps1', 'audio-driver.ps1', 'restore-playback.ps1', 'stop-ntpods.ps1', 'msi-setup.ps1', 'uninstall.ps1', 'bluetooth-host-identity.ps1') {
    Copy-Item (Join-Path $here $f) $dest -Force
}
# The WinUI app ships as a self-contained folder.
Copy-Item (Join-Path $here 'winui') $dest -Recurse -Force
Copy-Item (Join-Path $here 'tools') $dest -Recurse -Force
Copy-Item (Join-Path $here 'licenses') $dest -Recurse -Force
# Files from a downloaded zip carry the internet zone mark; clear it on the
# installed copy so startup and the scheduled tasks don't get blocked or prompted.
Get-ChildItem $dest -Recurse -File | Unblock-File

Register-NTPodsTasks $dest $sid
New-Item -Path 'HKLM:\SOFTWARE\NTPods' -Force | Out-Null
Set-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\NTPods' -Name InstalledUserSid -Value $sid
Set-ItemProperty -LiteralPath 'HKLM:\SOFTWARE\NTPods' -Name InstallKind -Value 'zip'

# The daemon needs the user's audio session. These Run values also back the
# app's Start with Windows setting; --tray keeps the window hidden at login.
Write-Host '==> Adding the daemon + WinUI app to startup...'
$startup = Get-StartupDir $env:APPDATA
foreach ($l in 'NTPods Daemon.lnk', 'NTPods.lnk') {   # older installs used shortcuts
    Remove-Item (Join-Path $startup $l) -Force -ErrorAction SilentlyContinue
}
Set-RunValues $sid (Join-Path $dest 'ntpodsd.exe') (Join-Path $dest 'winui\ntpods-winui.exe')
& (Join-Path $dest 'bluetooth-host-identity.ps1') -Action Install -TransactionId $transactionId
} catch {
    $failure = $_
    try { & (Join-Path $here 'bluetooth-host-identity.ps1') -Action Rollback -TransactionId $transactionId }
    catch { Write-Warning "Bluetooth identity recovery failed: $_ The original backup was retained." }
    try { Restore-NTPodsDriverCheckpoint $here $transactionId }
    catch { throw "Installation failed: $failure Driver recovery failed: $_ Backup retained at $(Get-DriverCheckpointPath)." }
    throw $failure
}
try { & (Join-Path $dest 'bluetooth-host-identity.ps1') -Action Commit -TransactionId $transactionId }
catch { Write-Warning "Installation completed, but Bluetooth identity transaction cleanup failed: $_" }
try { Complete-NTPodsDriverTransaction $transactionId }
catch { Write-Warning "Installation completed, but backup cleanup failed at $(Get-DriverCheckpointPath). Remaining backup files may be incomplete. $_" }

Write-Host ''
Write-Host '==> Done. A reboot is needed to finish the driver install.' -ForegroundColor Green
Write-Host '    After reboot, connect your AirPods — the WinUI app (tray) shows battery'
Write-Host '    + Noise Control, and the NTPods microphone appears in Sound > Input'
Write-Host '    (and in Discord etc.), renamed to your AirPods once they connect.'
