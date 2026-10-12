using System.Text.Json;
using System.Text.Json.Serialization;

namespace NTPods.WinUI.Ipc;

// The IPC contract mirrors windows/ipc/src/lib.rs (serde, snake_case, tagged by
// `event` / `cmd`). Every field is spelled with an explicit [JsonPropertyName] so
// the wire names never depend on a naming policy.

/// Battery levels (percent), each optional — a packet may carry only some.
public sealed class Battery
{
    [JsonPropertyName("left")] public byte? Left { get; set; }
    [JsonPropertyName("right")] public byte? Right { get; set; }
    [JsonPropertyName("case")] public byte? Case { get; set; }
    [JsonPropertyName("headphone")] public byte? Headphone { get; set; }

    // Per-component charging flag (mirrors the daemon's ipc::Battery).
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

/// The daemon's authoritative state, pushed on connect and on every change.
public sealed class Snapshot
{
    [JsonPropertyName("device_id")] public string DeviceId { get; set; } = "";
    [JsonPropertyName("connected")] public bool Connected { get; set; }

    /// The session is nominally up but the AAP channel has gone silent: the AirPods
    /// are attached to Windows yet not talking to the daemon, so the hi-res mic and
    /// live notifications are dead. Surfaces "Repair connection" -- Connected alone
    /// stays true forever in that state and used to hide the button exactly then.
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
    [JsonPropertyName("conversational_awareness")] public bool ConversationalAwareness { get; set; }
    [JsonPropertyName("conversational_awareness_pending")] public bool? ConversationalAwarenessPending { get; set; }
    [JsonPropertyName("conversation_volume")] public byte ConversationVolume { get; set; } = 25;
    [JsonPropertyName("adaptive_volume")] public bool AdaptiveVolume { get; set; }
    [JsonPropertyName("loud_sound_reduction")] public bool? LoudSoundReduction { get; set; }
    [JsonPropertyName("loud_sound_reduction_pending")] public bool LoudSoundReductionPending { get; set; }
    [JsonPropertyName("loud_sound_reduction_error")] public string? LoudSoundReductionError { get; set; }
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

    // Device metadata from the 0x1D packet (empty until it arrives). Serial is
    // sensitive — the Settings page keeps these hidden behind a reveal toggle.
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
        "watch" => "\uE916",   // Stopwatch-ish; closest stock glyph for a watch
        "audio" => "\uE7F6",   // Headphone
        _ => "\uE783",         // Error/unknown
    };

    /// "This PC" is shown plainly; anything else carries its address so an
    /// unidentified device is still recognisable.
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
/// (Named ControlId to avoid clashing with Microsoft.UI.Xaml.Controls.Control.)
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

public sealed class SetLoudSoundReductionCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_loud_sound_reduction";
    [JsonPropertyName("on")] public bool On { get; init; }
}

public sealed class SetHearingAidCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "set_hearing_aid";
    [JsonPropertyName("on")] public bool On { get; init; }
    [JsonPropertyName("left_eq")] public float[] LeftEq { get; init; } = new float[8];
    [JsonPropertyName("right_eq")] public float[] RightEq { get; init; } = new float[8];
    [JsonPropertyName("amplification")] public float Amplification { get; init; }
    [JsonPropertyName("balance")] public float Balance { get; init; }
    [JsonPropertyName("tone")] public float Tone { get; init; }
    [JsonPropertyName("conversation_boost")] public bool ConversationBoost { get; init; }
    [JsonPropertyName("ambient_noise_reduction")] public float AmbientNoiseReduction { get; init; }
    [JsonPropertyName("own_voice")] public float OwnVoice { get; init; }
}

public sealed class ConnectCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "connect";
}

public sealed class DisconnectCmd
{
    [JsonPropertyName("cmd")] public string Cmd => "disconnect";
}

/// Toggle the AirPods' audio services off and on -- what Disconnect + Connect does
/// by hand -- WITHOUT touching the AAP session. For the failure where the sound
/// stops while the session, the Bluetooth device and the endpoint all stay healthy,
/// which nothing can detect automatically.
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

    /// Device nearby (BLE) but not connected — show a clickable "Connect?" card.
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
                    // The snapshot is flattened alongside the tag (serde
                    // `Event::State(Snapshot)` with `#[serde(tag = "event")]`
                    // inlines the struct's fields), so deserialize the whole
                    // object into a Snapshot; the extra "event" key is ignored.
                    var snap = root.Deserialize<Snapshot>(Json) ?? new Snapshot();
                    if (snap.Battery is null || snap.BatterySource is null || snap.BatteryFreshness is { IsValid: false } ||
                        snap.DeviceControls is null || snap.DeviceControlsPending is null ||
                        snap.MicMode is not ("automatic" or "keep_active" or "manual") || snap.MicStopDelaySecs > 3600 ||
                        snap.HeadTrackingState is not ("idle" or "starting" or "streaming" or "error" or "waiting_for_ear") ||
                        snap.AdaptiveStrengthRequested is > 100 || snap.ConversationVolume > 100 ||
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
