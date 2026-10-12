# Playback driver test

Use a Windows 11 x64 test machine with a working NTPods installation and paired
AirPods. Test Mode weakens driver-signature checks; do not enable it on a work or
everyday PC for this experiment. This procedure does not change boot settings,
startup entries, installed app files or Alternative A2DP's driver package.

The commands below reuse the normal installer's driver transaction. They replace
both NTPods drivers, keep the original packages for rollback and sign the test
packages on the test machine. Do not copy a private signing key between PCs.

## Get the test build

Download `NTPods-Native-AAC.zip` from
[releases](https://github.com/Ripthulhu/ntpods/releases) and extract it to a fresh
folder on the test machine. The normal MSI and `NTPods-Windows.zip` do not include
this feature. Keep the test folder intact for rollback.

To build your own bundle, use the [build prerequisites](https://github.com/Ripthulhu/ntpods/blob/main/docs/BUILD-WINDOWS.md).
From the repository root, at a clean commit:

```powershell
pwsh -File windows/playback/build.ps1
pwsh -File windows/playback/package.ps1
```

The package contains the backend, unsigned drivers and catalogs, installer
helpers, notices and file checksums. Do not mix files from different builds.

## Read-only preflight on the test machine

Open administrator PowerShell using the same account that normally runs NTPods.
From the copied bundle:

```powershell
.\preflight.ps1 -Bundle .
```

If PowerShell blocks the downloaded scripts, run
`Set-ExecutionPolicy -Scope Process Bypass` in that window and retry. This applies
only until that PowerShell window closes. Keep any saved reports outside the
bundle so its file verification can detect unexpected files.

The report reads the running Test Mode state, Secure Boot, Memory Integrity,
installed NTPods packages, driver access policy and matching devices. It verifies
bundle files against `SHA256SUMS.txt` and reports whether a reusable local signing
certificate exists. It does not export certificates or show private key material.

Before continuing, confirm:

- Windows is x64, build 22621 or newer, and Test Mode is already active.
- `InstalledUserMatches` and both driver `UserMatches` values are true.
- No earlier rollback or playback recovery is pending. Resolve an existing
  checkpoint first; do not delete it to bypass the check.
- All bundle checksums pass. The existing NTPods microphone and AAP device
  work, and normal music playback is working on this machine.
- The device report shows no driver problem. Have the normal installer available
  separately in case recovery needs it.

Record the report locally. Machine state and device names do not belong in a
public commit. An unreadable security setting is unknown, not a pass. Leave
Memory Integrity unchanged for this test.

## Replace only the two NTPods drivers

Stop recording and playback. Quit any preview backend manually; the installer's
shutdown helper deliberately targets only the installed app. In the same
administrator PowerShell, from the bundle:

```powershell
$ErrorActionPreference = 'Stop'
$bundle = (Resolve-Path .).Path
. (Join-Path $bundle 'setup-common.ps1')
if (-not (Test-TestMode)) { throw 'Test Mode is not active.' }
if (-not (Test-MicSupported)) { throw 'Windows 11 22H2 or newer is required.' }
$sid = Get-CurrentUserSid
$installedSid = (Get-ItemProperty 'HKLM:\SOFTWARE\NTPods').InstalledUserSid
if ($sid -ne $installedSid) { throw 'Use the Windows account that runs NTPods.' }
if (Test-Path -LiteralPath (Get-DriverCheckpointPath)) { throw 'Recover the existing driver checkpoint first.' }
Stop-NTPods
$setup = Initialize-SetupDirectory
$work = Join-Path $setup ('drivers-' + [guid]::NewGuid())
$transaction = [guid]::NewGuid().ToString()
Install-NTPodsDrivers $bundle $work 'acx' $sid $transaction
```

`Install-NTPodsDrivers` exports every installed NTPods AAP/ACX package with
`pnputil /export-driver`, then saves microphone devices, user access policy and
certificate identities under `%ProgramData%\NTPods\driver-rollback`. Export must
succeed before any signing or driver replacement. Both new packages must stage
successfully before either old driver is removed. Installation failure triggers
rollback; failed rollback retains the checkpoint.

Keep that checkpoint throughout the test. **Do not call
`Complete-NTPodsDriverTransaction`**, run another NTPods installer, or delete the
backup. If Windows requests a restart, restart before checking the new endpoint.
Run the preflight again and verify both devices have problem code 0.

Use [the playback instructions](README.md#try-it) from an ordinary, unelevated
terminal. First check the new output at low volume with the microphone off.
Then check left/right separation, recording, microphone start/stop, pause/resume,
disconnect and normal audio restoration. A passing install is not a playback
validation.

## Restore the original drivers

Quit the experimental backend cleanly. If it crashed, run the updated backend
once **without** `--experimental-playback` from an ordinary terminal so it can
restore any saved Bluetooth service state. Verify that
`%LOCALAPPDATA%\NTPods\playback-restore.json` is gone, then quit it. Driver rollback
does not perform that per-user Bluetooth recovery.

In administrator PowerShell, from the same bundle:

```powershell
$ErrorActionPreference = 'Stop'
$bundle = (Resolve-Path .).Path
. (Join-Path $bundle 'setup-common.ps1')
Stop-NTPods
$checkpoint = Get-DriverCheckpointPath
$saved = Get-Content -LiteralPath (Join-Path $checkpoint 'checkpoint.json') -Raw | ConvertFrom-Json
Restore-NTPodsDriverCheckpoint $bundle $saved.TransactionId
```

This removes the test NTPods packages, restores the exported packages and
microphone device, restores their user access policy, and removes only signing
certificates introduced by this transaction. Existing certificates and unrelated
drivers remain in place. A successful rollback removes the checkpoint. If it
fails, keep the checkpoint and error report for recovery; do not run uninstall.

Restart if required, then open the installed NTPods app normally. Check normal
stereo output, the NTPods microphone, settings controls and absence of the
experimental speaker endpoint. Boot security settings and the app's original
startup entries have not been changed.

References: [PnPUtil](https://learn.microsoft.com/en-us/windows-hardware/drivers/devtest/pnputil-command-syntax),
[Inf2Cat](https://learn.microsoft.com/en-us/windows-hardware/drivers/devtest/inf2cat),
and the existing transaction in `windows/installer/setup-common.ps1`.
