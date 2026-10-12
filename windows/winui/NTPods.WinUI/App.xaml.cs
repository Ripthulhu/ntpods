using NTPods.WinUI.Ipc;
using NTPods.WinUI.Popup;
using NTPods.WinUI.Services;
using NTPods.WinUI.Tray;
using Microsoft.UI.Xaml;
using Microsoft.UI.Dispatching;
using Microsoft.Windows.AppLifecycle;

namespace NTPods.WinUI;

/// Tray client of ntpodsd. Normal launches show the window; --tray keeps it hidden.
public partial class App : Application
{
    public DaemonClient Daemon { get; } = new();

    private MainWindow? _window;
    private TrayController? _tray;
    private AppInstance? _instance;

    // Reuse one island window and show connection notifications on false-to-true transitions.
    private IslandWindow? _island;
    private readonly IslandPresence _islandPresence = new();
    private Snapshot? _latestSnapshot;
    // Last-seen model number (from the 0x1D metadata), so message-mode island
    // popups can show the right device render even without a full Snapshot.
    private string _lastModel = "";
    private bool _quitting;

    public App()
    {
        // InitializeComponent loads App.xaml, which creates the single Loc instance
        // (<services:Loc x:Key="Loc"/>); its ctor publishes Loc.Instance for code.
        InitializeComponent();
    }

    protected override async void OnLaunched(LaunchActivatedEventArgs args)
    {
        _instance = AppInstance.FindOrRegisterForKey("NTPods.WinUI");
        if (!_instance.IsCurrent)
        {
            await _instance.RedirectActivationToAsync(AppInstance.GetCurrent().GetActivatedEventArgs());
            Exit();
            return;
        }
        var dispatcher = DispatcherQueue.GetForCurrentThread();
        _instance.Activated += (_, _) => dispatcher.TryEnqueue(() => _window?.ShowFromTray());

        // The main window is created hidden; closing it hides back to the tray.
        _window = new MainWindow(Daemon);

        _tray = new TrayController(
            onOpen: () => _window.ShowFromTray(),
            onQuit: ExitApp,
            onBatteries: () => ShowIsland(_latestSnapshot ?? new Snapshot()),
            client: Daemon);
        _tray.Show();

        // Surface daemon overlays as the centred floating island (not a Windows
        // toast — the toast was the corner card the user didn't want). Marshalled
        // to the UI thread.
        Daemon.OverlayReceived += (title, body) =>
            _window.DispatcherQueue.TryEnqueue(() => ShowIslandMessage(title, body));

        // Drive the tray's tooltip / menu / icon badge from daemon state. Snapshots
        // arrive on a background thread; marshal to the UI thread (same as the window).
        Daemon.SnapshotReceived += s =>
            _window.DispatcherQueue.TryEnqueue(() => _tray?.UpdateSnapshot(s));
        Daemon.ConnectionChanged += connected =>
        {
            if (!connected) _window.DispatcherQueue.TryEnqueue(() =>
            {
                _latestSnapshot = null;
                _tray?.UpdateSnapshot(new Snapshot());
                _island?.UpdateSnapshot(new Snapshot());
            });
        };

        // Show the connection island on the AirPods connect transition only
        // (Snapshot.Connected false→true). Tracking the snapshot's connected flag
        // — rather than the pipe-level ConnectionChanged — means a daemon pipe
        // reconnect (which re-pushes the same connected=true snapshot) does not
        // re-fire the popup. Marshalled to the UI thread.
        Daemon.SnapshotReceived += s =>
        {
            bool sessionReady = Daemon.IsReady;
            _window.DispatcherQueue.TryEnqueue(() =>
            {
                _latestSnapshot = s;
                if (!string.IsNullOrEmpty(s.Model))
                {
                    _lastModel = s.Model;
                    AppSettings.SetLastModel(s.Model, s.DeviceId); // survive restarts for this device's connect island
                }
                if (_islandPresence.Update(s, sessionReady)) ShowIsland(s);
                else _island?.UpdateSnapshot(s);
            });
        };

        // Reuse a running daemon so opening the app does not interrupt capture.
        Daemon.Start();

        // Startup opens only the tray; manual launches share the tray's entrance.
        var argv = Environment.GetCommandLineArgs();
        bool startHidden = Array.IndexOf(argv, "--tray") >= 0
                        || Array.IndexOf(argv, "--minimized") >= 0;
        if (!startHidden) _window.ShowFromTray();
    }

    /// Create (or reuse) the single island window and play the connect popup.
    /// Must run on the UI thread. Fully guarded — a popup failure never crashes
    /// the app; at worst there is simply no island.
    private void ShowIsland(Snapshot snapshot)
    {
        try
        {
            if (_island is null)
            {
                _island = new IslandWindow();
                // The island hides + reuses itself after each slide-out (it never
                // calls Window.Close, which fail-fasts in WinUI's theme teardown).
                // Keep the Closed handler only as a shutdown safety net.
                _island.Closed += (_, _) => _island = null;
            }
            _island.ShowConnected(snapshot);
        }
        catch
        {
            _island = null;
        }
    }

    /// Show a daemon overlay as the centred floating island (message mode).
    private void ShowIslandMessage(string title, string body)
    {
        // The daemon follows its first battery snapshot with this connection event.
        if (body.StartsWith(Localize.Get("Status_Connected") + "  ·  ", StringComparison.Ordinal) && _latestSnapshot is { Connected: true } snapshot)
        {
            _island?.UpdateSnapshot(snapshot);
            return;
        }
        try
        {
            if (_island is null)
            {
                _island = new IslandWindow();
                _island.Closed += (_, _) => _island = null;
            }
            _island.ShowMessage(title, body, _latestSnapshot, _lastModel);
        }
        catch
        {
            _island = null;
        }
    }

    /// The tray "Quit": stop the daemon too and exit. On Windows one front-end
    /// runs at a time, and this app spawned the daemon, so quitting it should not
    /// leave a headless ntpodsd lingering. (Closing the *window* only hides to
    /// the tray — the daemon stays; Quit is the full teardown.)
    public async void ExitApp()
    {
        if (_quitting) return;
        _quitting = true;
        _window?.SetClosingState(true);
        try
        {
            await Daemon.ShutdownAsync();
        }
        catch (Exception)
        {
            _quitting = false;
            _window?.SetClosingState(false);
            if (_window is not null) await _window.ShowShutdownFailureAsync();
            return;
        }
        _tray?.Dispose();
        await Daemon.DisposeAsync();
        Exit();
    }
}
