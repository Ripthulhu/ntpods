//! Executed by windows/tests/test-ipc.ps1 against the real C# wire model.
use ntpods_ipc::{
    from_line, to_line, BatteryFreshness, BatteryObservation, Command, Equalizer, Event,
    HearingAidSettings, MicMode, PlaybackBitrate, PlaybackMode, Snapshot, TransparencySettings,
};

fn transparency(enabled: bool) -> TransparencySettings {
    TransparencySettings {
        enabled,
        left_amplification: -1.0,
        right_amplification: 1.0,
        left_tone: -0.25,
        right_tone: 0.5,
        left_conversation_boost: true,
        right_conversation_boost: false,
        left_ambient_noise_reduction: 0.0,
        right_ambient_noise_reduction: 1.0,
    }
}

fn hearing_settings() -> HearingAidSettings {
    HearingAidSettings {
        left_eq: [5., 10., 15., 20., 25., 30., 35., 40.],
        right_eq: [10., 15., 20., 25., 30., 35., 40., 45.],
        left_amplification: -0.25,
        right_amplification: 0.5,
        left_tone: -0.5,
        right_tone: 0.25,
        left_conversation_boost: true,
        right_conversation_boost: false,
        left_ambient_noise_reduction: 0.25,
        right_ambient_noise_reduction: 0.75,
        own_voice: 0.5,
    }
}

fn snapshots() -> Vec<Snapshot> {
    [
        "idle",
        "starting",
        "capturing",
        "stopping",
        "recovering",
        "unavailable",
        "error",
        "waiting_for_ear",
    ]
    .into_iter()
    .map(|phase| Snapshot {
        connected: true,
        playback_mode: match phase {
            "idle" => PlaybackMode::Windows,
            "capturing" => PlaybackMode::AacEld,
            _ => PlaybackMode::Aac,
        },
        playback_preferred_mode: match phase {
            "idle" => Some(PlaybackMode::Windows),
            "unavailable" => None,
            _ => Some(PlaybackMode::AacEld),
        },
        playback_state: match phase {
            "idle" => "windows",
            "starting" => "starting",
            "stopping" => "stopping",
            "error" => "error",
            "unavailable" => "disconnected",
            _ => "ready",
        }
        .into(),
        audio_output_ready: matches!(phase, "idle" | "capturing"),
        playback_error: (phase == "error")
            .then(|| "The AirPods cannot use this AAC bitrate.".into()),
        playback_available: phase != "unavailable",
        playback_native_driver: phase != "unavailable",
        playback_aac_bitrate: PlaybackBitrate {
            best_effort: !matches!(phase, "capturing" | "error"),
            kbps: 320,
            variable: false,
        },
        playback_aac_eld_bitrate: PlaybackBitrate {
            best_effort: phase != "error",
            kbps: if phase == "capturing" { 256 } else { 128 },
            variable: phase == "capturing",
        },
        playback_bitrate_kbps: (phase == "capturing").then_some(256),
        playback_delay_reporting: matches!(
            phase,
            "starting" | "capturing" | "stopping" | "recovering"
        ),
        playback_delay_tenths_ms: match phase {
            "starting" => Some(0),
            "capturing" => Some(1500),
            "stopping" => Some(u16::MAX),
            _ => None,
        },
        playback_buffer_target_ms: (phase == "capturing").then_some(60),
        playback_buffer_error: (phase == "error")
            .then(|| "A restoration baseline is required.".into()),
        battery_live: phase != "error",
        battery_timestamp_ms: Some(1_792_000_000_100),
        battery_freshness: Some(BatteryFreshness {
            left: BatteryObservation {
                timestamp_ms: Some(1_792_000_000_100),
                live: phase != "error",
                present: Some(true),
            },
            right: BatteryObservation {
                timestamp_ms: Some(1_792_000_000_090),
                live: phase != "error",
                present: Some(true),
            },
            case: BatteryObservation {
                timestamp_ms: Some(1_791_999_900_000),
                live: false,
                present: Some(false),
            },
            ..Default::default()
        }),
        battery_source: if phase == "idle" { "ble" } else { "aap" }.into(),
        ble_scan_while_connected: phase == "idle",
        dev_name: "AirPods fixture — \"test\"".into(),
        mic_state: phase.into(),
        mic_capture_active: matches!(
            phase,
            "starting" | "capturing" | "error" | "waiting_for_ear"
        ),
        mic_uplink_active: matches!(phase, "starting" | "capturing" | "stopping" | "recovering"),
        mic_recording: phase == "capturing",
        mic_available: phase != "unavailable",
        auto_mode: !matches!(phase, "unavailable" | "error"),
        mic_mode: match phase {
            "capturing" => MicMode::KeepActive,
            "waiting_for_ear" => MicMode::Manual,
            _ => MicMode::Automatic,
        },
        mic_stop_delay_secs: if phase == "starting" { 30 } else { 0 },
        mic_manual_active: phase == "waiting_for_ear",
        conversational_awareness_pending: match phase {
            "starting" => Some(true),
            "stopping" => Some(false),
            _ => None,
        },
        mic_error: (phase == "error")
            .then(|| "Output failed: \"device gone\"\nTry again — later.".into()),
        device_controls: [(1, 0), (31, 0x504B), (36, 0x0300), (40, 2)].into(),
        device_controls_requested: if phase == "starting" {
            [(0x23, 2)].into()
        } else {
            Default::default()
        },
        aacp_capabilities: [(0x11, 1), (0xC0, 2), (0x30, u32::MAX)].into(),
        device_controls_pending: if phase == "starting" {
            [(35, 3), (40, 1)].into()
        } else {
            Default::default()
        },
        adaptive_strength_requested: match phase {
            "idle" => Some(0),
            "starting" => Some(50),
            "capturing" => Some(100),
            _ => None,
        },
        allow_off_requested: match phase {
            "idle" => Some(true),
            "starting" => Some(false),
            _ => None,
        },
        device_settings_error: (phase == "error")
            .then(|| "AirPods did not confirm the setting.".into()),
        eq: (phase != "unavailable").then_some(Equalizer {
            mode: 2,
            low: 35,
            mid: 50,
            high: 65,
        }),
        eq_pending: (phase == "starting").then_some(Equalizer {
            mode: 2,
            low: 40,
            mid: 55,
            high: 70,
        }),
        hearing_aid_enabled: match phase {
            "idle" => Some(false),
            "capturing" => Some(true),
            _ => None,
        },
        hearing_aid_revision: 12,
        hearing_aid_enrolled: (phase != "unavailable").then_some(true),
        media_assist_enabled: match phase {
            "idle" => Some(false),
            "capturing" => Some(true),
            _ => None,
        },
        media_assist_revision: 25,
        media_assist_available: matches!(phase, "idle" | "capturing"),
        hearing_aid_settings: (phase != "unavailable").then_some(hearing_settings()),
        hearing_aid_editable: matches!(phase, "idle" | "capturing"),
        hearing_aid_pending: phase == "starting",
        hearing_aid_error: (phase == "error")
            .then(|| "Existing audiograms could not be read.".into()),
        hardware_diagnostics: (phase != "unavailable").then_some(ntpods_ipc::HardwareDiagnostics {
            read_at_unix_seconds: 1_700_000_100,
            left: ntpods_ipc::HardwareDiagnosticEar {
                measured_at_unix_seconds: u64::MAX, general: 1, measurement: 0,
                component_failures: Some(1), unknown_bits: 0x10000,
            },
            right: ntpods_ipc::HardwareDiagnosticEar {
                measured_at_unix_seconds: 0, general: 0, measurement: 0,
                component_failures: None, unknown_bits: 0,
            },
        }),
        hardware_diagnostics_pending: phase == "starting",
        hardware_diagnostics_error: (phase == "error").then(|| "Read failed.".into()),
        loud_sound_reduction: match phase {
            "idle" => Some(true),
            "starting" | "error" => Some(false),
            _ => None,
        },
        loud_sound_reduction_pending: phase == "starting",
        loud_sound_reduction_error: (phase == "error")
            .then(|| "AirPods did not apply Loud Sound Reduction. Try again.".into()),
        transparency: (phase != "unavailable").then_some(transparency(phase == "capturing")),
        transparency_pending: phase == "starting",
        transparency_error: (phase == "error")
            .then(|| "Transparency settings could not be read.".into()),
        auto_pause: phase != "stopping",
        conversation_volume: match phase {
            "idle" => 0,
            "starting" => 25,
            "capturing" => 100,
            _ => 60,
        },
        heart_rate_enabled: phase == "capturing",
        heart_rate: (phase == "capturing").then_some(72),
        heart_rate_timestamp_ms: (phase == "capturing").then_some(1_792_000_000_123),
        head_tracking_enabled: matches!(phase, "starting" | "capturing" | "waiting_for_ear"),
        head_tracking_state: match phase {
            "capturing" => "streaming",
            "starting" | "waiting_for_ear" | "error" => phase,
            _ => "idle",
        }
        .into(),
        head_tracking_error: (phase == "error").then(|| "No head-tracking data.".into()),
        anc_unconfirmed: matches!(phase, "starting" | "error"),
        ..Snapshot::default()
    })
    .collect()
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(args.len(), 3, "usage: mic_wire_fixtures emit|check FILE");
    let states = snapshots();
    let old_bitrate: PlaybackBitrate =
        serde_json::from_str(r#"{"best_effort":true,"kbps":256}"#).unwrap();
    assert_eq!(old_bitrate, PlaybackBitrate::default());
    assert!(matches!(
        from_line::<Command>(
            r#"{"cmd":"set_playback_bitrate","mode":"aac_eld","best_effort":true,"kbps":256}"#
        ),
        Some(Command::SetPlaybackBitrate {
            variable: false,
            ..
        })
    ));
    for value in ["null", "1", "\"true\""] {
        let malformed = format!(
            r#"{{"cmd":"set_playback_bitrate","mode":"aac_eld","best_effort":true,"kbps":256,"variable":{value}}}"#
        );
        assert!(from_line::<Command>(&malformed).is_none());
    }
    let valid = serde_json::to_value(Command::SetTransparency {
        expected: Some(transparency(false)),
        settings: transparency(true),
    })
    .unwrap();
    assert!(transparency(true).is_valid());
    for key in valid["settings"].as_object().unwrap().keys() {
        let mut missing = valid.clone();
        missing["settings"].as_object_mut().unwrap().remove(key);
        assert!(from_line::<Command>(&to_line(&missing)).is_none());
    }
    for (key, bad) in [
        ("left_amplification", -1.01),
        ("right_amplification", 1.01),
        ("left_tone", -1.01),
        ("right_tone", 1.01),
        ("left_ambient_noise_reduction", -0.01),
        ("right_ambient_noise_reduction", 1.01),
    ] {
        let mut value = valid.clone();
        value["settings"][key] = serde_json::json!(bad);
        assert!(matches!(from_line::<Command>(&to_line(&value)),
            Some(Command::SetTransparency { settings, .. }) if !settings.is_valid()));
    }
    match args[1].as_str() {
        "emit" => {
            let mut lines = String::new();
            for snapshot in &states {
                let line = to_line(&Event::State(snapshot.clone()));
                assert_eq!(
                    line.lines().count(),
                    1,
                    "escaped text must stay one NDJSON message"
                );
                let value: serde_json::Value = serde_json::from_str(&line).unwrap();
                assert_eq!(value["event"], "state");
                assert!(
                    value.get("snapshot").is_none(),
                    "state must remain flattened"
                );
                let Event::State(roundtrip) = from_line::<Event>(&line).unwrap() else {
                    panic!("wrong event");
                };
                assert_eq!(&roundtrip, snapshot);
                lines.push_str(&line);
            }
            // An older daemon can report mic_recording=true without the new
            // lifecycle fields. Neither client may infer new permissions from it.
            let mut legacy = serde_json::to_value(Event::State(states[2].clone())).unwrap();
            for key in [
                "battery_live",
                "battery_freshness",
                "battery_timestamp_ms",
                "battery_source",
                "ble_scan_while_connected",
                "anc_unconfirmed",
                "mic_capture_active",
                "mic_uplink_active",
                "mic_state",
                "mic_error",
                "mic_mode",
                "mic_stop_delay_secs",
                "mic_manual_active",
                "conversational_awareness_pending",
                "device_controls",
                "device_controls_requested",
                "device_controls_pending",
                "aacp_capabilities",
                "adaptive_strength_requested",
                "allow_off_requested",
                "device_settings_error",
                "eq",
                "eq_pending",
                "hearing_aid_enabled",
                "hearing_aid_revision",
                "hearing_aid_settings",
                "hearing_aid_editable",
                "hearing_aid_pending",
                "hearing_aid_error",
                "loud_sound_reduction",
                "loud_sound_reduction_pending",
                "loud_sound_reduction_error",
                "transparency",
                "transparency_pending",
                "transparency_error",
                "hardware_diagnostics",
                "hardware_diagnostics_pending",
                "hardware_diagnostics_error",
                "auto_pause",
                "conversation_volume",
                "heart_rate_enabled",
                "heart_rate_timestamp_ms",
                "head_tracking_enabled",
                "head_tracking_state",
                "head_tracking_error",
                "playback_mode",
                "playback_preferred_mode",
                "playback_state",
                "audio_output_ready",
                "playback_error",
                "playback_available",
                "playback_native_driver",
                "playback_aac_bitrate",
                "playback_aac_eld_bitrate",
                "playback_bitrate_kbps",
                "playback_delay_reporting",
                "playback_delay_tenths_ms",
                "playback_buffer_target_ms",
                "playback_buffer_error",
            ] {
                legacy.as_object_mut().unwrap().remove(key);
            }
            let legacy_line = to_line(&legacy);
            let Event::State(old) = from_line::<Event>(&legacy_line).unwrap() else {
                panic!("wrong legacy event");
            };
            assert!(!old.mic_capture_active && !old.mic_uplink_active);
            assert!(old.mic_state.is_empty() && old.mic_error.is_none());
            assert!(old.conversational_awareness_pending.is_none());
            assert!(old.adaptive_strength_requested.is_none());
            assert!(old.allow_off_requested.is_none());
            assert!(old.hearing_aid_enabled.is_none() && old.hearing_aid_revision == 0);
            assert!(!old.hearing_aid_editable);
            assert!(old.hardware_diagnostics.is_none() && !old.hardware_diagnostics_pending && old.hardware_diagnostics_error.is_none());
            assert!(
                old.transparency.is_none()
                    && !old.transparency_pending
                    && old.transparency_error.is_none()
            );
            assert!(
                old.loud_sound_reduction.is_none()
                    && !old.loud_sound_reduction_pending
                    && old.loud_sound_reduction_error.is_none()
            );
            assert!(old.device_controls.is_empty() && old.device_controls_requested.is_empty() && old.device_controls_pending.is_empty());
            assert!(old.aacp_capabilities.is_empty());
            assert!(
                old.eq.is_none() && old.eq_pending.is_none() && old.device_settings_error.is_none()
            );
            assert!(old.auto_pause);
            assert_eq!(old.conversation_volume, 25);
            assert!(!old.anc_unconfirmed);
            assert!(!old.heart_rate_enabled && old.heart_rate_timestamp_ms.is_none());
            assert!(
                !old.head_tracking_enabled
                    && old.head_tracking_state == "idle"
                    && old.head_tracking_error.is_none()
            );
            assert!(!old.ble_scan_while_connected);
            assert!(
                !old.battery_live
                    && old.battery_freshness.is_none()
                    && old.battery_timestamp_ms.is_none()
                    && old.battery_source.is_empty()
            );
            lines.push_str(&legacy_line);
            lines.push_str(&to_line(&Event::PairingNearby {
                model: 0x2720,
                lid_open: true,
            }));
            lines.push_str(&to_line(&Event::PairedLid {
                device_id: "001122334455".into(),
                lid_open: false,
                sequence: 2,
            }));
            std::fs::write(&args[2], lines).unwrap();
            println!("Rust: emitted eight lifecycle states, one legacy state and two lid events");
        }
        "check" => {
            let text = std::fs::read_to_string(&args[2]).unwrap();
            let lines: Vec<_> = text.lines().collect();
            assert_eq!(
                lines.len(),
                states.len() + 57,
                "C# states, legacy state, fifty-four commands and two lid events expected"
            );
            for (line, expected) in lines.iter().zip(&states) {
                let Event::State(actual) = from_line::<Event>(line).expect("C# event must parse")
                else {
                    panic!("wrong event");
                };
                assert_eq!(actual.hearing_aid_enrolled, expected.hearing_aid_enrolled);
                assert_eq!(actual.media_assist_enabled, expected.media_assist_enabled);
                assert_eq!(actual.media_assist_revision, expected.media_assist_revision);
                assert_eq!(
                    actual.media_assist_available,
                    expected.media_assist_available
                );
                assert_eq!(
                    &actual, expected,
                    "C# roundtrip changed the production state"
                );
            }
            let Event::State(legacy) = from_line::<Event>(lines[states.len()]).unwrap() else {
                panic!("wrong legacy event");
            };
            assert!(!legacy.mic_capture_active && !legacy.mic_uplink_active);
            assert_eq!(legacy.mic_state, "unavailable");
            assert!(legacy.mic_error.is_none());
            assert!(!legacy.playback_delay_reporting);
            assert!(!legacy.audio_output_ready);
            assert!(legacy.playback_delay_tenths_ms.is_none());
            assert!(legacy.playback_buffer_target_ms.is_none());
            assert!(legacy.playback_buffer_error.is_none());
            assert!(legacy.auto_pause && legacy.device_controls.is_empty() && legacy.device_controls_requested.is_empty() && legacy.eq.is_none());
            assert!(legacy.aacp_capabilities.is_empty());
            assert!(!legacy.anc_unconfirmed);
            assert!(legacy.adaptive_strength_requested.is_none());
            assert!(legacy.allow_off_requested.is_none());
            assert!(legacy.hearing_aid_enabled.is_none() && legacy.hearing_aid_revision == 0);
            assert!(!legacy.hearing_aid_editable);
            assert!(legacy.hardware_diagnostics.is_none() && !legacy.hardware_diagnostics_pending && legacy.hardware_diagnostics_error.is_none());
            assert!(
                legacy.transparency.is_none()
                    && !legacy.transparency_pending
                    && legacy.transparency_error.is_none()
            );
            assert!(
                legacy.loud_sound_reduction.is_none()
                    && !legacy.loud_sound_reduction_pending
                    && legacy.loud_sound_reduction_error.is_none()
            );
            assert_eq!(legacy.conversation_volume, 25);
            assert_eq!(legacy.mic_mode, MicMode::Automatic);
            assert_eq!(legacy.mic_stop_delay_secs, 0);
            assert!(!legacy.mic_manual_active);
            assert!(!legacy.ble_scan_while_connected);
            assert!(
                !legacy.head_tracking_enabled
                    && legacy.head_tracking_state == "idle"
                    && legacy.head_tracking_error.is_none()
            );
            assert!(
                !legacy.battery_live
                    && legacy.battery_freshness.is_none()
                    && legacy.battery_timestamp_ms.is_none()
                    && legacy.battery_source.is_empty()
            );
            assert!(matches!(
                from_line::<Event>(lines[lines.len() - 2]),
                Some(Event::PairingNearby {
                    model: 0x2720,
                    lid_open: true
                })
            ));
            assert!(matches!(
                from_line::<Event>(lines.last().unwrap()),
                Some(Event::PairedLid { device_id, lid_open: false, sequence: 2 })
                    if device_id == "001122334455"
            ));
            let commands = &lines[states.len() + 1..lines.len() - 2];
            for (line, expected_auto) in commands[..2].iter().zip([false, true]) {
                let Command::SetMicMode { auto, manual } = from_line::<Command>(line).unwrap()
                else {
                    panic!("wrong command");
                };
                assert_eq!(auto, expected_auto);
                assert!(
                    !manual,
                    "the UI must not request a manual microphone override"
                );
            }
            assert!(matches!(
                from_line::<Command>(commands[2]),
                Some(Command::SetDeviceControl {
                    id: 0x24,
                    value: 0x0200
                })
            ));
            assert!(matches!(
                from_line::<Command>(commands[3]),
                Some(Command::SetEq {
                    mode: 2,
                    low: 40,
                    mid: 50,
                    high: 60
                })
            ));
            assert!(matches!(
                from_line::<Command>(commands[4]),
                Some(Command::SetAutoPause { on: false })
            ));
            assert!(matches!(
                from_line::<Command>(commands[5]),
                Some(Command::RefreshDeviceSettings)
            ));
            for (line, expected) in commands[6..9].iter().zip([0, 25, 100]) {
                assert!(matches!(from_line::<Command>(line),
                    Some(Command::SetConversationVolume { percent }) if percent == expected));
            }
            assert!(matches!(from_line::<Command>(commands[9]),
                Some(Command::SetName { name, request_id: 42 }) if name == "Écouteurs 🎧"));
            assert!(matches!(
                from_line::<Command>(commands[10]),
                Some(Command::Ping)
            ));
            for (line, (expected_mode, expected_delay)) in commands[11..15].iter().zip([
                (MicMode::Automatic, 0),
                (MicMode::Automatic, 3600),
                (MicMode::KeepActive, 0),
                (MicMode::Manual, 0),
            ]) {
                assert!(matches!(from_line::<Command>(line),
                    Some(Command::SetMicPolicy { mode, stop_delay_secs })
                    if mode == expected_mode && stop_delay_secs == expected_delay));
            }
            for (line, expected) in commands[15..17].iter().zip([false, true]) {
                assert!(matches!(from_line::<Command>(line),
                    Some(Command::SetMicManualActive { active }) if active == expected));
            }
            assert!(matches!(
                from_line::<Command>(commands[17]),
                Some(Command::SetBleScanWhileConnected { enabled: true })
            ));
            for (line, expected) in commands[18..20].iter().zip([false, true]) {
                assert!(matches!(from_line::<Command>(line),
                    Some(Command::SetHeadTracking { on }) if on == expected));
            }
            assert!(matches!(
                from_line::<Command>(commands[20]),
                Some(Command::GetLoudSoundReduction)
            ));
            for (line, expected) in commands[21..23].iter().zip([false, true]) {
                assert!(matches!(from_line::<Command>(line),
                    Some(Command::SetLoudSoundReduction { on }) if on == expected));
            }
            assert!(matches!(
                from_line::<Command>(commands[23]),
                Some(Command::ToggleMicManual)
            ));
            for (line, expected) in commands[24..27].iter().zip([
                PlaybackMode::Windows,
                PlaybackMode::Aac,
                PlaybackMode::AacEld,
            ]) {
                assert!(
                    matches!(from_line::<Command>(line), Some(Command::SetPlaybackMode { mode }) if mode == expected)
                );
            }
            assert!(matches!(
                from_line::<Command>(commands[27]),
                Some(Command::RetryPlayback)
            ));
            for (line, (expected_mode, expected_best, expected_kbps)) in
                commands[28..32].iter().zip([
                    (PlaybackMode::Aac, true, 512),
                    (PlaybackMode::Aac, false, 320),
                    (PlaybackMode::AacEld, true, 256),
                    (PlaybackMode::AacEld, false, 128),
                ])
            {
                assert!(
                    matches!(from_line::<Command>(line), Some(Command::SetPlaybackBitrate {mode, best_effort, kbps, variable: false}) if mode == expected_mode && best_effort == expected_best && kbps == expected_kbps)
                );
            }
            assert!(matches!(
                from_line::<Command>(commands[32]),
                Some(Command::SetPlaybackBitrate {
                    mode: PlaybackMode::AacEld,
                    best_effort: true,
                    kbps: 256,
                    variable: true
                })
            ));
            assert!(matches!(
                from_line::<Command>(commands[33]),
                Some(Command::GetTransparency)
            ));
            for (line, enabled) in commands[34..36].iter().zip([false, true]) {
                assert!(
                    matches!(from_line::<Command>(line), Some(Command::SetTransparency { settings, expected })
                    if settings == transparency(enabled) && settings.is_valid() && expected == Some(transparency(false)))
                );
            }
            assert!(matches!(
                from_line::<Command>(commands[36]),
                Some(Command::GetHearingAid)
            ));
            for (line, expected) in commands[37..39].iter().zip([0, 73]) {
                assert!(
                    matches!(from_line::<Command>(line), Some(Command::SetHearingAid {
                    request_id, on: true, expected: Some(baseline), settings: Some(settings),
                }) if request_id == expected && baseline == hearing_settings() && settings == hearing_settings())
                );
                let mut legacy: serde_json::Value = serde_json::from_str(line).unwrap();
                legacy.as_object_mut().unwrap().remove("request_id");
                assert!(matches!(
                    serde_json::from_value::<Command>(legacy),
                    Ok(Command::SetHearingAid { request_id: 0, .. })
                ));
            }
            assert!(matches!(
                from_line::<Command>(commands[39]),
                Some(Command::SetHearingAid {
                    request_id: 74,
                    on: false,
                    expected: None,
                    settings: None,
                })
            ));
            for (line, expected) in commands[40..46].iter().zip([
                None,
                Some(60),
                Some(80),
                Some(100),
                Some(120),
                Some(140),
            ]) {
                assert!(matches!(from_line::<Command>(line),
                    Some(Command::SetPlaybackBuffer { target_ms, restore_ms })
                    if target_ms == expected && restore_ms == expected.map(|_| 150)));
            }
            assert!(matches!(from_line::<Command>(commands[46]),
                Some(Command::PairingCompleted { address }) if address == "12:34:56:78:9a:bc"));
            assert!(matches!(
                from_line::<Command>(commands[47]),
                Some(Command::Connect { device_id: None })
            ));
            assert!(matches!(from_line::<Command>(commands[48]),
                Some(Command::Connect { device_id: Some(id) }) if id == "001122334455"));
            let modes: Vec<_> = commands
                .iter()
                .filter_map(|line| match from_line::<Command>(line) {
                    Some(Command::SetHearingAidMode { request_id, on }) => {
                        Some((false, request_id, on))
                    }
                    Some(Command::SetMediaAssist { request_id, on }) => {
                        Some((true, request_id, on))
                    }
                    _ => None,
                })
                .collect();
            assert!(matches!(from_line::<Command>(commands.last().unwrap()), Some(Command::GetHardwareDiagnostics)));
            assert_eq!(
                modes,
                [
                    (false, 81, false),
                    (true, 82, false),
                    (false, 81, true),
                    (true, 82, true)
                ]
            );
            for malformed in [
                r#"{"cmd":"pairing_completed"}"#,
                r#"{"cmd":"pairing_completed","address":42}"#,
            ] {
                assert!(from_line::<Command>(malformed).is_none());
            }
            println!("Rust: C# state, microphone and device-setting command roundtrips passed");
        }
        _ => panic!("expected emit or check"),
    }
}
