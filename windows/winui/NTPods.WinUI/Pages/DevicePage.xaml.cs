using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace NTPods.WinUI.Pages;

/// Fans each snapshot out to the device controls.
public sealed partial class DevicePage : UserControl
{
    public event Action? SettingsRequested;
    public event Action? PairingRequested;
    private DaemonClient? _client;
    private readonly AutoHideScrollbars _scrollbars;
    private readonly DeviceViewIdentity _identity = new();
    private readonly SectionTransitions _sections;
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
            TransparencyCard.Client = value;
            HardwareDiagnosticsCard.Client = value;
        }
    }

    public DevicePage()
    {
        InitializeComponent();
        Header.SettingsRequested += () => SettingsRequested?.Invoke();
        Header.PairingRequested += () => PairingRequested?.Invoke();
        _scrollbars = new AutoHideScrollbars(PageScrollViewer);
        _sections = new SectionTransitions(this, DeviceRoot, DeviceSections, VolumeSections, ExtraSections);
        // Hearing assistance remains an opt-in for supported models.
        ApplyExperimentalVisibility();
    }

    /// Apply the experimental preference and animate any resulting layout change.
    public void RefreshExperimentalVisibility()
    {
        _sections.Capture();
        ApplyExperimentalVisibility();
        _sections.AnimateChanges();
    }

    internal void CancelShortcutCapture() => MicCard.CancelShortcutCapture();

    /// Show/hide every experimental card from the single experimental opt-in.
    private void ApplyExperimentalVisibility()
    {
        var vis = AppSettings.EnableExperimental && DeviceModels.Find(_snapshot.Model).HearingAid ? Visibility.Visible : Visibility.Collapsed;
        HearingAidCard.Visibility = vis;
        HardwareDiagnosticsCard.Visibility = vis;
        TransparencyCard.Visibility = AppSettings.EnableExperimental && DeviceModels.Find(_snapshot.Model).CustomTransparency
            ? Visibility.Visible : Visibility.Collapsed;
    }

    /// Fan a fresh Snapshot out to the header and every card that renders state.
    public void Update(Snapshot s)
    {
        _sections.Capture();
        bool changed = _identity.Update(s);
        var profile = DeviceModels.Find(s.Model);
        if (s.DeviceId != _snapshot.DeviceId || !DeviceModels.HeadTracking(s))
            HeadTrackingCard.Stop(notifyDevice: s.DeviceId == _snapshot.DeviceId);
        _snapshot = s;
        PairFirstButton.Visibility = string.IsNullOrEmpty(s.DeviceId) ? Visibility.Visible : Visibility.Collapsed;
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
        bool featuresChanged = FeaturesCard.Update(s);
        MicCard.Update(s);
        MultipointCard.Update(s);
        HeartRateCard.Update(s);
        HeadTrackingCard.Update(s);
        HearingAidCard.Update(s);
        TransparencyCard.Update(s);
        HardwareDiagnosticsCard.Update(s);

        bool on = s.Connected;
        VolumeCard.IsEnabled = on;
        NoiseControlCard.IsEnabled = on;
        // The microphone permission is local; it must remain possible to turn
        // it off after the AirPods disconnect or the capture driver disappears.
        MicCard.IsEnabled = true;
        HeartRateCard.IsEnabled = _client?.IsReady == true && (profile.HeartRate || s.HeartRateEnabled);
        HearingAidCard.IsEnabled = on;
        _sections.AnimateChanges(changed || featuresChanged);
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

    public void ShowPairingError(string key = "Pairing.OpenFailed")
    {
        _sections.Capture();
        PairingError.Message = Localize.Get(key);
        PairingError.Visibility = Visibility.Visible;
        PairingError.IsOpen = true;
        _sections.AnimateChanges();
    }

    private void Pair_Click(object sender, RoutedEventArgs args)
    {
        PairingError.IsOpen = false;
        PairingRequested?.Invoke();
    }

    private void PairingError_Closed(InfoBar sender, InfoBarClosedEventArgs args)
    {
        _sections.Capture();
        PairingError.Visibility = Visibility.Collapsed;
        _sections.AnimateChanges();
    }

}
