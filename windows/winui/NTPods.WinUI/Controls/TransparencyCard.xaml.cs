using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

namespace NTPods.WinUI.Controls;

public sealed partial class TransparencyCard : UserControl
{
    public DaemonClient? Client { get; set; }
    private Snapshot _snapshot = new();
    private readonly TransparencyEditState _edit = new();
    private bool _applying = true;
    private readonly DispatcherTimer _timeout = new() { Interval = TimeSpan.FromSeconds(15) };

    public TransparencyCard()
    {
        InitializeComponent();
        _timeout.Tick += (_, _) => { _timeout.Stop(); _edit.Cancel(); Client?.RequestState(); Render(); };
        Unloaded += (_, _) => { _timeout.Stop(); _edit.Cancel(); };
        _applying = false;
        Render();
    }

    private bool Available => Client?.IsReady == true && _snapshot.Connected && !_snapshot.LinkStale &&
        DeviceModels.Find(_snapshot.Model).CustomTransparency;

    public void Update(Snapshot s)
    {
        _snapshot = s;
        _edit.Update(s, Available);
        if (!_edit.Requesting) _timeout.Stop();
        Render();
    }

    private void Render()
    {
        _applying = true;
        try
        {
            bool busy = _edit.Requesting || _snapshot.TransparencyPending;
            var value = _edit.Draft;
            Options.Visibility = value is null ? Visibility.Collapsed : Visibility.Visible;
            Options.IsEnabled = Available && !busy;
            ReadButton.IsEnabled = Available && !busy;
            ApplyButton.IsEnabled = Available && !busy && _edit.Dirty && value is not null;
            if (value is not null)
            {
                bool right = EarPicker.SelectedIndex == 1;
                EnableSwitch.IsOn = value.Enabled;
                Amplification.Value = 100 * (right ? value.RightAmplification : value.LeftAmplification);
                Tone.Value = 100 * (right ? value.RightTone : value.LeftTone);
                Ambient.Value = 100 * (right ? value.RightAmbientNoiseReduction : value.LeftAmbientNoiseReduction);
                BoostSwitch.IsOn = right ? value.RightConversationBoost : value.LeftConversationBoost;
            }
            string help = Localize.Get(!Available ? "DeviceSetting.Disconnected" : busy ? "DeviceSetting.Pending" :
                _edit.Dirty ? "DeviceSetting.Unsent" : "Transparency.Help");
            AutomationProperties.SetHelpText(TransparencyExpander, help);
            ToolTipService.SetToolTip(TransparencyExpander, help);
            ErrorText.Text = _snapshot.TransparencyError ?? "";
            ErrorText.Visibility = !busy && ErrorText.Text.Length > 0 ? Visibility.Visible : Visibility.Collapsed;
        }
        finally { _applying = false; }
    }

    private void Ear_Changed(object sender, SelectionChangedEventArgs e)
    {
        if (!_applying) Render();
    }

    private void Slider_Changed(object sender, RangeBaseValueChangedEventArgs e) => Edit();
    private void Toggle_Changed(object sender, RoutedEventArgs e) => Edit();

    private void Edit()
    {
        if (_applying || !Available || _edit.Requesting || _snapshot.TransparencyPending || _edit.Draft is not { } value) return;
        float amplification = (float)(Amplification.Value / 100);
        float tone = (float)(Tone.Value / 100);
        float ambient = (float)(Ambient.Value / 100);
        value = value with { Enabled = EnableSwitch.IsOn };
        _edit.Edit(EarPicker.SelectedIndex == 1
            ? value with { RightAmplification = amplification, RightTone = tone, RightAmbientNoiseReduction = ambient, RightConversationBoost = BoostSwitch.IsOn }
            : value with { LeftAmplification = amplification, LeftTone = tone, LeftAmbientNoiseReduction = ambient, LeftConversationBoost = BoostSwitch.IsOn });
        ApplyButton.IsEnabled = true;
    }

    private void Read_Click(object sender, RoutedEventArgs e)
    {
        if (ReadButton.IsEnabled) Send(new GetTransparencyCmd());
    }

    private void Apply_Click(object sender, RoutedEventArgs e)
    {
        if (ApplyButton.IsEnabled && _edit.Draft is { IsValid: true } value)
            Send(new SetTransparencyCmd { Settings = value });
    }

    private void Send(object command)
    {
        if (Client?.Send(command) != true) return;
        _edit.Begin(_snapshot, command is SetTransparencyCmd);
        _timeout.Start();
        Render();
    }
}
