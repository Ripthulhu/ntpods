using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace NTPods.WinUI.Pages;

/// The device view owns no daemon logic beyond fanning a Snapshot out to each control and
/// rendering the connect prompt.
public sealed partial class DevicePage : UserControl
{
    public event Action? SettingsRequested;
    private DaemonClient? _client;
    private readonly AutoHideScrollbars _scrollbars;
    private Snapshot _snapshot = new();

    /// The daemon client, propagated to the header + every command-sending card.
    public DaemonClient? Client
    {
        get => _client;
        set
        {
            _client = value;
            Header.Client = value;
            VolumeCard.Client = value;
            EqualizerCard.Client = value;
            DeviceSettingsCard.Client = value;
            NoiseControlCard.Client = value;
            FeaturesCard.Client = value;
            MicCard.Client = value;
            HeartRateCard.Client = value;
            HeadTrackingCard.Client = value;
            HearingAidCard.Client = value;
        }
    }

    public DevicePage()
    {
        InitializeComponent();
        Header.SettingsRequested += () => SettingsRequested?.Invoke();
        _scrollbars = new AutoHideScrollbars(PageScrollViewer);
        // Hearing assistance remains an opt-in for supported models.
        ApplyExperimentalVisibility();

        ConnectPromptBar.Closed += (_, _) => ConnectPromptBar.Visibility = Visibility.Collapsed;
    }

    /// Re-read the experimental opt-in (call after the Settings toggle changes so the
    /// cards appear/disappear without an app restart).
    public void RefreshExperimentalVisibility() => ApplyExperimentalVisibility();

    /// Show/hide every experimental card from the single experimental opt-in.
    private void ApplyExperimentalVisibility()
    {
        var vis = AppSettings.EnableExperimental && DeviceModels.Find(_snapshot.Model).HearingAid ? Visibility.Visible : Visibility.Collapsed;
        HearingAidCard.Visibility = vis;
    }

    /// Fan a fresh Snapshot out to the header and every card that renders state.
    public void Update(Snapshot s)
    {
        var profile = DeviceModels.Find(s.Model);
        if (s.DeviceId != _snapshot.DeviceId || !DeviceModels.HeadTracking(s)) HeadTrackingCard.Stop();
        _snapshot = s;
        NoiseControlCard.Visibility = DeviceModels.Listening(s) ? Visibility.Visible : Visibility.Collapsed;
        EqualizerCard.Visibility = DeviceModels.CustomEq(s) ? Visibility.Visible : Visibility.Collapsed;
        HeartRateCard.Visibility = profile.HeartRate || s.HeartRateEnabled ? Visibility.Visible : Visibility.Collapsed;
        HeadTrackingCard.Visibility = DeviceModels.HeadTracking(s) ? Visibility.Visible : Visibility.Collapsed;
        MicCard.Visibility = DeviceModels.HighQualityMic(s) || s.AutoMode || s.MicManualActive || s.MicCaptureActive
            ? Visibility.Visible : Visibility.Collapsed;
        ApplyExperimentalVisibility();
        Header.Update(s);
        BatteryCard.Update(s);
        VolumeCard.Update(s);
        EqualizerCard.Update(s);
        DeviceSettingsCard.Update(s);
        NoiseControlCard.Update(s);
        FeaturesCard.Update(s);
        MicCard.Update(s);
        MultipointCard.Update(s);
        HeartRateCard.Update(s);
        HeadTrackingCard.Update(s);
        HearingAidCard.Update(s);

        // Disable every interactive control while there's no proper connection — a
        // disconnected/desynced link can't apply commands, so the controls shouldn't
        // look actionable (Battery stays as an informational read-out). Use "Reparar
        // ligação" in the header to force a clean reconnect.
        bool on = s.Connected;
        VolumeCard.IsEnabled = on;
        NoiseControlCard.IsEnabled = on;
        // The microphone permission is local; it must remain possible to turn
        // it off after the AirPods disconnect or the capture driver disappears.
        MicCard.IsEnabled = true;
        HeartRateCard.IsEnabled = _client?.IsReady == true && (profile.HeartRate || s.HeartRateEnabled);
        HearingAidCard.IsEnabled = on;
    }

    /// Show the "nearby — connect?" prompt with a Connect action.
    public void ShowConnectPrompt(string name)
    {
        ConnectPromptBar.Title = Localize.Get("ConnectPrompt_Title", name);
        ConnectPromptBar.Message = Localize.Get("ConnectPrompt_Message");
        var btn = new Button { Content = Localize.Get("Action_Connect") };
        btn.Click += (_, _) =>
        {
            _client?.Connect();
            DismissConnectPrompt();
        };
        ConnectPromptBar.ActionButton = btn;
        ConnectPromptBar.Visibility = Visibility.Visible;
        ConnectPromptBar.IsOpen = true;
    }

    /// Hide the "nearby — connect?" prompt (e.g. once the device is connected).
    public void DismissConnectPrompt()
    {
        ConnectPromptBar.IsOpen = false;
        ConnectPromptBar.Visibility = Visibility.Collapsed;
    }

    /// The events pipe dropped — surface it in the header.
    public void ShowWaitingForDaemon() => Header.ShowWaitingForDaemon();

    public void StopHeadTracking() => HeadTrackingCard.Stop();

    public void SetDaemonReady(bool ready)
    {
        DeviceBody.IsEnabled = ready;
        Header.SetDaemonReady(ready);
    }

    public void FocusActions() => Header.FocusActions();

}
