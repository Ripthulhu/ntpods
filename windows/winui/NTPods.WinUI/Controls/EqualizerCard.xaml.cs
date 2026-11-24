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
    private readonly DispatcherTimer _debounce = new() { Interval = TimeSpan.FromMilliseconds(150) };
    private bool _ready;
    private string _owner = "";
    private readonly DispatcherTimer _requestTimeout = new() { Interval = TimeSpan.FromSeconds(10) };

    public EqualizerCard()
    {
        InitializeComponent();
        _debounce.Tick += (_, _) => { _debounce.Stop(); _ready = true; Flush(); };
        _requestTimeout.Tick += (_, _) =>
        {
            _requestTimeout.Stop();
            _edit.Cancel();
            Render();
        };
        Unloaded += (_, _) =>
        {
            _requestTimeout.Stop();
            _debounce.Stop();
            _ready = false;
            _edit.Cancel();
        };
        Loaded += (_, _) => Render();
        _applying = false;
        Update(_snapshot);
    }

    public void Update(Snapshot s)
    {
        _snapshot = s;
        _owner = LiveSettingScope.For(s, Client?.SessionEpoch ?? 0);
        _edit.Update(s, Available(s) && Client?.IsReady == true, Client?.SessionEpoch ?? 0);
        if (_edit.Requested is null) _requestTimeout.Stop();
        Render();
        if (_ready) Flush();
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
        if (_edit.Displayed is { } eq)
        {
            _applying = true;
            LowSlider.Value = eq.Low;
            MidSlider.Value = eq.Mid;
            HighSlider.Value = eq.High;
            _applying = false;
        }
        RecommendedButton.IsEnabled = available;
        string status = Localize.Get(!s.Connected ? "DeviceSetting.Disconnected" :
            s.LinkStale ? "DeviceSetting.Stale" :
            !supported ? "Equalizer.Unsupported" :
            _edit.Dirty ? "DeviceSetting.Unsent" :
            s.EqPending is not null || _edit.Requested is not null ? "DeviceSetting.Pending" :
            s.Eq is null ? "Equalizer.Unknown" :
            s.Eq.Mode == 1 ? "Equalizer.RecommendedActive" : "Equalizer.CustomActive");
        var displayed = _edit.Displayed;
        bool showPreset = displayed is not null;
        StatusText.Text = showPreset
            ? Localize.Get(displayed!.Mode == 1 ? "Equalizer.RecommendedActive" : "Equalizer.CustomActive") : "";
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
        _edit.Edit(new Equalizer { Mode = 2, Low = (byte)LowSlider.Value, Mid = (byte)MidSlider.Value, High = (byte)HighSlider.Value });
        _ready = false;
        if (!_debounce.IsEnabled) _debounce.Start();
        Render();
    }

    private void Recommended_Click(object sender, RoutedEventArgs e)
    {
        if (!RecommendedButton.IsEnabled || Client is null) return;
        var requested = new Equalizer { Mode = 1, Low = 50, Mid = 50, High = 50 };
        _edit.Edit(requested);
        _debounce.Stop();
        _ready = true;
        Flush();
        Render();
    }

    private void Flush()
    {
        if (!Available(_snapshot) || Client?.IsReady != true ||
            _owner != LiveSettingScope.For(_snapshot, Client.SessionEpoch) || _edit.Take() is not { } requested) return;
        _requestTimeout.Stop();
        _requestTimeout.Start();
        if (!Client.Send(new SetEqCmd { Mode = requested.Mode, Low = requested.Low, Mid = requested.Mid, High = requested.High }))
            _edit.Cancel();
        Render();
    }
}
