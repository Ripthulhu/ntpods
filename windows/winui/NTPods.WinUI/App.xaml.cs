using System.Diagnostics;
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
    private readonly IslandTakeover _islandTakeover = new();
    private readonly Stopwatch _islandClock = Stopwatch.StartNew();
    private DispatcherQueueTimer? _islandTimer;
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
        _window.NearbyPairingSucceeded += deviceId =>
        {
            if (_islandTakeover.PairingSucceeded(deviceId)) _island?.DismissConnection();
            RefreshIslandAction();
        };
        _islandTimer = dispatcher.CreateTimer();
        _islandTimer.Interval = TimeSpan.FromMilliseconds(250);
        _islandTimer.Tick += (_, _) => RefreshIslandAction();
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
        {
            ulong epoch = Daemon.SessionEpoch;
            _window.DispatcherQueue.TryEnqueue(() =>
            {
                if (epoch == Daemon.SessionEpoch && Daemon.IsReady) ShowIslandMessage(title, body);
            });
        };
        Daemon.PairedLidReceived += lid =>
        {
            ulong epoch = Daemon.SessionEpoch;
            _window.DispatcherQueue.TryEnqueue(() =>
            {
                if (epoch != Daemon.SessionEpoch || !Daemon.IsReady) return;
                bool? dismiss = _islandTakeover.ObserveLid(lid, _latestSnapshot, _islandClock.ElapsedMilliseconds);
                if (!lid.LidOpen && dismiss is not null)
                {
                    _islandPresence.OnPairedLidClose();
                    if (dismiss == true) _island?.DismissConnection();
                }
                RefreshIslandAction();
            });
        };

        // Daemon callbacks arrive on a background thread; tray updates require the UI thread.
        Daemon.SnapshotReceived += s =>
        {
            ulong epoch = Daemon.SessionEpoch;
            _window.DispatcherQueue.TryEnqueue(() =>
            {
                if (epoch == Daemon.SessionEpoch && Daemon.IsReady) _tray?.UpdateSnapshot(s);
            });
        };
        Daemon.ConnectionChanged += connected =>
        {
            ulong epoch = Daemon.SessionEpoch;
            if (!connected) _window.DispatcherQueue.TryEnqueue(() =>
            {
                if (epoch != Daemon.SessionEpoch) return;
                _latestSnapshot = null;
                _islandTakeover.SessionLost();
                _islandTimer?.Stop();
                _micNotifications.Reset();
                _tray?.UpdateSnapshot(new Snapshot());
                _island?.DismissConnection();
            });
        };

        // Track AirPods connection transitions so reopening the daemon pipe does
        // not repeat the connection popup.
        Daemon.SnapshotReceived += s =>
        {
            ulong epoch = Daemon.SessionEpoch;
            bool sessionReady = Daemon.IsReady;
            _window.DispatcherQueue.TryEnqueue(() =>
            {
                if (!sessionReady || !Daemon.IsReady || epoch != Daemon.SessionEpoch) return;
                bool changedDevice = _latestSnapshot is { DeviceId: { Length: > 0 } previousId } &&
                    !string.Equals(previousId, s.DeviceId, StringComparison.OrdinalIgnoreCase);
                if (changedDevice) _island?.DismissConnection();
                _latestSnapshot = s;
                _islandTakeover.ObserveSnapshot(s, _islandClock.ElapsedMilliseconds);
                if (!string.IsNullOrEmpty(s.Model))
                {
                    _lastModel = s.Model;
                    AppSettings.SetLastModel(s.Model, s.DeviceId); // survive restarts for this device's connect island
                }
                bool connectionMessage = _islandPresence.Update(s, sessionReady, _islandTakeover.MayShow(s));
                string? micMessage = _micNotifications.Update(s, sessionReady);
                if (micMessage is not null) ShowIslandMessage(s.DevName, Localize.Get(micMessage));
                else if (connectionMessage && _islandTakeover.MayShow(s)) ShowIsland(s);
                else _island?.UpdateSnapshot(s);
                RefreshIslandAction();
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
        if (registered && !AppSettings.SetMicHotkey(gesture))
            ShowIslandMessage("", Localize.Get("Settings.SaveFailed"));
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
                _island.ConnectRequested += ConnectFromIsland;
                // Dismissal hides the island; Closed is only a shutdown fallback.
                _island.Closed += (_, _) => _island = null;
            }
            _island.ShowConnected(snapshot);
            _islandTakeover.Shown(snapshot);
            RefreshIslandAction();
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
            _islandTakeover.MessageReplaced();
            _islandTimer?.Stop();
            if (_island is null)
            {
                _island = new IslandWindow();
                _island.ConnectRequested += ConnectFromIsland;
                _island.Closed += (_, _) => _island = null;
            }
            _island.ShowMessage(title, body, _latestSnapshot, _lastModel);
        }
        catch
        {
            _island = null;
        }
    }

    private void ConnectFromIsland()
    {
        if (_latestSnapshot is not { } snapshot || !Daemon.IsReady ||
            !_islandTakeover.BeginConnect(snapshot, _islandClock.ElapsedMilliseconds))
        {
            RefreshIslandAction();
            return;
        }
        if (!Daemon.Send(new ConnectCmd { DeviceId = snapshot.DeviceId })) _islandTakeover.ConnectSendFailed();
        RefreshIslandAction();
    }

    private void RefreshIslandAction()
    {
        if (_latestSnapshot is not { } snapshot || _island is null)
        {
            _islandTimer?.Stop();
            return;
        }
        long now = _islandClock.ElapsedMilliseconds;
        _island.SetConnectState(_islandTakeover.State(snapshot, now), _islandTakeover.InGrace(now));
        if (_islandTakeover.NeedsTimer(now))
        {
            if (_islandTimer is { IsRunning: false }) _islandTimer.Start();
        }
        else _islandTimer?.Stop();
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
