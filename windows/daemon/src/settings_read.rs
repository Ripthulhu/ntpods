//! Bounded, read-only initialization on the existing settings channel.

use crate::{
    device_models, diagnostic_controls, hardware_diagnostics, hearing, hearing_metadata,
    transparency, Ctx,
};
use ntpods_ipc::{HardwareDiagnostics, HearingMetadata, Snapshot, TransparencySettings};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Hearing,
    Transparency,
    Protection,
    Diagnostics,
    Metadata,
}

impl Kind {
    const ALL: [Self; 5] = [
        Self::Hearing,
        Self::Transparency,
        Self::Protection,
        Self::Diagnostics,
        Self::Metadata,
    ];
    fn bit(self) -> u8 {
        1 << self as u8
    }
    fn supported(self, state: &Snapshot) -> bool {
        match self {
            Self::Hearing | Self::Diagnostics | Self::Metadata => device_models::hearing_aid(state),
            Self::Transparency => {
                device_models::find(&state.model).is_some_and(|model| model.custom_transparency)
            }
            Self::Protection => device_models::loud_sound_reduction(state),
        }
    }
    fn pending(self, state: &mut Snapshot, on: bool) {
        match self {
            Self::Hearing => state.hearing_aid_pending = on,
            Self::Transparency => state.transparency_pending = on,
            Self::Protection => state.loud_sound_reduction_pending = on,
            Self::Diagnostics => state.hardware_diagnostics_pending = on,
            Self::Metadata => state.hearing_metadata_pending = on,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Owner {
    address: u64,
    connection: u64,
    session: u64,
}

impl Owner {
    fn of(ctx: &Ctx) -> Self {
        Self {
            address: ctx.mac(),
            connection: ctx.connect_requested.snapshot(),
            session: hearing::session(),
        }
    }
    fn current(self, ctx: &Ctx) -> bool {
        self == Self::of(ctx)
            && !ctx.shutting_down.load(Ordering::Acquire)
            && !ctx.user_disconnected.load(Ordering::Acquire)
    }
}

#[derive(Default)]
pub(super) struct Initialization {
    owner: Owner,
    attempted: u8,
    running: Option<Kind>,
}

impl Initialization {
    fn claim(&mut self, owner: Owner, state: &Snapshot, eligible: bool) -> Option<Kind> {
        if self.owner != owner {
            *self = Self {
                owner,
                ..Default::default()
            };
        }
        if !eligible || self.running.is_some() {
            return None;
        }
        let kind = Kind::ALL
            .into_iter()
            .find(|kind| kind.supported(state) && self.attempted & kind.bit() == 0)?;
        self.running = Some(kind);
        Some(kind)
    }
    fn finish(&mut self, owner: Owner, kind: Kind, io_started: bool) -> bool {
        if self.owner != owner || self.running != Some(kind) {
            return false;
        }
        self.running = None;
        if io_started {
            self.attempted |= kind.bit();
        }
        true
    }
}

fn idle(
    state: &Snapshot,
    capturing: bool,
    mic_on: bool,
    streams: Option<u32>,
    running: Option<Kind>,
) -> bool {
    state.connected
        && !state.link_stale
        && device_models::find(&state.model).is_some()
        && !capturing
        && !mic_on
        && !state.mic_capture_active
        && !state.mic_uplink_active
        && !state.mic_recording
        && matches!(state.mic_state.as_str(), "idle" | "unavailable")
        && (streams == Some(0) || streams.is_none() && !state.mic_available)
        && !diagnostic_controls::active(state)
        && !(state.hearing_aid_pending && running != Some(Kind::Hearing))
        && !(state.transparency_pending && running != Some(Kind::Transparency))
        && !(state.loud_sound_reduction_pending && running != Some(Kind::Protection))
        && !(state.hardware_diagnostics_pending && running != Some(Kind::Diagnostics))
        && !(state.hearing_metadata_pending && running != Some(Kind::Metadata))
        && !state
            .headphone_accommodations
            .as_ref()
            .is_some_and(|value| value.pending)
}

fn available(ctx: &Ctx, state: &Snapshot, running: Option<Kind>) -> bool {
    idle(
        state,
        ctx.capture_active.load(Ordering::Acquire),
        ctx.mic_on.load(Ordering::Acquire),
        ctx.pipe.status(),
        running,
    )
}

enum Value {
    Hearing(hearing::ProfileRead),
    Transparency(TransparencySettings),
    Protection(bool),
    Diagnostics(Option<HardwareDiagnostics>),
    Metadata(HearingMetadata),
}

fn publish(ctx: &Ctx, owner: Owner, kind: Kind, result: Result<Option<Value>, String>) {
    let mut state = ctx.state.lock().unwrap();
    // Cancellation can happen while this worker is waiting for the snapshot.
    if !owner.current(ctx) {
        return;
    }
    kind.pending(&mut state, false);
    if !state.connected || state.link_stale {
        return;
    }
    match result {
        Ok(Some(Value::Hearing(profile))) => {
            state.hearing_aid_settings = Some(profile.settings);
            state.hearing_aid_error = None;
            // The same complete read supplies both the editor baseline and Audiogram.
            let _ = ctx.accommodations_edits.lock().unwrap().record_source(
                crate::accommodations_owner(ctx),
                owner.session,
                profile.value,
            );
        }
        Ok(Some(Value::Transparency(value))) => {
            state.transparency = Some(value);
            state.transparency_error = None;
        }
        Ok(Some(Value::Protection(value))) => {
            state.loud_sound_reduction = Some(value);
            state.loud_sound_reduction_error = None;
        }
        Ok(Some(Value::Diagnostics(value))) => {
            state.hardware_diagnostics = value;
            state.hardware_diagnostics_error = None;
        }
        Ok(Some(Value::Metadata(value))) => {
            state.hearing_metadata = Some(value);
            state.hearing_metadata_error = None;
        }
        Err(error) => match kind {
            Kind::Hearing => state.hearing_aid_error = Some(error),
            Kind::Transparency => state.transparency_error = Some(error),
            Kind::Protection => state.loud_sound_reduction_error = Some(error),
            Kind::Diagnostics => state.hardware_diagnostics_error = Some(error),
            Kind::Metadata => state.hearing_metadata_error = Some(error),
        },
        Ok(None) => (),
    }
}

pub(super) fn poll(ctx: &Ctx) {
    let owner = Owner::of(ctx);
    if !owner.current(ctx) || !hearing::att_available() || ctx.driver_cell.lock().unwrap().is_none() {
        return;
    }
    let kind = {
        let state = ctx.state.lock().unwrap();
        ctx.automatic_settings
            .lock()
            .unwrap()
            .claim(owner, &state, available(ctx, &state, None))
    };
    let Some(kind) = kind else {
        return;
    };
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let mut io_started = false;
        let mut run = || {
            // User edits and microphone startup win; the receiver retries this
            // unstarted read on its normal status tick, without a queued worker.
            let _write = ctx.settings_write.try_lock().ok()?;
            let _preference = ctx.mic_preference.try_lock().ok()?;
            let mut state = ctx.state.lock().unwrap();
            if !owner.current(&ctx) || !available(&ctx, &state, None) {
                return None;
            }
            drop(state);
            // HFP calls can use a different microphone path from NTPods' HQ mic.
            let endpoints = crate::hfp_endpoints::Endpoints::new().ok()?;
            if endpoints.busy(owner.address).unwrap_or(true) {
                return None;
            }
            state = ctx.state.lock().unwrap();
            if !owner.current(&ctx) || !available(&ctx, &state, None) {
                return None;
            }
            kind.pending(&mut state, true);
            let model = state.model.clone();
            drop(state);
            let Some(driver) = ctx.driver_cell.lock().unwrap().clone() else {
                publish(&ctx, owner, kind, Ok(None));
                return None;
            };
            let deadline = Instant::now() + Duration::from_secs(5);
            let current = || {
                owner.current(&ctx)
                    && Instant::now() < deadline
                    && available(&ctx, &ctx.state.lock().unwrap(), Some(kind))
            };
            let result = hearing::try_att_transaction(&driver, &current, |exchange| {
                let mut exchange = |request: &[u8]| {
                    io_started = true;
                    exchange(request)
                };
                match kind {
                    Kind::Hearing => {
                        hearing::read_profile_value(&current, &mut exchange).map(Value::Hearing)
                    }
                    Kind::Transparency => transparency::read_record(&mut exchange)
                        .map(|(_, _, value)| Value::Transparency(value)),
                    Kind::Protection => {
                        hearing::read_loud_sound(&mut exchange).map(Value::Protection)
                    }
                    Kind::Diagnostics => hardware_diagnostics::transaction(&model, &mut exchange)
                        .map(Value::Diagnostics),
                    Kind::Metadata => {
                        hearing_metadata::transaction(&current, &mut exchange).map(Value::Metadata)
                    }
                }
            });
            publish(&ctx, owner, kind, result);
            Some(())
        };
        let _ = run();
        ctx.automatic_settings
            .lock()
            .unwrap()
            .finish(owner, kind, io_started);
        ctx.push_state();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pro() -> Snapshot {
        Snapshot {
            model: "A3063".into(),
            ..Default::default()
        }
    }

    #[test]
    fn initialization_serializes_all_supported_reads_once_without_retrying_failures() {
        let owner = Owner {
            address: 1,
            connection: 2,
            session: 3,
        };
        let mut work = Initialization::default();
        for kind in Kind::ALL {
            assert_eq!(work.claim(owner, &pro(), true), Some(kind));
            assert_eq!(work.claim(owner, &pro(), true), None);
            assert!(work.finish(owner, kind, true));
        }
        assert_eq!(work.claim(owner, &pro(), true), None);
    }

    #[test]
    fn busy_or_unstarted_reads_wait_and_a_new_owner_cannot_be_finished_by_old_work() {
        let owner = Owner {
            address: 1,
            connection: 2,
            session: 3,
        };
        let mut work = Initialization::default();
        for _ in 0..100 {
            assert_eq!(work.claim(owner, &pro(), false), None);
        }
        assert_eq!(work.claim(owner, &pro(), true), Some(Kind::Hearing));
        assert!(work.finish(owner, Kind::Hearing, false));
        assert_eq!(work.claim(owner, &pro(), true), Some(Kind::Hearing));
        let newer = Owner {
            session: 4,
            ..owner
        };
        assert_eq!(work.claim(newer, &pro(), true), Some(Kind::Hearing));
        assert!(!work.finish(owner, Kind::Hearing, true));
        assert_eq!(work.running, Some(Kind::Hearing));
        assert!(work.finish(newer, Kind::Hearing, false));
        assert_eq!(work.claim(newer, &pro(), true), Some(Kind::Hearing));
        assert!(work.finish(newer, Kind::Hearing, true));
        assert_eq!(work.claim(newer, &pro(), true), Some(Kind::Transparency));
    }

    #[test]
    fn unsupported_and_unknown_models_never_get_hearing_or_diagnostic_reads() {
        let owner = Owner {
            address: 1,
            connection: 2,
            session: 3,
        };
        let mut work = Initialization::default();
        assert_eq!(work.claim(owner, &Snapshot::default(), true), None);
        let old = Snapshot {
            model: "A2084".into(),
            ..Default::default()
        };
        assert_eq!(work.claim(owner, &old, true), Some(Kind::Transparency));
        work.finish(owner, Kind::Transparency, true);
        assert_eq!(work.claim(owner, &old, true), None);
    }

    #[test]
    fn microphone_and_other_writes_defer_reads_but_missing_optional_mic_does_not() {
        let mut state = pro();
        state.connected = true;
        state.mic_state = "idle".into();
        assert!(idle(&state, false, false, None, None));
        for streams in [Some(1), Some(2)] {
            assert!(!idle(&state, false, false, streams, None));
        }
        assert!(!idle(&state, true, false, Some(0), None));
        assert!(!idle(&state, false, true, Some(0), None));
        state.mic_available = true;
        assert!(!idle(&state, false, false, None, None));
        state.hearing_aid_pending = true;
        assert!(!idle(&state, false, false, Some(0), None));
        assert!(idle(&state, false, false, Some(0), Some(Kind::Hearing)));
        state.transparency_pending = true;
        assert!(!idle(&state, false, false, Some(0), Some(Kind::Hearing)));
    }

    #[test]
    fn first_complete_profile_populates_the_editor_without_another_read_and_stale_owner_cannot_publish(
    ) {
        let _lock = hearing::HEARING_STATE.lock().unwrap();
        let ctx = Ctx::for_test();
        ctx.mac.store(1, Ordering::Release);
        ctx.connect_requested.store(true, Ordering::Release);
        let owner = Owner::of(&ctx);
        {
            let mut state = ctx.state.lock().unwrap();
            state.connected = true;
            state.hearing_aid_pending = true;
            state.playback_state = "streaming".into();
            state.audio_output_ready = true;
        }
        let mut value = vec![0; 104];
        value[..4].copy_from_slice(&[2, 2, 0x60, 0]);
        let settings = ntpods_ipc::HearingAidSettings {
            left_eq: [5.; 8],
            right_eq: [8.; 8],
            left_amplification: 0.,
            right_amplification: 0.,
            left_tone: 0.,
            right_tone: 0.,
            left_conversation_boost: false,
            right_conversation_boost: false,
            left_ambient_noise_reduction: 0.,
            right_ambient_noise_reduction: 0.,
            own_voice: 0.,
        };
        publish(
            &ctx,
            owner,
            Kind::Hearing,
            Ok(Some(Value::Hearing(hearing::ProfileRead {
                value,
                settings,
            }))),
        );
        let state = ctx.state.lock().unwrap().clone();
        assert_eq!(state.hearing_aid_settings, Some(settings));
        assert!(!state.hearing_aid_pending);
        assert!(state.connected && state.audio_output_ready);
        assert_eq!(state.playback_state, "streaming");
        assert!(ctx.wants_settings_refresh.lock().unwrap().is_none());
        ctx.connect_requested.store(true, Ordering::Release);
        publish(&ctx, owner, Kind::Hearing, Err("old connection".into()));
        assert_eq!(
            ctx.state.lock().unwrap().hearing_aid_settings,
            Some(settings)
        );
        assert_eq!(ctx.state.lock().unwrap().hearing_aid_error, None);
    }

    #[test]
    fn cancellation_while_publication_waits_cannot_clear_pending_or_publish_old_values() {
        let _lock = hearing::HEARING_STATE.lock().unwrap();
        let ctx = Ctx::for_test();
        ctx.mac.store(1, Ordering::Release);
        ctx.connect_requested.store(true, Ordering::Release);
        let owner = Owner::of(&ctx);
        let mut state = ctx.state.lock().unwrap();
        state.connected = true;
        state.transparency_pending = true;
        state.transparency_error = Some("current request".into());
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        let worker_ctx = ctx.clone();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            publish(&worker_ctx, owner, Kind::Transparency, Err("old request".into()));
            finished_tx.send(()).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(finished_rx.recv_timeout(Duration::from_millis(25)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout));
        // Session teardown does not wait for a settings or snapshot lock.
        hearing::invalidate_session();
        drop(state);
        finished_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
        let state = ctx.state.lock().unwrap();
        assert!(state.transparency_pending);
        assert_eq!(state.transparency_error.as_deref(), Some("current request"));
    }
}
