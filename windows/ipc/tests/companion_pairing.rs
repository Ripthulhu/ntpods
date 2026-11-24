use ntpods_ipc::{from_line, to_line, Command, CompanionPairingStatus, CompanionStatus, Event};

#[test]
fn pairing_commands_require_correlation_and_preserve_the_original_cancel_identifier() {
    let begin = serde_json::json!({"cmd":"companion_begin_pairing","peer_address":"11:22:33:44:55:66",
        "device_id":"aabbccddeeff","code":"01234567","request_id":u64::MAX});
    assert!(
        matches!(from_line::<Command>(&begin.to_string()),Some(Command::CompanionBeginPairing { code, request_id:u64::MAX,..}) if code=="01234567")
    );
    let mut missing = begin.clone();
    missing.as_object_mut().unwrap().remove("request_id");
    assert!(from_line::<Command>(&missing.to_string()).is_none());
    let mut wrong_type = begin;
    wrong_type["code"] = 12345678.into();
    assert!(from_line::<Command>(&wrong_type.to_string()).is_none());
    let cancel = to_line(&Command::CompanionCancelPairing {
        request_id: u64::MAX,
    });
    assert!(matches!(
        from_line::<Command>(&cancel),
        Some(Command::CompanionCancelPairing {
            request_id: u64::MAX
        })
    ));
}

#[test]
fn window_status_is_optional_for_legacy_clients_and_never_carries_provisioning_material() {
    let legacy = to_line(&Event::CompanionStatus {
        status: CompanionStatus::default(),
    });
    assert!(!legacy.contains("pairing"));
    let event = Event::CompanionStatus {
        status: CompanionStatus {
            pairing: Some(CompanionPairingStatus {
                request_id: u64::MAX,
                device_id: "aabbccddeeff".into(),
                peer_address: "11:22:33:44:55:66".into(),
                phase: "waiting".into(),
                remaining_ms: 120000,
            }),
            ..Default::default()
        },
    };
    let line = to_line(&event);
    assert!(!line.contains("\"code\"") && !line.contains("\"key\""));
    let Some(Event::CompanionStatus { status }) = from_line::<Event>(&line) else {
        panic!("pairing status lost")
    };
    assert_eq!(status.pairing.unwrap().request_id, u64::MAX);
}
