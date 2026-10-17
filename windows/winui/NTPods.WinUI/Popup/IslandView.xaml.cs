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
    private readonly DeviceArtworkState _artwork = new();
    private bool _connectionMessage = true;
    private readonly IslandMessageMotion _message = new();
    private readonly IslandActionMotion _action = new();
    private bool _nearby;
    private string _status = "";
    private string _description = "";
    public bool Animating => _message.Animating || _action.Animating || _gauges.Any(g => g.Animating);
    public bool IsConnectionPayload => _connectionMessage;
    public bool IsNearbyConnection => _connectionMessage && _nearby;
    public bool IsConnecting => IsNearbyConnection && _action.State == IslandConnectState.Connecting;
    public IslandConnectState ConnectState => _action.State;
    public bool CanConnect => IsNearbyConnection && _action.CanConnect;
    public event Action? ConnectRequested;

    public IslandView()
    {
        InitializeComponent();
        _ = new ArtworkTransitions(this, DeviceImage);
        ConnectLabel.Text = Localize.Get("Action_Connect");
        RetryLabel.Text = Localize.Get("Playback.Retry");
    }

    public void Apply(Snapshot s, double now, bool animate)
    {
        _connectionMessage = true;
        _action.Set(IslandConnectState.Hidden, now, false);
        DeviceName.Text = DeviceModels.DisplayName(s, Localize.Get("Device_DefaultName"));
        UpdateSnapshot(s, now, animate);
    }

    public void ApplyMessage(string title, string body, Snapshot? snapshot, string? model, double now, bool animate)
    {
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

    public void UpdateSnapshot(Snapshot s, double now, bool animate)
    {
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
        bool live = BatteryPresentation.IsLive(s);
        bool headphone = DeviceModels.Find(_model).Headphone || DeviceModels.Headphone(s);
        LeftLabel.Text = headphone ? DeviceModels.DisplayName(s, Localize.Get("Device_DefaultName")) : Localize.Get("BatteryLeftLabel_Text");
        RightLabel.Text = Localize.Get("BatteryRightLabel_Text");
        CaseLabel.Text = Localize.Get("BatteryCaseLabel_Text");
        RightCell.Visibility = CaseCell.Visibility = headphone ? Visibility.Collapsed : Visibility.Visible;
        Grid.SetColumnSpan(LeftCell, headphone ? 3 : 1);
        LeftCell.HorizontalAlignment = headphone ? HorizontalAlignment.Center : HorizontalAlignment.Stretch;
        SetBattery(0, LeftRing, LeftText, LeftBolt, LeftCell, LeftLabel.Text,
            s, headphone ? BatteryPart.Headphone : BatteryPart.Left, now, animate);
        SetBattery(1, RightRing, RightText, RightBolt, RightCell, RightLabel.Text, s, BatteryPart.Right, now, animate);
        SetBattery(2, CaseRing, CaseText, CaseBolt, CaseCell, CaseLabel.Text, s, BatteryPart.Case, now, animate);
        string freshness = BatteryPresentation.FreshnessText(s);
        FreshnessText.Text = freshness;
        FreshnessText.Visibility = !live && BatteryPresentation.HasData(s) ? Visibility.Visible : Visibility.Collapsed;
        RefreshDescription();
    }

    public void SetConnectState(IslandConnectState state, double now, bool animate)
    {
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
        string freshness = FreshnessText.Text;
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
        string label, Snapshot snapshot, BatteryPart part, double now, bool animate)
    {
        byte? value = BatteryPresentation.Reading(snapshot, part);
        bool live = BatteryPresentation.IsLive(snapshot, part);
        bool charging = BatteryPresentation.IsCharging(snapshot, part);
        string freshness = BatteryPresentation.FreshnessText(snapshot, part);
        _gauges[index].Set(value, now, animate);
        cell.Opacity = live ? 1 : 0.60;
        AutomationProperties.SetHelpText(cell, freshness);
        ToolTipService.SetToolTip(cell, string.IsNullOrEmpty(freshness) ? null : freshness);
        text.Text = value.HasValue ? $"{value}%" : "—";
        bolt.Visibility = live && value.HasValue && charging ? Visibility.Visible : Visibility.Collapsed;
        string prefix = index == 0 ? "Left" : index == 1 ? "Right" : "Case";
        VisualStateManager.GoToState(this, prefix + (value is <= 20 ? "Low" : "Normal"), false);
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
        PreviousMessage.Opacity = 1 - _message.Opacity;
        MessageBody.Opacity = _message.Opacity;
        PreviousMessageOffset.Y = -8 * _message.Opacity;
        MessageOffset.Y = 8 * (1 - _message.Opacity);
        double IconOpacity(IslandMessageIcon icon) =>
            (_message.Previous.Icon == icon ? 1 - _message.Opacity : 0) +
            (_message.Current.Icon == icon ? _message.Opacity : 0);
        TransparencyIcon.Opacity = IconOpacity(IslandMessageIcon.Transparency);
        AdaptiveIcon.Opacity = IconOpacity(IslandMessageIcon.Adaptive);
        NoiseCancellationIcon.Opacity = IconOpacity(IslandMessageIcon.NoiseCancellation);
        MicIcon.Opacity = IconOpacity(IslandMessageIcon.Microphone);
        MicOffIcon.Opacity = IconOpacity(IslandMessageIcon.MicrophoneOff);
        MessageIcons.Visibility = _message.Current.Icon != IslandMessageIcon.None ||
            _message.Animating && _message.Previous.Icon != IslandMessageIcon.None ? Visibility.Visible : Visibility.Collapsed;
    }

    private void StatusViewport_SizeChanged(object sender, SizeChangedEventArgs e) =>
        StatusViewport.Clip = new RectangleGeometry { Rect = new Windows.Foundation.Rect(0, 0, e.NewSize.Width, e.NewSize.Height) };

    private void RenderAction()
    {
        double opacity = 1 - _action.Opacity(IslandConnectState.Hidden);
        ConnectButton.Visibility = opacity > 0 ? Visibility.Visible : Visibility.Collapsed;
        ConnectButton.Opacity = opacity;
        MessageIcons.Opacity = 1 - opacity;
        ConnectButton.IsEnabled = ConnectButton.IsHitTestVisible = CanConnect;
        AutomationProperties.SetAccessibilityView(ConnectButton, CanConnect ? AccessibilityView.Content : AccessibilityView.Raw);
        AutomationProperties.SetName(ConnectButton, Localize.Get(_action.State == IslandConnectState.Failed
            ? "Playback.Retry" : _action.State == IslandConnectState.Connecting ? "Status_Connecting" : "Action_Connect"));
        ConnectLabel.Opacity = _action.Opacity(IslandConnectState.Ready);
        ConnectProgress.Opacity = _action.Opacity(IslandConnectState.Connecting);
        ConnectProgress.IsActive = ConnectProgress.Opacity > 0;
        RetryLabel.Opacity = _action.Opacity(IslandConnectState.Failed);
    }

    private void ConnectButton_Click(object sender, RoutedEventArgs e)
    {
        if (CanConnect) ConnectRequested?.Invoke();
    }

    internal void RenderFrame(double now, IslandMotion motion)
    {
        PayloadTransform.ScaleX = PayloadTransform.ScaleY = motion.PayloadScale;
        PayloadTransform.TranslateY = motion.PayloadOffset;
        HeaderGrid.Opacity = motion.HeaderOpacity;
        BatteryPanel.Opacity = motion.BatteryOpacity;
        _message.Advance(now);
        RenderMessage();
        _action.Advance(now);
        RenderAction();
        foreach (var gauge in _gauges) gauge.Advance(now);
        LeftRing.Value = _gauges[0].Value;
        RightRing.Value = _gauges[1].Value;
        CaseRing.Value = _gauges[2].Value;
    }

    public void FinishAnimations()
    {
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
