using System;
using System.Runtime.InteropServices;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Windows.Graphics;

namespace NTPods.WinUI.Popup;

/// A borderless, themed, self-dismissing window that hosts <see cref="TrayMenuView"/>
/// as the tray's context menu. Replaces the WinUI MenuFlyout (which clipped to ~1
/// char in H.NotifyIcon's SecondWindow host) and the native Win32 menu (no per-item
/// icons, no app theme). It appears at the cursor, growing up from the tray, and
/// closes when it loses focus or an item is chosen. Every windowing call is guarded.
public sealed class TrayMenuWindow : Window
{
    private const double MenuWidthDip = 260;
    private const double FallbackHeightDip = 420;

    private readonly TrayMenuView _view = new();
    private readonly DaemonClient _client;
    private readonly Action _onOpen;
    private readonly Action _onQuit;
    private bool _closed;
    private bool _positioned;
    private PointInt32 _anchor;
    private DisplayArea? _display;

    public TrayMenuWindow(DaemonClient client, Action onOpen, Action onQuit, Action? onBatteries = null)
    {
        _client = client;
        _onOpen = onOpen;
        _onQuit = onQuit;

        Content = _view;
        Title = "NTPods menu";

        _view.OnAnc = a => _client.SetAnc(a);
        _view.OnMute = () => _client.ToggleMute();
        _view.OnOpen = () => _onOpen();
        _view.OnBatteries = onBatteries;
        _view.OnQuit = () => _onQuit();
        _view.OnDismiss = SafeClose;
        _view.AddHandler(UIElement.KeyDownEvent, new KeyEventHandler((_, e) =>
        {
            if (e.Key != Windows.System.VirtualKey.Escape) return;
            e.Handled = true;
            SafeClose();
        }), true);

        try { ConfigurePresenter(); } catch { }
        try { SystemBackdrop = new DesktopAcrylicBackdrop(); } catch { }

        _view.Loaded += OnViewLoaded;
        Activated += OnActivated;
        Closed += (_, _) => _closed = true;
    }

    /// Populate + show the menu at the current cursor position.
    public void UpdateSnapshot(Snapshot snapshot)
    {
        if (_closed) return;
        _view.Apply(snapshot);
        if (_positioned) PositionMenu();
    }

    public void ShowAt(Snapshot snapshot)
    {
        if (_closed) return;
        try
        {
            _view.RequestedTheme = AppSettings.Theme;
            _view.Apply(snapshot);
            try { ApplyExStyles(); } catch { }
            try { RoundCorners(); } catch { }
            if (!GetCursorPos(out var cursor)) { SafeClose(); return; }
            _anchor = new PointInt32(cursor.X, cursor.Y);
            _display = DisplayArea.GetFromPoint(_anchor, DisplayAreaFallback.Nearest);
            // Destination DPI must be selected before measuring the menu.
            AppWindow.Move(_anchor);
            // Realize the HWND + lay out the content; final placement happens in
            // OnViewLoaded once the rasterization scale is known.
            AppWindow.Show(true);
        }
        catch { SafeClose(); }
    }

    private void OnViewLoaded(object sender, RoutedEventArgs e) => PositionMenu();

    private void PositionMenu()
    {
        if (_closed) return;
        try
        {
            double scale = GetDpiForWindow(WinRT.Interop.WindowNative.GetWindowHandle(this)) / 96.0;
            if (scale <= 0) scale = 1.0;

            var work = (_display ?? DisplayArea.Primary).WorkArea;
            _view.Width = Math.Min(MenuWidthDip, work.Width / scale);
            _view.MaxHeight = work.Height / scale;
            _view.Measure(new Windows.Foundation.Size(_view.Width, _view.MaxHeight));
            double heightDip = _view.DesiredSize.Height > 0 ? _view.DesiredSize.Height : FallbackHeightDip;
            double widthDip = _view.DesiredSize.Width > 0 ? _view.DesiredSize.Width : MenuWidthDip;

            var bounds = TrayMenuLayout.Fit(_anchor.X, _anchor.Y,
                (int)Math.Ceiling(widthDip * scale), (int)Math.Ceiling(heightDip * scale),
                work.X, work.Y, work.Width, work.Height);
            if (AppWindow.Size.Width != bounds.Width || AppWindow.Size.Height != bounds.Height)
                AppWindow.Resize(new SizeInt32(bounds.Width, bounds.Height));
            if (AppWindow.Position.X != bounds.X || AppWindow.Position.Y != bounds.Y)
                AppWindow.Move(new PointInt32(bounds.X, bounds.Y));
            // Take focus so the first click-away deactivates (and dismisses) us.
            if (!_positioned)
                try { SetForegroundWindow(WinRT.Interop.WindowNative.GetWindowHandle(this)); } catch { }
            _positioned = true;
        }
        catch { SafeClose(); }
    }

    private void OnActivated(object sender, WindowActivatedEventArgs args)
    {
        // Only dismiss on genuine focus loss after we've placed + shown the menu.
        if (_positioned && args.WindowActivationState == WindowActivationState.Deactivated)
            SafeClose();
    }

    private void ConfigurePresenter()
    {
        AppWindow.IsShownInSwitchers = false;
        if (AppWindow.Presenter is OverlappedPresenter p)
        {
            p.SetBorderAndTitleBar(false, false);
            p.IsAlwaysOnTop = true;
            p.IsResizable = false;
            p.IsMaximizable = false;
            p.IsMinimizable = false;
        }
    }

    private void SafeClose()
    {
        if (_closed) return;
        try { Close(); } catch { _closed = true; }
    }

    // ---- Win32 ex-styles + rounded corners + cursor -----------------------

    private void ApplyExStyles()
    {
        var hwnd = WinRT.Interop.WindowNative.GetWindowHandle(this);
        nint ex = GetWindowLongPtr(hwnd, GWL_EXSTYLE);
        ex |= WS_EX_TOOLWINDOW; // no taskbar / Alt-Tab (but stays activatable)
        SetWindowLongPtr(hwnd, GWL_EXSTYLE, ex);
    }

    private void RoundCorners()
    {
        var hwnd = WinRT.Interop.WindowNative.GetWindowHandle(this);
        int pref = DWMWCP_ROUND;
        DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, ref pref, sizeof(int));
    }

    private const int GWL_EXSTYLE = -20;
    private const nint WS_EX_TOOLWINDOW = 0x00000080;
    private const int DWMWA_WINDOW_CORNER_PREFERENCE = 33;
    private const int DWMWCP_ROUND = 2;

    [StructLayout(LayoutKind.Sequential)]
    private struct POINT { public int X; public int Y; }

    [DllImport("user32.dll")]
    private static extern bool GetCursorPos(out POINT lpPoint);

    [DllImport("user32.dll")]
    private static extern uint GetDpiForWindow(nint hwnd);

    [DllImport("user32.dll")]
    private static extern bool SetForegroundWindow(nint hWnd);

    [DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW", SetLastError = true)]
    private static extern nint GetWindowLongPtr(nint hWnd, int nIndex);

    [DllImport("user32.dll", EntryPoint = "SetWindowLongPtrW", SetLastError = true)]
    private static extern nint SetWindowLongPtr(nint hWnd, int nIndex, nint dwNewLong);

    [DllImport("dwmapi.dll", SetLastError = true)]
    private static extern int DwmSetWindowAttribute(nint hwnd, int attribute, ref int pvAttribute, int cbAttribute);
}
