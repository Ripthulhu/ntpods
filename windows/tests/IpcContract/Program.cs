using System.Text.Json;
using System.Text.Json.Nodes;
using NTPods.WinUI.Ipc;

if (args is ["--dongle", var dongleInput, var dongleOutput])
{
    DongleChecks.Run(dongleInput, dongleOutput);
    return;
}

if (args is ["--preferences", var preferenceInput, var preferenceOutput])
{
    CompanionPreferenceChecks.Run(preferenceInput, preferenceOutput);
    return;
}

static void Check(bool condition, string message)
{
    if (!condition) throw new InvalidOperationException(message);
}

Check(args.Length == 2, "usage: IpcContract INPUT OUTPUT");
string[] phases = ["idle", "starting", "capturing", "stopping", "recovering", "unavailable", "error", "waiting_for_ear"];
var allLines = File.ReadAllLines(args[0]);
Check(allLines.Length >= 2, "Expected two companion status fixtures");
var companionLines = allLines[^2..];
var lines = allLines[..^2];
Check(lines.Length == phases.Length + 3, "Expected lifecycle states, one legacy state and two lid events");
var output = new List<string>();
for (var index = 0; index < lines.Length - 2; ++index)
{
    var evt = Wire.ParseEvent(lines[index]) as DaemonEvent.State;
    Check(evt is not null, $"Rust fixture {index} did not parse as a flattened state");
    var s = evt!.Snapshot;
    using var raw = JsonDocument.Parse(lines[index]);
    Check(s.PlaybackMode == (raw.RootElement.TryGetProperty("playback_mode", out var playbackMode) ? playbackMode.GetString() : "windows") &&
        s.PlaybackPreferredMode == (raw.RootElement.TryGetProperty("playback_preferred_mode", out var preferredMode) ? preferredMode.GetString() : null) &&
        s.PlaybackState == (raw.RootElement.TryGetProperty("playback_state", out var playbackState) ? playbackState.GetString() : "windows") &&
        s.AudioOutputReady == (raw.RootElement.TryGetProperty("audio_output_ready", out var outputReady) && outputReady.GetBoolean()) &&
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
    Check(s.PlaybackLink == (raw.RootElement.TryGetProperty("playback_link", out var link) && link.ValueKind != JsonValueKind.Null
        ? link.Deserialize<PlaybackLinkSettings>() : null), "The saved frame policy must survive IPC; older backends leave it unavailable.");
    Check(s.PlaybackGameMode == (raw.RootElement.TryGetProperty("playback_game_mode", out var gameMode) && gameMode.ValueKind != JsonValueKind.Null
        ? gameMode.GetBoolean() : (bool?)null), "Game Mode preference must survive IPC without inventing support on older backends.");
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
    Check(s.NearbyFinder == (raw.RootElement.TryGetProperty("nearby_finder", out var nearbyFinder)
        ? nearbyFinder.Deserialize<NearbyFinderState>() : null),
        "Nearby finder state must preserve the selected-pair signal and pause state.");
    Check(s.DevName == "AirPods fixture — \"test\"", "Unicode or string escaping changed");
    Check(s.HearingAidEnabled == (raw.RootElement.TryGetProperty("hearing_aid_enabled", out var hearing) &&
        hearing.ValueKind != JsonValueKind.Null ? hearing.GetBoolean() : (bool?)null) &&
        s.HearingAidRevision == (raw.RootElement.TryGetProperty("hearing_aid_revision", out var hearingRevision) ? hearingRevision.GetUInt64() : 0),
        "Reported hearing state and its revision must survive IPC; missing state stays unknown.");
    Check(s.HearingAidEnrolled == (raw.RootElement.TryGetProperty("hearing_aid_enrolled", out var enrolled) &&
        enrolled.ValueKind != JsonValueKind.Null ? enrolled.GetBoolean() : (bool?)null) &&
        s.MediaAssistEnabled == (raw.RootElement.TryGetProperty("media_assist_enabled", out var mediaEnabled) &&
        mediaEnabled.ValueKind != JsonValueKind.Null ? mediaEnabled.GetBoolean() : (bool?)null) &&
        s.MediaAssistRevision == (raw.RootElement.TryGetProperty("media_assist_revision", out var mediaRevision) ? mediaRevision.GetUInt64() : 0) &&
        s.MediaAssistAvailable == (raw.RootElement.TryGetProperty("media_assist_available", out var mediaAvailable) && mediaAvailable.GetBoolean()),
        "Enrollment and media reports must retain unknown state and exact revisions across IPC.");
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
    Check(s.HardwareDiagnostics == (raw.RootElement.TryGetProperty("hardware_diagnostics", out var diagnostics) && diagnostics.ValueKind != JsonValueKind.Null
            ? diagnostics.Deserialize<HardwareDiagnostics>() : null) &&
        s.HardwareDiagnosticsPending == (raw.RootElement.TryGetProperty("hardware_diagnostics_pending", out var diagnosticPending) && diagnosticPending.GetBoolean()) &&
        s.HardwareDiagnosticsError == (raw.RootElement.TryGetProperty("hardware_diagnostics_error", out var diagnosticError) && diagnosticError.ValueKind != JsonValueKind.Null
            ? diagnosticError.GetString() : null), "Saved diagnostics must retain their full-width dates and separate read status.");
    var expectedMetadata = raw.RootElement.TryGetProperty("hearing_metadata", out var metadata) && metadata.ValueKind != JsonValueKind.Null
        ? metadata.Deserialize<HearingMetadata>() : null;
    Check((s.HearingMetadata is null ? expectedMetadata is null : s.HearingMetadata.SameValues(expectedMetadata)) &&
        s.HearingMetadataPending == (raw.RootElement.TryGetProperty("hearing_metadata_pending", out var metadataPending) && metadataPending.GetBoolean()) &&
        s.HearingMetadataError == (raw.RootElement.TryGetProperty("hearing_metadata_error", out var metadataError) && metadataError.ValueKind != JsonValueKind.Null
            ? metadataError.GetString() : null), "Raw hearing metadata and its read status must survive IPC without inventing calibration units.");
    Check(s.HardwareCheck == (raw.RootElement.TryGetProperty("hardware_check", out var hardwareCheck) && hardwareCheck.ValueKind != JsonValueKind.Null
            ? hardwareCheck.Deserialize<HardwareCheckState>() : null) &&
        s.EarbudsInCase == (raw.RootElement.TryGetProperty("earbuds_in_case", out var inCase) && inCase.ValueKind != JsonValueKind.Null
            ? inCase.GetBoolean() : (bool?)null), "Hardware-check progress and unknown case presence must survive IPC independently.");
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
        if (phase != "unavailable")
            Check(s.HardwareDiagnostics?.Left.FirstUseUnixSeconds == uint.MaxValue && s.HardwareDiagnostics.Right.FirstUseUnixSeconds is null,
                "First-use timestamps must preserve the full unsigned width and independent unknown ears.");
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
        Check(s.DeviceControlsRequested.Count == (phase == "starting" ? 1 : 0) &&
            (phase != "starting" || s.DeviceControlsRequested[0x23] == 2), $"Sent choices changed: {phase}");
        Check(s.DeviceControlsPending.Count == (phase == "starting" ? 2 : 0), $"Pending controls changed: {phase}");
        Check(s.AacpCapabilities.Count == 3 && s.AacpCapabilities[0x11] == 1 &&
            s.AacpCapabilities[0xC0] == 2 && s.AacpCapabilities[0x30] == uint.MaxValue,
            $"Raw AACP capability IDs and full values changed: {phase}");
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
        Check(!s.AudioOutputReady, "Legacy snapshots must not claim an active AirPods output.");
        Check(s.PlaybackAacBitrate == new PlaybackBitrate() && s.PlaybackAacEldBitrate == new PlaybackBitrate() && s.PlaybackBitrateKbps is null,
            "Missing bitrate preferences default to Best effort without inventing a current bitrate.");
        Check(!s.PlaybackDelayReporting && s.PlaybackDelayTenthsMs is null && s.PlaybackBufferTargetMs is null && s.PlaybackBufferError is null,
            "Older snapshots must leave playback delay and buffer targets unknown.");
        Check(s.Transparency is null && !s.TransparencyPending && s.TransparencyError is null,
            "Older snapshots must leave Transparency settings unknown.");
        Check(s.HardwareDiagnostics is null && !s.HardwareDiagnosticsPending && s.HardwareDiagnosticsError is null,
            "Older snapshots must leave hardware checks unknown.");
        Check(s.HearingMetadata is null && !s.HearingMetadataPending && s.HearingMetadataError is null &&
            s.HardwareCheck is null && s.EarbudsInCase is null,
            "Older snapshots must not invent readable hearing metadata, an active check or case presence.");
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
        Check(s.AutoPause && !s.AncUnconfirmed && s.DeviceControls.Count == 0 && s.DeviceControlsRequested.Count == 0 && s.DeviceControlsPending.Count == 0 &&
            s.Eq is null && s.EqPending is null && s.DeviceSettingsError is null, "Legacy device settings must remain unknown");
        Check(s.AacpCapabilities.Count == 0, "Older snapshots must not invent AACP capabilities.");
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
    "{\"event\":\"state\",\"audio_output_ready\":null}",
    "{\"event\":\"state\",\"audio_output_ready\":1}",
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
    "{\"event\":\"state\",\"device_controls_requested\":null}",
    "{\"event\":\"state\",\"device_controls_requested\":{\"256\":1}}",
    "{\"event\":\"state\",\"device_controls_requested\":{\"1\":-1}}",
    "{\"event\":\"state\",\"device_controls_pending\":null}",
    "{\"event\":\"state\",\"aacp_capabilities\":null}",
    "{\"event\":\"state\",\"aacp_capabilities\":{\"256\":1}}",
    "{\"event\":\"state\",\"aacp_capabilities\":{\"1\":-1}}",
    "{\"event\":\"state\",\"aacp_capabilities\":{\"1\":4294967296}}",
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
foreach (string value in new[] { "{}", "[]", "1", "true", "{\"read_at_unix_seconds\":1,\"left\":null,\"right\":null}" })
    Check(Wire.ParseEvent("{\"event\":\"state\",\"hardware_diagnostics\":" + value + "}") is null,
        "Malformed saved checks must not become successful-looking defaults.");
var diagnosticFixture = JsonNode.Parse(lines[0])!;
foreach (string ear in new[] { "left", "right" })
    foreach (string key in new[] { "measured_at_unix_seconds", "general", "measurement", "component_failures", "unknown_bits" })
    {
        var diagnosticMissing = diagnosticFixture.DeepClone();
        diagnosticMissing["hardware_diagnostics"]![ear]!.AsObject().Remove(key);
        Check(Wire.ParseEvent(diagnosticMissing.ToJsonString()) is null, "Incomplete saved checks must be rejected.");
    }
foreach (string key in new[] { "general", "measurement" })
{
    var diagnosticOutOfRange = diagnosticFixture.DeepClone();
    diagnosticOutOfRange["hardware_diagnostics"]!["left"]![key] = 8;
    Check(Wire.ParseEvent(diagnosticOutOfRange.ToJsonString()) is null, "Unknown status encodings must not exceed their three-bit fields.");
}
var oldDiagnostic = diagnosticFixture.DeepClone();
foreach (string ear in new[] { "left", "right" })
    oldDiagnostic["hardware_diagnostics"]![ear]!.AsObject().Remove("first_use_unix_seconds");
Check(Wire.ParseEvent(oldDiagnostic.ToJsonString()) is DaemonEvent.State { Snapshot.HardwareDiagnostics: { Left.FirstUseUnixSeconds: null, Right.FirstUseUnixSeconds: null } },
    "Saved checks from older daemons must keep first-use time unknown.");
foreach (string value in new[] { "-1", "4294967296", "1.5", "true", "\"1\"" })
{
    var invalid = diagnosticFixture.DeepClone();
    invalid["hardware_diagnostics"]!["left"]!["first_use_unix_seconds"] = JsonNode.Parse(value);
    Check(Wire.ParseEvent(invalid.ToJsonString()) is null, "First-use time must fit an unsigned 32-bit integer or be null.");
}

var metadataState = new JsonObject { ["event"] = "state", ["hearing_metadata"] = JsonSerializer.SerializeToNode(new HearingMetadata {
    ReadAtUnixSeconds = ulong.MaxValue, TestModeRaw = byte.MaxValue, DosimetrySensitivityRaw = -3.5f,
    DosimetryVolumeCurveRaw = Enumerable.Range(0, 17).Select(index => index * 0.25f - 2).ToArray()
}) };
Check(Wire.ParseEvent(metadataState.ToJsonString()) is DaemonEvent.State { Snapshot.HearingMetadata: { TestModeRaw: byte.MaxValue, ReadAtUnixSeconds: ulong.MaxValue } },
    "Unknown raw mode bytes and full-width read times must remain visible without assigning meanings.");
foreach (string key in metadataState["hearing_metadata"]!.AsObject().Select(pair => pair.Key).ToArray())
{
    var missing = metadataState.DeepClone();
    missing["hearing_metadata"]!.AsObject().Remove(key);
    Check(Wire.ParseEvent(missing.ToJsonString()) is null, "A supplied metadata object must include every nullable evidence field.");
}
foreach (string key in new[] { "test_mode_raw", "dosimetry_sensitivity_raw", "dosimetry_volume_curve_raw" })
{
    var absent = metadataState.DeepClone();
    absent["hearing_metadata"]![key] = null;
    Check(Wire.ParseEvent(absent.ToJsonString()) is DaemonEvent.State, "An unavailable metadata characteristic must stay null independently.");
}
foreach (string value in new[] { "-1", "256", "1.5", "true", "\"1\"" })
{
    var invalid = metadataState.DeepClone();
    invalid["hearing_metadata"]!["test_mode_raw"] = JsonNode.Parse(value);
    Check(Wire.ParseEvent(invalid.ToJsonString()) is null, "Raw mode states must fit a byte without coercion.");
}
foreach (string value in new[] { "1e40", "-1e40", "\"NaN\"", "true", "[]" })
{
    var invalid = metadataState.DeepClone();
    invalid["hearing_metadata"]!["dosimetry_sensitivity_raw"] = JsonNode.Parse(value);
    Check(Wire.ParseEvent(invalid.ToJsonString()) is null, "Raw sensitivity must remain a finite Float32 or null.");
}
foreach (int length in new[] { 0, 16, 18 })
{
    var invalid = metadataState.DeepClone();
    invalid["hearing_metadata"]!["dosimetry_volume_curve_raw"] = JsonSerializer.SerializeToNode(new float[length]);
    Check(Wire.ParseEvent(invalid.ToJsonString()) is null, "A supplied volume curve must contain exactly 17 values.");
}
foreach (string value in new[] { "null", "1e40", "-1e40", "\"NaN\"", "true" })
{
    var invalid = metadataState.DeepClone();
    invalid["hearing_metadata"]!["dosimetry_volume_curve_raw"]![8] = JsonNode.Parse(value);
    Check(Wire.ParseEvent(invalid.ToJsonString()) is null, "Every raw curve entry must be a finite Float32.");
}
Check(!new HearingMetadata { DosimetrySensitivityRaw = float.NaN }.IsValid &&
    !new HearingMetadata { DosimetryVolumeCurveRaw = Enumerable.Repeat(float.PositiveInfinity, 17).ToArray() }.IsValid,
    "Programmatic non-finite values must also fail validation.");
foreach (string key in new[] { "hearing_metadata", "hearing_metadata_pending", "hearing_metadata_error", "hardware_check", "earbuds_in_case" })
    Check(Wire.ParseEvent("{\"event\":\"state\",\"" + key + "\":1}") is null,
        "Malformed metadata, check status and case presence must not become defaults.");

var checkState = new JsonObject { ["event"] = "state", ["hardware_check"] = JsonSerializer.SerializeToNode(new HardwareCheckState {
    RequestId = ulong.MaxValue, Phase = "waiting_case_close", LeftUpdated = false, RightUpdated = false, Error = null
}) };
foreach (string key in checkState["hardware_check"]!.AsObject().Select(pair => pair.Key).ToArray())
{
    var missing = checkState.DeepClone();
    missing["hardware_check"]!.AsObject().Remove(key);
    Check(Wire.ParseEvent(missing.ToJsonString()) is null, "Partial hardware-check progress must not invent acknowledgements or outcomes.");
}
foreach (string phase in new[] { "waiting_case_close", "waiting_result", "completed", "timed_out", "cancelled", "error" })
{
    var valid = checkState.DeepClone();
    valid["hardware_check"]!["phase"] = phase;
    valid["hardware_check"]!["left_updated"] = phase == "completed";
    valid["hardware_check"]!["right_updated"] = phase == "completed";
    valid["hardware_check"]!["error"] = phase == "error" ? "Read failed." : null;
    Check(Wire.ParseEvent(valid.ToJsonString()) is DaemonEvent.State, "Known hardware-check phases must retain their outcome and request identity.");
}
foreach (var (key, value) in new[] { ("request_id", "0"), ("request_id", "-1"), ("request_id", "\"1\""),
    ("phase", "null"), ("phase", "\"running\""), ("left_updated", "null"), ("right_updated", "1"),
    ("error", "\"unexpected error\""), ("left_updated", "true"), ("phase", "\"completed\""), ("phase", "\"error\"") })
{
    var invalid = checkState.DeepClone();
    invalid["hardware_check"]![key] = JsonNode.Parse(value);
    Check(Wire.ParseEvent(invalid.ToJsonString()) is null, "Inconsistent, unknown or malformed hardware-check status must fail closed.");
}
foreach (string phase in new[] { "waiting_result", "timed_out", "cancelled", "error" })
{
    var invalid = checkState.DeepClone();
    invalid["hardware_check"]!["phase"] = phase;
    invalid["hardware_check"]!["left_updated"] = true;
    invalid["hardware_check"]!["right_updated"] = true;
    invalid["hardware_check"]!["error"] = phase == "error" ? "Read failed." : null;
    Check(Wire.ParseEvent(invalid.ToJsonString()) is null, "Both fresh ears require the completed phase.");
}
var hearingState = new JsonObject { ["event"] = "state", ["hearing_aid_settings"] = JsonSerializer.SerializeToNode(Hearing()) };
Check(Wire.ParseEvent(hearingState.ToJsonString()) is DaemonEvent.State { Snapshot.HearingAidSettings: { } hearingRoundtrip } &&
    hearingRoundtrip.SameValues(Hearing()), "Asymmetric hearing curves and tuning must survive C# parsing.");
Check(Wire.ParseEvent(hearingState.ToJsonString()) is DaemonEvent.State { Snapshot.HearingAidEditable: false },
    "An older daemon's hearing settings must remain read-only without explicit editability.");
foreach (var amplification in new[] { -1.5f, 1.5f })
{
    var boundary = hearingState.DeepClone();
    boundary["hearing_aid_settings"]!["left_amplification"] = amplification;
    boundary["hearing_aid_settings"]!["right_amplification"] = -amplification;
    Check(Wire.ParseEvent(boundary.ToJsonString()) is DaemonEvent.State,
        "Hearing amplification must accept both fitted profile boundaries.");
}
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
    ("left_amplification", -1.51f), ("right_amplification", 1.51f), ("left_tone", -1.01f), ("right_tone", 1.01f),
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
    output.Add(Wire.ToLine(new SetTransparencyCmd { Settings = Transparency(enabled), Expected = Transparency(false) }).TrimEnd('\n'));
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
output.Add(Wire.ToLine(new PairingCompletedCmd { Address = "12:34:56:78:9a:bc" }).TrimEnd('\n'));
Check(Wire.ToLine(new ConnectCmd()) == "{\"cmd\":\"connect\"}\n",
    "Existing Connect callers must keep the legacy wire shape.");
output.Add(Wire.ToLine(new ConnectCmd()).TrimEnd('\n'));
output.Add(Wire.ToLine(new ConnectCmd { DeviceId = "001122334455" }).TrimEnd('\n'));
foreach (bool on in new[] { false, true })
{
    string aid = Wire.ToLine(new SetHearingAidModeCmd { RequestId = 81, On = on });
    string media = Wire.ToLine(new SetMediaAssistCmd { RequestId = 82, On = on });
    foreach (string line in new[] { aid, media })
    {
        using var modeCommand = JsonDocument.Parse(line);
        Check(!modeCommand.RootElement.TryGetProperty("settings", out _) &&
            !modeCommand.RootElement.TryGetProperty("expected", out _), "Mode switches must never carry fitting edits.");
        output.Add(line.TrimEnd('\n'));
    }
}
output.Add(Wire.ToLine(new StartNearbyFinderCmd { DeviceId = "001122334455" }).TrimEnd('\n'));
output.Add(Wire.ToLine(new StopNearbyFinderCmd { DeviceId = "001122334455" }).TrimEnd('\n'));
output.Add(Wire.ToLine(new GetHardwareDiagnosticsCmd()).TrimEnd('\n'));
Check(Wire.ToLine(new SetAncCmd { Mode = 3 }) == "{\"cmd\":\"set_anc\",\"mode\":3}\n" &&
    Wire.ToLine(new SetDeviceControlCmd { Id = 0x2E, Value = 50 }) == "{\"cmd\":\"set_device_control\",\"id\":46,\"value\":50}\n",
    "Legacy control callers must keep omitting an unspecified device target.");
output.Add(Wire.ToLine(new SetAncCmd { Mode = 3 }).TrimEnd('\n'));
foreach (object command in new object[] {
    new SetAncCmd { Mode = 3, DeviceId = "001122334455" },
    new SetDeviceControlCmd { Id = 0x2E, Value = 50, DeviceId = "001122334455" }
})
{
    string line = Wire.ToLine(command);
    using var targeted = JsonDocument.Parse(line);
    Check(targeted.RootElement.GetProperty("device_id").GetString() == "001122334455",
        "A control action must retain the exact device displayed when it was requested.");
    output.Add(line.TrimEnd('\n'));
}
foreach (string mode in new[] { "off", "automatic", "manual" })
    output.Add(Wire.ToLine(new SetPlaybackLinkCmd { Settings = new() { Mode = mode, Kbps = 174 } }).TrimEnd('\n'));
foreach (bool enabled in new[] { false, true })
    output.Add(Wire.ToLine(new SetPlaybackGameModeCmd { Enabled = enabled }).TrimEnd('\n'));
foreach (string target in new[] { "", "not-a-device", "00112233445g" })
foreach (object command in new object[] {
    new SetAncCmd { Mode = 3, DeviceId = target },
    new SetDeviceControlCmd { Id = 0x2E, Value = 50, DeviceId = target }
})
{
    string line = Wire.ToLine(command);
    using var invalid = JsonDocument.Parse(line);
    Check(invalid.RootElement.GetProperty("device_id").GetString() == target,
        "An invalid explicit target must remain explicit so the daemon can reject it.");
    output.Add(line.TrimEnd('\n'));
}
foreach (string target in new[] { "null", "42", "true", "[]", "{}" })
foreach (object command in new object[] { new SetAncCmd { Mode = 3 }, new SetDeviceControlCmd { Id = 0x2E, Value = 50 } })
{
    var invalid = JsonNode.Parse(Wire.ToLine(command))!.AsObject();
    invalid["device_id"] = JsonNode.Parse(target);
    output.Add(invalid.ToJsonString());
}
Check(Wire.ParseEvent(lines[^2]) is DaemonEvent.PairingNearby(0x2720, true),
    "Nearby invitations must retain the public model hint and lid state.");
Check(Wire.ParseEvent("{\"event\":\"pairing_nearby\",\"model\":10016,\"lid_open\":false}")
    is DaemonEvent.PairingNearby(0x2720, false), "Closing the lid must survive event parsing.");
foreach (string value in new[] { "null", "-1", "65536", "1.5", "true", "\"10016\"", "[]", "{}" })
    Check(Wire.ParseEvent("{\"event\":\"pairing_nearby\",\"model\":" + value + ",\"lid_open\":true}") is null,
        "Malformed invitation models must not reach the UI.");
Check(Wire.ParseEvent("{\"event\":\"pairing_nearby\"}") is null, "Missing invitation models must be rejected.");
foreach (string value in new[] { "null", "0", "1", "\"true\"", "[]", "{}" })
    Check(Wire.ParseEvent("{\"event\":\"pairing_nearby\",\"model\":10016,\"lid_open\":" + value + "}") is null,
        "Malformed lid states must not dismiss or open a pairing card.");
Check(Wire.ParseEvent("{\"event\":\"pairing_nearby\",\"model\":10016}") is null,
    "Missing lid states must not become open invitations.");
output.Add(JsonSerializer.Serialize(new { @event = "pairing_nearby", model = 0x2720, lid_open = true }));
Check(Wire.ParseEvent(lines[^1]) is DaemonEvent.PairedLid("001122334455", false, 2),
    "Paired lid edges must retain exact owner, state and daemon-session sequence.");
foreach (string malformedLid in new[] {
    "{\"event\":\"paired_lid\",\"device_id\":\"001122334455\",\"lid_open\":true}",
    "{\"event\":\"paired_lid\",\"device_id\":\"001122334455\",\"sequence\":1}",
    "{\"event\":\"paired_lid\",\"lid_open\":true,\"sequence\":1}",
    "{\"event\":\"paired_lid\",\"device_id\":\"001122334455\",\"lid_open\":1,\"sequence\":1}",
    "{\"event\":\"paired_lid\",\"device_id\":\"001122334455\",\"lid_open\":true,\"sequence\":0}",
    "{\"event\":\"paired_lid\",\"device_id\":\"00112233445g\",\"lid_open\":true,\"sequence\":1}",
}) Check(Wire.ParseEvent(malformedLid) is null, "Malformed paired lid edges must not reach the island.");
output.Add(JsonSerializer.Serialize(new { @event = "paired_lid", device_id = "001122334455", lid_open = false, sequence = 2UL }));
var modeProbe = JsonNode.Parse("""
    {"event":"hearing_test_mode_probe","request_id":17,"device_id":"001122334455","enable_check":true,
     "result":{"initial_state":0,"enabled_state":1,"final_state":0,"enable_sent":true,"disable_sent":true,"restoration_verified":true,"error":null}}
    """)!.AsObject();
Check(Wire.ParseEvent(modeProbe.ToJsonString()) is DaemonEvent.HearingTestModeProbe { Result.RestorationVerified: true },
    "A verified mode restoration must remain distinct from a completed write.");
foreach (var field in new[] { "enable_sent", "disable_sent", "restoration_verified" })
{
    var missing = modeProbe.DeepClone().AsObject();
    missing["result"]!.AsObject().Remove(field);
    Check(Wire.ParseEvent(missing.ToJsonString()) is null, "Missing restoration evidence must fail closed.");
}
foreach (var (field, value) in new[] { ("initial_state", 6), ("enabled_state", 256), ("final_state", 1) })
{
    var invalid = modeProbe.DeepClone().AsObject();
    invalid["result"]![field] = value;
    Check(Wire.ParseEvent(invalid.ToJsonString()) is null, "Invalid or inconsistent mode readback must fail closed.");
}
var partialProbe = modeProbe.DeepClone().AsObject();
partialProbe["result"]!["final_state"] = null;
partialProbe["result"]!["restoration_verified"] = false;
partialProbe["result"]!["error"] = "Restoration readback timed out.";
Check(Wire.ParseEvent(partialProbe.ToJsonString()) is DaemonEvent.HearingTestModeProbe { Result.RestorationVerified: false },
    "A failed restoration must retain its partial evidence.");
partialProbe["result"]!["enable_sent"] = false;
Check(Wire.ParseEvent(partialProbe.ToJsonString()) is DaemonEvent.HearingTestModeProbe { Result.DisableSent: true, Result.EnableSent: false },
    "An ambiguous enable-send failure can still attempt a same-session disable.");
foreach (JsonNode? malformedProbeId in new JsonNode?[] { null, JsonValue.Create("17"), JsonValue.Create(-1) })
{
    var invalid = modeProbe.DeepClone().AsObject();
    invalid["request_id"] = malformedProbeId;
    Check(Wire.ParseEvent(invalid.ToJsonString()) is null, "Malformed probe IDs must not throw or match another request.");
}
output.Add(Wire.ToLine(new GetHearingMetadataCmd { DeviceId = "001122334455" }).TrimEnd('\n'));
output.Add(Wire.ToLine(new StartHardwareCheckCmd { RequestId = ulong.MaxValue, DeviceId = "001122334455", BudsInCaseConfirmed = true }).TrimEnd('\n'));
output.Add(Wire.ToLine(new CancelHardwareCheckCmd { RequestId = ulong.MaxValue, DeviceId = "001122334455" }).TrimEnd('\n'));
const string companionSecret = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
for (var index = 0; index < companionLines.Length; ++index)
{
    Check(Wire.ParseEvent(companionLines[index]) is DaemonEvent.Companion, "Companion status must dispatch to its typed event");
    var status = ((DaemonEvent.Companion)Wire.ParseEvent(companionLines[index])!).Status;
    Check(status.Linked == (index == 0) && status.Connected == (index == 0) && status.HeldAway == (index == 0) &&
        status.DeviceId == (index == 0 ? "001122334455" : null) &&
        status.PeerAddress == (index == 0 ? "AA:BB:CC:DD:EE:FF" : null) &&
        status.Phase == (index == 0 ? "released" : "idle") &&
        status.Error == (index == 0 ? "Transfer \"stopped\".\nTry again." : null),
        "Companion approval, held-away intent, phase, nulls and escaped errors must retain their values");
    string line = JsonSerializer.Serialize(new { @event = "companion_status", status }, Wire.Json);
    using var raw = JsonDocument.Parse(line);
    Check(raw.RootElement.GetProperty("status").EnumerateObject().Select(field => field.Name).Order().SequenceEqual(
        new[] { "linked", "connected", "device_id", "peer_address", "held_away", "phase", "error" }.Order()) &&
        !line.Contains(companionSecret, StringComparison.Ordinal), "Companion events must contain public status only, without approval secrets");
    output.Add(line);
}
foreach (string malformedCompanion in new[] {
    "{\"event\":\"companion_status\"}",
    "{\"event\":\"companion_status\",\"status\":null}",
    "{\"event\":\"companion_status\",\"status\":true}",
    "{\"event\":\"companion_status\",\"status\":{\"linked\":1}}"
}) Check(Wire.ParseEvent(malformedCompanion) is null, "Malformed companion status must not reach its UI");
string linkLine = Wire.ToLine(new CompanionLinkCmd {
    PeerAddress = "AA:BB:CC:DD:EE:FF", SecretHex = companionSecret, DeviceId = "001122334455"
});
var linkedCommand = JsonSerializer.Deserialize<CompanionLinkCmd>(linkLine)!;
Check(linkedCommand.PeerAddress == "AA:BB:CC:DD:EE:FF" && linkedCommand.SecretHex == companionSecret &&
    linkedCommand.DeviceId == "001122334455", "Explicit link approval must carry the intended peer, accessory and full secret");
output.Add(linkLine.TrimEnd('\n'));
Check(Wire.ToLine(new CompanionStatusCmd()) == "{\"cmd\":\"companion_status\"}\n", "Status requests must have no audio action or secret payload");
output.Add(Wire.ToLine(new CompanionStatusCmd()).TrimEnd('\n'));
foreach (string target in new[] { "pc", "android" })
{
    string line = Wire.ToLine(new CompanionSwitchCmd { Target = target, DeviceId = "001122334455", RequestId = ulong.MaxValue });
    var command = JsonSerializer.Deserialize<CompanionSwitchCmd>(line)!;
    Check(command.Target == target && command.DeviceId == "001122334455" && command.RequestId == ulong.MaxValue,
        "Either transfer direction must preserve the exact accessory and full-width request correlation");
    output.Add(line.TrimEnd('\n'));
}
Check(Wire.ToLine(new CompanionForgetCmd()) == "{\"cmd\":\"companion_forget\"}\n", "Forgetting approval must remain a separate command");
output.Add(Wire.ToLine(new CompanionForgetCmd()).TrimEnd('\n'));
File.WriteAllLines(args[1], output);
Console.WriteLine($"C#: {lines.Length} production/legacy events, two companion events, five companion commands and malformed-input checks passed");

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
