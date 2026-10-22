use ntpods_ipc::{canonical_percent, ConversationPolicy, ConversationVolumeMode};

#[test]
fn percentage_wire_values_are_bounded_canonical_decimal_strings() {
    for value in ["0", "100", "0.000000001", "19.8", "99.999999999"] {
        assert!(canonical_percent(value), "{value}");
    }
    for value in [
        "",
        " 20",
        "20 ",
        "00",
        "01",
        "1.",
        ".1",
        "+1",
        "-1",
        "1e1",
        "NaN",
        "Infinity",
        "100.1",
        "101",
        "0.0000000001",
        "19.80",
        "0000000000000",
    ] {
        assert!(!canonical_percent(value), "{value}");
    }
}

#[test]
fn stage_two_cannot_increase_volume_and_legacy_values_are_preserved() {
    let legacy = ConversationPolicy::from_legacy(33);
    assert_eq!(legacy.first, "33");
    assert_eq!(legacy.second, "19.8");
    assert!(!legacy.pause_media);
    assert!(legacy.is_valid());
    let invalid = ConversationPolicy {
        second: "33.000000001".into(),
        ..legacy.clone()
    };
    assert!(!invalid.is_valid());
    assert!(invalid.target(0.7, 2).is_none());
    let unknown =
        r#"{"mode":"relative","first":"43","second":"20","pause_media":false,"extra":true}"#;
    assert!(serde_json::from_str::<ConversationPolicy>(unknown).is_err());
}

#[test]
fn absolute_and_relative_policies_never_raise_the_original_volume() {
    for mode in [
        ConversationVolumeMode::AbsCap,
        ConversationVolumeMode::Relative,
    ] {
        let policy = ConversationPolicy {
            mode,
            first: "43".into(),
            second: "20".into(),
            pause_media: true,
        };
        for step in 0..=100 {
            let original = step as f32 / 100.0;
            let first = policy.target(original, 1).unwrap();
            let second = policy.target(original, 2).unwrap();
            assert!(second <= first && first <= original);
            assert_eq!(policy.target(original, 3), Some(first));
        }
        for invalid in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
            assert!(policy.target(invalid, 1).is_none());
        }
    }
    let cap = ConversationPolicy {
        mode: ConversationVolumeMode::AbsCap,
        first: "43".into(),
        second: "20".into(),
        pause_media: false,
    };
    let relative = ConversationPolicy {
        mode: ConversationVolumeMode::Relative,
        ..cap.clone()
    };
    assert!((cap.target(0.5, 1).unwrap() - 0.43).abs() < 0.00001);
    assert!((relative.target(0.5, 1).unwrap() - 0.215).abs() < 0.00001);
}
