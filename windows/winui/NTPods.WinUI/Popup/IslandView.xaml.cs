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
    private string? _modeIcon;
    private bool _connectionMessage = true;
    private double _messageStarted;
    private bool _messageAnimating;
    private string _description = "";
    public bool Animating => _messageAnimating || _gauges.Any(g => g.Animating);

    public IslandView()
    {
        InitializeComponent();
        _ = new ArtworkTransitions(this, DeviceImage);
    }

    public void Apply(Snapshot s, double now, bool animate)
    {
        _connectionMessage = true;
        DeviceName.Text = DeviceModels.DisplayName(s, Localize.Get("Device_DefaultName"));
        ModeIcon.Visibility = Visibility.Collapsed;
        MicIcon.Visibility = Visibility.Collapsed;
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
        SetMessage(modeKey is null ? body : Localize.Get(modeKey), now, animate);
        string? icon = modeKey switch
        {
            "Anc_Transparency" => "ic_transparency.png",
            "Anc_Adaptive" => "ic_adaptive.png",
            "Anc_NoiseCancellation" or "Anc_Off" => "ic_noise_cancellation.png",
            _ => null
        };
        ModeIcon.Visibility = icon is null ? Visibility.Collapsed : Visibility.Visible;
        bool micOff = body == Localize.Get("Mic_Off") || body == Localize.Get("Mic_Disabled");
        bool micMessage = micOff || body == Localize.Get("Mic_Ready") || body == Localize.Get("Mic_Enabled") ||
            body == Localize.Get("Mic_Starting") || body == Localize.Get("Mic_ShortcutUnavailable") ||
            body == Localize.Get("Mic_ConnectFirst") || body == Localize.Get("Mic_RecoveringShort") ||
            body == Localize.Get("Mic_NeedsAttention");
        MicIcon.Visibility = micMessage ? Visibility.Visible : Visibility.Collapsed;
        MicOffSlash.Visibility = micOff ? Visibility.Visible : Visibility.Collapsed;
        if (icon is not null && icon != _modeIcon)
        {
            ModeIcon.UriSource = new Uri("ms-appx:///Assets/" + icon);
            _modeIcon = icon;
        }
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
        string description = $"{DeviceName.Text}, {MessageBody.Text}, {detail}";
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

    private void SetMessage(string body, double now, bool animate)
    {
        if (MessageBody.Text == body) return;
        PreviousMessage.Text = MessageBody.Text;
        MessageBody.Text = body;
        _messageStarted = now;
        _messageAnimating = animate && !string.IsNullOrEmpty(PreviousMessage.Text);
        if (!_messageAnimating) { PreviousMessage.Opacity = 0; MessageBody.Opacity = ModeIcon.Opacity = MicIcon.Opacity = 1; MessageOffset.X = 0; }
    }

    public void RenderFrame(double now, double headerOpacity, double batteryOpacity)
    {
        HeaderGrid.Opacity = headerOpacity;
        HeaderOffset.Y = 5 * (1 - headerOpacity);
        BatteryPanel.Opacity = batteryOpacity;
        BatteryOffset.Y = 7 * (1 - batteryOpacity);
        if (_messageAnimating)
        {
            double t = Math.Clamp((now - _messageStarted) / 180, 0, 1);
            double eased = 1 - Math.Pow(1 - t, 3);
            PreviousMessage.Opacity = 1 - eased;
            MessageBody.Opacity = ModeIcon.Opacity = MicIcon.Opacity = eased;
            MessageOffset.X = 7 * (1 - eased);
            _messageAnimating = t < 1;
        }
        foreach (var gauge in _gauges) gauge.Advance(now);
        LeftRing.Value = _gauges[0].Value;
        RightRing.Value = _gauges[1].Value;
        CaseRing.Value = _gauges[2].Value;
    }

    public void FinishAnimations()
    {
        foreach (var gauge in _gauges) gauge.Finish();
        _messageAnimating = false;
        PreviousMessage.Opacity = 0;
        MessageBody.Opacity = ModeIcon.Opacity = MicIcon.Opacity = 1;
        MessageOffset.X = 0;
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
