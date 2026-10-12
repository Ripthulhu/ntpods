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

The packages replace the paired AirPods Pro 1 or Pro 3 playback driver with
NTPodsAudio. AAC and AAC-ELD share that AirPods output; both offer Best effort and
Manual kbps settings under **Playback and microphone**. No separate speaker or
per-app routing is needed. Initial driver replacement may require selecting the
output again. Codec changes keep the same output.

Keep NTPods running for playback, or enable Start with Windows. If the backend
stops, playback stops too. Keep the AirPods connected during uninstall to restore
the previous playback provider. Its package is retained during installation.
Other models keep their current
playback driver. Available controls depend on model and firmware.

The new playback driver, microphone coexistence and recovery have not passed
hardware testing. Existing microphone results used Alternative A2DP and do not
validate the replacement. See the
[playback test guide](https://github.com/Ripthulhu/ntpods/blob/main/windows/playback/TESTING.md)
and [validation record](https://github.com/Ripthulhu/ntpods/blob/main/docs/WINDOWS-VALIDATION.md).

The ACX microphone driver's licensing issue remains unresolved. Retain all bundled
licenses and notices; see
[THIRD_PARTY.md](https://github.com/Ripthulhu/ntpods/blob/main/THIRD_PARTY.md).
