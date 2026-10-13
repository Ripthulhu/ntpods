using System.Text.Json;
using System.Text.Json.Nodes;
using NTPods.WinUI.Ipc;

static void Check(bool condition, string message)
{
    if (!condition) throw new InvalidOperationException(message);
}

Check(args.Length == 2, "usage: IpcContract INPUT OUTPUT");
string[] phases = ["idle", "starting", "capturing", "stopping", "recovering", "unavailable", "error", "waiting_for_ear"];
var lines = File.ReadAllLines(args[0]);
Check(lines.Length == phases.Length + 1, "Expected each lifecycle fixture and one legacy fixture");
var output = new List<string>();
for (var index = 0; index < lines.Length; ++index)
{
    var evt = Wire.ParseEvent(lines[index]) as DaemonEvent.State;
    Check(evt is not null, $"Rust fixture {index} did not parse as a flattened state");
    var s = evt!.Snapshot;
    using var raw = JsonDocument.Parse(lines[index]);
    Check(s.PlaybackMode == (raw.RootElement.TryGetProperty("playback_mode", out var playbackMode) ? playbackMode.GetString() : "windows") &&
        s.PlaybackPreferredMode == (raw.RootElement.TryGetProperty("playback_preferred_mode", out var preferredMode) ? preferredMode.GetString() : null) &&
        s.PlaybackState == (raw.RootElement.TryGetProperty("playback_state", out var playbackState) ? playbackState.GetString() : "windows") &&
        s.PlaybackAvailable == (raw.RootElement.TryGetProperty("playback_available", out var playbackAvailable) && playbackAvailable.GetBoolean()) &&
        s.PlaybackNativeDriver == (raw.RootElement.TryGetProperty("playback_native_driver", out var nativeDriver) && nativeDriver.GetBoolean()) &&
        s.PlaybackError == (raw.RootElement.TryGetProperty("playback_error", out var playbackError) && playbackError.ValueKind != JsonValueKind.Null ? playbackError.GetString() : null),
        "Playback preference, active codec, route state and errors must remain separate across IPC.");
    foreach (var (key, bitrate) in new[] { ("playback_aac_bitrate", s.PlaybackAacBitrate), ("playback_aac_eld_bitrate", s.PlaybackAacEldBitrate) })
    {
        var expected = raw.RootElement.TryGetProperty(key, out var value) ? value.Deserialize<PlaybackBitrate>() : new PlaybackBitrate();
        Check(bitrate == expected, "Each codec must retain its own bitrate preference across IPC.");
    }
    Check(s.PlaybackBitrateKbps == (raw.RootElement.TryGetProperty("playback_bitrate_kbps", out var actualBitrate) && actualBitrate.ValueKind != JsonValueKind.Null
        ? actualBitrate.GetUInt32() : (uint?)null), "The active bitrate must remain separate from the saved preferences.");
    Check(s.PlaybackDelayReporting == (raw.RootElement.TryGetProperty("playback_delay_reporting", out var delayReporting) && delayReporting.GetBoolean()) &&
        s.PlaybackDelayTenthsMs == (raw.RootElement.TryGetProperty("playback_delay_tenths_ms", out var delay) && delay.ValueKind != JsonValueKind.Null
            ? delay.GetUInt16() : (ushort?)null) &&
        s.PlaybackBufferTargetMs == (raw.RootElement.TryGetProperty("playback_buffer_target_ms", out var bufferTarget) && bufferTarget.ValueKind != JsonValueKind.Null
            ? bufferTarget.GetUInt16() : (ushort?)null) &&
        s.PlaybackBufferError == (raw.RootElement.TryGetProperty("playback_buffer_error", out var bufferError) && bufferError.ValueKind != JsonValueKind.Null
            ? bufferError.GetString() : null),
        "Negotiated reporting, earbud-reported delay and requested buffer target must remain separate across IPC.");
    Check(s.BleScanWhileConnected == (raw.RootElement.TryGetProperty("ble_scan_while_connected", out var scanDuringPlayback) && scanDuringPlayback.GetBoolean()),
        "Playback battery scan preference changed across IPC");
    Check(s.BatteryLive == (raw.RootElement.TryGetProperty("battery_live", out var liveBattery) && liveBattery.GetBoolean()),
        "Battery freshness changed across IPC");
    Check(s.BatterySource == (raw.RootElement.TryGetProperty("battery_source", out var source) ? source.GetString() : ""),
        "Battery report source changed across IPC");
    Check(s.BatteryTimestampMs == (raw.RootElement.TryGetProperty("battery_timestamp_ms", out var timestamp) &&
        timestamp.ValueKind != JsonValueKind.Null ? timestamp.GetUInt64() : (ulong?)null), "Battery report time changed across IPC");
    Check(s.HeadTrackingEnabled == (raw.RootElement.TryGetProperty("head_tracking_enabled", out var tracking) && tracking.GetBoolean()) &&
        s.HeadTrackingState == (raw.RootElement.TryGetProperty("head_tracking_state", out var trackingState) ? trackingState.GetString() : "idle") &&
        s.HeadTrackingError == (raw.RootElement.TryGetProperty("head_tracking_error", out var trackingError) && trackingError.ValueKind != JsonValueKind.Null ? trackingError.GetString() : null),
        "Head tracking state must match Rust and default to idle for older snapshots.");
    Check(s.DevName == "AirPods fixture — \"test\"", "Unicode or string escaping changed");
    Check(s.HearingAidEnabled == (raw.RootElement.TryGetProperty("hearing_aid_enabled", out var hearing) &&
        hearing.ValueKind != JsonValueKind.Null ? hearing.GetBoolean() : (bool?)null) &&
        s.HearingAidRevision == (raw.RootElement.TryGetProperty("hearing_aid_revision", out var hearingRevision) ? hearingRevision.GetUInt64() : 0),
        "Reported hearing state and its revision must survive IPC; missing state stays unknown.");
    var expectedHearing = raw.RootElement.TryGetProperty("hearing_aid_settings", out var hearingSettings) && hearingSettings.ValueKind != JsonValueKind.Null
        ? hearingSettings.Deserialize<HearingAidSettings>() : null;
    Check((s.HearingAidSettings is null ? expectedHearing is null : s.HearingAidSettings.SameValues(expectedHearing)) &&
        s.HearingAidEditable == (raw.RootElement.TryGetProperty("hearing_aid_editable", out var hearingEditable) && hearingEditable.GetBoolean()) &&
        s.HearingAidPending == (raw.RootElement.TryGetProperty("hearing_aid_pending", out var hearingPending) && hearingPending.GetBoolean()) &&
        s.HearingAidError == (raw.RootElement.TryGetProperty("hearing_aid_error", out var hearingError) && hearingError.ValueKind != JsonValueKind.Null
            ? hearingError.GetString() : null),
        "Hearing curves, per-ear tuning, pending and error states must survive IPC without supplying defaults.");
    Check(s.LoudSoundReduction == (raw.RootElement.TryGetProperty("loud_sound_reduction", out var reduction) &&
        reduction.ValueKind != JsonValueKind.Null ? reduction.GetBoolean() : (bool?)null) &&
        s.LoudSoundReductionPending == (raw.RootElement.TryGetProperty("loud_sound_reduction_pending", out var reductionPending) && reductionPending.GetBoolean()) &&
        s.LoudSoundReductionError == (raw.RootElement.TryGetProperty("loud_sound_reduction_error", out var reductionError) &&
        reductionError.ValueKind != JsonValueKind.Null ? reductionError.GetString() : null),
        "Confirmed, pending and failed Loud Sound Reduction states must remain separate across IPC.");
    Check(s.Transparency == (raw.RootElement.TryGetProperty("transparency", out var transparency) && transparency.ValueKind != JsonValueKind.Null
            ? transparency.Deserialize<TransparencySettings>() : null) &&
        s.TransparencyPending == (raw.RootElement.TryGetProperty("transparency_pending", out var transparencyPending) && transparencyPending.GetBoolean()) &&
        s.TransparencyError == (raw.RootElement.TryGetProperty("transparency_error", out var transparencyError) && transparencyError.ValueKind != JsonValueKind.Null
            ? transparencyError.GetString() : null),
        "Transparency readback, pending and error states must remain separate across IPC.");
    if (index < phases.Length)
    {
        var phase = phases[index];
        Check(!s.PlaybackAacBitrate.Variable && s.PlaybackAacEldBitrate.Variable == (phase == "capturing"),
            "Variable bitrate must stay attached to the AAC-ELD preference.");
        Check(s.Transparency == (phase == "unavailable" ? null : Transparency(phase == "capturing")) &&
            s.TransparencyPending == (phase == "starting") &&
            s.TransparencyError == (phase == "error" ? "Transparency settings could not be read." : null),
            "Per-ear Transparency values changed across IPC.");
        Check(s.BatteryFreshness is { } ages && ages.Left.TimestampMs == 1_792_000_000_100 &&
            ages.Right.TimestampMs == 1_792_000_000_090 && ages.Case.TimestampMs == 1_791_999_900_000 &&
            ages.Left.Live == (phase != "error") && ages.Right.Live == (phase != "error") &&
            !ages.Case.Live && !ages.Headphone.Live && ages.Headphone.TimestampMs is null,
            "Independent battery report ages and freshness changed across IPC");
        Check(s.BatteryFreshness is { Left.Present: true, Right.Present: true, Case.Present: false, Headphone.Present: null },
            "Explicit component presence must remain separate from cached battery readings across IPC.");
        Check(s.MicState == phase, $"Lifecycle state changed: {phase}");
        Check(s.MicCaptureActive == (phase is "starting" or "capturing" or "error" or "waiting_for_ear"), $"Capture-client flag wrong: {phase}");
        Check(s.MicUplinkActive == (phase is "starting" or "capturing" or "stopping" or "recovering"), $"Physical-uplink flag wrong: {phase}");
        Check(s.MicRecording == (phase == "capturing"), $"Recording flag wrong: {phase}");
        Check(s.MicAvailable == (phase != "unavailable"), $"Availability wrong: {phase}");
        Check(s.AutoMode == (phase is not "unavailable" and not "error"), $"Permission wrong: {phase}");
        Check(s.MicMode == (phase == "capturing" ? "keep_active" : phase == "waiting_for_ear" ? "manual" : "automatic"), $"Microphone policy changed: {phase}");
        Check(s.MicStopDelaySecs == (phase == "starting" ? 30u : 0u), $"Microphone stop delay changed: {phase}");
        Check(s.MicManualActive == (phase == "waiting_for_ear"), $"Manual microphone demand changed: {phase}");
        Check(s.ConversationalAwarenessPending == (phase == "starting" ? true : phase == "stopping" ? false : (bool?)null),
            $"Pending setting changed: {phase}");
        Check(s.MicError == (phase == "error" ? "Output failed: \"device gone\"\nTry again — later." : null), $"Error text changed: {phase}");
        Check(s.DeviceControls.Count == 4 && s.DeviceControls[1] == 0 && s.DeviceControls[31] == 0x504B &&
            s.DeviceControls[36] == 0x0300 && s.DeviceControls[40] == 2, $"Four-byte device controls changed: {phase}");
        Check(s.DeviceControlsPending.Count == (phase == "starting" ? 2 : 0), $"Pending controls changed: {phase}");
        uint? requestedStrength = phase switch { "idle" => 0u, "starting" => 50u, "capturing" => 100u, _ => null };
        Check(s.AdaptiveStrengthRequested == requestedStrength, $"Sent strength changed: {phase}");
        Check(s.AllowOffRequested == (phase == "idle" ? true : phase == "starting" ? false : (bool?)null),
            $"Sent Allow Off choice changed: {phase}");
        if (phase == "starting") Check(s.DeviceControlsPending[35] == 3 && s.DeviceControlsPending[40] == 1, "Pending values changed");
        Check(s.Eq == (phase == "unavailable" ? null : new Equalizer { Mode = 2, Low = 35, Mid = 50, High = 65 }), $"EQ readback changed: {phase}");
        Check(s.EqPending == (phase == "starting" ? new Equalizer { Mode = 2, Low = 40, Mid = 55, High = 70 } : null), $"Pending EQ changed: {phase}");
        Check(s.AutoPause == (phase != "stopping"), $"Auto-pause changed: {phase}");
        byte conversationVolume = phase switch { "idle" => 0, "starting" => 25, "capturing" => 100, _ => 60 };
        Check(s.ConversationVolume == conversationVolume, $"Conversation volume changed: {phase}");
        Check(s.HeartRateEnabled == (phase == "capturing"), $"Heart-rate enabled state changed: {phase}");
        Check(s.HeartRate == (phase == "capturing" ? (ushort?)72 : null), $"Heart-rate value changed: {phase}");
        Check(s.HeartRateTimestampMs == (phase == "capturing" ? (ulong?)1_792_000_000_123 : null), $"Heart-rate sample timestamp changed: {phase}");
        Check(s.AncUnconfirmed == (phase is "starting" or "error"), $"Unconfirmed noise mode changed: {phase}");
        Check(s.DeviceSettingsError == (phase == "error" ? "AirPods did not confirm the setting." : null), $"Settings error changed: {phase}");
    }
    else
    {
        Check(!s.BatteryLive && s.BatteryTimestampMs is null && s.BatterySource == "", "Legacy snapshots must not invent report metadata.");
        Check(s.BatteryFreshness is null, "Legacy snapshots must not invent per-component report metadata.");
        Check(!s.BleScanWhileConnected, "Legacy snapshots must keep playback BLE scanning disabled.");
        Check(s.PlaybackPreferredMode is null && s.PlaybackMode == "windows" && s.PlaybackState == "windows" && !s.PlaybackAvailable && !s.PlaybackNativeDriver && s.PlaybackError is null,
            "Legacy snapshots must keep Windows playback and must not claim native playback support.");
        Check(s.PlaybackAacBitrate == new PlaybackBitrate() && s.PlaybackAacEldBitrate == new PlaybackBitrate() && s.PlaybackBitrateKbps is null,
            "Missing bitrate preferences default to Best effort without inventing a current bitrate.");
        Check(!s.PlaybackDelayReporting && s.PlaybackDelayTenthsMs is null && s.PlaybackBufferTargetMs is null && s.PlaybackBufferError is null,
            "Older snapshots must leave playback delay and buffer targets unknown.");
        Check(s.Transparency is null && !s.TransparencyPending && s.TransparencyError is null,
            "Older snapshots must leave Transparency settings unknown.");
        Check(s.HearingAidEnabled is null && s.HearingAidRevision == 0,
            "Older snapshots must not manufacture a hearing-assistance report.");
        Check(s.HearingAidSettings is null && !s.HearingAidEditable && !s.HearingAidPending && s.HearingAidError is null,
            "Older snapshots must leave the saved audiogram and tuning unknown.");
        Check(s.MicRecording, "Legacy fixture must exercise an old recording=true state");
        Check(s.ConversationalAwarenessPending is null, "Legacy fixture cannot claim a pending setting");
        Check(s.AdaptiveStrengthRequested is null, "Legacy fixture cannot claim a sent strength");
        Check(s.AllowOffRequested is null, "Legacy fixture cannot claim an Allow Off request");
        Check(s.LoudSoundReduction is null && !s.LoudSoundReductionPending && s.LoudSoundReductionError is null,
            "Legacy snapshots must not invent hearing-protection readback or a pending request.");
        Check(s.ConversationVolume == 25, "Legacy conversation volume must keep the original 25% limit");
        Check(!s.HeartRateEnabled && s.HeartRateTimestampMs is null, "A legacy BPM must not imply active or fresh monitoring.");
        Check(s.MicMode == "automatic" && s.MicStopDelaySecs == 0 && !s.MicManualActive, "A legacy snapshot must not enable a new microphone policy.");
        Check(!s.MicCaptureActive && !s.MicUplinkActive && s.MicState == "unavailable" && s.MicError is null,
            "Missing lifecycle fields must not infer capture permission or uplink state");
        Check(s.AutoPause && !s.AncUnconfirmed && s.DeviceControls.Count == 0 && s.DeviceControlsPending.Count == 0 &&
            s.Eq is null && s.EqPending is null && s.DeviceSettingsError is null, "Legacy device settings must remain unknown");
    }
    // Serialize the actual C# Snapshot, then add the same flattened event tag.
    var json = JsonSerializer.SerializeToNode(s, Wire.Json)!.AsObject();
    json["event"] = "state";
    output.Add(json.ToJsonString());
}

string[] malformed = ["", " ", "{", "null", "true", "12", "\"text\"", "[]", "[{}]", "{}",
    "{\"event\":\"state\",\"playback_mode\":null}",
    "{\"event\":\"state\",\"playback_mode\":\"unknown\"}",
    "{\"event\":\"state\",\"playback_mode\":1}",
    "{\"event\":\"state\",\"playback_preferred_mode\":\"unknown\"}",
    "{\"event\":\"state\",\"playback_preferred_mode\":\"\"}",
    "{\"event\":\"state\",\"playback_preferred_mode\":1}",
    "{\"event\":\"state\",\"playback_state\":null}",
    "{\"event\":\"state\",\"playback_state\":\"playing\"}",
    "{\"event\":\"state\",\"playback_available\":1}",
    "{\"event\":\"state\",\"playback_available\":null}",
    "{\"event\":\"state\",\"playback_native_driver\":null}",
    "{\"event\":\"state\",\"playback_native_driver\":1}",
    "{\"event\":\"state\",\"playback_error\":false}",
    "{\"event\":\"state\",\"playback_aac_bitrate\":null}",
    "{\"event\":\"state\",\"playback_aac_eld_bitrate\":null}",
    "{\"event\":\"state\",\"playback_aac_bitrate\":{}}",
    "{\"event\":\"state\",\"playback_aac_eld_bitrate\":{}}",
    "{\"event\":\"state\",\"playback_aac_bitrate\":{\"best_effort\":true}}",
    "{\"event\":\"state\",\"playback_aac_eld_bitrate\":{\"best_effort\":true}}",
    "{\"event\":\"state\",\"playback_aac_bitrate\":{\"kbps\":256}}",
    "{\"event\":\"state\",\"playback_aac_eld_bitrate\":{\"kbps\":256}}",
    "{\"event\":\"state\",\"playback_aac_bitrate\":{\"best_effort\":null,\"kbps\":256}}",
    "{\"event\":\"state\",\"playback_aac_bitrate\":{\"best_effort\":1,\"kbps\":256}}",
    "{\"event\":\"state\",\"playback_aac_bitrate\":{\"best_effort\":true,\"kbps\":95}}",
    "{\"event\":\"state\",\"playback_aac_bitrate\":{\"best_effort\":true,\"kbps\":513}}",
    "{\"event\":\"state\",\"playback_aac_eld_bitrate\":{\"best_effort\":true,\"kbps\":127}}",
    "{\"event\":\"state\",\"playback_aac_eld_bitrate\":{\"best_effort\":true,\"kbps\":257}}",
    "{\"event\":\"state\",\"playback_aac_eld_bitrate\":{\"best_effort\":true,\"kbps\":128.5}}",
    "{\"event\":\"state\",\"playback_aac_bitrate\":{\"best_effort\":true,\"kbps\":null}}",
    "{\"event\":\"state\",\"playback_bitrate_kbps\":0}",
    "{\"event\":\"state\",\"playback_bitrate_kbps\":513}",
    "{\"event\":\"state\",\"loud_sound_reduction\":1}",
    "{\"event\":\"state\",\"loud_sound_reduction\":\"true\"}",
    "{\"event\":\"state\",\"loud_sound_reduction_pending\":null}",
    "{\"event\":\"state\",\"loud_sound_reduction_pending\":1}",
    "{\"event\":\"state\",\"loud_sound_reduction_pending\":\"false\"}",
    "{\"event\":\"state\",\"loud_sound_reduction_error\":1}",
    "{\"event\":\"state\",\"loud_sound_reduction_error\":{}}",
    "{\"event\":\"state\",\"head_tracking_enabled\":1}",
    "{\"event\":\"state\",\"head_tracking_state\":null}",
    "{\"event\":\"state\",\"head_tracking_state\":\"unknown\"}",
    "{\"event\":\"state\",\"head_tracking_error\":1}",
    "{\"event\":\"head_tracking\"}",
    "{\"event\":\"head_tracking\",\"timestamp_ms\":1,\"horizontal\":1}",
    "{\"event\":\"head_tracking\",\"timestamp_ms\":-1,\"horizontal\":1,\"vertical\":2}",
    "{\"event\":\"head_tracking\",\"timestamp_ms\":\"1\",\"horizontal\":1,\"vertical\":2}",
    "{\"event\":\"head_tracking\",\"timestamp_ms\":1,\"horizontal\":32768,\"vertical\":2}",
    "{\"event\":\"head_tracking\",\"timestamp_ms\":1,\"horizontal\":1,\"vertical\":-32769}",
    "{\"event\":\"head_tracking\",\"timestamp_ms\":1,\"horizontal\":1.5,\"vertical\":2}",
    "{\"event\":\"head_tracking\",\"timestamp_ms\":1,\"horizontal\":1,\"vertical\":null}",
    "{\"event\":\"state\",\"ble_scan_while_connected\":1}",
    "{\"event\":\"state\",\"ble_scan_while_connected\":\"true\"}",
    "{\"event\":\"state\",\"ble_scan_while_connected\":null}",
    "{\"event\":\"state\",\"battery\":null}",
    "{\"event\":\"state\",\"battery_freshness\":42}",
    "{\"event\":\"state\",\"battery_freshness\":{}}",
    "{\"event\":\"state\",\"battery_freshness\":{\"left\":null,\"right\":{\"live\":false},\"case\":{\"live\":false},\"headphone\":{\"live\":false}}}",
    "{\"event\":\"state\",\"battery_freshness\":{\"left\":{\"timestamp_ms\":-1,\"live\":true},\"right\":{\"live\":false},\"case\":{\"live\":false},\"headphone\":{\"live\":false}}}",
    "{\"event\":\"state\",\"battery_freshness\":{\"left\":{\"live\":1},\"right\":{\"live\":false},\"case\":{\"live\":false},\"headphone\":{\"live\":false}}}",
    "{\"event\":\"state\",\"battery_source\":null}",
    "{\"event\":\"state\",\"battery_source\":42}",
    "{\"event\":\"state\",\"battery_live\":1}",
    "{\"event\":\"state\",\"battery_live\":\"true\"}",
    "{\"event\":\"state\",\"battery_timestamp_ms\":-1}",
    "{\"event\":\"state\",\"battery_timestamp_ms\":1.5}",
    "{\"event\":\"state\",\"battery_timestamp_ms\":\"1000\"}",
    "{\"event\":null}", "{\"event\":42}", "{\"event\":false}", "{\"event\":{}}", "{\"event\":[]}",
    "{\"event\":\"unknown\"}", "{\"event\":\"state\",\"mic_capture_active\":\"true\"}",
    "{\"event\":\"state\",\"mic_uplink_active\":1}", "{\"event\":\"state\",\"mic_state\":false}",
    "{\"event\":\"state\",\"mic_error\":[]}",
    "{\"event\":\"state\",\"conversational_awareness_pending\":1}",
    "{\"event\":\"state\",\"allow_off_requested\":1}",
    "{\"event\":\"state\",\"allow_off_requested\":\"true\"}",
    "{\"event\":\"state\",\"device_controls\":null}",
    "{\"event\":\"state\",\"device_controls_pending\":null}",
    "{\"event\":\"state\",\"adaptive_strength_requested\":101}",
    "{\"event\":\"state\",\"adaptive_strength_requested\":-1}",
    "{\"event\":\"state\",\"adaptive_strength_requested\":1.5}",
    "{\"event\":\"state\",\"adaptive_strength_requested\":true}",
    "{\"event\":\"state\",\"adaptive_strength_requested\":\"50\"}",
    "{\"event\":\"state\",\"device_controls\":{\"256\":1}}",
    "{\"event\":\"state\",\"device_controls\":{\"1\":-1}}",
    "{\"event\":\"state\",\"eq\":{\"mode\":2,\"low\":101,\"mid\":50,\"high\":50}}",
    "{\"event\":\"state\",\"eq\":{\"mode\":3,\"low\":50,\"mid\":50,\"high\":50}}",
    "{\"event\":\"state\",\"eq_pending\":{\"mode\":0}}",
    "{\"event\":\"state\",\"eq\":{\"mode\":2}}",
    "{\"event\":\"state\",\"auto_pause\":1}",
    "{\"event\":\"state\",\"conversation_volume\":101}",
    "{\"event\":\"state\",\"conversation_volume\":-1}",
    "{\"event\":\"state\",\"conversation_volume\":1.5}",
    "{\"event\":\"state\",\"conversation_volume\":true}",
    "{\"event\":\"state\",\"conversation_volume\":null}",
    "{\"event\":\"state\",\"conversation_volume\":\"25\"}",
    "{\"event\":\"state\",\"anc_unconfirmed\":1}",
    "{\"event\":\"state\",\"heart_rate_enabled\":1}",
    "{\"event\":\"state\",\"heart_rate_timestamp_ms\":-1}",
    "{\"event\":\"state\",\"mic_mode\":\"always_record\"}",
    "{\"event\":\"state\",\"mic_mode\":null}",
    "{\"event\":\"state\",\"mic_stop_delay_secs\":3601}",
    "{\"event\":\"state\",\"mic_manual_active\":1}",
    "{\"event\":\"command_result\",\"command\":\"set_name\",\"ok\":true,\"request_id\":\"42\"}",
    "{\"event\":\"command_result\",\"command\":\"set_name\",\"ok\":1,\"request_id\":42}"];
foreach (var json in malformed)
    Check(Wire.ParseEvent(json) is null, $"Malformed event must be ignored without throwing: {json}");

foreach (string key in new[] { "playback_delay_tenths_ms", "playback_buffer_target_ms" })
    foreach (string value in new[] { "-1", "65536", "1.5", "true", "\"60\"" })
        Check(Wire.ParseEvent($"{{\"event\":\"state\",\"{key}\":{value}}}") is null,
            "Delay and buffer values must fit an unsigned 16-bit integer or be null.");
foreach (string value in new[] { "null", "1", "\"true\"" })
    Check(Wire.ParseEvent($"{{\"event\":\"state\",\"playback_delay_reporting\":{value}}}") is null,
        "Delay reporting support must be a boolean.");
foreach (string value in new[] { "1", "false", "{}" })
    Check(Wire.ParseEvent($"{{\"event\":\"state\",\"playback_buffer_error\":{value}}}") is null,
        "A buffer error must be text or null.");

foreach (var (mode, bestEffort, kbps) in new[] { ("aac", true, 256u), ("aac_eld", false, 256u), ("aac_eld", true, 192u) })
{
    string json = JsonSerializer.Serialize(new Dictionary<string, object> {
        ["event"] = "state", [$"playback_{mode}_bitrate"] = new PlaybackBitrate { BestEffort = bestEffort, Kbps = kbps, Variable = true }
    });
    Check(Wire.ParseEvent(json) is null, "Unsupported variable bitrate combinations must be rejected.");
}
foreach (string value in new[] { "null", "1", "\"true\"" })
    Check(Wire.ParseEvent("{\"event\":\"state\",\"playback_aac_eld_bitrate\":{\"best_effort\":true,\"kbps\":256,\"variable\":" + value + "}}") is null,
        "Variable bitrate must be a boolean.");
Check(Wire.ParseEvent("{\"event\":\"state\",\"playback_aac_eld_bitrate\":{\"best_effort\":true,\"kbps\":256}}")
    is DaemonEvent.State { Snapshot.PlaybackAacEldBitrate.Variable: false }, "Older bitrate preferences must remain constant bitrate.");

var transparencyState = new JsonObject { ["event"] = "state", ["transparency"] = JsonSerializer.SerializeToNode(Transparency(true)) };
foreach (string key in transparencyState["transparency"]!.AsObject().Select(pair => pair.Key).ToArray())
{
    var missing = transparencyState.DeepClone();
    missing["transparency"]!.AsObject().Remove(key);
    Check(Wire.ParseEvent(missing.ToJsonString()) is null, "Incomplete Transparency settings must be rejected.");
}
foreach (var (key, value) in new[] {
    ("left_amplification", -1.01f), ("right_amplification", 1.01f), ("left_tone", -1.01f), ("right_tone", 1.01f),
    ("left_ambient_noise_reduction", -0.01f), ("right_ambient_noise_reduction", 1.01f) })
{
    var invalid = transparencyState.DeepClone();
    invalid["transparency"]![key] = value;
    Check(Wire.ParseEvent(invalid.ToJsonString()) is null, "Out-of-range Transparency settings must be rejected.");
}
foreach (string invalidValue in new[] { "{}", "[]", "1", "true", "\"settings\"" })
    Check(Wire.ParseEvent("{\"event\":\"state\",\"transparency\":" + invalidValue + "}") is null,
        "Malformed Transparency settings must be rejected.");
foreach (string key in new[] { "transparency_pending", "transparency_error" })
    Check(Wire.ParseEvent("{\"event\":\"state\",\"" + key + "\":1}") is null, "Malformed Transparency state must be rejected.");

string[] malformedPresence = ["0", "1", "\"true\"", "[]"];
var hearingState = new JsonObject { ["event"] = "state", ["hearing_aid_settings"] = JsonSerializer.SerializeToNode(Hearing()) };
Check(Wire.ParseEvent(hearingState.ToJsonString()) is DaemonEvent.State { Snapshot.HearingAidSettings: { } hearingRoundtrip } &&
    hearingRoundtrip.SameValues(Hearing()), "Asymmetric hearing curves and tuning must survive C# parsing.");
Check(Wire.ParseEvent(hearingState.ToJsonString()) is DaemonEvent.State { Snapshot.HearingAidEditable: false },
    "An older daemon's hearing settings must remain read-only without explicit editability.");
foreach (bool editable in new[] { false, true })
{
    var state = hearingState.DeepClone();
    state["hearing_aid_editable"] = editable;
    Check(Wire.ParseEvent(state.ToJsonString()) is DaemonEvent.State parsed && parsed.Snapshot.HearingAidEditable == editable,
        "Explicit hearing editability must survive C# parsing.");
}
foreach (string key in hearingState["hearing_aid_settings"]!.AsObject().Select(pair => pair.Key).ToArray())
{
    var missing = hearingState.DeepClone();
    missing["hearing_aid_settings"]!.AsObject().Remove(key);
    Check(Wire.ParseEvent(missing.ToJsonString()) is null, $"Missing hearing field {key} must be rejected.");
    var nullValue = hearingState.DeepClone();
    nullValue["hearing_aid_settings"]![key] = null;
    Check(Wire.ParseEvent(nullValue.ToJsonString()) is null, $"Null hearing field {key} must be rejected.");
}
foreach (var (key, value) in new[] {
    ("left_amplification", -1.01f), ("right_amplification", 1.01f), ("left_tone", -1.01f), ("right_tone", 1.01f),
    ("left_ambient_noise_reduction", -0.01f), ("right_ambient_noise_reduction", 1.01f), ("own_voice", 1.01f) })
{
    var invalid = hearingState.DeepClone();
    invalid["hearing_aid_settings"]![key] = value;
    Check(Wire.ParseEvent(invalid.ToJsonString()) is null, "Out-of-range hearing settings must be rejected.");
}
foreach (string key in new[] { "left_eq", "right_eq" })
foreach (string curve in new[] { "[]", "[1,2,3,4,5,6,7]", "[1,2,3,4,5,6,7,8,9]", "[0,1,2,3,4,5,6,-1]", "[0,1,2,3,4,5,6,91]", "[0,1,2,3,4,5,6,1e40]", "[0,1,2,3,4,5,6,null]", "[0,1,2,3,4,5,6,\"NaN\"]" })
{
    var invalid = hearingState.DeepClone();
    invalid["hearing_aid_settings"]![key] = JsonNode.Parse(curve);
    Check(Wire.ParseEvent(invalid.ToJsonString()) is null, "Hearing curves must contain exactly eight finite thresholds in range.");
}
foreach (string key in new[] { "hearing_aid_pending", "hearing_aid_editable", "hearing_aid_error", "hearing_aid_settings" })
    Check(Wire.ParseEvent("{\"event\":\"state\",\"" + key + "\":1}") is null, "Malformed hearing readback state must be rejected.");
foreach (string present in malformedPresence)
{
    string json = "{\"event\":\"state\",\"battery_freshness\":{\"left\":{\"live\":true,\"present\":" + present +
        "},\"right\":{\"live\":false},\"case\":{\"live\":false},\"headphone\":{\"live\":false}}}";
    Check(Wire.ParseEvent(json) is null, "Malformed component presence must not reach the UI.");
}
Check(Wire.ParseEvent("{\"event\":\"state\",\"battery_freshness\":{\"left\":{\"live\":true},\"right\":{\"live\":false},\"case\":{\"live\":false},\"headphone\":{\"live\":false}}}")
    is DaemonEvent.State { Snapshot.BatteryFreshness.Left.Present: null },
    "Older snapshots without component presence must remain compatible.");

Check(Wire.ParseEvent("{\"event\":\"overlay\",\"title\":\"a\",\"body\":\"b\"}") is DaemonEvent.Overlay("a", "b"), "Overlay dispatch changed");
Check(Wire.ParseEvent("{\"event\":\"connect_prompt\",\"name\":\"buds\"}") is DaemonEvent.ConnectPrompt("buds"), "Connect prompt dispatch changed");
Check(Wire.ParseEvent("{\"event\":\"head_tracking\",\"timestamp_ms\":1792000000100,\"horizontal\":-32768,\"vertical\":32767}")
    is DaemonEvent.HeadTracking(1_792_000_000_100, short.MinValue, short.MaxValue), "Motion events must preserve signed sensor samples and their observation time.");
foreach (bool ok in new[] { false, true })
{
    string result = JsonSerializer.Serialize(new { @event = "command_result", command = "set_name", request_id = 42, ok, message = "sent" });
    Check(Wire.ParseEvent(result) is DaemonEvent.CommandResult("set_name", 42, var actual, "sent") && actual == ok,
        "Rename acknowledgements must retain outcome and request correlation.");
}
foreach (var enabled in new[] { false, true })
{
    var line = Wire.ToLine(new SetMicModeCmd { Auto = enabled });
    Check(line.EndsWith('\n') && line.Count(c => c == '\n') == 1, "Command framing must be one NDJSON line");
    output.Add(line.TrimEnd('\n'));
}
output.Add(Wire.ToLine(new SetDeviceControlCmd { Id = 0x24, Value = 0x0200 }).TrimEnd('\n'));
output.Add(Wire.ToLine(new SetEqCmd { Mode = 2, Low = 40, Mid = 50, High = 60 }).TrimEnd('\n'));
output.Add(Wire.ToLine(new SetAutoPauseCmd { On = false }).TrimEnd('\n'));
output.Add(Wire.ToLine(new RefreshDeviceSettingsCmd()).TrimEnd('\n'));
foreach (byte percent in new byte[] { 0, 25, 100 })
    output.Add(Wire.ToLine(new SetConversationVolumeCmd { Percent = percent }).TrimEnd('\n'));
output.Add(Wire.ToLine(new SetNameCmd { Name = "Écouteurs 🎧", RequestId = 42 }).TrimEnd('\n'));
output.Add(Wire.ToLine(new PingCmd()).TrimEnd('\n'));
foreach (var (mode, delay) in new[] { ("automatic", 0u), ("automatic", 3600u), ("keep_active", 0u), ("manual", 0u) })
    output.Add(Wire.ToLine(new SetMicPolicyCmd { Mode = mode, StopDelaySecs = delay }).TrimEnd('\n'));
foreach (bool active in new[] { false, true })
    output.Add(Wire.ToLine(new SetMicManualActiveCmd { Active = active }).TrimEnd('\n'));
foreach (bool enabled in new[] { false, true })
{
    using var command = JsonDocument.Parse(Wire.ToLine(new SetBleScanWhileConnectedCmd { Enabled = enabled }));
    Check(command.RootElement.GetProperty("cmd").GetString() == "set_ble_scan_while_connected" &&
        command.RootElement.GetProperty("enabled").GetBoolean() == enabled, "Both playback battery scan choices must serialize faithfully.");
}
output.Add(Wire.ToLine(new SetBleScanWhileConnectedCmd { Enabled = true }).TrimEnd('\n'));
foreach (bool on in new[] { false, true })
    output.Add(Wire.ToLine(new SetHeadTrackingCmd { On = on }).TrimEnd('\n'));
output.Add(Wire.ToLine(new GetLoudSoundReductionCmd()).TrimEnd('\n'));
foreach (bool on in new[] { false, true })
    output.Add(Wire.ToLine(new SetLoudSoundReductionCmd { On = on }).TrimEnd('\n'));
output.Add(Wire.ToLine(new ToggleMicManualCmd()).TrimEnd('\n'));
foreach (string mode in new[] { "windows", "aac", "aac_eld" })
    output.Add(Wire.ToLine(new SetPlaybackModeCmd { Mode = mode }).TrimEnd('\n'));
output.Add(Wire.ToLine(new RetryPlaybackCmd()).TrimEnd('\n'));
foreach (var (mode, bestEffort, kbps) in new[] { ("aac", true, 512u), ("aac", false, 320u), ("aac_eld", true, 256u), ("aac_eld", false, 128u) })
    output.Add(Wire.ToLine(new SetPlaybackBitrateCmd { Mode = mode, BestEffort = bestEffort, Kbps = kbps }).TrimEnd('\n'));
output.Add(Wire.ToLine(new SetPlaybackBitrateCmd { Mode = "aac_eld", BestEffort = true, Kbps = 256, Variable = true }).TrimEnd('\n'));
output.Add(Wire.ToLine(new GetTransparencyCmd()).TrimEnd('\n'));
foreach (bool enabled in new[] { false, true })
    output.Add(Wire.ToLine(new SetTransparencyCmd { Settings = Transparency(enabled) }).TrimEnd('\n'));
output.Add(Wire.ToLine(new GetHearingAidCmd()).TrimEnd('\n'));
foreach (ulong id in new ulong[] { 0, 73 })
{
    string line = Wire.ToLine(new SetHearingAidCmd { RequestId = id, On = true, Expected = Hearing(), Settings = Hearing() });
    using var command = JsonDocument.Parse(line);
    Check(command.RootElement.GetProperty("expected").Deserialize<HearingAidSettings>()!.SameValues(Hearing()) &&
        command.RootElement.GetProperty("settings").Deserialize<HearingAidSettings>()!.SameValues(Hearing()),
        "Each hearing write must carry its caller's exact baseline as well as the requested values.");
    output.Add(line.TrimEnd('\n'));
}
output.Add(Wire.ToLine(new SetHearingAidCmd { RequestId = 74, On = false }).TrimEnd('\n'));
foreach (ushort? target in new ushort?[] { null, 60, 80, 100, 120, 140 })
    output.Add(Wire.ToLine(new SetPlaybackBufferCmd { TargetMs = target, RestoreMs = target.HasValue ? (ushort)150 : null }).TrimEnd('\n'));
File.WriteAllLines(args[1], output);
Console.WriteLine($"C#: {lines.Length} production/legacy events, VBR/Transparency roundtrips and malformed-input checks passed");

static TransparencySettings Transparency(bool enabled) => new() {
    Enabled = enabled, LeftAmplification = -1, RightAmplification = 1, LeftTone = -0.25f, RightTone = 0.5f,
    LeftConversationBoost = true, RightConversationBoost = false, LeftAmbientNoiseReduction = 0, RightAmbientNoiseReduction = 1
};

static HearingAidSettings Hearing() => new() {
    LeftEq = [5, 10, 15, 20, 25, 30, 35, 40], RightEq = [10, 15, 20, 25, 30, 35, 40, 45],
    LeftAmplification = -0.25f, RightAmplification = 0.5f, LeftTone = -0.5f, RightTone = 0.25f,
    LeftConversationBoost = true, RightConversationBoost = false,
    LeftAmbientNoiseReduction = 0.25f, RightAmbientNoiseReduction = 0.75f, OwnVoice = 0.5f
};
