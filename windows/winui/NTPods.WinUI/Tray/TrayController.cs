using System.Drawing;
using System.Drawing.Drawing2D;
using System.Drawing.Text;
using System.IO;
using System.Runtime.InteropServices;
using System.Windows.Input;
using H.NotifyIcon;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Popup;
using NTPods.WinUI.Services;
using Microsoft.UI.Xaml.Media.Imaging;

namespace NTPods.WinUI.Tray;

/// The system-tray presence, built on H.NotifyIcon.WinUI. Double-click (or the
/// menu's "Open") shows the main window; right-click opens a custom themed menu
/// (TrayMenuWindow — the WinUI MenuFlyout clipped and the native Win32 menu can't
/// carry icons or follow the theme). The tooltip + icon badge are driven live from
/// the daemon's Snapshot; the icon shows the lower bud's battery % as a number.
///
/// Because the TaskbarIcon is created outside the visual tree, we call ForceCreate().
public sealed class TrayController : IDisposable
{
    private readonly TaskbarIcon _icon;
    private readonly DaemonClient _client;
    private readonly Action _onOpen;
    private readonly Action _onQuit;
    private readonly Action? _onBatteries;

    // Latest snapshot, so the menu (built on right-click) reflects current state.
    private Snapshot? _lastSnapshot;
    private TrayMenuWindow? _menuWindow;

    // Icon lifetime: the plain NTPods icon is persistent; each number badge is a
    // freshly generated GDI icon whose HICON we must destroy when it's replaced.
    private readonly Icon? _baseIcon;
    private Icon? _generatedIcon;
    private IntPtr _generatedHandle;
    private bool _badgeInitialized;
    private byte? _badgeLevel;
    private bool _badgeLight;

    [DllImport("user32.dll", SetLastError = true)]
    private static extern bool DestroyIcon(IntPtr hIcon);

    public TrayController(Action onOpen, Action onQuit, DaemonClient client, Action? onBatteries = null)
    {
        _client = client;
        _onOpen = onOpen;
        _onQuit = onQuit;
        _onBatteries = onBatteries;

        _icon = new TaskbarIcon
        {
            ToolTipText = "NTPods",
            // Right-click opens our custom themed menu window instead of a flyout.
            ContextFlyout = null,
            RightClickCommand = new RelayCommand(ShowMenu),
            IconSource = new BitmapImage(new Uri("ms-appx:///Assets/tray.ico")),
            // Double-click opens the app; single click is left to the OS so a
            // double-click isn't consumed early.
            DoubleClickCommand = new RelayCommand(onOpen),
        };

        // Load the plain icon from disk for the "no number" fallback.
        try
        {
            var path = Path.Combine(AppContext.BaseDirectory, "Assets", "tray.ico");
            _baseIcon = File.Exists(path) ? new Icon(path) : (Icon)SystemIcons.Application.Clone();
        }
        catch { _baseIcon = (Icon)SystemIcons.Application.Clone(); }
    }

    public void Show() => _icon.ForceCreate();

    /// Open the custom tray menu at the cursor, from the latest snapshot.
    private void ShowMenu()
    {
        try
        {
            try { _menuWindow?.Close(); } catch { }
            var menu = new TrayMenuWindow(_client, _onOpen, _onQuit, _onBatteries);
            _menuWindow = menu;
            menu.Closed += (_, _) => { if (ReferenceEquals(_menuWindow, menu)) _menuWindow = null; };
            menu.ShowAt(_lastSnapshot ?? new Snapshot());
        }
        catch { }
    }

    /// Refresh the tooltip + icon badge from the latest snapshot, and cache it for
    /// the menu. MUST be called on the UI thread (the App marshals via DispatcherQueue).
    public void UpdateSnapshot(Snapshot s)
    {
        _lastSnapshot = s;
        _menuWindow?.UpdateSnapshot(s);

        var name = string.IsNullOrWhiteSpace(s.DevName) ? "NTPods" : s.DevName;
        bool live = BatteryPresentation.IsLive(s);
        string statusKey = BatteryPresentation.StatusKey(s);
        string status = Localize.Get(statusKey);
        if (!live && BatteryPresentation.HasData(s)) status += " · " + Localize.Get("Battery.LastKnown");
        else if (BatteryPresentation.IsCharging(s) && statusKey != "Battery.NearbyCharging")
            status += " · " + Localize.Get("Battery_Charging");
        var parts = new List<string>();
        foreach (var (part, label) in new[] { (BatteryPart.Left, Localize.Get("Battery_LeftShort")),
            (BatteryPart.Right, Localize.Get("Battery_RightShort")), (BatteryPart.Headphone, ""), (BatteryPart.Case, Localize.Get("Tray_CaseShort")) })
            if (BatteryPresentation.ReadingText(s, part, label) is { Length: > 0 } text) parts.Add(text);
        string tip = $"{status}\n{Clip(name, 40)}";
        if (parts.Count > 0) tip += "\n" + string.Join("  ", parts);
        if (s.Connected && !s.LinkStale && AncName(s.Anc) is string mode) tip += " · " + mode;
        // Win32 NOTIFYICONDATA tooltip caps at 127 chars.
        _icon.ToolTipText = Clip(tip, 127);

        // Icon badge: lower present bud (min of L/R), else plain icon.
        UpdateIconBadge(BatteryPresentation.LiveEarbudReading(s));
    }

    /// The localized noise-control mode name for anc 1..4, or null if unknown.
    private static string? AncName(byte anc) => anc switch
    {
        1 => Localize.Get("Anc_Off"),
        2 => Localize.Get("Anc_NoiseCancellation"),
        3 => Localize.Get("Anc_Transparency"),
        4 => Localize.Get("Anc_Adaptive"),
        _ => null,
    };

    private void UpdateIconBadge(byte? level)
    {
        bool light = level is not null && LightTaskbar();
        if (_badgeInitialized && _badgeLevel == level && _badgeLight == light) return;
        if (level is not byte v)
        {
            if (_baseIcon is not null && SetIcon(_baseIcon, IntPtr.Zero)) RememberBadge(null, light);
            return;
        }

        try
        {
            var (icon, handle) = RenderNumberIcon(v.ToString(), light);
            if (SetIcon(icon, handle)) RememberBadge(level, light);
        }
        catch
        {
            _badgeInitialized = false;
            if (_baseIcon is not null) SetIcon(_baseIcon, IntPtr.Zero);
        }
    }

    private static string Clip(string value, int limit)
    {
        if (value.Length <= limit) return value;
        int length = limit - 1;
        if (char.IsHighSurrogate(value[length - 1])) length--;
        return value[..length] + "…";
    }

    private void RememberBadge(byte? level, bool light)
    {
        _badgeInitialized = true;
        _badgeLevel = level;
        _badgeLight = light;
    }

    /// Assign a new tray icon and release the previously generated one (never the
    /// persistent base icon, which is passed with a zero handle).
    private bool SetIcon(Icon icon, IntPtr generatedHandle)
    {
        try { _icon.Icon = icon; }
        catch
        {
            if (generatedHandle != IntPtr.Zero) { icon.Dispose(); DestroyIcon(generatedHandle); }
            return false;
        }

        _generatedIcon?.Dispose();
        if (_generatedHandle != IntPtr.Zero) DestroyIcon(_generatedHandle);

        _generatedIcon = generatedHandle == IntPtr.Zero ? null : icon;
        _generatedHandle = generatedHandle;
        return true;
    }

    /// Draw the battery number as LARGE as it fits, centred on a 64px transparent
    /// bitmap for a crisp tray downscale. Returns the Icon + its backing HICON.
    private static (Icon icon, IntPtr handle) RenderNumberIcon(string text, bool lightTaskbar)
    {
        const int size = 64;
        using var bmp = new Bitmap(size, size);
        using (var g = Graphics.FromImage(bmp))
        {
            g.SmoothingMode = SmoothingMode.AntiAlias;
            g.TextRenderingHint = TextRenderingHint.AntiAliasGridFit;
            g.Clear(Color.Transparent);

            using var brush = new SolidBrush(lightTaskbar ? Color.FromArgb(255, 32, 32, 32) : Color.White);
            using var fmt = new StringFormat
            {
                Alignment = StringAlignment.Center,
                LineAlignment = StringAlignment.Center,
            };

            // Shrink from a large face until the number nearly fills the box, so a
            // big "9" and a big "100" both read large in the tray.
            float emSize = 64f;
            while (emSize > 12f)
            {
                using var probe = new Font("Segoe UI", emSize, FontStyle.Bold, GraphicsUnit.Pixel);
                var m = g.MeasureString(text, probe);
                if (m.Width <= size * 1.0f && m.Height <= size) break;
                emSize -= 2f;
            }
            using var font = new Font("Segoe UI", emSize, FontStyle.Bold, GraphicsUnit.Pixel);
            g.DrawString(text, font, brush, new RectangleF(0, 0, size, size), fmt);
        }

        var handle = bmp.GetHicon();
        return (Icon.FromHandle(handle), handle);
    }

    /// True when the taskbar uses the light theme (→ draw dark text). Defaults to
    /// false (dark taskbar, white text) — the Windows 11 default — if unreadable.
    private static bool LightTaskbar()
    {
        try
        {
            using var key = Microsoft.Win32.Registry.CurrentUser.OpenSubKey(
                @"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
            return key?.GetValue("SystemUsesLightTheme") is int i && i != 0;
        }
        catch
        {
            return false;
        }
    }

    public void Dispose()
    {
        _icon.Dispose();
        _generatedIcon?.Dispose();
        if (_generatedHandle != IntPtr.Zero) DestroyIcon(_generatedHandle);
        _baseIcon?.Dispose();
    }

    /// Minimal ICommand so the tray's clicks can invoke an Action.
    private sealed class RelayCommand(Action execute) : ICommand
    {
        public event EventHandler? CanExecuteChanged { add { } remove { } }
        public bool CanExecute(object? parameter) => true;
        public void Execute(object? parameter) => execute();
    }
}
