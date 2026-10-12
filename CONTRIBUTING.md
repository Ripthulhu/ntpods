# Contributing

NTPods has a Rust backend, a WinUI app and two Windows kernel drivers.
Start with the code for the feature you want to change; most tests run without
AirPods or installed drivers.

## Code layout

| Area | Where to work |
| --- | --- |
| Device connection and command dispatch | `windows/daemon/src/main.rs` |
| AirPods packets and settings | `windows/daemon/src/aap.rs`, `device_settings.rs` |
| Microphone start, stop and recovery | `windows/daemon/src/mic_lifecycle.rs`; integration in `main.rs` |
| AAC-ELD decoding and PCM output | `windows/daemon/src/eld.rs`, `eld_shim.c`, `micpipe.rs` |
| Nearby battery reports | `windows/daemon/src/battery_ble.rs` |
| Supported models and capabilities | `windows/device-models.json` |
| Shared commands and state | `windows/ipc/src/lib.rs` and `windows/winui/NTPods.WinUI/Ipc/Messages.cs` |
| Named-pipe connections and message framing | `windows/daemon/src/ipc_server.rs` |
| Settings page and controls | `windows/winui/NTPods.WinUI/Pages` and `Controls` |
| Dynamic Island | `windows/winui/NTPods.WinUI/Popup` |
| Bluetooth AAP driver | `windows/drivers/aap` |
| Windows 11 ACX microphone driver | `windows/drivers/mic` |
| Optional AAC and AAC-ELD playback | `windows/playback`, `windows/daemon/src/playback` |
| Installation and packaging | `windows/installer`, `windows/build.ps1` |

The backend owns Bluetooth connections and publishes state through named pipes.
The UI displays that state and sends commands; it does not open the drivers.
The microphone driver receives PCM from the backend. Windows keeps control of
normal Bluetooth playback.

The [playback experiment](windows/playback/README.md) has a separate build and
download, `NTPods-Native-AAC.zip`. The normal MSI and Windows ZIP keep the existing
playback driver. AAC-ELD encoding remains a local development option.

## Build and test

Install the tools listed in [BUILD-WINDOWS.md](docs/BUILD-WINDOWS.md). From the
repository root, PowerShell 7 runs the full build and test suite:

```powershell
./windows/build.ps1
```

This builds the app, both drivers and MSI. It does not install them or change
boot settings. Test Mode is required to load the drivers, not to compile them.
Use this script for WinUI builds; `dotnet build` alone can miss the resource targets.

For a smaller change, run the relevant checks:

```powershell
# Shared Rust/C# command and state format
pwsh -File windows/tests/test-ipc.ps1

# Listening-mode geometry and request ordering
dotnet run --project windows/tests/ListeningMode --configuration Release

# Settings, battery readings and model capabilities
dotnet run --project windows/tests/DeviceSettings --configuration Release

# Pipe connection and shutdown handling
dotnet run --project windows/tests/ClientSession --configuration Release

# Dynamic Island motion
dotnet run --project windows/tests/IslandMotion --configuration Release
```

For backend tests, use VS2022 Developer PowerShell after fetching FFmpeg:

```powershell
bash windows/daemon/fetch-ffmpeg.sh
$env:PATH = "$(Resolve-Path windows/daemon/vendor/ffmpeg/bin);$env:PATH"
cargo test --locked --manifest-path windows/daemon/Cargo.toml
```

[windows/tests/README.md](windows/tests/README.md) describes the checks.
Driver changes also need the C/C++ tests run by `windows/build.ps1`.
[WINDOWS-VALIDATION.md](docs/WINDOWS-VALIDATION.md) records hardware results and
the tests still needed. A passing unit test does not establish hardware support.

## Making changes

- Follow [DESIGN.md](DESIGN.md) for UI work. Reuse the existing controls, styles
  and motion helpers; check light, dark and narrow layouts.
- Keep protocol parsing and state transitions in their existing modules.
  Keep `main.rs` focused on connecting those parts. Add a module when it has a
  separate responsibility.
- Change both IPC definitions when adding a command or state field, and extend
  the cross-language fixtures.
- Add model capabilities to `windows/device-models.json`; both languages use it.
- Keep UI text in the localization resources and preserve accessibility labels.
- Test changed behavior with existing test projects. Include a regression case
  for a bug; describe any hardware test that remains unverified.
- Write comments for a constraint, protocol quirk or ownership rule that the
  code cannot explain. Avoid narrating each statement or recounting the change.
- Keep recordings, device addresses, private certificates and build output out
  of commits. Preserve copyright and license notices.

Rebuild after changing source or dependencies before packaging a release.
`windows/package-release.ps1` checks the source commit and tested file hashes.
Keep the FFmpeg DLLs, source archives and build recipes matched; see
[THIRD_PARTY.md](THIRD_PARTY.md).

In a pull request, explain the behavior changed and how you checked it.
For connection or audio changes, include the AirPods model, firmware, Bluetooth
adapter and playback driver. Remove personal device identifiers from logs.
