using System.Text.Json;
using System.Text.Json.Serialization;

namespace NTPods.WinUI.Ipc;

// Keep wire names and event/cmd tags in sync with windows/ipc/src/lib.rs.

/// Battery levels (percent), each optional — a packet may carry only some.
public sealed class Battery
{
    [JsonPropertyName("left")] public byte? Left { get; set; }
    [JsonPropertyName("right")] public byte? Right { get; set; }
    [JsonPropertyName("case")] public byte? Case { get; set; }
    [JsonPropertyName("headphone")] public byte? Headphone { get; set; }

    [JsonPropertyName("left_charging")] public bool LeftCharging { get; set; }
    [JsonPropertyName("right_charging")] public bool RightCharging { get; set; }
    [JsonPropertyName("case_charging")] public bool CaseCharging { get; set; }
    [JsonPropertyName("headphone_charging")] public bool HeadphoneCharging { get; set; }
}

public sealed class BatteryObservation
{
    [JsonPropertyName("timestamp_ms")] public ulong? TimestampMs { get; set; }
    [JsonRequired, JsonPropertyName("live")] public bool Live { get; set; }
    // Explicit absence must survive retaining a cached percentage; age alone does not imply absence.
    [JsonPropertyName("present")] public bool? Present { get; set; }
}

public sealed class BatteryFreshness
{
    [JsonRequired, JsonPropertyName("left")] public BatteryObservation Left { get; set; } = new();
    [JsonRequired, JsonPropertyName("right")] public BatteryObservation Right { get; set; } = new();
    [JsonRequired, JsonPropertyName("case")] public BatteryObservation Case { get; set; } = new();
    [JsonRequired, JsonPropertyName("headphone")] public BatteryObservation Headphone { get; set; } = new();
    [JsonIgnore] public bool IsValid => Left is not null && Right is not null && Case is not null && Headphone is not null;
}

public sealed record PlaybackBitrate
{
    [JsonRequired, JsonPropertyName("best_effort")] public bool BestEffort { get; init; } = true;
    [JsonRequired, JsonPropertyName("kbps")] public uint Kbps { get; init; } = 256;
    [JsonPropertyName("variable")] public bool Variable { get; init; }
    public bool IsValidFor(string mode) => mode switch
    {
        "aac" => !Variable && Kbps is >= 96 and <= 512,
        "aac_eld" => Kbps is >= 128 and <= 256 && (!Variable || BestEffort && Kbps == 256),
        _ => false
    };
}

public sealed record PlaybackLinkSettings
{
    [JsonRequired, JsonPropertyName("mode")] public string Mode { get; init; } = "off";
    [JsonRequired, JsonPropertyName("kbps")] public ushort Kbps { get; init; } = 174;
    [JsonIgnore] public bool IsValid => Mode is "off" or "automatic" or "manual" && Kbps is >= 128 and <= 256;

    public static PlaybackLinkSettings? FromInput(string mode, double kbps) =>
        double.IsFinite(kbps) && kbps == Math.Truncate(kbps) && kbps is >= 128 and <= 256 &&
        mode is "off" or "automatic" or "manual" ? new() { Mode = mode, Kbps = (ushort)kbps } : null;
}

public sealed record HearingAidSettings
{
    [JsonRequired, JsonPropertyName("left_eq")] public float[] LeftEq { get; init; } = [];
    [JsonRequired, JsonPropertyName("right_eq")] public float[] RightEq { get; init; } = [];
    [JsonRequired, JsonPropertyName("left_amplification")] public float LeftAmplification { get; init; }
    [JsonRequired, JsonPropertyName("right_amplification")] public float RightAmplification { get; init; }
    [JsonRequired, JsonPropertyName("left_tone")] public float LeftTone { get; init; }
    [JsonRequired, JsonPropertyName("right_tone")] public float RightTone { get; init; }
    [JsonRequired, JsonPropertyName("left_conversation_boost")] public bool LeftConversationBoost { get; init; }
    [JsonRequired, JsonPropertyName("right_conversation_boost")] public bool RightConversationBoost { get; init; }
    [JsonRequired, JsonPropertyName("left_ambient_noise_reduction")] public float LeftAmbientNoiseReduction { get; init; }
    [JsonRequired, JsonPropertyName("right_ambient_noise_reduction")] public float RightAmbientNoiseReduction { get; init; }
    [JsonRequired, JsonPropertyName("own_voice")] public float OwnVoice { get; init; }
    [JsonIgnore] public bool IsValid => ValidCurve(LeftEq) && ValidCurve(RightEq) &&
        InRange(LeftAmplification, -1.5f, 1.5f) && InRange(RightAmplification, -1.5f, 1.5f) &&
        InRange(LeftTone, -1, 1) && InRange(RightTone, -1, 1) &&
        InRange(LeftAmbientNoiseReduction, 0, 1) && InRange(RightAmbientNoiseReduction, 0, 1) &&
        InRange(OwnVoice, 0, 1);

    private static bool InRange(float value, float min, float max) => float.IsFinite(value) && value >= min && value <= max;
    private static bool ValidCurve(float[]? curve) => curve is { Length: 8 } && curve.All(value => InRange(value, 0, 90));

    public bool SameValues(HearingAidSettings? other) => other is not null &&
        LeftEq.SequenceEqual(other.LeftEq) && RightEq.SequenceEqual(other.RightEq) &&
        this with { LeftEq = other.LeftEq, RightEq = other.RightEq } == other;
}

public sealed record TransparencySettings
{
    [JsonRequired, JsonPropertyName("enabled")] public bool Enabled { get; init; }
    [JsonRequired, JsonPropertyName("left_amplification")] public float LeftAmplification { get; init; }
    [JsonRequired, JsonPropertyName("right_amplification")] public float RightAmplification { get; init; }
    [JsonRequired, JsonPropertyName("left_tone")] public float LeftTone { get; init; }
    [JsonRequired, JsonPropertyName("right_tone")] public float RightTone { get; init; }
    [JsonRequired, JsonPropertyName("left_conversation_boost")] public bool LeftConversationBoost { get; init; }
    [JsonRequired, JsonPropertyName("right_conversation_boost")] public bool RightConversationBoost { get; init; }
    [JsonRequired, JsonPropertyName("left_ambient_noise_reduction")] public float LeftAmbientNoiseReduction { get; init; }
    [JsonRequired, JsonPropertyName("right_ambient_noise_reduction")] public float RightAmbientNoiseReduction { get; init; }
    [JsonIgnore] public bool IsValid =>
        float.IsFinite(LeftAmplification) && LeftAmplification is >= -1 and <= 1 &&
        float.IsFinite(RightAmplification) && RightAmplification is >= -1 and <= 1 &&
        float.IsFinite(LeftTone) && LeftTone is >= -1 and <= 1 &&
        float.IsFinite(RightTone) && RightTone is >= -1 and <= 1 &&
        float.IsFinite(LeftAmbientNoiseReduction) && LeftAmbientNoiseReduction is >= 0 and <= 1 &&
        float.IsFinite(RightAmbientNoiseReduction) && RightAmbientNoiseReduction is >= 0 and <= 1;
}

public sealed record HardwareDiagnosticEar
{
    [JsonRequired, JsonPropertyName("measured_at_unix_seconds")] public ulong MeasuredAtUnixSeconds { get; init; }
    [JsonPropertyName("first_use_unix_seconds")] public uint? FirstUseUnixSeconds { get; init; }
    [JsonRequired, JsonPropertyName("general")] public byte General { get; init; }
    [JsonRequired, JsonPropertyName("measurement")] public byte Measurement { get; init; }
    [JsonRequired, JsonPropertyName("component_failures")] public byte? ComponentFailures { get; init; }
    [JsonRequired, JsonPropertyName("unknown_bits")] public uint UnknownBits { get; init; }
    [JsonIgnore] public bool IsValid => General <= 7 && Measurement <= 7 &&
        (ComponentFailures is null || (MeasuredAtUnixSeconds != 0 && Measurement <= 1));
}

public sealed record HardwareDiagnostics
{
    [JsonRequired, JsonPropertyName("read_at_unix_seconds")] public ulong ReadAtUnixSeconds { get; init; }
    [JsonRequired, JsonPropertyName("left")] public required HardwareDiagnosticEar Left { get; init; }
    [JsonRequired, JsonPropertyName("right")] public required HardwareDiagnosticEar Right { get; init; }
    [JsonIgnore] public bool IsValid => Left is { IsValid: true } && Right is { IsValid: true };
}

public sealed record HearingMetadata
{
    [JsonRequired, JsonPropertyName("read_at_unix_seconds")] public ulong ReadAtUnixSeconds { get; init; }
    [JsonRequired, JsonPropertyName("test_mode_raw")] public byte? TestModeRaw { get; init; }
    [JsonRequired, JsonPropertyName("dosimetry_sensitivity_raw")] public float? DosimetrySensitivityRaw { get; init; }
    [JsonRequired, JsonPropertyName("dosimetry_volume_curve_raw")] public float[]? DosimetryVolumeCurveRaw { get; init; }
    [JsonIgnore] public bool IsValid => (DosimetrySensitivityRaw is null || float.IsFinite(DosimetrySensitivityRaw.Value)) &&
        (DosimetryVolumeCurveRaw is null || DosimetryVolumeCurveRaw.Length == 17 && DosimetryVolumeCurveRaw.All(float.IsFinite));

    public bool SameValues(HearingMetadata? other) => other is not null && ReadAtUnixSeconds == other.ReadAtUnixSeconds &&
        TestModeRaw == other.TestModeRaw && DosimetrySensitivityRaw == other.DosimetrySensitivityRaw &&
        (DosimetryVolumeCurveRaw is null ? other.DosimetryVolumeCurveRaw is null :
            other.DosimetryVolumeCurveRaw is not null && DosimetryVolumeCurveRaw.SequenceEqual(other.DosimetryVolumeCurveRaw));
}

public sealed record HardwareCheckState
{
    [JsonRequired, JsonPropertyName("request_id")] public ulong RequestId { get; init; }
    [JsonRequired, JsonPropertyName("phase")] public required string Phase { get; init; }
    [JsonRequired, JsonPropertyName("left_updated")] public bool LeftUpdated { get; init; }
    [JsonRequired, JsonPropertyName("right_updated")] public bool RightUpdated { get; init; }
    [JsonRequired, JsonPropertyName("error")] public string? Error { get; init; }
    [JsonIgnore] public bool IsValid => RequestId != 0 &&
        Phase is "waiting_case_close" or "waiting_result" or "completed" or "timed_out" or "cancelled" or "error" &&
        (Phase == "completed") == (LeftUpdated && RightUpdated) &&
        (Phase != "waiting_case_close" || !LeftUpdated && !RightUpdated) &&
        (Phase == "error" ? !string.IsNullOrWhiteSpace(Error) : Error is null);
}

public sealed record NearbyFinderState
{
    [JsonPropertyName("revision")] public ulong Revision { get; init; }
    [JsonPropertyName("active")] public bool Active { get; init; }
    [JsonPropertyName("paused")] public bool Paused { get; init; }
    [JsonPropertyName("available_identity")] public bool AvailableIdentity { get; init; }
    [JsonPropertyName("rssi")] public short? Rssi { get; init; }
    [JsonPropertyName("trend")] public string Trend { get; init; } = "unknown";
    [JsonPropertyName("last_heard_age_ms")] public ulong? LastHeardAgeMs { get; init; }
}

public sealed class DongleAudioState
{
    [JsonPropertyName("requested_codec")] public byte RequestedCodec { get; init; }
    [JsonPropertyName("codec")] public byte Codec { get; init; }
    [JsonPropertyName("variable")] public bool Variable { get; init; }
    [JsonPropertyName("requested_bitrate")] public uint RequestedBitrate { get; init; }
    [JsonPropertyName("bitrate")] public uint Bitrate { get; init; }
    [JsonPropertyName("game_mode_requested")] public bool GameModeRequested { get; init; }
    [JsonPropertyName("game_mode_state")] public string GameModeState { get; init; } = "off";
    [JsonPropertyName("input")] public byte Input { get; init; }
    [JsonPropertyName("pending")] public bool Pending { get; init; }
    [JsonPropertyName("streaming")] public bool Streaming { get; init; }
    [JsonPropertyName("microphone_active")] public bool MicrophoneActive { get; init; }
    [JsonPropertyName("game_mode_supported")] public bool GameModeSupported { get; init; }
    [JsonPropertyName("reported_delay_tenths_ms")] public ushort? ReportedDelayTenthsMs { get; init; }
    [JsonPropertyName("priming_ms")] public ushort PrimingMs { get; init; }
    [JsonIgnore] public bool IsValid => RequestedCodec <= 3 && Codec <= 3 && Input <= 2 && PrimingMs == 100 &&
        GameModeState is "off" or "pending" or "unconfirmed" or "confirmed" or "failed" or "unsupported" or "restoring";
}

public sealed class DonglePerformanceState
{
    [JsonPropertyName("input")] public byte Input { get; init; }
    [JsonPropertyName("codec")] public byte Codec { get; init; }
    [JsonPropertyName("flags")] public byte Flags { get; init; }
    [JsonPropertyName("uptime_ms")] public uint? UptimeMs { get; init; }
    [JsonPropertyName("pcm_used")] public uint? PcmUsed { get; init; }
    [JsonPropertyName("pcm_capacity")] public uint? PcmCapacity { get; init; }
    [JsonPropertyName("pcm_high_water")] public uint? PcmHighWater { get; init; }
    [JsonPropertyName("underflow_frames")] public uint? UnderflowFrames { get; init; }
    [JsonPropertyName("dropped_frames")] public uint? DroppedFrames { get; init; }
    [JsonPropertyName("encoded_units")] public uint? EncodedUnits { get; init; }
    [JsonPropertyName("encode_us")] public uint? EncodeUs { get; init; }
    [JsonPropertyName("encode_max_us")] public uint? EncodeMaxUs { get; init; }
    [JsonPropertyName("send_failures")] public uint? SendFailures { get; init; }
    [JsonPropertyName("recoveries")] public uint? Recoveries { get; init; }
    [JsonPropertyName("heap_capacity")] public uint? HeapCapacity { get; init; }
    [JsonPropertyName("heap_allocated")] public uint? HeapAllocated { get; init; }
    [JsonPropertyName("heap_remaining")] public uint? HeapRemaining { get; init; }
    [JsonPropertyName("heap_arena")] public uint? HeapArena { get; init; }
    [JsonIgnore] public bool IsValid => Input <= 2 && Codec <= 3 && (Flags & ~15) == 0 &&
        !(PcmUsed > PcmCapacity) && !(PcmHighWater > PcmCapacity) && !(HeapAllocated > HeapCapacity) && !(HeapRemaining > HeapCapacity);
}

public sealed class DongleHealthState
{
    [JsonPropertyName("available")] public bool Available { get; set; }
    [JsonPropertyName("model")] public string Model { get; set; } = "";
    [JsonPropertyName("firmware")] public string Firmware { get; set; } = "";
    [JsonPropertyName("earbuds_in_case")] public bool? EarbudsInCase { get; set; }
    [JsonPropertyName("device_controls")] public Dictionary<byte, uint> DeviceControls { get; set; } = new();
    [JsonPropertyName("device_controls_requested")] public Dictionary<byte, uint> DeviceControlsRequested { get; set; } = new();
    [JsonPropertyName("device_controls_pending")] public Dictionary<byte, uint> DeviceControlsPending { get; set; } = new();
    [JsonPropertyName("device_settings_pending")] public bool DeviceSettingsPending { get; set; }
    [JsonPropertyName("device_settings_error")] public string? DeviceSettingsError { get; set; }
    [JsonPropertyName("allow_off_requested")] public bool? AllowOffRequested { get; set; }
    [JsonPropertyName("adaptive_strength_requested")] public uint? AdaptiveStrengthRequested { get; set; }
    [JsonPropertyName("anc_unconfirmed")] public bool AncUnconfirmed { get; set; }
    [JsonPropertyName("audio")] public DongleAudioState? Audio { get; set; }
    [JsonPropertyName("performance")] public DonglePerformanceState? Performance { get; set; }
    [JsonPropertyName("nearby_finder")] public NearbyFinderState NearbyFinder { get; set; } = new();
    [JsonPropertyName("hearing_aid_enabled")] public bool? HearingAidEnabled { get; set; }
    [JsonPropertyName("hearing_aid_revision")] public ulong HearingAidRevision { get; set; }
    [JsonPropertyName("hearing_aid_enrolled")] public bool? HearingAidEnrolled { get; set; }
    [JsonPropertyName("media_assist_enabled")] public bool? MediaAssistEnabled { get; set; }
    [JsonPropertyName("media_assist_revision")] public ulong MediaAssistRevision { get; set; }
    [JsonPropertyName("media_assist_available")] public bool MediaAssistAvailable { get; set; }
    [JsonPropertyName("hearing_aid_settings")] public HearingAidSettings? HearingAidSettings { get; set; }
    [JsonPropertyName("hearing_aid_editable")] public bool HearingAidEditable { get; set; }
    [JsonPropertyName("hearing_aid_pending")] public bool HearingAidPending { get; set; }
    [JsonPropertyName("hearing_aid_error")] public string? HearingAidError { get; set; }
    [JsonPropertyName("hardware_diagnostics")] public HardwareDiagnostics? HardwareDiagnostics { get; set; }
    [JsonPropertyName("hardware_diagnostics_pending")] public bool HardwareDiagnosticsPending { get; set; }
    [JsonPropertyName("hardware_diagnostics_error")] public string? HardwareDiagnosticsError { get; set; }
    [JsonPropertyName("hearing_metadata")] public HearingMetadata? HearingMetadata { get; set; }
    [JsonPropertyName("hearing_metadata_pending")] public bool HearingMetadataPending { get; set; }
    [JsonPropertyName("hearing_metadata_error")] public string? HearingMetadataError { get; set; }
    [JsonPropertyName("hardware_check")] public HardwareCheckState? HardwareCheck { get; set; }
    [JsonIgnore] public bool IsValid => Model is not null && Firmware is not null && DeviceControls is not null &&
        DeviceControlsRequested is not null && DeviceControlsPending is not null && NearbyFinder is not null &&
        DeviceControls.Count <= 32 && DeviceControlsRequested.Count <= 32 && DeviceControlsPending.Count <= 32 &&
        NearbyFinder.Rssi is null or (>= -126 and <= 20) &&
        NearbyFinder.Trend is "unknown" or "stronger" or "steady" or "weaker" &&
        (!NearbyFinder.Active || NearbyFinder.AvailableIdentity) &&
        Audio is not { IsValid: false } && Performance is not { IsValid: false } &&
        HearingAidSettings is not { IsValid: false } && HardwareDiagnostics is not { IsValid: false } &&
        HearingMetadata is not { IsValid: false } && HardwareCheck is not { IsValid: false };
}

/// The daemon's authoritative state, pushed on connect and on every change.
public sealed class DongleState
{
    [JsonPropertyName("present")] public bool Present { get; set; }
    [JsonPropertyName("ambiguous")] public bool Ambiguous { get; set; }
    [JsonPropertyName("session")] public ulong Session { get; set; }
    [JsonPropertyName("generation")] public uint Generation { get; set; }
    [JsonPropertyName("pair_token")] public ulong PairToken { get; set; }
    [JsonPropertyName("selected")] public bool Selected { get; set; }
    [JsonPropertyName("connected")] public bool Connected { get; set; }
    [JsonPropertyName("controls_ready")] public bool ControlsReady { get; set; }
    [JsonPropertyName("streaming")] public bool Streaming { get; set; }
    [JsonPropertyName("phone_connected")] public bool PhoneConnected { get; set; }
    [JsonPropertyName("microphone_active")] public bool MicrophoneActive { get; set; }
    [JsonPropertyName("phone_pairing")] public bool PhonePairing { get; set; }
    [JsonPropertyName("usb_playback")] public bool UsbPlayback { get; set; }
    [JsonPropertyName("anc")] public byte Anc { get; set; }
    [JsonPropertyName("battery")] public Battery Battery { get; set; } = new();
    [JsonPropertyName("battery_age_ms")] public uint?[] BatteryAgeMs { get; set; } = [null, null, null];
    [JsonPropertyName("battery_present")] public byte BatteryPresent { get; set; }
    [JsonPropertyName("battery_case_source")] public byte BatteryCaseSource { get; set; }
    [JsonPropertyName("codec")] public byte Codec { get; set; }
    [JsonPropertyName("capabilities")] public ushort Capabilities { get; set; }
    [JsonPropertyName("pending")] public bool Pending { get; set; }
    [JsonPropertyName("error")] public string? Error { get; set; }
    [JsonPropertyName("health")] public DongleHealthState Health { get; set; } = new();
    [JsonIgnore] public bool IsValid => Battery is not null && BatteryAgeMs is { Length: 3 } &&
        (BatteryPresent & ~7) == 0 && (BatteryCaseSource & ~7) == 0 &&
        ((Capabilities & 64) != 0 ?
            ValidBatteryObservation(1, Battery.Left, Battery.LeftCharging, 0) &&
            ValidBatteryObservation(2, Battery.Right, Battery.RightCharging, 1) &&
            ValidBatteryObservation(4, Battery.Case, Battery.CaseCharging, 2) : BatteryCaseSource == 0);

    private bool ValidBatteryObservation(int bit, int? level, bool charging, int component) =>
        (BatteryPresent & bit) != 0 ? level is >= 0 and <= 100 && BatteryAgeMs[component] is not null : !charging;
}

public sealed class Snapshot
{
    [JsonPropertyName("dongle")] public DongleState Dongle { get; set; } = new();
    [JsonPropertyName("device_id")] public string DeviceId { get; set; } = "";
    [JsonPropertyName("connected")] public bool Connected { get; set; }

    /// The daemon detected a stale AAP session; Connected alone does not imply usable controls.
    [JsonPropertyName("link_stale")] public bool LinkStale { get; set; }
    [JsonPropertyName("dev_name")] public string DevName { get; set; } = "";
    [JsonPropertyName("battery")] public Battery Battery { get; set; } = new();
    [JsonPropertyName("battery_freshness")] public BatteryFreshness? BatteryFreshness { get; set; }
    [JsonPropertyName("battery_live")] public bool BatteryLive { get; set; }
    [JsonPropertyName("battery_timestamp_ms")] public ulong? BatteryTimestampMs { get; set; }
    [JsonPropertyName("battery_source")] public string BatterySource { get; set; } = "";
    [JsonPropertyName("ble_scan_while_connected")] public bool BleScanWhileConnected { get; set; }

    /// 0 = unknown, 1 = Off, 2 = Noise Cancellation, 3 = Transparency, 4 = Adaptive.
    [JsonPropertyName("anc")] public byte Anc { get; set; }
    [JsonPropertyName("anc_unconfirmed")] public bool AncUnconfirmed { get; set; }

    [JsonPropertyName("mic_recording")] public bool MicRecording { get; set; }
    [JsonPropertyName("mic_capture_active")] public bool MicCaptureActive { get; set; }
    [JsonPropertyName("mic_uplink_active")] public bool MicUplinkActive { get; set; }
    [JsonPropertyName("mic_state")] public string MicState { get; set; } = "unavailable";
    [JsonPropertyName("mic_error")] public string? MicError { get; set; }
    [JsonPropertyName("mic_available")] public bool MicAvailable { get; set; }
    [JsonPropertyName("auto_mode")] public bool AutoMode { get; set; }
    [JsonPropertyName("mic_mode")] public string MicMode { get; set; } = "automatic";
    [JsonPropertyName("mic_stop_delay_secs")] public uint MicStopDelaySecs { get; set; }
    [JsonPropertyName("mic_manual_active")] public bool MicManualActive { get; set; }
    [JsonPropertyName("playback_mode")] public string PlaybackMode { get; set; } = "windows";
    [JsonPropertyName("playback_preferred_mode")] public string? PlaybackPreferredMode { get; set; }
    [JsonPropertyName("playback_state")] public string PlaybackState { get; set; } = "windows";
    [JsonPropertyName("audio_output_ready")] public bool AudioOutputReady { get; set; }
    [JsonPropertyName("playback_error")] public string? PlaybackError { get; set; }
    [JsonPropertyName("playback_available")] public bool PlaybackAvailable { get; set; }
    [JsonPropertyName("playback_native_driver")] public bool PlaybackNativeDriver { get; set; }
    [JsonPropertyName("playback_aac_bitrate")] public PlaybackBitrate PlaybackAacBitrate { get; set; } = new();
    [JsonPropertyName("playback_aac_eld_bitrate")] public PlaybackBitrate PlaybackAacEldBitrate { get; set; } = new();
    [JsonPropertyName("playback_link")] public PlaybackLinkSettings? PlaybackLink { get; set; }
    [JsonPropertyName("playback_game_mode")] public bool? PlaybackGameMode { get; set; }
    [JsonPropertyName("playback_bitrate_kbps")] public uint? PlaybackBitrateKbps { get; set; }
    [JsonPropertyName("playback_delay_reporting")] public bool PlaybackDelayReporting { get; set; }
    [JsonPropertyName("playback_delay_tenths_ms")] public ushort? PlaybackDelayTenthsMs { get; set; }
    [JsonPropertyName("playback_buffer_target_ms")] public ushort? PlaybackBufferTargetMs { get; set; }
    [JsonPropertyName("playback_buffer_error")] public string? PlaybackBufferError { get; set; }
    [JsonPropertyName("conversational_awareness")] public bool ConversationalAwareness { get; set; }
    [JsonPropertyName("conversational_awareness_pending")] public bool? ConversationalAwarenessPending { get; set; }
    [JsonPropertyName("conversation_volume")] public byte ConversationVolume { get; set; } = 25;
    [JsonPropertyName("adaptive_volume")] public bool AdaptiveVolume { get; set; }
    [JsonPropertyName("hearing_aid_enabled")] public bool? HearingAidEnabled { get; set; }
    [JsonPropertyName("hearing_aid_revision")] public ulong HearingAidRevision { get; set; }
    [JsonPropertyName("hearing_aid_enrolled")] public bool? HearingAidEnrolled { get; set; }
    [JsonPropertyName("media_assist_enabled")] public bool? MediaAssistEnabled { get; set; }
    [JsonPropertyName("media_assist_revision")] public ulong MediaAssistRevision { get; set; }
    [JsonPropertyName("media_assist_available")] public bool MediaAssistAvailable { get; set; }
    [JsonPropertyName("hearing_aid_settings")] public HearingAidSettings? HearingAidSettings { get; set; }
    [JsonPropertyName("hearing_aid_editable")] public bool HearingAidEditable { get; set; }
    [JsonPropertyName("hearing_aid_pending")] public bool HearingAidPending { get; set; }
    [JsonPropertyName("hearing_aid_error")] public string? HearingAidError { get; set; }
    [JsonPropertyName("hardware_diagnostics")] public HardwareDiagnostics? HardwareDiagnostics { get; set; }
    [JsonPropertyName("hardware_diagnostics_pending")] public bool HardwareDiagnosticsPending { get; set; }
    [JsonPropertyName("hardware_diagnostics_error")] public string? HardwareDiagnosticsError { get; set; }
    [JsonPropertyName("hearing_metadata")] public HearingMetadata? HearingMetadata { get; set; }
    [JsonPropertyName("hearing_metadata_pending")] public bool HearingMetadataPending { get; set; }
    [JsonPropertyName("hearing_metadata_error")] public string? HearingMetadataError { get; set; }
    [JsonPropertyName("hardware_check")] public HardwareCheckState? HardwareCheck { get; set; }
    [JsonPropertyName("earbuds_in_case")] public bool? EarbudsInCase { get; set; }
    [JsonPropertyName("loud_sound_reduction")] public bool? LoudSoundReduction { get; set; }
    [JsonPropertyName("loud_sound_reduction_pending")] public bool LoudSoundReductionPending { get; set; }
    [JsonPropertyName("loud_sound_reduction_error")] public string? LoudSoundReductionError { get; set; }
    [JsonPropertyName("transparency")] public TransparencySettings? Transparency { get; set; }
    [JsonPropertyName("transparency_pending")] public bool TransparencyPending { get; set; }
    [JsonPropertyName("transparency_error")] public string? TransparencyError { get; set; }
    [JsonPropertyName("allow_off")] public bool AllowOff { get; set; }
    /// Last successfully sent choice on Pro 3, which may not acknowledge this setting.
    [JsonPropertyName("allow_off_requested")] public bool? AllowOffRequested { get; set; }
    [JsonPropertyName("device_controls")] public Dictionary<byte, uint> DeviceControls { get; set; } = new();
    [JsonPropertyName("device_controls_requested")] public Dictionary<byte, uint> DeviceControlsRequested { get; set; } = new();
    [JsonPropertyName("device_controls_pending")] public Dictionary<byte, uint> DeviceControlsPending { get; set; } = new();
    [JsonPropertyName("aacp_capabilities")] public Dictionary<byte, uint> AacpCapabilities { get; set; } = new();
    /// Successfully sent strength, awaiting an actual device report.
    [JsonPropertyName("adaptive_strength_requested")] public uint? AdaptiveStrengthRequested { get; set; }
    [JsonPropertyName("device_settings_error")] public string? DeviceSettingsError { get; set; }
    [JsonPropertyName("eq")] public Equalizer? Eq { get; set; }
    [JsonPropertyName("eq_pending")] public Equalizer? EqPending { get; set; }
    [JsonPropertyName("auto_pause")] public bool AutoPause { get; set; } = true;
    [JsonPropertyName("volume")] public byte Volume { get; set; }
    [JsonPropertyName("muted")] public bool Muted { get; set; }
    [JsonPropertyName("heart_rate")] public ushort? HeartRate { get; set; }
    [JsonPropertyName("heart_rate_enabled")] public bool HeartRateEnabled { get; set; }
    [JsonPropertyName("heart_rate_timestamp_ms")] public ulong? HeartRateTimestampMs { get; set; }
    [JsonPropertyName("head_tracking_enabled")] public bool HeadTrackingEnabled { get; set; }
    [JsonPropertyName("head_tracking_state")] public string HeadTrackingState { get; set; } = "idle";
    [JsonPropertyName("head_tracking_error")] public string? HeadTrackingError { get; set; }
    [JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    [JsonPropertyName("nearby_finder")] public NearbyFinderState? NearbyFinder { get; set; }

    // AAP 0x1D metadata includes the serial number; keep it behind the Settings reveal toggle.
    [JsonPropertyName("model")] public string Model { get; set; } = "";
    [JsonPropertyName("firmware")] public string Firmware { get; set; } = "";
    [JsonPropertyName("serial")] public string Serial { get; set; } = "";

    /// Hosts reported by AAP 0x2E, including connections Windows cannot observe.
    [JsonPropertyName("multipoint")] public List<ConnectedDevice> Multipoint { get; set; } = new();
}

public sealed record Equalizer
{
    [JsonRequired, JsonPropertyName("mode")] public byte Mode { get; init; }
    [JsonRequired, JsonPropertyName("low")] public byte Low { get; init; }
    [JsonRequired, JsonPropertyName("mid")] public byte Mid { get; init; }
    [JsonRequired, JsonPropertyName("high")] public byte High { get; init; }

    [JsonIgnore] public bool IsValid => Mode is 1 or 2 && Low <= 100 && Mid <= 100 && High <= 100;
}

/// One host connected to the AirPods, as reported by the buds themselves.
public sealed class ConnectedDevice
{
    [JsonPropertyName("address")] public string Address { get; set; } = "";
    [JsonPropertyName("is_this_pc")] public bool IsThisPc { get; set; }
    [JsonPropertyName("name")] public string Name { get; set; } = "";

    /// "pc", "laptop", "phone", "watch", "tablet", "audio" or "unknown".
    [JsonPropertyName("kind")] public string Kind { get; set; } = "unknown";

    /// Segoe Fluent Icons glyph for this kind.
    public string Glyph => Kind switch
    {
        "pc" => "\uE977",      // Devices2 (desktop)
        "laptop" => "\uE7F8",  // Laptop
        "phone" => "\uE8EA",   // CellPhone
        "tablet" => "\uE70A",  // Tablet
        "watch" => "\uE916",   // Stopwatch glyph used for watches
        "audio" => "\uE7F6",   // Headphone
        _ => "\uE783",         // Error/unknown
    };

    public string Detail => IsThisPc ? Address : Address;
}

/// AAP control-command feature ids (the `id` byte of a 0x09 control command).
public static class Feature
{
    public const byte AdaptiveVolume = 0x26;         // 38
    public const byte ConversationalAwareness = 0x28; // 40
    public const byte AllowOff = 0x34;                // 52
}

/// The raw control id for Adaptive-Audio noise strength (value 0..=100).
public static class ControlId
{
    public const byte AdaptiveNoiseStrength = 0x2E; // 46
}

// ---------------------------------------------------------------------------
// Commands (app → daemon). Each carries its snake_case `cmd` tag literally.
// ---------------------------------------------------------------------------

public sealed class HelloCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "hello";
    [JsonPropertyName("kind")] public string Kind => "app"; // "tray" | "app"
}

public sealed class GetStateCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "get_state";
}

public sealed class DongleControlCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "dongle_control";
    [JsonPropertyName("session")] public ulong Session { get; init; }
    [JsonPropertyName("generation")] public uint Generation { get; init; }
    [JsonPropertyName("opcode")] public byte Opcode { get; init; }
    [JsonPropertyName("argument")] public byte Argument { get; init; }
}

public sealed class DongleHealthCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "dongle_health";
    [JsonPropertyName("session")] public ulong Session { get; init; }
    [JsonPropertyName("generation")] public uint Generation { get; init; }
    [JsonPropertyName("command")] public required object Command { get; init; }
}

public sealed class PingCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "ping";
}

public sealed class SetBleScanWhileConnectedCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_ble_scan_while_connected";
    [JsonPropertyName("enabled")] public bool Enabled { get; init; }
}

public sealed class SetAncCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_anc";
    [JsonPropertyName("mode")] public byte Mode { get; init; } // 1..=4
    [JsonPropertyName("device_id"), JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? DeviceId { get; init; }
}

public sealed class SetMicModeCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_mic_mode";
    [JsonPropertyName("auto")] public bool Auto { get; init; }
    [JsonPropertyName("manual")] public bool Manual { get; init; }
}

public sealed class SetMicPolicyCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_mic_policy";
    [JsonPropertyName("mode")] public string Mode { get; init; } = "automatic";
    [JsonPropertyName("stop_delay_secs")] public uint StopDelaySecs { get; init; }
}

public sealed class SetMicManualActiveCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_mic_manual_active";
    [JsonPropertyName("active")] public bool Active { get; init; }
}

public sealed class ToggleMicManualCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "toggle_mic_manual";
}

public sealed class SetPlaybackModeCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_playback_mode";
    [JsonPropertyName("mode")] public string Mode { get; init; } = "windows";
}

public sealed class RetryPlaybackCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "retry_playback";
}

public sealed class SetPlaybackBitrateCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_playback_bitrate";
    [JsonPropertyName("mode")] public string Mode { get; init; } = "aac";
    [JsonPropertyName("best_effort")] public bool BestEffort { get; init; } = true;
    [JsonPropertyName("kbps")] public uint Kbps { get; init; } = 256;
    [JsonPropertyName("variable")] public bool Variable { get; init; }
}

public sealed class SetPlaybackGameModeCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_playback_game_mode";
    [JsonPropertyName("enabled")] public bool Enabled { get; init; }
}

public sealed class SetDongleAudioProfileCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_dongle_audio_profile";
    [JsonPropertyName("codec")] public byte Codec { get; init; }
    [JsonPropertyName("variable")] public bool Variable { get; init; }
    [JsonPropertyName("bitrate")] public uint Bitrate { get; init; }
}

public sealed class GetDonglePerformanceCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "get_dongle_performance";
}

public sealed class SetPlaybackLinkCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_playback_link";
    [JsonPropertyName("settings")] public PlaybackLinkSettings Settings { get; init; } = new();
}

public sealed class SetPlaybackBufferCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_playback_buffer";
    [JsonPropertyName("target_ms")] public ushort? TargetMs { get; init; }
    [JsonPropertyName("restore_ms")] public ushort? RestoreMs { get; init; }
}

public sealed class SetFeatureCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_feature";
    [JsonPropertyName("feature")] public byte Feature { get; init; }
    [JsonPropertyName("on")] public bool On { get; init; }
}

public sealed class SetControlCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_control";
    [JsonPropertyName("id")] public byte Id { get; init; }
    [JsonPropertyName("value")] public byte Value { get; init; }
}

public sealed class SetDeviceControlCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_device_control";
    [JsonPropertyName("id")] public byte Id { get; init; }
    [JsonPropertyName("value")] public uint Value { get; init; }
    [JsonPropertyName("device_id"), JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? DeviceId { get; init; }
}

public sealed class SetEqCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_eq";
    [JsonPropertyName("mode")] public byte Mode { get; init; }
    [JsonPropertyName("low")] public byte Low { get; init; }
    [JsonPropertyName("mid")] public byte Mid { get; init; }
    [JsonPropertyName("high")] public byte High { get; init; }
}

public sealed class SetAutoPauseCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_auto_pause";
    [JsonPropertyName("on")] public bool On { get; init; }
}

public sealed class RefreshDeviceSettingsCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "refresh_device_settings";
}

public sealed class CompanionLinkCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "companion_link";
    [JsonPropertyName("peer_address")] public required string PeerAddress { get; init; }
    [JsonPropertyName("secret_hex")] public required string SecretHex { get; init; }
    [JsonPropertyName("device_id")] public required string DeviceId { get; init; }
}

public sealed class CompanionStatusCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "companion_status";
}

public sealed class CompanionSwitchCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "companion_switch";
    [JsonPropertyName("target")] public required string Target { get; init; }
    [JsonPropertyName("device_id")] public required string DeviceId { get; init; }
    [JsonPropertyName("request_id")] public ulong RequestId { get; init; }
}

public sealed class CompanionForgetCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "companion_forget";
}

public sealed class CompanionSetPreferenceCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "companion_set_preference";
    [JsonPropertyName("key")] public required string Key { get; init; }
    [JsonPropertyName("value")] public required JsonElement Value { get; init; }
    [JsonPropertyName("device_id")] public string? DeviceId { get; init; }
    [JsonPropertyName("request_id")] public ulong RequestId { get; init; }
}

public sealed class CompanionCallConsentCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "companion_call_consent";
    [JsonPropertyName("enabled")] public bool Enabled { get; init; }
    [JsonPropertyName("device_id")] public required string DeviceId { get; init; }
    [JsonPropertyName("request_id")] public ulong RequestId { get; init; }
}

public sealed record ConversationPolicy
{
    [JsonPropertyName("mode")] public string Mode { get; init; } = "abs_cap";
    [JsonPropertyName("first")] public string First { get; init; } = "25";
    [JsonPropertyName("second")] public string Second { get; init; } = "15";
    [JsonPropertyName("pause_media")] public bool PauseMedia { get; init; }
    [JsonIgnore] public bool IsValid => Mode is "abs_cap" or "relative" &&
        Percent(First, out var first) && Percent(Second, out var second) && second <= first;

    internal static bool Percent(string? text, out decimal value)
    {
        value = 0;
        return text is { Length: > 0 and <= 13 } &&
            System.Text.RegularExpressions.Regex.IsMatch(text, @"\A(?:0|[1-9][0-9]?|100)(?:\.[0-9]{0,8}[1-9])?\z") &&
            decimal.TryParse(text, System.Globalization.NumberStyles.AllowDecimalPoint,
                System.Globalization.CultureInfo.InvariantCulture, out value) && value <= 100;
    }
}

public sealed class CompanionPreference
{
    [JsonPropertyName("key")] public string Key { get; init; } = "";
    [JsonPropertyName("rev")] public string Rev { get; init; } = "";
    [JsonPropertyName("origin")] public string Origin { get; init; } = "";
    [JsonPropertyName("value")] public JsonElement Value { get; init; }
    [JsonIgnore] public bool IsValid => Rev is { Length: 16 } && Rev.Any(c => c != '0') &&
        Rev.All(c => c is >= '0' and <= '9' or >= 'a' and <= 'f') && Origin is "pc" or "android" && ValidValue(Key, Value);

    public static bool ValidValue(string key, JsonElement value)
    {
        if (key is "host_auto_pause" or "connection_popup_enabled" or "cache_disconnected_battery" or
            "call_takeover_enabled" or "call_return_enabled") return value.ValueKind is JsonValueKind.True or JsonValueKind.False;
        if (key == "conversation_policy") return ExactObject(value, "mode", "first", "second", "pause_media") &&
            value.Deserialize<ConversationPolicy>(Wire.Json) is { IsValid: true };
        if (key == "desired_hold_actions")
        {
            if (!ExactObject(value, "left", "right") || !String(value.GetProperty("left"), out var left) ||
                !String(value.GetProperty("right"), out var right)) return false;
            return left == "unset" && right == "unset" || left is "listening" or "assistant" && right is "listening" or "assistant";
        }
        if (!String(value, out var text)) return false;
        return key switch
        {
            "theme" => text is "system" or "light" or "dark",
            "desired_microphone_side" => text is "unset" or "auto" or "right" or "left",
            "desired_ear_detection" or "desired_case_sounds" or "desired_allow_off" => text is "unset" or "on" or "off",
            "desired_swipe_duration" => text is "unset" or "0" or "1" or "2",
            "desired_case_volume" => text == "unset" || System.Text.RegularExpressions.Regex.IsMatch(text!, @"\A(?:0|[1-9][0-9]?|100)\z"),
            "desired_noise_modes_cycle" => text == "unset" || text is { Length: 2 } && text[0] == '0' &&
                "35679abcdef".Contains(text[1]),
            _ => false,
        };
    }

    private static bool String(JsonElement value, out string? text)
    {
        text = value.ValueKind == JsonValueKind.String ? value.GetString() : null;
        return text is not null;
    }

    private static bool ExactObject(JsonElement value, params string[] keys) => value.ValueKind == JsonValueKind.Object &&
        value.EnumerateObject().Count() == keys.Length && keys.All(key => value.TryGetProperty(key, out _));
}

public sealed class CompanionPreferences
{
    [JsonPropertyName("device_id")] public string? DeviceId { get; init; }
    [JsonPropertyName("records")] public CompanionPreference[] Records { get; init; } = [];
    [JsonPropertyName("local_call_consent")] public bool LocalCallConsent { get; init; }
    [JsonPropertyName("peer_prefs")] public bool PeerPrefs { get; init; }
    [JsonPropertyName("peer_calls")] public bool PeerCalls { get; init; }
    [JsonPropertyName("error")] public string? Error { get; init; }
    [JsonIgnore] public bool IsValid => Records is { Length: <= 15 } && Records.All(record => record is { IsValid: true }) &&
        Records.Select(record => record.Key).Distinct(StringComparer.Ordinal).Count() == Records.Length && Error is null or { Length: <= 512 };
    public bool Matches(string deviceId) => DeviceId is { Length: > 0 } &&
        string.Equals(DeviceId.Replace(":", ""), deviceId.Replace(":", ""), StringComparison.OrdinalIgnoreCase);
    public JsonElement? Find(string key) => Records.FirstOrDefault(record => record.Key == key)?.Value;
    public bool? Boolean(string key) => Find(key) is { ValueKind: JsonValueKind.True or JsonValueKind.False } value ? value.GetBoolean() : null;
    [JsonIgnore] public bool LocalCallTakeoverEnabled => LocalCallConsent && Boolean("call_takeover_enabled") == true;
    [JsonIgnore] public ConversationPolicy? Conversation => Find("conversation_policy") is { } value ? value.Deserialize<ConversationPolicy>(Wire.Json) : null;
}

public sealed class CompanionStatus
{
    [JsonPropertyName("linked")] public bool Linked { get; init; }
    [JsonPropertyName("connected")] public bool Connected { get; init; }
    [JsonPropertyName("device_id")] public string? DeviceId { get; init; }
    [JsonPropertyName("peer_address")] public string? PeerAddress { get; init; }
    [JsonPropertyName("held_away")] public bool HeldAway { get; init; }
    [JsonPropertyName("phase")] public string Phase { get; init; } = "idle";
    [JsonPropertyName("error")] public string? Error { get; init; }
}

public sealed class SetConversationVolumeCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_conversation_volume";
    [JsonPropertyName("percent")] public byte Percent { get; init; }
}

public sealed class StepVolumeCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "step_volume";
    [JsonPropertyName("delta")] public int Delta { get; init; }
}

public sealed class SetVolumeCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_volume";
    [JsonPropertyName("percent")] public byte Percent { get; init; }
}

public sealed class ToggleMuteCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "toggle_mute";
}

public sealed class SetHeartRateCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_heart_rate";
    [JsonPropertyName("on")] public bool On { get; init; }
}

public sealed class SetHeadTrackingCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_head_tracking";
    [JsonPropertyName("on")] public bool On { get; init; }
}

public sealed class StartNearbyFinderCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "start_nearby_finder";
    [JsonPropertyName("device_id")] public required string DeviceId { get; init; }
}

public sealed class StopNearbyFinderCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "stop_nearby_finder";
    [JsonPropertyName("device_id")] public required string DeviceId { get; init; }
}

public sealed class GetLoudSoundReductionCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "get_loud_sound_reduction";
}

public sealed class GetTransparencyCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "get_transparency";
}

public sealed class SetTransparencyCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_transparency";
    [JsonPropertyName("expected")] public TransparencySettings? Expected { get; init; }
    [JsonPropertyName("settings")] public required TransparencySettings Settings { get; init; }
}

public sealed class SetLoudSoundReductionCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_loud_sound_reduction";
    [JsonPropertyName("on")] public bool On { get; init; }
}

public sealed class GetHardwareDiagnosticsCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "get_hardware_diagnostics";
}

public sealed class GetHearingMetadataCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "get_hearing_metadata";
    [JsonPropertyName("device_id")] public string DeviceId { get; init; } = "";
}

public sealed class StartHardwareCheckCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "start_hardware_check";
    [JsonPropertyName("request_id")] public ulong RequestId { get; init; }
    [JsonPropertyName("device_id")] public string DeviceId { get; init; } = "";
    [JsonPropertyName("buds_in_case_confirmed")] public bool BudsInCaseConfirmed { get; init; }
}

public sealed class CancelHardwareCheckCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "cancel_hardware_check";
    [JsonPropertyName("request_id")] public ulong RequestId { get; init; }
    [JsonPropertyName("device_id")] public string DeviceId { get; init; } = "";
}

public sealed class ProbeHearingTestModeCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "probe_hearing_test_mode";
    [JsonPropertyName("request_id")] public ulong RequestId { get; init; }
    [JsonPropertyName("device_id")] public string DeviceId { get; init; } = "";
    [JsonPropertyName("enable_check")] public bool EnableCheck { get; init; }
}

public sealed record HearingTestModeProbeResult
{
    [JsonPropertyName("initial_state")] public byte? InitialState { get; init; }
    [JsonPropertyName("enabled_state")] public byte? EnabledState { get; init; }
    [JsonPropertyName("final_state")] public byte? FinalState { get; init; }
    [JsonRequired, JsonPropertyName("enable_sent")] public bool EnableSent { get; init; }
    [JsonRequired, JsonPropertyName("disable_sent")] public bool DisableSent { get; init; }
    [JsonRequired, JsonPropertyName("restoration_verified")] public bool RestorationVerified { get; init; }
    [JsonPropertyName("error")] public string? Error { get; init; }
    [JsonIgnore] public bool IsValid => (InitialState is null or <= 5) && (EnabledState is null or <= 5)
        && (FinalState is null or <= 5) && (!DisableSent || EnableSent || !string.IsNullOrEmpty(Error))
        && (!RestorationVerified || InitialState is not null && FinalState == InitialState);
}

public sealed class GetHearingAidCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "get_hearing_aid";
}

public sealed class SetHearingAidCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_hearing_aid";
    [JsonPropertyName("request_id")] public ulong RequestId { get; init; }
    [JsonPropertyName("on")] public bool On { get; init; }
    [JsonPropertyName("expected")] public HearingAidSettings? Expected { get; init; }
    [JsonPropertyName("settings")] public HearingAidSettings? Settings { get; init; }
}

public sealed class SetHearingAidModeCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_hearing_aid_mode";
    [JsonPropertyName("request_id")] public ulong RequestId { get; init; }
    [JsonPropertyName("on")] public bool On { get; init; }
}

public sealed class SetMediaAssistCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_media_assist";
    [JsonPropertyName("request_id")] public ulong RequestId { get; init; }
    [JsonPropertyName("on")] public bool On { get; init; }
}

public sealed class ConnectCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "connect";
    [JsonPropertyName("device_id"), JsonIgnore(Condition = JsonIgnoreCondition.WhenWritingNull)]
    public string? DeviceId { get; init; }
}

public sealed class PairingCompletedCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "pairing_completed";
    [JsonPropertyName("address")] public string Address { get; init; } = "";
}

public sealed class DisconnectCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "disconnect";
}

/// Restart audio services while preserving the AAP control session.
public sealed class RebuildAudioCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "rebuild_audio";
}

public sealed class RepairConnectionCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "repair_connection";
}

public sealed class SetNameCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_name";
    [JsonPropertyName("name")] public string Name { get; init; } = "";
    [JsonPropertyName("request_id")] public ulong RequestId { get; init; }
}

public sealed class ShutdownCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "shutdown";
}

// ---------------------------------------------------------------------------
// Event parsing (daemon → app). Dispatched on the `event` tag.
// ---------------------------------------------------------------------------

public abstract record DaemonEvent
{
    public sealed record Companion(CompanionStatus Status) : DaemonEvent;
    public sealed record Preferences(CompanionPreferences Value) : DaemonEvent;
    /// Full state, pushed on connect and whenever it changes.
    public sealed record State(Snapshot Snapshot) : DaemonEvent;

    /// A transient notification rendered in the island.
    public sealed record Overlay(string Title, string Body) : DaemonEvent;

    /// BLE proximity notification; the UI checks connection state before offering Connect.
    public sealed record ConnectPrompt(string Name) : DaemonEvent;
    public sealed record PairingNearby(ushort Model, bool LidOpen) : DaemonEvent;
    public sealed record PairedLid(string DeviceId, bool LidOpen, ulong Sequence) : DaemonEvent;
    public sealed record CommandResult(string Command, ulong RequestId, bool Ok, string Message) : DaemonEvent;
    public sealed record HearingTestModeProbe(ulong RequestId, string DeviceId, bool EnableCheck, HearingTestModeProbeResult Result) : DaemonEvent;
    public sealed record HeadTracking(ulong TimestampMs, short Horizontal, short Vertical) : DaemonEvent;
}

public static class Wire
{
    public static readonly JsonSerializerOptions Json = new()
    {
        DefaultIgnoreCondition = JsonIgnoreCondition.Never,
    };

    /// Serialize a command as one NDJSON line (trailing '\n').
    public static string ToLine(object command)
        => JsonSerializer.Serialize(command, command.GetType(), Json) + "\n";

    /// Parse one NDJSON line into a DaemonEvent, or null if unrecognized.
    public static DaemonEvent? ParseEvent(string line)
    {
        line = line.Trim();
        if (line.Length == 0) return null;

        try
        {
            using var doc = JsonDocument.Parse(line);
            var root = doc.RootElement;
            if (root.ValueKind != JsonValueKind.Object ||
                !root.TryGetProperty("event", out var tagProp) ||
                tagProp.ValueKind != JsonValueKind.String) return null;
            var tag = tagProp.GetString();

            switch (tag)
            {
                case "companion_preferences":
                    if (!root.TryGetProperty("preferences", out var preferences) || preferences.ValueKind != JsonValueKind.Object ||
                        preferences.Deserialize<CompanionPreferences>(Json) is not { IsValid: true } preferenceState) return null;
                    return new DaemonEvent.Preferences(preferenceState);
                case "companion_status":
                    if (!root.TryGetProperty("status", out var companion) || companion.ValueKind != JsonValueKind.Object ||
                        companion.Deserialize<CompanionStatus>(Json) is not { } companionStatus) return null;
                    return new DaemonEvent.Companion(companionStatus);
                case "state":
                    // Serde flattens Snapshot beside the event tag; deserialize the whole object.
                    var snap = root.Deserialize<Snapshot>(Json) ?? new Snapshot();
                    if (snap.Dongle is not { IsValid: true } || snap.Dongle.Health is not { IsValid: true } ||
                        snap.Battery is null || snap.BatterySource is null || snap.BatteryFreshness is { IsValid: false } ||
                        snap.DeviceControls is null || snap.DeviceControlsRequested is null || snap.DeviceControlsPending is null || snap.AacpCapabilities is null ||
                        snap.MicMode is not ("automatic" or "keep_active" or "manual") || snap.MicStopDelaySecs > 3600 ||
                        snap.PlaybackMode is not ("windows" or "aac" or "aac_eld") ||
                        snap.PlaybackPreferredMode is not (null or "windows" or "aac" or "aac_eld") ||
                        snap.PlaybackState is not ("windows" or "unavailable" or "disconnected" or "starting" or "ready" or "stopping" or "error") ||
                        snap.PlaybackAacBitrate is null || !snap.PlaybackAacBitrate.IsValidFor("aac") ||
                        snap.PlaybackAacEldBitrate is null || !snap.PlaybackAacEldBitrate.IsValidFor("aac_eld") ||
                        snap.PlaybackLink is { IsValid: false } ||
                        snap.PlaybackBitrateKbps is < 96 or > 512 ||
                        snap.HeadTrackingState is not ("idle" or "starting" or "streaming" or "error" or "waiting_for_ear") ||
                        snap.AdaptiveStrengthRequested is > 100 || snap.ConversationVolume > 100 ||
                        snap.Transparency is { IsValid: false } || snap.HearingAidSettings is { IsValid: false } ||
                        snap.HardwareDiagnostics is { IsValid: false } || snap.HearingMetadata is { IsValid: false } ||
                        snap.HardwareCheck is { IsValid: false } ||
                        snap.Eq is { IsValid: false } || snap.EqPending is { IsValid: false }) return null;
                    return new DaemonEvent.State(snap);

                case "overlay":
                    return new DaemonEvent.Overlay(
                        GetStr(root, "title"),
                        GetStr(root, "body"));

                case "connect_prompt":
                    return new DaemonEvent.ConnectPrompt(GetStr(root, "name"));

                case "pairing_nearby":
                    if (!root.TryGetProperty("model", out var model) || model.ValueKind != JsonValueKind.Number ||
                        !model.TryGetUInt16(out var modelId) || !root.TryGetProperty("lid_open", out var lidOpen) ||
                        lidOpen.ValueKind is not (JsonValueKind.True or JsonValueKind.False)) return null;
                    return new DaemonEvent.PairingNearby(modelId, lidOpen.GetBoolean());

                case "paired_lid":
                    if (!root.TryGetProperty("device_id", out var deviceId) || deviceId.ValueKind != JsonValueKind.String ||
                        deviceId.GetString() is not { Length: 12 } pairedId ||
                        pairedId.Any(c => c is not (>= '0' and <= '9' or >= 'a' and <= 'f')) ||
                        !root.TryGetProperty("lid_open", out var pairedLidOpen) ||
                        pairedLidOpen.ValueKind is not (JsonValueKind.True or JsonValueKind.False) ||
                        !root.TryGetProperty("sequence", out var sequence) || sequence.ValueKind != JsonValueKind.Number ||
                        !sequence.TryGetUInt64(out var number) || number == 0) return null;
                    return new DaemonEvent.PairedLid(pairedId, pairedLidOpen.GetBoolean(), number);

                case "hearing_test_mode_probe":
                    if (!root.TryGetProperty("request_id", out var probeRequest) || probeRequest.ValueKind != JsonValueKind.Number || !probeRequest.TryGetUInt64(out var probeId) ||
                        !root.TryGetProperty("device_id", out var probeTarget) || probeTarget.ValueKind != JsonValueKind.String ||
                        probeTarget.GetString() is not { Length: 12 } probeDevice || !probeDevice.All(Uri.IsHexDigit) ||
                        !root.TryGetProperty("enable_check", out var enableCheck) || enableCheck.ValueKind is not (JsonValueKind.True or JsonValueKind.False) ||
                        !root.TryGetProperty("result", out var probeResult) || probeResult.ValueKind != JsonValueKind.Object ||
                        probeResult.Deserialize<HearingTestModeProbeResult>(Json) is not { IsValid: true } parsedProbe) return null;
                    return new DaemonEvent.HearingTestModeProbe(probeId, probeDevice, enableCheck.GetBoolean(), parsedProbe);

                case "command_result":
                    if (!root.TryGetProperty("ok", out var ok) || ok.ValueKind is not (JsonValueKind.True or JsonValueKind.False)) return null;
                    if (!root.TryGetProperty("request_id", out var requestId) || requestId.ValueKind != JsonValueKind.Number || !requestId.TryGetUInt64(out var id)) return null;
                    return new DaemonEvent.CommandResult(GetStr(root, "command"), id, ok.GetBoolean(), GetStr(root, "message"));

                case "head_tracking":
                    if (!root.TryGetProperty("timestamp_ms", out var time) || time.ValueKind != JsonValueKind.Number || !time.TryGetUInt64(out var stamp) ||
                        !root.TryGetProperty("horizontal", out var horizontal) || horizontal.ValueKind != JsonValueKind.Number || !horizontal.TryGetInt16(out var h) ||
                        !root.TryGetProperty("vertical", out var vertical) || vertical.ValueKind != JsonValueKind.Number || !vertical.TryGetInt16(out var v)) return null;
                    return new DaemonEvent.HeadTracking(stamp, h, v);

                default:
                    return null;
            }
        }
        catch (JsonException)
        {
            return null;
        }
    }

    private static string GetStr(JsonElement el, string name)
        => el.TryGetProperty(name, out var p) && p.ValueKind == JsonValueKind.String
            ? p.GetString() ?? ""
            : "";
}
