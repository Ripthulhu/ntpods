<p align="center"><img src="assets/logo.png" alt="NTPods logo" width="160"></p>

# NTPods

Low-latency AirPods playback, controls, battery levels and a high-quality
microphone for Windows 11 x64.
Based on [arctumn/NTPods](https://github.com/arctumn/ntpods) and
[LibrePods](https://github.com/librepods-org/librepods).

> [!WARNING]
> The optional **Direct Bluetooth** drivers require Windows **Test Mode (test signing)**.
> App-only installation and the USB dongle do not require Test Mode. The direct
> Bluetooth drivers are not Microsoft production-signed; setup signs them locally with a
> test certificate. Test Mode weakens Windows' normal driver-signature checks.
> Enabling it for an everyday or work PC is a bad idea. Use a dedicated test PC.
> Secure Boot can block Test Mode, and changing either setting can trigger
> BitLocker recovery. Save your recovery key before changing boot settings.

[Download](https://github.com/Ripthulhu/ntpods/releases) ·
[Installation](#installing-on-windows) ·
[Low-latency playback](#low-latency-playback) ·
[Model support](docs/WINDOWS-MODEL-SUPPORT.md)

## Preview

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: light)" srcset="docs/images/app-light.png">
    <img src="docs/images/app-dark.png" alt="NTPods with AirPods Pro connected" width="640">
  </picture>
</p>

### Dynamic Island

<p align="center"><img src="docs/images/dynamic-island.png" alt="Dynamic Island showing Adaptive mode and battery levels" width="440"></p>

<details>
<summary>Video preview</summary>

https://github.com/user-attachments/assets/44b47894-22f6-49f1-a891-99ce0b7346bc

</details>

## Features

These features describe the direct Bluetooth path. The experimental Pico 2 W
dongle uses Windows' standard USB audio and HID drivers, with its own pairing,
reconnect, battery and ANC/transparency controls in NTPods. Its USB interface also
supports hearing assistance, profile edits, saved diagnostics, read-only hearing
metadata and guarded in-case checks. Sensors and companion switching are not yet
available through the dongle.
Dongle hardware validation is pending.

- Listening-mode controls with an adjustable Adaptive slider.
- Earbud and case battery levels, including nearby charging updates.
- Find nearby with signal strength, trend and last-heard time for your paired AirPods.
- Nearby charging-case information on Pro 3, without a separate Windows BLE bond.
- A pairing popup when you open a nearby unpaired AirPods case.
- Three-band EQ, Conversation Awareness and Personalized Volume.
- Low-latency AAC-ELD and AAC playback through one AirPods output, with separate
  bitrate settings.
- Automatic pause when you remove an earbud. Manually paused audio stays paused.
- A 48 kHz mono microphone on supported models, with stereo playback and a
  keyboard shortcut.
- Custom Transparency adjustments on supported models, under experimental settings.
- Hearing Aid and Media Assist switches for a profile already set up on an iPhone, under experimental settings.
- A live head-movement graph and heart-rate monitoring on supported models.
- A Dynamic Island popup and Start with Windows.
- Audio transfer and shared app preferences with a linked LibrePods phone.
- Optional incoming mobile-call takeover and return to the interrupted PC player.
  NTPods resumes only the same player it paused, with no newer user choice.

Update both apps together for companion v2. Call takeover requires an explicit
opt-in in each app and phone-state permission on Android; synced preferences
cannot grant consent or permissions. Return defaults on but stays inactive until
takeover is enabled. The new v2 settings and call flow have not been validated on
hardware. See [the companion contract](docs/COMPANION-PROTOCOL.md) for the shared
settings, host-specific exclusions and transfer limits.

### Low-latency playback

AAC-ELD is the default, with AAC fallback for pairs without a supported AAC-ELD
profile. **Game Mode** reduces the receiver buffer on supported devices and
adjusts for microphone use; it is enabled by default. Choose the codec, bitrate,
Game Mode and Wi-Fi coexistence under **Playback and microphone**. Choices are
saved across restarts. See [audio latency](docs/WINDOWS-AUDIO-LATENCY.md)
for supported devices, technical details and limits.

### Compatibility

Requires Windows 11 22H2 or newer, x64. Hardware testing covers an Intel AX200
with AirPods Pro 3 and first-generation AirPods Pro. Features and test coverage vary by model;
see [model support](docs/WINDOWS-MODEL-SUPPORT.md) and
[validation](docs/WINDOWS-VALIDATION.md).

Stopping or recovering the high-quality microphone briefly interrupts playback.
For Android or Linux, use [LibrePods](https://github.com/librepods-org/librepods).

## Installing on Windows

Download **NTPods.msi** for guided setup or **NTPods-Windows.zip** for manual installation from
[releases](https://github.com/Ripthulhu/ntpods/releases). Both include the app and
drivers. Fresh installations default to **app only**, suitable for the USB dongle.
Select **Direct Bluetooth** during setup to install the optional test-signed
drivers for a PC Bluetooth adapter. Existing driver installations retain their
upgrade choices. Source archives and checksums are included.
See the [installation guide](windows/README.md) and [release notes](docs/WINDOWS-RELEASE-NOTES.md).
Upstream nightly builds do not include this fork's changes.

Direct Bluetooth setup configures an [Apple Bluetooth identity](docs/BLUETOOTH-HOST-IDENTITY.md)
for AirPods features such as Custom Transparency. This affects the PC's identity
to all Bluetooth devices and requires a Windows restart when first applied.
Uninstall restores the original values.

The optional Direct Bluetooth feature includes an experimental playback driver for AirPods 1–4,
Pro 1–3 and the original Max (Lightning and USB-C). See the
[playback model list](docs/WINDOWS-MODEL-SUPPORT.md#native-playback) for test status.
It replaces the paired AirPods output driver. NTPods must stay running for
playback. No extra speaker or per-app routing is needed. Keep the AirPods
connected during uninstall to restore their saved driver, or Windows playback
when no prior binding was saved; see the
[playback test guide](windows/playback/TESTING.md).

NTPods leaves Windows' hands-free service and audio endpoints available.
You can disable unwanted legacy hands-free entries in the Sound control panel;
keep the NTPods playback and high-quality microphone endpoints enabled.
NTPods preserves those manual choices.

## Contributing

Start with [CONTRIBUTING.md](CONTRIBUTING.md) for the code layout and tests,
or [build the app and drivers](docs/BUILD-WINDOWS.md).

## What's in the repo

- `windows/`: the drivers (AAP, playback and virtual mic), the daemon (Rust), the
  WinUI app (C#) and the installer
- `docs/`: [AirPods protocol reference](docs/AIRPODS-PROTOCOL.md), capture procedure and hardware results
- `.github/workflows/ci-windows.yml`: builds, tests and opt-in experimental packages

## Credits

- [arctumn/NTPods](https://github.com/arctumn/ntpods) for the Windows application,
  daemon, drivers and installer this fork builds on.
- [LibrePods](https://github.com/kavishdevar/librepods) by
  [kavishdevar](https://github.com/kavishdevar) and its contributors for the AirPods
  protocol work. NTPods isn't affiliated with LibrePods.
- [@thibaup](https://github.com/thibaup) (heart rate on Android, LibrePods PR #702)
  and [@SAGIRIxr](https://github.com/SAGIRIxr) for their heart-rate findings.
- [@Gab4545](https://github.com/Gab4545) for building the drivers in CI.
- [pabloaul/apple-wireshark](https://github.com/pabloaul/apple-wireshark) for the
  Wireshark dissectors.

## License

NTPods application code is GPL-3.0, with component-specific notices and an
unresolved microphone-driver licensing issue in [THIRD_PARTY.md](THIRD_PARTY.md).
AirPods is a trademark of Apple Inc. This project isn't affiliated with Apple.
