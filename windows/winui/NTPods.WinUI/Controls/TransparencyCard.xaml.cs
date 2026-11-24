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
    public Func<object, bool>? SendCommand { get; set; }
    private Snapshot _snapshot = new();
    private readonly TransparencyEditState _edit = new();
    private bool _applying = true;
    private ulong _clientEpoch;
    private readonly DispatcherTimer _timeout = new() { Interval = TimeSpan.FromSeconds(15) };
    private readonly DispatcherTimer _live = new() { Interval = TimeSpan.FromMilliseconds(150) };

    public TransparencyCard()
    {
        InitializeComponent();
        _live.Tick += (_, _) => { _live.Stop(); Flush(); };
        _timeout.Tick += (_, _) => { _timeout.Stop(); _edit.Cancel(); Client?.RequestState(); Render(); };
        Unloaded += (_, _) => StopEditing();
        _applying = false;
        Render();
    }

    private bool Available => Client?.IsReady == true && _snapshot.Connected && !_snapshot.LinkStale &&
        DeviceModels.Find(_snapshot.Model).CustomTransparency;
    private bool OtherBusy => _snapshot.HeadphoneAccommodations?.Pending == true || _snapshot.HearingAidPending ||
        _snapshot.HardwareDiagnosticsPending || _snapshot.HearingMetadataPending || _snapshot.LoudSoundReductionPending ||
        _snapshot.MicCaptureActive || _snapshot.MicUplinkActive ||
        _snapshot.MicState is not ("idle" or "unavailable") ||
        HardwareDiagnosticsPresentation.IsRunning(_snapshot.HardwareCheck);

    public void Update(Snapshot s)
    {
        if (_clientEpoch != (Client?.SessionEpoch ?? 0))
        {
            _clientEpoch = Client?.SessionEpoch ?? 0;
            StopEditing();
            _edit.Update(new(), false);
        }
        _snapshot = s;
        _edit.Update(s, Available);
        if (!_edit.Sending) _timeout.Stop();
        Render();
        if (_edit.Dirty && !_edit.Sending) Schedule();
    }

    private void Render()
    {
        _applying = true;
        try
        {
            bool busy = _edit.Sending || _snapshot.TransparencyPending;
            bool editable = Available && !OtherBusy && (!_snapshot.TransparencyPending || _edit.Sending) && _snapshot.Transparency is not null && _edit.Baseline is not null;
            var value = _edit.Draft;
            Options.Visibility = value is null ? Visibility.Collapsed : Visibility.Visible;
            EnableSwitch.IsEnabled = BoostSwitch.IsEnabled = editable;
            Amplification.IsEnabled = Balance.IsEnabled = editable;
            Tone.IsEnabled = Ambient.IsEnabled = OwnVoice.IsEnabled = editable;
            if (value is not null)
            {
                EnableSwitch.IsOn = value.Enabled;
                Amplification.Maximum = Math.Max(100, 100 * TransparencyTuning.Amplification(value));
                Amplification.Value = 100 * TransparencyTuning.Amplification(value);
                Balance.Minimum = Math.Min(-100, 100 * TransparencyTuning.Balance(value));
                Balance.Maximum = Math.Max(100, 100 * TransparencyTuning.Balance(value));
                Balance.Value = 100 * TransparencyTuning.Balance(value);
                bool canonical = TransparencyTuning.CanEditCombined(value);
                Amplification.IsEnabled = Balance.IsEnabled = editable && canonical;
                string gainHelp = canonical ? "" : Localize.Get("Transparency.CustomGainRange");
                AutomationProperties.SetHelpText(Balance, gainHelp);
                AutomationProperties.SetHelpText(Amplification, gainHelp);
                ToolTipService.SetToolTip(Balance, gainHelp.Length > 0 ? gainHelp : null);
                ToolTipService.SetToolTip(Amplification, gainHelp.Length > 0 ? gainHelp : null);
                Tone.Value = 50 * (value.LeftTone + value.RightTone);
                Ambient.Value = 50 * (value.LeftAmbientNoiseReduction + value.RightAmbientNoiseReduction);
                OwnVoice.Value = 100 * value.OwnVoice;
                BoostSwitch.IsOn = value.LeftConversationBoost && value.RightConversationBoost;
            }
            string help = Localize.Get(!Available ? "DeviceSetting.Disconnected" : busy ? "DeviceSetting.Pending" :
                _edit.Dirty ? "DeviceSetting.Unsent" : "Transparency.Help");
            AutomationProperties.SetHelpText(this, help);
            ToolTipService.SetToolTip(this, help);
            ErrorText.Text = _snapshot.TransparencyError ?? "";
            ErrorText.Visibility = !busy && ErrorText.Text.Length > 0 ? Visibility.Visible : Visibility.Collapsed;
        }
        finally { _applying = false; }
    }

    private void Slider_Changed(object sender, RangeBaseValueChangedEventArgs e) => Edit(sender);
    private void Toggle_Changed(object sender, RoutedEventArgs e) => Edit(sender);

    private void Edit(object sender)
    {
        if (_applying || !Available || _snapshot.Transparency is null || _snapshot.TransparencyPending && !_edit.Sending || _edit.Draft is not { } value) return;
        if (ReferenceEquals(sender, EnableSwitch)) value = value with { Enabled = EnableSwitch.IsOn };
        else if (ReferenceEquals(sender, BoostSwitch)) value = value with
            { LeftConversationBoost = BoostSwitch.IsOn, RightConversationBoost = BoostSwitch.IsOn };
        else if (ReferenceEquals(sender, Amplification)) value = TransparencyTuning.WithAmplification(value, (float)(Amplification.Value / 100));
        else if (ReferenceEquals(sender, Balance)) value = TransparencyTuning.WithBalance(value, (float)(Balance.Value / 100));
        else if (ReferenceEquals(sender, Tone)) value = value with
            { LeftTone = (float)(Tone.Value / 100), RightTone = (float)(Tone.Value / 100) };
        else if (ReferenceEquals(sender, Ambient)) value = value with
            { LeftAmbientNoiseReduction = (float)(Ambient.Value / 100), RightAmbientNoiseReduction = (float)(Ambient.Value / 100) };
        else if (ReferenceEquals(sender, OwnVoice)) value = value with { OwnVoice = (float)(OwnVoice.Value / 100) };
        else return;
        _edit.Edit(value);
        Render();
        Schedule();
    }

    private void Schedule()
    {
        if (_edit.Dirty && !_live.IsEnabled) _live.Start();
    }

    private void Flush()
    {
        if (Available && _clientEpoch == Client?.SessionEpoch && !OtherBusy && !_edit.Sending && !_snapshot.TransparencyPending && _edit.Dirty && _edit.Draft is { IsValid: true } value)
            Send(new SetTransparencyCmd { Settings = value, Expected = _edit.Baseline });
    }

    private void Send(object command)
    {
        if ((SendCommand?.Invoke(command) ?? Client?.Send(command)) != true) { _edit.Cancel(); Render(); return; }
        _edit.Begin(_snapshot, command is SetTransparencyCmd);
        _timeout.Start();
        Render();
    }
    public void StopEditing() { _live.Stop(); _timeout.Stop(); _edit.Cancel(); }
}
