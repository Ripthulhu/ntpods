using System.IO;
using System.Numerics;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Hosting;
using Microsoft.UI.Xaml.Media;
using Windows.Graphics;
using Windows.UI.ViewManagement;

namespace NTPods.WinUI;

/// The window swaps between the device and settings pages. It wires the daemon
/// client into the pages, forwards daemon events (marshalled to the UI thread) to
/// the device page, and applies the settings page's theme choice to the window
/// root. Closing the window hides it to the tray rather than exiting.
public sealed partial class MainWindow : Window
{
    private readonly DaemonClient _client;
    private readonly UISettings _uiSettings = new();
    private bool _entrancePending;
    private bool _closed;

    public MainWindow(DaemonClient client)
    {
        _client = client;
        InitializeComponent();

        RootGrid.Loaded += (_, _) =>
        {
            if (_entrancePending && AppWindow.IsVisible) AnimateEntrance();
        };
        if (OperatingSystem.IsWindowsVersionAtLeast(10, 0, 19041))
            _uiSettings.AnimationsEnabledChanged += OnAnimationsEnabledChanged;
        Closed += (_, _) =>
        {
            _closed = true;
            FinishEntrance();
            if (OperatingSystem.IsWindowsVersionAtLeast(10, 0, 19041))
                _uiSettings.AnimationsEnabledChanged -= OnAnimationsEnabledChanged;
        };

        RootGrid.ActualThemeChanged += (_, _) => ApplyTitleBarTheme();

        // Hand the client to the pages: DevicePage fans it out to its cards; the
        // SettingsPage uses it for "Refresh state".
        DevicePageView.Client = _client;
        DevicePageView.SettingsRequested += () => ShowSettings(true);
        SettingsPageView.Client = _client;
        DevicePageView.SetDaemonReady(_client.IsReady);

        // The settings theme picker owns the choice; the window root owns the theme.
        SettingsPageView.ThemeChanged += theme =>
        {
            if (RootGrid is not null) RootGrid.RequestedTheme = theme;
        };
        // Apply the saved theme before showing the window.
        if (RootGrid is not null) RootGrid.RequestedTheme = AppSettings.Theme;
        SettingsPageView.InitThemeSelector(AppSettings.ThemeIndex);

        // The experimental opt-in (hearing aid) lives in Settings; the DevicePage owns
        // the card. Refresh its visibility live when the toggle flips.
        SettingsPageView.ExperimentalVisibilityChanged += () => DevicePageView.RefreshExperimentalVisibility();
        SettingsPageView.InitExperimentalSetting();

        AppWindow.Resize(new SizeInt32(820, 880));
        ApplyTitleBarTheme();
        // Enforce a minimum size — the layout breaks if the window is dragged
        // absurdly narrow (no app is usable at ~100px). Clamp on resize.
        AppWindow.Changed += (sender, e) =>
        {
            if (e.DidVisibilityChange && !sender.IsVisible) DevicePageView.CancelShortcutCapture();
            if (!e.DidSizeChange) return;
            const int minW = 420, minH = 540;
            var sz = sender.Size;
            if (sz.Width < minW || sz.Height < minH)
                sender.Resize(new SizeInt32(Math.Max(sz.Width, minW), Math.Max(sz.Height, minH)));
        };
        TrySetWindowIcon();

        Services.Loc.Instance.PropertyChanged += (_, _) =>
            DispatcherQueue.TryEnqueue(() =>
            {
                if (_lastSnapshot is { } s) RenderSnapshot(s);
            });

        // Close hides to tray (the app keeps running as an IPC client).
        AppWindow.Closing += OnClosing;

        // Daemon → UI. These fire on a background thread; marshal to the UI queue.
        _client.SnapshotReceived += OnSnapshot;
        _client.ConnectPromptReceived += OnConnectPrompt;
        _client.ConnectionChanged += OnConnectionChanged;
    }

    private void ApplyTitleBarTheme()
    {
        bool dark = RootGrid.ActualTheme == ElementTheme.Dark;
        var background = dark ? Microsoft.UI.Colors.Black : Windows.UI.Color.FromArgb(255, 242, 242, 247);
        var foreground = dark ? Microsoft.UI.Colors.White : Microsoft.UI.Colors.Black;
        AppWindow.TitleBar.BackgroundColor = background;
        AppWindow.TitleBar.ForegroundColor = foreground;
        AppWindow.TitleBar.ButtonBackgroundColor = background;
        AppWindow.TitleBar.ButtonForegroundColor = foreground;
        AppWindow.TitleBar.InactiveBackgroundColor = background;
        AppWindow.TitleBar.ButtonInactiveBackgroundColor = background;
    }

    private void TrySetWindowIcon()
    {
        try
        {
            var ico = Path.Combine(AppContext.BaseDirectory, "Assets", "app.ico");
            if (File.Exists(ico)) AppWindow.SetIcon(ico);
        }
        catch { }
    }

    /// Show + focus the window (from the tray icon / "Open").
    public void ShowFromTray()
    {
        bool opening = !AppWindow.IsVisible;
        if (opening) _entrancePending = true;
        AppWindow.Show();
        Activate();
        if (_entrancePending && RootGrid.IsLoaded) AnimateEntrance();
    }

    private void AnimateEntrance()
    {
        FinishEntrance();
        if (!_uiSettings.AnimationsEnabled) return;

        var visual = ElementCompositionPreview.GetElementVisual(ShellGrid);
        var compositor = visual.Compositor;
        visual.CenterPoint = new Vector3((float)ShellGrid.ActualWidth / 2, (float)ShellGrid.ActualHeight / 2, 0);
        var ease = compositor.CreateCubicBezierEasingFunction(new Vector2(0.16f, 1), new Vector2(0.3f, 1));
        var scale = compositor.CreateVector3KeyFrameAnimation();
        scale.InsertKeyFrame(0, new Vector3(0.985f, 0.985f, 1));
        scale.InsertKeyFrame(1, Vector3.One, ease);
        scale.Duration = TimeSpan.FromMilliseconds(220);
        var opacity = compositor.CreateScalarKeyFrameAnimation();
        opacity.InsertKeyFrame(0, 0.88f);
        opacity.InsertKeyFrame(1, 1, ease);
        opacity.Duration = TimeSpan.FromMilliseconds(140);
        visual.StartAnimation(nameof(visual.Scale), scale);
        visual.StartAnimation(nameof(visual.Opacity), opacity);
    }

    private void FinishEntrance()
    {
        _entrancePending = false;
        var visual = ElementCompositionPreview.GetElementVisual(ShellGrid);
        visual.StopAnimation(nameof(visual.Scale));
        visual.StopAnimation(nameof(visual.Opacity));
        visual.Scale = Vector3.One;
        visual.Opacity = 1;
    }

    private void OnAnimationsEnabledChanged(UISettings sender, UISettingsAnimationsEnabledChangedEventArgs args) =>
        DispatcherQueue.TryEnqueue(() =>
        {
            if (!_closed && !sender.AnimationsEnabled) FinishEntrance();
        });

    public void SetClosingState(bool closing)
    {
        if (closing) DevicePageView.StopHeadTracking();
        Title = closing ? Localize.Get("Shutdown.Closing") : "NTPods";
        DevicePageView.SetDaemonReady(!closing && _client.IsReady);
    }

    public async Task ShowShutdownFailureAsync()
    {
        ShowFromTray();
        var dialog = new ContentDialog
        {
            XamlRoot = RootGrid.XamlRoot,
            RequestedTheme = RootGrid.RequestedTheme,
            Title = Localize.Get("Shutdown.FailedTitle"),
            Content = Localize.Get("Shutdown.FailedBody"),
            CloseButtonText = Localize.Get("Action_Close"),
        };
        await dialog.ShowAsync();
    }

    private void OnClosing(AppWindow sender, AppWindowClosingEventArgs args)
    {
        args.Cancel = true;   // don't destroy the window…
        DevicePageView.StopHeadTracking();
        FinishEntrance();
        AppWindow.Hide();     // …just hide it back to the tray.
    }

    // ---- Navigation --------------------------------------------------------

    private void ShowSettings(bool settings)
    {
        if (settings) DevicePageView.StopHeadTracking();
        DevicePageView.Visibility = settings ? Visibility.Collapsed : Visibility.Visible;
        SettingsPageView.Visibility = settings ? Visibility.Visible : Visibility.Collapsed;
        SettingsNavigation.Visibility = settings ? Visibility.Visible : Visibility.Collapsed;
        DispatcherQueue.TryEnqueue(() =>
        {
            if (SettingsPageView.Visibility == Visibility.Visible) BackButton.Focus(FocusState.Programmatic);
            else DevicePageView.FocusActions();
        });
    }

    private void Back_Click(object sender, RoutedEventArgs args) => ShowSettings(false);

    // ---- Daemon events (marshalled to the UI thread) -----------------------

    private void OnSnapshot(Snapshot s) =>
        DispatcherQueue.TryEnqueue(() =>
        {
            _lastSnapshot = s;
            RenderSnapshot(s);
        });

    // Language changes also refresh strings set from the last snapshot.
    private void RenderSnapshot(Snapshot s)
    {
            _connected = s.Connected;

            // A live connection dismisses any stale "Connect?" prompt (in-app InfoBar
            // + the centred popup window).
            if (s.Connected)
            {
                DevicePageView.DismissConnectPrompt();
                try { _connectPrompt?.Close(); } catch { }
            }

            DevicePageView.Update(s);
            SettingsPageView.UpdateDeviceInfo(s);
    }

    private Popup.ConnectPromptWindow? _connectPrompt;
    private bool _connected;
    private Snapshot? _lastSnapshot;

    private void OnConnectPrompt(string name) =>
        DispatcherQueue.TryEnqueue(() =>
        {
            // Already connected — a proximity "Connect?" prompt is stale (the daemon
            // can still emit it from a BLE advertisement while the AACP session is
            // live). Suppress both the popup and the in-app InfoBar.
            if (_connected) return;

            // The iOS-style centred "Connect?" popup — shows even when the app is
            // hidden to the tray. Also mirror it in the in-app InfoBar.
            try
            {
                _connectPrompt?.Close();
                _connectPrompt = new Popup.ConnectPromptWindow(name, () => _client.Connect());
                _connectPrompt.Closed += (_, _) => _connectPrompt = null;
                _connectPrompt.ShowPrompt();
            }
            catch { }
            DevicePageView.ShowConnectPrompt(name);
        });

    private void OnConnectionChanged(bool connected) =>
        DispatcherQueue.TryEnqueue(() =>
        {
            DevicePageView.SetDaemonReady(connected && _client.IsReady);
            if (!connected) DevicePageView.ShowWaitingForDaemon();
        });
}
