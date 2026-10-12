> [!WARNING]
> **Test Mode (test signing) is required. Enabling it on your everyday or work PC
> is a bad idea.** Use a dedicated test PC.
>
> These experimental kernel drivers are not Microsoft production-signed. Setup
> signs them locally with a test certificate. Test Mode weakens Windows' normal
> driver-signature enforcement, and faulty kernel drivers can crash Windows.
> Secure Boot can block Test Mode. Save your BitLocker recovery key before
> changing either setting; Windows may ask for it at the next boot.

Use **NTPods.msi** for the setup wizard. **NTPods-Windows.zip** contains the same app and drivers
with `install.ps1` for manual installation from an administrator PowerShell.
Both require Windows 11 22H2 or newer, x64, and Test Mode already enabled.
Neither installer changes Secure Boot, Memory Integrity or Test Mode for you.
After uninstalling the drivers, turn Test Mode off and restore any Secure Boot
setting you changed. See [Microsoft's test-signing documentation](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/the-testsigning-boot-configuration-option).

Those packages retain the PC's A2DP playback driver and codec settings. Available
controls depend on the AirPods model and firmware. Microphone recovery interrupts
playback, and sustained capture, latency/drift and broader hardware testing remain
incomplete. Read the [validation record](https://github.com/Ripthulhu/ntpods/blob/main/docs/WINDOWS-VALIDATION.md)
before testing.

**NTPods-Native-AAC.zip** adds a separate experimental 48 kHz stereo AAC playback
endpoint. It needs an existing working NTPods installation on Windows 11 x64
with Test Mode active. It replaces both NTPods drivers, keeps a rollback copy and
temporarily releases normal AirPods audio while running. Playback, recording
coexistence and recovery still need hardware testing. Follow the included
`TESTING.md` before installing it. AAC-ELD encoder binaries are excluded.

The ACX microphone driver's licensing issue remains unresolved. Retain all bundled
licenses and notices; see
[THIRD_PARTY.md](https://github.com/Ripthulhu/ntpods/blob/main/THIRD_PARTY.md).
