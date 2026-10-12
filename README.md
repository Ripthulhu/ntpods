<p align="center"><img src="assets/logo.png" alt="NTPods logo" width="160"></p>

# NTPods

> [!WARNING]
> These experimental builds require Windows **Test Mode (test signing)**. The
> drivers are not Microsoft production-signed; setup signs them locally with a
> test certificate. Test Mode weakens Windows' normal driver-signature checks.
> Enabling it for an everyday or work PC is a bad idea. Use a dedicated test PC.
> Secure Boot can block Test Mode, and changing either setting can trigger
> BitLocker recovery. Save your recovery key before changing boot settings.

NTPods brings AirPods controls, battery information and a virtual microphone to
Windows. This is [Ripthulhu's fork](https://github.com/Ripthulhu/ntpods) of
[arctumn/NTPods](https://github.com/arctumn/ntpods), based on commit
`188e45df20259b616ce155be812086259d1a2488`. NTPods derives its protocol work from
[LibrePods](https://github.com/librepods-org/librepods).

This fork targets Windows 11 x64 and remains experimental. The
[microphone driver's licensing issue](THIRD_PARTY.md) is unresolved.
Production signing and hardware acceptance also remain incomplete. See the
[validation record](docs/WINDOWS-VALIDATION.md) for completed checks and open gates.

## Features in this fork

- A focused settings page with continuous listening-mode and Adaptive controls,
  animated menus, and a battery popup on the primary monitor.
- Connected and nearby battery readings, with per-component freshness and cached
  case percentages.
- Native three-band EQ, Conversation Awareness, personalized volume and
  model-specific device settings.
- Automatic pause that resumes only playback NTPods paused.
- A 48 kHz mono Windows microphone while playback remains stereo, with automatic,
  delayed-stop, keep-active and manual modes.
- Live head movement and heart-rate views on supported models.
- Account-restricted driver and IPC access, bounded microphone recovery, and
  installation under Program Files with a persistent Start with Windows option.

Controls depend on the model, firmware and reported settings. Hardware checks used
an Intel AX200 and AirPods Pro 3 on firmware 9A348. First-generation AirPods Pro
also passed battery, listening-mode and automatic-pause checks. Other catalog
entries still need hardware testing; see [model support](docs/WINDOWS-MODEL-SUPPORT.md).

Short stereo recording tests and twenty microphone start/stop cycles passed on
Pro 3. A sustained recording exposed one-sided playback after stopping; the
subsequent automatic recovery passed the shorter cycle checks. Sustained capture
with that recovery, calibrated latency/drift, Discord and interrupted recovery
still need validation. Windows keeps the existing A2DP playback driver and codec
configuration. See [controls and protocol limits](docs/WINDOWS-FEATURES.md).

The supported build uses the Windows 11 ACX microphone driver. Older Windows and
PortCls support are outside this fork's validation scope. For Android or Linux,
use [LibrePods](https://github.com/librepods-org/librepods).

## Installing on Windows

Read the [release notes](docs/WINDOWS-RELEASE-NOTES.md),
[Windows README](windows/README.md) and [validation record](docs/WINDOWS-VALIDATION.md)
before installing. Public binary uploads remain disabled while the licensing
issue is unresolved. The build workflow can publish experimental MSI and ZIP
packages once a maintainer enables it; see the
[build instructions](docs/BUILD-WINDOWS.md). Upstream nightly builds do not include
this fork's changes.

## What's in the repo

- `windows/`: the two drivers (AAP channel and virtual mic), the daemon (Rust), the
  WinUI app (C#) and the installer
- `docs/` and `AAP Definitions.md`: notes on the AirPods protocol
- `.github/workflows/ci-windows.yml`: builds, tests and opt-in experimental packages

## Credits

- [arctumn/NTPods](https://github.com/arctumn/ntpods) for the Windows application,
  daemon, drivers and installer this fork builds on.
- [LibrePods](https://github.com/kavishdevar/librepods) by
  [kavishdevar](https://github.com/kavishdevar) and its contributors. The protocol
  work this is built on comes from there. NTPods isn't affiliated with LibrePods.
- [@thibaup](https://github.com/thibaup) (heart rate on Android, LibrePods PR #702)
  and [@SAGIRIxr](https://github.com/SAGIRIxr) for their heart-rate findings.
- [@Gab4545](https://github.com/Gab4545) for building the drivers in CI.
- [pabloaul/apple-wireshark](https://github.com/pabloaul/apple-wireshark) for the
  Wireshark dissectors.

## License

NTPods application code is GPL-3.0, with component-specific notices and an
unresolved microphone-driver licensing issue in [THIRD_PARTY.md](THIRD_PARTY.md).
AirPods is a trademark of Apple Inc. This project isn't affiliated with Apple.
