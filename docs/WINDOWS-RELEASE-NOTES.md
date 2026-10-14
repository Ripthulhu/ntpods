> [!WARNING]
> **Test Mode (test signing) is required. Enabling it on your everyday or work PC
> is a bad idea.** Use a dedicated test PC.
>
> These experimental kernel drivers are not Microsoft production-signed. Setup
> signs them locally with a test certificate. Test Mode weakens Windows' normal
> driver-signature enforcement, and faulty kernel drivers can crash Windows.
> Secure Boot can block Test Mode. Save your BitLocker recovery key before
> changing either setting; Windows may ask for it at the next boot.

## Installation

Use **NTPods.msi** for the setup wizard. **NTPods-Windows.zip** contains the same
app and drivers with `install.ps1` for manual installation from an administrator
PowerShell. Both require Windows 11 22H2 or newer, x64, and Test Mode already
enabled. Neither installer changes Secure Boot, Memory Integrity or Test Mode.
After removing the drivers, turn Test Mode off and restore any Secure Boot
setting you changed. See
[Microsoft's test-signing documentation](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/the-testsigning-boot-configuration-option).

Setup configures an Apple Bluetooth identity for AirPods features such as Custom
Transparency. This changes the PC's identity to all Bluetooth devices and needs
a Windows restart when first applied. Setup saves the original values and
restores them on uninstall. See the
[host identity reference](https://github.com/Ripthulhu/ntpods/blob/main/docs/BLUETOOTH-HOST-IDENTITY.md).

## Playback

The packages replace the playback driver for AirPods 1–4, Pro 1–3 and the original
Max (Lightning and USB-C), using model-qualified Apple AudioSink IDs. AAC and
AAC-ELD share the paired AirPods output. Initial driver replacement may require
selecting that output again; later codec changes keep it.

Choose the codec under **Playback and microphone**. AAC-ELD is the default;
an existing saved codec choice is preserved. Playback uses one AAC-ELD frame
per packet, with a 20 ms PCM reserve for AAC-ELD and 30 ms for AAC. Each codec
saves its own Best effort or Manual bitrate. AAC-ELD also offers
**Variable (experimental)** with a 256 kbps peak limit; it passed short Pro 3
playback checks, including pause/resume and microphone recovery. Best effort
selects the highest supported rate and does not adapt to radio conditions.

If the pair lacks a supported AAC-ELD profile, NTPods uses AAC with its saved
bitrate. The menu keeps the preferred codec, and its tooltip shows the one in
use. Unsupported manual rates and stream failures report errors. Stop recording
and wait for microphone recovery before changing playback settings.

NTPods requests a supported 5 ms Windows processing period during playback and
uses driver notifications when PCM is ready. Older drivers retain polling.
Pro 3 also requests a 120 ms receiver buffer while the mic is idle, restoring
an explicit 150 ms target for pause, microphone use and playback changes.
Short Pro 3 checks passed with automatic pause/resume transitions, restoration
before microphone capture and the 20 ms AAC-ELD reserve. Sustained use and
loaded-system behavior remain untested; no measured latency improvement is
claimed. See the
[latency reference](https://github.com/Ripthulhu/ntpods/blob/main/docs/WINDOWS-AUDIO-LATENCY.md).

Keep NTPods running, or enable Start with Windows. Playback stops if its backend
stops. During uninstall, keep supported pairs connected so setup can restore
their saved playback drivers. For pairs connected for the first time after
installation, there may be no saved provider; restoration then uses Microsoft's
Bluetooth audio driver. If a pair was absent during uninstall, reconnect it and
run `restore-playback.ps1` from the extracted Windows ZIP as administrator.
Unlisted models keep their current provider.

## Compatibility

AirPods Pro 1 (6F21) and Pro 3 (9A348) passed short AAC/AAC-ELD stereo checks on an
Intel AX200, including 70-second AAC-ELD playback. Pro 3 also kept stereo during
short microphone captures and after stopping. Pro 1's high-quality microphone
is disabled. Other models, long sessions and repeated lifecycle checks remain
unverified with the native playback driver.

Available controls depend on model and firmware. Head tracking displays raw
movement. Custom Transparency passed a Pro 3 adjustment and restoration check
with the Apple host identity. The captured hearing profile remains read-only;
hearing edits still need hardware validation. See the
[model list](https://github.com/Ripthulhu/ntpods/blob/main/docs/WINDOWS-MODEL-SUPPORT.md),
[playback test guide](https://github.com/Ripthulhu/ntpods/blob/main/windows/playback/TESTING.md)
and [validation results](https://github.com/Ripthulhu/ntpods/blob/main/docs/WINDOWS-VALIDATION.md).

The ACX microphone driver's licensing issue remains unresolved. Retain all bundled
licenses and notices; see
[THIRD_PARTY.md](https://github.com/Ripthulhu/ntpods/blob/main/THIRD_PARTY.md).
