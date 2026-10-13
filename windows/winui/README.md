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
