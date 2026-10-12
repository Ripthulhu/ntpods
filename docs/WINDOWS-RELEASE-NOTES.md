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

The packages replace the playback driver for AirPods 1–4, Pro 1–3 and the original
Max (Lightning and USB-C), using verified Apple AudioSink IDs. Pro 1 and Pro 3
have passed short hardware checks; the other models are available to test.
AAC and AAC-ELD share that AirPods output; both offer Best effort and
Manual kbps settings under **Playback and microphone**. No separate speaker or
per-app routing is needed. Initial driver replacement may require selecting the
output again. Codec changes keep the same output.

An initial connection rejection now gets two automatic retries. Disconnecting
or changing the selected pair cancels the wait.

AAC remains the default. A pair without a supported AAC-ELD profile uses AAC
with its saved bitrate setting. NTPods reports the active codec and retains the
preferred codec for future connections. Stream failures still report errors.

AAC-ELD uses plain ELD without SBR, constant bitrate and two access units per
packet with a 48 kHz RTP clock, following the
[USBPods fork tested on Pro 3](https://github.com/han-um/USBPods-Pico2W-2-way-audio-implement/blob/d18d083bc52de3143d8d922c4abaae865cc0dc28/src/btstack/btstack_avdtp_source.c).
Best effort selects the highest supported rate; it does not adapt to radio
conditions.

Keep NTPods running for playback, or enable Start with Windows. If the backend
stops, playback stops too. Keep the AirPods connected during uninstall to restore
the previous playback provider. Its package is retained during installation.
A pair first connected after installation may bind automatically; without a
saved prior binding, uninstall restores Microsoft's Bluetooth audio driver.
Unlisted models keep their current provider. Available controls depend on model
and firmware; see the [model list](https://github.com/Ripthulhu/ntpods/blob/main/docs/WINDOWS-MODEL-SUPPORT.md#native-playback).

Short tests on AirPods Pro 3 (A3063, firmware 9A348) with Intel AX200 confirmed
stereo AAC at 256 kbps, AAC-ELD at 128, 192 and 256 kbps, and working volume and
mute. Both codecs kept stereo playback during 20-second microphone captures
and after microphone stop and recovery. Codec changes kept the same Windows
output.

First-generation AirPods Pro (A2084, firmware 6F21) also passed AAC at 256 kbps
and AAC-ELD at 128, 192 and 256 kbps. Both models completed continuous 70-second
AAC-ELD tests without audible dropouts. Pro 1's high-quality microphone remains
disabled. Long sessions, repeated lifecycle tests and other models remain
untested. See the
[playback test guide](https://github.com/Ripthulhu/ntpods/blob/main/windows/playback/TESTING.md)
and [validation record](https://github.com/Ripthulhu/ntpods/blob/main/docs/WINDOWS-VALIDATION.md).

The ACX microphone driver's licensing issue remains unresolved. Retain all bundled
licenses and notices; see
[THIRD_PARTY.md](https://github.com/Ripthulhu/ntpods/blob/main/THIRD_PARTY.md).
