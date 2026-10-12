> [!WARNING]
> **Test Mode (test signing) is required. Enabling it on your everyday or work PC
> is a bad idea.** Use a dedicated test PC.
>
> These experimental kernel drivers are not Microsoft production-signed. Setup
> signs them locally with a test certificate. Test Mode weakens Windows' normal
> driver-signature enforcement, and faulty kernel drivers can crash Windows.
> Secure Boot can block Test Mode. Save your BitLocker recovery key before
> changing either setting; Windows may ask for it at the next boot.

Use the **MSI** for the setup wizard. The **ZIP** contains the same app and drivers
with `install.ps1` for manual installation from an administrator PowerShell.
Both require Windows 11 22H2 or newer, x64, and Test Mode already enabled.
Neither installer changes Secure Boot, Memory Integrity or Test Mode for you.
After uninstalling the drivers, turn Test Mode off and restore any Secure Boot
setting you changed. See [Microsoft's test-signing documentation](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/the-testsigning-boot-configuration-option).

These builds retain the PC's A2DP playback driver and codec settings. Available
controls depend on the AirPods model and firmware. Microphone recovery interrupts
playback, and sustained capture, latency/drift and broader hardware testing remain
incomplete. Read the [validation record](https://github.com/Ripthulhu/ntpods/blob/codex/windows-airpods-hardening/docs/WINDOWS-VALIDATION.md)
before testing.

The ACX microphone driver's licensing issue remains unresolved. Retain all bundled
licenses and notices; see
[THIRD_PARTY.md](https://github.com/Ripthulhu/ntpods/blob/codex/windows-airpods-hardening/THIRD_PARTY.md).
