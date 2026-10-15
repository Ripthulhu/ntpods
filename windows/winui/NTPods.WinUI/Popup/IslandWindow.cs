using System.Diagnostics;
using NTPods.WinUI.Ipc;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Media;
using Windows.Graphics;
using Windows.UI.ViewManagement;

namespace NTPods.WinUI.Popup;

/// One non-activating capsule, reused for connection and control notifications.
public sealed class IslandWindow : Window
{
    private readonly IslandView _view = new();
    private readonly IslandMotion _motion = new();
    private readonly UISettings _uiSettings = new();
    private readonly Stopwatch _clock = Stopwatch.StartNew();
    private readonly DispatcherQueueTimer _dismissTimer;
    private bool _rendering;
    private (int Left, int Width, int Height, int Diameter)? _lastRegion;
    private int _width;
    private int _height;
    private double _scale = 1;
    private DisplayArea? _display;
    private bool _closed;

    public IslandWindow()
    {
        Content = _view;
        Title = "NTPods";
        PopupWindowNative.Configure(this, interactive: false);

        _dismissTimer = DispatcherQueue.CreateTimer();
        _dismissTimer.IsRepeating = false;
        _dismissTimer.Tick += (_, _) => AdvanceFrame();
        _view.Loaded += (_, _) =>
        {
            if (!_motion.Visible) return;
            try { Prepare(); AdvanceFrame(); } catch { Hide(); }
        };
        if (OperatingSystem.IsWindowsVersionAtLeast(10, 0, 19041))
            _uiSettings.AnimationsEnabledChanged += OnAnimationsEnabledChanged;
        Closed += (_, _) =>
        {
            _closed = true;
            _dismissTimer.Stop();
            StopRendering();
            if (OperatingSystem.IsWindowsVersionAtLeast(10, 0, 19041))
                _uiSettings.AnimationsEnabledChanged -= OnAnimationsEnabledChanged;
        };
    }

    public void ShowConnected(Snapshot snapshot)
    {
        if (_closed) return;
        try { _view.Apply(snapshot, _clock.Elapsed.TotalMilliseconds, _uiSettings.AnimationsEnabled); Show(); } catch { Hide(); }
    }

    public void ShowMessage(string title, string body, Snapshot? snapshot = null, string? model = null)
    {
        if (_closed) return;
        try { _view.ApplyMessage(title, body, snapshot, model, _clock.Elapsed.TotalMilliseconds, _uiSettings.AnimationsEnabled); Show(); } catch { Hide(); }
    }

    public void UpdateSnapshot(Snapshot snapshot)
    {
        if (_closed || !_motion.Visible) return;
        try
        {
            _view.UpdateSnapshot(snapshot, _clock.Elapsed.TotalMilliseconds, _uiSettings.AnimationsEnabled);
            Prepare();
            AdvanceFrame();
        }
        catch { Hide(); }
    }

    private void Show()
    {
        _view.RequestedTheme = Services.AppSettings.Theme;
        var display = DisplayArea.Primary;
        bool changedDisplay = _display is null || !_display.DisplayId.Equals(display.DisplayId);
        _display = display;
        if (changedDisplay)
        {
            // Move hidden so destination DPI is current before measuring the content.
            AppWindow.Hide();
            var work = display.WorkArea;
            AppWindow.Move(new PointInt32(work.X + work.Width / 2, work.Y));
        }
        Prepare();
        _motion.Show(_clock.Elapsed.TotalMilliseconds, _uiSettings.AnimationsEnabled);
        Render(_clock.Elapsed.TotalMilliseconds);
        AppWindow.Show(false);
        ScheduleFrame(_clock.Elapsed.TotalMilliseconds);
    }

    private void Prepare()
    {
        _scale = PopupWindowNative.Scale(this);
        var work = (_display ?? DisplayArea.Primary).WorkArea;
        _view.Width = Math.Max(80, Math.Min(400, work.Width / _scale - 32));
        _view.Measure(new Windows.Foundation.Size(_view.Width, double.PositiveInfinity));
        _width = (int)Math.Ceiling(_view.Width * _scale);
        _height = (int)Math.Ceiling(Math.Max(_view.MinHeight, _view.DesiredSize.Height) * _scale);
        _motion.SetHeight(_height, _clock.Elapsed.TotalMilliseconds, _uiSettings.AnimationsEnabled);
        ResizeSurface();
        AppWindow.Move(new PointInt32(work.X + (work.Width - _width) / 2, work.Y + (int)Math.Round(16 * _scale)));
    }

    private void OnRendering(object? sender, object args) => AdvanceFrame();

    private void AdvanceFrame()
    {
        if (_closed) { _dismissTimer.Stop(); StopRendering(); return; }
        try
        {
            double now = _clock.Elapsed.TotalMilliseconds;
            _motion.Advance(now);
            if (!_motion.Visible) { Hide(); return; }
            Render(now);
            ScheduleFrame(now);
        }
        catch { Hide(); }
    }

    private void ScheduleFrame(double now)
    {
        if (_motion.Animating || _view.Animating)
        {
            _dismissTimer.Stop();
            if (_rendering) return;
            // Follow the display's frames; a rearmed timer adds render time to every interval.
            CompositionTarget.Rendering += OnRendering;
            _rendering = true;
        }
        else
        {
            StopRendering();
            _dismissTimer.Stop();
            _dismissTimer.Interval = TimeSpan.FromMilliseconds(_motion.DismissDelay(now));
            _dismissTimer.Start();
        }
    }

    private void StopRendering()
    {
        if (!_rendering) return;
        CompositionTarget.Rendering -= OnRendering;
        _rendering = false;
    }

    private void OnAnimationsEnabledChanged(UISettings sender, UISettingsAnimationsEnabledChangedEventArgs args) =>
        DispatcherQueue.TryEnqueue(() =>
        {
            if (_closed || !_motion.Visible) return;
            try
            {
                _motion.SetAnimationsEnabled(_clock.Elapsed.TotalMilliseconds, _uiSettings.AnimationsEnabled);
                if (!_uiSettings.AnimationsEnabled) _view.FinishAnimations();
                if (!_motion.Visible) { Hide(); return; }
                AdvanceFrame();
            }
            catch { Hide(); }
        });

    private void Render(double now)
    {
        ResizeSurface();
        int revealedWidth = (int)Math.Round(96 * _scale + (_width - 96 * _scale) * _motion.Expansion);
        int revealedHeight = (int)Math.Round(34 * _scale + (_motion.Height - 34 * _scale) * _motion.Expansion);
        int diameter = (int)Math.Round((34 + 38 * _motion.Expansion) * _scale);
        int left = (_width - revealedWidth) / 2;
        var bounds = (left, revealedWidth, revealedHeight, diameter);
        if (_lastRegion != bounds)
        {
            PopupWindowNative.SetRegion(this, left, 0, revealedWidth, revealedHeight, diameter);
            // SetWindowRgn transfers ownership to Windows and needlessly invalidates unchanged edges.
            _lastRegion = bounds;
        }
        _view.RenderFrame(now, _motion.HeaderOpacity, _motion.BatteryOpacity);
    }

    private void ResizeSurface()
    {
        // Keep enough backing surface while the rounded region changes height.
        int height = _motion.Resizing ? Math.Max(AppWindow.Size.Height, _height) : _height;
        if (AppWindow.Size.Width != _width || AppWindow.Size.Height != height)
            AppWindow.Resize(new SizeInt32(_width, height));
    }

    private void Hide()
    {
        _dismissTimer.Stop();
        StopRendering();
        _motion.Hide();
        // Reuse the window: closing themed WinUI content here has crashed XAML teardown.
        if (!_closed) try { AppWindow.Hide(); } catch { }
    }

}
