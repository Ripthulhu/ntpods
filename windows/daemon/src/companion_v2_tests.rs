use super::*;

fn vectors() -> serde_json::Value {
    serde_json::from_str(include_str!("../tests/fixtures/companion-v2.json")).unwrap()
}

#[test]
fn strict_v2_frames_match_the_android_fixtures() {
    let fixture = vectors();
    let device =
        u64::from_str_radix(&fixture["device"].as_str().unwrap().replace(':', ""), 16).unwrap();
    let sid = fixture["sid"].as_str().unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let frame = case["frame"].as_str().unwrap();
        let authenticated = !matches!(
            case["name"].as_str().unwrap(),
            "hello" | "capability_subset" | "unknown_capability"
        );
        let actual = Frame::parse(frame.as_bytes(), device, authenticated.then_some(sid)).is_ok();
        assert_eq!(
            actual,
            case["accepted"].as_bool().unwrap(),
            "{}: {}",
            case["name"],
            frame
        );
    }
}

#[test]
fn both_authentication_roles_bind_the_v2_capabilities() {
    let fixture = vectors();
    let device =
        u64::from_str_radix(&fixture["device"].as_str().unwrap().replace(':', ""), 16).unwrap();
    let key = decode_hex::<32>(fixture["key"].as_str().unwrap()).unwrap();
    for case in fixture["auth"].as_array().unwrap() {
        let bytes = transcript(
            device,
            fixture["client_nonce"].as_str().unwrap(),
            fixture["server_nonce"].as_str().unwrap(),
            case["client_caps"].as_str().unwrap(),
            case["server_caps"].as_str().unwrap(),
        );
        assert_eq!(bytes, case["transcript"].as_str().unwrap().as_bytes());
        assert_eq!(
            hex(&digest(None, &bytes).unwrap()),
            case["sid"].as_str().unwrap()
        );
        for role in ["server", "client"] {
            assert_eq!(
                hex(&digest(
                    Some(&key),
                    &[format!("{role}\n").as_bytes(), bytes.as_slice()].concat()
                )
                .unwrap()),
                case[format!("{role}_proof")].as_str().unwrap()
            );
        }
    }
}
