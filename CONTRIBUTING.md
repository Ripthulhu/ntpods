# Contributing

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
| AirPods AudioSink playback driver | `windows/drivers/audio` |
| AAC and AAC-ELD playback | `windows/playback`, `windows/daemon/src/playback` |
| Installation and packaging | `windows/installer`, `windows/build.ps1` |

The backend owns the dongle HID session and publishes state through named
pipes. Windows sends USB audio directly to the dongle.
See the [dongle setup and control guide](docs/PICO-DONGLE.md).

For Bluetooth work, start with the [protocol reference](docs/AIRPODS-PROTOCOL.md)
and [macOS capture procedure](docs/MACOS-PROTOCOL-CAPTURE.md). Keep decoded fields,
hardware observations and unresolved interpretations distinct.

## Build and test

Install the tools listed in [BUILD-WINDOWS.md](docs/BUILD-WINDOWS.md). From the
repository root, PowerShell 7 runs the build and its automated checks:

```powershell
./windows/build.ps1
```

This builds and tests the app and produces a portable ZIP.
Use this script for WinUI builds so the package includes the WinUI resources.

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

```powershell
cargo test --locked --manifest-path windows/daemon/Cargo.toml
```

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

In a pull request, explain the behavior changed and how you checked it.
For connection or audio changes, include the AirPods model, firmware, Bluetooth
adapter and playback driver. Remove personal device identifiers from logs.
For audio timing changes, follow the Microsoft references and measurement
procedure in [Windows audio latency](docs/WINDOWS-AUDIO-LATENCY.md).
