# Bluetooth host identity

NTPods setup configures Windows to advertise Apple’s Bluetooth vendor ID. AirPods
require this identity for some settings. There is no app switch: MSI and ZIP
installation apply it, upgrades preserve it, and uninstall restores the saved
original values. Restart Windows when setup requests it.

This changes the PC’s Bluetooth identity for **all remote devices**, not just
AirPods. Other Bluetooth behavior may change. It does not change the radio firmware,
Product ID, or version. Microsoft documents the two registry values in
[Bluetooth host radio support](https://learn.microsoft.com/en-us/windows-hardware/drivers/bluetooth/bluetooth-host-radio-support#how-to-change-the-device-id-profile-record-published-by-windows).

## Hardware coverage

On Windows 11 with an Intel AX200, NTPods 0.2.25 and AirPods Pro 3 A3064 firmware
9A348, hearing and Custom Transparency reads returned ATT error `0x0E` before
this change. Both reads succeeded after applying it and restarting Windows,
without re-pairing. NTPods 0.2.26 correctly reads the unenrolled hearing status.
Custom Transparency passed an audible noise-reduction adjustment and restoration
by readback. The hearing value remains read-only; hearing-assistance activation
and edits are unverified. The Windows SDP reply has not yet been captured, and
other models and adapters remain untested.

## Backup and recovery

The helper saves the original values, including their registry types and whether
they existed, before setting `DIDVendorID=0x004C` and `DIDVendorIDSource=1` under
`HKLM\SYSTEM\CurrentControlSet\Services\BTHPORT\Parameters`. Its backup is protected
with Windows DPAPI in `%ProgramData%\NTPods\bluetooth-host-identity`; only administrators
and SYSTEM can access that directory. Repeated installation keeps the original backup.

Install rollback restores only an identity first configured by that transaction.
It preserves an identity and backup from a previous installation. MSI’s removal
of an old version during an upgrade does not restore the identity. App-only
upgrades request a restart if they configure it for the first time. Uninstall
requests a restart to finish restoring the Bluetooth identity and audio drivers.
MSI restores the identity only after removal commits, so cancelling uninstall
leaves it unchanged.

From the repository root, inspect the configured values:

```powershell
pwsh -File windows/bluetooth-host-identity.ps1 -Action Status
```

If installation is interrupted, its transaction journal is retained. From an
administrator PowerShell, roll back that transaction before retrying setup:

```powershell
$pending = Get-Content "$env:ProgramData\NTPods\bluetooth-host-identity\transaction.json" -Raw | ConvertFrom-Json
pwsh -File windows/bluetooth-host-identity.ps1 -Action Rollback -TransactionId $pending.TransactionId
```

Manual recovery can restore the saved original values from an administrator
PowerShell. Restart Windows afterward; the next installation configures the Apple
identity again.

```powershell
pwsh -File windows/bluetooth-host-identity.ps1 -Action Restore
```

If MSI removes the app but cannot restore the identity, it retains the helper
scripts under `%ProgramData%\NTPods\identity-removal`. After resolving the reported
error, finish that removal from an administrator PowerShell:

```powershell
$recovery = "$env:ProgramData\NTPods\identity-removal"
$transaction = Get-Content "$recovery\transaction.txt" -Raw
& "$recovery\msi-setup.ps1" -Action finish-remove -TransactionId $transaction
```

Restore refuses values that another program changed after installation and keeps
the backup for inspection. Do not delete the backup until restoration succeeds.
The helper never restarts the radio, changes pairing, or enables hearing features.
In a release ZIP or the installed folder, the helper is at the root; omit
`windows/` from these commands.

Read the hearing settings before trying any adjustment. Do not replace unreadable
settings with defaults or create an audiogram for a protocol test.

Run the isolated checks without changing Bluetooth settings:

```powershell
pwsh -File windows/bluetooth-host-identity.ps1 -SelfTest
```
