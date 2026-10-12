# NTPods for Windows

This checkout builds on NTPods `188e45d` for **Windows 11 x64, Intel AX200, and
AirPods Pro 3 / firmware 9A348**. It retains the Rust daemon and WinUI interface,
with the native AAP Bluetooth driver and ACX virtual microphone. Windows continues
to own the Bluetooth adapter and ordinary audio playback.

The app includes native three-band EQ, a local automatic-pause preference and
additional AirPods settings for stem controls, accessibility and charging.
Controls distinguish reported values from requests awaiting confirmation.
See [Windows controls and protocol limits](../docs/WINDOWS-FEATURES.md).

Battery, noise controls, ear detection, automatic pause and Conversation Awareness
have passed the checks listed in the validation record. The microphone retains
stereo playback during capture. A sustained capture ran for about 21 minutes, but
stopping it left only one earbud playing until a manual reconnect. Since version
0.1.7, NTPods restores the audio services automatically after capture stops.
Twenty repeated capture/stop cycles
passed, with both channels confirmed afterward and AAC stereo preserved at
48 kHz / 256 kbps. Recovery interrupts playback for about 17 seconds. Another
sustained recording and broader recovery checks remain pending. See the
[hardware results](../docs/WINDOWS-VALIDATION.md).

See [build and installation instructions](../docs/BUILD-WINDOWS.md) for the pinned
toolchain and `windows/build.ps1`. The build produces an experimental MSI and
manifest from current source; committed driver prebuilts are not used. Only the
hardened ACX microphone is packaged. The older PortCls source remains available
for reference and is outside this build's supported scope.

- `drivers/aap`: a KMDF profile driver opening AACP L2CAP PSM `0x1001`.
- `drivers/mic`: a capture-only ACX endpoint, `ROOT\NTPodsMicACX`, with bounded
  48 kHz mono 16-bit PCM input from the daemon.
- `daemon`: Bluetooth protocol, AAC-ELD decoding, microphone lifecycle, state and
  user-restricted named-pipe IPC.
- `winui`: tray and settings interface using that IPC.
- `installer`: Program Files installation, account-bound driver policy, narrow
  helper tasks and explicit cleanup.

No production-signed packages are available from this local build. Development
installation requires a separately prepared test setup; building does not change
Secure Boot, Memory Integrity or boot settings. Do not use the upstream nightly
or old driver-only scripts as substitutes for this hardened source build.

[Third-party notices and redistribution gates](../THIRD_PARTY.md) include the
Microsoft ACX sample's MS-PL terms and the verified FFmpeg LGPL-3.0-or-later
bundle. Retain those notices with build artifacts.
