//! Typed dongle features; device reports remain separate from accepted requests.
use crate::dongle_protocol::spec::{self, feature};
use ntpods_ipc::{DongleAudioState, DonglePerformanceState, NearbyFinderState};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FirmwareVersion {
    pub major: u8,
    pub minor: u8,
    pub patch: u16,
    pub variant: u8,
}

pub fn firmware_version(bytes: &[u8]) -> Result<FirmwareVersion, String> {
    use spec::firmware_version as wire;
    if bytes.len() != wire::SIZE
        || bytes[wire::VERSION_OFFSET] != wire::VERSION
        || !matches!(
            bytes[wire::VARIANT_OFFSET],
            spec::firmware_variant::STANDARD | spec::firmware_variant::RADIO0076
        )
    {
        return Err("Unsupported dongle firmware version response.".into());
    }
    Ok(FirmwareVersion {
        major: bytes[wire::MAJOR_OFFSET],
        minor: bytes[wire::MINOR_OFFSET],
        patch: u16::from_le_bytes(
            bytes[wire::PATCH_OFFSET..wire::PATCH_OFFSET + wire::PATCH_SIZE]
                .try_into()
                .unwrap(),
        ),
        variant: bytes[wire::VARIANT_OFFSET],
    })
}

pub fn audio(bytes: &[u8]) -> Result<DongleAudioState, String> {
    use spec::audio_profile::*;
    if bytes.len() != SIZE
        || bytes[VERSION_OFFSET] != VERSION
        || [bytes[REQUESTED_CODEC_OFFSET], bytes[ACTUAL_CODEC_OFFSET]]
            .iter()
            .any(|codec| *codec > spec::control::codec::AAC_ELD)
        || bytes[RATE_MODE_OFFSET] > rate_mode::VARIABLE_BITRATE
        || bytes[GAME_MODE_REQUESTED_OFFSET] > 1
        || bytes[GAME_MODE_STATE_OFFSET] > game_mode_state::RESTORING
        || bytes[INPUT_OWNERSHIP_OFFSET] > spec::input_ownership::PHONE
        || bytes[FLAGS_OFFSET] & flag::UNDEFINED_BITS != 0
        || u16::from_le_bytes(
            bytes[STARTUP_PRIMING_MILLISECONDS_OFFSET
                ..STARTUP_PRIMING_MILLISECONDS_OFFSET + STARTUP_PRIMING_MILLISECONDS_SIZE]
                .try_into()
                .unwrap(),
        ) != u16::from(STARTUP_PRIMING_MILLISECONDS)
    {
        return Err("Unsupported dongle audio profile.".into());
    }
    let u32_at = |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    let delay = u16::from_le_bytes(
        bytes[DELAY_TENTHS_MILLISECONDS_OFFSET
            ..DELAY_TENTHS_MILLISECONDS_OFFSET + DELAY_TENTHS_MILLISECONDS_SIZE]
            .try_into()
            .unwrap(),
    );
    Ok(DongleAudioState {
        requested_codec: bytes[REQUESTED_CODEC_OFFSET],
        codec: bytes[ACTUAL_CODEC_OFFSET],
        variable: bytes[RATE_MODE_OFFSET] == rate_mode::VARIABLE_BITRATE,
        requested_bitrate: u32_at(REQUESTED_BITRATE_OFFSET),
        bitrate: u32_at(ACTUAL_BITRATE_OFFSET),
        game_mode_requested: bytes[GAME_MODE_REQUESTED_OFFSET] == 1,
        game_mode_state: [
            "off",
            "pending",
            "unconfirmed",
            "confirmed",
            "failed",
            "unsupported",
            "restoring",
        ][usize::from(bytes[GAME_MODE_STATE_OFFSET])]
        .into(),
        input: bytes[INPUT_OWNERSHIP_OFFSET],
        pending: bytes[FLAGS_OFFSET] & flag::PROFILE_PENDING != 0,
        streaming: bytes[FLAGS_OFFSET] & flag::STREAMING != 0,
        microphone_active: bytes[FLAGS_OFFSET] & flag::MICROPHONE != 0,
        game_mode_supported: bytes[FLAGS_OFFSET] & flag::GAME_MODE_SUPPORTED != 0,
        reported_delay_tenths_ms: (u32::from(delay) != DELAY_UNKNOWN).then_some(delay),
        priming_ms: u16::from(STARTUP_PRIMING_MILLISECONDS),
    })
}

pub fn reconnect_policy(bytes: &[u8]) -> Result<bool, String> {
    match bytes {
        [1, spec::reconnect_policy::MANUAL] => Ok(false),
        [1, spec::reconnect_policy::AUTOMATIC] => Ok(true),
        _ => Err("Unsupported reconnect policy response.".into()),
    }
}

pub fn set_reconnect_policy(
    saved: &mut Option<bool>,
    automatic: bool,
    mut request: impl FnMut(&[u8]) -> Result<Vec<u8>, String>,
) -> Result<(), String> {
    let read = [feature::RECONNECT_POLICY];
    *saved = None;
    *saved = Some(reconnect_policy(&request(&read)?)?);
    if *saved == Some(automatic) {
        return Ok(());
    }
    // Firmware verifies flash before acknowledging. If the receipt was lost, read once without replaying the write.
    let response = request(&[feature::RECONNECT_POLICY, u8::from(automatic)])
        .and_then(|bytes| reconnect_policy(&bytes));
    if response == Ok(automatic) {
        *saved = Some(automatic);
        return Ok(());
    }
    *saved = request(&read)
        .ok()
        .and_then(|bytes| reconnect_policy(&bytes).ok());
    if *saved == Some(automatic) {
        Ok(())
    } else {
        Err(response
            .err()
            .unwrap_or_else(|| "Reconnect policy was not confirmed.".into()))
    }
}

pub fn performance(bytes: &[u8]) -> Result<DonglePerformanceState, String> {
    use spec::performance::*;
    if bytes.len() != SIZE
        || bytes[VERSION_OFFSET] != VERSION
        || bytes[INPUT_OWNERSHIP_OFFSET] > spec::input_ownership::PHONE
        || bytes[ACTUAL_CODEC_OFFSET] > spec::control::codec::AAC_ELD
        || bytes[FLAGS_OFFSET] & flag::UNDEFINED_BITS != 0
    {
        return Err("Unsupported dongle performance snapshot.".into());
    }
    let field = |offset| {
        let value = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        (value != UNAVAILABLE).then_some(value)
    };
    let state = DonglePerformanceState {
        input: bytes[INPUT_OWNERSHIP_OFFSET],
        codec: bytes[ACTUAL_CODEC_OFFSET],
        flags: bytes[FLAGS_OFFSET],
        uptime_ms: field(UPTIME_MILLISECONDS_OFFSET),
        pcm_used: field(PCM_FRAMES_USED_OFFSET),
        pcm_capacity: field(PCM_FRAMES_CAPACITY_OFFSET),
        pcm_high_water: field(PCM_FRAMES_HIGH_WATER_MARK_OFFSET),
        underflow_frames: field(PCM_UNDERFLOW_FRAMES_OFFSET),
        dropped_frames: field(PCM_DROPPED_FRAMES_OFFSET),
        encoded_units: field(ENCODED_UNITS_OFFSET),
        encode_us: field(LAST_ENCODE_MICROSECONDS_OFFSET),
        encode_max_us: field(MAXIMUM_ENCODE_MICROSECONDS_OFFSET),
        send_failures: field(SEND_FAILURES_OFFSET),
        recoveries: field(RECOVERY_REQUESTS_OFFSET),
        heap_capacity: field(LINKED_HEAP_CAPACITY_OFFSET),
        heap_allocated: field(HEAP_ALLOCATED_BYTES_OFFSET),
        heap_remaining: field(HEAP_REMAINING_BYTES_OFFSET),
        heap_arena: field(ALLOCATOR_ARENA_BYTES_OFFSET),
    };
    if state
        .pcm_used
        .zip(state.pcm_capacity)
        .is_some_and(|(used, cap)| used > cap)
        || state
            .pcm_high_water
            .zip(state.pcm_capacity)
            .is_some_and(|(peak, cap)| peak > cap)
        || state
            .heap_allocated
            .zip(state.heap_capacity)
            .is_some_and(|(used, cap)| used > cap)
        || state
            .heap_remaining
            .zip(state.heap_capacity)
            .is_some_and(|(free, cap)| free > cap)
    {
        return Err("Inconsistent dongle performance counters.".into());
    }
    Ok(state)
}

pub fn nearby(bytes: &[u8], revision: u64) -> Result<NearbyFinderState, String> {
    use spec::nearby_finder::*;
    if bytes.len() != SIZE
        || bytes[VERSION_OFFSET] != VERSION
        || bytes[STATE_OFFSET] & STATE_MASK > state::PAUSED
        || bytes[TREND_OFFSET] > trend::WEAKER
    {
        return Err("Unsupported dongle nearby-finder snapshot.".into());
    }
    let state = bytes[STATE_OFFSET] & STATE_MASK;
    let identity = bytes[STATE_OFFSET] & IDENTITY_AVAILABLE_FLAG != 0;
    let latest = bytes[RSSI_OFFSET] as i8;
    let q8 = i32::from_le_bytes(
        bytes[SMOOTHED_RSSI_OFFSET..SMOOTHED_RSSI_OFFSET + SMOOTHED_RSSI_SIZE]
            .try_into()
            .unwrap(),
    );
    let rssi = if q8 == SMOOTHED_RSSI_UNKNOWN {
        None
    } else {
        if !(-126 * 256..=20 * 256).contains(&q8) {
            return Err("Invalid dongle signal strength.".into());
        }
        Some((f64::from(q8) / 256.0).round() as i16)
    };
    if (latest != RSSI_UNKNOWN as i8 && !(-126..=20).contains(&latest))
        || (state != state::STOPPED && !identity)
        || (!identity && rssi.is_some())
    {
        return Err("Unverified dongle signal reading.".into());
    }
    let age = u32::from_le_bytes(
        bytes[LAST_HEARD_AGE_MILLISECONDS_OFFSET
            ..LAST_HEARD_AGE_MILLISECONDS_OFFSET + LAST_HEARD_AGE_MILLISECONDS_SIZE]
            .try_into()
            .unwrap(),
    );
    Ok(NearbyFinderState {
        revision,
        active: state != state::STOPPED,
        paused: state == state::PAUSED,
        available_identity: identity,
        rssi,
        trend: ["unknown", "stronger", "steady", "weaker"][usize::from(bytes[TREND_OFFSET])].into(),
        last_heard_age_ms: (age != AGE_UNKNOWN).then_some(u64::from(age)),
    })
}

pub fn audio_request(
    codec: u8,
    variable: bool,
    bitrate: u32,
) -> Result<[u8; spec::configure_audio_profile::REQUEST_SIZE], String> {
    use spec::configure_audio_profile::*;
    if codec > 3
        || (codec <= 1 && (variable || bitrate != 0))
        || (variable && codec != 3)
        || (codec == 2 && !matches!(bitrate, 0 | 96_000 | 128_000 | 174_000 | 192_000 | 220_000))
        || (codec == 3 && !matches!(bitrate, 0 | 128_000 | 174_000 | 192_000 | 220_000 | 256_000))
    {
        return Err("Unsupported dongle audio profile.".into());
    }
    let mut request = [0; REQUEST_SIZE];
    request[FEATURE_OFFSET] = feature::CONFIGURE_AUDIO_PROFILE;
    request[REQUESTED_CODEC_OFFSET] = codec;
    request[RATE_MODE_OFFSET] = u8::from(variable);
    request[REQUESTED_BITRATE_OFFSET..REQUESTED_BITRATE_OFFSET + REQUESTED_BITRATE_SIZE]
        .copy_from_slice(&bitrate.to_le_bytes());
    Ok(request)
}

pub use crate::aap::EarAction;

#[derive(Default)]
pub struct Ears {
    revision: Option<u32>,
    wearing: Option<[crate::aap::EarStatus; 2]>,
}
impl Ears {
    pub fn observe(
        &mut self,
        packet: Option<&crate::dongle_rpc::Cached>,
    ) -> Result<Option<EarAction>, ()> {
        let without_case = |statuses: [crate::aap::EarStatus; 2]| {
            statuses.map(|status| {
                if status == crate::aap::EarStatus::InCase {
                    crate::aap::EarStatus::Disconnected
                } else {
                    status
                }
            })
        };
        let Some(packet) = packet else {
            self.wearing = None;
            return Err(());
        };
        if self.revision == Some(packet.revision) {
            if packet.age_ms > 1500 {
                self.wearing = self.wearing.map(without_case);
                return Err(());
            }
            return Ok(None);
        }
        if self
            .revision
            .is_some_and(|old| packet.revision.wrapping_sub(old) >= 0x8000_0000)
        {
            return Ok(None);
        }
        let initial = self.revision.is_none();
        self.revision = Some(packet.revision);
        let statuses = crate::aap::parse_ear_detection(&packet.packet);
        let Some((left, right)) = statuses else {
            self.wearing = None;
            return Err(());
        };
        let previous = self.wearing;
        let now = crate::aap::ear_status_state(
            previous.unwrap_or([crate::aap::EarStatus::Disconnected; 2]),
            [left, right],
        );
        if packet.age_ms > 1500 {
            // A cached initial baseline preserves worn buds, never case qualification or an action.
            self.wearing = initial.then_some(without_case(now));
            return Err(());
        }
        self.wearing = Some(now);
        Ok(previous.and_then(|before| crate::aap::ear_media_action(before, now)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Setting {
    pub age_ms: u32,
    pub revision: u32,
    pub value: u32,
}

pub fn settings(bytes: &[u8]) -> Result<BTreeMap<u8, Setting>, String> {
    use spec::settings_snapshot::*;
    if bytes.len() < HEADER_SIZE
        || bytes[VERSION_OFFSET] != VERSION
        || bytes[COUNT_OFFSET] > 32
        || bytes.len() != HEADER_SIZE + usize::from(bytes[COUNT_OFFSET]) * RECORD_SIZE
    {
        return Err("Unsupported dongle settings snapshot.".into());
    }
    let mut result = BTreeMap::new();
    for record in bytes[HEADER_SIZE..].chunks_exact(RECORD_SIZE) {
        let id = record[IDENTIFIER_OFFSET];
        let u32_at = |offset| u32::from_le_bytes(record[offset..offset + 4].try_into().unwrap());
        let setting = Setting {
            age_ms: u32_at(AGE_MILLISECONDS_OFFSET),
            revision: u32_at(REVISION_OFFSET),
            value: u32_at(VALUE_OFFSET),
        };
        if setting.revision == 0
            || !spec::setting::IDS.contains(&id)
            || result.insert(id, setting).is_some()
        {
            return Err("Malformed dongle setting report.".into());
        }
    }
    Ok(result)
}

pub fn setting_request(id: u8, value: u32) -> Result<[u8; 6], String> {
    if !crate::device_settings::valid_control(id, value) {
        return Err("Unsupported AirPods setting value.".into());
    }
    let mut request = [feature::WRITE_SETTING, id, 0, 0, 0, 0];
    request[2..].copy_from_slice(&value.to_le_bytes());
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_typed_layouts_use_the_production_decoders() {
        use crate::dongle_protocol::golden;
        let fixtures = golden::fixtures();
        let mut covered = std::collections::BTreeSet::new();
        for vector in fixtures["layouts"].as_array().unwrap() {
            let bytes = golden::bytes(&vector["hex"]);
            let fields = &vector["fields"];
            let layout = vector["layout"].as_str().unwrap();
            match layout {
                "audio_profile" => {
                    let report = audio(&bytes).unwrap();
                    assert_eq!(
                        [
                            "off",
                            "pending",
                            "sent",
                            "confirmed",
                            "failed",
                            "unsupported",
                            "restoring"
                        ][usize::from(bytes[spec::audio_profile::GAME_MODE_STATE_OFFSET])],
                        fields["game_mode_state"].as_str().unwrap()
                    );
                    assert_eq!(
                        report.requested_bitrate,
                        fields["requested_bitrate"].as_u64().unwrap_or(0) as u32
                    );
                    assert_eq!(
                        report.bitrate,
                        fields["actual_bitrate"].as_u64().unwrap_or(0) as u32
                    );
                    let delay = fields["delay_tenths_milliseconds"].as_u64().unwrap() as u16;
                    assert_eq!(
                        report.reported_delay_tenths_ms,
                        (delay != u16::MAX).then_some(delay)
                    );
                }
                "performance" => {
                    let report = performance(&bytes).unwrap();
                    assert_eq!(
                        report.uptime_ms,
                        fields["uptime_milliseconds"].as_u64().map(|v| v as u32)
                    );
                    assert_eq!(
                        report.pcm_used,
                        Some(fields["pcm_frames_used"].as_u64().unwrap_or(0) as u32)
                    );
                    let available = fields["heap_remaining_bytes"].as_u64().unwrap() as u32;
                    assert_eq!(
                        report.heap_remaining,
                        (available != u32::MAX).then_some(available)
                    );
                }
                "nearby_finder" => {
                    let report =
                        nearby(&bytes, 42).unwrap_or_else(|e| panic!("{}: {e}", vector["name"]));
                    assert_eq!(report.revision, 42);
                    assert_eq!(report.trend, fields["trend"].as_str().unwrap());
                    let q8 = fields["smoothed_rssi"].as_i64().unwrap() as i32;
                    assert_eq!(
                        report.rssi,
                        (q8 != i32::MIN).then(|| (f64::from(q8) / 256.0).round() as i16)
                    );
                }
                "firmware_version" => {
                    let report = firmware_version(&bytes).unwrap();
                    assert_eq!(report.major, fields["major"].as_u64().unwrap() as u8);
                    assert_eq!(report.minor, fields["minor"].as_u64().unwrap() as u8);
                    assert_eq!(report.patch, fields["patch"].as_u64().unwrap() as u16);
                    assert_eq!(report.variant, fields["variant"].as_u64().unwrap() as u8);
                }
                "configure_audio_profile" => {
                    let wire = &spec::configure_audio_profile::REQUEST_SIZE;
                    assert_eq!(bytes.len(), *wire);
                    let result = audio_request(
                        bytes[1],
                        bytes[2] != 0,
                        u32::from_le_bytes(bytes[3..7].try_into().unwrap()),
                    );
                    if vector["valid"] == false {
                        assert!(result.is_err());
                    } else {
                        assert_eq!(result.unwrap().as_slice(), bytes);
                    }
                }
                _ => continue,
            }
            covered.insert(layout);
        }
        assert_eq!(
            covered,
            [
                "audio_profile",
                "configure_audio_profile",
                "firmware_version",
                "nearby_finder",
                "performance"
            ]
            .into_iter()
            .collect()
        );
        for vector in fixtures["settings_sessions"].as_array().unwrap() {
            let bytes = golden::bytes(&vector["snapshot"]);
            let records = settings(&bytes).unwrap();
            assert_eq!(
                records.len(),
                bytes[spec::settings_snapshot::COUNT_OFFSET] as usize
            );
        }
    }

    #[test]
    fn reconnect_policy_keeps_unknown_separate_from_manual() {
        assert_eq!(reconnect_policy(&[1, 0]), Ok(false));
        assert_eq!(reconnect_policy(&[1, 1]), Ok(true));
        for bad in [&[][..], &[0, 0], &[1, 2], &[1, 1, 0]] {
            assert!(reconnect_policy(bad).is_err());
        }
    }
    #[test]
    fn reconnect_policy_confirms_without_replaying_a_write() {
        let read = &[feature::RECONNECT_POLICY][..];
        let set = &[feature::RECONNECT_POLICY, 1][..];
        let mut saved = None;
        let mut script = [
            (read, Ok(vec![1, 0])),
            (set, Err("receipt lost".into())),
            (read, Ok(vec![1, 1])),
        ]
        .into_iter();
        set_reconnect_policy(&mut saved, true, |request| {
            let (expected, reply) = script.next().unwrap();
            assert_eq!(request, expected);
            reply
        })
        .unwrap();
        assert_eq!(script.len(), 0);
        assert_eq!(saved, Some(true));
        set_reconnect_policy(&mut saved, true, |request| {
            assert_eq!(request, read);
            Ok(vec![1, 1])
        })
        .unwrap();
        assert!(set_reconnect_policy(&mut saved, true, |_| Err("removed".into())).is_err());
        assert_eq!(saved, None);
        let mut replies = [Ok(vec![1, 0]), Err("busy".into()), Ok(vec![1, 0])].into_iter();
        assert!(set_reconnect_policy(&mut saved, true, |_| replies.next().unwrap()).is_err());
        assert_eq!(saved, Some(false));
    }
    #[test]
    fn firmware_version_requires_exact_schema_length_and_supported_variant() {
        let good = [1, 1, 2, 3, 1, 1];
        assert_eq!(
            firmware_version(&good).unwrap(),
            FirmwareVersion {
                major: 1,
                minor: 2,
                patch: 259,
                variant: 1
            }
        );
        for len in 0..good.len() {
            assert!(firmware_version(&good[..len]).is_err());
        }
        assert!(firmware_version(&[good.as_slice(), &[0]].concat()).is_err());
        for (offset, value) in [(0, 0), (0, 2), (5, 2)] {
            let mut bad = good;
            bad[offset] = value;
            assert!(firmware_version(&bad).is_err());
        }
    }
    #[test]
    fn audio_profiles_keep_requested_and_actual_codec_delay_and_limit_separate() {
        let mut bytes = [0; 20];
        bytes[..4].copy_from_slice(&[1, 3, 2, 1]);
        bytes[4..8].copy_from_slice(&174000u32.to_le_bytes());
        bytes[8..12].copy_from_slice(&173600u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&[1, 2, 2, 8]);
        bytes[16..18].copy_from_slice(&u16::MAX.to_le_bytes());
        bytes[18] = 100;
        let parsed = audio(&bytes).unwrap();
        assert_eq!(
            (parsed.requested_codec, parsed.codec, parsed.input),
            (3, 2, 2)
        );
        assert_eq!((parsed.requested_bitrate, parsed.bitrate), (174000, 173600));
        assert_eq!(parsed.game_mode_state, "unconfirmed");
        assert_eq!(parsed.reported_delay_tenths_ms, None);
        for length in 0..bytes.len() {
            assert!(audio(&bytes[..length]).is_err());
        }
        bytes[15] |= 16;
        assert!(audio(&bytes).is_err());
        assert_eq!(
            audio_request(3, true, 174000).unwrap(),
            [3, 3, 1, 0xb0, 0xa7, 2, 0]
        );
        assert!(audio_request(2, true, 174000).is_err());
        assert!(audio_request(3, false, 512000).is_err());
        assert!(audio_request(0, false, 0).is_ok());
        assert!(audio_request(1, false, 0).is_ok());
        assert!(audio_request(0, false, 256000).is_err());
    }
    #[test]
    fn performance_preserves_wrapping_counters_unknowns_and_checks_capacity() {
        let mut bytes = [0xff; 64];
        bytes[..4].copy_from_slice(&[1, 1, 3, 15]);
        bytes[8..12].copy_from_slice(&10u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&20u32.to_le_bytes());
        bytes[28..32].copy_from_slice(&(u32::MAX - 1).to_le_bytes());
        let parsed = performance(&bytes).unwrap();
        assert_eq!(parsed.uptime_ms, None);
        assert_eq!(parsed.encoded_units, Some(u32::MAX - 1));
        for length in 0..bytes.len() {
            assert!(performance(&bytes[..length]).is_err());
        }
        bytes[8..12].copy_from_slice(&21u32.to_le_bytes());
        assert!(performance(&bytes).is_err());
    }
    #[test]
    fn nearby_requires_selected_pair_identity_and_rejects_unbounded_signal() {
        let mut bytes = [0; 24];
        bytes[..4].copy_from_slice(&[1, 0x82, (-65i8) as u8, 1]);
        bytes[4..8].copy_from_slice(&123u32.to_le_bytes());
        bytes[20..24].copy_from_slice(&(-64 * 256i32).to_le_bytes());
        let parsed = nearby(&bytes, u64::MAX).unwrap();
        assert!(parsed.active && parsed.available_identity);
        assert_eq!(
            (parsed.rssi, parsed.last_heard_age_ms),
            (Some(-64), Some(123))
        );
        assert_eq!(parsed.revision, u64::MAX);
        for length in 0..bytes.len() {
            assert!(nearby(&bytes[..length], 1).is_err());
        }
        bytes[1] &= 0x7f;
        assert!(nearby(&bytes, 1).is_err());
        bytes[1] = 0x84;
        bytes[20..24].copy_from_slice(&i32::MIN.to_le_bytes());
        assert!(nearby(&bytes, 1).unwrap().paused);
        bytes[20..24].copy_from_slice(&i32::MAX.to_le_bytes());
        assert!(nearby(&bytes, 1).is_err());
    }
    fn record(id: u8, age: u32, revision: u32, value: u32) -> Vec<u8> {
        [
            &[id],
            age.to_le_bytes().as_slice(),
            revision.to_le_bytes().as_slice(),
            value.to_le_bytes().as_slice(),
        ]
        .concat()
    }
    #[test]
    fn snapshots_preserve_exact_values_revisions_and_ages() {
        let bytes = [
            &[1, 2][..],
            record(0x16, 123, u32::MAX, 0x0501).as_slice(),
            record(0x28, 0, 1, 1).as_slice(),
        ]
        .concat();
        let parsed = settings(&bytes).unwrap();
        assert_eq!(
            parsed[&0x16],
            Setting {
                age_ms: 123,
                revision: u32::MAX,
                value: 0x0501
            }
        );
        for n in 0..bytes.len() {
            assert!(settings(&bytes[..n]).is_err());
        }
        let mut duplicate = vec![1, 2];
        duplicate.extend(record(0x28, 0, 1, 1));
        duplicate.extend(record(0x28, 1, 2, 2));
        assert!(settings(&duplicate).is_err());
        assert!(settings(&[1, 0]).unwrap().is_empty());
        assert!(settings(&[2, 0]).is_err());
        let invalid = [&[1, 1][..], record(0x28, 0, 0, 1).as_slice()].concat();
        assert!(settings(&invalid).is_err());
    }
    #[test]
    fn writes_preserve_both_stem_actions_and_reject_unsupported_values() {
        assert_eq!(
            setting_request(0x16, 0x0501).unwrap(),
            [1, 0x16, 1, 5, 0, 0]
        );
        assert!(setting_request(0x16, 1).is_err());
        assert!(setting_request(0x16, 0x01050501).is_err());
        assert!(setting_request(0x24, 0x00022301).is_err());
        assert!(setting_request(0xff, 1).is_err());
    }

    #[test]
    fn unfamiliar_known_values_do_not_discard_other_settings_in_the_snapshot() {
        let bytes = [
            &[1, 3][..],
            record(0x24, 15, 1, 0x00022301).as_slice(),
            record(0x16, 100, 2, 0x01050501).as_slice(),
            record(0x28, 0, 3, 1).as_slice(),
        ]
        .concat();
        let parsed = settings(&bytes).unwrap();
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[&0x24].value, 0x00022301);
        assert_eq!(parsed[&0x16].value, 0x01050501);
        assert_eq!(parsed[&0x28].value, 1);
        let unknown = [&[1, 1][..], record(0x03, 0, 1, 1).as_slice()].concat();
        assert!(settings(&unknown).is_err());
    }
    #[test]
    fn ear_actions_need_a_fresh_transition_and_never_replay_cached_or_invalid_state() {
        let report = |revision, age_ms, left, right| crate::dongle_rpc::Cached {
            revision,
            age_ms,
            packet: vec![4, 0, 4, 0, 6, 0, left, right],
        };
        let mut ears = Ears::default();
        assert_eq!(ears.observe(Some(&report(1, 0, 0, 0))), Ok(None));
        assert_eq!(
            ears.observe(Some(&report(2, 0, 1, 0))),
            Ok(Some(EarAction::Pause))
        );
        assert_eq!(ears.observe(Some(&report(2, 0, 1, 0))), Ok(None));
        assert_eq!(ears.observe(Some(&report(2, 60000, 1, 0))), Err(()));
        assert_eq!(ears.observe(Some(&report(1, 0, 0, 0))), Ok(None));
        assert_eq!(ears.observe(Some(&report(3, 0, 4, 0))), Ok(None));
        assert_eq!(
            ears.observe(Some(&report(4, 0, 0, 0))),
            Ok(Some(EarAction::Resume))
        );
        assert_eq!(ears.observe(Some(&report(5, 1501, 1, 0))), Err(()));
        assert_eq!(ears.observe(Some(&report(6, 0, 0, 0))), Ok(None));
        assert_eq!(ears.observe(Some(&report(7, 0, 9, 0))), Err(()));
        assert_eq!(ears.observe(None), Err(()));
        assert_eq!(ears.observe(Some(&report(8, 0, 1, 0))), Ok(None));
        let mut after_microphone = Ears::default();
        assert_eq!(
            after_microphone.observe(Some(&report(10, 60000, 0, 0))),
            Err(())
        );
        assert_eq!(
            after_microphone.observe(Some(&report(10, 61000, 0, 0))),
            Err(())
        );
        assert_eq!(
            after_microphone.observe(Some(&report(11, 0, 1, 0))),
            Ok(Some(EarAction::Pause))
        );
    }

    #[test]
    fn one_bud_case_transitions_keep_the_other_bud_usable_in_both_orientations() {
        for solo in [[0, 2], [2, 0]] {
            let report = |revision, statuses: [u8; 2]| crate::dongle_rpc::Cached {
                revision,
                age_ms: 0,
                packet: vec![4, 0, 4, 0, 6, 0, statuses[0], statuses[1]],
            };
            let removed = solo.map(|status| if status == 2 { 1 } else { status });
            let mut ears = Ears::default();
            assert_eq!(ears.observe(Some(&report(1, [0, 0]))), Ok(None));
            assert_eq!(ears.observe(Some(&report(2, solo))), Ok(None));
            assert_eq!(
                ears.observe(Some(&report(3, removed))),
                Ok(Some(EarAction::Pause))
            );
            assert_eq!(
                ears.observe(Some(&report(4, solo))),
                Ok(Some(EarAction::Resume))
            );
            assert_eq!(
                ears.observe(Some(&report(5, [2, 2]))),
                Ok(Some(EarAction::Pause))
            );
            assert_eq!(
                ears.observe(Some(&report(6, solo))),
                Ok(Some(EarAction::Resume))
            );
        }
    }

    #[test]
    fn stale_case_state_and_report_gaps_cannot_qualify_a_transitional_partner() {
        let report = |revision, age_ms, first, second| crate::dongle_rpc::Cached {
            revision,
            age_ms,
            packet: vec![4, 0, 4, 0, 6, 0, first, second],
        };
        let mut ears = Ears::default();
        assert_eq!(ears.observe(Some(&report(1, 0, 2, 0))), Ok(None));
        assert_eq!(
            ears.observe(Some(&report(2, 0, 2, 1))),
            Ok(Some(EarAction::Pause))
        );
        assert_eq!(ears.observe(Some(&report(2, 1501, 2, 1))), Err(()));
        assert_eq!(ears.observe(Some(&report(3, 0, 4, 0))), Ok(None));
        assert_eq!(
            ears.wearing,
            Some([
                crate::aap::EarStatus::Disconnected,
                crate::aap::EarStatus::InEar
            ])
        );
        assert_eq!(ears.observe(None), Err(()));
        assert_eq!(ears.observe(Some(&report(4, 0, 0, 3))), Ok(None));
        let mut reconnected = Ears::default();
        assert_eq!(reconnected.observe(Some(&report(1, 60000, 2, 0))), Err(()));
        assert_eq!(reconnected.observe(Some(&report(2, 0, 4, 0))), Ok(None));
        assert_eq!(
            reconnected.wearing,
            Some([
                crate::aap::EarStatus::Disconnected,
                crate::aap::EarStatus::InEar
            ])
        );
    }
}
