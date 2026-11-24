//! Received legacy PME state and stock-only changes, isolated from clinical mode controls.
use crate::{
    accommodations::{self, ChangeResult, Plan, ReceivedRecord, Report, SendOutcome},
    hearing, transparency,
};
use ntpods_ipc::{
    AccommodationsOptimization, AccommodationsPreset, AccommodationsResult, AccommodationsTuning,
    HeadphoneAccommodationsState, TransparencySettings,
};

pub(super) fn preset(value: AccommodationsPreset) -> accommodations::Preset {
    accommodations::Preset {
        tuning: match value.tuning {
            AccommodationsTuning::BalancedTone => accommodations::Tuning::BalancedTone,
            AccommodationsTuning::VocalRange => accommodations::Tuning::VocalRange,
            AccommodationsTuning::Brightness => accommodations::Tuning::Brightness,
        },
        optimization: match value.optimization {
            AccommodationsOptimization::Slight => accommodations::Optimization::Slight,
            AccommodationsOptimization::Moderate => accommodations::Optimization::Moderate,
            AccommodationsOptimization::Strong => accommodations::Optimization::Strong,
        },
    }
}

fn wire_preset(value: accommodations::Preset) -> AccommodationsPreset {
    AccommodationsPreset {
        tuning: match value.tuning {
            accommodations::Tuning::BalancedTone => AccommodationsTuning::BalancedTone,
            accommodations::Tuning::VocalRange => AccommodationsTuning::VocalRange,
            accommodations::Tuning::Brightness => AccommodationsTuning::Brightness,
        },
        optimization: match value.optimization {
            accommodations::Optimization::Slight => AccommodationsOptimization::Slight,
            accommodations::Optimization::Moderate => AccommodationsOptimization::Moderate,
            accommodations::Optimization::Strong => AccommodationsOptimization::Strong,
        },
    }
}

pub(super) fn project(
    previous: Option<HeadphoneAccommodationsState>,
    session: u64,
    received: Option<&ReceivedRecord>,
    writable: Option<&Report>,
) -> HeadphoneAccommodationsState {
    let mut state = previous
        .filter(|old| old.session == session || old.pending)
        .unwrap_or_default();
    state.session = session;
    let received = received.filter(|record| record.session() == session);
    state.phone = received.map(ReceivedRecord::phone);
    state.media = received.map(ReceivedRecord::media);
    state.preset = received.and_then(ReceivedRecord::preset).map(wire_preset);
    state.audiogram = false;
    state.audiogram_available = false;
    state.revision = received.map_or(0, ReceivedRecord::revision);
    state.edit_version = 0;
    state.requested = None;
    state.editable = received
        .is_some_and(|record| writable.is_some_and(|report| report.received() == record))
        && !state.pending;
    if received.is_some() && state.result.is_none() {
        state.result = Some(AccommodationsResult::Reported);
    }
    state
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Owner {
    pub address: u64,
    pub connection: u64,
    pub pme_reception: u64,
}

#[derive(Clone)]
pub(super) struct Edit {
    owner: Owner,
    pub original: Report,
    version: u64,
    pub plan: Plan,
    clinical: Option<Vec<u8>>,
}

impl Edit {
    pub fn source_reception(&self) -> u64 { self.owner.pme_reception }
}

struct Sent {
    plan: Plan,
    clinical: Vec<u8>,
    audiogram: bool,
}

struct AudiogramSource {
    owner: Owner,
    session: u64,
    profile: Vec<u8>,
    curves: [u8; 128],
}

/// Local send ordering never advances a device reception revision.
#[derive(Default)]
pub(super) struct Edits {
    anchor: Option<(Owner, Report)>,
    version: u64,
    sent: Option<Sent>,
    blocked: Option<Owner>,
    source: Option<AudiogramSource>,
}

impl Edits {
    fn reconcile(&mut self, owner: Option<Owner>, report: Option<&Report>) -> Option<AccommodationsResult> {
        // AHPS can arrive before the initial AACP reports. Keep that validated
        // source hidden until the first complete legacy report supplies its guards.
        if report.is_none() && self.anchor.is_some() || self.source.as_ref().is_some_and(|source|
            owner.is_none_or(|owner| owner.address != source.owner.address || owner.connection != source.owner.connection)) {
            self.source = None;
        }
        if self.anchor.as_ref().is_some_and(|(previous_owner, previous)|
            Some(*previous_owner) == owner && Some(previous) == report) {
            return None;
        }
        let same_reception = self.anchor.as_ref().is_some_and(|(previous_owner, _)| Some(*previous_owner) == owner);
        let result = self.sent.as_ref().map(|sent| {
            if same_reception { return AccommodationsResult::Unconfirmed; }
            let confirmed = self.anchor.as_ref().is_some_and(|(previous_owner, previous)|
                owner.is_some_and(|owner| owner.address == previous_owner.address && owner.connection == previous_owner.connection)
                    && report.is_some_and(|current|
                    current.session() == previous.session() && current.revision() > previous.revision()
                        && current.packet() == sent.plan.expected_report_packet()));
            if confirmed { AccommodationsResult::Confirmed } else { AccommodationsResult::Reported }
        }).or_else(|| self.blocked.filter(|blocked| Some(*blocked) != owner).map(|_| AccommodationsResult::Reported));
        if self.sent.is_some() && same_reception {
            self.blocked = owner;
            self.version = self.version.saturating_add(1);
        } else if self.blocked.is_some_and(|blocked| Some(blocked) != owner) {
            self.blocked = None;
            self.version = 0;
        } else if self.blocked.is_none() {
            self.version = 0;
        }
        self.anchor = owner.zip(report.cloned());
        self.sent = None;
        result
    }

    pub fn project(&mut self, owner: Option<Owner>, report: Option<&Report>, state: &mut HeadphoneAccommodationsState) {
        if let Some(result) = self.reconcile(owner, report) {
            state.result = report.map(|_| result);
            state.error = None;
        }
        state.edit_version = self.version;
        let source = self.source.as_ref().filter(|source| owner.is_some_and(|owner|
            owner.address == source.owner.address && owner.connection == source.owner.connection)
            && report.is_some_and(|report| report.session() == source.session));
        state.audiogram_available = source.is_some();
        state.audiogram = source.is_some_and(|source| report.is_some_and(|report| report.packet()[12..] == source.curves));
        if state.audiogram { state.preset = None; }
        state.requested = self.sent.as_ref().map(|sent| {
            let (phone, media, preset) = sent.plan.selection();
            ntpods_ipc::AccommodationsSelection { phone, media,
                preset: if sent.audiogram { None } else { preset.map(wire_preset) }, audiogram: sent.audiogram }
        });
        state.editable &= self.blocked.is_none();
    }

    pub fn prepare(&mut self, owner: Owner, report: &Report, version: u64,
        phone: Option<bool>, media: Option<bool>, preset: Option<accommodations::Preset>) -> Result<Edit, String> {
        self.reconcile(Some(owner), Some(report));
        if self.blocked.is_some() {
            return Err("Reconnect the AirPods before changing Headphone Accommodations again.".into());
        }
        if version != self.version {
            return Err("Headphone Accommodations changed. Try your change again.".into());
        }
        Ok(Edit { owner, original: report.clone(), version,
            plan: self.sent.as_ref().map_or_else(|| report.plan(phone, media, preset),
                |sent| sent.plan.then(phone, media, preset)),
            clinical: self.sent.as_ref().map(|sent| sent.clinical.clone()).or_else(|| self.source.as_ref().filter(|source|
                source.owner.address == owner.address && source.owner.connection == owner.connection
                    && source.session == report.session()).map(|source| source.profile.clone())) })
    }

    pub fn current(&self, edit: &Edit, owner: Owner, report: Option<&Report>) -> bool {
        self.blocked.is_none() && edit.owner == owner && edit.version == self.version
            && self.anchor.as_ref().is_some_and(|(saved_owner, saved)| *saved_owner == owner && saved == &edit.original)
            && report == Some(&edit.original)
    }

    pub fn complete(&mut self, edit: &Edit, change: &Change) -> Result<AccommodationsResult, String> {
        if !self.current(edit, edit.owner, Some(&edit.original)) {
            return Err("Headphone Accommodations changed before the send finished.".into());
        }
        if change.error.is_some() || (change.plan.changes_required() && change.sent != SendOutcome::Complete) {
            return Err("Headphone Accommodations was not completely sent. Reconnect before trying again.".into());
        }
        if let Some(profile) = change.audiogram_source.as_ref().map(|(profile, _)| profile).or(change.clinical.as_ref()) {
            let _ = self.record_source(edit.owner, edit.original.session(), profile.clone());
        }
        if !change.verification_required { return Ok(AccommodationsResult::NoChange); }
        if change.plan.changes_required() || self.sent.is_some() {
            let clinical = change.clinical.clone().ok_or("The hearing profile was not verified.")?;
            let version = self.version.checked_add(1).ok_or("Reconnect the AirPods before changing Headphone Accommodations again.")?;
            self.version = version;
            let audiogram = change.audiogram_source.is_some() || (change.retain_audiogram &&
                (self.sent.as_ref().is_some_and(|sent| sent.audiogram) || self.source.as_ref().is_some_and(|source|
                    source.owner.address == edit.owner.address && source.owner.connection == edit.owner.connection
                        && source.session == edit.original.session() && change.plan.expected_report_packet()[12..] == source.curves)));
            self.sent = Some(Sent { plan: change.plan.clone(),
                clinical, audiogram });
            Ok(AccommodationsResult::Sent)
        } else {
            Ok(AccommodationsResult::Confirmed)
        }
    }

    pub fn fail(&mut self, edit: &Edit) {
        if self.current(edit, edit.owner, Some(&edit.original)) {
            self.source = None;
            self.sent = None;
            self.version = self.version.saturating_add(1);
            self.blocked = Some(edit.owner);
        }
    }

    pub fn clear(&mut self) { *self = Self::default(); }
    pub fn has_sent(&self) -> bool { self.sent.is_some() }
    pub fn forget_source(&mut self) { self.source = None; }
    pub fn record_source(&mut self, owner: Owner, session: u64, profile: Vec<u8>) -> Result<(), String> {
        self.source = None;
        let curves = hearing::legacy_audiogram_curves(&profile)?;
        self.source = Some(AudiogramSource { owner, session, profile, curves });
        Ok(())
    }
}

pub(super) struct Change {
    pub original: Report,
    pub plan: Plan,
    clinical: Option<Vec<u8>>,
    transparency: Option<transparency::StockCurvePlan>,
    unchanged_transparency: Option<Vec<u8>>,
    audiogram_source: Option<(Vec<u8>, [u8; 128])>,
    retain_audiogram: bool,
    sent: SendOutcome,
    error: Option<String>,
    pub verification_required: bool,
}

impl Change {
    pub fn result(&self) -> AccommodationsResult {
        if !self.verification_required {
            AccommodationsResult::NoChange
        } else if self.sent == SendOutcome::NotAttempted
            && self.error.is_some()
            && self.transparency.is_none()
        {
            AccommodationsResult::NotSent
        } else {
            AccommodationsResult::Unconfirmed
        }
    }

    pub fn matches_report(&self, report: &Report, refreshed_session: Option<u64>) -> bool {
        let same_session = self.plan.result(self.sent, Some(report)) == ChangeResult::Confirmed
            || (!self.plan.changes_required()
                && report.session() == self.original.session()
                && report.revision() >= self.original.revision()
                && report.packet() == self.plan.expected_report_packet());
        let refreshed = refreshed_session == Some(report.session())
            && report.session() != self.original.session()
            && report.packet() == self.plan.expected_report_packet();
        let sent = !self.plan.changes_required() || self.sent == SendOutcome::Complete;
        sent && (same_session || refreshed)
    }

    pub fn verify(
        &self,
        report: &Report,
        refreshed_session: Option<u64>,
        current: impl Fn() -> bool,
        exchange: &mut dyn FnMut(&[u8]) -> Result<Vec<u8>, String>,
    ) -> Result<Option<TransparencySettings>, String> {
        if !current() || !self.matches_report(report, refreshed_session) {
            return Err("Headphone Accommodations was sent, but a fresh complete device report did not confirm it. Refresh device settings before trying again.".into());
        }
        self.verify_sent(current, exchange)
    }

    pub fn verify_sent(&self, current: impl Fn() -> bool,
        exchange: &mut dyn FnMut(&[u8]) -> Result<Vec<u8>, String>) -> Result<Option<TransparencySettings>, String> {
        if !current() { return Err("Headphone Accommodations changed during the send.".into()); }
        if let Some(original) = &self.clinical {
            hearing::verify_profile(original, |request| exchange(request))?;
        }
        let transparency = self
            .transparency
            .as_ref()
            .map(|plan| plan.verify(exchange))
            .transpose()?;
        let transparency = if let Some(original) = &self.unchanged_transparency {
            let (_, value, settings) = transparency::read_record(exchange)?;
            if &value != original { return Err("Custom Transparency changed during the send. Reconnect before trying again.".into()); }
            Some(settings)
        } else { transparency };
        if !current() {
            return Err("Headphone Accommodations changed during confirmation. Reconnect before trying again.".into());
        }
        if let Some(error) = &self.error {
            return Err(format!("The change was incomplete ({error}). The current device state was refreshed; review it before trying again."));
        }
        Ok(transparency)
    }
}

pub(super) fn execute(
    original: Report,
    phone: Option<bool>,
    media: Option<bool>,
    stock: Option<accommodations::Preset>,
    current: impl Fn() -> bool,
    reserve: impl FnOnce(bool) -> Result<(), String>,
    send: impl FnMut(&[u8]) -> Result<(), String>,
    exchange: &mut dyn FnMut(&[u8]) -> Result<Vec<u8>, String>,
) -> Result<Change, String> {
    let plan = original.plan(phone, media, stock);
    execute_plan(original, plan, stock, None, None, current, reserve, send, exchange)
}

pub(super) fn execute_edit(
    edit: &Edit, stock: Option<accommodations::Preset>, audiogram: bool, current: impl Fn() -> bool,
    reserve: impl FnOnce(bool) -> Result<(), String>, send: impl FnMut(&[u8]) -> Result<(), String>,
    exchange: &mut dyn FnMut(&[u8]) -> Result<Vec<u8>, String>,
) -> Result<Change, String> {
    if audiogram && stock.is_some() { return Err("Choose either Audiogram or a stock tuning.".into()); }
    if !current() { return Err("AirPods connection changed.".into()); }
    let source = if audiogram {
        let profile = hearing::read_settings(&mut |packet| exchange(packet))?.1;
        let curves = hearing::legacy_audiogram_curves(&profile)?;
        if edit.clinical.as_ref().is_some_and(|previous| previous != &profile) {
            return Err("The hearing profile changed. Reconnect the AirPods before changing Headphone Accommodations again.".into());
        }
        Some((profile, curves))
    } else { None };
    let plan = source.as_ref().map_or_else(|| edit.plan.clone(), |(_, curves)| edit.plan.with_curves(curves));
    let unchanged = if stock.is_none() && !audiogram && plan.changes_required() {
        Some(transparency::read_record(exchange)?.1)
    } else { None };
    let mut change = execute_plan(edit.original.clone(), plan, stock, source.as_ref().map(|(_, curves)| curves),
        source.as_ref().map(|(profile, _)| profile.as_slice()).or(edit.clinical.as_deref()),
        current, reserve, send, exchange)?;
    change.unchanged_transparency = unchanged;
    change.audiogram_source = source;
    change.retain_audiogram = stock.is_none();
    Ok(change)
}

fn execute_plan(
    original: Report, plan: Plan, stock: Option<accommodations::Preset>, audiogram: Option<&[u8; 128]>, expected_clinical: Option<&[u8]>,
    current: impl Fn() -> bool, reserve: impl FnOnce(bool) -> Result<(), String>,
    mut send: impl FnMut(&[u8]) -> Result<(), String>,
    exchange: &mut dyn FnMut(&[u8]) -> Result<Vec<u8>, String>,
) -> Result<Change, String> {
    if !current() {
        return Err("AirPods connection changed.".into());
    }
    let transparency = if let Some(curves) = audiogram {
        Some(transparency::prepare_audiogram_curves(exchange, curves)?)
    } else { stock.map(|preset| transparency::prepare_stock_curves(exchange, preset)).transpose()? };
    let changes = plan.changes_required()
        || transparency
            .as_ref()
            .is_some_and(|plan| plan.changes_required());
    let mut change = Change {
        original,
        plan,
        clinical: None,
        transparency,
        unchanged_transparency: None,
        audiogram_source: None,
        retain_audiogram: false,
        sent: SendOutcome::NotAttempted,
        error: None,
        verification_required: changes,
    };
    if !changes {
        if let Some(expected) = expected_clinical {
            let profile = hearing::read_settings(&mut |request| exchange(request))?.1;
            if profile != expected { return Err("The hearing profile changed. Reconnect before trying again.".into()); }
            change.clinical = Some(profile);
        }
        return Ok(change);
    }
    change.clinical = Some(hearing::read_settings(&mut |request| exchange(request))?.1);
    if expected_clinical.is_some_and(|expected| change.clinical.as_deref() != Some(expected)) {
        return Err("The hearing profile changed. Reconnect the AirPods before changing Headphone Accommodations again.".into());
    }
    if !current() {
        return Err("AirPods connection changed before the request started.".into());
    }
    reserve(change.plan.changes_required())?;
    if !current() {
        change.error = Some("AirPods connection changed before sending.".into());
        return Ok(change);
    }
    if change.plan.changes_required() {
        match send(change.plan.desired_packet()) {
            Ok(()) => change.sent = SendOutcome::Complete,
            Err(error) => {
                change.sent = SendOutcome::Incomplete;
                change.error = Some(error);
                return Ok(change);
            }
        }
    }
    if let Some(plan) = &change.transparency {
        if !current() {
            change.error = Some("AirPods connection changed after sending.".into());
        } else if let Err(error) = plan.apply(exchange) {
            change.error = Some(error);
        }
    }
    Ok(change)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    const CT: [u8; 16] = [
        0x90, 0x53, 0x4d, 0x74, 0xaf, 0x9e, 3, 0x94, 0xdb, 0x45, 0xab, 0xf7, 0xc1, 0x1c, 0x62, 0xd5,
    ];
    const AHPS: [u8; 16] = [
        0x0a, 0x7e, 0x45, 0x41, 0x0b, 0x0f, 0x98, 0x90, 0x6f, 0x4d, 0xc5, 0x95, 5, 0, 0x12, 0xa4,
    ];

    struct Fake {
        ct: Vec<u8>,
        clinical: Vec<u8>,
        writes: usize,
        fail_write: bool,
    }
    impl Fake {
        fn new() -> Self {
            let mut ct = vec![0; 104];
            for (offset, value) in [
                (0, 1.0f32),
                (36, 2.0),
                (40, -0.5),
                (44, 1.0),
                (48, 0.75),
                (84, -0.0),
                (88, 0.25),
                (92, 0.0),
                (96, 0.5),
                (100, 0.5),
            ] {
                ct[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            }
            let mut clinical = vec![0; 104];
            clinical[..4].copy_from_slice(&[2, 2, 0x60, 0]);
            clinical[100..104].copy_from_slice(&0.5f32.to_le_bytes());
            Self {
                ct,
                clinical,
                writes: 0,
                fail_write: false,
            }
        }
        fn exchange(&mut self, pdu: &[u8]) -> Result<Vec<u8>, String> {
            match pdu {
                [6, ..] => Ok(vec![7, 0x30, 0, 0x40, 0]),
                [8, 0x30, 0, ..] => {
                    let mut result = vec![9, 21];
                    for (handle, uuid) in [(0x35u16, CT), (0x39, AHPS)] {
                        result.extend_from_slice(&(handle - 1).to_le_bytes());
                        result.push(0x0a);
                        result.extend_from_slice(&handle.to_le_bytes());
                        result.extend_from_slice(&uuid);
                    }
                    Ok(result)
                }
                [8, start, high, ..] => Ok(vec![1, 8, *start, *high, 0x0a]),
                [0x0a, handle @ (0x35 | 0x39), 0] => {
                    let mut result = vec![0x0b];
                    result.extend_from_slice(if *handle == 0x35 {
                        &self.ct
                    } else {
                        &self.clinical
                    });
                    Ok(result)
                }
                [0x12, 0x35, 0, value @ ..] => {
                    assert_eq!(value.len(), 104);
                    self.writes += 1;
                    self.ct = value.to_vec();
                    if self.fail_write {
                        Err("Simulated incomplete CT write".into())
                    } else {
                        Ok(vec![0x13])
                    }
                }
                _ => panic!("Unexpected ATT operation; clinical writes are forbidden: {pdu:?}"),
            }
        }
    }
    fn report(packet: &[u8], session: u64, revision: u64) -> Report {
        Report::parse(packet, session, revision, Some(false), Some(false)).unwrap()
    }
    fn original() -> Report {
        let mut packet = [0; 140];
        packet[..12].copy_from_slice(&[4, 0, 4, 0, 0x53, 0, 0x84, 0, 2, 2, 1, 2]);
        packet[12..].copy_from_slice(accommodations::Preset::ALL[0].curves());
        report(&packet, 3, 7)
    }

    #[test]
    fn flag_edit_sends_one_preserved_legacy_record_and_needs_actual_reception() {
        let original = original();
        let fake = RefCell::new(Fake::new());
        let sent = RefCell::new(Vec::new());
        let change = execute(
            original.clone(),
            Some(false),
            Some(true),
            None,
            || true,
            |invalidate| {
                assert!(invalidate);
                Ok(())
            },
            |packet| {
                sent.borrow_mut().push(packet.to_vec());
                Ok(())
            },
            &mut |request| fake.borrow_mut().exchange(request),
        )
        .unwrap();
        assert_eq!(sent.borrow().len(), 1);
        assert_eq!(&sent.borrow()[0][..10], &original.packet()[..10]);
        assert_eq!(&sent.borrow()[0][12..], &original.packet()[12..]);
        assert_eq!(fake.borrow().writes, 0);
        assert_eq!(change.result(), AccommodationsResult::Unconfirmed);
        let actual = report(change.plan.desired_packet(), 3, 8);
        assert!(change
            .verify(&actual, None, || true, &mut |request| fake
                .borrow_mut()
                .exchange(request))
            .is_ok());
        assert!(change
            .verify(
                &report(change.plan.desired_packet(), 3, 7),
                None,
                || true,
                &mut |_| panic!("Stale readback reached ATT")
            )
            .is_err());
        assert!(change
            .verify(
                &report(change.plan.desired_packet(), 4, 8),
                None,
                || true,
                &mut |_| panic!("An unreserved replacement session reached ATT")
            )
            .is_err());
        assert!(change
            .verify(
                &report(change.plan.desired_packet(), 4, 1),
                Some(4),
                || true,
                &mut |request| fake.borrow_mut().exchange(request)
            )
            .is_ok());
    }

    #[test]
    fn normalized_zero_or_two_reports_confirm_only_the_exact_owned_target() {
        for enrollment in [0, 2] {
            let mut packet = *original().packet();
            packet[9] = enrollment;
            packet[12..16].copy_from_slice(&(-0f32).to_le_bytes());
            let original = report(&packet, 3, 7);
            let fake = RefCell::new(Fake::new());
            let sent = RefCell::new(Vec::new());
            let change = execute(
                original.clone(),
                Some(false),
                Some(true),
                None,
                || true,
                |invalidate| {
                    assert!(invalidate);
                    Ok(())
                },
                |packet| {
                    sent.borrow_mut().push(packet.to_vec());
                    Ok(())
                },
                &mut |request| fake.borrow_mut().exchange(request),
            )
            .unwrap();
            let mut command = *change.plan.expected_report_packet();
            command[9] = 2;
            assert_eq!(*sent.borrow(), vec![command.to_vec()]);
            assert_eq!(&command[12..], &original.packet()[12..]);
            for (session, revision, refreshed, matches) in [
                (3, 7, None, false),
                (3, 8, None, true),
                (4, 1, None, false),
                (4, 1, Some(4), true),
                (5, 1, Some(4), false),
            ] {
                let actual = report(change.plan.expected_report_packet(), session, revision);
                assert_eq!(change.matches_report(&actual, refreshed), matches);
                if matches {
                    assert!(change
                        .verify(&actual, refreshed, || true, &mut |request| fake
                            .borrow_mut()
                            .exchange(request))
                        .is_ok());
                }
            }
            let actual = report(change.plan.expected_report_packet(), 4, 1);
            assert!(change
                .verify(&actual, Some(4), || false, &mut |_| panic!(
                    "A cancelled owner reached ATT"
                ))
                .is_err());
            let mut changed = *change.plan.expected_report_packet();
            changed[9] = if enrollment == 0 { 2 } else { 0 };
            assert!(!change.matches_report(&report(&changed, 4, 1), Some(4)));
            assert_eq!(fake.borrow().writes, 0);
        }
    }

    #[test]
    fn every_stock_preset_changes_only_ct_curves_and_preserves_clinical_bytes() {
        for stock in accommodations::Preset::ALL {
            let fake = RefCell::new(Fake::new());
            let ct = fake.borrow().ct.clone();
            let clinical = fake.borrow().clinical.clone();
            let change = execute(
                original(),
                None,
                None,
                Some(stock),
                || true,
                |_| Ok(()),
                |_| Ok(()),
                &mut |request| fake.borrow_mut().exchange(request),
            )
            .unwrap();
            let changed = fake.borrow().ct.clone();
            assert_eq!(&changed[4..36], &stock.curves()[..32]);
            assert_eq!(&changed[52..84], &stock.curves()[64..96]);
            for index in
                (0..104).filter(|index| !(4..36).contains(index) && !(52..84).contains(index))
            {
                assert_eq!(changed[index], ct[index], "untouched byte {index}");
            }
            let actual = report(change.plan.desired_packet(), 3, 8);
            assert!(change
                .verify(&actual, None, || true, &mut |request| fake
                    .borrow_mut()
                    .exchange(request))
                .is_ok());
            assert_eq!(fake.borrow().clinical, clinical);
        }
    }

    #[test]
    fn stale_cancelled_and_failed_reservations_never_send_or_write() {
        let fake = RefCell::new(Fake::new());
        assert!(execute(
            original(),
            Some(false),
            None,
            None,
            || false,
            |_| panic!("A stale request reserved"),
            |_| panic!("A stale request sent"),
            &mut |_| panic!("A stale request read ATT")
        )
        .is_err());
        assert!(execute(
            original(),
            Some(false),
            None,
            None,
            || true,
            |_| Err("Baseline changed".into()),
            |_| panic!("An invalid baseline sent"),
            &mut |request| fake.borrow_mut().exchange(request)
        )
        .is_err());
        let current = Cell::new(true);
        let cancelled = execute(
            original(),
            Some(false),
            None,
            None,
            || current.get(),
            |_| {
                current.set(false);
                Ok(())
            },
            |_| panic!("A cancelled request sent"),
            &mut |request| fake.borrow_mut().exchange(request),
        )
        .unwrap();
        assert_eq!(cancelled.result(), AccommodationsResult::NotSent);
        assert_eq!(fake.borrow().writes, 0);
    }

    #[test]
    fn partial_pme_send_never_confirms_or_continues_into_ct() {
        let fake = RefCell::new(Fake::new());
        let change = execute(
            original(),
            Some(false),
            None,
            Some(accommodations::Preset::ALL[8]),
            || true,
            |_| Ok(()),
            |_| Err("Simulated incomplete send".into()),
            &mut |request| fake.borrow_mut().exchange(request),
        )
        .unwrap();
        assert_eq!(fake.borrow().writes, 0);
        assert_eq!(change.result(), AccommodationsResult::Unconfirmed);
        assert!(change
            .verify(
                &report(change.plan.desired_packet(), 4, 1),
                Some(4),
                || true,
                &mut |_| panic!("An incomplete send cannot confirm")
            )
            .is_err());
    }

    #[test]
    fn partial_ct_write_never_rolls_back_unproven_pme_or_claims_confirmation() {
        let fake = RefCell::new(Fake {
            fail_write: true,
            ..Fake::new()
        });
        let change = execute(
            original(),
            None,
            None,
            Some(accommodations::Preset::ALL[8]),
            || true,
            |_| Ok(()),
            |_| Ok(()),
            &mut |request| fake.borrow_mut().exchange(request),
        )
        .unwrap();
        assert_eq!(fake.borrow().writes, 1);
        assert!(change
            .verify(
                &report(change.plan.desired_packet(), 4, 1),
                Some(4),
                || true,
                &mut |request| fake.borrow_mut().exchange(request)
            )
            .is_err());
        assert_eq!(fake.borrow().writes, 1);
    }

    #[test]
    fn a_changed_clinical_profile_prevents_confirmation_without_any_clinical_write() {
        let fake = RefCell::new(Fake::new());
        let change = execute(
            original(),
            Some(false),
            None,
            None,
            || true,
            |_| Ok(()),
            |_| Ok(()),
            &mut |request| fake.borrow_mut().exchange(request),
        )
        .unwrap();
        fake.borrow_mut().clinical[4..8].copy_from_slice(&1f32.to_le_bytes());
        assert!(change
            .verify(
                &report(change.plan.desired_packet(), 4, 1),
                Some(4),
                || true,
                &mut |request| fake.borrow_mut().exchange(request)
            )
            .is_err());
        assert_eq!(fake.borrow().writes, 0);
    }

    #[test]
    fn a_new_pme_report_during_attribute_verification_cannot_confirm_the_old_target() {
        let fake = RefCell::new(Fake::new());
        let change = execute(
            original(),
            Some(false),
            None,
            None,
            || true,
            |_| Ok(()),
            |_| Ok(()),
            &mut |request| fake.borrow_mut().exchange(request),
        )
        .unwrap();
        let target = report(change.plan.desired_packet(), 3, 8);
        let current = RefCell::new(target.clone());
        let mut changed = *change.plan.desired_packet();
        changed[10] = if changed[10] == 1 { 2 } else { 1 };
        assert!(change
            .verify(
                &target,
                None,
                || change.matches_report(&current.borrow(), None),
                &mut |request| {
                    if request == [0x0a, 0x39, 0] {
                        *current.borrow_mut() = report(&changed, 3, 9);
                    }
                    fake.borrow_mut().exchange(request)
                }
            )
            .is_err());
        assert_eq!(fake.borrow().writes, 0);
    }

    #[test]
    fn received_values_need_current_clinical_guards_and_never_cross_sessions() {
        let mut packet = *original().packet();
        packet[9] = 0;
        let received = ReceivedRecord::parse(&packet, 3, 7).unwrap();
        let state = project(None, 3, Some(&received), None);
        assert_eq!(
            (state.session, state.revision, state.phone, state.media),
            (3, 7, Some(true), Some(false))
        );
        assert_eq!(
            state.preset,
            Some(wire_preset(accommodations::Preset::ALL[0]))
        );
        assert_eq!(state.result, Some(AccommodationsResult::Reported));
        assert!(!state.editable && !state.pending && state.error.is_none());
        assert!(Report::parse(received.packet(), 3, 7, None, Some(false)).is_err());
        let normalized = report(received.packet(), 3, 7);
        assert_eq!(normalized.packet()[9], 0);
        assert!(project(None, 3, Some(&received), Some(&normalized)).editable);

        let writable = original();
        assert!(!project(None, 3, Some(&received), Some(&writable)).editable);
        let state = project(Some(state), 4, Some(&received), Some(&writable));
        assert_eq!(
            (state.revision, state.phone, state.media, state.preset),
            (0, None, None, None)
        );
        assert!(!state.editable);
        assert_eq!(state.result, None);

        let state = project(None, 3, Some(writable.received()), None);
        assert_eq!(state.phone, Some(true));
        assert!(!state.editable);
        assert!(project(None, 3, Some(writable.received()), Some(&writable)).editable);
    }

    #[test]
    fn no_change_and_unknown_curves_remain_distinct() {
        let mut packet = *original().packet();
        packet[9] = 0;
        packet[12..16].copy_from_slice(&(-0f32).to_le_bytes());
        let original = report(&packet, 3, 7);
        let state = project(None, 3, Some(original.received()), Some(&original));
        assert!(state.preset.is_none());
        let change = execute(
            original.clone(),
            None,
            None,
            None,
            || true,
            |_| panic!("No change reserved"),
            |_| panic!("No change sent"),
            &mut |_| panic!("No change read ATT"),
        )
        .unwrap();
        assert_eq!(change.result(), AccommodationsResult::NoChange);
        let stale = project(Some(state), 4, None, None);
        assert!(!stale.editable && stale.phone.is_none() && stale.preset.is_none());
    }

    fn owner() -> Owner { Owner { address: 42, connection: 7, pme_reception: 1 } }

    #[test]
    fn initial_profile_waits_for_complete_mode_reports_without_losing_its_source() {
        let mut edits = Edits::default();
        let original = original();
        edits.record_source(owner(), original.session(), Fake::new().clinical).unwrap();
        let mut view = project(None, original.session(), None, None);
        edits.project(Some(owner()), None, &mut view);
        assert!(!view.audiogram_available && !view.editable);
        assert!(edits.source.is_some());
        view = project(Some(view), original.session(), Some(original.received()), Some(&original));
        edits.project(Some(owner()), Some(&original), &mut view);
        assert!(view.audiogram_available);
        edits.project(Some(owner()), None, &mut view);
        assert!(edits.source.is_none());
        edits.record_source(owner(), original.session(), Fake::new().clinical).unwrap();
        edits.project(Some(Owner { connection: 8, ..owner() }), None, &mut view);
        assert!(edits.source.is_none());
    }

    fn perform_edit(edits: &mut Edits, original: &Report, version: u64, phone: bool, media: bool,
        fake: &RefCell<Fake>, packets: &RefCell<Vec<Vec<u8>>>) -> Edit {
        let edit = edits.prepare(owner(), original, version, Some(phone), Some(media), None).unwrap();
        let change = execute_edit(&edit, None, false, || true, |_| Ok(()),
            |packet| { packets.borrow_mut().push(packet.to_vec()); Ok(()) },
            &mut |request| fake.borrow_mut().exchange(request)).unwrap();
        change.verify_sent(|| true, &mut |request| fake.borrow_mut().exchange(request)).unwrap();
        assert_eq!(edits.complete(&edit, &change).unwrap(), AccommodationsResult::Sent);
        edit
    }

    #[test]
    fn ordered_silent_on_off_sends_preserve_received_bytes_and_reject_stale_clients_and_aba() {
        let mut packet = *original().packet();
        packet[9..12].copy_from_slice(&[0, 2, 2]);
        packet[12..16].copy_from_slice(&(-0f32).to_le_bytes());
        let original = report(&packet, 3, 7);
        let mut edits = Edits::default();
        let fake = RefCell::new(Fake::new());
        let clinical = fake.borrow().clinical.clone();
        let ct = fake.borrow().ct.clone();
        let packets = RefCell::new(Vec::new());
        perform_edit(&mut edits, &original, 0, true, true, &fake, &packets);
        let mut view = project(None, 3, Some(original.received()), Some(&original));
        edits.project(Some(owner()), Some(&original), &mut view);
        assert_eq!((view.revision, view.phone, view.media), (7, Some(false), Some(false)));
        let requested = view.requested.unwrap();
        assert!(requested.phone && requested.media);
        assert!(requested.preset.is_none());
        assert_eq!(view.edit_version, 1);
        assert!(view.editable);
        assert!(edits.prepare(owner(), &original, 0, Some(false), None, None).is_err());
        perform_edit(&mut edits, &original, 1, false, false, &fake, &packets);
        assert_eq!(edits.version, 2);
        assert!(edits.prepare(owner(), &original, 0, Some(true), None, None).is_err());
        assert!(edits.prepare(owner(), &original, 1, Some(true), None, None).is_err());
        for (index, sent) in packets.borrow().iter().enumerate() {
            let mut expected = packet;
            expected[9] = 2;
            expected[10..12].fill(if index == 0 { 1 } else { 2 });
            assert_eq!(sent, &expected);
        }
        assert_eq!(fake.borrow().clinical, clinical);
        assert_eq!(fake.borrow().ct, ct);
        assert_eq!(fake.borrow().writes, 0);
    }

    #[test]
    fn fresh_reports_clinical_guards_and_owner_changes_retire_requested_state() {
        let original = original();
        for invalidation in 0..5 {
            let mut edits = Edits::default();
            let fake = RefCell::new(Fake::new());
            let packets = RefCell::new(Vec::new());
            let edit = perform_edit(&mut edits, &original, 0, false, true, &fake, &packets);
            let target = report(edit.plan.expected_report_packet(), 3, 8);
            let replacement = report(original.packet(), 4, 1);
            let (next_owner, next_report) = match invalidation {
                0 => (Some(Owner { pme_reception: 2, ..owner() }), Some(&target)),
                1 => (Some(owner()), None),
                2 => (Some(Owner { address: 43, ..owner() }), Some(&original)),
                3 => (Some(Owner { pme_reception: 2, ..owner() }), Some(&replacement)),
                _ => (None, None),
            };
            let mut view = project(None, next_report.map_or(0, Report::session),
                next_report.map(Report::received), next_report);
            edits.project(next_owner, next_report, &mut view);
            assert!(view.requested.is_none());
            assert_eq!(view.edit_version, if invalidation == 1 { 2 } else { 0 });
            assert!(!edits.current(&edit, owner(), Some(&original)));
            assert_eq!(edits.has_sent(), false);
            if invalidation == 0 { assert_eq!(view.result, Some(AccommodationsResult::Confirmed)); }
            if invalidation == 1 { assert!(!view.editable); }
        }
    }

    #[test]
    fn failed_send_or_attribute_proof_blocks_reuse_until_a_new_received_report() {
        for failure in 0..3 {
            let original = original();
            let mut edits = Edits::default();
            let edit = edits.prepare(owner(), &original, 0, Some(false), Some(true), None).unwrap();
            let fake = RefCell::new(Fake::new());
            let change = execute_edit(&edit, None, false, || true, |_| Ok(()),
                |_| if failure == 0 { Err("Incomplete send".into()) } else { Ok(()) },
                &mut |request| fake.borrow_mut().exchange(request)).unwrap();
            if failure == 1 { fake.borrow_mut().clinical[4..8].copy_from_slice(&1f32.to_le_bytes()); }
            if failure == 2 { fake.borrow_mut().ct[52..56].copy_from_slice(&1f32.to_le_bytes()); }
            assert!(change.verify_sent(|| true, &mut |request| fake.borrow_mut().exchange(request)).is_err());
            edits.fail(&edit);
            assert!(edits.prepare(owner(), &original, 0, None, None, None).is_err());
            assert!(edits.prepare(owner(), &original, 1, None, None, None).is_err());
            let mut view = project(None, 3, Some(original.received()), Some(&original));
            edits.project(Some(owner()), Some(&original), &mut view);
            assert!(!view.editable && view.requested.is_none());
            let fresh = report(original.packet(), 3, 8);
            assert!(edits.prepare(Owner { pme_reception: 2, ..owner() }, &fresh, 0, Some(false), None, None).is_ok());
        }
    }

    #[test]
    fn cancellation_and_changed_profile_never_extend_a_successful_sent_chain() {
        let original = original();
        let mut edits = Edits::default();
        let fake = RefCell::new(Fake::new());
        let packets = RefCell::new(Vec::new());
        perform_edit(&mut edits, &original, 0, false, true, &fake, &packets);
        let edit = edits.prepare(owner(), &original, 1, Some(true), None, None).unwrap();
        assert!(execute_edit(&edit, None, false, || false, |_| panic!("Cancelled reservation"),
            |_| panic!("Cancelled send"), &mut |_| panic!("Cancelled ATT")).is_err());
        fake.borrow_mut().clinical[4..8].copy_from_slice(&1f32.to_le_bytes());
        assert!(execute_edit(&edit, None, false, || true, |_| panic!("Changed profile reserved"),
            |_| panic!("Changed profile sent"), &mut |request| fake.borrow_mut().exchange(request)).is_err());
        edits.fail(&edit);
        assert!(!edits.has_sent());
        assert!(edits.prepare(owner(), &original, 2, Some(true), None, None).is_err());
        assert_eq!(packets.borrow().len(), 1);
    }

    #[test]
    fn clinical_only_on_off_cannot_reauthorize_the_old_pme_reception() {
        let original = original();
        let mut edits = Edits::default();
        let fake = RefCell::new(Fake::new());
        let packets = RefCell::new(Vec::new());
        perform_edit(&mut edits, &original, 0, false, true, &fake, &packets);
        let mut view = project(None, 3, Some(original.received()), None);
        edits.project(Some(owner()), None, &mut view);
        assert!(!view.editable && view.requested.is_none());
        let clinical_off = report(original.packet(), 3, 9);
        assert!(edits.prepare(owner(), &clinical_off, 0, Some(true), None, None).is_err());
        let mut view = project(None, 3, Some(clinical_off.received()), Some(&clinical_off));
        edits.project(Some(owner()), Some(&clinical_off), &mut view);
        assert!(!view.editable);
        let fresh = report(original.packet(), 3, 10);
        assert!(edits.prepare(Owner { pme_reception: 2, ..owner() }, &fresh, 0, Some(true), None, None).is_ok());
    }

    fn hex(value: &str) -> Vec<u8> {
        value.as_bytes().chunks_exact(2).map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap()).collect()
    }

    #[test]
    fn legacy_audiogram_fixture_maps_all_slots_preserves_source_and_recognizes_fresh_readback() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!("../tests/fixtures/legacy-audiogram-synthetic-v1.json")).unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let source = hex(case["ahps_hex"].as_str().unwrap());
            let expected = hex(case["pme_curves_hex"].as_str().unwrap());
            assert_eq!(hearing::legacy_audiogram_curves(&source).unwrap().as_slice(), expected);
            let original = original();
            let mut edits = Edits::default();
            let edit = edits.prepare(owner(), &original, 0, None, None, None).unwrap();
            let fake = RefCell::new(Fake::new());
            fake.borrow_mut().clinical = source.clone();
            let ct = fake.borrow().ct.clone();
            let packets = RefCell::new(Vec::new());
            let change = execute_edit(&edit, None, true, || true, |_| Ok(()),
                |packet| { packets.borrow_mut().push(packet.to_vec()); Ok(()) },
                &mut |request| fake.borrow_mut().exchange(request)).unwrap();
            change.verify_sent(|| true, &mut |request| fake.borrow_mut().exchange(request)).unwrap();
            assert_eq!(&packets.borrow()[0][12..], expected);
            assert_eq!(fake.borrow().clinical, source);
            let ct_eq = hex(case["ct_eq_hex"].as_str().unwrap());
            assert_eq!(&fake.borrow().ct[4..36], &ct_eq[..32]);
            assert_eq!(&fake.borrow().ct[52..84], &ct_eq[32..]);
            for index in (0..104).filter(|index| !(4..36).contains(index) && !(52..84).contains(index)) {
                assert_eq!(fake.borrow().ct[index], ct[index], "untouched CT byte {index}");
            }
            assert_eq!(edits.complete(&edit, &change).unwrap(), AccommodationsResult::Sent);
            let mut view = project(None, 3, Some(original.received()), Some(&original));
            edits.project(Some(owner()), Some(&original), &mut view);
            assert!(view.audiogram_available && view.requested.as_ref().unwrap().audiogram);
            assert!(view.requested.as_ref().unwrap().preset.is_none());
            let actual = report(change.plan.expected_report_packet(), 3, 8);
            let mut view = project(Some(view), 3, Some(actual.received()), Some(&actual));
            edits.project(Some(Owner { pme_reception: 2, ..owner() }), Some(&actual), &mut view);
            assert!(view.audiogram && view.audiogram_available && view.requested.is_none());
            edits.clear();
            let actual = report(actual.packet(), 4, 1);
            let next_owner = Owner { connection: 8, pme_reception: 3, ..owner() };
            let mut view = project(None, 4, Some(actual.received()), Some(&actual));
            edits.project(Some(next_owner), Some(&actual), &mut view);
            assert!(!view.audiogram_available && !view.audiogram);
            edits.record_source(next_owner, 4, source).unwrap();
            edits.project(Some(next_owner), Some(&actual), &mut view);
            assert!(view.audiogram_available && view.audiogram);
        }
    }

    #[test]
    fn source_stock_collision_has_one_selection_and_flags_retain_recognized_audiogram() {
        let original = original();
        let fake = RefCell::new(Fake::new());
        fake.borrow_mut().clinical[4..36].copy_from_slice(&original.packet()[12..44]);
        fake.borrow_mut().clinical[52..84].copy_from_slice(&original.packet()[12..44]);
        let mut edits = Edits::default();
        edits.record_source(owner(), 3, fake.borrow().clinical.clone()).unwrap();
        let mut view = project(None, 3, Some(original.received()), Some(&original));
        edits.project(Some(owner()), Some(&original), &mut view);
        assert!(view.audiogram && view.audiogram_available && view.preset.is_none());
        let packets = RefCell::new(Vec::new());
        perform_edit(&mut edits, &original, 0, false, false, &fake, &packets);
        let mut view = project(None, 3, Some(original.received()), Some(&original));
        edits.project(Some(owner()), Some(&original), &mut view);
        assert!(view.requested.as_ref().unwrap().audiogram);
        assert!(view.requested.as_ref().unwrap().preset.is_none());
        perform_edit(&mut edits, &original, 1, true, true, &fake, &packets);
        edits.project(Some(owner()), Some(&original), &mut view);
        assert!(view.requested.as_ref().unwrap().audiogram);
    }

    #[test]
    fn audiogram_rejects_unknown_invalid_and_changed_source_before_any_write() {
        let original = original();
        for mutation in 0..6 {
            let mut edits = Edits::default();
            let edit = edits.prepare(owner(), &original, 0, None, None, None).unwrap();
            let fake = RefCell::new(Fake::new());
            match mutation {
                0 => fake.borrow_mut().clinical.truncate(100),
                1 => fake.borrow_mut().clinical[1] = 0,
                2 => fake.borrow_mut().clinical[4..8].copy_from_slice(&f32::NAN.to_le_bytes()),
                3 => fake.borrow_mut().clinical[4..8].copy_from_slice(&91f32.to_le_bytes()),
                4 => fake.borrow_mut().clinical[36..40].copy_from_slice(&2f32.to_le_bytes()),
                _ => {},
            }
            let reads = Cell::new(0);
            let result = execute_edit(&edit, None, true, || true, |_| panic!("Invalid source reserved"),
                |_| panic!("Invalid source sent"), &mut |request| {
                    let response = fake.borrow_mut().exchange(request);
                    if mutation == 5 && request == [0x0a, 0x39, 0] {
                        reads.set(reads.get() + 1);
                        if reads.get() == 1 { fake.borrow_mut().clinical[4..8].copy_from_slice(&1f32.to_le_bytes()); }
                    }
                    response
                });
            assert!(result.is_err());
            assert_eq!(fake.borrow().writes, 0);
        }
        let mut edits = Edits::default();
        let edit = edits.prepare(owner(), &original, 0, None, None, None).unwrap();
        assert!(execute_edit(&edit, Some(accommodations::Preset::ALL[0]), true, || true,
            |_| panic!("Conflicting selection reserved"), |_| panic!("Conflicting selection sent"),
            &mut |_| panic!("Conflicting selection read")).is_err());
    }

    #[test]
    fn audiogram_keeps_pme_negative_zero_but_ct_clamp_normalizes_it() {
        let original = original();
        let mut edits = Edits::default();
        let edit = edits.prepare(owner(), &original, 0, None, None, None).unwrap();
        let fake = RefCell::new(Fake::new());
        for offset in [4, 52] { fake.borrow_mut().clinical[offset..offset + 4].copy_from_slice(&(-0f32).to_le_bytes()); }
        let change = execute_edit(&edit, None, true, || true, |_| Ok(()), |_| Ok(()),
            &mut |request| fake.borrow_mut().exchange(request)).unwrap();
        assert_eq!(&change.plan.desired_packet()[12..16], &(-0f32).to_le_bytes());
        assert_eq!(&fake.borrow().ct[4..8], &0f32.to_le_bytes());
        assert_eq!(&fake.borrow().ct[52..56], &0f32.to_le_bytes());
    }
}
