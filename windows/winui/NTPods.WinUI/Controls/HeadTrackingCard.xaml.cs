using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;
using Windows.Foundation;

namespace NTPods.WinUI.Controls;

public sealed partial class HeadTrackingCard : UserControl
{
    private readonly HeadTrackingLiveState _live = new();
    private DaemonClient? _client;
    private DaemonEvent.HeadTracking? _latest;
    private bool _subscribed;
    private bool _rendering;
    private bool? _requested;
    private long _requestDeadline;
    private bool _fresh;
    private bool _ownsStream;
    private bool _supported;
    private long _lastDescription;

    public DaemonClient? Client
    {
        get => _client;
        set
        {
            if (ReferenceEquals(_client, value)) return;
            Stop();
            Detach();
            _client = value;
            if (IsLoaded) Attach();
        }
    }

    public HeadTrackingCard()
    {
        InitializeComponent();
        Loaded += (_, _) => { Attach(); RenderStatus(); };
        Unloaded += (_, _) => { Stop(); Detach(); };
    }

    private void Attach()
    {
        if (_subscribed || _client is null) return;
        _client.HeadTrackingReceived += OnSample;
        _subscribed = true;
    }

    private void Detach()
    {
        if (_subscribed && _client is not null) _client.HeadTrackingReceived -= OnSample;
        _subscribed = false;
        Interlocked.Exchange(ref _latest, null);
    }

    // Keep at most one pending sample while the UI thread is busy.
    private void OnSample(DaemonEvent.HeadTracking sample) => Interlocked.Exchange(ref _latest, sample);

    public void Update(Snapshot snapshot)
    {
        _supported = DeviceModels.HeadTracking(snapshot);
        _live.Update(snapshot);
        if (!_live.Connected || _requested == _live.Enabled) _requested = null;
        RenderStatus();
        Draw();
        SetRendering(Panel.IsExpanded && (_live.Enabled || _requested is not null));
    }

    public void Stop(bool notifyDevice = true)
    {
        if (_ownsStream && notifyDevice) _client?.Send(new SetHeadTrackingCmd { On = false });
        _ownsStream = false;
        _requested = null;
        Panel.IsExpanded = false;
        SetRendering(false);
        Interlocked.Exchange(ref _latest, null);
    }

    private void Panel_Expanding(Expander sender, ExpanderExpandingEventArgs args)
    {
        // Reopening may precede the previous stop's snapshot. Queue the new
        // intent after that stop, even when the last reported state is enabled.
        Request(true);
        Draw();
    }

    private void Panel_Collapsed(Expander sender, ExpanderCollapsedEventArgs args) => Stop();

    private void Start_Click(object sender, RoutedEventArgs args) => Request(!_live.Enabled);

    private void Request(bool on)
    {
        if (on && (!_live.Connected || !_supported) || _client?.Send(new SetHeadTrackingCmd { On = on }) != true) return;
        _ownsStream = on;
        _requested = on;
        _requestDeadline = Environment.TickCount64 + 5_000;
        RenderStatus();
        SetRendering(Panel.IsExpanded);
    }

    private static ulong Now => (ulong)DateTimeOffset.UtcNow.ToUnixTimeMilliseconds();

    private void RenderStatus()
    {
        bool enabled = _requested ?? _live.Enabled;
        StartIcon.Glyph = enabled ? "\uE769" : "\uE768";
        string action = Localize.Get(enabled ? "HeadTracking.Pause" : "HeadTracking.Start");
        AutomationProperties.SetName(StartButton, action);
        ToolTipService.SetToolTip(StartButton, action);
        StartButton.IsEnabled = _requested is null && _client?.IsReady == true && (_live.Connected && _supported || enabled);
        ErrorText.Text = _live.Error ?? "";
        ErrorText.Visibility = _live.State == "error" && !string.IsNullOrEmpty(_live.Error) ? Visibility.Visible : Visibility.Collapsed;
        EmptyText.Visibility = _live.Samples.Count == 0 ? Visibility.Visible : Visibility.Collapsed;
        EmptyText.Text = Localize.Get(!_live.Connected ? "HeadTracking.Connect" :
            _live.State == "waiting_for_ear" ? "HeadTracking.Wear" : enabled ? "HeadTracking.Waiting" : "HeadTracking.Ready");
        _fresh = _live.IsFresh(Now);
        HorizontalTrace.Opacity = VerticalTrace.Opacity = _fresh ? 1 : 0.45;
        if (!_fresh || Environment.TickCount64 - _lastDescription >= 250)
        {
            _lastDescription = Environment.TickCount64;
            MovementValues.Text = _fresh ? Localize.Get("HeadTracking.Values", _live.Samples[^1].Horizontal, _live.Samples[^1].Vertical) : "—";
            AutomationProperties.SetName(GraphHost, Localize.Get("HeadTracking_LiveMovement") + ". " +
                (_fresh ? MovementValues.Text : EmptyText.Text));
        }
    }

    private void SetRendering(bool on)
    {
        on &= IsLoaded;
        if (on == _rendering) return;
        _rendering = on;
        if (on) CompositionTarget.Rendering += RenderFrame;
        else CompositionTarget.Rendering -= RenderFrame;
    }

    private void RenderFrame(object? sender, object args)
    {
        bool changed = false;
        if (Interlocked.Exchange(ref _latest, null) is { } sample) changed = _live.Add(sample, Now);
        if (_requested is not null && Environment.TickCount64 >= _requestDeadline)
        {
            _requested = null;
            _client?.RequestState();
            changed = true;
        }
        if (changed) Draw();
        if (changed || _fresh != _live.IsFresh(Now)) RenderStatus();
        if (!_live.Enabled && _requested is null) SetRendering(false);
    }

    private void Graph_SizeChanged(object sender, SizeChangedEventArgs args)
    {
        double w = GraphHost.ActualWidth, h = GraphHost.ActualHeight;
        if (w <= 0 || h <= 0) return;
        Graph.Clip = new RectangleGeometry { Rect = new Rect(0, 0, w, h) };
        var geometry = new PathGeometry();
        for (int i = 1; i < 10; i++) AddLine(geometry, new Point(w * i / 10, 0), new Point(w * i / 10, h));
        for (int i = 1; i < 4; i++) AddLine(geometry, new Point(0, h * i / 4), new Point(w, h * i / 4));
        Guides.Data = geometry;
        ZeroLine.X2 = w;
        ZeroLine.Y1 = ZeroLine.Y2 = h / 2;
        Draw();
    }

    private static void AddLine(PathGeometry geometry, Point from, Point to)
    {
        var figure = new PathFigure { StartPoint = from, IsClosed = false };
        figure.Segments.Add(new LineSegment { Point = to });
        geometry.Figures.Add(figure);
    }

    private void Draw()
    {
        double w = GraphHost.ActualWidth, h = GraphHost.ActualHeight;
        if (w <= 0 || h <= 0) return;
        var horizontal = new PointCollection();
        var vertical = new PointCollection();
        double scale = (h / 2 - 8) / _live.Range;
        var samples = _live.Samples;
        for (int i = 0; i < samples.Count; ++i)
        {
            double x = (HeadTrackingLiveState.Capacity - samples.Count + i) * w / (HeadTrackingLiveState.Capacity - 1);
            horizontal.Add(new Point(x, h / 2 - samples[i].Horizontal * scale));
            vertical.Add(new Point(x, h / 2 - samples[i].Vertical * scale));
        }
        HorizontalTrace.Points = horizontal;
        VerticalTrace.Points = vertical;
    }
}
