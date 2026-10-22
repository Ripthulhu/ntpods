using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;
using NTPods.WinUI.Controls;
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
    private string? _caseInformationTarget;
    private string? _caseInformationPreviousDevice;
    private long _caseInformationDeadline;

    /// The daemon client, propagated to the header + every command-sending card.
    public DaemonClient? Client
    {
        get => _client;
        set
        {
            _client = value;
            Header.Client = value;
            DongleCard.Client = value;
            VolumeCard.Client = value;
            EqualizerCard.Client = value;
            DeviceSettingsCard.Client = value;
            CompanionCard.Client = value;
            NoiseControlCard.Client = value;
            FeaturesCard.Client = value;
            MicCard.Client = value;
            HeartRateCard.Client = value;
            HeadTrackingCard.Client = value;
            HearingAidCard.Client = value;
            TransparencyCard.Client = value;
            HardwareDiagnosticsCard.Client = value;
            NearbyFinderCard.Client = value;
            NearbyCaseCard.Client = value;
        }
    }

    public DevicePage()
    {
        InitializeComponent();
        CompanionCard.LocalBusy = () => DeviceSettingsCard.PlaybackPending;
        DeviceSettingsCard.PlaybackPendingChanged += CompanionCard.RefreshLocalBusy;
        Header.SettingsRequested += () => SettingsRequested?.Invoke();
        Header.PairingRequested += () => PairingRequested?.Invoke();
        _scrollbars = new AutoHideScrollbars(PageScrollViewer);
        _sections = new SectionTransitions(this, DeviceRoot, DeviceSections, VolumeSections, ExtraSections, HealthSections);
        // Hearing assistance remains an opt-in for supported models.
        ApplyExperimentalVisibility();
        Unloaded += async (_, _) => await StopNearbyCaseInformationAsync();
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
        var health = DongleHealthPresentation.Project(_snapshot);
        var vis = AppSettings.EnableExperimental && DeviceModels.Find(health.Model).HearingAid ? Visibility.Visible : Visibility.Collapsed;
        HearingAidCard.Visibility = vis;
        HardwareDiagnosticsCard.Visibility = DongleHealthPresentation.ShowDiagnostics(health, AppSettings.EnableExperimental)
            ? Visibility.Visible : Visibility.Collapsed;
        HealthSections.Visibility = vis == Visibility.Visible || HardwareDiagnosticsCard.Visibility == Visibility.Visible
            ? Visibility.Visible : Visibility.Collapsed;
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
        var health = DongleHealthPresentation.Project(s);
        string healthTarget = health.DeviceId;
        Func<object, bool>? route = DongleHealthPresentation.UsesDongle(s)
            ? command => DongleHealthPresentation.Route(_snapshot, healthTarget, command) is { } routed && _client?.Send(routed) == true
            : null;
        HearingAidCard.SendCommand = HardwareDiagnosticsCard.SendCommand = route;
        DeviceSettingsCard.SendCommand = FeaturesCard.SendCommand = NoiseControlCard.SendCommand = route;
        bool dongle = DongleHealthPresentation.UsesDongle(s);
        DongleCard.Update(s.Dongle);
        DeviceBody.Visibility = !dongle || s.Dongle.Selected ? Visibility.Visible : Visibility.Collapsed;
        PairFirstButton.Visibility = string.IsNullOrEmpty(s.DeviceId) ? Visibility.Visible : Visibility.Collapsed;
        PairFirstButton.Content = Localize.Get(s.Dongle.Present || s.Dongle.Ambiguous ? "Dongle_PairNative" : "Pairing_Title");
        NearbyFinderCard.Visibility = string.IsNullOrEmpty(DongleHealthPresentation.FinderProject(s).DeviceId) ? Visibility.Collapsed : Visibility.Visible;
        NearbyCaseCard.Visibility = !string.IsNullOrEmpty(s.DeviceId) && CaseInformationPresentation.CasePnpProduct(s.Model).HasValue
            ? Visibility.Visible : Visibility.Collapsed;
        NoiseControlCard.Visibility = DeviceModels.Listening(health) ? Visibility.Visible : Visibility.Collapsed;
        EqualizerCard.Visibility = DeviceModels.CustomEq(s) ? Visibility.Visible : Visibility.Collapsed;
        HeartRateCard.Visibility = profile.HeartRate || s.HeartRateEnabled ? Visibility.Visible : Visibility.Collapsed;
        HeadTrackingCard.Visibility = DeviceModels.HeadTracking(s) ? Visibility.Visible : Visibility.Collapsed;
        MicCard.Visibility = DeviceModels.HighQualityMic(s) || s.AutoMode || s.MicManualActive || s.MicCaptureActive
            ? Visibility.Visible : Visibility.Collapsed;
        ApplyExperimentalVisibility();
        Header.Update(s);
        BatteryCard.Update(s);
        CompanionCard.Update(s);
        VolumeCard.Update(s);
        EqualizerCard.Update(s);
        DeviceSettingsCard.Update(health);
        NoiseControlCard.Update(health);
        bool featuresChanged = FeaturesCard.Update(health);
        MicCard.Update(s);
        MultipointCard.Update(s);
        HeartRateCard.Update(s);
        HeadTrackingCard.Update(s);
        HearingAidCard.Update(health);
        TransparencyCard.Update(s);
        HardwareDiagnosticsCard.Update(health);
        NearbyFinderCard.Update(DongleHealthPresentation.FinderProject(s));
        NearbyCaseCard.Update(s);
        ReadCaseAfterPairing();

        if (dongle)
        {
            BatteryCard.Visibility = CompanionCard.Visibility = VolumeSections.Visibility = MicCard.Visibility = Visibility.Collapsed;
            HeartRateCard.Visibility = HeadTrackingCard.Visibility = MultipointCard.Visibility = NearbyCaseCard.Visibility = Visibility.Collapsed;
        }
        else
        {
            BatteryCard.Visibility = VolumeSections.Visibility = Visibility.Visible;
        }
        bool on = health.Connected;
        VolumeCard.IsEnabled = on;
        NoiseControlCard.IsEnabled = on;
        // The microphone permission is local; it must remain possible to turn
        // it off after the AirPods disconnect or the capture driver disappears.
        MicCard.IsEnabled = true;
        HeartRateCard.IsEnabled = _client?.IsReady == true && (profile.HeartRate || s.HeartRateEnabled);
        HearingAidCard.IsEnabled = health.Connected;
        _sections.AnimateChanges(changed || featuresChanged);
    }

    /// The events pipe dropped — surface it in the header.
    public void ShowWaitingForDaemon()
    {
        DongleCard.Disconnected();
        NearbyFinderCard.ResetSession();
        _ = StopNearbyCaseInformationAsync();
        Header.ShowWaitingForDaemon();
    }

    public void StopHeadTracking() => HeadTrackingCard.Stop();

    public void StopNearbyFinder()
    {
        NearbyFinderCard.Stop();
        _ = StopNearbyCaseInformationAsync();
    }

    public Task<bool> StopNearbyCaseInformationAsync()
    {
        _caseInformationTarget = null;
        return NearbyCaseCard.StopAsync();
    }

    internal void QueueNearbyCaseInformation(string deviceId, ushort? advertisedModel)
    {
        if (string.IsNullOrEmpty(deviceId) || advertisedModel.HasValue &&
            !CaseInformationPresentation.AdvertisedCasePnpProduct(advertisedModel).HasValue) return;
        // Wait for the daemon to select and identify the paired buds before reading a nearby case.
        _caseInformationTarget = deviceId;
        _caseInformationPreviousDevice = _snapshot.DeviceId;
        _caseInformationDeadline = Environment.TickCount64 + 35_000;
        ReadCaseAfterPairing();
    }

    private void ReadCaseAfterPairing()
    {
        if (_caseInformationTarget is not { } target) return;
        bool selectedAnotherPair = !string.IsNullOrEmpty(_snapshot.DeviceId) &&
            !string.Equals(target, _snapshot.DeviceId, StringComparison.OrdinalIgnoreCase) &&
            !string.Equals(_caseInformationPreviousDevice, _snapshot.DeviceId, StringComparison.OrdinalIgnoreCase);
        if (Environment.TickCount64 >= _caseInformationDeadline || selectedAnotherPair)
        {
            _caseInformationTarget = null;
            return;
        }
        if (!string.Equals(target, _snapshot.DeviceId, StringComparison.OrdinalIgnoreCase) ||
            !_snapshot.Connected || _snapshot.LinkStale || _client?.IsReady != true ||
            !NearbyCaseCard.CanRead) return;
        _caseInformationTarget = null;
        _ = NearbyCaseCard.ReadInformationAsync();
    }

    public void SetDaemonReady(bool ready)
    {
        if (!ready) NearbyFinderCard.Stop();
        if (!ready) _caseInformationTarget = null;
        NearbyCaseCard.SetDaemonReady(ready);
        DeviceBody.IsEnabled = ready;
        Header.SetDaemonReady(ready);
    }

    public void SetCompanionSession(ulong epoch, bool ready) => CompanionCard.SetCompanionSession(epoch, ready);
    public void UpdateCompanionStatus(CompanionStatus status, long receivedAtMs)
    {
        _sections.Capture();
        CompanionCard.UpdateCompanionStatus(status, receivedAtMs);
        _sections.AnimateChanges();
    }
    public void CompleteCompanion(DaemonEvent.CommandResult result, ulong epoch)
    {
        CompanionCard.CompleteCompanion(result, epoch);
        FeaturesCard.CompletePreference(result);
    }
    public void UpdateCompanionPreferences(CompanionPreferences preferences)
    {
        CompanionCard.UpdatePreferences(preferences);
        FeaturesCard.UpdatePreferences(preferences);
        BatteryCard.Update(_snapshot);
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
