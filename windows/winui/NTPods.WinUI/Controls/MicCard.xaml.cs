using System;
using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Automation;
using Microsoft.UI.Xaml.Controls;

namespace NTPods.WinUI.Controls;

/// Microphone permission, stream policy and confirmed capture state.
public sealed partial class MicCard : UserControl
{
    public DaemonClient? Client { get; set; }
    private bool _applying;
    private Snapshot _snapshot = new();
    private readonly MicPolicyState _policy = new();
    private readonly DispatcherTimer _debounce = new() { Interval = TimeSpan.FromMilliseconds(350) };
    private readonly DispatcherTimer _timeout = new() { Interval = TimeSpan.FromSeconds(5) };

    public MicCard()
    {
        InitializeComponent();
        _debounce.Tick += (_, _) => SendPolicy();
        _timeout.Tick += (_, _) => { _timeout.Stop(); _policy.Cancel(); Render(); Client?.RequestState(); };
        Unloaded += (_, _) => { if (_debounce.IsEnabled) SendPolicy(); };
    }

    public void Update(Snapshot s)
    {
        _snapshot = s;
        _policy.Update(s);
        if (Client?.IsReady != true) { _debounce.Stop(); _timeout.Stop(); _policy.Cancel(); }
        if (_policy.PendingManual is null && _policy.PendingPolicy is null) _timeout.Stop();
        Render();
    }

    private void Render()
    {
        var s = _snapshot;
        _applying = true;
        try
        {
            MicAutoSwitch.IsOn = s.AutoMode;
            bool supported = DeviceModels.HighQualityMic(s);
            MicAutoSwitch.IsEnabled = supported && s.MicAvailable || s.AutoMode;
            string key = s.MicState switch
            {
                "starting" => "Mic_Starting",
                "capturing" when s.MicRecording => "Mic_Recording",
                "capturing" when s.MicUplinkActive => "Mic_Active",
                "stopping" => "Mic_Stopping",
                "recovering" => "Mic_Recovering",
                "waiting_for_ear" => "Mic_WaitingEar",
                "error" => "Mic_Error",
                _ when !supported => "Mic.UnsupportedModel",
                _ when !s.MicAvailable => "Mic_Unavailable",
                _ when s.MicCaptureActive && !s.AutoMode => "Mic_DisabledCapture",
                _ when s.MicCaptureActive && !s.Connected => "Mic_WaitingConnection",
                _ => "Mic_Idle"
            };
            MicStatusText.Text = Localize.Get(key);
            if (!string.IsNullOrWhiteSpace(s.MicError)) MicStatusText.Text += " " + s.MicError;
            MicStatusText.Visibility = key is "Mic_Idle" or "Mic.UnsupportedModel" && string.IsNullOrWhiteSpace(s.MicError)
                ? Visibility.Collapsed : Visibility.Visible;
            AutomationProperties.SetHelpText(MicAutoSwitch, MicStatusText.Text);
            ToolTipService.SetToolTip(MicAutoSwitch, supported ? null : Localize.Get("Mic.UnsupportedModel"));
            string mode = _policy.Mode;
            ModePicker.SelectedIndex = mode == "keep_active" ? 1 : mode == "manual" ? 2 : 0;
            ModePicker.IsEnabled = Client?.IsReady == true && supported;
            StopAfter.IsEnabled = ModePicker.IsEnabled;
            StopAfter.Value = _policy.Delay;
            DelayOptions.Visibility = mode == "automatic" ? Visibility.Visible : Visibility.Collapsed;
            ManualButton.Visibility = mode == "manual" ? Visibility.Visible : Visibility.Collapsed;
            ManualButton.Content = Localize.Get(s.MicManualActive ? "Mic_StopManual" : "Mic_StartManual");
            ManualButton.IsEnabled = Client?.IsReady == true && _policy.PendingPolicy is null && _policy.PendingManual is null &&
                (MicPolicyState.CanStart(s) || MicPolicyState.CanStop(s));
            ModeHelp.Text = Localize.Get(mode == "keep_active" ? "Mic_KeepActiveHelp" : mode == "manual" ? "Mic_ManualHelp" : "Mic_AutomaticHelp");
        }
        finally { _applying = false; }
    }

    private void MicAuto_Toggled(object sender, RoutedEventArgs e)
    {
        if (!_applying && (!MicAutoSwitch.IsOn || DeviceModels.HighQualityMic(_snapshot)))
            Client?.SetMicMode(auto: MicAutoSwitch.IsOn, manual: false);
    }

    private void Mode_Changed(object sender, SelectionChangedEventArgs e)
    {
        if (_applying || !ModePicker.IsEnabled || ModePicker.SelectedItem is not ComboBoxItem { Tag: string mode }) return;
        if (_policy.QueuePolicy(mode, _policy.Delay)) SendPolicy();
    }

    private void Delay_Changed(NumberBox sender, NumberBoxValueChangedEventArgs args)
    {
        if (_applying || !StopAfter.IsEnabled || !double.IsFinite(args.NewValue) || args.NewValue < 0 || args.NewValue > 3600) return;
        if (!_policy.QueuePolicy(_policy.Mode, (uint)Math.Round(args.NewValue))) return;
        _timeout.Stop();
        _debounce.Stop();
        _debounce.Start();
    }

    private void SendPolicy()
    {
        _debounce.Stop();
        if (_policy.PendingPolicy is not { } desired) return;
        if (Client?.Send(new SetMicPolicyCmd { Mode = desired.Mode, StopDelaySecs = desired.Delay }) == true)
        {
            _timeout.Stop();
            _timeout.Start();
        }
        else _policy.Cancel();
        Render();
    }

    private void Manual_Click(object sender, RoutedEventArgs e)
    {
        if (!ManualButton.IsEnabled) return;
        bool active = !_snapshot.MicManualActive;
        if (Client?.Send(new SetMicManualActiveCmd { Active = active }) != true) return;
        _policy.RequestManual(active);
        _timeout.Stop();
        _timeout.Start();
        Render();
    }
}
