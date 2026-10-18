# Windows tests

Run commands from the repository root. The complete build and test entry point is
[`windows/build.ps1`](../build.ps1); see the
[toolchain requirements](../../docs/BUILD-WINDOWS.md).

`python windows/tests/test-aap-extract.py` checks ear-state decoding in the
[capture helper](../docs/aap_extract.py), including truncated packets, unknown
values and command-line output. It also runs in the complete build.

`python windows/tests/test-pfc-decode.py` checks the offline
[Fast Connect decoder](../docs/pfc_decode.py): message framing, channel and
endpoint descriptors, delay units, extended TLV lengths and truncated input.
It uses synthetic packets and also runs in the complete build.

`pwsh -File windows/tests/test-bluetooth-host-identity.ps1` checks the
[host identity helper](../../docs/BLUETOOTH-HOST-IDENTITY.md), including backup,
rollback and restoration. It uses isolated fixtures and leaves the Bluetooth
registry and controller untouched. The complete build runs this check too.

## IPC contract

For the IPC checks alone, install Rust/Cargo and the .NET 10 SDK, then run:

```powershell
pwsh -File windows/tests/test-ipc.ps1
```

The Rust `ntpods-ipc` crate emits its actual flattened `Event::State` messages.
A small .NET console links the production WinUI `Ipc/Messages.cs`, checks every
microphone lifecycle phase, capture and uplink flags, availability, and escaped
error text, then serializes the C# snapshots and microphone commands back for
Rust to verify. There are no additional NuGet test dependencies or WinUI runtime
requirements. Old snapshots missing the new fields stay inactive, and malformed
JSON roots, tags, and microphone fields must be ignored without exceptions.

The runner creates temporary NDJSON fixtures and removes only that temporary
directory. It does not connect to named pipes or Bluetooth or load drivers.

Kernel PCM/SID/request validation has its separate portable harness in
[`../drivers/mic/README.md`](../drivers/mic/README.md).

## Playback restoration

With the [backend toolchain](../../CONTRIBUTING.md#build-and-test) configured, run
`cargo test --locked --manifest-path windows/daemon/Cargo.toml --features experimental-playback --test playback_buffer`.
This runs the production automatic-buffer and diagnostic-trial logic against
simulated drivers, including microphone demand, restore/reapply, old-driver
fallback and partial media-reopen failures. It checks that microphone startup
stays blocked until restoration and playback re-priming succeed.
The complete build runs it too; it does not open audio devices or Bluetooth.

## Windows startup checks

`dotnet run --project windows/tests/Startup/Startup.csproj --configuration Release`
checks the production startup service against a private temporary registry key.
It covers paired commands, missing binaries, stale paths and denied writes.

After packaging, `windows/installer/msi/test-startup.ps1 -Msi <path>` checks the
compiled MSI's startup actions in a restricted Windows Installer session. It
verifies fresh-install defaults and preservation of on/off through repair and
upgrade. Both checks run in `windows/build.ps1` and leave installed startup entries
untouched.

`pwsh -File windows/installer/test-app-transaction.ps1` checks ZIP application
staging and rollback with temporary files and mocked system operations. It covers
copy and setup failures, retained recovery backups and redirected paths. The
complete build runs it in PowerShell 7 and Windows PowerShell.

## UI interaction and motion

```powershell
dotnet run --project windows/tests/ListeningMode/ListeningMode.csproj --configuration Release
dotnet run --project windows/tests/IslandMotion/IslandMotion.csproj --configuration Release
```

These checks use the production geometry, request queue and motion helpers without
opening a window or changing AirPods settings. They cover continuous strength,
request ordering, transition interruption and reduced motion. Native checks must
still cover pointer tracking, icon stability, popup clipping and scrollbar fading.

## IPC sessions and shutdown

`dotnet run --project windows/tests/ClientSession --configuration Release`
uses isolated Windows named pipes and short-lived child processes. It tests
fresh-state readiness, mismatched server processes, idle command-pipe loss,
event-pipe loss, discarded queued commands, and shutdown waiting for recovery
and process exit. A timeout must leave the test backend alive. These fixtures
never use NTPods' installed pipes or connect to Bluetooth.

Backend IPC tests use unique Windows pipe names to check ownership, rejection of
pre-existing pipes, and reconnection after the first client closes. They run with
the backend tests in the complete build.

`dotnet run --project windows/tests/DeviceSettings --configuration Release`
also checks rename byte limits and reply correlation, real heart-rate timestamps,
unchanged BPM samples, stale readings, and gaps without invented samples.
Settings-save checks interrupt a write in a separate process and deny replacement
of an existing file. The previous settings must survive, and a later save must
succeed. All files stay in isolated temporary directories.
Microphone policy checks cover the unchanged automatic default, delay bounds,
pending versus confirmed choices, manual demand, the permission toggle and recovery.
They also check microphone notifications for repeated snapshots, reconnects,
manual stops and changes between capture policies.

`dotnet run --project windows/tests/SensorUi --configuration Release` checks
heart-rate graph caching, downsampling, exact sample selection and timestamp gaps.
It uses the production data helper without creating a WinUI window or connecting
to the sensors.

## Global microphone shortcut

`dotnet run --project windows/tests/Hotkey --configuration Release` checks allowed
key combinations and modifier handling. Add `-- --native` on Windows to check
registration conflicts, replacement, capture suppression and cleanup with hidden
test windows. It briefly reserves Ctrl+Alt+Shift+F23/F24 and injects no keyboard input.
