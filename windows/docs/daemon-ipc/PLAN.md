# Windows daemon and IPC

`ntpodsd.exe` owns the Bluetooth session, driver handles, audio processing and
device state. The WinUI app displays that state and sends commands over named
pipes. Its tray icon and Dynamic Island use the same client connection.

## Wire contract

| Pipe | Direction |
| --- | --- |
| `\\.\pipe\NTPods-events` | Daemon to client |
| `\\.\pipe\NTPods-cmds` | Client to daemon |

Each pipe carries newline-delimited UTF-8 JSON. Separate handles let command reads
and event writes proceed independently; a blocking read on a synchronous duplex
handle would also block writes on that handle. The Windows pipes use byte mode,
so readers must assemble complete lines across reads.

Access is restricted to the daemon's Windows user and SYSTEM. Remote pipe clients
are rejected. The command reader bounds its buffered input to 16 KiB and ignores
invalid JSON commands. Each event client has a 32-message queue and its own writer
thread so a slow UI does not block Bluetooth processing.

The authoritative types are in [`ipc/src/lib.rs`](../../ipc/src/lib.rs), with C#
definitions in [`Messages.cs`](../../winui/NTPods.WinUI/Ipc/Messages.cs). Commands
cover connection, device settings, audio, sensors and shutdown. Events include
full state snapshots, command results, overlays, nearby invitations and head movement. Change both
definitions together and run [`test-ipc.ps1`](../../tests/test-ipc.ps1).

`SetAnc` and `SetDeviceControl` accept an optional `device_id`: the selected
pair's twelve hexadecimal address digits. The app sends the identity shown by
the control. The daemon ignores the action if selection has changed, checking
under the same lock as pair selection. Omitted targets retain legacy current-pair
behavior; empty, malformed or null targets are rejected. Deploy the app and daemon
together: older daemons ignore the added target field.

## Connection lifecycle

`PairingNearby` carries the public advertisement's 16-bit model hint and
`lid_open` state. Opening the lid can show a pairing card; closing it cancels and
dismisses that model's lid invitation. This event cannot identify or connect a
Classic endpoint. The client finds that endpoint through Windows discovery.

The app starts the daemon if needed and reconnects after a lost pipe. The daemon
is single-instance and sends a state snapshot when an event client connects.
The client also sends `Hello` and `GetState`, then waits for a snapshot before
treating the session as ready. Idle command connections send `Ping` every two
seconds to detect a broken command pipe even if events are still arriving.

Closing a pipe releases that client's resources; it does not stop the daemon.
`Shutdown` requests daemon cleanup and exit. Head-tracking requests belong to the
command connection that made them, so losing that connection releases its stream.

After Windows pairing succeeds, `PairingCompleted` carries the selected device's
Classic Bluetooth address. The daemon waits up to 30 seconds for that exact
remembered, authenticated pair and its AAP service before changing audio. It
closes the previous control session, selects the new pair and requests its audio
connection. Automatic discovery cannot replace this target; a later `Connect`,
`RepairConnection` or `Disconnect` cancels the pending handoff.

## Implementation

`GetHearingMetadata` requires the selected `device_id`. Its optional snapshot
record preserves the test-mode byte, raw sensitivity and 17 raw curve values;
pending/error fields are separate from the last complete read.

`StartHardwareCheck` requires `request_id`, the selected `device_id` and
`buds_in_case_confirmed`. The daemon also checks model/firmware, a fresh in-case
report and idle audio before its single request write. `hardware_check` carries
the phase and independent per-ear timestamp changes. A job survives radio loss
for the same pair, but cannot write to another selection. `CancelHardwareCheck`
stops waiting for that exact job; it does not cancel the firmware check.
`earbuds_in_case` is unknown after disconnect, a stale link or an expired report.

- [`ipc_server.rs`](../../daemon/src/ipc_server.rs): pipe access, framing and
  per-client I/O.
- [`security.rs`](../../daemon/src/security.rs): current-user and SYSTEM ACL.
- [`main.rs`](../../daemon/src/main.rs): command dispatch, state publication and
  Bluetooth/audio integration.
- [`DaemonClient.cs`](../../winui/NTPods.WinUI/Ipc/DaemonClient.cs): WinUI connection,
  outgoing queue and reconnection.

Volume belongs to the daemon so device controls and Conversation Awareness share
one Windows endpoint state. UI clients never open the AAP or PCM driver handles.
