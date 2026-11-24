//! Pure planning for the captured legacy Headphone Accommodations configuration.
//! This module neither sends packets nor changes the clinical hearing modes.

const PREFIX: [u8; 9] = [4, 0, 4, 0, 0x53, 0, 0x84, 0, 2];
// Only anonymous stock curve blocks, ordered by shape then Optimization level.
const STOCK: &[u8; 1152] = include_bytes!("accommodations-stock-v1.bin");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tuning {
    BalancedTone = 1,
    VocalRange = 2,
    Brightness = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Optimization {
    Slight = 1,
    Moderate = 2,
    Strong = 3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preset {
    pub tuning: Tuning,
    pub optimization: Optimization,
}

impl Preset {
    pub const ALL: [Self; 9] = [
        Self {
            tuning: Tuning::BalancedTone,
            optimization: Optimization::Slight,
        },
        Self {
            tuning: Tuning::BalancedTone,
            optimization: Optimization::Moderate,
        },
        Self {
            tuning: Tuning::BalancedTone,
            optimization: Optimization::Strong,
        },
        Self {
            tuning: Tuning::VocalRange,
            optimization: Optimization::Slight,
        },
        Self {
            tuning: Tuning::VocalRange,
            optimization: Optimization::Moderate,
        },
        Self {
            tuning: Tuning::VocalRange,
            optimization: Optimization::Strong,
        },
        Self {
            tuning: Tuning::Brightness,
            optimization: Optimization::Slight,
        },
        Self {
            tuning: Tuning::Brightness,
            optimization: Optimization::Moderate,
        },
        Self {
            tuning: Tuning::Brightness,
            optimization: Optimization::Strong,
        },
    ];

    pub fn curves(self) -> &'static [u8; 128] {
        let index = (self.tuning as usize - 1) * 3 + self.optimization as usize - 1;
        STOCK[index * 128..(index + 1) * 128].try_into().unwrap()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidReport,
    ClinicalModesNotOff,
    InvalidReception,
    RestoreBaselineChanged,
}

/// Legacy Audiogram uses one common curve, unlike the clinical per-ear bridge.
pub(super) fn audiogram_curves(left: &[f32; 8], right: &[f32; 8]) -> Result<[u8; 128], Error> {
    if !left.iter().chain(right).all(|value| value.is_finite() && (0.0..=90.0).contains(value)) {
        return Err(Error::InvalidReport);
    }
    let pta = |ear: &[f32; 8]| [1, 2, 3, 5].into_iter().map(|index| f64::from(ear[index])).sum::<f64>() / 4.0;
    let (left_pta, right_pta) = (pta(left), pta(right));
    let common: [f64; 8] = std::array::from_fn(|index| {
        if (left_pta - right_pta).abs() < 15.0 {
            f64::from(left[index]).mul_add(0.5, f64::from(right[index]) * 0.5)
        } else { f64::from(if left_pta < right_pta { left[index] } else { right[index] }) }
    });
    let normalized: [f32; 8] = std::array::from_fn(|index| {
        // The 8 kHz endpoint fills the legacy 10 kHz key; no new measured band.
        let value = common[index];
        (if index > 0 && value == 0.0 { common[7] } else { value }) as f32
    });
    let mut curves = [0; 128];
    for (index, value) in normalized.iter().cycle().take(32).enumerate() {
        curves[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
    }
    Ok(curves)
}

/// A complete received record, including unresolved enrollment values seen on the wire.
/// Revisions are positive, monotonic counters within a session, not send counters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceivedRecord {
    packet: [u8; 140],
    session: u64,
    reception_revision: u64,
}

impl ReceivedRecord {
    pub fn parse(packet: &[u8], session: u64, reception_revision: u64) -> Result<Self, Error> {
        if session == 0 || reception_revision == 0 {
            return Err(Error::InvalidReception);
        }
        if packet.len() != 140
            || packet[..9] != PREFIX
            || packet[9] > 2
            || !packet[10..12].iter().all(|value| matches!(value, 1 | 2))
            || !packet[12..]
                .chunks_exact(4)
                .all(|value| f32::from_le_bytes(value.try_into().unwrap()).is_finite())
        {
            return Err(Error::InvalidReport);
        }
        Ok(Self {
            packet: packet.try_into().unwrap(),
            session,
            reception_revision,
        })
    }

    pub fn packet(&self) -> &[u8; 140] {
        &self.packet
    }

    pub fn session(&self) -> u64 {
        self.session
    }

    pub fn revision(&self) -> u64 {
        self.reception_revision
    }

    pub fn phone(&self) -> bool {
        self.packet[10] == 1
    }

    pub fn media(&self) -> bool {
        self.packet[11] == 1
    }

    pub fn preset(&self) -> Option<Preset> {
        Preset::ALL
            .into_iter()
            .find(|preset| self.packet[12..] == preset.curves()[..])
    }
}

/// Legacy received formats need independent clinical Off reports before writing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    record: ReceivedRecord,
}

impl Report {
    pub fn parse(
        packet: &[u8],
        session: u64,
        reception_revision: u64,
        hearing_aid_enabled: Option<bool>,
        shared_assist_enabled: Option<bool>,
    ) -> Result<Self, Error> {
        if hearing_aid_enabled != Some(false) || shared_assist_enabled != Some(false) {
            return Err(Error::ClinicalModesNotOff);
        }
        let record = ReceivedRecord::parse(packet, session, reception_revision)?;
        if !matches!(packet[9], 0 | 2) {
            return Err(Error::InvalidReport);
        }
        Ok(Self { record })
    }

    pub fn received(&self) -> &ReceivedRecord {
        &self.record
    }

    pub fn packet(&self) -> &[u8; 140] {
        self.record.packet()
    }

    pub fn session(&self) -> u64 {
        self.record.session()
    }

    pub fn revision(&self) -> u64 {
        self.record.revision()
    }

    pub fn preset(&self) -> Option<Preset> {
        self.record.preset()
    }

    pub fn plan(&self, phone: Option<bool>, media: Option<bool>, preset: Option<Preset>) -> Plan {
        let mut expected_report = *self.packet();
        if let Some(phone) = phone {
            expected_report[10] = if phone { 1 } else { 2 };
        }
        if let Some(media) = media {
            expected_report[11] = if media { 1 } else { 2 };
        }
        if let Some(preset) = preset {
            expected_report[12..].copy_from_slice(preset.curves());
        }
        let mut desired = expected_report;
        // Native commands use 2; received enrollment remains an exact, opaque baseline.
        desired[9] = 2;
        Plan {
            original: self.clone(),
            previous: *self.packet(),
            desired,
            expected_report,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SendOutcome {
    NotAttempted,
    Complete,
    Incomplete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeResult {
    NoChange,
    NotSent,
    Unconfirmed,
    Confirmed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    original: Report,
    previous: [u8; 140],
    desired: [u8; 140],
    expected_report: [u8; 140],
}

impl Plan {
    pub fn desired_packet(&self) -> &[u8; 140] {
        &self.desired
    }

    pub fn expected_report_packet(&self) -> &[u8; 140] {
        &self.expected_report
    }

    pub fn changes_required(&self) -> bool {
        self.expected_report != self.previous
    }

    /// Continue an ordered local send without treating its bytes as a device report.
    pub(super) fn then(&self, phone: Option<bool>, media: Option<bool>, preset: Option<Preset>) -> Self {
        let mut next = self.clone();
        next.previous = self.expected_report;
        if let Some(phone) = phone { next.expected_report[10] = if phone { 1 } else { 2 }; }
        if let Some(media) = media { next.expected_report[11] = if media { 1 } else { 2 }; }
        if let Some(preset) = preset { next.expected_report[12..].copy_from_slice(preset.curves()); }
        next.desired = next.expected_report;
        next.desired[9] = 2;
        next
    }

    pub(super) fn selection(&self) -> (bool, bool, Option<Preset>) {
        (self.expected_report[10] == 1, self.expected_report[11] == 1,
            Preset::ALL.into_iter().find(|preset| self.expected_report[12..] == preset.curves()[..]))
    }

    pub(super) fn with_curves(&self, curves: &[u8; 128]) -> Self {
        let mut next = self.clone();
        next.expected_report[12..].copy_from_slice(curves);
        next.desired = next.expected_report;
        next.desired[9] = 2;
        next
    }

    pub fn can_start(&self, current: &Report) -> bool {
        current == &self.original
    }

    pub fn result(&self, sent: SendOutcome, current: Option<&Report>) -> ChangeResult {
        let matches = current.is_some_and(|current| {
            current.session() == self.original.session()
                && *current.packet() == self.expected_report
        });
        if sent == SendOutcome::NotAttempted {
            return if !self.changes_required()
                && matches
                && current.unwrap().revision() >= self.original.revision()
            {
                ChangeResult::NoChange
            } else {
                ChangeResult::NotSent
            };
        }
        if sent == SendOutcome::Complete
            && self.changes_required()
            && matches
            && current.unwrap().revision() > self.original.revision()
        {
            ChangeResult::Confirmed
        } else {
            ChangeResult::Unconfirmed
        }
    }

    /// Restore only after a fresh report proves the current record is this plan's target.
    pub fn restore(&self, current: &Report) -> Result<Self, Error> {
        if current.session() != self.original.session()
            || current.revision() <= self.original.revision()
            || *current.packet() != self.expected_report
        {
            return Err(Error::RestoreBaselineChanged);
        }
        let expected_report = *self.original.packet();
        let mut desired = expected_report;
        desired[9] = 2;
        Ok(Self {
            original: current.clone(),
            previous: *current.packet(),
            desired,
            expected_report,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(preset: Preset) -> [u8; 140] {
        let mut packet = [0; 140];
        packet[..9].copy_from_slice(&PREFIX);
        packet[9..12].copy_from_slice(&[2, 1, 2]);
        packet[12..].copy_from_slice(preset.curves());
        packet
    }

    fn report(packet: &[u8], session: u64, revision: u64) -> Report {
        Report::parse(packet, session, revision, Some(false), Some(false)).unwrap()
    }

    #[test]
    fn stock_fixture_is_complete_finite_unique_and_hash_checked() {
        let hash = STOCK.iter().fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
        assert_eq!(hash, 0x29a923798db53075);
        for (index, preset) in Preset::ALL.iter().enumerate() {
            let record = report(&packet(*preset), 1, 1);
            assert_eq!(record.preset(), Some(*preset));
            for other in &Preset::ALL[index + 1..] {
                assert_ne!(preset.curves(), other.curves());
            }
        }
    }

    #[test]
    fn received_enrollment_is_preserved_and_only_legacy_zero_or_two_can_authorize_writes() {
        for enrollment in 0..=2 {
            for preset in Preset::ALL {
                let mut packet = packet(preset);
                packet[9] = enrollment;
                let received = ReceivedRecord::parse(&packet, 3, 7).unwrap();
                assert_eq!(received.packet(), &packet);
                assert_eq!((received.session(), received.revision()), (3, 7));
                assert!(received.phone());
                assert!(!received.media());
                assert_eq!(received.preset(), Some(preset));
                assert_eq!(
                    Report::parse(&packet, 3, 7, Some(false), Some(false)).is_ok(),
                    matches!(enrollment, 0 | 2)
                );
            }
        }
        let mut custom = packet(Preset::ALL[0]);
        custom[9..12].copy_from_slice(&[0, 2, 1]);
        custom[12..16].copy_from_slice(&(-0f32).to_le_bytes());
        let received = ReceivedRecord::parse(&custom, 3, 8).unwrap();
        assert!(!received.phone());
        assert!(received.media());
        assert_eq!(received.preset(), None);
    }

    #[test]
    fn received_records_reject_unknown_partial_nonfinite_and_invalid_reception() {
        let mut good = packet(Preset::ALL[0]);
        good[9] = 0;
        for length in 0..140 {
            assert_eq!(
                ReceivedRecord::parse(&good[..length], 3, 7),
                Err(Error::InvalidReport)
            );
        }
        assert_eq!(
            ReceivedRecord::parse(&[good.as_slice(), &[0]].concat(), 3, 7),
            Err(Error::InvalidReport)
        );
        for offset in 0..12 {
            let mut bad = good;
            bad[offset] = match offset {
                9 => 3,
                10 | 11 => 0,
                _ => good[offset] ^ 1,
            };
            assert_eq!(ReceivedRecord::parse(&bad, 3, 7), Err(Error::InvalidReport));
        }
        for offset in (12..140).step_by(4) {
            for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                let mut bad = good;
                bad[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
                assert_eq!(ReceivedRecord::parse(&bad, 3, 7), Err(Error::InvalidReport));
            }
        }
        assert_eq!(
            ReceivedRecord::parse(&good, 0, 7),
            Err(Error::InvalidReception)
        );
        assert_eq!(
            ReceivedRecord::parse(&good, 3, 0),
            Err(Error::InvalidReception)
        );
    }

    #[test]
    fn malformed_unknown_clinical_and_nonfinite_records_cannot_authorize_writes() {
        let good = packet(Preset::ALL[0]);
        for length in 0..140 {
            assert_eq!(
                Report::parse(&good[..length], 1, 1, Some(false), Some(false)),
                Err(Error::InvalidReport)
            );
        }
        assert_eq!(
            Report::parse(
                &[good.as_slice(), &[0]].concat(),
                1,
                1,
                Some(false),
                Some(false)
            ),
            Err(Error::InvalidReport)
        );
        for offset in 0..12 {
            let mut bad = good;
            bad[offset] = if offset < 10 { good[offset] ^ 1 } else { 0 };
            assert_eq!(
                Report::parse(&bad, 1, 1, Some(false), Some(false)),
                Err(Error::InvalidReport)
            );
        }
        for enrollment in [1, 3] {
            let mut bad = good;
            bad[9] = enrollment;
            assert_eq!(
                Report::parse(&bad, 1, 1, Some(false), Some(false)),
                Err(Error::InvalidReport)
            );
        }
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            for offset in (12..140).step_by(4) {
                let mut bad = good;
                bad[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
                assert_eq!(
                    Report::parse(&bad, 1, 1, Some(false), Some(false)),
                    Err(Error::InvalidReport)
                );
            }
        }
        for hearing in [None, Some(false), Some(true)] {
            for gate in [None, Some(false), Some(true)] {
                if hearing != Some(false) || gate != Some(false) {
                    assert_eq!(
                        Report::parse(&good, 1, 1, hearing, gate),
                        Err(Error::ClinicalModesNotOff)
                    );
                }
            }
        }
        assert_eq!(
            Report::parse(&good, 0, 1, Some(false), Some(false)),
            Err(Error::InvalidReception)
        );
        assert_eq!(
            Report::parse(&good, 1, 0, Some(false), Some(false)),
            Err(Error::InvalidReception)
        );
    }

    #[test]
    fn phone_media_and_presets_preserve_every_other_byte() {
        let original = report(&packet(Preset::ALL[0]), 3, 7);
        for phone in [None, Some(false), Some(true)] {
            for media in [None, Some(false), Some(true)] {
                let plan = original.plan(phone, media, None);
                let mut expected = *original.packet();
                if let Some(value) = phone {
                    expected[10] = if value { 1 } else { 2 };
                }
                if let Some(value) = media {
                    expected[11] = if value { 1 } else { 2 };
                }
                assert_eq!(plan.desired_packet(), &expected);
            }
        }
        for preset in Preset::ALL {
            let plan = original.plan(None, None, Some(preset));
            assert_eq!(&plan.desired_packet()[..12], &original.packet()[..12]);
            assert_eq!(&plan.desired_packet()[12..], preset.curves());
        }
        let mut custom = *original.packet();
        custom[12..16].copy_from_slice(&(-0.0f32).to_le_bytes());
        let custom = report(&custom, 3, 7);
        assert_eq!(custom.preset(), None);
        assert_eq!(
            &custom.plan(Some(false), None, None).desired_packet()[12..],
            &custom.packet()[12..]
        );
    }

    #[test]
    fn only_complete_send_and_fresh_exact_same_session_report_confirm() {
        let original = report(&packet(Preset::ALL[0]), 3, 7);
        let plan = original.plan(Some(false), Some(true), Some(Preset::ALL[8]));
        assert!(plan.can_start(&original));
        assert!(!plan.can_start(&report(original.packet(), 3, 8)));
        assert!(!plan.can_start(&report(original.packet(), 4, 7)));
        let fresh = report(plan.desired_packet(), 3, 8);
        assert_eq!(
            plan.result(SendOutcome::Complete, Some(&fresh)),
            ChangeResult::Confirmed
        );
        for sent in [SendOutcome::NotAttempted, SendOutcome::Incomplete] {
            assert_ne!(plan.result(sent, Some(&fresh)), ChangeResult::Confirmed);
        }
        for invalid in [
            None,
            Some(original),
            Some(report(plan.desired_packet(), 3, 7)),
            Some(report(plan.desired_packet(), 4, 8)),
        ] {
            assert_eq!(
                plan.result(SendOutcome::Complete, invalid.as_ref()),
                ChangeResult::Unconfirmed
            );
        }
        let mut changed = *plan.desired_packet();
        changed[139] ^= 1;
        assert_eq!(
            plan.result(SendOutcome::Complete, Some(&report(&changed, 3, 8))),
            ChangeResult::Unconfirmed
        );
    }

    #[test]
    fn no_change_is_separate_from_sent_or_confirmed() {
        let original = report(&packet(Preset::ALL[0]), 3, 7);
        let plan = original.plan(None, None, None);
        assert!(!plan.changes_required());
        assert_eq!(
            plan.result(SendOutcome::NotAttempted, Some(&original)),
            ChangeResult::NoChange
        );
        assert_eq!(
            plan.result(SendOutcome::NotAttempted, None),
            ChangeResult::NotSent
        );
        assert_eq!(
            plan.result(
                SendOutcome::Complete,
                Some(&report(original.packet(), 3, 8))
            ),
            ChangeResult::Unconfirmed
        );
    }

    #[test]
    fn legacy_zero_and_two_commands_and_expected_reports_preserve_every_other_byte() {
        for enrollment in [0, 2] {
            let mut raw = packet(Preset::ALL[0]);
            raw[9] = enrollment;
            for (index, value) in raw[12..].chunks_exact_mut(4).enumerate() {
                value.copy_from_slice(&(index as f32 / 3.0).to_le_bytes());
            }
            raw[12..16].copy_from_slice(&(-0.0f32).to_le_bytes());
            let original = report(&raw, 3, 7);
            for hearing in [None, Some(false), Some(true)] {
                for gate in [None, Some(false), Some(true)] {
                    assert_eq!(
                        Report::parse(&raw, 3, 7, hearing, gate).is_ok(),
                        hearing == Some(false) && gate == Some(false)
                    );
                }
            }
            for phone in [None, Some(false), Some(true)] {
                for media in [None, Some(false), Some(true)] {
                    for preset in [None].into_iter().chain(Preset::ALL.into_iter().map(Some)) {
                        let plan = original.plan(phone, media, preset);
                        let mut expected = raw;
                        if let Some(value) = phone {
                            expected[10] = if value { 1 } else { 2 };
                        }
                        if let Some(value) = media {
                            expected[11] = if value { 1 } else { 2 };
                        }
                        if let Some(value) = preset {
                            expected[12..].copy_from_slice(value.curves());
                        }
                        assert_eq!(plan.expected_report_packet(), &expected);
                        let mut command = expected;
                        command[9] = 2;
                        assert_eq!(plan.desired_packet(), &command);
                        assert_eq!(plan.changes_required(), expected != raw);
                        assert_eq!(original.packet(), &raw);
                    }
                }
            }
            let unchanged = original.plan(None, None, None);
            assert!(!unchanged.changes_required());
            assert_eq!(unchanged.desired_packet()[9], 2);
            assert_eq!(unchanged.expected_report_packet(), &raw);
            assert_eq!(
                unchanged.result(SendOutcome::NotAttempted, Some(&original)),
                ChangeResult::NoChange
            );
        }
    }

    #[test]
    fn normalized_confirmation_requires_exact_enrollment_and_all_other_received_bytes() {
        for enrollment in [0, 2] {
            let mut raw = packet(Preset::ALL[0]);
            raw[9] = enrollment;
            let original = report(&raw, 3, 7);
            let plan = original.plan(Some(false), Some(true), Some(Preset::ALL[8]));
            assert!(plan.can_start(&original));
            let expected = plan.expected_report_packet();
            let fresh = report(expected, 3, 8);
            assert_eq!(
                plan.result(SendOutcome::Complete, Some(&fresh)),
                ChangeResult::Confirmed
            );
            for sent in [SendOutcome::NotAttempted, SendOutcome::Incomplete] {
                assert_ne!(plan.result(sent, Some(&fresh)), ChangeResult::Confirmed);
            }
            for stale in [report(expected, 3, 7), report(expected, 4, 8)] {
                assert!(!plan.can_start(&stale));
                assert_eq!(
                    plan.result(SendOutcome::Complete, Some(&stale)),
                    ChangeResult::Unconfirmed
                );
            }
            for offset in 0..140 {
                let mut changed = *expected;
                changed[offset] = if offset == 9 {
                    if enrollment == 0 {
                        2
                    } else {
                        0
                    }
                } else {
                    changed[offset] ^ 1
                };
                if let Ok(changed) = Report::parse(&changed, 3, 8, Some(false), Some(false)) {
                    assert_eq!(
                        plan.result(SendOutcome::Complete, Some(&changed)),
                        ChangeResult::Unconfirmed,
                        "changed byte {offset}"
                    );
                    assert_eq!(plan.restore(&changed), Err(Error::RestoreBaselineChanged));
                }
            }
            for enrollment in [1, 3, 255] {
                let mut changed = *expected;
                changed[9] = enrollment;
                assert!(Report::parse(&changed, 3, 8, Some(false), Some(false)).is_err());
            }
        }
    }

    #[test]
    fn restoration_preserves_raw_zero_or_two_but_uses_the_native_command_header() {
        for enrollment in [0, 2] {
            let mut raw = packet(Preset::ALL[0]);
            raw[9] = enrollment;
            raw[12..16].copy_from_slice(&(-0.0f32).to_le_bytes());
            let original = report(&raw, 3, 7);
            let plan = original.plan(Some(false), Some(true), Some(Preset::ALL[8]));
            let applied = report(plan.expected_report_packet(), 3, 8);
            let restore = plan.restore(&applied).unwrap();
            assert!(restore.can_start(&applied));
            assert_eq!(restore.expected_report_packet(), &raw);
            let mut command = raw;
            command[9] = 2;
            assert_eq!(restore.desired_packet(), &command);
            assert_eq!(
                restore.result(SendOutcome::Complete, Some(&report(&raw, 3, 9))),
                ChangeResult::Confirmed
            );
            assert_eq!(
                restore.result(SendOutcome::Incomplete, Some(&report(&raw, 3, 9))),
                ChangeResult::Unconfirmed
            );
            assert_eq!(
                plan.restore(&report(plan.expected_report_packet(), 4, 8)),
                Err(Error::RestoreBaselineChanged)
            );
        }
    }

    #[test]
    fn restoration_requires_fresh_exact_target_and_confirms_original_bytes() {
        let mut custom = packet(Preset::ALL[0]);
        for (index, value) in custom[12..].chunks_exact_mut(4).enumerate() {
            value.copy_from_slice(&(index as f32 / 3.0).to_le_bytes());
        }
        custom[12..16].copy_from_slice(&(-0.0f32).to_le_bytes());
        let original = report(&custom, 3, 7);
        let plan = original.plan(Some(false), Some(true), Some(Preset::ALL[8]));
        assert_eq!(plan.restore(&original), Err(Error::RestoreBaselineChanged));
        assert_eq!(
            plan.restore(&report(plan.desired_packet(), 3, 7)),
            Err(Error::RestoreBaselineChanged)
        );
        assert_eq!(
            plan.restore(&report(plan.desired_packet(), 4, 8)),
            Err(Error::RestoreBaselineChanged)
        );
        let applied = report(plan.desired_packet(), 3, 8);
        let restore = plan.restore(&applied).unwrap();
        assert!(restore.can_start(&applied));
        assert_eq!(restore.desired_packet(), original.packet());
        assert_eq!(
            restore.result(
                SendOutcome::Complete,
                Some(&report(original.packet(), 3, 9))
            ),
            ChangeResult::Confirmed
        );
        assert_eq!(
            restore.result(
                SendOutcome::Incomplete,
                Some(&report(original.packet(), 3, 9))
            ),
            ChangeResult::Unconfirmed
        );
    }
}
