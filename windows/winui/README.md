# NTPods WinUI client

`ntpods-winui.exe` is the C# / WinUI 3 interface to `ntpodsd.exe`. The daemon owns
the drivers, Bluetooth session and microphone decoding. The app displays its
state and sends commands over named pipes.

Normal launches show the main window; `--tray` starts it hidden. Closing the
window hides it in the tray. **Quit** exits the app and shuts down the daemon.

## Build and run

Use [BUILD-WINDOWS.md](../../docs/BUILD-WINDOWS.md) for the toolchain and build
commands. `windows/build.ps1` builds and tests the app, daemon, drivers and
installer together.

WinUI needs Visual Studio's MSBuild. Plain `dotnet build` / `dotnet publish`
can fail at the Windows App SDK PRI task. Keep the project's package versions
and lock file together; do not change dependencies to work around a missing
build tool.

The app is unpackaged and self-contained. Distribute the whole generated
`winui/` directory, including `ntpods-winui.pri` and the runtime files, alongside
the daemon and its FFmpeg DLLs. A lone executable is not a complete app.

From the generated `winui/` directory:

```powershell
.\ntpods-winui.exe
```

## IPC

The shared contract is in [`windows/ipc/src/lib.rs`](../ipc/src/lib.rs).

| Pipe | Direction | Contents |
|------|-----------|----------|
| `NTPods-events` | daemon to app | NDJSON state, overlay and connection-prompt events |
| `NTPods-cmds` | app to daemon | NDJSON commands |

`Ipc/DaemonClient.cs` handles connection, retries and daemon startup. It looks for
`ntpodsd.exe` beside the client or in its parent directory, matching the generated
distribution. Pipe I/O runs asynchronously; state updates reach WinUI through
`DispatcherQueue`.

For an already linked phone and the selected AirPods, the device page places a
**Listen on** card between the batteries and listening modes. **Phone** and
**This PC** are equal native icon buttons; neither indicates the current audio
owner. A reserved progress slot shows an active transfer. Routine guidance stays
in tooltips and accessible help; an unconfirmed switch shows an inline error.

The controls use the companion link independently of the PC audio connection,
so **This PC** remains usable after handing audio away. They require a ready,
connected companion session for the selected pair and pause during microphone
use or pending playback changes. Each click waits for its matching transfer
result; session loss or selecting another pair clears the pending UI request.
If no terminal receipt arrives within 80 seconds, the UI requests fresh status
once before allowing another explicit choice. It never retries a transfer
automatically.

The card's **Shared settings** expander contains call takeover/return, automatic
connection popups and retained disconnected battery levels. Call takeover starts
off and requires an explicit local choice in both apps; a received preference
cannot grant local consent or phone permissions. Return applies only to a call
that interrupted the PC, and resumes only the same player NTPods paused.
Existing theme, host auto-pause and conversation controls share their matching
preferences without sending displayed remote values back as edits. Saved
unconfirmed accessory choices remain separate from confirmed reports and never
send to the AirPods merely because they were synced. See
[the companion contract](../../docs/COMPANION-PROTOCOL.md) for scope and limits.

Launch `ntpods-winui.exe --attach` to use an already running daemon during a
preview. This mode never starts a daemon, and Quit closes only the UI. Keep the
self-contained build output together; no app installation is needed.

For a receipt investigation, add `--companion-trace`. This opt-in flag writes
at most 64 KiB to `%LOCALAPPDATA%/NTPods/companion-ui.log`, replacing the previous
launch's trace. It records request IDs, phases and UI dispatch decisions, without
device addresses, secrets or message payloads. Logging failures do not affect controls.

## Source map

| Path under `NTPods.WinUI/` | Purpose |
|---------------------------|---------|
| `App.xaml` / `App.xaml.cs` | Startup, tray integration and daemon client |
| `MainWindow.xaml` / `MainWindow.xaml.cs` | Window and navigation |
| `Pages/DevicePage.xaml` / `Pages/DevicePage.xaml.cs` | Device controls and battery display |
| `Ipc/Messages.cs` | C# IPC types and event parsing |
| `Ipc/DaemonClient.cs` | Named-pipe connection and session lifecycle |
| `Tray/` | Tray icon and menu |
| `Services/` | Localization, history, startup and other app services |
| `Strings/` | Translations |
| `Assets/` | Product artwork and application icons |

The UI checks in [`windows/tests`](../tests/README.md) exercise IPC and shared
interaction logic without opening the app. Visual changes also need a native
WinUI check in light and dark themes, including narrow windows and reduced motion.

## Localization

Strings live in `Strings/<culture>/Resources.resw`, with `en-US` as the fallback.
`Services/Loc.cs` loads the embedded resources and updates bindings when the user
changes language. `Localize.Get()` uses the same service for code lookups.

To add a language, translate a copy of `Strings/en-US`, add its embedded resource
to the project, and register the culture in `Loc.cs` and the language selector.
The project also compiles the `.resw` files into `ntpods-winui.pri`; keep that file
in the distribution.

## Design

Follow [DESIGN.md](../../DESIGN.md) and [AGENTS.md](../../AGENTS.md) when changing
the interface. The app uses one focused settings page, rounded
sections, AirPods artwork and the LibrePods-inspired listening control and
notification island. Keep motion, typography and spacing consistent with the
existing controls.
