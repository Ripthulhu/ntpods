using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Automation.Peers;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;

namespace NTPods.WinUI.Popup;

public sealed partial class IslandView : UserControl
{
    private readonly IslandGaugeMotion[] _gauges = [new(), new(), new()];
    private string? _artFamily;
    private string _model = "";
    private string _deviceId = "";
    private readonly DeviceArtworkState _artwork = new();
    private bool _connectionMessage = true;
    private readonly IslandMessageMotion _message = new();
    private readonly IslandActionMotion _action = new();
    private bool _nearby;
    private string _status = "";
    private string _description = "";
    private bool _closing;
    private bool _animate;
    private bool _iconOnly;
    public bool Animating => !_closing && (_message.Animating || _action.Animating || _gauges.Any(g => g.Animating)
        || _animate && _action.State == IslandConnectState.Connecting);
    public bool IsConnectionPayload => _connectionMessage;
    public bool IsNearbyConnection => _connectionMessage && _nearby;
    public bool IsConnecting => IsNearbyConnection && _action.State == IslandConnectState.Connecting;
    public IslandConnectState ConnectState => _action.State;
    public DateTimeOffset? BatteryDisplayExpiresAt { get; private set; }
    public bool CanConnect => !_closing && IsNearbyConnection && _action.CanConnect;
    public event Action? ConnectRequested;

    public IslandView()
    {
        InitializeComponent();
        ConnectLabel.Text = Localize.Get("Action_Connect");
        RetryLabel.Text = Localize.Get("Playback.Retry");
    }

    public void Apply(Snapshot s, double now, bool animate)
    {
        _closing = false;
        _animate = animate;
        _connectionMessage = true;
        if (!string.Equals(_deviceId, s.DeviceId, StringComparison.OrdinalIgnoreCase))
            _action.Set(IslandConnectState.Hidden, now, false);
        _deviceId = s.DeviceId;
        DeviceName.Text = DeviceModels.DisplayName(s, Localize.Get("Device_DefaultName"));
        UpdateSnapshot(s, now, animate);
    }

    public void ApplyMessage(string title, string body, Snapshot? snapshot, string? model, double now, bool animate)
    {
        _closing = false;
        _animate = animate;
        _connectionMessage = false;
        _nearby = false;
        _action.Set(IslandConnectState.Hidden, now, animate);
        RenderAction();
        DeviceName.Text = string.IsNullOrWhiteSpace(title) ? DeviceModels.DisplayName(snapshot ?? new Snapshot { Model = model ?? "" }, Localize.Get("Device_DefaultName")) : title;
        string? modeKey = body == "Transparency" || body == Localize.Get("Anc_Transparency") ? "Anc_Transparency" :
            body == "Adaptive" || body == Localize.Get("Anc_Adaptive") ? "Anc_Adaptive" :
            body == "Noise Cancellation" || body == Localize.Get("Anc_NoiseCancellation") ? "Anc_NoiseCancellation" :
            body == "Off" || body == Localize.Get("Anc_Off") ? "Anc_Off" : null;
        var icon = modeKey switch
        {
            "Anc_Transparency" => IslandMessageIcon.Transparency,
            "Anc_Adaptive" => IslandMessageIcon.Adaptive,
            "Anc_NoiseCancellation" or "Anc_Off" => IslandMessageIcon.NoiseCancellation,
            _ => IslandMessageIcon.None
        };
        bool micOff = body == Localize.Get("Mic_Off") || body == Localize.Get("Mic_Disabled");
        bool micMessage = micOff || body == Localize.Get("Mic_Ready") || body == Localize.Get("Mic_Enabled") ||
            body == Localize.Get("Mic_Starting") || body == Localize.Get("Mic_ShortcutUnavailable") ||
            body == Localize.Get("Mic_ConnectFirst") || body == Localize.Get("Mic_RecoveringShort") ||
            body == Localize.Get("Mic_NeedsAttention");
        if (micMessage) icon = micOff ? IslandMessageIcon.MicrophoneOff : IslandMessageIcon.Microphone;
        SetMessage(modeKey is null ? body : Localize.Get(modeKey), now, animate, icon);
        UpdateSnapshot(snapshot ?? new Snapshot { Model = model ?? "" }, now, animate);
    }

    public void UpdateSnapshot(Snapshot s, double now, bool animate, DateTimeOffset? utcNow = null)
    {
        var batteryNow = utcNow ?? DateTimeOffset.UtcNow;
        BatteryDisplayExpiresAt = IslandBatteryPresentation.CaseGraceExpiresAt(s, batteryNow);
        _animate = animate;
        SetImage(s);
        if (_connectionMessage)
        {
            _nearby = !s.AudioOutputReady && BatteryPresentation.IsLive(s) && BatteryPresentation.HasData(s);
            _status = Localize.Get(s.AudioOutputReady
                ? s.LinkStale ? "Status_ControlUnavailable" : "Status_Connected"
                : _nearby ? BatteryPresentation.IsCharging(s) ? "Battery.NearbyCharging" : "Battery.Nearby"
                : "Status_Disconnected");
            if (!_nearby) _action.Set(IslandConnectState.Hidden, now, animate);
            SetMessage(ConnectionStatus(), now, animate);
            RenderAction();
        }
        bool headphone = DeviceModels.Find(_model).Headphone || DeviceModels.Headphone(s);
        LeftLabel.Text = headphone ? DeviceModels.DisplayName(s, Localize.Get("Device_DefaultName")) : Localize.Get("BatteryLeftLabel_Text");
        RightLabel.Text = Localize.Get("BatteryRightLabel_Text");
        CaseLabel.Text = Localize.Get("BatteryCaseLabel_Text");
        RightCell.Visibility = CaseCell.Visibility = headphone ? Visibility.Collapsed : Visibility.Visible;
        Grid.SetColumnSpan(LeftCell, headphone ? 3 : 1);
        Grid.SetColumnSpan(LeftFreshness, headphone ? 3 : 1);
        LeftCell.HorizontalAlignment = headphone ? HorizontalAlignment.Center : HorizontalAlignment.Stretch;
        SetBattery(0, LeftRing, LeftText, LeftBolt, LeftCell, LeftLabel.Text,
            s, headphone ? BatteryPart.Headphone : BatteryPart.Left, now, animate, batteryNow);
        SetBattery(1, RightRing, RightText, RightBolt, RightCell, RightLabel.Text, s, BatteryPart.Right, now, animate, batteryNow);
        SetBattery(2, CaseRing, CaseText, CaseBolt, CaseCell, CaseLabel.Text, s, BatteryPart.Case, now, animate, batteryNow);
        string freshness = IslandBatteryPresentation.SharedFreshnessText(s, headphone, batteryNow);
        FreshnessText.Text = freshness;
        SetFreshness(LeftFreshness, s, headphone ? BatteryPart.Headphone : BatteryPart.Left, freshness, batteryNow);
        SetFreshness(RightFreshness, s, BatteryPart.Right, freshness, batteryNow, headphone);
        SetFreshness(CaseFreshness, s, BatteryPart.Case, freshness, batteryNow, headphone);
        RefreshDescription();
    }

    private static void SetFreshness(TextBlock text, Snapshot snapshot, BatteryPart part, string shared,
        DateTimeOffset utcNow, bool hidden = false)
    {
        text.Text = !hidden && shared.Length == 0 && BatteryPresentation.Reading(snapshot, part).HasValue
            && !IslandBatteryPresentation.IsCurrentForDisplay(snapshot, part, utcNow)
            ? IslandBatteryPresentation.FreshnessText(snapshot, part, utcNow) : "";
    }

    public void SetConnectState(IslandConnectState state, double now, bool animate)
    {
        _animate = animate;
        _action.Set(IsNearbyConnection ? state : IslandConnectState.Hidden, now, animate);
        if (_connectionMessage) SetMessage(ConnectionStatus(), now, animate);
        RenderAction();
        RefreshDescription();
    }

    private string ConnectionStatus() => _action.State switch
    {
        IslandConnectState.Connecting => Localize.Get("Status_Connecting"),
        IslandConnectState.Failed => Localize.Get("Island_ConnectFailed"),
        _ => _status,
    };

    private void RefreshDescription()
    {
        bool headphone = RightCell.Visibility == Visibility.Collapsed;
        string freshness = string.Join(", ", new[] { FreshnessText.Text,
            LeftFreshness.Text.Length > 0 ? $"{LeftLabel.Text} {LeftFreshness.Text}" : "",
            !headphone && RightFreshness.Text.Length > 0 ? $"{RightLabel.Text} {RightFreshness.Text}" : "",
            !headphone && CaseFreshness.Text.Length > 0 ? $"{CaseLabel.Text} {CaseFreshness.Text}" : "" }
            .Where(x => x.Length > 0));
        string detail = string.Join(", ", new[] { $"{LeftLabel.Text} {LeftText.Text}",
            headphone ? "" : $"{RightLabel.Text} {RightText.Text}", headphone ? "" : $"{CaseLabel.Text} {CaseText.Text}", freshness }
            .Where(x => !string.IsNullOrWhiteSpace(x)));
        string description = $"{DeviceName.Text}, {_message.Latest.Text}, {detail}";
        if (_description != description)
        {
            _description = description;
            AutomationProperties.SetName(this, description);
            ToolTipService.SetToolTip(this, description);
        }
    }

    private void SetBattery(int index, ProgressRing ring, TextBlock text, FrameworkElement bolt, FrameworkElement cell,
        string label, Snapshot snapshot, BatteryPart part, double now, bool animate, DateTimeOffset utcNow)
    {
        byte? value = BatteryPresentation.Reading(snapshot, part);
        bool current = IslandBatteryPresentation.IsCurrentForDisplay(snapshot, part, utcNow);
        bool charging = BatteryPresentation.IsCharging(snapshot, part);
        string freshness = IslandBatteryPresentation.FreshnessText(snapshot, part, utcNow);
        _gauges[index].Set(value, now, animate);
        text.Opacity = current ? 1 : 0.60;
        AutomationProperties.SetHelpText(cell, freshness);
        ToolTipService.SetToolTip(cell, string.IsNullOrEmpty(freshness) ? null : freshness);
        text.Text = value.HasValue ? $"{value}%" : "—";
        bolt.Visibility = value.HasValue && charging ? Visibility.Visible : Visibility.Collapsed;
        string prefix = index == 0 ? "Left" : index == 1 ? "Right" : "Case";
        VisualStateManager.GoToState(this, prefix + (!current ? "Cached" : value is <= 25 ? "Low" : "Normal"), false);
        AutomationProperties.SetName(cell, $"{label}, {text.Text}" + (bolt.Visibility == Visibility.Visible ? ", " + Localize.Get("Battery_Charging") : ""));
        ring.Value = _gauges[index].Value;
    }

    private void SetMessage(string body, double now, bool animate, IslandMessageIcon icon = IslandMessageIcon.None)
    {
        _message.Set(new(body, icon), now, animate);
        RenderMessage();
    }

    private void RenderMessage()
    {
        PreviousMessage.Text = _message.Previous.Text;
        MessageBody.Text = _message.Current.Text;
        double height = Math.Max(StatusViewport.MinHeight, StatusViewport.ActualHeight);
        PreviousMessageOffset.Y = -height * _message.Progress;
        MessageOffset.Y = height * (1 - _message.Progress);
        double IconOpacity(IslandMessageIcon icon) =>
            (_message.Previous.Icon == icon ? 1 - _message.Progress : 0) +
            (_message.Current.Icon == icon ? _message.Progress : 0);
        TransparencyIcon.Opacity = IconOpacity(IslandMessageIcon.Transparency);
        AdaptiveIcon.Opacity = IconOpacity(IslandMessageIcon.Adaptive);
        NoiseCancellationIcon.Opacity = IconOpacity(IslandMessageIcon.NoiseCancellation);
        MicIcon.Opacity = IconOpacity(IslandMessageIcon.Microphone);
        MicOffIcon.Opacity = IconOpacity(IslandMessageIcon.MicrophoneOff);
        MessageIcons.Visibility = _message.Current.Icon != IslandMessageIcon.None ||
            _message.Animating && _message.Previous.Icon != IslandMessageIcon.None ? Visibility.Visible : Visibility.Collapsed;
    }

    private void StatusViewport_SizeChanged(object sender, SizeChangedEventArgs e)
    {
        StatusViewport.Clip = new RectangleGeometry { Rect = new Windows.Foundation.Rect(0, 0, e.NewSize.Width, e.NewSize.Height) };
        RenderMessage();
    }

    protected override Windows.Foundation.Size MeasureOverride(Windows.Foundation.Size availableSize)
    {
        double width = double.IsFinite(availableSize.Width) ? availableSize.Width : Width;
        bool iconOnly = width < 340;
        if (_iconOnly != iconOnly)
        {
            _iconOnly = iconOnly;
            ActionColumn.Width = new GridLength(_iconOnly ? 48 : 108);
            ConnectButton.Width = _iconOnly ? 48 : 108;
            RenderAction();
        }
        double available = (width - Payload.Padding.Left - Payload.Padding.Right - 2 * BatteryRow.ColumnSpacing) / 3;
        BatteryMeasure.Measure(new Windows.Foundation.Size(double.PositiveInfinity, double.PositiveInfinity));
        var orientation = RightCell.Visibility == Visibility.Visible && BatteryMeasure.DesiredSize.Width + 34 > available
            ? Orientation.Vertical : Orientation.Horizontal;
        LeftReadout.Orientation = RightReadout.Orientation = CaseReadout.Orientation = orientation;
        return base.MeasureOverride(availableSize);
    }

    private void RenderAction()
    {
        double opacity = _action.Visibility;
        ConnectButton.Opacity = opacity;
        MessageIcons.Opacity = 1 - opacity;
        ConnectButton.IsEnabled = _action.CanConnect;
        ConnectButton.IsHitTestVisible = ConnectButton.IsTabStop = CanConnect;
        AutomationProperties.SetAccessibilityView(ConnectButton, CanConnect ? AccessibilityView.Content : AccessibilityView.Raw);
        AutomationProperties.SetName(ConnectButton, Localize.Get(_action.State == IslandConnectState.Failed
            ? "Playback.Retry" : _action.State == IslandConnectState.Connecting ? "Status_Connecting" : "Action_Connect"));
        ConnectLabel.Opacity = _iconOnly ? 0 : _action.Opacity(IslandConnectState.Ready);
        ConnectIcon.Opacity = _iconOnly ? _action.Opacity(IslandConnectState.Ready) : 0;
        ConnectProgress.Opacity = _action.Opacity(IslandConnectState.Connecting);
        RetryLabel.Opacity = _iconOnly ? 0 : _action.Opacity(IslandConnectState.Failed);
        RetryIcon.Opacity = _iconOnly ? _action.Opacity(IslandConnectState.Failed) : 0;
    }

    private void ConnectButton_Click(object sender, RoutedEventArgs e)
    {
        if (CanConnect) ConnectRequested?.Invoke();
    }

    internal void RenderFrame(double now, IslandMotion motion)
    {
        PayloadTransform.ScaleX = PayloadTransform.ScaleY = motion.PayloadScale;
        PayloadTransform.TranslateY = motion.PayloadOffset;
        if (!_closing && motion.Closing)
        {
            ArtworkTransitions.CancelPendingSource(DeviceImage);
            _artFamily = null;
        }
        _closing = motion.Closing;
        if (!_closing)
        {
            _message.Advance(now);
            _action.Advance(now);
            foreach (var gauge in _gauges) gauge.Advance(now);
            if (_animate && _action.State == IslandConnectState.Connecting) ProgressRotation.Angle = now % 1000 * 0.36;
        }
        RenderMessage();
        RenderAction();
        LeftRing.Value = _gauges[0].Value;
        RightRing.Value = _gauges[1].Value;
        CaseRing.Value = _gauges[2].Value;
    }

    public void FinishAnimations()
    {
        _animate = false;
        foreach (var gauge in _gauges) gauge.Finish();
        _message.Finish();
        _action.Finish();
        RenderMessage();
        RenderAction();
    }

    private void SetImage(Snapshot snapshot)
    {
        _model = _artwork.Update(snapshot, AppSettings.LastModel, AppSettings.LastDeviceId);
        bool headphone = DeviceModels.Find(_model).Headphone || DeviceModels.Headphone(snapshot);
        string family = DeviceArt.Family(_model, headphone);
        if (family == _artFamily) return;
        string image = DeviceArt.IslandImage(_model, headphone);
        ArtworkTransitions.SetSource(DeviceImage, image);
        _artFamily = family;
    }
}
