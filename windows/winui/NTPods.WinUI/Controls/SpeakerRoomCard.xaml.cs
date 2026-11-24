using Microsoft.UI.Input;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Shapes;
using NTPods.WinUI.Services;
using Windows.System;
using Windows.UI.Core;
using RectangleF = System.Drawing.RectangleF;

namespace NTPods.WinUI.Controls;

public sealed partial class SpeakerRoomCard : UserControl
{
    private SpatialRoomLayout _room = SpatialRoomLayout.Load(0x63f, AppSettings.SpatialRoom(0x63f));
    private sealed record MapSpeaker(Button Node, Ellipse Dot, Line Height, Line Leader, Ellipse Pin);
    private readonly Dictionary<int, MapSpeaker> _nodes = new();
    private const double OrbitDepth = 60, OrbitHeight = 68;
    private readonly Windows.UI.ViewManagement.UISettings _uiSettings = new();
    private readonly DispatcherTimer _save = new() { Interval = TimeSpan.FromMilliseconds(350) };
    private readonly DispatcherTimer _send = new() { Interval = TimeSpan.FromMilliseconds(60) };
    private bool _sync = true;
    private RoomFrame? _frame;
    private readonly SpeakerSelection _selection = new();
    internal event Action<string>? LayoutChanged;
    internal string Command => _room.Command();
    internal bool HasStreamFormat => _frame != null;
    internal string LayoutFor(int mask) => mask == _room.Mask ? Command :
        SpatialRoomLayout.Load(mask, AppSettings.SpatialRoom(mask)).Command();

    public SpeakerRoomCard()
    {
        InitializeComponent();
        _save.Tick += (_, _) => Flush();
        _send.Tick += (_, _) => { _send.Stop(); LayoutChanged?.Invoke(Command); };
        Unloaded += (_, _) =>
        {
            Loc.Instance.PropertyChanged -= LanguageChanged;
            _uiSettings.ColorValuesChanged -= ColorsChanged;
            if (_save.IsEnabled) Flush();
            ReleaseScene();
        };
        ActualThemeChanged += (_, _) => LayoutAndDraw();
        Loaded += (_, _) =>
        {
            Loc.Instance.PropertyChanged += LanguageChanged;
            _uiSettings.ColorValuesChanged += ColorsChanged;
            if (_sceneVisible) OpenScene();
            LayoutAndDraw();
        };
        Rebuild();
    }

    private void LanguageChanged(object? sender, System.ComponentModel.PropertyChangedEventArgs args) =>
        DispatcherQueue.TryEnqueue(() => { if (IsLoaded) Rebuild(); });

    private void ColorsChanged(Windows.UI.ViewManagement.UISettings sender, object args) =>
        DispatcherQueue.TryEnqueue(() => { if (IsLoaded) LayoutAndDraw(); });

    private void Flush()
    {
        _save.Stop();
        AppSettings.SaveSpatialRoom(_room.Mask, Command);
    }

    private void Rebuild()
    {
        _sync = true;
        foreach (var speaker in _nodes.Values)
        {
            Map.Children.Remove(speaker.Node);
            Map.Children.Remove(speaker.Height);
            Map.Children.Remove(speaker.Leader);
            Map.Children.Remove(speaker.Pin);
        }
        _nodes.Clear();
        _sceneDrag = -1;
        _selection.Reconcile(_room.Mask);
        ChannelChoices.Children.Clear();
        SceneHint.Text = Localize.Get("Room_OrbitHint") + " " + Localize.Get("Room_SelectionHint");
        foreach (var (bit, label) in SpatialRoomLayout.Channels.Where(s => _room.Includes(s.Bit)))
        {
            string name = Localize.Get("Room_" + label);
            var item = new CheckBox { Tag = bit, Content = name };
            item.Checked += ChannelChoiceChanged;
            item.Unchecked += ChannelChoiceChanged;
            ChannelChoices.Children.Add(item);
            if (bit == 3) continue;
            var dot = new Ellipse { Width = 5, Height = 5 };
            var meter = new Grid { Width = 13, Height = 13, VerticalAlignment = VerticalAlignment.Center };
            meter.Children.Add(dot);
            var stack = new StackPanel { Orientation = Orientation.Horizontal, Spacing = 4 };
            stack.Children.Add(meter);
            stack.Children.Add(new TextBlock { Text = label, FontSize = 12 });
            var button = new Button { Content = stack, Tag = bit, MinWidth = 44, MinHeight = 36,
                Padding = new Thickness(5, 0, 5, 0), CornerRadius = new CornerRadius(18) };
            AutomationProperties.SetName(button, name);
            AutomationProperties.SetHelpText(button, Localize.Get("Room_SelectionHint"));
            ToolTipService.SetToolTip(button, name);
            BindMapInput(button, bit);
            var height = new Line { StrokeThickness = 1, IsHitTestVisible = false };
            var leader = new Line { StrokeThickness = 1, Opacity = .75, IsHitTestVisible = false };
            var pin = new Ellipse { Width = 4, Height = 4, IsHitTestVisible = false };
            Map.Children.Add(height);
            Map.Children.Add(leader);
            Map.Children.Add(pin);
            Canvas.SetZIndex(button, 1);
            Map.Children.Add(button);
            _nodes.Add(bit, new(button, dot, height, leader, pin));
            button.SizeChanged += (_, _) => LayoutAndDraw();
        }
        SyncSelection();
    }

    private static bool ControlDown() =>
        (InputKeyboardSource.GetKeyStateForCurrentThread(VirtualKey.Control) & CoreVirtualKeyStates.Down) != 0;

    private void BindMapInput(Button button, int bit)
    {
        uint? pointer = null;
        Windows.Foundation.Point start = default;
        double bearing = 0;
        bool dragged = false;
        button.ManipulationMode = ManipulationModes.None;
        button.AddHandler(PointerPressedEvent, new PointerEventHandler((_, e) =>
        {
            var point = e.GetCurrentPoint(Map);
            if (pointer != null || !point.Properties.IsLeftButtonPressed) return;
            pointer = e.Pointer.PointerId;
            start = point.Position;
            bearing = _room.Bearing(bit);
            dragged = false;
        }), true);
        button.AddHandler(PointerMovedEvent, new PointerEventHandler((_, e) =>
        {
            if (pointer != e.Pointer.PointerId) return;
            var current = e.GetCurrentPoint(Map).Position;
            double dx = current.X - start.X, dy = current.Y - start.Y;
            if (!dragged)
            {
                if (dx * dx + dy * dy < 25) return;
                dragged = true;
                SelectSpeaker(bit, drag: true);
            }
            if (_selection.Active != bit) return;
            int elevation = _room.Get(bit).Elevation;
            double horizontal = Math.Cos(elevation * Math.PI / 180);
            double radius = Math.Min(Map.ActualWidth / 2 - 28, 200);
            if (horizontal < .1 || radius <= 0) return;
            double right = Math.Sin(bearing) + dx / (radius * horizontal);
            double forward = Math.Cos(bearing) - dy / (OrbitDepth * horizontal);
            if (right * right + forward * forward >= .01 &&
                _room.DragSelection(_selection.Mask, bit, Math.Atan2(right, forward) * 180 / Math.PI, elevation))
                Changed();
        }), true);
        foreach (var end in new[] { PointerReleasedEvent, PointerCaptureLostEvent, PointerCanceledEvent })
            button.AddHandler(end, new PointerEventHandler((_, e) =>
            {
                if (pointer != e.Pointer.PointerId) return;
                pointer = null;
                // Ignore a Click synthesized during release, then allow keyboard and automation again.
                if (dragged) DispatcherQueue.TryEnqueue(() => { if (pointer == null) dragged = false; });
            }), true);
        button.PreviewKeyDown += (_, e) =>
        {
            dragged = false;
            if (!ControlDown() || e.Key is not (VirtualKey.Space or VirtualKey.Enter)) return;
            e.Handled = true;
            if (!e.KeyStatus.WasKeyDown)
            {
                SelectSpeaker(bit, toggle: true);
                Placement.IsExpanded = true;
            }
        };
        button.Click += (_, _) =>
        {
            if (dragged) return;
            SelectSpeaker(bit, toggle: ControlDown());
            Placement.IsExpanded = true;
        };
    }

    private void SelectSpeaker(int bit, bool toggle = false, bool drag = false)
    {
        _sceneDrag = -1;
        if (drag)
        {
            _selection.ForDrag(bit);
            _room.BeginDrag();
        }
        else _selection.Select(bit, toggle);
        SyncSelection();
    }

    private void ChannelChoiceChanged(object sender, RoutedEventArgs e)
    {
        if (!_sync && sender is CheckBox { Tag: int bit } item && item.IsChecked != _selection.Contains(bit))
            SelectSpeaker(bit, toggle: true);
    }

    private void SyncSelection()
    {
        _sync = true;
        var selected = SpatialRoomLayout.Channels.Where(s => _selection.Contains(s.Bit)).ToArray();
        var names = selected.Select(s => Localize.Get("Room_" + s.Label)).ToArray();
        string prompt = Localize.Get("Room_SelectSpeakers");
        ChannelLabel.Text = selected.Length switch
        {
            0 => prompt,
            1 => names[0],
            _ => string.Join(" · ", selected.Select(s => s.Label))
        };
        AutomationProperties.SetName(Channel, names.Length == 0 ? prompt : prompt + ", " + string.Join(", ", names));
        foreach (var item in ChannelChoices.Children.OfType<CheckBox>()) item.IsChecked = _selection.Contains((int)item.Tag);
        foreach (var (bit, speaker) in _nodes)
            AutomationProperties.SetItemStatus(speaker.Node, _selection.Contains(bit) ? Localize.Get("Room_Selected") : "");
        PositionControls.Visibility = _selection.PositionAnchor < 0 ? Visibility.Collapsed : Visibility.Visible;
        GainControls.Visibility = _selection.Active < 0 ? Visibility.Collapsed : Visibility.Visible;
        SyncControls();
    }

    private void SyncControls()
    {
        _sync = true;
        WidthSlider.Value = _room.Width;
        WidthValue.Text = $"{_room.Width}%";
        string AnchorSuffix(int bit) => System.Numerics.BitOperations.PopCount((uint)_selection.Mask) > 1
            ? $" ({SpatialRoomLayout.Channels.First(s => s.Bit == bit).Label})" : "";
        if (_selection.PositionAnchor >= 0)
        {
            var p = _room.Get(_selection.PositionAnchor);
            string anchor = AnchorSuffix(_selection.PositionAnchor);
            Az.Value = p.Azimuth;
            El.Value = p.Elevation;
            AzValue.Text = $"{p.Azimuth}°{anchor}";
            ElValue.Text = $"{p.Elevation}°{anchor}";
        }
        if (_selection.Active >= 0)
        {
            int gain = _room.Get(_selection.Active).Gain;
            Gain.Value = gain;
            GainValue.Text = $"{gain} dB{AnchorSuffix(_selection.Active)}";
        }
        _sync = false;
        LayoutAndDraw();
    }

    private void Edit_Changed(object sender, RangeBaseValueChangedEventArgs e)
    {
        if (_sync || sender is not Slider slider) return;
        bool changed;
        if (slider == WidthSlider)
        {
            int width = (int)Math.Round(WidthSlider.Value);
            changed = width != _room.Width;
            _room.Width = width;
        }
        else if (slider == Gain && _selection.Active >= 0)
            changed = _room.AdjustGain(_selection.Mask, _selection.Active, (int)Math.Round(Gain.Value));
        else if ((slider == Az || slider == El) && _selection.PositionAnchor >= 0)
            changed = _room.MoveSelection(_selection.Mask, _selection.PositionAnchor,
                (int)Math.Round(Az.Value), (int)Math.Round(El.Value));
        else return;
        if (changed) Changed();
        else SyncControls();
    }

    private void Reset_Click(object sender, RoutedEventArgs e)
    {
        _room = new(_room.Mask);
        Changed();
    }

    private void Changed()
    {
        SyncControls();
        if (!_send.IsEnabled) _send.Start();
        _save.Stop();
        _save.Start();
    }

    internal void ShowFrame(RoomFrame? frame)
    {
        bool first = _frame is null;
        if (_frame == frame) return;
        _frame = frame;
        if (frame != null && frame.Mask != _room.Mask)
        {
            if (_save.IsEnabled) Flush();
            _send.Stop();
            _room = SpatialRoomLayout.Load(frame.Mask, AppSettings.SpatialRoom(frame.Mask));
            Rebuild();
            first = true;
        }
        if (first && frame != null) LayoutChanged?.Invoke(Command);
        if (_sceneVisible && _windowVisible) Draw();
    }

    private void Map_SizeChanged(object sender, SizeChangedEventArgs e) => LayoutAndDraw();

    private void LayoutAndDraw()
    {
        if (Map == null || _sync || _nodes.Count == 0) return;
        if (_sceneReady) { Draw(); return; }
        double cx = Map.ActualWidth / 2, cy = Map.Height / 2;
        if (cx < 28) return;
        double radius = Math.Min(cx - 28, 200);
        void Ring(Ellipse ring, double scale)
        {
            ring.Width = 2 * radius * scale; ring.Height = 2 * OrbitDepth * scale;
            Canvas.SetLeft(ring, cx - radius * scale); Canvas.SetTop(ring, cy - OrbitDepth * scale);
        }
        Ring(Orbit, 1); Ring(InnerOrbit, .5);
        Meridian.X1 = Meridian.X2 = cx; Meridian.Y1 = cy - OrbitDepth; Meridian.Y2 = cy + OrbitDepth;
        Equator.Y1 = Equator.Y2 = cy; Equator.X1 = cx - radius; Equator.X2 = cx + radius;
        var outline = Orbit.Stroke;
        // Center gets first choice at the screen center. Keep this order stable
        // across selection and meter changes, including during a direct drag.
        var speakers = _nodes.OrderBy(pair => pair.Key == 2 ? 0 : 1).ToArray();
        var targets = new RectangleF[speakers.Length];
        for (int i = 0; i < speakers.Length; i++)
        {
            var (bit, speaker) = speakers[i];
            var az = _room.Bearing(bit);
            var el = _room.Get(bit).Elevation * Math.PI / 180;
            double x = cx + radius * Math.Sin(az) * Math.Cos(el);
            double floor = cy - OrbitDepth * Math.Cos(az) * Math.Cos(el);
            double speakerY = floor - OrbitHeight * Math.Sin(el);
            var node = speaker.Node;
            float width = (float)Math.Max(node.MinWidth, node.ActualWidth);
            float height = (float)Math.Max(node.MinHeight, node.ActualHeight);
            targets[i] = new((float)x - width / 2, (float)speakerY - height / 2, width, height);
            speaker.Height.X1 = speaker.Height.X2 = x;
            speaker.Height.Y1 = floor;
            speaker.Height.Y2 = speakerY;
            speaker.Height.Stroke = outline;
        }
        var labels = SpeakerMapLabels.Place(new(0, 0, (float)Map.ActualWidth, (float)Map.Height),
            new((float)cx - 22, (float)cy - 24, 44, 48), targets);
        for (int i = 0; i < speakers.Length; i++)
        {
            var speaker = speakers[i].Value;
            var box = labels[i];
            Canvas.SetLeft(speaker.Node, box.Left);
            Canvas.SetTop(speaker.Node, box.Top);
            double x = speaker.Height.X2, speakerY = speaker.Height.Y2;
            bool displaced = Math.Abs(box.Left - targets[i].Left) >= 1 || Math.Abs(box.Top - targets[i].Top) >= 1;
            speaker.Leader.Visibility = speaker.Pin.Visibility = displaced ? Visibility.Visible : Visibility.Collapsed;
            if (!displaced) continue;
            speaker.Leader.Stroke = speaker.Pin.Fill = PoseLabel.Foreground;
            speaker.Leader.X1 = x;
            speaker.Leader.Y1 = speakerY;
            speaker.Leader.X2 = Math.Clamp(x, box.Left, box.Right);
            speaker.Leader.Y2 = Math.Clamp(speakerY, box.Top, box.Bottom);
            Canvas.SetLeft(speaker.Pin, x - 2);
            Canvas.SetTop(speaker.Pin, speakerY - 2);
        }
        Draw();
    }

    private void Draw()
    {
        if (Map == null || _sync || _nodes.Count == 0) return;
        if (_sceneReady) UpdateScene();
        else DrawPlanPose();
        LayoutLabel.Text = _room.Mask == 0x63f ? "7.1" : "5.1";
        var pose = _frame?.Yaw is int y ? (_frame.Pitch is int p && _frame.Roll is int r
            ? Localize.Get("Room_Pose", y, p, r) : Localize.Get("Room_Yaw", y)) : Localize.Get("Room_NoPose");
        if (PoseLabel.Text != pose) PoseLabel.Text = pose;
        var activeNames = SpatialRoomLayout.Channels.Where(c => ((_frame?.Active ?? 0) & (1 << c.Bit)) != 0).Select(c => c.Label);
        var activity = _frame == null ? Localize.Get("Room_Preview") : _frame.Active == 0
            ? Localize.Get("Room_Quiet") : Localize.Get("Room_Active", string.Join(" · ", activeNames));
        if (ActivityLabel.Text != activity) ActivityLabel.Text = activity;
    }

    private void DrawPlanPose()
    {
        double cx = Map.ActualWidth / 2, cy = Map.Height / 2;
        var accent = Heading.Stroke;
        var secondary = PoseLabel.Foreground;
        var outline = Orbit.Stroke;
        foreach (var (bit, speaker) in _nodes)
        {
            var node = speaker.Node;
            var dot = speaker.Dot;
            Canvas.SetZIndex(node, _selection.Active == bit ? 3 : _selection.Contains(bit) ? 2 : 1);
            node.BorderBrush = _selection.Contains(bit) ? accent : outline;
            bool active = ((_frame?.Active ?? 0) & (1 << bit)) != 0;
            dot.Fill = active ? accent : secondary;
            double strength = ((_frame?.Level(bit) ?? -60) + 60) / 60.0;
            dot.Width = dot.Height = active ? 6 + 7 * strength : 5;
            dot.Opacity = active ? .6 + .4 * strength : .55;
        }
        double yaw = (_frame?.Yaw ?? 0) * Math.PI / 180;
        double pitch = (_frame?.Pitch ?? 0) * Math.PI / 180;
        double roll = (_frame?.Roll ?? 0) * Math.PI / 180;
        Canvas.SetLeft(Head, cx-12.5); Canvas.SetTop(Head, cy-12.5);
        Heading.X1 = cx; Heading.Y1 = cy;
        Heading.X2 = cx + 24 * Math.Sin(yaw) * Math.Cos(pitch);
        Heading.Y2 = cy - 19 * Math.Cos(yaw) * Math.Cos(pitch) - 16 * Math.Sin(pitch);
        double dx = 13 * Math.Cos(yaw) * Math.Cos(roll);
        double dy = 9 * Math.Sin(yaw) * Math.Cos(roll) + 9 * Math.Sin(roll);
        EarLine.X1 = cx-dx; EarLine.Y1 = cy-dy; EarLine.X2 = cx+dx; EarLine.Y2 = cy+dy;
        Heading.Opacity = EarLine.Opacity = _frame?.Yaw is null ? .35 : 1;
    }

}
