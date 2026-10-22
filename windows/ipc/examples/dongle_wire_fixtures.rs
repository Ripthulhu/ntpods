use ntpods_ipc::{
    from_line, to_line, Battery, Command, DongleAudioState, DongleHealthState,
    DonglePerformanceState, DongleState, Event, HardwareCheckState, HardwareDiagnosticEar,
    HardwareDiagnostics, HearingAidSettings, HearingMetadata, NearbyFinderState, Snapshot,
};

fn state() -> DongleState {
    DongleState {
        present: true,
        session: u64::MAX,
        generation: u32::MAX,
        pair_token: u64::MAX - 2,
        selected: true,
        connected: true,
        controls_ready: true,
        streaming: true,
        phone_connected: true,
        microphone_active: true,
        phone_pairing: true,
        usb_playback: true,
        anc: 3,
        battery: Battery {
            left: Some(91),
            right: Some(73),
            case: None,
            left_charging: true,
            ..Default::default()
        },
        battery_age_ms: [Some(1234), Some(u32::MAX - 1), None],
        codec: 3,
        capabilities: 31,
        pending: true,
        error: Some("timeout".into()),
        health: DongleHealthState {
            available: true,
            model: "A3063".into(),
            firmware: "9A348".into(),
            earbuds_in_case: Some(true),
            hearing_aid_enabled: Some(true),
            hearing_aid_revision: u64::MAX,
            hearing_aid_enrolled: Some(true),
            media_assist_enabled: Some(false),
            media_assist_revision: u64::MAX - 1,
            media_assist_available: true,
            hearing_aid_settings: Some(HearingAidSettings {
                left_eq: [12.25; 8],
                right_eq: [19.5; 8],
                left_amplification: -0.25,
                right_amplification: 0.125,
                left_tone: -0.5,
                right_tone: 0.75,
                left_conversation_boost: true,
                right_conversation_boost: false,
                left_ambient_noise_reduction: 0.25,
                right_ambient_noise_reduction: 0.5,
                own_voice: 0.75,
            }),
            hearing_aid_editable: true,
            hearing_aid_pending: true,
            hearing_aid_error: Some("readback".into()),
            hardware_diagnostics: Some(HardwareDiagnostics {
                read_at_unix_seconds: 1_700_000_001,
                left: HardwareDiagnosticEar {
                    measured_at_unix_seconds: 1_700_000_000,
                    first_use_unix_seconds: Some(1_600_000_000),
                    general: 0,
                    measurement: 0,
                    component_failures: Some(0),
                    unknown_bits: 0,
                },
                right: HardwareDiagnosticEar {
                    measured_at_unix_seconds: 1_699_999_999,
                    first_use_unix_seconds: None,
                    general: 1,
                    measurement: 1,
                    component_failures: Some(128),
                    unknown_bits: 64,
                },
            }),
            hardware_diagnostics_pending: true,
            hardware_diagnostics_error: Some("diagnostics".into()),
            hearing_metadata: Some(HearingMetadata {
                read_at_unix_seconds: 1_700_000_002,
                test_mode_raw: Some(0),
                dosimetry_sensitivity_raw: Some(-12.5),
                dosimetry_volume_curve_raw: Some([0.125; 17]),
            }),
            hearing_metadata_pending: true,
            hearing_metadata_error: Some("metadata".into()),
            hardware_check: Some(HardwareCheckState {
                request_id: u64::MAX,
                phase: "waiting_result".into(),
                left_updated: true,
                right_updated: false,
                error: None,
            }),
            device_controls: [(0x16, 0x0501), (0x28, 2)].into(),
            device_controls_requested: [(0x23, 1)].into(),
            device_controls_pending: [(0x0d, 3)].into(),
            device_settings_pending: true,
            device_settings_error: Some("setting".into()),
            allow_off_requested: Some(true),
            adaptive_strength_requested: Some(51),
            anc_unconfirmed: true,
            audio: Some(DongleAudioState {
                requested_codec: 3,
                codec: 3,
                variable: true,
                requested_bitrate: 174000,
                bitrate: 173600,
                game_mode_requested: true,
                game_mode_state: "unconfirmed".into(),
                input: 2,
                pending: true,
                streaming: false,
                microphone_active: true,
                game_mode_supported: true,
                reported_delay_tenths_ms: Some(65534),
                priming_ms: 100,
            }),
            performance: Some(DonglePerformanceState {
                input: 1,
                codec: 3,
                flags: 15,
                uptime_ms: Some(u32::MAX - 1),
                pcm_used: Some(100),
                pcm_capacity: Some(1000),
                pcm_high_water: Some(200),
                underflow_frames: Some(123),
                dropped_frames: Some(12),
                encoded_units: Some(u32::MAX - 1),
                encode_us: Some(2500),
                encode_max_us: Some(3000),
                send_failures: Some(1),
                recoveries: Some(2),
                heap_capacity: Some(64000),
                heap_allocated: Some(1000),
                heap_remaining: Some(63000),
                heap_arena: None,
            }),
            nearby_finder: NearbyFinderState {
                active: true,
                paused: true,
                available_identity: true,
                rssi: Some(-67),
                trend: "weaker".into(),
                last_heard_age_ms: Some(u64::MAX),
                revision: u64::MAX,
            },
            ..Default::default()
        },
        ..Default::default()
    }
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    match args[1].as_str() {
        "emit" => std::fs::write(
            &args[2],
            [state(), DongleState::default()]
                .into_iter()
                .map(|dongle| {
                    to_line(&Event::State(Snapshot {
                        dongle,
                        ..Default::default()
                    }))
                })
                .collect::<String>(),
        )
        .unwrap(),
        "check" => {
            let text = std::fs::read_to_string(&args[2]).unwrap();
            let lines: Vec<_> = text.lines().collect();
            assert_eq!(lines.len(), 8);
            for (line, expected) in lines[..2].iter().zip([state(), DongleState::default()]) {
                let Some(Event::State(snapshot)) = from_line::<Event>(line) else {
                    panic!("wrong event")
                };
                assert_eq!(snapshot.dongle, expected);
            }
            assert!(matches!(
                from_line::<Command>(lines[2]),
                Some(Command::DongleControl {
                    session: u64::MAX,
                    generation: u32::MAX,
                    opcode: 3,
                    argument: 2
                })
            ));
            let target = format!("dongle:{}:{}", u64::MAX, u32::MAX);
            for (line, kind) in lines[3..].iter().zip([0, 1, 2, 3, 4]) {
                let Some(Command::DongleHealth {
                    session: u64::MAX,
                    generation: u32::MAX,
                    command,
                }) = from_line::<Command>(line)
                else {
                    panic!("health command lost USB target")
                };
                match (kind, *command) {
                    (
                        0,
                        Command::SetHearingAidMode {
                            request_id: u64::MAX,
                            on: false,
                        },
                    ) => {}
                    (1, Command::GetHearingMetadata { device_id }) if device_id == target => {}
                    (
                        2,
                        Command::StartHardwareCheck {
                            request_id: u64::MAX,
                            device_id,
                            buds_in_case_confirmed: true,
                        },
                    ) if device_id == target => {}
                    (
                        3,
                        Command::SetDongleAudioProfile {
                            codec: 3,
                            variable: true,
                            bitrate: 174000,
                        },
                    ) => {}
                    (4, Command::GetDonglePerformance) => {}
                    _ => panic!("health command changed"),
                }
            }
            println!(
                "Rust/C#: USB dongle state and commands roundtrip without narrowing identifiers"
            );
        }
        _ => panic!("expected emit or check"),
    }
}
