using NTPods.WinUI.Ipc;
using NTPods.WinUI.Services;

namespace NTPods.WinUI.Controls;

internal static class DongleHealthPresentation
{
    public static bool Busy(DongleHealthState health) => health.HearingAidPending ||
        health.DeviceSettingsPending ||
        health.HardwareDiagnosticsPending || health.HearingMetadataPending ||
        HardwareDiagnosticsPresentation.IsRunning(health.HardwareCheck);

    public static bool ShowDiagnostics(Snapshot health, bool experimental) => experimental &&
        (DeviceModels.Find(health.Model).HearingAid || HardwareDiagnosticsPresentation.IsRunning(health.HardwareCheck));

    public static bool UsesDongle(Snapshot state) => !state.Connected &&
        (state.Dongle.Present || state.Dongle.Ambiguous);

    public static Snapshot Project(Snapshot state)
    {
        if (!UsesDongle(state)) return state;
        var dongle = state.Dongle;
        var health = dongle.Health;
        bool connected = dongle.Present && !dongle.Ambiguous && !dongle.Pending && dongle.Session != 0 &&
            dongle.Selected && dongle.Connected && dongle.ControlsReady && health.Available;
        return new Snapshot {
            Dongle = state.Dongle,
            DeviceId = $"dongle:{dongle.Session}:{dongle.Generation}",
            Connected = connected,
            LinkStale = !connected,
            Model = health.Model,
            Firmware = health.Firmware,
            EarbudsInCase = health.EarbudsInCase,
            DeviceControls = new(health.DeviceControls),
            DeviceControlsRequested = new(health.DeviceControlsRequested),
            DeviceControlsPending = new(health.DeviceControlsPending),
            DeviceSettingsError = health.DeviceSettingsError,
            Anc = (byte)health.DeviceControls.GetValueOrDefault((byte)0x0d),
            AncUnconfirmed = health.AncUnconfirmed,
            ConversationalAwareness = health.DeviceControls.GetValueOrDefault((byte)0x28) == 1,
            ConversationalAwarenessPending = health.DeviceControlsPending.TryGetValue(0x28, out uint ca) ? ca == 1 : null,
            AdaptiveVolume = health.DeviceControls.GetValueOrDefault((byte)0x26) == 1,
            AllowOff = health.DeviceControls.GetValueOrDefault((byte)0x34) == 1,
            AllowOffRequested = health.AllowOffRequested,
            AdaptiveStrengthRequested = health.AdaptiveStrengthRequested,
            AutoPause = state.AutoPause,
            ConversationVolume = state.ConversationVolume,
            PlaybackAvailable = (dongle.Capabilities & 16) != 0 && health.Audio is not null,
            PlaybackState = health.Audio?.Pending == true ? "starting" : connected ? "ready" : "disconnected",
            PlaybackMode = health.Audio?.Codec == 3 ? "aac_eld" : health.Audio?.Codec == 2 ? "aac" : "windows",
            PlaybackPreferredMode = health.Audio?.RequestedCodec == 3 ? "aac_eld" : health.Audio?.RequestedCodec == 2 ? "aac" : "windows",
            PlaybackBitrateKbps = health.Audio is { Bitrate: > 0 } audio ? audio.Bitrate / 1000 : null,
            PlaybackAacBitrate = new PlaybackBitrate { BestEffort = false, Kbps = health.Audio?.RequestedCodec == 2 ? health.Audio.RequestedBitrate / 1000 : 220 },
            PlaybackAacEldBitrate = new PlaybackBitrate { BestEffort = false, Kbps = health.Audio?.RequestedCodec == 3 ? health.Audio.RequestedBitrate / 1000 : 256, Variable = health.Audio?.Variable == true },
            PlaybackGameMode = health.Audio?.GameModeSupported == true ? health.Audio.GameModeRequested : null,
            PlaybackError = health.DeviceSettingsError,
            NearbyFinder = health.NearbyFinder,
            MicCaptureActive = dongle.MicrophoneActive,
            MicUplinkActive = dongle.MicrophoneActive,
            MicState = dongle.MicrophoneActive ? "capturing" : "idle",
            HearingAidEnabled = health.HearingAidEnabled,
            HearingAidRevision = health.HearingAidRevision,
            HearingAidEnrolled = health.HearingAidEnrolled,
            MediaAssistEnabled = health.MediaAssistEnabled,
            MediaAssistRevision = health.MediaAssistRevision,
            MediaAssistAvailable = health.MediaAssistAvailable,
            HearingAidSettings = health.HearingAidSettings,
            HearingAidEditable = health.HearingAidEditable,
            HearingAidPending = health.HearingAidPending,
            HearingAidError = health.HearingAidError,
            HardwareDiagnostics = health.HardwareDiagnostics,
            HardwareDiagnosticsPending = health.HardwareDiagnosticsPending,
            HardwareDiagnosticsError = health.HardwareDiagnosticsError,
            HearingMetadata = health.HearingMetadata,
            HearingMetadataPending = health.HearingMetadataPending,
            HearingMetadataError = health.HearingMetadataError,
            HardwareCheck = health.HardwareCheck,
        };
    }

    public static object? Route(Snapshot state, string target, object command)
    {
        var selected = Project(state);
        if (selected.DeviceId != target) return null;
        if (!UsesDongle(state)) return command;
        if (!state.Dongle.Present || state.Dongle.Ambiguous || state.Dongle.Session == 0 ||
            !state.Dongle.Selected || (!selected.Connected && command is not (CancelHardwareCheckCmd or GetDonglePerformanceCmd)) || command is not (
            GetHearingAidCmd or SetHearingAidModeCmd or SetMediaAssistCmd or SetHearingAidCmd or
            GetHardwareDiagnosticsCmd or GetHearingMetadataCmd or StartHardwareCheckCmd or CancelHardwareCheckCmd or
            SetDeviceControlCmd or SetAncCmd or SetFeatureCmd or SetControlCmd or RefreshDeviceSettingsCmd or
            SetPlaybackModeCmd or SetPlaybackBitrateCmd or SetPlaybackGameModeCmd or SetDongleAudioProfileCmd or GetDonglePerformanceCmd)) return null;
        return new DongleHealthCmd { Session = state.Dongle.Session, Generation = state.Dongle.Generation, Command = command };
    }

    public static Snapshot FinderProject(Snapshot state) => !UsesDongle(state) ? state : new Snapshot {
        DeviceId = state.Dongle.PairToken == 0 ? "" : $"dongle-finder:{state.Dongle.Session}:{state.Dongle.PairToken}",
        NearbyFinder = state.Dongle.Health.NearbyFinder,
    };
}
