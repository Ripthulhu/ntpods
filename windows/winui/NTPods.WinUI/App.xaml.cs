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

    // Reuse the island window to avoid WinUI theme teardown on dismissal.
    private IslandWindow? _island;
    private readonly IslandPresence _islandPresence = new();
    private readonly MicrophoneNotifications _micNotifications = new();
    internal GlobalHotkey? MicHotkey { get; private set; }
    internal int MicHotkeyError { get; private set; }
    private Snapshot? _latestSnapshot;
    // Retain artwork identity for messages that arrive without a snapshot.
    private string _lastModel = "";
    private bool _quitting;

    public App()
    {
        // App.xaml creates the Loc instance shared by code and XAML.
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
        try
        {
            MicHotkey = new GlobalHotkey(WinRT.Interop.WindowNative.GetWindowHandle(_window));
            MicHotkey.Pressed += ToggleMicrophone;
            MicHotkey.TrySet(AppSettings.MicHotkey, out int error);
            MicHotkeyError = error;
        }
        catch (InvalidOperationException) { MicHotkeyError = -1; }
        _window.Closed += (_, _) => MicHotkey?.Dispose();

        _tray = new TrayController(
            onOpen: () => _window.ShowFromTray(),
            onQuit: ExitApp,
            onBatteries: () => ShowIsland(_latestSnapshot ?? new Snapshot()),
            client: Daemon);
        _tray.Show();

        Daemon.OverlayReceived += (title, body) =>
            _window.DispatcherQueue.TryEnqueue(() => ShowIslandMessage(title, body));

        // Daemon callbacks arrive on a background thread; tray updates require the UI thread.
        Daemon.SnapshotReceived += s =>
            _window.DispatcherQueue.TryEnqueue(() => _tray?.UpdateSnapshot(s));
        Daemon.ConnectionChanged += connected =>
        {
            if (!connected) _window.DispatcherQueue.TryEnqueue(() =>
            {
                _latestSnapshot = null;
                _micNotifications.Reset();
                _tray?.UpdateSnapshot(new Snapshot());
                _island?.UpdateSnapshot(new Snapshot());
            });
        };

        // Track AirPods connection transitions so reopening the daemon pipe does
        // not repeat the connection popup.
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
                bool connectionMessage = _islandPresence.Update(s, sessionReady);
                string? micMessage = _micNotifications.Update(s, sessionReady);
                if (micMessage is not null) ShowIslandMessage(s.DevName, Localize.Get(micMessage));
                else if (connectionMessage) ShowIsland(s);
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

    internal bool SetMicHotkey(HotkeyGesture? gesture)
    {
        if (MicHotkey is null) { MicHotkeyError = -1; return false; }
        bool registered = MicHotkey.TrySet(gesture, out int error);
        MicHotkeyError = error;
        if (registered) AppSettings.SetMicHotkey(gesture);
        return registered;
    }

    private void ToggleMicrophone()
    {
        if (_quitting) return;
        if (!Daemon.Send(new ToggleMicManualCmd()))
            ShowIslandMessage(_latestSnapshot?.DevName ?? "", Localize.Get("Mic_ShortcutUnavailable"));
    }

    /// Show the connection popup on the UI thread, containing window-creation failures.
    private void ShowIsland(Snapshot snapshot)
    {
        try
        {
            if (_island is null)
            {
                _island = new IslandWindow();
                // Dismissal hides the island; Closed is only a shutdown fallback.
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

    /// Quit waits for daemon shutdown; closing the main window only hides it to the tray.
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
        MicHotkey?.Dispose();
        await Daemon.DisposeAsync();
        Exit();
    }
}
