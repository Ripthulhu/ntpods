using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;

namespace NTPods.WinUI.Popup;

public sealed partial class IslandView : UserControl
{
    private readonly IslandGaugeMotion[] _gauges = [new(), new(), new()];
    private string? _artFamily;
    private string _model = "";
    private readonly DeviceArtworkState _artwork = new();
    private bool _connectionMessage = true;
    private readonly IslandMessageMotion _message = new();
    private string _description = "";
    public bool Animating => _message.Animating || _gauges.Any(g => g.Animating);

    public IslandView()
    {
        InitializeComponent();
        _ = new ArtworkTransitions(this, DeviceImage);
    }

    public void Apply(Snapshot s, double now, bool animate)
    {
        _connectionMessage = true;
        DeviceName.Text = DeviceModels.DisplayName(s, Localize.Get("Device_DefaultName"));
        SetMessage(Localize.Get(BatteryPresentation.StatusKey(s)), now, animate);
        UpdateSnapshot(s, now, animate);
    }

    public void ApplyMessage(string title, string body, Snapshot? snapshot, string? model, double now, bool animate)
    {
        _connectionMessage = false;
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
        if (_connectionMessage) SetMessage(Localize.Get(BatteryPresentation.StatusKey(s)), now, animate);
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
        MessageOffset.X = 7 * (1 - _message.Opacity);
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

    public void RenderFrame(double now, double headerOpacity, double batteryOpacity)
    {
        HeaderGrid.Opacity = headerOpacity;
        HeaderOffset.Y = 5 * (1 - headerOpacity);
        BatteryPanel.Opacity = batteryOpacity;
        BatteryOffset.Y = 7 * (1 - batteryOpacity);
        _message.Advance(now);
        RenderMessage();
        foreach (var gauge in _gauges) gauge.Advance(now);
        LeftRing.Value = _gauges[0].Value;
        RightRing.Value = _gauges[1].Value;
        CaseRing.Value = _gauges[2].Value;
    }

    public void FinishAnimations()
    {
        foreach (var gauge in _gauges) gauge.Finish();
        _message.Finish();
        RenderMessage();
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
