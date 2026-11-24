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
    public event Action? PhoneConnectionRequested;
    private DaemonClient? _client;
    private readonly AutoHideScrollbars _scrollbars;
    private readonly DeviceViewIdentity _identity = new();
    private readonly SectionTransitions _sections;
    private Snapshot _snapshot = new();
    private string? _caseInformationTarget;
    private string? _caseInformationPreviousDevice;
    private long _caseInformationDeadline;
    private string _page = "main";
    private bool _daemonReady;
    private Button? _lastMainNavigation;
    private readonly Dictionary<string, (StackPanel Panel, string Title)> _pages;

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
            HearingAidCard.Client = value;
            TransparencyCard.Client = value;
            AccommodationsCard.Client = value;
            HardwareDiagnosticsCard.Client = value;
            NearbyFinderCard.Client = value;
            NearbyCaseCard.Client = value;
        }
    }

    public DevicePage()
    {
        InitializeComponent();
        _pages = new()
        {
            ["recorder"] = (RecorderPage, "MicTitle.Text"), ["heart"] = (HeartRatePage, "HeartRateTitle.Text"),
            ["left"] = (HoldPage, "Apple.Left"), ["right"] = (HoldPage, "Apple.Right"),
            ["calls"] = (CallPage, "Apple.CallControls"), ["microphone"] = (MicrophonePage, "Apple.MicrophoneMode"),
            ["equalizer"] = (EqualizerPage, "Apple.Equalizer"), ["hearing"] = (HearingPage, "Apple.HearingAid"),
            ["protection"] = (ProtectionPage, "Apple.HearingProtection"), ["accessibility"] = (AccessibilityPage, "Apple.Accessibility"),
            ["accommodations"] = (AccommodationsPage, "Accommodations.Title"), ["transparency"] = (TransparencyPage, "Apple.Transparency"),
            ["playback"] = (PlaybackPage, "DeviceControls.Playback"), ["headtracking"] = (HeadTrackingPage, "Spatial_Title"),
            ["finder"] = (FinderPage, "NearbyFinder.Title"), ["case"] = (CasePage, "NearbyCase.Title"),
            ["multipoint"] = (MultipointPage, "MultipointTitle.Text"), ["hardware"] = (HardwarePage, "HardwareChecks.Title"),
            ["about"] = (AboutPage, "SettingsAbout.Text")
        };
        DeviceSettingsCard.MoveRows(EarDetectionRows, 0x0A);
        DeviceSettingsCard.MoveRows(SleepRows, 0x35);
        DeviceSettingsCard.MoveRows(OptimizedRows, 0x3B);
        DeviceSettingsCard.MoveRows(MicrophoneRows, 0x01);
        DeviceSettingsCard.MoveRows(HoldRows, 0x16, 0x1A);
        DeviceSettingsCard.MoveRows(CallRows, 0x24);
        DeviceSettingsCard.MoveRows(PressSpeedRows, 0x17);
        DeviceSettingsCard.MoveRows(HoldDurationRows, 0x18);
        DeviceSettingsCard.MoveRows(OneBudRows, 0x1B);
        DeviceSettingsCard.MoveRows(ToneVolumeRows, 0x1F);
        DeviceSettingsCard.MoveRows(VolumeControlRows, 0x25);
        DeviceSettingsCard.MoveRows(SwipeSpeedRows, 0x23);
        ProtectionHost.Content = DeviceSettingsCard.TakeHearingProtection();
        OffListeningHost.Content = FeaturesCard.TakeOffListeningMode();
        AccommodationsCard.TransparencyRequested += () => ShowPage("transparency", true);
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
        HearingAidButton.Visibility = vis;
        HardwareButton.Visibility = HardwareDiagnosticsCard.Visibility;
        HearingProtectionButton.Visibility = DongleHealthPresentation.ShowHearingProtection(_snapshot) ? Visibility.Visible : Visibility.Collapsed;
        HealthSections.Visibility = vis == Visibility.Visible || HearingProtectionButton.Visibility == Visibility.Visible
            ? Visibility.Visible : Visibility.Collapsed;
        HealthDivider.Visibility = vis == Visibility.Visible && HearingProtectionButton.Visibility == Visibility.Visible
            ? Visibility.Visible : Visibility.Collapsed;
        TransparencyCard.Visibility = AppSettings.EnableExperimental && DeviceModels.Find(health.Model).CustomTransparency
            ? Visibility.Visible : Visibility.Collapsed;
        AccommodationsButton.Visibility = AppSettings.EnableExperimental && DeviceModels.Find(health.Model).CustomTransparency
            ? Visibility.Visible : Visibility.Collapsed;
    }

    /// Fan a fresh Snapshot out to the header and every card that renders state.
    public void Update(Snapshot s)
    {
        _sections.Capture();
        bool changed = _identity.Update(s);
        if (s.DeviceId != _snapshot.DeviceId) { ShowPage("main"); AboutSerial.Visibility = Visibility.Collapsed; }
        var profile = DeviceModels.Find(s.Model);
        _snapshot = s;
        var health = DongleHealthPresentation.Project(s);
        string healthTarget = health.DeviceId;
        Func<object, bool>? route = DongleHealthPresentation.UsesDongle(s)
            ? command => DongleHealthPresentation.Route(_snapshot, healthTarget, command) is { } routed && _client?.Send(routed) == true
            : null;
        HearingAidCard.SendCommand = HardwareDiagnosticsCard.SendCommand = route;
        AccommodationsCard.SendCommand = route;
        TransparencyCard.SendCommand = route;
        DeviceSettingsCard.SendCommand = FeaturesCard.SendCommand = NoiseControlCard.SendCommand = route;
        bool dongle = DongleHealthPresentation.UsesDongle(s);
        DeviceSettingsCard.PlaceNoiseCycle(HoldRows, DeviceModels.Headphone(health));
        DongleCard.Update(s.Dongle);
        DeviceBody.Visibility = s.Dongle.Selected ? Visibility.Visible : Visibility.Collapsed;
        NearbyFinderCard.Visibility = string.IsNullOrEmpty(DongleHealthPresentation.FinderProject(s).DeviceId) ? Visibility.Collapsed : Visibility.Visible;
        NearbyCaseCard.Visibility = !string.IsNullOrEmpty(s.DeviceId) && CaseInformationPresentation.CasePnpProduct(s.Model).HasValue
            ? Visibility.Visible : Visibility.Collapsed;
        NoiseControlCard.Visibility = DeviceModels.Listening(health) ? Visibility.Visible : Visibility.Collapsed;
        EqualizerCard.Visibility = DeviceModels.CustomEq(s) ? Visibility.Visible : Visibility.Collapsed;
        HeartRateCard.Visibility = profile.HeartRate || s.HeartRateEnabled ? Visibility.Visible : Visibility.Collapsed;
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
        HeadTrackingCard.Update(s.Dongle);
        RefreshSpatialStop();
        HearingAidCard.Update(health);
        TransparencyCard.Update(health);
        AccommodationsCard.Update(health, dongle ? s.Dongle.PairedDeviceId : s.DeviceId);
        HardwareDiagnosticsCard.Update(health);
        NearbyFinderCard.Update(DongleHealthPresentation.FinderProject(s));
        NearbyCaseCard.Update(s);
        ReadCaseAfterPairing();

        if (dongle)
        {
            BatteryCard.Visibility = Visibility.Visible;
            VolumeSections.Visibility = MicCard.Visibility = Visibility.Collapsed;
            HeartRateCard.Visibility = MultipointCard.Visibility = NearbyCaseCard.Visibility = Visibility.Collapsed;
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
        MicrophoneControlsButton.Visibility = MicCard.Visibility;
        HeartRateButton.Visibility = HeartRateCard.Visibility;
        HeadTrackingButton.Visibility = Controls.HeadTrackingCard.Supports(s.Dongle) || HeadTrackingCard.WaitingForRoute
            ? Visibility.Visible : Visibility.Collapsed;
        EqualizerButton.Visibility = EqualizerCard.Visibility;
        FinderButton.Visibility = NearbyFinderCard.Visibility;
        CaseButton.Visibility = NearbyCaseCard.Visibility;
        MultipointButton.Visibility = MultipointCard.Visibility;
        PressHoldGroup.Visibility = DeviceModels.SupportsControl(health, 0x16) ? Visibility.Visible : Visibility.Collapsed;
        CallControlsButton.Visibility = DeviceModels.SupportsControl(health, 0x24) ? Visibility.Visible : Visibility.Collapsed;
        MicrophoneButton.Visibility = DeviceModels.SupportsControl(health, 0x01) ? Visibility.Visible : Visibility.Collapsed;
        EarDetectionGroup.Visibility = DeviceModels.SupportsControl(health, 0x0A) ? Visibility.Visible : Visibility.Collapsed;
        SleepGroup.Visibility = DeviceModels.SupportsControl(health, 0x35) ? Visibility.Visible : Visibility.Collapsed;
        OptimizedGroup.Visibility = DeviceModels.SupportsControl(health, 0x3B) ? Visibility.Visible : Visibility.Collapsed;
        OffListeningGroup.Visibility = DeviceModels.SupportsControl(health, 0x34) ? Visibility.Visible : Visibility.Collapsed;
        AccessibilityButton.Visibility = new byte[] { 0x17, 0x18, 0x1B, 0x1F, 0x25, 0x23 }.Any(id => DeviceModels.SupportsControl(health, id)) ||
            AccommodationsButton.Visibility == Visibility.Visible ? Visibility.Visible : Visibility.Collapsed;
        foreach (var (group, id) in new[] { (PressSpeedGroup, (byte)0x17), (HoldDurationGroup, (byte)0x18),
            (OneBudGroup, (byte)0x1B), (ToneVolumeGroup, (byte)0x1F), (VolumeControlGroup, (byte)0x25), (SwipeSpeedGroup, (byte)0x23) })
            group.Visibility = DeviceModels.SupportsControl(health, id) ? Visibility.Visible : Visibility.Collapsed;
        var presentation = SelectedDevicePresentation.Project(s, (ulong)DateTimeOffset.UtcNow.ToUnixTimeMilliseconds());
        NameButton.Visibility = dongle ? Visibility.Collapsed : Visibility.Visible;
        NameButton.IsEnabled = !dongle && s.Connected && !s.LinkStale;
        NameValue.Text = string.IsNullOrWhiteSpace(presentation.DevName) ? DeviceModels.DisplayName(presentation, Localize.Get("Device_DefaultName")) : presentation.DevName;
        var hold = DeviceSettingStatus.DisplayValue(health, 0x16);
        LeftHoldValue.Text = HoldActionText(hold is { } actions ? (actions >> 8) & 0xFF : null);
        RightHoldValue.Text = HoldActionText(hold is { } rightActions ? rightActions & 0xFF : null);
        AboutModel.Text = DeviceModels.Find(health.Model) is { Known: true } model ? model.Name : health.Model;
        AboutFirmware.Text = string.IsNullOrEmpty(health.Firmware) ? "—" : health.Firmware;
        AboutSerial.Text = string.IsNullOrEmpty(health.Serial) ? "—" : health.Serial;
        if (_pages.TryGetValue(_page, out var current)) SubpageTitle.Text = Localize.Get(current.Title);
        _sections.AnimateChanges(changed || featuresChanged);
    }

    private void Navigate_Click(object sender, RoutedEventArgs e)
    {
        if (sender is not Button { Tag: string route }) return;
        if (route == "rename") { Header.BeginRename(); return; }
        if (_page == "main") _lastMainNavigation = (Button)sender;
        ShowPage(route, true);
    }

    internal void SetRoomWindowVisible(bool visible) => HeadTrackingCard.SetRoomWindowVisible(visible);
    internal void StopHeadTracking() => HeadTrackingCard.Stop();

    private void OfflineSpatialStop_Click(object sender, RoutedEventArgs args)
    {
        HeadTrackingCard.Stop();
        RefreshSpatialStop();
    }

    private void RefreshSpatialStop()
    {
        bool waiting = HeadTrackingCard.WaitingForRoute;
        OfflineSpatialStop.Visibility = waiting && (!_daemonReady || DeviceBody.Visibility != Visibility.Visible)
            ? Visibility.Visible : Visibility.Collapsed;
        DetailSections.IsEnabled = _daemonReady || waiting && _page == "headtracking";
    }

    private void ShowPage(string route, bool focus = false)
    {
        if (_page == "finder" && route != "finder") NearbyFinderCard.Stop();
        _page = route;
        bool main = route == "main";
        Header.Visibility = main ? Visibility.Visible : Visibility.Collapsed;
        MainSections.Visibility = main ? Visibility.Visible : Visibility.Collapsed;
        SubpageNavigation.Visibility = DetailSections.Visibility = main ? Visibility.Collapsed : Visibility.Visible;
        foreach (var panel in _pages.Values.Select(value => value.Panel).Distinct()) panel.Visibility = Visibility.Collapsed;
        if (_pages.TryGetValue(route, out var page))
        {
            page.Panel.Visibility = Visibility.Visible;
            SubpageTitle.Text = Localize.Get(page.Title);
        }
        if (route is "left" or "right") DeviceSettingsCard.ShowHoldSide(route == "right");
        HeadTrackingCard.ShowRoom(route == "headtracking");
        RefreshSpatialStop();
        if (route == "protection") DeviceSettingsCard.ReadHearingProtection();
        PageScrollViewer.ChangeView(null, 0, null, true);
        if (focus)
        {
            if (main) _lastMainNavigation?.Focus(FocusState.Programmatic);
            else SubpageBackButton.Focus(FocusState.Programmatic);
        }
    }

    private void SubpageBack_Click(object sender, RoutedEventArgs e) => ShowPage(_page switch
    {
        "transparency" => "accommodations", "accommodations" => "accessibility", _ => "main"
    }, true);

    private static string HoldActionText(uint? action) => action switch
    {
        1 => Localize.Get("DeviceControls.Assistant"), 5 => Localize.Get("DeviceControls.NoiseControl"), _ => "—"
    };

    private void RevealSerial_Click(object sender, RoutedEventArgs e) =>
        AboutSerial.Visibility = AboutSerial.Visibility == Visibility.Visible ? Visibility.Collapsed : Visibility.Visible;

    /// The events pipe dropped — surface it in the header.
    public void ShowWaitingForDaemon()
    {
        DongleCard.Disconnected();
        HeadTrackingCard.Update(new DongleState());
        RefreshSpatialStop();
        NearbyFinderCard.ResetSession();
        _ = StopNearbyCaseInformationAsync();
        Header.ShowWaitingForDaemon();
    }


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
        _daemonReady = ready;
        if (!ready) NearbyFinderCard.Stop();
        if (!ready) _caseInformationTarget = null;
        NearbyCaseCard.SetDaemonReady(ready);
        DeviceBody.IsEnabled = ready;
        RefreshSpatialStop();
        Header.SetDaemonReady(ready);
    }

    public void SetCompanionSession(ulong epoch, bool ready) => CompanionCard.SetCompanionSession(epoch, ready);
    private void PhoneConnection_Click(object sender, RoutedEventArgs e) => PhoneConnectionRequested?.Invoke();
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
