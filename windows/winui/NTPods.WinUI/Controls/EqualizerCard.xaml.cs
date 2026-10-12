using System;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;

namespace NTPods.WinUI.Controls;

public sealed partial class EqualizerCard : UserControl
{
    public DaemonClient? Client { get; set; }
    private Snapshot _snapshot = new();
    private bool _applying = true;
    private readonly EqualizerEditState _edit = new();
    private readonly DispatcherTimer _requestTimeout = new() { Interval = TimeSpan.FromSeconds(10) };

    public EqualizerCard()
    {
        InitializeComponent();
        _requestTimeout.Tick += (_, _) =>
        {
            _requestTimeout.Stop();
            _edit.EndRequest();
            Render();
        };
        Unloaded += (_, _) =>
        {
            _requestTimeout.Stop();
            _edit.EndRequest();
        };
        Loaded += (_, _) => Render();
        _applying = false;
        Update(_snapshot);
    }

    public void Update(Snapshot s)
    {
        _snapshot = s;
        _edit.Update(s, Available(s));
        if (_edit.Requested is null) _requestTimeout.Stop();
        Render();
    }

    private static bool Supported(Snapshot s) => DeviceModels.CustomEq(s);

    private static bool Available(Snapshot s) => s.Connected && !s.LinkStale && Supported(s);

    private void Render()
    {
        var s = _snapshot;
        bool supported = Supported(s);
        bool available = Available(s);
        LowSlider.IsEnabled = available;
        MidSlider.IsEnabled = available;
        HighSlider.IsEnabled = available;
        // An unsubmitted slider edit belongs to the user, even as battery updates arrive.
        if (!_edit.Dirty)
        {
            var eq = s.EqPending ?? _edit.Requested ?? s.Eq ?? new Equalizer { Mode = 1, Low = 50, Mid = 50, High = 50 };
            _applying = true;
            LowSlider.Value = eq.Low;
            MidSlider.Value = eq.Mid;
            HighSlider.Value = eq.High;
            _applying = false;
        }
        ApplyButton.IsEnabled = available && (_edit.Dirty || s.Eq?.Mode != 2) && s.EqPending is null && _edit.Requested is null;
        RecommendedButton.IsEnabled = available && s.EqPending is null && _edit.Requested is null;
        string status = Localize.Get(!s.Connected ? "DeviceSetting.Disconnected" :
            s.LinkStale ? "DeviceSetting.Stale" :
            !supported ? "Equalizer.Unsupported" :
            _edit.Dirty ? "DeviceSetting.Unsent" :
            s.EqPending is not null || _edit.Requested is not null ? "DeviceSetting.Pending" :
            s.Eq is null ? "Equalizer.Unknown" :
            s.Eq.Mode == 1 ? "Equalizer.RecommendedActive" : "Equalizer.CustomActive");
        bool showPreset = s.Eq is not null;
        StatusText.Text = showPreset
            ? Localize.Get(s.Eq!.Mode == 1 ? "Equalizer.RecommendedActive" : "Equalizer.CustomActive") : "";
        StatusText.Visibility = showPreset ? Visibility.Visible : Visibility.Collapsed;
        AutomationProperties.SetHelpText(EqualizerExpander, status);
        AutomationProperties.SetHelpText(LowSlider, status);
        AutomationProperties.SetHelpText(MidSlider, status);
        AutomationProperties.SetHelpText(HighSlider, status);
        ToolTipService.SetToolTip(EqualizerExpander, available && !_edit.Dirty && s.EqPending is null && _edit.Requested is null && showPreset ? null : status);
        ValuesText.Text = Localize.Get("Equalizer.Values", (int)LowSlider.Value, (int)MidSlider.Value, (int)HighSlider.Value);
    }

    private void Band_Changed(object sender, RangeBaseValueChangedEventArgs e)
    {
        if (_applying || !Available(_snapshot)) return;
        _edit.Edit();
        Render();
    }

    private void Apply_Click(object sender, RoutedEventArgs e)
    {
        if (!ApplyButton.IsEnabled || Client is null) return;
        var requested = new Equalizer { Mode = 2, Low = (byte)LowSlider.Value, Mid = (byte)MidSlider.Value, High = (byte)HighSlider.Value };
        Send(requested);
    }

    private void Recommended_Click(object sender, RoutedEventArgs e)
    {
        if (!RecommendedButton.IsEnabled || Client is null) return;
        var requested = new Equalizer { Mode = 1, Low = 50, Mid = 50, High = 50 };
        Send(requested);
    }

    private void Send(Equalizer requested)
    {
        _edit.Begin(requested);
        _requestTimeout.Stop();
        _requestTimeout.Start();
        Client!.SetEq(requested);
        Render();
    }
}
