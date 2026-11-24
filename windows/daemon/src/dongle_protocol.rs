//! USB HID feature reports; see airpods-dongle/protocol/dongle-protocol.json.
use ntpods_ipc::{Battery, DongleState};
#[path = "dongle_control_protocol.rs"]
pub mod spec;
use spec::control::{
    self, capability, codec, command_field as cf, component, listening_mode, opcode as op, result,
    status_field as sf, status_flag as flag,
};

pub const SIZE: usize = spec::frame::SIZE;
pub const HEADER: [u8; spec::frame::HEADER_SIZE] = control::HEADER;

#[derive(Clone)]
pub struct Report {
    pub state: DongleState,
    pub address: [u8; cf::SELECTED_ADDRESS_SIZE],
    pub sequence: u32,
    pub opcode: u8,
    pub result: u8,
}

pub fn paired_device_id(address: [u8; cf::SELECTED_ADDRESS_SIZE]) -> Option<String> {
    (address != [0; cf::SELECTED_ADDRESS_SIZE])
        .then(|| address.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub fn parse(bytes: &[u8]) -> Option<Report> {
    if bytes.len() != SIZE
        || bytes[..HEADER.len()] != HEADER
        || bytes[sf::FLAGS_OFFSET] & flag::UNDEFINED_BITS != 0
        || bytes[sf::LISTENING_MODE_OFFSET] > listening_mode::ADAPTIVE
        || bytes[sf::CHARGING_COMPONENTS_OFFSET] & component::UNDEFINED_BITS != 0
        || bytes[sf::OUTPUT_CODEC_OFFSET] > codec::AAC_ELD
        || bytes[sf::LAST_COMMAND_RESULT_OFFSET] > result::MALFORMED
        || bytes[sf::LAST_COMMAND_OPCODE_OFFSET] > op::OPEN_PHONE_PAIRING
        || bytes[sf::RESERVED_OFFSET..].iter().any(|b| *b != 0)
        || bytes[sf::BATTERY_PERCENT_OFFSET..sf::BATTERY_PERCENT_OFFSET + sf::BATTERY_PERCENT_SIZE]
            .iter()
            .any(|b| *b > 100 && *b != control::BATTERY_UNKNOWN)
    {
        return None;
    }
    let u32_at = |i| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
    let capabilities = u16::from_le_bytes(
        bytes[sf::CAPABILITIES_OFFSET..sf::CAPABILITIES_OFFSET + sf::CAPABILITIES_SIZE]
            .try_into()
            .unwrap(),
    );
    let observed = capabilities & capability::BATTERY_OBSERVATION_FLAGS != 0;
    if (observed
        && (bytes[sf::PRESENT_COMPONENTS_OFFSET] | bytes[sf::CASE_OBSERVED_COMPONENTS_OFFSET])
            & component::UNDEFINED_BITS
            != 0)
        || (!observed
            && (bytes[sf::PRESENT_COMPONENTS_OFFSET] != 0
                || bytes[sf::CASE_OBSERVED_COMPONENTS_OFFSET] != 0))
    {
        return None;
    }
    let flags = bytes[sf::FLAGS_OFFSET];
    let ages: [Option<u32>; component::SLOT_COUNT] = std::array::from_fn(|i| {
        let age = u32_at(sf::BATTERY_AGES_MILLISECONDS_OFFSET + i * 4);
        (age != control::AGE_UNKNOWN).then_some(age)
    });
    let level = |i| {
        (bytes[sf::BATTERY_PERCENT_OFFSET + i] <= 100)
            .then_some(bytes[sf::BATTERY_PERCENT_OFFSET + i])
    };
    if observed
        && (bytes[sf::CHARGING_COMPONENTS_OFFSET] & !bytes[sf::PRESENT_COMPONENTS_OFFSET] != 0
            || (0..component::SLOT_COUNT).any(|i| {
                bytes[sf::PRESENT_COMPONENTS_OFFSET] & (1 << i) != 0
                    && (level(i).is_none() || ages[i].is_none())
            }))
    {
        return None;
    }
    let present = if observed {
        bytes[sf::PRESENT_COMPONENTS_OFFSET]
    } else {
        (0..component::SLOT_COUNT).fold(0u8, |mask, i| {
            mask | if level(i).is_some() { 1 << i } else { 0 }
        })
    };
    let address = bytes
        [sf::SELECTED_ADDRESS_OFFSET..sf::SELECTED_ADDRESS_OFFSET + sf::SELECTED_ADDRESS_SIZE]
        .try_into()
        .unwrap();
    Some(Report {
        state: DongleState {
            present: true,
            generation: u32_at(sf::CONNECTION_GENERATION_OFFSET),
            selected: address != [0; cf::SELECTED_ADDRESS_SIZE],
            paired_device_id: paired_device_id(address),
            connected: flags & flag::A2DP_CONNECTED != 0,
            controls_ready: flags & flag::AACP_READY != 0,
            streaming: flags & flag::STREAMING != 0,
            phone_connected: flags & flag::PHONE_CONNECTED != 0,
            microphone_active: flags & flag::MICROPHONE_ACTIVE != 0,
            phone_pairing: flags & flag::PHONE_PAIRING != 0,
            usb_playback: flags & flag::USB_PLAYBACK != 0,
            anc: bytes[sf::LISTENING_MODE_OFFSET],
            battery: Battery {
                left: level(component::LEFT_SLOT),
                right: level(component::RIGHT_SLOT),
                case: level(component::CASE_SLOT),
                left_charging: bytes[sf::CHARGING_COMPONENTS_OFFSET]
                    & present
                    & component::LEFT_BIT
                    != 0
                    && level(component::LEFT_SLOT).is_some(),
                right_charging: bytes[sf::CHARGING_COMPONENTS_OFFSET]
                    & present
                    & component::RIGHT_BIT
                    != 0
                    && level(component::RIGHT_SLOT).is_some(),
                case_charging: bytes[sf::CHARGING_COMPONENTS_OFFSET]
                    & present
                    & component::CASE_BIT
                    != 0
                    && level(component::CASE_SLOT).is_some(),
                ..Default::default()
            },
            battery_age_ms: ages,
            battery_present: present,
            battery_case_source: bytes[sf::CASE_OBSERVED_COMPONENTS_OFFSET],
            codec: bytes[sf::OUTPUT_CODEC_OFFSET],
            capabilities,
            ..Default::default()
        },
        address,
        sequence: u32_at(sf::LAST_COMMAND_SEQUENCE_OFFSET),
        opcode: bytes[sf::LAST_COMMAND_OPCODE_OFFSET],
        result: bytes[sf::LAST_COMMAND_RESULT_OFFSET],
    })
}

pub fn command(
    report: &Report,
    generation: u32,
    sequence: u32,
    opcode: u8,
    argument: u8,
) -> Result<[u8; SIZE], &'static str> {
    validate_command(&report.state, generation, opcode, argument)?;
    if sequence == 0 {
        return Err("unsupported");
    }
    let mut bytes = [0; SIZE];
    bytes[..HEADER.len()].copy_from_slice(&HEADER);
    bytes[cf::OPCODE_OFFSET] = opcode;
    bytes[cf::CONNECTION_GENERATION_OFFSET
        ..cf::CONNECTION_GENERATION_OFFSET + cf::CONNECTION_GENERATION_SIZE]
        .copy_from_slice(&generation.to_le_bytes());
    bytes[cf::SELECTED_ADDRESS_OFFSET..cf::SELECTED_ADDRESS_OFFSET + cf::SELECTED_ADDRESS_SIZE]
        .copy_from_slice(&report.address);
    bytes[cf::COMMAND_SEQUENCE_OFFSET..cf::COMMAND_SEQUENCE_OFFSET + cf::COMMAND_SEQUENCE_SIZE]
        .copy_from_slice(&sequence.to_le_bytes());
    bytes[cf::ARGUMENT_OFFSET] = argument;
    Ok(bytes)
}

pub fn validate_command(
    state: &DongleState,
    generation: u32,
    opcode: u8,
    argument: u8,
) -> Result<(), &'static str> {
    if generation != state.generation {
        return Err("stale");
    }
    if !matches!(
        opcode,
        op::RECONNECT_AIRPODS
            | op::PAIR_AIRPODS
            | op::CHANGE_LISTENING_MODE
            | op::OPEN_PHONE_PAIRING
    ) || (opcode == op::CHANGE_LISTENING_MODE
        && !matches!(argument, listening_mode::ANC | listening_mode::TRANSPARENCY))
        || (opcode != op::CHANGE_LISTENING_MODE && argument != 0)
    {
        return Err("unsupported");
    }
    let capability = match opcode {
        op::RECONNECT_AIRPODS | op::PAIR_AIRPODS => capability::PAIR_AND_RECONNECT,
        op::CHANGE_LISTENING_MODE => capability::CONFIRMED_NOISE_CONTROL,
        _ => capability::PHONE_PAIRING,
    };
    if state.capabilities & capability == 0 {
        return Err("unsupported");
    }
    if opcode == op::CHANGE_LISTENING_MODE && !state.controls_ready {
        return Err("disconnected");
    }
    if opcode == op::PAIR_AIRPODS
        && (state.usb_playback || state.microphone_active || state.phone_connected)
    {
        return Err("busy");
    }
    Ok(())
}

pub fn receipt(report: &Report, sequence: u32, opcode: u8) -> Option<Result<(), &'static str>> {
    if report.sequence != sequence || report.opcode != opcode || report.result == result::NONE {
        return None;
    }
    Some(match report.result {
        result::ACCEPTED => Ok(()),
        result::BUSY => Err("busy"),
        result::STALE => Err("stale"),
        result::UNSUPPORTED => Err("unsupported"),
        result::DISCONNECTED => Err("disconnected"),
        _ => Err("malformed"),
    })
}

/// A receipt is tied to one USB session, not to the Bluetooth generation it changes.
pub struct Pending {
    pub sequence: u32,
    pub opcode: u8,
    pub session: u64,
    pub since: std::time::Instant,
}
impl Pending {
    pub fn poll(
        &self,
        report: Option<&Report>,
        session: u64,
        now: std::time::Instant,
    ) -> Option<Result<(), &'static str>> {
        if session != self.session {
            return Some(Err("stale"));
        }
        let Some(report) = report else {
            return Some(Err("disconnected"));
        };
        if now.saturating_duration_since(self.since) >= std::time::Duration::from_secs(2) {
            return Some(Err("timeout"));
        }
        receipt(report, self.sequence, self.opcode)
    }
}

#[cfg(test)]
pub(crate) mod golden {
    pub fn fixtures() -> serde_json::Value {
        serde_json::from_str(include_str!(
            "../tests/fixtures/dongle-protocol-vectors.json"
        ))
        .unwrap()
    }
    pub fn bytes(value: &serde_json::Value) -> Vec<u8> {
        value
            .as_str()
            .unwrap()
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_status_and_command_vectors_use_the_production_parser_and_encoder() {
        let fixtures = golden::fixtures();
        assert_eq!(fixtures["protocol_version"], spec::PROTOCOL_VERSION);
        for vector in fixtures["control_status"].as_array().unwrap() {
            let bytes = golden::bytes(&vector["hex"]);
            let report = parse(&bytes).unwrap_or_else(|| panic!("{}", vector["name"]));
            let expected = &vector["fields"];
            assert_eq!(
                report.state.generation,
                expected["connection_generation"].as_u64().unwrap() as u32
            );
            assert_eq!(
                report.address.as_slice(),
                golden::bytes(&expected["selected_address"])
            );
            assert_eq!(
                report.sequence,
                expected["last_command_sequence"].as_u64().unwrap() as u32
            );
            assert_eq!(
                report.opcode,
                expected["last_command_opcode"].as_u64().unwrap() as u8
            );
            assert_eq!(
                [
                    "none",
                    "accepted",
                    "busy",
                    "stale",
                    "unsupported",
                    "disconnected",
                    "malformed"
                ][usize::from(report.result)],
                expected["last_command_result"].as_str().unwrap()
            );
            assert_eq!(
                ["unknown", "off", "anc", "transparency", "adaptive"]
                    [usize::from(report.state.anc)],
                expected["listening_mode"].as_str().unwrap()
            );
            assert_eq!(
                ["unknown", "sbc", "aac", "aac_eld"][usize::from(report.state.codec)],
                expected["output_codec"].as_str().unwrap()
            );
            assert_eq!(
                report.state.battery_present,
                expected["present_components"].as_u64().unwrap() as u8
            );
            assert_eq!(
                report.state.battery_case_source,
                expected["case_observed_components"].as_u64().unwrap() as u8
            );
            for (slot, value) in [
                report.state.battery.left,
                report.state.battery.right,
                report.state.battery.case,
            ]
            .iter()
            .enumerate()
            {
                let raw = expected["battery_percent"][slot].as_u64().unwrap() as u8;
                assert_eq!(*value, (raw != control::BATTERY_UNKNOWN).then_some(raw));
                let age = expected["battery_ages_milliseconds"][slot]
                    .as_u64()
                    .unwrap() as u32;
                assert_eq!(
                    report.state.battery_age_ms[slot],
                    (age != control::AGE_UNKNOWN).then_some(age)
                );
            }
        }
        let mut report = parse(&status()).unwrap();
        report.state.capabilities = capability::PAIR_AND_RECONNECT
            | capability::CONFIRMED_NOISE_CONTROL
            | capability::PHONE_PAIRING;
        report.state.generation = 42;
        report.address = [0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6];
        for vector in fixtures["control_commands"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|v| v["valid"] == true)
        {
            let expected = &vector["fields"];
            let opcode = match expected["opcode"].as_str().unwrap() {
                "reconnect_airpods" => op::RECONNECT_AIRPODS,
                "pair_airpods" => op::PAIR_AIRPODS,
                "change_listening_mode" => op::CHANGE_LISTENING_MODE,
                "open_phone_pairing" => op::OPEN_PHONE_PAIRING,
                other => panic!("unknown command {other}"),
            };
            let encoded = command(
                &report,
                expected["connection_generation"].as_u64().unwrap() as u32,
                expected["command_sequence"].as_u64().unwrap() as u32,
                opcode,
                expected["argument"].as_u64().unwrap() as u8,
            )
            .unwrap();
            assert_eq!(encoded.as_slice(), golden::bytes(&vector["hex"]));
        }
    }
    fn status() -> [u8; SIZE] {
        let mut b = [0; SIZE];
        b[..7].copy_from_slice(&HEADER);
        b[7] = 3;
        b[8] = 7;
        b[12..18].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        b[42] = 7;
        b[19..22].fill(255);
        b[24..36].fill(255);
        b
    }
    #[test]
    fn rejects_incomplete_unknown_and_malformed_reports() {
        let good = status();
        for n in 0..SIZE {
            assert!(parse(&good[..n]).is_none());
        }
        for (offset, value) in [
            (0, 1),
            (5, 2),
            (6, 63),
            (18, 5),
            (19, 101),
            (22, 8),
            (23, 4),
            (40, 7),
            (44, 1),
        ] {
            let mut b = good;
            b[offset] = value;
            assert!(parse(&b).is_none());
        }
        let s = parse(&good).unwrap();
        assert_eq!(s.state.battery.left, None);
        assert_eq!(s.state.battery_age_ms, [None; 3]);
    }

    #[test]
    fn unknown_capability_bits_do_not_hide_supported_status_or_authorize_new_commands() {
        let mut bytes = status();
        bytes[42] = 128;
        bytes[43] = 128;
        let report = parse(&bytes).unwrap();
        assert_eq!(report.state.capabilities, 0x8080);
        assert_eq!(command(&report, 7, 1, 1, 0), Err("unsupported"));
    }

    #[test]
    fn paired_device_identity_is_stable_across_control_generations_and_clears_when_unselected() {
        let mut bytes = status();
        assert_eq!(
            parse(&bytes).unwrap().state.paired_device_id.as_deref(),
            Some("010203040506")
        );
        bytes[8..12].copy_from_slice(&99u32.to_le_bytes());
        assert_eq!(
            parse(&bytes).unwrap().state.paired_device_id.as_deref(),
            Some("010203040506")
        );
        bytes[12..18].copy_from_slice(&[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);
        assert_eq!(
            parse(&bytes).unwrap().state.paired_device_id.as_deref(),
            Some("aabbccddeeff")
        );
        bytes[12..18].fill(0);
        assert!(!parse(&bytes).unwrap().state.selected);
        assert!(parse(&bytes).unwrap().state.paired_device_id.is_none());
        assert!(DongleState::default().paired_device_id.is_none());
    }
    #[test]
    fn battery_observation_flags_preserve_cached_levels_and_age_without_presence_or_identity() {
        let mut bytes = status();
        bytes[42] |= 64;
        bytes[19..22].copy_from_slice(&[94, 93, 61]);
        bytes[22] = 5;
        bytes[24..28].copy_from_slice(&1000u32.to_le_bytes());
        bytes[28..32].copy_from_slice(&90001u32.to_le_bytes());
        bytes[32..36].copy_from_slice(&2000u32.to_le_bytes());
        bytes[44] = 5;
        bytes[45] = 7;
        let report = parse(&bytes).unwrap();
        assert_eq!(report.state.battery_present, 5);
        assert_eq!(report.state.battery_case_source, 7);
        assert_eq!(report.state.battery.right, Some(93));
        assert_eq!(
            report.state.battery_age_ms,
            [Some(1000), Some(90001), Some(2000)]
        );
        assert!(report.state.battery.left_charging && report.state.battery.case_charging);
        assert!(!report.state.battery.right_charging);
        let mut bad = bytes;
        bad[22] = 7;
        assert!(
            parse(&bad).is_none(),
            "absent components cannot claim charging"
        );
        for i in [0, 2] {
            let mut bad = bytes;
            bad[19 + i] = 255;
            assert!(
                parse(&bad).is_none(),
                "presence requires a known percentage"
            );
            let mut bad = bytes;
            bad[24 + i * 4..28 + i * 4].fill(255);
            assert!(
                parse(&bad).is_none(),
                "presence requires a known sample age"
            );
        }
        let mut empty = bytes;
        empty[19..22].fill(255);
        empty[22] = 0;
        empty[24..36].fill(255);
        empty[44] = 0;
        assert!(
            parse(&empty).is_some(),
            "case provenance can report absence without history"
        );
        bytes[7] = 0;
        let report = parse(&bytes).unwrap();
        assert!(!report.state.connected && !report.state.controls_ready && !report.state.streaming);
        for at in 44..SIZE {
            let mut bad = bytes;
            bad[at] = if at < 46 { 8 } else { 1 };
            assert!(parse(&bad).is_none());
        }
        bytes[42] &= !64;
        assert!(parse(&bytes).is_none());
        bytes[44] = 0;
        bytes[45] = 0;
        let legacy = parse(&bytes).unwrap();
        assert_eq!(
            (
                legacy.state.battery_present,
                legacy.state.battery_case_source
            ),
            (7, 0)
        );
    }
    #[test]
    fn commands_pin_identity_and_reject_stale_generation() {
        let report = parse(&status()).unwrap();
        assert_eq!(command(&report, 6, 1, 3, 2), Err("stale"));
        assert_eq!(command(&report, 7, 1, 3, 4), Err("unsupported"));
        let b = command(&report, 7, 1, 3, 2).unwrap();
        assert_eq!(&b[12..18], &[1, 2, 3, 4, 5, 6]);
        assert!(b[23..].iter().all(|b| *b == 0));
    }
    #[test]
    fn receipt_survives_expected_pair_generation_change() {
        let mut b = status();
        b[8] = 8;
        b[36] = 42;
        b[40] = 1;
        b[41] = 2;
        let report = parse(&b).unwrap();
        assert_eq!(receipt(&report, 41, 2), None);
        assert_eq!(receipt(&report, 42, 3), None);
        assert_eq!(receipt(&report, 42, 2), Some(Ok(())));
    }
    #[test]
    fn pending_rejects_unplug_replacement_and_late_receipts() {
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let pending = Pending {
            sequence: 42,
            opcode: 2,
            session: 9,
            since: now,
        };
        let mut b = status();
        b[36] = 42;
        b[40] = 1;
        b[41] = 2;
        let r = parse(&b).unwrap();
        assert_eq!(pending.poll(None, 9, now), Some(Err("disconnected")));
        assert_eq!(pending.poll(Some(&r), 10, now), Some(Err("stale")));
        assert_eq!(
            pending.poll(Some(&r), 9, now + Duration::from_secs(2)),
            Some(Err("timeout"))
        );
        assert_eq!(pending.poll(Some(&r), 9, now), Some(Ok(())));
    }
    #[test]
    fn destructive_pair_rejects_active_audio_and_unknown_controls() {
        for flag in [8, 16, 64] {
            let mut b = status();
            b[7] |= flag;
            assert_eq!(command(&parse(&b).unwrap(), 7, 1, 2, 0), Err("busy"));
        }
        let mut b = status();
        b[42] = 1;
        assert_eq!(command(&parse(&b).unwrap(), 7, 1, 3, 2), Err("unsupported"));
        b[42] = 7;
        b[7] = 1;
        assert_eq!(
            command(&parse(&b).unwrap(), 7, 1, 3, 2),
            Err("disconnected")
        );
    }
}
