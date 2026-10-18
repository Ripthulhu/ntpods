using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using NTPods.WinUI.Services;

namespace NTPods.WinUI.Popup;

public sealed partial class ConnectPromptView : UserControl
{
    private readonly TranslateTransform _sheet = new();
    private bool _headphoneModel;
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
        DeviceName.Text = name;
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
        SetStatusText(Localize.Get(statusKey) +
            (beforeConnect ? $"\n\n{Localize.Get("Pairing.ThenConnect")}" : ""));
        ConnectLabel.Text = Localize.Get(retry ? "Playback.Retry" : "ConnectPrompt_Connect");
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(ConnectButton, ConnectLabel.Text);
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(DeviceName, DeviceName.Text);
        TransitionContent(paired: false, busy);
    }

    public void SetPaired()
    {
        SetBattery(null, null, null, false, false, false, _headphoneModel, null, false);
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(ConnectButton, ContinueLabel.Text);
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(DeviceName,
            $"{DeviceName.Text}. {Localize.Get("ConnectPrompt_Paired")}");
        TransitionContent(paired: true, busy: false);
    }

    public bool SetBattery(byte? left, byte? right, byte? caseLevel,
        bool leftCharging, bool rightCharging, bool caseCharging,
        bool headphone, byte? headphoneLevel, bool headphoneCharging)
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
        Grid.SetColumnSpan(BudsCell, headphone ? 2 : 1);
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
        caseCharging &= caseLevel.HasValue;
        bool split = PairingBatteryLayout.Split(left, right, leftCharging, rightCharging);
        BudsLabel.Text = Localize.Get("ConnectPrompt_AirPods");
        BudsValue.Text = split
            ? $"{Localize.Get("BatteryLeftLabel_Text")} {Percent(left)}"
            : Percent(left);
        RightValue.Text = $"{Localize.Get("BatteryRightLabel_Text")} {Percent(right)}";
        RightRow.Visibility = split ? Visibility.Visible : Visibility.Collapsed;
        BudsValue.FontSize = split ? 14 : 18;
        BudsRing.Value = left ?? 0;
        BudsRing.Opacity = left.HasValue ? 1 : 0.4;
        BudsCharging.Visibility = left.HasValue && leftCharging ? Visibility.Visible : Visibility.Collapsed;
        RightCharging.Visibility = right.HasValue && rightCharging ? Visibility.Visible : Visibility.Collapsed;
        CaseValue.Text = Percent(caseLevel);
        CaseRing.Value = caseLevel ?? 0;
        CaseRing.Opacity = caseLevel.HasValue ? 1 : 0.4;
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
