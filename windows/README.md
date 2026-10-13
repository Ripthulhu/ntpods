# NTPods for Windows

> [!WARNING]
> Installation requires Windows **Test Mode (test signing)**. Setup signs these
> experimental drivers with a local test certificate; they are not Microsoft
> production-signed. Test Mode weakens normal driver-signature enforcement.
> Do not enable it on your everyday or work PC for NTPods. Use a dedicated test PC.
> Secure Boot can block Test Mode. Save your BitLocker recovery key before
> changing boot settings.

NTPods provides AirPods controls, batteries, a high-quality microphone and
experimental AAC/AAC-ELD playback on Windows 11 22H2 or newer, x64. Windows owns the Bluetooth
adapter; NTPods adds an AAP control driver, an ACX microphone and an AudioSink
playback driver.

The MSI and ZIP include experimental AAC and AAC-ELD playback for the
[listed AirPods models](../docs/WINDOWS-MODEL-SUPPORT.md#native-playback). Choose
the codec and bitrate under **Playback and microphone**.
Both use the paired AirPods output; NTPods must remain running. Pro 1 and Pro 3
have passed short playback checks. Other models are available to test using the
[playback guide](playback/TESTING.md).

The app includes native three-band EQ, a local automatic-pause preference and
additional AirPods settings for stem controls, accessibility and charging.
Controls distinguish reported values from requests awaiting confirmation.
See [Windows controls and protocol limits](../docs/WINDOWS-FEATURES.md).

Hardware checks used an Intel AX200, AirPods Pro 3 on firmware 9A348 and Pro 1
on 6F21. Pro 3 kept stereo playback during short microphone captures with both
codecs. Stopping the microphone briefly interrupts playback; native-driver
recovery took about two seconds in those checks. Longer capture, repeated
interruptions and other adapters remain unverified. See the
[hardware results](../docs/WINDOWS-VALIDATION.md).

See [build and installation instructions](../docs/BUILD-WINDOWS.md) for the pinned
toolchain and `windows/build.ps1`. The build produces an experimental MSI and
manifest from current source; committed driver prebuilts are not used. Only the
hardened ACX microphone is packaged. The older PortCls source remains available
for reference and is outside this build's supported scope.

- `drivers/aap`: a KMDF profile driver opening AACP L2CAP PSM `0x1001`.
- `drivers/mic`: an ACX device, `ROOT\NTPodsMicACX`, with bounded 48 kHz mono
  microphone input.
- `drivers/audio`: the ACX AudioSink driver for the paired AirPods playback output.
- `daemon`: Bluetooth protocol, AAC-ELD decoding, microphone lifecycle, state and
  user-restricted named-pipe IPC.
- `winui`: tray and settings interface using that IPC.
- `installer`: Program Files installation, account-bound driver policy, narrow
  helper tasks and explicit cleanup.

No production-signed packages are available. Both MSI and ZIP installations
require a separately prepared test setup; building and installing do not change
Secure Boot, Memory Integrity or Test Mode. See the
[release notes](../docs/WINDOWS-RELEASE-NOTES.md) for package choices and limits,
and [Microsoft's test-signing documentation](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/the-testsigning-boot-configuration-option)
for the boot policy. Do not use the upstream nightly or old driver-only scripts
as substitutes for this fork's build.

[Third-party notices](../THIRD_PARTY.md) include the
Microsoft ACX sample's MS-PL terms and the verified FFmpeg LGPL-3.0-or-later
bundle. Retain those notices with build artifacts.
