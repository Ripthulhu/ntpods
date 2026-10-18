//! Shared with WinUI; a device report confirms a value, not every model's capabilities.
use ntpods_ipc::Snapshot;
use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Deserialize)]
pub struct Profile {
    pub models: Vec<String>,
    pub listening: bool,
    pub adaptive: bool,
    pub allow_off_option: bool,
    pub heart_rate: bool,
    pub head_tracking: bool,
    pub high_quality_mic: bool,
    pub custom_eq: bool,
    pub hearing_aid: bool,
    pub custom_transparency: bool,
    pub loud_sound_reduction: bool,
    pub controls: Vec<u8>,
}

pub fn find(model: &str) -> Option<&'static Profile> {
    static MODELS: OnceLock<Vec<Profile>> = OnceLock::new();
    MODELS
        .get_or_init(|| {
            serde_json::from_str(include_str!("../../device-models.json"))
                .expect("embedded device catalog must be valid")
        })
        .iter()
        .find(|p| {
            p.models
                .iter()
                .any(|m| m.eq_ignore_ascii_case(model.trim()))
        })
}

pub fn firmware_major(firmware: &str) -> u32 {
    let firmware = firmware.trim();
    let digits = firmware.bytes().take_while(u8::is_ascii_digit).count();
    let major = firmware[..digits].parse::<u32>().unwrap_or(0);
    if !(1..=99).contains(&major) {
        return 0;
    }
    let rest = &firmware.as_bytes()[digits..];
    let public_build = rest.first().is_some_and(u8::is_ascii_uppercase) && {
        let build = &rest[1..];
        let length = build.iter().take_while(|b| b.is_ascii_digit()).count();
        length > 0
            && (length == build.len()
                || (length + 1 == build.len() && build[length].is_ascii_lowercase()))
    };
    let parts: Vec<_> = firmware.split('.').collect();
    let legacy_version = parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.bytes().all(|b| b.is_ascii_digit())
                && part.parse::<u8>().is_ok()
        });
    if public_build || legacy_version {
        major
    } else {
        0
    }
}

pub fn microphone(s: &Snapshot) -> bool {
    find(&s.model).is_some_and(|p| p.high_quality_mic) && firmware_major(&s.firmware) >= 8
}

pub fn equalizer(s: &Snapshot) -> bool {
    find(&s.model).is_some_and(|p| p.custom_eq) && firmware_major(&s.firmware) >= 9
}

pub fn adaptive(s: &Snapshot) -> bool {
    find(&s.model).is_some_and(|p| p.adaptive) && firmware_major(&s.firmware) >= 6
}

pub fn heart_rate(s: &Snapshot) -> bool {
    find(&s.model).is_some_and(|p| p.heart_rate)
}

pub fn head_tracking(s: &Snapshot) -> bool {
    find(&s.model).is_some_and(|p| p.head_tracking) && firmware_major(&s.firmware) > 0
}

pub fn hearing_aid(s: &Snapshot) -> bool {
    find(&s.model).is_some_and(|p| p.hearing_aid)
}

pub fn loud_sound_reduction(s: &Snapshot) -> bool {
    find(&s.model).is_some_and(|p| p.loud_sound_reduction)
}

pub fn allow_off(s: &Snapshot) -> bool {
    if let Some(profile) = find(&s.model) {
        if !profile.listening {
            return false;
        }
        if !profile.allow_off_option {
            return s.device_controls.get(&0x34) != Some(&2);
        }
    }
    if let Some(requested) = s.allow_off_requested {
        return requested;
    }
    if let Some(&reported) = s.device_controls.get(&0x34) {
        if matches!(reported, 1 | 2) {
            return reported == 1;
        }
    }
    s.device_controls.get(&0x0D) == Some(&1)
        || (find(&s.model).is_some() && (1..=6).contains(&firmware_major(&s.firmware)))
}

pub fn control(s: &Snapshot, id: u8, value: u32) -> bool {
    if !crate::device_settings::valid_control(id, value) {
        return false;
    }
    let profile = find(&s.model);
    let supported = match profile {
        Some(p) => p.controls.contains(&id),
        None => matches!(
            id,
            0x01 | 0x0A
                | 0x0D
                | 0x16
                | 0x17
                | 0x18
                | 0x1A
                | 0x1B
                | 0x1F
                | 0x23
                | 0x24
                | 0x31
                | 0x3B
                | 0x40
        ),
    };
    if !supported {
        return false;
    }
    if id == 0x16
        && profile.is_some_and(|p| !p.listening)
        && (value & 0xFF == 1 || (value >> 8) & 0xFF == 1)
    {
        return false;
    }
    if id == 0x0D {
        if value == 4 && !adaptive(s) {
            return false;
        }
        if value == 1 && !allow_off(s) {
            return false;
        }
    }
    if id == 0x1A && ((value & 8 != 0 && !adaptive(s)) || (value & 1 != 0 && !allow_off(s))) {
        return false;
    }
    if id == 0x2E && !adaptive(s) {
        return false;
    }
    if id == 0x35 && firmware_major(&s.firmware) < 8 {
        return false;
    }
    s.device_controls.contains_key(&id)
        || explicit_choice(s, id, value)
        || (id == 0x34 && crate::device_settings::supports_silent_allow_off(&s.model))
}

pub fn explicit_choice(s: &Snapshot, id: u8, value: u32) -> bool {
    crate::device_settings::explicit_choice_id(id)
        && find(&s.model).is_some_and(|p| p.controls.contains(&id))
        && crate::device_settings::valid_control(id, value)
}

pub fn unreported_choice(s: &Snapshot, id: u8, value: u32) -> bool {
    !s.device_controls.contains_key(&id) && explicit_choice(s, id, value) && control(s, id, value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn firmware_gates_reject_internal_and_unrecognized_versions() {
        for (value, major) in [
            ("6F21", 6),
            ("9A5347a", 9),
            ("6.8.8", 6),
            (" 10A1 ", 10),
            ("6619472", 0),
            ("90.3431.6807", 0),
            ("9", 0),
            ("9A", 0),
            ("9A348garbage", 0),
            ("９A348", 0),
            ("", 0),
        ] {
            assert_eq!(firmware_major(value), major, "{value}");
        }
        let s = device("A3063", "6619472");
        assert!(!microphone(&s) && !equalizer(&s) && !head_tracking(&s));
    }
    fn device(model: &str, firmware: &str) -> Snapshot {
        Snapshot {
            model: model.into(),
            firmware: firmware.into(),
            device_controls: [(0x0D, 2), (0x1A, 6), (0x28, 1), (0x0A, 1)].into(),
            ..Snapshot::default()
        }
    }

    #[test]
    fn pro_one_has_three_modes_and_no_newer_streams() {
        let s = device("A2083", "6F21");
        for mode in 1..=3 {
            assert!(control(&s, 0x0D, mode));
        }
        assert!(!control(&s, 0x0D, 4));
        assert!(control(&s, 0x1A, 7));
        assert!(!control(&s, 0x1A, 14));
        assert!(!control(&s, 0x28, 1));
        assert!(!microphone(&s));
        assert!(!equalizer(&s));
        assert!(!heart_rate(&s));
        assert!(head_tracking(&s));
    }

    #[test]
    fn model_and_firmware_both_gate_microphone_and_eq() {
        for model in ["A2698", "A3049", "A3065", "A3050", "A3454"] {
            assert!(!microphone(&device(model, "7E108")));
            assert!(microphone(&device(model, "8B34")));
            assert!(!equalizer(&device(model, "8B34")));
            assert!(equalizer(&device(model, "9A348")));
        }
        for model in [
            "A1523", "A2031", "A2564", "A2084", "A2096", "A3184", "future",
        ] {
            assert!(!microphone(&device(model, "99A1")));
            assert!(!equalizer(&device(model, "99A1")));
            assert!(!heart_rate(&device(model, "99A1")));
        }
        assert!(heart_rate(&device("A3065", "9A348")));
        assert!(!microphone(&device("A3063", "")));
        assert!(!microphone(&device("A3063", "unknown9")));
    }

    #[test]
    fn legacy_max_and_unknown_are_not_pro_three() {
        for model in ["A2096", "A3184"] {
            let s = device(model, "7E108");
            assert!(control(&s, 0x0D, 1));
            assert!(!control(&s, 0x0D, 4));
        }
        let s = device("A1523", "6.8.8");
        assert!(!control(&s, 0x0D, 2));
        let s = device("future", "9A348");
        assert!(control(&s, 0x0A, 2));
        assert!(control(&s, 0x0D, 2));
        assert!(!control(&s, 0x0D, 4));
        assert!(!control(&s, 0x28, 1));
        assert!(!head_tracking(&s));
    }

    #[test]
    fn newer_off_option_and_reports_are_respected() {
        let mut s = device("A3063", "9A348");
        assert!(!control(&s, 0x0D, 1));
        assert!(control(&s, 0x34, 1));
        s.allow_off_requested = Some(true);
        assert!(control(&s, 0x0D, 1));
        assert!(control(&s, 0x31, 1));
        s.device_controls.insert(0x31, 2);
        assert!(control(&s, 0x31, 1));
    }

    #[test]
    fn only_catalogued_choices_can_be_sent_without_a_report() {
        let known = Snapshot {
            model: "A3063".into(),
            firmware: "9A348".into(),
            ..Default::default()
        };
        for (id, value) in [
            (0x01, 1),
            (0x0A, 2),
            (0x16, 0x0101),
            (0x1A, 6),
            (0x23, 2),
            (0x31, 1),
            (0x40, 50),
        ] {
            assert!(unreported_choice(&known, id, value), "id {id:#x}");
        }
        assert!(!unreported_choice(&known, 0x25, 1));
        assert!(!control(&known, 0x25, 1));
        assert!(!control(&known, 0x1A, 9));
        let unknown = Snapshot {
            model: "future".into(),
            ..known
        };
        assert!(!unreported_choice(&unknown, 0x23, 2));
        assert!(!control(&unknown, 0x23, 2));
    }

    #[test]
    fn airpods_five_variants_keep_case_and_swipe_controls_separate() {
        for model in ["A3531", "A3532", "A3533", "A3439", "A3440", "A3441"] {
            let mut s = device(model, "9A350");
            assert!(adaptive(&s) && head_tracking(&s) && microphone(&s) && equalizer(&s));
            assert!(!heart_rate(&s) && !hearing_aid(&s) && !loud_sound_reduction(&s));
            assert!(control(&s, 0x0D, 1));
            s.device_controls.insert(0x34, 2);
            assert!(!allow_off(&s) && !control(&s, 0x0D, 1));
            s.device_controls.insert(0x34, 1);
            assert!(control(&s, 0x0D, 1));
            s.device_controls.insert(0x34, 0);
            assert!(control(&s, 0x0D, 1));
            let wireless_case = matches!(model, "A3439" | "A3440" | "A3441");
            for (id, value) in [(0x23, 1), (0x25, 1), (0x31, 1), (0x40, 50)] {
                assert_eq!(
                    control(&s, id, value),
                    wireless_case && matches!(id, 0x23 | 0x31 | 0x40)
                );
                s.device_controls.insert(id, value);
                assert_eq!(control(&s, id, value), wireless_case);
            }
            assert!(!control(&s, 0x35, 1));
            s.device_controls.insert(0x35, 2);
            assert!(control(&s, 0x35, 1));
        }
    }

    #[test]
    fn legacy_charging_and_max_features_follow_model_and_report_support() {
        for model in ["A1523", "A1722", "A2032", "A2031"] {
            let mut s = device(model, "6.8.8");
            s.device_controls.insert(0x3B, 1);
            assert!(!control(&s, 0x3B, 1));
        }
        for model in ["A2096", "A3184", "A3454"] {
            let mut s = device(model, "8E258");
            s.device_controls.insert(0x01, 0);
            for side in 0..=2 {
                assert!(!control(&s, 0x01, side));
            }
            assert!(!control(&s, 0x1C, 1));
            s.device_controls.insert(0x1C, 1);
            assert!(control(&s, 0x1C, 2));
            assert!(!control(&s, 0x24, 0x0200));
            s.device_controls.insert(0x24, 0x0200);
            assert!(control(&s, 0x24, 0x0300));
            s.device_controls.insert(0x35, 2);
            assert_eq!(control(&s, 0x35, 1), model == "A3454");
            assert_eq!(loud_sound_reduction(&s), model == "A3454");
            assert!(!hearing_aid(&s));
            s.firmware = "7E108".into();
            assert!(!control(&s, 0x35, 1));
        }
    }

    #[test]
    fn powerbeats_reports_do_not_enable_unverified_streams() {
        let mut s = device("A3157", "99A1");
        assert!(control(&s, 0x0A, 1));
        for mode in 1..=3 {
            assert!(control(&s, 0x0D, mode));
        }
        assert!(!control(&s, 0x0D, 4));
        for (id, value) in [(0x28, 1), (0x35, 1), (0x34, 1), (0x25, 1)] {
            s.device_controls.insert(id, value);
            assert!(!control(&s, id, value));
        }
        assert!(!microphone(&s) && !equalizer(&s) && !head_tracking(&s));
        assert!(!heart_rate(&s) && !hearing_aid(&s) && !loud_sound_reduction(&s));
    }

    #[test]
    fn legacy_firmware_off_and_non_anc_stem_settings() {
        let mut s = device("A2698", "6F21");
        assert!(control(&s, 0x0D, 1));
        s.device_controls.insert(0x34, 2);
        assert!(!control(&s, 0x0D, 1));
        assert!(!control(&device("A2698", "7B19"), 0x0D, 1));
        assert!(!control(&device("A2698", ""), 0x0D, 1));
        let mut s = device("A3053", "9A348");
        s.device_controls.insert(0x16, 0x0505);
        assert!(control(&s, 0x16, 0x0505));
        assert!(!control(&s, 0x16, 0x0105));
        assert!(!control(&s, 0x16, 0x0501));
    }

    #[test]
    fn catalog_has_unique_models_and_valid_control_ids() {
        let profiles: Vec<Profile> =
            serde_json::from_str(include_str!("../../device-models.json")).unwrap();
        let mut models = std::collections::BTreeSet::new();
        for p in profiles {
            for model in p.models {
                assert!(models.insert(model));
            }
            for id in p.controls {
                assert!(
                    (0..=0xFFFF).any(|v| crate::device_settings::valid_control(id, v)),
                    "{id:02X}"
                );
            }
        }
    }
}
