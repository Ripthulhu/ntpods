use crate::{aap, hardware_check, hardware_diagnostics, hearing, hearing_metadata, volume, Ctx};
use ntpods_ipc::{HardwareDiagnosticEar, HardwareDiagnostics, Snapshot};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

pub(super) struct Job {
    id: u64,
    target: String,
    owner: Arc<AtomicBool>,
    coordinator: hardware_check::Coordinator,
}

fn owns(job: &Job, id: u64, target: &str, owner: &Arc<AtomicBool>) -> bool {
    job.id == id && job.target == target && Arc::ptr_eq(&job.owner, owner)
}

pub fn select_pair(jobs: &mut Option<Job>, target: &str) {
    if let Some(job) = jobs.as_mut().filter(|job| job.target != target) {
        job.owner.store(false, Ordering::Release);
        job.coordinator.cancel(job.id, &job.target);
    }
}

fn receipt(ctx: &Ctx, command: &str, id: u64, error: Option<String>) {
    ctx.send_event(&ntpods_ipc::Event::CommandResult {
        command: command.into(),
        request_id: id,
        ok: error.is_none(),
        message: error.unwrap_or_default(),
    });
}

fn now(ctx: &Ctx) -> u64 {
    ctx.mic_clock.elapsed().as_millis() as u64
}

pub fn case_report_fresh(tick: u64, now_ms: u64) -> bool {
    tick != 0 && now_ms.saturating_sub(tick - 1) <= 30_000
}

pub(super) fn in_case(ears: [aap::EarStatus; 2]) -> Option<bool> {
    use aap::EarStatus::*;
    if ears == [InCase; 2] {
        Some(true)
    } else if ears.iter().any(|ear| matches!(ear, InEar | OutOfEar)) {
        Some(false)
    } else {
        None
    }
}

pub fn observe_case(ctx: &Ctx, ears: [aap::EarStatus; 2]) {
    ctx.case_observed_tick
        .store(now(ctx).saturating_add(1), Ordering::Release);
    let value = in_case(ears);
    let changed = {
        let mut state = ctx.state.lock().unwrap();
        let changed = state.earbuds_in_case != value;
        state.earbuds_in_case = value;
        changed
    };
    if changed {
        ctx.push_state();
    }
}

fn selected(ctx: &Ctx, address: u64, owner: &AtomicBool) -> bool {
    owner.load(Ordering::Acquire)
        && ctx.mac() == address
        && !ctx.shutting_down.load(Ordering::Acquire)
        && !ctx.user_disconnected.load(Ordering::Acquire)
}

fn idle_error(ctx: &Ctx, state: &Snapshot) -> Option<&'static str> {
    if !state.connected
        || state.link_stale
        || ctx.user_disconnected.load(Ordering::Acquire)
        || ctx.shutting_down.load(Ordering::Acquire)
    {
        Some("Connect the AirPods before reading device data.")
    } else {
        crate::hardware_diagnostics_read_error(
            state,
            ctx.capture_active.load(Ordering::Acquire) || ctx.mic_on.load(Ordering::Acquire),
            ctx.pipe.status(),
        )
    }
}

pub fn read_metadata(ctx: &Ctx, target: &str) {
    let work = {
        let _write = ctx.settings_write.lock().unwrap();
        let _mic = ctx.mic_preference.lock().unwrap();
        let driver = ctx.driver_cell.lock().unwrap().clone();
        let mut state = ctx.state.lock().unwrap();
        if state.device_id != target || state.hearing_metadata_pending {
            return;
        }
        let error = idle_error(ctx, &state)
            .or_else(|| {
                (state.hardware_diagnostics_pending || active(&state))
                    .then_some("Wait for the hardware check to finish.")
            })
            .or_else(|| {
                driver
                    .is_none()
                    .then_some("Connect the AirPods before reading device data.")
            });
        state.hearing_metadata_error = error.map(str::to_owned);
        error.is_none().then(|| {
            state.hearing_metadata_pending = true;
            (
                driver.unwrap(),
                hearing::session(),
                ctx.mac(),
                state.device_id.clone(),
            )
        })
    };
    ctx.push_state();
    if let Some((driver, session, address, target)) = work {
        let ctx = ctx.clone();
        thread::spawn(move || {
            let current = || {
                hearing::session() == session
                    && ctx.mac() == address
                    && !ctx.shutting_down.load(Ordering::Acquire)
                    && !ctx.user_disconnected.load(Ordering::Acquire)
            };
            let result = hearing_metadata::read(&driver, current);
            let _write = ctx.settings_write.lock().unwrap();
            let mut state = ctx.state.lock().unwrap();
            if !current() || !state.connected || state.device_id != target {
                return;
            }
            state.hearing_metadata_pending = false;
            match result {
                Ok(Some(value)) => state.hearing_metadata = Some(value),
                Err(error) => state.hearing_metadata_error = Some(error),
                Ok(None) => {
                    state.hearing_metadata_error =
                        Some("Device data could not be read. Try again after reconnecting.".into())
                }
            }
            drop(state);
            drop(_write);
            ctx.push_state();
        });
    }
}

pub fn active(state: &Snapshot) -> bool {
    state
        .hardware_check
        .as_ref()
        .is_some_and(|job| matches!(job.phase.as_str(), "waiting_case_close" | "waiting_result"))
}

fn supported(state: &Snapshot) -> bool {
    matches!(state.model.as_str(), "A3063" | "A3064" | "A3065") && state.firmware == "9A348"
}

fn start_error(
    state: &Snapshot,
    confirmed: bool,
    fresh: bool,
    music: bool,
) -> Option<&'static str> {
    if !supported(state) {
        Some("In-case checks have not been verified on this model and firmware.")
    } else if !confirmed || state.earbuds_in_case != Some(true) || !fresh {
        Some("Keep both AirPods inside the open case and out of your ears before starting.")
    } else if music {
        Some("Pause playback before starting an in-case check.")
    } else if state.hardware_diagnostics_pending || state.hearing_metadata_pending || active(state)
    {
        Some("Wait for the current device-data request to finish.")
    } else {
        None
    }
}

fn empty_record() -> HardwareDiagnostics {
    let ear = HardwareDiagnosticEar {
        measured_at_unix_seconds: 0,
        first_use_unix_seconds: None,
        general: 0,
        measurement: 7,
        component_failures: None,
        unknown_bits: 0,
    };
    HardwareDiagnostics {
        read_at_unix_seconds: 0,
        left: ear,
        right: ear,
    }
}

pub fn start_check(ctx: &Ctx, id: u64, target: &str, confirmed: bool) {
    let work = {
        let _write = ctx.settings_write.lock().unwrap();
        let _mic = ctx.mic_preference.lock().unwrap();
        let driver = ctx.driver_cell.lock().unwrap().clone();
        let mut job = ctx.hardware_check.lock().unwrap();
        let mut state = ctx.state.lock().unwrap();
        if state.device_id != target
            || id == 0
            || job
                .as_ref()
                .is_some_and(|job| job.id == id && job.target == target)
        {
            receipt(
                ctx,
                "start_hardware_check",
                id,
                Some("The hardware-check request is no longer current.".into()),
            );
            return;
        }
        let error = idle_error(ctx, &state)
            .or_else(|| {
                start_error(
                    &state,
                    confirmed,
                    case_report_fresh(ctx.case_observed_tick.load(Ordering::Acquire), now(ctx)),
                    false,
                )
            })
            .or_else(|| {
                driver
                    .is_none()
                    .then_some("Connect the AirPods before starting a check.")
            });
        if let Some(error) = error {
            state.hardware_diagnostics_error = Some(error.into());
            None
        } else {
            if let Some(old) = job.as_ref() {
                old.owner.store(false, Ordering::Release);
            }
            let owner = Arc::new(AtomicBool::new(true));
            let coordinator =
                hardware_check::Coordinator::start(id, target.into(), empty_record(), now(ctx))
                    .unwrap();
            state.hardware_check = Some(coordinator.status());
            state.hardware_diagnostics_pending = true;
            state.hardware_diagnostics_error = None;
            *job = Some(Job {
                id,
                target: target.into(),
                owner: owner.clone(),
                coordinator,
            });
            Some((driver.unwrap(), ctx.mac(), target.to_owned(), owner))
        }
    };
    ctx.push_state();
    receipt(
        ctx,
        "start_hardware_check",
        id,
        work.is_none().then(|| {
            ctx.state
                .lock()
                .unwrap()
                .hardware_diagnostics_error
                .clone()
                .unwrap_or_else(|| "The hardware check could not start.".into())
        }),
    );
    if let Some((driver, address, target, owner)) = work {
        let ctx = ctx.clone();
        thread::spawn(move || {
            volume::init();
            let result = (|| {
                let _write = ctx.settings_write.lock().unwrap();
                let _mic = ctx.mic_preference.lock().unwrap();
                let session = hearing::session();
                let current = || selected(&ctx, address, &owner) && hearing::session() == session;
                let music = volume::render_activity(ctx.volume_target())
                    .map_err(|_| "Windows playback activity could not be checked. Try again after reconnecting.".to_owned())?;
                {
                    let state = ctx.state.lock().unwrap();
                    // This request owns the pending flag while preparing its fresh baseline.
                    let mut check = state.clone();
                    check.hardware_diagnostics_pending = false;
                    check.hardware_check = None;
                    if let Some(error) = idle_error(&ctx, &check).or_else(|| {
                        start_error(
                            &check,
                            confirmed,
                            case_report_fresh(
                                ctx.case_observed_tick.load(Ordering::Acquire),
                                now(&ctx),
                            ),
                            music,
                        )
                    }) {
                        return Err(error.to_owned());
                    }
                }
                if !current() {
                    return Err("The AirPods connection changed before the check.".into());
                }
                let model = ctx.state.lock().unwrap().model.clone();
                let baseline =
                    hardware_diagnostics::read(&driver, hardware_diagnostics::token(), &model)?
                        .ok_or("The baseline hardware results could not be read.")?;
                {
                    let mut jobs = ctx.hardware_check.lock().unwrap();
                    let job = jobs
                        .as_mut()
                        .filter(|job| owns(job, id, &target, &owner))
                        .ok_or("The hardware-check request is no longer current.")?;
                    if !current() || !job.coordinator.is_active() {
                        return Err("The hardware-check request is no longer current.".into());
                    }
                    job.coordinator = hardware_check::Coordinator::start(
                        id,
                        target.clone(),
                        baseline,
                        now(&ctx),
                    )?;
                    if !job.coordinator.claim_trigger(id, &target, now(&ctx)) {
                        return Err("The hardware-check request was already sent.".into());
                    }
                }
                // ATT acknowledgement confirms receipt, not completion or an audible sweep.
                let safe = || {
                    current()
                        && {
                            let state = ctx.state.lock().unwrap();
                            supported(&state)
                                && state.connected
                                && !state.link_stale
                                && case_safe(
                                    &state,
                                    ctx.case_observed_tick.load(Ordering::Acquire),
                                    now(&ctx),
                                )
                                && crate::hardware_diagnostics_read_error(
                                    &state,
                                    ctx.capture_active.load(Ordering::Acquire)
                                        || ctx.mic_on.load(Ordering::Acquire),
                                    ctx.pipe.status(),
                                )
                                .is_none()
                        }
                        && volume::render_activity(ctx.volume_target()) == Ok(false)
                };
                hardware_check::trigger(&driver, safe)?
                    .ok_or("The check reply was interrupted; execution is uncertain.")?;
                Ok(baseline)
            })();
            {
                let mut jobs = ctx.hardware_check.lock().unwrap();
                if let Some(job) = jobs.as_mut().filter(|job| owns(job, id, &target, &owner)) {
                    let mut state = ctx.state.lock().unwrap();
                    if state.device_id == target {
                        state.hardware_diagnostics_pending = false;
                        match result {
                            Ok(record) => state.hardware_diagnostics = Some(record),
                            Err(error) => job.coordinator.fail(id, &target, error, now(&ctx)),
                        }
                    }
                }
            }
            publish_job(&ctx, id, &target, Some(&owner));
            poll_check(&ctx, id, address, &target, &owner);
        });
    }
}

fn update_job(
    ctx: &Ctx,
    id: u64,
    target: &str,
    owner: &Arc<AtomicBool>,
    update: impl FnOnce(&mut hardware_check::Coordinator),
) {
    let mut jobs = ctx.hardware_check.lock().unwrap();
    if let Some(job) = jobs.as_mut().filter(|job| owns(job, id, target, owner)) {
        update(&mut job.coordinator);
    }
}

fn publish_job(ctx: &Ctx, id: u64, target: &str, owner: Option<&Arc<AtomicBool>>) -> bool {
    let active = {
        let jobs = ctx.hardware_check.lock().unwrap();
        let Some(job) = jobs.as_ref().filter(|job| {
            job.id == id
                && job.target == target
                && owner.is_none_or(|owner| Arc::ptr_eq(owner, &job.owner))
        }) else {
            return false;
        };
        let mut state = ctx.state.lock().unwrap();
        if state.device_id != target {
            return false;
        }
        state.hardware_check = Some(job.coordinator.status());
        job.coordinator.is_active()
    };
    ctx.push_state();
    active
}

pub fn cancel_check(ctx: &Ctx, id: u64, target: &str) {
    let found = {
        let mut jobs = ctx.hardware_check.lock().unwrap();
        if let Some(job) = jobs
            .as_mut()
            .filter(|job| job.id == id && job.target == target)
        {
            job.owner.store(false, Ordering::Release);
            job.coordinator.cancel(id, target);
            true
        } else {
            false
        }
    };
    publish_job(ctx, id, target, None);
    receipt(
        ctx,
        "cancel_hardware_check",
        id,
        (!found).then(|| "The hardware-check request is no longer current.".into()),
    );
}

fn poll_check(ctx: &Ctx, id: u64, address: u64, target: &str, owner: &Arc<AtomicBool>) {
    let started = now(ctx);
    let mut next_read = started + 30_000;
    loop {
        if !selected(ctx, address, owner) {
            update_job(ctx, id, target, owner, |job| job.cancel(id, target));
            publish_job(ctx, id, target, Some(owner));
            return;
        }
        update_job(ctx, id, target, owner, |job| job.tick(now(ctx)));
        if !publish_job(ctx, id, target, Some(owner)) {
            return;
        }
        if !ctx.state.lock().unwrap().connected {
            update_job(ctx, id, target, owner, |job| {
                job.waiting_for_result(id, target, now(ctx))
            });
        }
        if now(ctx) >= next_read {
            let work = {
                let _write = ctx.settings_write.lock().unwrap();
                let _mic = ctx.mic_preference.lock().unwrap();
                let driver = ctx.driver_cell.lock().unwrap().clone();
                let state = ctx.state.lock().unwrap();
                if idle_error(ctx, &state).is_none() && !state.hardware_diagnostics_pending {
                    driver
                        .map(|driver| (driver, hardware_diagnostics::token(), state.model.clone()))
                } else {
                    None
                }
            };
            if let Some((driver, token, model)) = work {
                if let Ok(Some(record)) = hardware_diagnostics::read(&driver, token, &model) {
                    if token.is_current() && selected(ctx, address, owner) {
                        let mut jobs = ctx.hardware_check.lock().unwrap();
                        if let Some(job) = jobs.as_mut().filter(|job| owns(job, id, target, owner))
                        {
                            job.coordinator.observe(id, target, &record, now(ctx));
                            let mut state = ctx.state.lock().unwrap();
                            if state.device_id == target {
                                state.hardware_diagnostics = Some(record);
                            }
                        }
                    }
                }
            }
            next_read = now(ctx) + 10_000;
        }
        thread::sleep(Duration::from_millis(500));
    }
}

fn case_safe(state: &Snapshot, tick: u64, now_ms: u64) -> bool {
    state.earbuds_in_case == Some(true) && case_report_fresh(tick, now_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn case_safety_requires_both_exact_statuses_and_recent_observation() {
        use aap::EarStatus::*;
        assert_eq!(in_case([InCase, InCase]), Some(true));
        assert_eq!(in_case([InCase, InEar]), Some(false));
        assert_eq!(in_case([InCase, OutOfEar]), Some(false));
        assert_eq!(in_case([InCase, Disconnected]), None);
        assert_eq!(in_case([InCase, Transitional]), None);
        assert!(!case_report_fresh(0, 1));
        assert!(case_report_fresh(1, 30_000));
        assert!(!case_report_fresh(1, 30_001));
    }
    #[test]
    fn sound_request_requires_verified_firmware_confirmation_and_paused_audio() {
        let mut state = Snapshot {
            model: "A3063".into(),
            firmware: "9A348".into(),
            earbuds_in_case: Some(true),
            ..Snapshot::default()
        };
        assert_eq!(start_error(&state, true, true, false), None);
        assert!(start_error(&state, false, true, false).is_some());
        assert!(start_error(&state, true, false, false).is_some());
        assert!(start_error(&state, true, true, true).is_some());
        state.earbuds_in_case = None;
        assert!(start_error(&state, true, true, false).is_some());
        state.earbuds_in_case = Some(true);
        state.firmware = "unknown".into();
        assert!(start_error(&state, true, true, false).is_some());
    }

    #[test]
    fn leaving_the_case_during_discovery_revokes_the_write_guard() {
        let mut state = Snapshot {
            earbuds_in_case: Some(true),
            ..Snapshot::default()
        };
        assert!(case_safe(&state, 1, 1_000));
        state.earbuds_in_case = in_case([aap::EarStatus::InEar; 2]);
        assert!(!case_safe(&state, 1, 2_000));
        state.earbuds_in_case = Some(true);
        assert!(!case_safe(&state, 1, 30_001));
    }

    #[test]
    fn reused_public_ids_cannot_give_an_old_worker_ownership() {
        let old = Arc::new(AtomicBool::new(false));
        let current = Arc::new(AtomicBool::new(true));
        let job = Job {
            id: 1,
            target: "pair".into(),
            owner: current.clone(),
            coordinator: hardware_check::Coordinator::start(1, "pair".into(), empty_record(), 0)
                .unwrap(),
        };
        assert!(!owns(&job, 1, "pair", &old));
        assert!(owns(&job, 1, "pair", &current));
        assert!(!owns(&job, 1, "other", &current));
        assert!(!owns(&job, 2, "pair", &current));
        let mut jobs = Some(job);
        select_pair(&mut jobs, "other");
        select_pair(&mut jobs, "pair");
        assert!(!current.load(Ordering::Acquire));
        assert!(!jobs.unwrap().coordinator.is_active());
    }
}
