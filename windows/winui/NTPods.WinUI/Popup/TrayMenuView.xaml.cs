using System;
using System.Collections.Generic;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Controls;
using NTPods.WinUI.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media.Imaging;

namespace NTPods.WinUI.Popup;

/// The content of the custom tray menu: a themed, icon-per-row list rendered as a
/// UserControl so it follows the app theme and carries per-item artwork (things the
/// WinUI MenuFlyout / native Win32 menu can't do in the tray). Pure view — the
/// window owns positioning + dismissal; actions are surfaced as callbacks.
public sealed partial class TrayMenuView : UserControl
{
    public Action<byte>? OnAnc;
    public Action? OnMute;
    public Action? OnOpen;
    public Action? OnBatteries;
    public Action? OnQuit;
    public Action? OnDismiss;
    private string? _imageUri;
    private readonly DeviceArtworkState _artwork = new();

    public TrayMenuView()
    {
        InitializeComponent();
    }

    /// Fill the menu from the latest snapshot.
    public void Apply(Snapshot s)
    {
        HeaderName.Text = string.IsNullOrWhiteSpace(s.DevName) ? "NTPods" : s.DevName;

        var model = _artwork.Update(s, AppSettings.LastModel, AppSettings.LastDeviceId);
        string imageUri = DeviceArt.MainImage(model, DeviceModels.Headphone(s));
        try
        {
            if (_imageUri != imageUri)
            {
                HeaderImage.Source = new BitmapImage(new Uri(imageUri));
                _imageUri = imageUri;
            }
        }
        catch { }

        var parts = new List<string>();
        foreach (var (part, label) in new[] { (BatteryPart.Left, Localize.Get("Battery_LeftShort")),
            (BatteryPart.Right, Localize.Get("Battery_RightShort")), (BatteryPart.Headphone, ""), (BatteryPart.Case, Localize.Get("Tray_CaseShort")) })
            if (BatteryPresentation.ReadingText(s, part, label) is { Length: > 0 } text) parts.Add(text);
        bool live = BatteryPresentation.IsLive(s);
        string statusKey = BatteryPresentation.StatusKey(s);
        string status = Localize.Get(statusKey);
        if (parts.Count > 0 && !live) status += " · " + Localize.Get("Battery.LastKnown");
        else if (BatteryPresentation.IsCharging(s) && statusKey != "Battery.NearbyCharging")
            status += " · " + Localize.Get("Battery_Charging");
        BatteryText.Text = status + "\n" + (parts.Count > 0 ? string.Join("  ·  ", parts) : Localize.Get("Tray_NoBatteryData"));
        string freshness = BatteryPresentation.FreshnessText(s);
        AutomationProperties.SetHelpText(BatteryText, freshness);
        ToolTipService.SetToolTip(BatteryText, string.IsNullOrEmpty(freshness) ? null : freshness);

        AncLabel.Text = Localize.Get("Tray_NoiseControl");
        AncOffText.Text = Localize.Get("Anc_Off");
        AncNcText.Text = Localize.Get("Anc_NoiseCancellation");
        AncTransText.Text = Localize.Get("Anc_Transparency");
        AncAdaptiveText.Text = Localize.Get("Anc_Adaptive");

        AncLabel.Visibility = AncNcRow.Visibility = AncTransRow.Visibility = DeviceModels.Listening(s) ? Visibility.Visible : Visibility.Collapsed;
        AncOffRow.Visibility = DeviceModels.OffAvailable(s) ? Visibility.Visible : Visibility.Collapsed;
        AncAdaptiveRow.Visibility = DeviceModels.Adaptive(s) ? Visibility.Visible : Visibility.Collapsed;

        AncOffRow.IsEnabled = ListeningModePolicy.CanSend(s, false, 0x0D, 1);
        AncNcRow.IsEnabled = ListeningModePolicy.CanSend(s, false, 0x0D, 2);
        AncTransRow.IsEnabled = ListeningModePolicy.CanSend(s, false, 0x0D, 3);
        AncAdaptiveRow.IsEnabled = ListeningModePolicy.CanSend(s, false, 0x0D, 4);
        MuteRow.IsEnabled = s.Connected;
        AncOffCheck.Visibility = Check(s.Anc == 1);
        AncNcCheck.Visibility = Check(s.Anc == 2);
        AncTransCheck.Visibility = Check(s.Anc == 3);
        AncAdaptiveCheck.Visibility = Check(s.Anc == 4);

        MuteText.Text = Localize.Get(s.Muted ? "Action_Unmute" : "Action_Mute");
        OpenText.Text = Localize.Get("Action_Open");
        BatteriesText.Text = Localize.Get("Tray.ShowBatteries");
        BatteriesRow.Visibility = OnBatteries is null ? Visibility.Collapsed : Visibility.Visible;
        AutomationProperties.SetName(BatteriesRow, BatteriesText.Text);
        QuitText.Text = Localize.Get("Action_Quit");
    }

    private static Visibility Check(bool on) => on ? Visibility.Visible : Visibility.Collapsed;

    private void Fire(Action? a)
    {
        try { a?.Invoke(); } finally { OnDismiss?.Invoke(); }
    }

    private void AncOff_Click(object s, RoutedEventArgs e) => Fire(() => OnAnc?.Invoke(1));
    private void AncNc_Click(object s, RoutedEventArgs e) => Fire(() => OnAnc?.Invoke(2));
    private void AncTrans_Click(object s, RoutedEventArgs e) => Fire(() => OnAnc?.Invoke(3));
    private void AncAdaptive_Click(object s, RoutedEventArgs e) => Fire(() => OnAnc?.Invoke(4));
    private void Mute_Click(object s, RoutedEventArgs e) => Fire(() => OnMute?.Invoke());
    private void Open_Click(object s, RoutedEventArgs e) => Fire(() => OnOpen?.Invoke());
    private void Batteries_Click(object s, RoutedEventArgs e) => Fire(OnBatteries);
    private void Quit_Click(object s, RoutedEventArgs e) => Fire(() => OnQuit?.Invoke());
}
