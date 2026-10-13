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
        InRange(LeftAmplification, -1, 1) && InRange(RightAmplification, -1, 1) &&
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

/// The daemon's authoritative state, pushed on connect and on every change.
public sealed class Snapshot
{
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
    [JsonPropertyName("playback_error")] public string? PlaybackError { get; set; }
    [JsonPropertyName("playback_available")] public bool PlaybackAvailable { get; set; }
    [JsonPropertyName("playback_native_driver")] public bool PlaybackNativeDriver { get; set; }
    [JsonPropertyName("playback_aac_bitrate")] public PlaybackBitrate PlaybackAacBitrate { get; set; } = new();
    [JsonPropertyName("playback_aac_eld_bitrate")] public PlaybackBitrate PlaybackAacEldBitrate { get; set; } = new();
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
    [JsonPropertyName("hearing_aid_settings")] public HearingAidSettings? HearingAidSettings { get; set; }
    [JsonPropertyName("hearing_aid_editable")] public bool HearingAidEditable { get; set; }
    [JsonPropertyName("hearing_aid_pending")] public bool HearingAidPending { get; set; }
    [JsonPropertyName("hearing_aid_error")] public string? HearingAidError { get; set; }
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
    [JsonPropertyName("device_controls_pending")] public Dictionary<byte, uint> DeviceControlsPending { get; set; } = new();
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
    [JsonPropertyName("settings")] public required TransparencySettings Settings { get; init; }
}

public sealed class SetLoudSoundReductionCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_loud_sound_reduction";
    [JsonPropertyName("on")] public bool On { get; init; }
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

public sealed class ConnectCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "connect";
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
    /// Full state, pushed on connect and whenever it changes.
    public sealed record State(Snapshot Snapshot) : DaemonEvent;

    /// A transient notification rendered in the island.
    public sealed record Overlay(string Title, string Body) : DaemonEvent;

    /// BLE proximity notification; the UI checks connection state before offering Connect.
    public sealed record ConnectPrompt(string Name) : DaemonEvent;
    public sealed record CommandResult(string Command, ulong RequestId, bool Ok, string Message) : DaemonEvent;
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
                case "state":
                    // Serde flattens Snapshot beside the event tag; deserialize the whole object.
                    var snap = root.Deserialize<Snapshot>(Json) ?? new Snapshot();
                    if (snap.Battery is null || snap.BatterySource is null || snap.BatteryFreshness is { IsValid: false } ||
                        snap.DeviceControls is null || snap.DeviceControlsPending is null ||
                        snap.MicMode is not ("automatic" or "keep_active" or "manual") || snap.MicStopDelaySecs > 3600 ||
                        snap.PlaybackMode is not ("windows" or "aac" or "aac_eld") ||
                        snap.PlaybackPreferredMode is not (null or "windows" or "aac" or "aac_eld") ||
                        snap.PlaybackState is not ("windows" or "unavailable" or "disconnected" or "starting" or "ready" or "stopping" or "error") ||
                        snap.PlaybackAacBitrate is null || !snap.PlaybackAacBitrate.IsValidFor("aac") ||
                        snap.PlaybackAacEldBitrate is null || !snap.PlaybackAacEldBitrate.IsValidFor("aac_eld") ||
                        snap.PlaybackBitrateKbps is < 96 or > 512 ||
                        snap.HeadTrackingState is not ("idle" or "starting" or "streaming" or "error" or "waiting_for_ear") ||
                        snap.AdaptiveStrengthRequested is > 100 || snap.ConversationVolume > 100 ||
                        snap.Transparency is { IsValid: false } || snap.HearingAidSettings is { IsValid: false } ||
                        snap.Eq is { IsValid: false } || snap.EqPending is { IsValid: false }) return null;
                    return new DaemonEvent.State(snap);

                case "overlay":
                    return new DaemonEvent.Overlay(
                        GetStr(root, "title"),
                        GetStr(root, "body"));

                case "connect_prompt":
                    return new DaemonEvent.ConnectPrompt(GetStr(root, "name"));

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
