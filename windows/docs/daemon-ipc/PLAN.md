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
full state snapshots, command results, overlays and head movement. Change both
definitions together and run [`test-ipc.ps1`](../../tests/test-ipc.ps1).

## Connection lifecycle

The app starts the daemon if needed and reconnects after a lost pipe. The daemon
is single-instance and sends a state snapshot when an event client connects.
The client also sends `Hello` and `GetState`, then waits for a snapshot before
treating the session as ready. Idle command connections send `Ping` every two
seconds to detect a broken command pipe even if events are still arriving.

Closing a pipe releases that client's resources; it does not stop the daemon.
`Shutdown` requests daemon cleanup and exit. Head-tracking requests belong to the
command connection that made them, so losing that connection releases its stream.

## Implementation

- [`ipc_server.rs`](../../daemon/src/ipc_server.rs): pipe access, framing and
  per-client I/O.
- [`security.rs`](../../daemon/src/security.rs): current-user and SYSTEM ACL.
- [`main.rs`](../../daemon/src/main.rs): command dispatch, state publication and
  Bluetooth/audio integration.
- [`DaemonClient.cs`](../../winui/NTPods.WinUI/Ipc/DaemonClient.cs): WinUI connection,
  outgoing queue and reconnection.

Volume belongs to the daemon so device controls and Conversation Awareness share
one Windows endpoint state. UI clients never open the AAP or PCM driver handles.
