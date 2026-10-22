use ntpods_ipc::{from_line, to_line, Command, CompanionPreference, CompanionPreferences, Event};
use serde_json::json;

fn preferences() -> CompanionPreferences {
    let values = [
        ("host_auto_pause", json!(true)),
        ("theme", json!("dark")),
        ("conversation_policy", json!({"mode":"relative","first":"43.25","second":"19.8","pause_media":true})),
        ("connection_popup_enabled", json!(false)),
        ("cache_disconnected_battery", json!(false)),
        ("call_takeover_enabled", json!(true)),
        ("call_return_enabled", json!(true)),
        ("desired_microphone_side", json!("right")),
        ("desired_ear_detection", json!("unset")),
        ("desired_hold_actions", json!({"left":"listening","right":"assistant"})),
        ("desired_noise_modes_cycle", json!("0b")),
        ("desired_swipe_duration", json!("2")),
        ("desired_case_sounds", json!("off")),
        ("desired_case_volume", json!("100")),
        ("desired_allow_off", json!("on")),
    ];
    CompanionPreferences {
        device_id: Some("001122334455".into()),
        records: values.into_iter().map(|(key, value)| CompanionPreference {
            key: key.into(), rev: "ffffffffffffffff".into(), origin: "android".into(), value,
        }).collect(),
        local_call_consent: false,
        peer_prefs: true,
        peer_calls: true,
        error: Some("Could not save \"setting\".\nTry again.".into()),
    }
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let path = &args[2];
    match args[1].as_str() {
        "emit" => {
            let lines = [preferences(), CompanionPreferences::default()].map(|preferences|
                to_line(&Event::CompanionPreferences { preferences }));
            std::fs::write(path, lines.concat()).unwrap();
        }
        "check" => {
            let text = std::fs::read_to_string(path).unwrap();
            let lines: Vec<_> = text.lines().collect();
            assert_eq!(lines.len(), 5);
            for (line, expected) in lines[..2].iter().zip([preferences(), CompanionPreferences::default()]) {
                let Some(Event::CompanionPreferences { preferences }) = from_line::<Event>(line) else { panic!("wrong event") };
                assert_eq!(preferences, expected);
            }
            assert!(matches!(from_line::<Command>(lines[2]), Some(Command::CompanionSetPreference { key, value, device_id: None, request_id: u64::MAX })
                if key == "theme" && value == json!("light")));
            assert!(matches!(from_line::<Command>(lines[3]), Some(Command::CompanionSetPreference { key, value, device_id: Some(id), request_id: 9_007_199_254_740_993 })
                if key == "conversation_policy" && value == json!({"mode":"abs_cap","first":"33","second":"19.8","pause_media":false}) && id == "001122334455"));
            assert!(matches!(from_line::<Command>(lines[4]), Some(Command::CompanionCallConsent { enabled: true, device_id, request_id: u64::MAX })
                if device_id == "001122334455"));
            println!("Rust: shared preference and local call-consent IPC roundtrips passed");
        }
        _ => panic!("expected emit or check"),
    }
}
