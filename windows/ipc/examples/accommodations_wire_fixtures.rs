use ntpods_ipc::{
    from_line, to_line, AccommodationsOptimization, AccommodationsPreset, AccommodationsResult,
    AccommodationsSelection, AccommodationsTuning, Command, Event, HeadphoneAccommodationsState, Snapshot,
};

fn states() -> Vec<Snapshot> {
    let unknown = HeadphoneAccommodationsState {
        session: 17,
        ..Default::default()
    };
    let reported = HeadphoneAccommodationsState {
        session: 17,
        revision: 4,
        phone: Some(true),
        media: Some(false),
        preset: Some(AccommodationsPreset {
            tuning: AccommodationsTuning::BalancedTone,
            optimization: AccommodationsOptimization::Slight,
        }),
        editable: true,
        result: Some(AccommodationsResult::Reported),
        ..Default::default()
    };
    vec![
        None,
        Some(unknown),
        Some(reported.clone()),
        Some(HeadphoneAccommodationsState {
            pending: true,
            editable: false,
            ..reported.clone()
        }),
        Some(HeadphoneAccommodationsState {
            result: Some(AccommodationsResult::Unconfirmed),
            editable: false,
            error: Some("Sent; refresh to verify.".into()),
            ..reported.clone()
        }),
        Some(HeadphoneAccommodationsState {
            result: Some(AccommodationsResult::Confirmed),
            ..reported.clone()
        }),
        Some(HeadphoneAccommodationsState {
            edit_version: u64::MAX,
            audiogram_available: true,
            requested: Some(AccommodationsSelection { phone: false, media: true, preset: None, audiogram: true }),
            result: Some(AccommodationsResult::Sent),
            ..reported
        }),
    ]
    .into_iter()
    .map(|headphone_accommodations| Snapshot {
        device_id: "001122334455".into(),
        headphone_accommodations,
        ..Default::default()
    })
    .collect()
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    match args[1].as_str() {
        "emit" => {
            let mut lines = String::new();
            for (index, state) in states().into_iter().enumerate() {
                let mut event = serde_json::to_value(Event::State(state)).unwrap();
                if index == 0 {
                    event
                        .as_object_mut()
                        .unwrap()
                        .remove("headphone_accommodations");
                }
                lines.push_str(&to_line(&event));
            }
            std::fs::write(&args[2], lines).unwrap();
        }
        "check" => {
            let text = std::fs::read_to_string(&args[2]).unwrap();
            let lines: Vec<_> = text.lines().collect();
            assert_eq!(lines.len(), 10);
            for (line, expected) in lines[..7].iter().zip(states()) {
                let Event::State(actual) = from_line::<Event>(line).unwrap() else {
                    panic!("expected state");
                };
                assert_eq!(
                    actual.headphone_accommodations,
                    expected.headphone_accommodations
                );
            }
            assert!(
                matches!(from_line::<Command>(lines[7]), Some(Command::GetHeadphoneAccommodations { device_id }) if device_id == "001122334455")
            );
            assert!(
                matches!(from_line::<Command>(lines[8]), Some(Command::SetHeadphoneAccommodations {
                request_id: u64::MAX, device_id, session: 17, revision: 4, edit_version: u64::MAX, audiogram: true, phone: Some(false), media: None, preset: None }) if device_id == "001122334455")
            );
            assert!(
                matches!(from_line::<Command>(lines[9]), Some(Command::SetHeadphoneAccommodations {
                request_id: 81, device_id, session: 17, revision: 4, edit_version: 0, audiogram: false, phone: None, media: None,
                preset: Some(AccommodationsPreset { tuning: AccommodationsTuning::Brightness,
                    optimization: AccommodationsOptimization::Strong }) }) if device_id == "001122334455")
            );
            for malformed in [
                r#"{"cmd":"get_headphone_accommodations"}"#,
                r#"{"cmd":"set_headphone_accommodations","request_id":1,"device_id":"001122334455","session":17}"#,
                r#"{"cmd":"set_headphone_accommodations","request_id":1,"device_id":"001122334455","session":17,"revision":4,"preset":{"tuning":"audiogram","optimization":"slight"}}"#,
                r#"{"cmd":"set_headphone_accommodations","request_id":1,"device_id":"001122334455","session":17,"revision":4,"phone":1}"#,
            ] {
                assert!(from_line::<Command>(malformed).is_none(), "{malformed}");
            }
            println!("Headphone Accommodations IPC roundtrip checks passed.");
        }
        _ => panic!("expected emit or check"),
    }
}
