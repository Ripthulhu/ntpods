using System.ComponentModel;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Shapes;
using Windows.Foundation;
using Windows.System;

namespace NTPods.WinUI.Controls;

/// Monitoring state and timestamped readings come from the daemon.
public sealed partial class HeartRateCard : UserControl
{
    private const double MinGraphBpmSpan = 20;
    public DaemonClient? Client { get; set; }
    private bool _applying, _populating;
    private readonly HeartRateLiveState _liveState = new();
    private readonly HeartRateGraphData _graph = new();
    private readonly List<HeartRateHistory.Session> _sessions = new();
    private HeartRateHistory.Session? _session;
    private CancellationTokenSource? _historyCancellation;
    private int _pickerGeneration, _sampleGeneration;
    private Point? _hover;
    private int? _keyboardIndex;
    private DateTime _plotNow;
    private double _lower, _span;
    private readonly Line _cursor = new() { StrokeThickness = 1, Opacity = 0.6 };
    private readonly Ellipse _dot = new() { Width = 8, Height = 8 };
    private readonly TextBlock _pointText = new();
    private readonly Border _pointLabel = new() { BorderThickness = new Thickness(1), CornerRadius = new CornerRadius(4), Padding = new Thickness(6, 2, 6, 2) };
    private readonly DispatcherTimer _sampler = new() { Interval = TimeSpan.FromSeconds(1) };

    public HeartRateCard()
    {
        InitializeComponent();
        _pointLabel.Child = _pointText;
        GraphOverlay.Children.Add(_cursor);
        GraphOverlay.Children.Add(_dot);
        GraphOverlay.Children.Add(_pointLabel);
        _sampler.Tick += (_, _) => Sample();
        Loaded += OnLoaded;
        Unloaded += OnUnloaded;
        ActualThemeChanged += (_, _) => DrawGraph();
        Panel.Expanding += (_, _) => DispatcherQueue.TryEnqueue(DrawGraph);
    }

    private void OnLoaded(object sender, RoutedEventArgs args)
    {
        _historyCancellation = new();
        Loc.Instance.PropertyChanged += LanguageChanged;
        if (_liveState.Enabled) _sampler.Start();
        _ = PopulatePickerAsync();
        if (_session is not null) _ = LoadSessionAsync(_session);
        DrawGraph();
    }

    private void OnUnloaded(object sender, RoutedEventArgs args)
    {
        _sampler.Stop();
        Loc.Instance.PropertyChanged -= LanguageChanged;
        _historyCancellation?.Cancel();
        _historyCancellation?.Dispose();
        _historyCancellation = null;
        _pickerGeneration++;
        _sampleGeneration++;
    }

    private void LanguageChanged(object? sender, PropertyChangedEventArgs args) => DispatcherQueue.TryEnqueue(() =>
    {
        if (!IsLoaded) return;
        PopulatePicker(_sessions.ToArray());
        DrawGraph();
    });

    public void Update(Snapshot s)
    {
        _applying = true;
        try
        {
            HeartRateSwitch.IsOn = s.HeartRateEnabled;
            HeartRateSwitch.IsEnabled = Client?.IsReady == true && (DeviceModels.Find(s.Model).HeartRate || s.HeartRateEnabled);
            _liveState.Update(s, DateTimeOffset.UtcNow);
            HeartRateBpm.Text = _liveState.Current(DateTimeOffset.UtcNow)?.ToString() ?? "—";
            if (s.HeartRateEnabled && IsLoaded) _sampler.Start(); else _sampler.Stop();
            if (_session is null && _graph.SetSamples(_liveState.Samples)) DrawGraph();
        }
        finally { _applying = false; }
    }

    private void HeartRate_Toggled(object sender, RoutedEventArgs e)
    {
        if (_applying) return;
        Client?.SetHeartRate(HeartRateSwitch.IsOn);
        _applying = true;
        HeartRateSwitch.IsOn = _liveState.Enabled;
        _applying = false;
    }

    private void Sample()
    {
        var now = DateTimeOffset.UtcNow;
        _liveState.Expire(now);
        HeartRateBpm.Text = _liveState.Current(now)?.ToString() ?? "—";
        if (_session is not null) return;
        _graph.SetSamples(_liveState.Samples);
        if (_graph.Samples.Count > 0 || GraphAxisMax.Text.Length > 0) DrawGraph();
    }

    private async Task PopulatePickerAsync()
    {
        if (_historyCancellation is not { } cancellation) return;
        int generation = ++_pickerGeneration;
        try
        {
            var sessions = await HeartRateHistory.SessionsAsync(cancellation.Token);
            if (IsLoaded && generation == _pickerGeneration) PopulatePicker(sessions);
        }
        catch (OperationCanceledException) { }
    }

    private void PopulatePicker(IEnumerable<HeartRateHistory.Session> sessions)
    {
        _populating = true;
        try
        {
            long? keep = _session?.Id;
            _sessions.Clear();
            _sessions.AddRange(sessions);
            HistoryPicker.Items.Clear();
            HistoryPicker.Items.Add(Loc.Instance.Get("HeartRateHistoryLive"));
            foreach (var session in _sessions)
            {
                var minutes = Math.Max(1, (int)Math.Round(((session.End ?? session.Start) - session.Start).TotalMinutes));
                HistoryPicker.Items.Add(Loc.Instance.Get("HeartRateHistorySession", session.Start, minutes, session.Avg));
            }
            int index = keep is long id ? _sessions.FindIndex(session => session.Id == id) + 1 : 0;
            HistoryPicker.SelectedIndex = Math.Max(0, index);
            if (index <= 0 && _session is not null)
            {
                _sampleGeneration++;
                _session = null;
                _graph.SetSamples(_liveState.Samples);
                DrawGraph();
            }
        }
        finally { _populating = false; }
    }

    private void HistoryPicker_DropDownOpened(object sender, object e) => _ = PopulatePickerAsync();

    private void HistoryPicker_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_populating) return;
        int index = HistoryPicker.SelectedIndex;
        _session = index >= 1 && index - 1 < _sessions.Count ? _sessions[index - 1] : null;
        _sampleGeneration++;
        _hover = null;
        _keyboardIndex = null;
        _graph.SetSamples(_session is null ? _liveState.Samples : []);
        DrawGraph();
        if (_session is { } session) _ = LoadSessionAsync(session);
    }

    private async Task LoadSessionAsync(HeartRateHistory.Session session)
    {
        if (_historyCancellation is not { } cancellation) return;
        int generation = ++_sampleGeneration;
        try
        {
            var samples = await HeartRateHistory.SamplesAsync(session.Id, cancellation.Token);
            if (!IsLoaded || generation != _sampleGeneration || _session?.Id != session.Id) return;
            _graph.SetSamples(samples);
            DrawGraph();
        }
        catch (OperationCanceledException) { }
    }

    private void HeartRateGraph_SizeChanged(object sender, SizeChangedEventArgs e) => DrawGraph();
    private void HeartRateGraph_PointerMoved(object sender, PointerRoutedEventArgs e)
    {
        _hover = e.GetCurrentPoint(HeartRateGraph).Position;
        _keyboardIndex = null;
        DrawSelection();
    }
    private void HeartRateGraph_PointerExited(object sender, PointerRoutedEventArgs e) { _hover = null; DrawSelection(); }
    private void Graph_GotFocus(object sender, RoutedEventArgs e)
    {
        _keyboardIndex = _graph.Samples.Count - 1;
        DrawSelection();
    }
    private void Graph_LostFocus(object sender, RoutedEventArgs e) { _keyboardIndex = null; DrawSelection(); }
    private void Graph_KeyDown(object sender, KeyRoutedEventArgs e)
    {
        int count = _graph.Samples.Count;
        if (count == 0) return;
        int index = Math.Clamp(_keyboardIndex ?? count - 1, 0, count - 1);
        switch (e.Key)
        {
            case VirtualKey.Left: index--; break;
            case VirtualKey.Right: index++; break;
            case VirtualKey.Home: index = 0; break;
            case VirtualKey.End: index = count - 1; break;
            default: return;
        }
        _keyboardIndex = Math.Clamp(index, 0, count - 1);
        _hover = null;
        e.Handled = true;
        DrawSelection();
    }

    private double X(int index)
    {
        double right = HeartRateGraph.ActualWidth - 2;
        return _session is null ? right - Math.Clamp((_plotNow - _graph.Samples[index].At).TotalSeconds, 0, 60) / 60 * (right - 2)
            : _graph.Samples.Count <= 1 ? right : 2 + index * (right - 2) / (_graph.Samples.Count - 1);
    }
    private double Y(ushort bpm) => HeartRateGraph.ActualHeight - 6 - Math.Clamp((bpm - _lower) / _span, 0, 1) * (HeartRateGraph.ActualHeight - 12);

    private void DrawGraph()
    {
        if (!IsLoaded || !Panel.IsExpanded) return;
        var canvas = HeartRateGraph;
        double width = canvas.ActualWidth, height = canvas.ActualHeight;
        if (width <= 0 || height <= 0) return;
        canvas.Children.Clear();
        var accent = HeartRateBpm.Foreground;
        var guide = GraphAxisMax.Foreground;
        foreach (var y in new[] { 6d, height / 2, height - 6 })
            canvas.Children.Add(new Line { X1 = 2, X2 = width - 2, Y1 = y, Y2 = y, Stroke = guide, StrokeThickness = 1,
                Opacity = 0.35, StrokeDashArray = new DoubleCollection { 3, 3 } });
        _plotNow = DateTime.Now;
        var raw = _graph.Samples;
        if (raw.Count == 0)
        {
            GraphAxisMax.Text = GraphAxisMid.Text = GraphAxisMin.Text = "";
            HeartRateGraphCaption.Text = Loc.Instance.Get("HeartRateGraphEmpty");
            DrawSelection();
            return;
        }
        _span = Math.Max(_graph.Maximum - _graph.Minimum, MinGraphBpmSpan);
        _lower = (_graph.Minimum + _graph.Maximum) / 2d - _span / 2;
        GraphAxisMax.Text = Math.Round(_lower + _span).ToString();
        GraphAxisMid.Text = Math.Round(_lower + _span / 2).ToString();
        GraphAxisMin.Text = Math.Round(_lower).ToString();
        HeartRateGraphCaption.Text = Loc.Instance.Get(_session is null ? "HeartRateGraphStats" : "HeartRateSessionStats",
            raw.Count, _graph.Minimum, _graph.Average, _graph.Maximum);
        var points = _graph.Points(_session is null ? raw.Count : (int)Math.Max(2, (width - 4) / 3));
        var line = new Polyline { Stroke = accent, StrokeThickness = 2, StrokeLineJoin = PenLineJoin.Round };
        bool bars = (width - 4) / Math.Max(1, points.Count - 1) >= 3;
        int previous = -1;
        foreach (var point in points)
        {
            double x = X(point.Index), y = Y(point.Bpm);
            if (_session is null && previous >= 0 && (raw[point.Index].At - raw[previous].At).TotalSeconds > 3)
            {
                if (line.Points.Count > 1) canvas.Children.Add(line);
                line = new Polyline { Stroke = accent, StrokeThickness = 2, StrokeLineJoin = PenLineJoin.Round };
            }
            if (bars) canvas.Children.Add(new Line { X1 = x, X2 = x, Y1 = height - 6, Y2 = y, Stroke = accent, StrokeThickness = 1, Opacity = 0.18 });
            line.Points.Add(new Point(x, y));
            previous = point.Index;
        }
        if (line.Points.Count > 1) canvas.Children.Add(line);
        var last = points[^1];
        var dot = new Ellipse { Width = 5, Height = 5, Fill = accent };
        Canvas.SetLeft(dot, X(last.Index) - 2.5); Canvas.SetTop(dot, Y(last.Bpm) - 2.5); canvas.Children.Add(dot);
        DrawSelection();
    }

    private void DrawSelection()
    {
        int count = _graph.Samples.Count;
        int index = _keyboardIndex is int selected ? Math.Clamp(selected, 0, Math.Max(0, count - 1)) :
            _hover is Point hover ? _graph.Nearest((hover.X - 2) / Math.Max(1, HeartRateGraph.ActualWidth - 4), _session is null, _plotNow) : -1;
        GraphOverlay.Visibility = count > 0 && index >= 0 ? Visibility.Visible : Visibility.Collapsed;
        if (count == 0 || index < 0)
        {
            AutomationProperties.SetName(GraphFocus, Loc.Instance.Get("HeartRateGraph_AccessibleName") + ". " + HeartRateGraphCaption.Text);
            return;
        }
        double x = X(index), y = Y(_graph.Samples[index].Bpm);
        var (at, bpm) = _graph.Samples[index];
        string text = Loc.Instance.Get("HeartRateGraphPointAt", bpm, at);
        _cursor.Stroke = _dot.Fill = HeartRateBpm.Foreground;
        _cursor.X1 = _cursor.X2 = x; _cursor.Y1 = 6; _cursor.Y2 = HeartRateGraph.ActualHeight - 6;
        Canvas.SetLeft(_dot, x - 4); Canvas.SetTop(_dot, y - 4);
        _pointText.Text = text;
        _pointText.Style = (Style)Application.Current.Resources["CaptionTextBlockStyle"];
        _pointLabel.Background = (Brush)Application.Current.Resources["CardBackgroundFillColorDefaultBrush"];
        _pointLabel.BorderBrush = GraphAxisMax.Foreground;
        _pointLabel.Measure(new Size(double.PositiveInfinity, double.PositiveInfinity));
        double labelWidth = _pointLabel.DesiredSize.Width;
        Canvas.SetLeft(_pointLabel, Math.Clamp(x - labelWidth / 2, 0, Math.Max(0, HeartRateGraph.ActualWidth - labelWidth)));
        Canvas.SetTop(_pointLabel, y > 32 ? y - 26 : y + 8);
        AutomationProperties.SetName(GraphFocus, Loc.Instance.Get("HeartRateGraph_AccessibleName") + ". " + text);
    }
}
