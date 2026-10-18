using System.Diagnostics;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Windowing;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using NTPods.WinUI.Services;
using Windows.Graphics;
using Windows.UI.ViewManagement;

namespace NTPods.WinUI.Popup;

/// Reused bottom card for pairing and reconnecting, shown without activating it.
public sealed class ConnectPromptWindow : Window
{
    private readonly ConnectPromptView _view = new();
    private readonly IslandMotion _motion = new(double.PositiveInfinity, openingMilliseconds: 420);
    private readonly UISettings _settings = new();
    private readonly Stopwatch _clock = Stopwatch.StartNew();
    private readonly DispatcherQueueTimer _expiry;
    private Func<CancellationToken, Task<string?>>? _connect;
    private string _busyStatusKey = "Pairing.Pairing";
    private CancellationTokenSource? _request;
    private bool _closed, _rendering, _hovered, _active, _shown, _hiding, _paired, _interacted;
    private int _generation, _width, _height;
    private double _scale = 1;
    private (int Left, int Top, int Width, int Height, int Diameter)? _region;
    public bool Busy => _request is not null;
    public bool PromptVisible => _motion.Visible;
    public event Action? Dismissed;
    public event Action? BusyChanged;

    public ConnectPromptWindow()
    {
        Title = "NTPods";
        Content = _view;
        PopupWindowNative.Configure(this, interactive: true);
        _expiry = DispatcherQueue.CreateTimer();
        _expiry.IsRepeating = false;
        _expiry.Tick += (_, _) =>
        {
            if (Busy || _hovered || _active) ArmExpiry();
            else HidePrompt();
        };
        _view.ConnectRequested += Connect;
        _view.DismissRequested += HidePrompt;
        _view.PointerEntered += (_, _) => _hovered = true;
        _view.PointerExited += (_, _) => { _hovered = false; ArmExpiry(); };
        _view.AddHandler(UIElement.KeyDownEvent, new KeyEventHandler((_, args) =>
        {
            if (args.Key != Windows.System.VirtualKey.Escape) return;
            args.Handled = true;
            HidePrompt();
        }), true);
        _view.Loaded += (_, _) => RefreshLayout();
        _view.SizeChanged += (_, _) => RefreshLayout();
        Activated += (_, args) =>
        {
            _active = args.WindowActivationState != WindowActivationState.Deactivated;
            if (!_active) ArmExpiry();
        };
        if (OperatingSystem.IsWindowsVersionAtLeast(10, 0, 19041))
            _settings.AnimationsEnabledChanged += AnimationsChanged;
        Closed += (_, _) =>
        {
            _closed = true;
            CancelRequest();
            _expiry.Stop();
            StopRendering();
            if (OperatingSystem.IsWindowsVersionAtLeast(10, 0, 19041))
                _settings.AnimationsEnabledChanged -= AnimationsChanged;
        };
    }

    public void ShowPrompt(string name, Func<CancellationToken, Task<string?>> connect,
        string statusKey = "Pairing.Nearby", string busyStatusKey = "Pairing.Pairing", ushort? model = null)
    {
        if (_closed || Busy) return;
        ++_generation;
        _hiding = false;
        _paired = false;
        _interacted = false;
        _shown = true;
        _connect = connect;
        _busyStatusKey = busyStatusKey;
        _view.SetName(name, model);
        string? gesture = PairingInstructions.InitialGesture(statusKey, busyStatusKey);
        _view.SetState(false, gesture ?? statusKey, beforeConnect: gesture is not null);
        var work = DisplayArea.Primary.WorkArea;
        AppWindow.Move(new PointInt32(work.X + work.Width / 2, work.Y + work.Height / 2));
        Prepare();
        _motion.Show(_clock.Elapsed.TotalMilliseconds, _settings.AnimationsEnabled);
        Advance();
        if (!PromptVisible) return;
        AppWindow.Show(false);
        ArmExpiry();
    }

    private async void Connect()
    {
        if (!PromptVisible || _closed || _request?.IsCancellationRequested == true) return;
        if (_paired) { HidePrompt(); return; }
        if (Busy || _connect is null) return;
        _interacted = true;
        using var request = new CancellationTokenSource(TimeSpan.FromSeconds(60));
        _request = request;
        int generation = _generation;
        _expiry.Stop();
        _view.SetState(true, _busyStatusKey);
        BusyChanged?.Invoke();
        string? error;
        try { error = await _connect(request.Token); }
        catch (OperationCanceledException) { error = "Pairing.TimedOut"; }
        catch (Exception) { error = "Pairing.Failed"; }
        if (_closed || generation != _generation) return;
        _request = null;
        if (error is null)
        {
            if (!_paired) HidePrompt();
        }
        else
        {
            _view.SetState(false, error, retry: true);
            RefreshLayout();
            ArmExpiry();
        }
        BusyChanged?.Invoke();
    }

    public void SetPairingStatus(string key)
    {
        if (_closed || _hiding || _paired || !Busy || _request?.IsCancellationRequested == true) return;
        _view.SetState(true, key);
        RefreshLayout();
    }

    public void MarkPaired()
    {
        if (_closed || _hiding || !PromptVisible || _request?.IsCancellationRequested == true) return;
        _paired = true;
        _expiry.Stop();
        _view.SetPaired();
        RefreshLayout();
    }

    public void UpdatePairedBattery(byte? left, byte? right, byte? caseLevel,
        bool leftCharging, bool rightCharging, bool caseCharging,
        bool headphone, byte? headphoneLevel, bool headphoneCharging)
    {
        if (_closed || _hiding || !_paired) return;
        if (_view.SetBattery(left, right, caseLevel, leftCharging, rightCharging, caseCharging,
            headphone, headphoneLevel, headphoneCharging)) RefreshLayout();
    }

    public void HidePrompt()
    {
        if (_closed || _hiding || !PromptVisible) return;
        _hiding = true;
        bool busy = Busy;
        CancelRequest();
        _connect = null;
        _expiry.Stop();
        _motion.Dismiss(_clock.Elapsed.TotalMilliseconds, _settings.AnimationsEnabled);
        Advance();
        if (busy) BusyChanged?.Invoke();
    }

    private void CancelRequest()
    {
        ++_generation;
        _request?.Cancel();
        _request = null;
    }

    private void ArmExpiry()
    {
        if (_closed || _hiding || !PromptVisible || Busy || _paired || _interacted) return;
        _expiry.Stop();
        _expiry.Interval = TimeSpan.FromSeconds(20);
        _expiry.Start();
    }

    private void Prepare()
    {
        _scale = PopupWindowNative.Scale(this);
        var work = DisplayArea.Primary.WorkArea;
        _view.Width = Math.Max(240, Math.Min(400, work.Width / _scale - 32));
        _view.Fit(Math.Max(160, work.Height / _scale - 32));
        _view.Measure(new Windows.Foundation.Size(_view.Width, double.PositiveInfinity));
        _width = (int)Math.Ceiling(_view.Width * _scale);
        _height = (int)Math.Ceiling(_view.DesiredSize.Height * _scale);
        _motion.SetHeight(_height, _clock.Elapsed.TotalMilliseconds, _settings.AnimationsEnabled);
        if (AppWindow.Size.Width != _width || AppWindow.Size.Height != _height)
            AppWindow.Resize(new SizeInt32(_width, _height));
        AppWindow.Move(new PointInt32(work.X + (work.Width - _width) / 2,
            work.Y + work.Height - _height - (int)Math.Round(16 * _scale)));
    }

    private void OnRendering(object? sender, object args) => Advance();

    private void RefreshLayout()
    {
        if (!PromptVisible || _closed) return;
        try { Prepare(); Advance(); }
        catch (Exception) { HideImmediately(); }
    }

    private void Advance()
    {
        try { AdvanceCore(); }
        catch (Exception) { HideImmediately(); }
    }

    private void HideImmediately()
    {
        _hiding = true;
        CancelRequest();
        _expiry.Stop();
        StopRendering();
        _motion.Hide();
        try { AppWindow.Hide(); } catch (Exception) { }
        if (!_shown) return;
        _shown = false;
        Dismissed?.Invoke();
    }

    private void AdvanceCore()
    {
        if (_closed) return;
        _motion.Advance(_clock.Elapsed.TotalMilliseconds);
        if (!PromptVisible)
        {
            StopRendering();
            AppWindow.Hide();
            if (_shown)
            {
                _shown = false;
                Dismissed?.Invoke();
            }
            return;
        }
        _view.Render(_motion.Expansion, _height / _scale);
        int height = Math.Max(1, (int)Math.Round(_motion.Height * _motion.Expansion));
        int diameter = Math.Min(height, (int)Math.Round(56 * _scale));
        var region = (0, _height - height, _width, height, diameter);
        if (_region != region)
        {
            PopupWindowNative.SetRegion(this, region.Item1, region.Item2, _width, height, diameter);
            _region = region;
        }
        if (_motion.Animating && !_rendering)
        {
            CompositionTarget.Rendering += OnRendering;
            _rendering = true;
        }
        else if (!_motion.Animating) StopRendering();
    }

    private void StopRendering()
    {
        if (!_rendering) return;
        CompositionTarget.Rendering -= OnRendering;
        _rendering = false;
    }

    private void AnimationsChanged(UISettings sender, UISettingsAnimationsEnabledChangedEventArgs args) =>
        DispatcherQueue.TryEnqueue(() =>
        {
            if (_closed || !PromptVisible) return;
            _motion.SetAnimationsEnabled(_clock.Elapsed.TotalMilliseconds, sender.AnimationsEnabled);
            Advance();
        });
}
