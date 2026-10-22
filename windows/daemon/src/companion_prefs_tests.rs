use super::*;

struct Fixture {
    directory: PathBuf,
    path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let directory = std::env::temp_dir().join(format!(
            "ntpods-companion-prefs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("preferences.json");
        Self { directory, path }
    }
    fn load(&self) -> Store {
        Store::load_at(Some(self.path.clone()), false)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(self.directory.join("blocked"));
        let _ = std::fs::remove_dir(&self.directory);
    }
}

#[test]
fn preference_payloads_cannot_address_credentials_permissions_or_unknown_controls() {
    for key in [
        "secret",
        "peer",
        "device",
        "microphone_enabled",
        "permission",
        "volume",
        "audiogram",
        "held_away",
        "unknown",
    ] {
        assert!(!valid(key, &json!(true)), "{key}");
    }
    assert!(valid("host_auto_pause", &json!(false)));
    assert!(!valid("host_auto_pause", &json!("false")));
    assert!(!valid("theme", &json!("unknown")));
    assert!(!valid("desired_case_volume", &json!(101)));
    assert!(!valid("desired_case_volume", &json!("101")));
    assert!(!valid("desired_noise_modes_cycle", &json!("01")));
    assert!(!valid("desired_noise_modes_cycle", &json!("ff")));
    assert!(!valid(
        "desired_hold_actions",
        &json!({"left":"unset","right":"listening"})
    ));
    assert!(!valid(
        "desired_hold_actions",
        &json!({"left":"listening","right":"assistant","extra":true})
    ));
}

#[test]
fn desired_choices_round_trip_without_inventing_confirmed_values() {
    for (id, value) in [
        (1, 0),
        (1, 1),
        (1, 2),
        (10, 1),
        (10, 2),
        (22, 0x0105),
        (26, 6),
        (35, 2),
        (49, 1),
        (49, 2),
        (64, 0),
        (64, 100),
    ] {
        let (key, saved) = from_control(id, value).unwrap();
        assert_eq!(to_control(key, &saved), Some((id, Some(value))));
    }
    for key in [
        "desired_microphone_side",
        "desired_ear_detection",
        "desired_noise_modes_cycle",
        "desired_swipe_duration",
        "desired_case_sounds",
        "desired_case_volume",
    ] {
        assert!(valid(key, &json!("unset")));
        assert_eq!(to_control(key, &json!("unset")).unwrap().1, None);
    }
    assert_eq!(
        to_control(
            "desired_hold_actions",
            &json!({"left":"unset","right":"unset"})
        ),
        Some((22, None))
    );
    assert!(from_control(1, 4).is_none());
    assert!(from_control(22, 0x0100).is_none());
    assert!(from_control(64, 101).is_none());
    assert!(from_control(0xff, 1).is_none());
}

#[test]
fn revision_comparison_is_unsigned_and_deterministic_across_platforms() {
    let mut pc = record("host_auto_pause", json!(true));
    let mut android = pc.clone();
    android.origin = "android".into();
    assert!(rank(&pc) > rank(&android));
    android.rev = "8000000000000000".into();
    pc.rev = "7fffffffffffffff".into();
    assert!(rank(&android) > rank(&pc));
    pc.rev = "ffffffffffffffff".into();
    assert!(rank(&pc) > rank(&android));
    for revision in [
        "0",
        "0000000000000000",
        "000000000000000A",
        "10000000000000000",
        "-000000000000001",
    ] {
        pc.rev = revision.into();
        assert!(rank(&pc).is_none(), "{revision}");
    }
}

#[test]
fn merge_is_idempotent_has_no_echo_and_rejects_equal_stamp_collisions() {
    let fixture = Fixture::new();
    let mut store = fixture.load();
    store.ensure_device(1).unwrap();
    let mut incoming = record("host_auto_pause", json!(false));
    incoming.rev = "0000000000000002".into();
    incoming.origin = "android".into();
    assert!(matches!(
        store.merge(1, incoming.clone()).unwrap(),
        Merge::Applied
    ));
    assert!(store.take_dirty(1).is_empty());
    assert!(matches!(
        store.merge(1, incoming.clone()).unwrap(),
        Merge::Unchanged
    ));
    incoming.value = json!(true);
    assert!(store.merge(1, incoming).is_err());
    assert_eq!(
        fixture.load().get(1, "host_auto_pause").unwrap().value,
        json!(false)
    );
}

#[test]
fn concurrent_edits_choose_pc_on_tie_and_stale_messages_offer_the_winner_once() {
    let fixture = Fixture::new();
    let mut store = fixture.load();
    store.local(1, "host_auto_pause", json!(false)).unwrap();
    let winner = store.get(1, "host_auto_pause").unwrap().clone();
    let incoming = CompanionPreference {
        origin: "android".into(),
        value: json!(true),
        ..winner.clone()
    };
    let Merge::Stale(offered) = store.merge(1, incoming).unwrap() else {
        panic!("stale Android tie")
    };
    assert_eq!(offered, winner);
    assert_eq!(store.get(1, "host_auto_pause").unwrap().value, json!(false));
    let first_offer = store.take_dirty(1);
    assert_eq!(first_offer.len(), 1);
    assert!(store.take_dirty(1).is_empty());
}

#[test]
fn all_behavior_and_desired_choices_are_pair_scoped_while_theme_is_global() {
    let fixture = Fixture::new();
    let mut store = fixture.load();
    store.ensure_device(1).unwrap();
    store.ensure_device(2).unwrap();
    store.local(1, "host_auto_pause", json!(false)).unwrap();
    store
        .local(1, "call_takeover_enabled", json!(true))
        .unwrap();
    store
        .local(1, "desired_microphone_side", json!("left"))
        .unwrap();
    store.local(1, "theme", json!("dark")).unwrap();
    assert_eq!(store.get(2, "host_auto_pause").unwrap().value, json!(true));
    assert_eq!(
        store.get(2, "call_takeover_enabled").unwrap().value,
        json!(false)
    );
    assert_eq!(
        store.get(2, "desired_microphone_side").unwrap().value,
        json!("unset")
    );
    assert_eq!(store.get(2, "theme").unwrap().value, json!("dark"));
    assert!(!store.consent());
}

#[test]
fn failed_persistence_rolls_back_existing_and_new_records_without_offering_them() {
    let fixture = Fixture::new();
    let mut store = fixture.load();
    store.ensure_device(1).unwrap();
    let before = store.get(1, "host_auto_pause").unwrap().clone();
    let blocked = fixture.directory.join("blocked");
    std::fs::write(&blocked, b"not a directory").unwrap();
    store.storage = Some(blocked.join("preferences.json"));
    assert!(store.local(1, "host_auto_pause", json!(false)).is_err());
    assert_eq!(store.get(1, "host_auto_pause"), Some(&before));
    store.remove(1, "desired_case_volume");
    assert!(store.local(1, "desired_case_volume", json!("50")).is_err());
    assert!(store.get(1, "desired_case_volume").is_none());
    let mut incoming = record("desired_case_volume", json!("50"));
    incoming.rev = "0000000000000002".into();
    incoming.origin = "android".into();
    assert!(matches!(store.merge(1, incoming).unwrap(), Merge::Failed));
    assert!(store.get(1, "desired_case_volume").is_none());
    assert!(store.take_dirty(1).is_empty());
    assert!(store.set_consent(true).is_err());
    assert!(!store.consent());
}

#[test]
fn seeds_are_durable_and_corruption_does_not_fabricate_revision_one() {
    let fixture = Fixture::new();
    let mut store = fixture.load();
    assert!(fixture.path.is_file());
    store.ensure_device(1).unwrap();
    let persisted = fixture.load();
    assert_eq!(store.records(1), persisted.records(1));
    std::fs::write(&fixture.path, b"{truncated").unwrap();
    let mut broken = fixture.load();
    assert!(broken.error.is_some());
    assert!(broken.records(1).is_empty());
    assert!(broken.ensure_device(1).is_err());
    assert!(broken.local(1, "host_auto_pause", json!(true)).is_err());
    assert!(broken.set_consent(true).is_err());
    assert_eq!(std::fs::read(&fixture.path).unwrap(), b"{truncated");
}

#[test]
fn new_call_defaults_require_opt_in_and_preserve_a_user_disabled_return() {
    let fixture = Fixture::new();
    let mut store = fixture.load();
    store.ensure_device(1).unwrap();
    assert_eq!(
        store.get(1, "call_takeover_enabled").unwrap().value,
        json!(false)
    );
    assert_eq!(
        store.get(1, "call_return_enabled").unwrap().value,
        json!(true)
    );
    assert!(!store.consent());
    store.local(1, "call_return_enabled", json!(false)).unwrap();
    let mut reloaded = fixture.load();
    reloaded.ensure_device(1).unwrap();
    assert_eq!(
        reloaded.get(1, "call_return_enabled").unwrap().value,
        json!(false)
    );
    assert!(!reloaded.consent());
}

#[test]
fn peer_schema_collisions_and_local_storage_failure_have_distinct_outcomes() {
    let fixture = Fixture::new();
    let mut store = fixture.load();
    store.ensure_device(1).unwrap();
    let blocked = fixture.directory.join("blocked");
    std::fs::write(&blocked, b"not a directory").unwrap();
    store.storage = Some(blocked.join("preferences.json"));
    let mut desired = record("desired_case_volume", json!("50"));
    desired.origin = "android".into();
    desired.rev = "0000000000000002".into();
    assert!(matches!(
        store.receive(1, desired, false).unwrap(),
        Merge::Failed
    ));
    assert_eq!(
        store.get(1, "desired_case_volume").unwrap().value,
        json!("unset")
    );
    let current = store.get(1, "host_auto_pause").unwrap().clone();
    let collision = CompanionPreference {
        value: json!(false),
        ..current
    };
    assert!(store.receive(1, collision, true).is_err());
    std::fs::write(&fixture.path, b"{truncated").unwrap();
    let mut broken = fixture.load();
    assert!(matches!(
        broken
            .receive(1, record("host_auto_pause", json!(false)), true)
            .unwrap(),
        Merge::Failed
    ));
    assert_eq!(std::fs::read(&fixture.path).unwrap(), b"{truncated");
    assert!(broken
        .receive(1, record("unknown", json!(false)), true)
        .is_err());
}

#[test]
fn overflow_refuses_an_edit_and_preserves_the_winner() {
    let fixture = Fixture::new();
    let mut store = fixture.load();
    store.ensure_device(1).unwrap();
    let incoming = CompanionPreference {
        rev: "ffffffffffffffff".into(),
        origin: "android".into(),
        ..record("host_auto_pause", json!(false))
    };
    assert!(matches!(
        store.merge(1, incoming.clone()).unwrap(),
        Merge::Applied
    ));
    assert!(store.local(1, "host_auto_pause", json!(true)).is_err());
    assert_eq!(store.get(1, "host_auto_pause"), Some(&incoming));
    assert!(store.take_dirty(1).is_empty());
}

#[test]
fn synchronized_intent_changes_only_fallbacks_and_never_confirmed_reports() {
    let mut state = ntpods_ipc::Snapshot {
        model: "A3064".into(),
        firmware: "9A348".into(),
        ..Default::default()
    };
    let mut pending = device_settings::PendingSettings::default();
    assert!(project_choice(
        &mut state,
        &pending,
        "desired_microphone_side",
        &json!("left")
    ));
    assert_eq!(state.device_controls_requested.get(&1), Some(&2));
    assert!(state.device_controls.is_empty());
    assert!(state.device_controls_pending.is_empty());
    assert!(project_choice(
        &mut state,
        &pending,
        "desired_microphone_side",
        &json!("right")
    ));
    assert_eq!(state.device_controls_requested.get(&1), Some(&1));
    pending.confirm_control(&mut state, 1, 0);
    assert!(!project_choice(
        &mut state,
        &pending,
        "desired_microphone_side",
        &json!("left")
    ));
    assert_eq!(state.device_controls.get(&1), Some(&0));
    assert!(!state.device_controls_requested.contains_key(&1));
    assert!(state.device_controls_pending.is_empty());
}

#[test]
fn pending_local_write_and_model_capabilities_block_synced_fallback_projection() {
    let mut state = ntpods_ipc::Snapshot {
        model: "A3064".into(),
        firmware: "9A348".into(),
        ..Default::default()
    };
    let mut pending = device_settings::PendingSettings::default();
    let token = pending.begin_control(&mut state, 1, 1, true);
    assert!(!project_choice(
        &mut state,
        &pending,
        "desired_microphone_side",
        &json!("left")
    ));
    assert!(!state.device_controls_requested.contains_key(&1));
    pending.finish_control(&mut state, 1, 1, token, true);
    assert_eq!(state.device_controls_requested.get(&1), Some(&1));
    state.model = "unknown".into();
    assert!(!project_choice(
        &mut state,
        &pending,
        "desired_microphone_side",
        &json!("left")
    ));
    assert_eq!(state.device_controls_requested.get(&1), Some(&1));
}

#[test]
fn synced_allow_off_remains_unconfirmed_and_cannot_override_readback_or_local_write() {
    let mut state = ntpods_ipc::Snapshot {
        model: "A3064".into(),
        firmware: "9A348".into(),
        ..Default::default()
    };
    let mut pending = device_settings::PendingSettings::default();
    assert!(project_choice(
        &mut state,
        &pending,
        "desired_allow_off",
        &json!("on")
    ));
    assert_eq!(state.allow_off_requested, Some(true));
    assert!(!state.allow_off);
    let token = pending.begin_control(&mut state, 0x34, 2, false);
    assert!(!project_choice(
        &mut state,
        &pending,
        "desired_allow_off",
        &json!("on")
    ));
    pending.finish_control(&mut state, 0x34, 2, token, true);
    assert_eq!(state.allow_off_requested, Some(false));
    pending.confirm_control(&mut state, 0x34, 2);
    assert!(!project_choice(
        &mut state,
        &pending,
        "desired_allow_off",
        &json!("on")
    ));
    assert_eq!(state.allow_off_requested, None);
    assert!(!state.allow_off);
    state.device_controls.clear();
    state.model = "A2083".into();
    assert!(!project_choice(
        &mut state,
        &pending,
        "desired_allow_off",
        &json!("on")
    ));
}

#[test]
fn unsupported_values_preserve_local_state_but_detect_collisions_and_advance_the_next_edit() {
    let fixture = Fixture::new();
    let mut store = fixture.load();
    store.ensure_device(1).unwrap();
    let old = store.get(1, "desired_case_volume").unwrap().clone();
    let observed = CompanionPreference {
        rev: "8000000000000000".into(),
        origin: "android".into(),
        ..record("desired_case_volume", json!("50"))
    };
    store.observe_unsupported(1, &observed).unwrap();
    assert_eq!(store.get(1, "desired_case_volume"), Some(&old));
    assert!(store.take_dirty(1).is_empty());
    let collision = CompanionPreference {
        value: json!("51"),
        ..observed.clone()
    };
    assert!(store.observe_unsupported(1, &collision).is_err());
    assert!(store.merge(1, collision).is_err());
    let stale = CompanionPreference {
        rev: "0000000000000002".into(),
        ..observed.clone()
    };
    let Merge::Stale(winner) = store.merge(1, stale).unwrap() else {
        panic!("unsupported winner still fences older messages")
    };
    assert_eq!(winner, observed);
    let mut restored = fixture.load();
    let edited = restored
        .local(1, "desired_case_volume", json!("25"))
        .unwrap();
    assert_eq!(edited.rev, "8000000000000001");
    assert_eq!(edited.origin, "pc");
}
