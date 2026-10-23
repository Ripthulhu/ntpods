//! Typed dongle features; device reports remain separate from accepted requests.
use ntpods_ipc::{DongleAudioState, DonglePerformanceState, NearbyFinderState};
use std::collections::BTreeMap;

pub fn audio(bytes: &[u8]) -> Result<DongleAudioState, String> {
    if bytes.len() != 20
        || bytes[0] != 1
        || bytes[1..=2].iter().any(|codec| *codec > 3)
        || bytes[3] > 1
        || bytes[12] > 1
        || bytes[13] > 6
        || bytes[14] > 2
        || bytes[15] & !15 != 0
        || u16::from_le_bytes([bytes[18], bytes[19]]) != 100
    {
        return Err("Unsupported dongle audio profile.".into());
    }
    let u32_at = |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
    let delay = u16::from_le_bytes([bytes[16], bytes[17]]);
    Ok(DongleAudioState {
        requested_codec: bytes[1],
        codec: bytes[2],
        variable: bytes[3] == 1,
        requested_bitrate: u32_at(4),
        bitrate: u32_at(8),
        game_mode_requested: bytes[12] == 1,
        game_mode_state: [
            "off",
            "pending",
            "unconfirmed",
            "confirmed",
            "failed",
            "unsupported",
            "restoring",
        ][usize::from(bytes[13])]
        .into(),
        input: bytes[14],
        pending: bytes[15] & 1 != 0,
        streaming: bytes[15] & 2 != 0,
        microphone_active: bytes[15] & 4 != 0,
        game_mode_supported: bytes[15] & 8 != 0,
        reported_delay_tenths_ms: (delay != u16::MAX).then_some(delay),
        priming_ms: 100,
    })
}

pub fn performance(bytes: &[u8]) -> Result<DonglePerformanceState, String> {
    if bytes.len() != 64 || bytes[0] != 1 || bytes[1] > 2 || bytes[2] > 3 || bytes[3] & !15 != 0 {
        return Err("Unsupported dongle performance snapshot.".into());
    }
    let field = |offset| {
        let value = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        (value != u32::MAX).then_some(value)
    };
    let state = DonglePerformanceState {
        input: bytes[1],
        codec: bytes[2],
        flags: bytes[3],
        uptime_ms: field(4),
        pcm_used: field(8),
        pcm_capacity: field(12),
        pcm_high_water: field(16),
        underflow_frames: field(20),
        dropped_frames: field(24),
        encoded_units: field(28),
        encode_us: field(32),
        encode_max_us: field(36),
        send_failures: field(40),
        recoveries: field(44),
        heap_capacity: field(48),
        heap_allocated: field(52),
        heap_remaining: field(56),
        heap_arena: field(60),
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
    if bytes.len() != 24 || bytes[0] != 1 || bytes[1] & 0x7f > 4 || bytes[3] > 3 {
        return Err("Unsupported dongle nearby-finder snapshot.".into());
    }
    let state = bytes[1] & 0x7f;
    let identity = bytes[1] & 0x80 != 0;
    let latest = bytes[2] as i8;
    let q8 = i32::from_le_bytes(bytes[20..24].try_into().unwrap());
    let rssi = if q8 == i32::MIN {
        None
    } else {
        if !(-126 * 256..=20 * 256).contains(&q8) {
            return Err("Invalid dongle signal strength.".into());
        }
        Some((f64::from(q8) / 256.0).round() as i16)
    };
    if (latest != 127 && !(-126..=20).contains(&latest))
        || (state != 0 && !identity)
        || (!identity && rssi.is_some())
    {
        return Err("Unverified dongle signal reading.".into());
    }
    let age = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    Ok(NearbyFinderState {
        revision,
        active: state != 0,
        paused: state == 4,
        available_identity: identity,
        rssi,
        trend: ["unknown", "stronger", "steady", "weaker"][usize::from(bytes[3])].into(),
        last_heard_age_ms: (age != u32::MAX).then_some(u64::from(age)),
    })
}

pub fn audio_request(codec: u8, variable: bool, bitrate: u32) -> Result<[u8; 7], String> {
    if codec > 3
        || (codec <= 1 && (variable || bitrate != 0))
        || (variable && codec != 3)
        || (codec == 2 && !matches!(bitrate, 0 | 96_000 | 128_000 | 174_000 | 192_000 | 220_000))
        || (codec == 3 && !matches!(bitrate, 0 | 128_000 | 174_000 | 192_000 | 220_000 | 256_000))
    {
        return Err("Unsupported dongle audio profile.".into());
    }
    let mut request = [3, codec, u8::from(variable), 0, 0, 0, 0];
    request[3..].copy_from_slice(&bitrate.to_le_bytes());
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
        let without_case = |statuses: [crate::aap::EarStatus; 2]| statuses.map(|status| {
            if status == crate::aap::EarStatus::InCase {
                crate::aap::EarStatus::Disconnected
            } else {
                status
            }
        });
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
    if bytes.len() < 2
        || bytes[0] != 1
        || bytes[1] > 32
        || bytes.len() != 2 + usize::from(bytes[1]) * 13
    {
        return Err("Unsupported dongle settings snapshot.".into());
    }
    let mut result = BTreeMap::new();
    for record in bytes[2..].chunks_exact(13) {
        let id = record[0];
        let u32_at = |offset| u32::from_le_bytes(record[offset..offset + 4].try_into().unwrap());
        let setting = Setting {
            age_ms: u32_at(1),
            revision: u32_at(5),
            value: u32_at(9),
        };
        if setting.revision == 0
            || !crate::device_settings::valid_control(id, setting.value)
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
    let mut request = [1, id, 0, 0, 0, 0];
    request[2..].copy_from_slice(&value.to_le_bytes());
    Ok(request)
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(setting_request(0xff, 1).is_err());
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
        assert_eq!(after_microphone.observe(Some(&report(10, 61000, 0, 0))), Err(()));
        assert_eq!(
            after_microphone.observe(Some(&report(11, 0, 1, 0))),
            Ok(Some(EarAction::Pause))
        );
    }

    #[test]
    fn one_bud_case_transitions_keep_the_other_bud_usable_in_both_orientations() {
        for solo in [[0, 2], [2, 0]] {
            let report = |revision, statuses: [u8; 2]| crate::dongle_rpc::Cached {
                revision, age_ms: 0,
                packet: vec![4, 0, 4, 0, 6, 0, statuses[0], statuses[1]],
            };
            let removed = solo.map(|status| if status == 2 { 1 } else { status });
            let mut ears = Ears::default();
            assert_eq!(ears.observe(Some(&report(1, [0, 0]))), Ok(None));
            assert_eq!(ears.observe(Some(&report(2, solo))), Ok(None));
            assert_eq!(ears.observe(Some(&report(3, removed))), Ok(Some(EarAction::Pause)));
            assert_eq!(ears.observe(Some(&report(4, solo))), Ok(Some(EarAction::Resume)));
            assert_eq!(ears.observe(Some(&report(5, [2, 2]))), Ok(Some(EarAction::Pause)));
            assert_eq!(ears.observe(Some(&report(6, solo))), Ok(Some(EarAction::Resume)));
        }
    }

    #[test]
    fn stale_case_state_and_report_gaps_cannot_qualify_a_transitional_partner() {
        let report = |revision, age_ms, first, second| crate::dongle_rpc::Cached {
            revision, age_ms, packet: vec![4, 0, 4, 0, 6, 0, first, second],
        };
        let mut ears = Ears::default();
        assert_eq!(ears.observe(Some(&report(1, 0, 2, 0))), Ok(None));
        assert_eq!(ears.observe(Some(&report(2, 0, 2, 1))), Ok(Some(EarAction::Pause)));
        assert_eq!(ears.observe(Some(&report(2, 1501, 2, 1))), Err(()));
        assert_eq!(ears.observe(Some(&report(3, 0, 4, 0))), Ok(None));
        assert_eq!(ears.wearing, Some([crate::aap::EarStatus::Disconnected, crate::aap::EarStatus::InEar]));
        assert_eq!(ears.observe(None), Err(()));
        assert_eq!(ears.observe(Some(&report(4, 0, 0, 3))), Ok(None));
        let mut reconnected = Ears::default();
        assert_eq!(reconnected.observe(Some(&report(1, 60000, 2, 0))), Err(()));
        assert_eq!(reconnected.observe(Some(&report(2, 0, 4, 0))), Ok(None));
        assert_eq!(reconnected.wearing, Some([crate::aap::EarStatus::Disconnected, crate::aap::EarStatus::InEar]));
    }
}
