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
Max (Lightning and USB-C). AAC and AAC-ELD share the paired AirPods output.
Initial driver replacement may require
selecting that output again; later codec changes keep it.

Choose the codec under **Playback and microphone**. AAC-ELD is the default;
an existing saved codec choice is preserved. Each codec saves its own Best effort
or Manual bitrate. AAC-ELD also offers **Variable (experimental)** with a 256 kbps
peak limit. Best effort selects the highest supported rate and does not adapt
to radio conditions.

If the pair lacks a supported AAC-ELD profile, NTPods uses AAC with its saved
bitrate. The menu keeps the preferred codec, and its tooltip shows the one in
use. Unsupported manual rates and stream failures report errors. Stop recording
and wait for microphone recovery before changing playback settings.

NTPods can automatically request a smaller playback buffer on Pro 3 and AAC-ELD
pairs that advertise gaming audio support while the microphone is idle.
End-to-end latency has not been measured. See the
[latency reference](https://github.com/Ripthulhu/ntpods/blob/main/docs/WINDOWS-AUDIO-LATENCY.md).

Keep NTPods running, or enable Start with Windows. Playback stops if its backend
stops. During uninstall, keep supported pairs connected so setup can restore
their saved playback drivers. For pairs connected for the first time after
installation, there may be no saved provider; restoration then uses Microsoft's
Bluetooth audio driver. If a pair was absent during uninstall, reconnect it and
run `restore-playback.ps1` from the extracted Windows ZIP as administrator.
Unlisted models keep their current provider.

## Compatibility

**Hardware checks** reads the saved self-check results on supported Pro 2 and Pro 3
pairs. Refresh does not run a test or play sounds. These cached flags do not
establish a current fault, and the device clock may differ from the PC's clock.

Hardware testing covers AirPods Pro 1 (6F21) and Pro 3 (9A348) with an Intel
AX200. Pro 1's high-quality microphone is disabled. Other models, long sessions
and microphone transitions with the smaller buffer remain unverified.

Available controls depend on model and firmware. Head tracking displays raw
movement. Custom Transparency, Hearing Aid and Media Assist are experimental.
On Pro 3 (9A348), Hearing Aid and Media Assist can independently use a profile
already fitted on an iPhone. Amplification, balance, tone, Conversation Boost
and one audiogram field passed write/readback and restoration checks on that
model. Complete audiogram replacement and other models remain untested.
NTPods cannot run the iPhone Hearing Test. See the
[model list](https://github.com/Ripthulhu/ntpods/blob/main/docs/WINDOWS-MODEL-SUPPORT.md),
[playback test guide](https://github.com/Ripthulhu/ntpods/blob/main/windows/playback/TESTING.md)
and [validation results](https://github.com/Ripthulhu/ntpods/blob/main/docs/WINDOWS-VALIDATION.md).

The ACX microphone driver's licensing issue remains unresolved. Retain all bundled
licenses and notices; see
[THIRD_PARTY.md](https://github.com/Ripthulhu/ntpods/blob/main/THIRD_PARTY.md).
