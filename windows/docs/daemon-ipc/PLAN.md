# Windows daemon and IPC: implementation history

This records the original daemon migration. The Rust tray and iced front-ends
described below were replaced by WinUI. For the current wire contract, see
[`windows/ipc/src/lib.rs`](../../ipc/src/lib.rs); for supported behavior, see
[Windows features](../../../docs/WINDOWS-FEATURES.md). The current transport lives
in [`ipc_server.rs`](../../daemon/src/ipc_server.rs); `main.rs` owns command
dispatch and starts the event and command servers.

The old tray and iced app each opened the exclusive driver handle. Only one could
run at a time, requiring an "Open App" handoff and duplicating AAP code.
The migration put driver ownership, the AAP session and microphone decoding in
`ntpodsd`, with the interfaces connected through IPC.

```
                 ┌─ old tray           (UI client, light)
ntpodsd.exe ──┤       IPC: \\.\pipe\NTPods  (NDJSON)
(owns the driver)└─ iced app           (UI client, iced GUI)
```

## Planned components

- **`ntpodsd`** (new, `windows/daemon/`) — headless. Owns the driver
  handle + AAP session (today's tray `run_receiver`), the mic pipeline (decode
  AAC-ELD → feed `\\.\NTPodsMic`), the auto-activate poll + A2DP reset, and
  the dynamic-rename trigger. Holds the **authoritative state**. Runs an IPC
  server. Single-instance (named mutex).
- **`ntpods-ipc`** (new lib crate) — the shared `Command` / `Event` serde
  types + a tiny NDJSON framing helper, so daemon and clients agree on the wire.
- **the old tray** (refactored) — pure UI client: connects to the pipe
  (spawns the daemon if absent), renders the icon/menu/overlay from daemon
  events, sends commands. Its `driver`/`aap`/`eld`/`micpipe`/`a2dp` modules
  **move into the daemon**.
- **the iced app** (Windows) — becomes an IPC client too
  (Phase 3): its `platform/windows` backend talks to the daemon instead of the
  driver directly. On **Linux nothing changes** (still `bluer`, no daemon).

## IPC protocol

- **Transport:** Windows **named pipe** `\\.\pipe\NTPods`, duplex, message
  mode, **multi-instance** (one pipe instance per connected client). Access
  restricted to the current user.
- **Framing:** newline-delimited JSON (NDJSON) — one `serde_json` value per line.
- **Client → Daemon `Command`:**
  - `Hello { kind: "tray" | "app" }` — sent on connect; daemon replies with a
    full `State` snapshot.
  - `SetAnc(u8)` (1..=4)
  - `SetMicMode { auto: bool, manual: bool }`
  - `GetState`
  - (volume was planned as **client-side** WASAPI; it ended up in the daemon
    instead — see the Status section, `SetVolume` / `StepVolume` / `ToggleMute`.)
- **Daemon → Client `Event`:**
  - `State(Snapshot { connected, battery{l,r,case,headphone}, anc, dev_name,
    mic_recording, auto_mode })` — pushed on every change, and once on connect.
  - `Overlay { title, body }` — a notification for the client to render (the
    daemon decides *when*; the tray/app draw it with their overlay UI).

## Lifecycle

- **Start:** the tray autostarts at login (as now) and **ensures the daemon is
  running** — if the pipe isn't there, it spawns `ntpodsd.exe`, then connects.
  This avoided a separate daemon autostart entry in the original plan.
- **Single-instance:** the daemon holds a named mutex; a second spawn exits.
- **Death/restart:** a client that sees the pipe drop retries/reconnects (and
  re-spawns the daemon if needed). The daemon keeps running when UIs close.
- **Shutdown:** closing a UI just disconnects its pipe; the daemon lives on.
  (A tray "Quit NTPods" can send a `Shutdown` that stops the daemon too.)

## Migration steps

1. **Daemon core.** New `ntpodsd` + `ntpods-ipc`. Move `run_receiver` +
   the auto-activate poll + the mic pipeline + `State` out of the tray into the
   daemon. Add the NDJSON named-pipe server; broadcast `State`/`Overlay`. Test
   the daemon standalone (it logs; the mic + battery work with no UI).
2. **Tray → client.** Strip the tray's driver/aap/eld/micpipe/a2dp; it connects
   to the daemon, renders from `Event`s, sends `Command`s, spawns the daemon if
   absent. This removes the tray's exclusive-handle handoff.
3. **App → client.** Route app's `platform/windows` L2CAP/session
   backend through the daemon IPC. Now tray + full app coexist. (Heaviest phase —
   touches the shared cross-platform code; Linux path untouched.)
4. **Polish.** Daemon single-instance + autostart-on-demand + reconnect; installer
   ships `ntpodsd.exe` and drops the exclusive-handoff shortcut logic.

## Initial implementation and hardware checks

- **`ntpodsd`** owns both drivers, the AAP session and the hi-res mic; the UI is
  a thin IPC client. Confirmed on hardware: battery / ANC / volume / mic / ear
  detection / hearing aid all shown and controlled over IPC, auto-reconnect,
  overlay cards.
- **IPC = two one-directional named pipes** (`PIPE_EVENTS` daemon→client,
  `PIPE_CMDS` client→daemon), NDJSON, **async** (per-connection queue + writer
  thread each side). Two failures determined this design:
  1. **Sync-handle serialization / deadlock** — a single *duplex* pipe deadlocked:
     a Windows synchronous handle serializes I/O, so the daemon's blocking
     ReadFile (commands) stalled its WriteFile (events) on the same handle (it
     wrote 2 messages then hung). Split into two one-directional pipes.
  2. **UI freeze** — a synchronous blocking WriteFile on the tray's UI thread
     froze the menu. Both sides now decouple I/O onto their own threads.
  - The original pipe descriptor denied the client access. The current daemon
    grants access to its intended Windows user and SYSTEM; see
    [`security.rs`](../../daemon/src/security.rs).
- **Volume moved into the daemon** (`daemon/src/volume.rs`, WASAPI
  `IAudioEndpointVolume`), against the original plan: the daemon is then the single
  arbiter, reports volume in the `Snapshot`, serves `SetVolume` / `StepVolume` /
  `ToggleMute`, and can duck for Conversational Awareness without an IPC round-trip.
- **Additional commands** — ANC, feature toggles
  (conversational awareness, adaptive volume, allow-off), per-control values, mic
  mode, hearing aid, heart rate (experimental), rename, connect / disconnect /
  repair, shutdown. See `ipc/src/lib.rs` for the wire types.
- **BLE proximity**: the initial `le.rs` implementation watched passively while
  disconnected. It first emitted `Event::ConnectPrompt`, then gained automatic
  connection with a >45 s absence required to re-arm it.
- **WinUI client**: WinUI replaced the Rust tray and iced front-ends and includes
  its own tray icon. The daemon still accepts multiple IPC clients.

## Original microphone implementation

Our Windows hi-res mic uses the **same** AAP commands (`START_AUDIO`/`STOP_AUDIO`,
0x58), AAC-ELD params (64 kHz true → resample 48 kHz, 480-sample/7.5 ms mono, ASC
`F8 E6 30 00`), 0x58 framing (22-byte AU header), and decoder (FFmpeg libavcodec,
LGPL) as Linux [PR #655](https://github.com/librepods-org/librepods/pull/655).
The first implementation added ×3 make-up gain with a tanh soft-limit because
recordings were quiet. Its stall handler resent `START_AUDIO` in place, at most
once per 5 s, rather than closing the channel on silence. These are historical
settings; current capture and recovery behavior is documented in
[Windows features](../../../docs/WINDOWS-FEATURES.md).

## Risks / notes

- **Multi-client pipe:** the daemon serves several pipe instances at once and
  broadcasts to all — one reader thread per client + a shared broadcast channel.
- **Mic ownership:** the daemon is the single owner of both the AAP driver and
  `\\.\NTPodsMic`, so no UI can take the handles from under it.
- **Modules:** `aap` / `driver` / `eld` / `micpipe` / `a2dp` / `volume` live in
  the daemon.
