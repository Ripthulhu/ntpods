using System;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;

namespace NTPods.WinUI.Controls;

/// Battery readings and artwork for earbuds, cases and headphones.
public sealed partial class BatteryCard : UserControl
{
    // Avoid decoding the same artwork on every snapshot.
    private string? _artFamily;
    private string _model = "";
    private readonly DeviceArtworkState _artwork = new();

    public BatteryCard()
    {
        InitializeComponent();
        _ = new ArtworkTransitions(this, LeftImg, RightImg, CaseImg);
    }

    /// Render the model's battery gauges and artwork from a daemon snapshot.
    public void Update(Snapshot s)
    {
        _model = _artwork.Update(s, AppSettings.LastModel, AppSettings.LastDeviceId);
        bool headphone = DeviceModels.Find(_model).Headphone || DeviceModels.Headphone(s);
        var family = DeviceArt.Family(_model, headphone);
        if (family != _artFamily)
        {
            _artFamily = family;
            ArtworkTransitions.SetSource(LeftImg, headphone ? DeviceArt.MainImage(_model, true) : DeviceArt.LeftImage(_model));
            ArtworkTransitions.SetSource(RightImg, headphone ? null : DeviceArt.RightImage(_model));
            ArtworkTransitions.SetSource(CaseImg, headphone ? null : DeviceArt.CaseImage(_model));
        }

        RightCell.Visibility = CaseCell.Visibility = headphone ? Visibility.Collapsed : Visibility.Visible;
        Grid.SetColumnSpan(LeftCell, headphone ? 3 : 1);
        LeftLabel.Text = headphone ? DeviceModels.DisplayName(s, Localize.Get("Device_DefaultName")) : Localize.Get("BatteryLeftLabel_Text");
        AutomationProperties.SetName(LeftBar, LeftLabel.Text);
        SetBattery(s, headphone ? BatteryPart.Headphone : BatteryPart.Left, LeftCell, LeftArtwork, LeftBar, LeftText, LeftCharging);
        SetBattery(s, BatteryPart.Right, RightCell, RightArtwork, RightBar, RightText, RightCharging);
        SetBattery(s, BatteryPart.Case, CaseCell, CaseArtwork, CaseBar, CaseText, CaseCharging);
    }

    private static void SetBattery(Snapshot s, BatteryPart part, FrameworkElement cell, FrameworkElement artwork,
        ProgressRing bar, TextBlock text, FrameworkElement bolt)
    {
        byte? value = BatteryPresentation.Reading(s, part);
        bool charging = BatteryPresentation.IsCharging(s, part);
        string freshness = BatteryPresentation.FreshnessText(s, part);
        artwork.Opacity = BatteryPresentation.ArtworkIsAvailable(s, part) ? 1 : 0.65;
        bar.Opacity = text.Opacity = BatteryPresentation.IsLive(s, part) ? 1 : 0.65;
        ToolTipService.SetToolTip(cell, string.IsNullOrEmpty(freshness) ? null : freshness);
        AutomationProperties.SetHelpText(cell, freshness);
        if (value is byte v)
        {
            bar.Value = v;
            bar.IsIndeterminate = false;
            text.Text = $"{v}%";
            bolt.Visibility = charging ? Visibility.Visible : Visibility.Collapsed;
            string help = string.Join(", ", new[] { charging ? Localize.Get("Battery_Charging") : "", freshness }
                .Where(part => !string.IsNullOrEmpty(part)));
            AutomationProperties.SetHelpText(bar, help);
            AutomationProperties.SetHelpText(text, help);
        }
        else
        {
            bar.Value = 0;
            text.Text = "—";
            bolt.Visibility = Visibility.Collapsed;
            AutomationProperties.SetHelpText(bar, Localize.Get("Tray_NoBatteryData"));
            AutomationProperties.SetHelpText(text, Localize.Get("Tray_NoBatteryData"));
        }
    }
}
