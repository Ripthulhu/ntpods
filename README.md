<p align="center"><img src="assets/logo.png" alt="NTPods logo" width="160"></p>

# NTPods

> [!WARNING]
> These experimental builds require Windows **Test Mode (test signing)**. The
> drivers are not Microsoft production-signed; setup signs them locally with a
> test certificate. Test Mode weakens Windows' normal driver-signature checks.
> Enabling it for an everyday or work PC is a bad idea. Use a dedicated test PC.
> Secure Boot can block Test Mode, and changing either setting can trigger
> BitLocker recovery. Save your recovery key before changing boot settings.

AirPods controls, battery levels and a microphone for Windows 11 x64.
Based on [arctumn/NTPods](https://github.com/arctumn/ntpods) and
[LibrePods](https://github.com/librepods-org/librepods).

## Preview

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: light)" srcset="docs/images/app-light.png">
    <img src="docs/images/app-dark.png" alt="NTPods with AirPods Pro connected" width="640">
  </picture>
</p>

### Dynamic Island

<p align="center"><img src="docs/images/dynamic-island.png" alt="Dynamic Island showing Adaptive mode and battery levels" width="440"></p>

## Features

- Listening-mode controls with an adjustable Adaptive slider.
- Earbud and case battery levels, including nearby charging updates.
- Three-band EQ, Conversation Awareness and Personalized Volume.
- Automatic pause when you remove an earbud. Manually paused audio stays paused.
- A 48 kHz mono microphone while playback stays stereo, with automatic,
  timed, always-active and manual controls, plus a configurable global shortcut.
- Live head tracking and heart rate on supported models.
- A Dynamic Island popup and Start with Windows.

Tested with an Intel AX200 and AirPods Pro 3 on firmware 9A348. First-generation
AirPods Pro also passed battery, listening-mode and automatic-pause checks.
Features vary by model and firmware; see [model support](docs/WINDOWS-MODEL-SUPPORT.md).

Microphone recovery briefly interrupts playback. Short recordings and twenty
start/stop cycles passed; longer recordings, latency, drift, Discord and interrupted
recovery still need testing. See [test results](docs/WINDOWS-VALIDATION.md).
The replacement AAC/AAC-ELD playback driver passed short Pro 3 stereo checks
with the microphone active and after it stopped. Longer tests remain pending.

This build uses the Windows 11 ACX microphone driver. Older Windows versions
are untested. For Android or Linux, use [LibrePods](https://github.com/librepods-org/librepods).

## Installing on Windows

Download **NTPods.msi** for guided setup or **NTPods-Windows.zip** for manual installation from
[releases](https://github.com/Ripthulhu/ntpods/releases). Both include the app and
drivers and require Test Mode. Source archives and checksums are included.
See the [installation guide](windows/README.md) and [release notes](docs/WINDOWS-RELEASE-NOTES.md).
Upstream nightly builds do not include this fork's changes.

Both downloads include an experimental playback driver for AirPods Pro 1 and Pro 3.
It replaces the paired AirPods output driver, with AAC/AAC-ELD and Best effort or
Manual kbps settings under **Playback and microphone**. NTPods must stay running
for playback. No extra speaker or per-app routing is needed. Keep the AirPods
connected during uninstall to restore their previous driver; see the
[playback test guide](windows/playback/TESTING.md).

## Contributing

Start with [CONTRIBUTING.md](CONTRIBUTING.md) for the code layout and tests,
or [build the app and drivers](docs/BUILD-WINDOWS.md).

## What's in the repo

- `windows/`: the drivers (AAP, playback and virtual mic), the daemon (Rust), the
  WinUI app (C#) and the installer
- `docs/` and `AAP Definitions.md`: notes on the AirPods protocol
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
