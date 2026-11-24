using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using NTPods.WinUI.Services;

namespace NTPods.WinUI.Popup;

public sealed partial class ConnectPromptView : UserControl
{
    private readonly TranslateTransform _sheet = new();
    private bool _headphoneModel;
    private string _deviceName = "";
    private string? _setupInstructions;
    public event Action? ConnectRequested;
    public event Action? DismissRequested;

    public ConnectPromptView()
    {
        InitializeComponent();
        CardScroll.RenderTransform = _sheet;
        InitializeTransitions();
        InitializeArtwork();
    }

    public void SetName(string name, ushort? model)
    {
        _deviceName = name;
        _setupInstructions = null;
        ContentViewport.MinHeight = 96;
        ModelHeadingMeasure.Text = name;
        string setup = model is ushort known ? PairingInstructions.ForModel(known) : PairingInstructions.ForName(name);
        SetupHeadingMeasure.Text = Localize.Get(PairingInstructions.HeadingKey(true, setup, null)!);
        SetPresentationText(name, _statusRequested);
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(DeviceName, name);
        string? catalogModel = model is ushort id ? PairingInvitationPolicy.NearbyCatalogModel(id) : null;
        _headphoneModel = DeviceModels.Find(catalogModel).Headphone ||
            name.Contains("AirPods Max", StringComparison.OrdinalIgnoreCase);
        SetArtworkModel(catalogModel);
    }

    public void Fit(double height)
    {
        CardScroll.MaxHeight = height;
        ArtworkHost.Height = Math.Clamp(height - 260, 80, 160);
    }

    public void SetState(bool busy, string statusKey, bool retry = false, bool beforeConnect = false)
    {
        if (beforeConnect)
        {
            _setupInstructions = statusKey;
            SetupHeadingMeasure.Text = Localize.Get(PairingInstructions.HeadingKey(true, statusKey, null)!);
        }
        string? heading = PairingInstructions.HeadingKey(busy, statusKey, _setupInstructions);
        SetPresentationText(heading is null ? _deviceName : Localize.Get(heading), Localize.Get(statusKey) +
            (beforeConnect ? $"\n\n{Localize.Get("Pairing.ThenConnect")}" : ""));
        ConnectLabel.Text = Localize.Get(retry ? "Playback.Retry" : "ConnectPrompt_Connect");
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(ConnectButton, ConnectLabel.Text);
        TransitionContent(paired: false, busy);
    }

    public void SetPaired()
    {
        SetBattery(null, null, null, false, false, false, _headphoneModel, null, false);
        SetPresentationText(_deviceName, _statusRequested);
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(ConnectButton, ContinueLabel.Text);
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(DeviceName,
            $"{_deviceName}. {Localize.Get("ConnectPrompt_Paired")}");
        TransitionContent(paired: true, busy: false);
    }

    public bool SetBattery(byte? left, byte? right, byte? caseLevel,
        bool leftCharging, bool rightCharging, bool caseCharging,
        bool headphone, byte? headphoneLevel, bool headphoneCharging, bool caseLive = true)
    {
        left = Valid(left);
        right = Valid(right);
        caseLevel = Valid(caseLevel);
        headphoneLevel = Valid(headphoneLevel);
        headphone |= _headphoneModel;
        headphoneCharging &= headphoneLevel.HasValue;
        if (headphone && !_headphoneModel)
        {
            _headphoneModel = true;
            SetArtworkModel(null);
        }
        bool wasHeadphone = CaseCell.Visibility == Visibility.Collapsed;
        bool wasSplit = RightRow.Visibility == Visibility.Visible;
        CaseCell.Visibility = headphone ? Visibility.Collapsed : Visibility.Visible;
        Grid.SetColumnSpan(BudsCell, headphone ? 3 : 1);
        RightBatteryColumn.Width = new GridLength(0);
        if (headphone)
        {
            BudsLabel.Text = Localize.Get("Device_DefaultName");
            BudsValue.Text = Percent(headphoneLevel);
            BudsValue.FontSize = 18;
            BudsRing.Value = headphoneLevel ?? 0;
            BudsRing.Opacity = headphoneLevel.HasValue ? 1 : 0.4;
            BudsCharging.Visibility = headphoneCharging ? Visibility.Visible : Visibility.Collapsed;
            RightRow.Visibility = Visibility.Collapsed;
            Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(BudsValue,
                Description(BudsLabel.Text, headphoneLevel, headphoneCharging));
            UpdateContentInput();
            return !wasHeadphone || wasSplit;
        }
        leftCharging &= left.HasValue;
        rightCharging &= right.HasValue;
        caseCharging &= caseLevel.HasValue && caseLive;
        bool split = PairingBatteryLayout.Split(left, right, leftCharging, rightCharging);
        RightBatteryColumn.Width = split ? new GridLength(1, GridUnitType.Star) : new GridLength(0);
        BudsLabel.Text = Localize.Get(split ? "BatteryLeftLabel_Text" : "ConnectPrompt_AirPods");
        BudsValue.Text = Percent(left);
        RightValue.Text = Percent(right);
        RightRow.Visibility = split ? Visibility.Visible : Visibility.Collapsed;
        BudsValue.FontSize = 18;
        BudsRing.Value = left ?? 0;
        BudsRing.Opacity = left.HasValue ? 1 : 0.4;
        BudsCharging.Visibility = left.HasValue && leftCharging ? Visibility.Visible : Visibility.Collapsed;
        RightRing.Value = right ?? 0;
        RightRing.Opacity = right.HasValue ? 1 : 0.4;
        RightCharging.Visibility = right.HasValue && rightCharging ? Visibility.Visible : Visibility.Collapsed;
        CaseValue.Text = Percent(caseLevel);
        CaseRing.Value = caseLevel ?? 0;
        CaseRing.Opacity = CaseValue.Opacity = caseLevel.HasValue && caseLive ? 1 : 0.4;
        ToolTipService.SetToolTip(CaseValue, caseLevel.HasValue && !caseLive ? Localize.Get("Battery.LastKnown") : null);
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetHelpText(CaseValue,
            caseLevel.HasValue && !caseLive ? Localize.Get("Battery.LastKnown") : "");
        CaseCharging.Visibility = caseLevel.HasValue && caseCharging ? Visibility.Visible : Visibility.Collapsed;
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(BudsValue,
            Description(split ? Localize.Get("BatteryLeftLabel_Text") : BudsLabel.Text, left, leftCharging));
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(RightValue,
            Description(Localize.Get("BatteryRightLabel_Text"), right, rightCharging));
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(CaseValue,
            Description(Localize.Get("BatteryCaseLabel_Text"), caseLevel, caseCharging));
        UpdateContentInput();
        return wasHeadphone || wasSplit != split;
    }

    private static byte? Valid(byte? level) => level is <= 100 ? level : null;
    private static string Percent(byte? level) => level is byte value ? $"{value}%" : "—";
    private static string Description(string label, byte? level, bool charging) =>
        $"{label}, {Percent(level)}" + (level.HasValue && charging ? $", {Localize.Get("Battery_Charging")}" : "");

    public void Render(double expansion, double height)
    {
        _sheet.Y = (1 - expansion) * height;
        UpdateArtworkEntrance(expansion);
    }
    private void Connect_Click(object sender, RoutedEventArgs args) => ConnectRequested?.Invoke();
    private void Close_Click(object sender, RoutedEventArgs args) => DismissRequested?.Invoke();
}
