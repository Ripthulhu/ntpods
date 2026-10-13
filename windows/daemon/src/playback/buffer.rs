//! Bounded, stream-owned receive-buffer trials.
use super::{render::Render, session::Session};
use crate::{aap, bt, driver::Driver, Ctx};
use std::io;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const SETTLE: Duration = Duration::from_millis(500);
const REPORT_WAIT: Duration = Duration::from_secs(2);
const TRIAL: Duration = Duration::from_secs(30);
const STOP_WAIT: Duration = Duration::from_secs(20);

#[derive(Clone, Copy)]
struct Request {
    target: u16,
    restore: u16,
}

#[derive(Default)]
struct Shared {
    started: Option<Instant>,
    pending: Option<Request>,
    busy: bool,
    cancel: bool,
    closing: bool,
    owner_done: bool,
    restore_failed: bool,
}

pub struct Active {
    address: u64,
    intent: u64,
    shared: Mutex<Shared>,
    changed: Condvar,
}

impl Active {
    fn new(address: u64, intent: u64) -> Self {
        Self {
            address,
            intent,
            shared: Mutex::new(Shared::default()),
            changed: Condvar::new(),
        }
    }
}

#[derive(Clone, Copy)]
struct ReportWait {
    serial: u64,
    deadline: Instant,
}

impl ReportWait {
    fn report(self, serial: u64, delay: Option<u16>, now: Instant) -> io::Result<Option<u16>> {
        if now > self.deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "The AirPods did not send a fresh delay report for the buffer change.",
            ));
        }
        Ok((serial != self.serial).then_some(delay).flatten())
    }
}

#[derive(Clone, Copy)]
enum Phase {
    AwaitStart,
    AwaitReport(ReportWait),
    Adjusting { generation: u32 },
    Holding,
    RestoreSend,
    RestoreQuiet,
    AwaitRestoredStart,
}

struct Trial {
    request: Request,
    phase: Phase,
    restoring: bool,
    // Set before sending: even a failed write can have reached the peer.
    control_changed: bool,
    clock_changed: bool,
    deadline: Instant,
    quiet_until: Instant,
}

fn cleanup_changes(
    trial: &mut Trial,
    cancel_clock: impl FnOnce() -> io::Result<()>,
    restore_control: impl FnOnce(u16) -> io::Result<()>,
) -> io::Result<()> {
    let mut error = None;
    if trial.clock_changed {
        match cancel_clock() {
            Ok(()) => trial.clock_changed = false,
            Err(failed) => error = Some(failed),
        }
    }
    if trial.control_changed {
        match restore_control(trial.request.restore) {
            Ok(()) => trial.control_changed = false,
            Err(failed) => {
                error.get_or_insert(failed);
            }
        }
    }
    error.map_or(Ok(()), Err)
}

pub struct Stream {
    ctx: Ctx,
    active: Arc<Active>,
    driver: Driver,
    trial: Option<Trial>,
    pending_error: Option<String>,
}

impl Stream {
    pub fn new(ctx: &Ctx, address: u64, intent: u64, driver: Driver) -> Self {
        let active = Arc::new(Active::new(address, intent));
        *ctx.playback_buffer.lock().unwrap() = Some(active.clone());
        Self {
            ctx: ctx.clone(),
            active,
            driver,
            trial: None,
            pending_error: None,
        }
    }

    pub fn started(&mut self) {
        let mut shared = self.active.shared.lock().unwrap();
        if shared.closing {
            return;
        }
        shared.started.get_or_insert_with(Instant::now);
        drop(shared);
        if self
            .trial
            .as_ref()
            .is_some_and(|trial| matches!(trial.phase, Phase::AwaitRestoredStart))
        {
            self.trial = None;
            self.finish(None, false, false);
            crate::log("playback buffer: control, host clock and 200 ms media flush restored; acoustic result unconfirmed");
        }
    }

    fn current(&self) -> bool {
        self.ctx.mac() == self.active.address
            && bt::audio_request_is_current(self.active.intent)
            && !self.ctx.shutting_down.load(Ordering::Acquire)
            && !self.ctx.user_disconnected.load(Ordering::Acquire)
            && !self.ctx.wants_reconnect.load(Ordering::Acquire)
            && !self.ctx.capture_active.load(Ordering::Acquire)
    }

    fn send(&self, value: u16) -> io::Result<()> {
        self.driver
            .send_playback_control(&aap::device_control_command(0x0B, value.into()))
    }

    // Keep the request gate locked through publication so an old completion
    // cannot clear a newly queued trial's target.
    fn finish(&self, error: Option<String>, failed: bool, clear_started: bool) {
        let mut shared = self.active.shared.lock().unwrap();
        shared.pending = None;
        shared.busy = false;
        shared.cancel = false;
        shared.restore_failed |= failed;
        if clear_started {
            shared.started = None;
        }
        let current = self
            .ctx
            .playback_buffer
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, &self.active));
        if current && self.ctx.mac() == self.active.address {
            let mut state = self.ctx.state.lock().unwrap();
            if !shared.restore_failed {
                state.playback_buffer_target_ms = None;
            }
            state.playback_buffer_error = error.or_else(|| {
                shared.restore_failed.then(|| {
                    "The buffer trial did not finish restoring. Reconnect the AirPods.".into()
                })
            });
        }
        self.active.changed.notify_all();
        drop(shared);
        self.ctx.push_state();
    }

    /// Teardown does not wait for a report or reopen a transport being closed.
    /// Call before suspend/reset and on every exit while Render is still alive.
    pub fn cleanup(&mut self, render: &Render) -> io::Result<()> {
        self.cleanup_before(render, || Ok(()))
    }

    /// Keep the trial guard until a continuing transport is safely paused or
    /// re-primed; a failed transport restore must keep microphone startup blocked.
    pub fn cleanup_before(
        &mut self,
        render: &Render,
        finish_transport: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        let had_work = {
            let mut shared = self.active.shared.lock().unwrap();
            shared.started = None;
            shared.pending = None;
            shared.busy
        };
        let mut first_error = None;
        if let Some(mut trial) = self.trial.take() {
            let quiet_until = trial.quiet_until;
            let mut last_send = None;
            first_error = cleanup_changes(
                &mut trial,
                || render.cancel_clock(),
                |restore| {
                    settle_until(quiet_until);
                    let result = self.send(restore);
                    last_send = Some(Instant::now());
                    result
                },
            )
            .err();
            if let Some(sent) = last_send {
                trial.quiet_until = sent + SETTLE;
            }
            // A failed send may still reach the peer. Suspension must preserve
            // the quiet interval after every control attempt, including errors.
            settle_until(trial.quiet_until);
            // Preserve failed work for Drop's control-only fallback.
            if trial.control_changed || trial.clock_changed {
                self.trial = Some(trial);
            }
        }
        if first_error.is_none() {
            first_error = finish_transport().err();
        }
        let pending_error = self.pending_error.take();
        if had_work || first_error.is_some() || pending_error.is_some() {
            let message = match (pending_error, first_error.as_ref()) {
                (Some(error), Some(cleanup)) => Some(format!(
                    "{error}; cleanup failed: {cleanup}. Reconnect the AirPods."
                )),
                (Some(error), None) => Some(error),
                (None, Some(cleanup)) => Some(format!(
                    "Buffer cleanup failed: {cleanup}. Reconnect the AirPods."
                )),
                (None, None) => None,
            };
            self.finish(message, first_error.is_some(), true);
        } else {
            self.active.changed.notify_all();
        }
        if let Some(error) = first_error {
            Err(error)
        } else if self.active.shared.lock().unwrap().restore_failed {
            Err(io::Error::other(
                "The buffer trial did not finish restoring. Reconnect the AirPods.",
            ))
        } else {
            Ok(())
        }
    }

    /// Returns true when media was reopened and the caller must re-prime/start.
    pub fn poll(&mut self, session: &mut Session, render: &Render) -> io::Result<bool> {
        let result = self.poll_inner(session, render);
        if let Err(error) = &result {
            // The owner publishes stopping before cleanup. Completing here
            // would unblock microphone startup while altered media is open.
            self.pending_error = Some(error.to_string());
        }
        result
    }

    fn poll_inner(&mut self, session: &mut Session, render: &Render) -> io::Result<bool> {
        let (closing, cancel, pending) = {
            let mut shared = self.active.shared.lock().unwrap();
            (shared.closing, shared.cancel, shared.pending.take())
        };
        if closing {
            return Err(closing_error());
        }
        if let Some(request) = pending {
            if cancel || !self.current() {
                self.finish(None, false, false);
                return Ok(false);
            }
            // Both checks are read-only. Legacy drivers refuse the trial without
            // disrupting otherwise healthy playback.
            let preflight = session
                .check_flush_support()
                .and_then(|_| render.clock().map(|_| ()));
            if let Err(error) = preflight {
                self.finish(Some(error.to_string()), false, false);
                return Ok(false);
            }
            // External teardown may have arrived while preflight was in flight.
            {
                let shared = self.active.shared.lock().unwrap();
                if shared.closing {
                    return Err(closing_error());
                }
                if shared.cancel {
                    drop(shared);
                    self.finish(None, false, false);
                    return Ok(false);
                }
            }
            let now = Instant::now();
            self.trial = Some(Trial {
                request,
                phase: Phase::AwaitStart,
                restoring: false,
                control_changed: false,
                clock_changed: false,
                deadline: now + TRIAL,
                quiet_until: now,
            });
            self.active.shared.lock().unwrap().started = None;
            session.reopen_media(request.target)?;
            return Ok(true);
        }
        let Some(trial) = self.trial.as_ref() else {
            return Ok(false);
        };
        let now = Instant::now();
        if !trial.restoring && (cancel || !self.current() || now >= trial.deadline) {
            self.begin_restore(render)?;
        }
        let trial = self.trial.as_ref().unwrap();
        let restoring = trial.restoring;
        let request = trial.request;
        let quiet_until = trial.quiet_until;
        let phase = trial.phase;
        match phase {
            Phase::AwaitStart => {
                let started = self.active.shared.lock().unwrap().started;
                if started.is_some_and(|started| started.elapsed() >= SETTLE) {
                    // Never issue a late target after an external close/cancel.
                    let shared = self.active.shared.lock().unwrap();
                    if shared.closing {
                        return Err(closing_error());
                    }
                    if shared.cancel || !self.current() {
                        drop(shared);
                        self.begin_restore(render)?;
                        return Ok(false);
                    }
                    let target = request.target;
                    self.trial.as_mut().unwrap().control_changed = true;
                    let serial = session.delay_serial;
                    let applied = self.send(target);
                    let sent = Instant::now();
                    self.trial.as_mut().unwrap().quiet_until = sent + SETTLE;
                    drop(shared);
                    applied?;
                    let trial = self.trial.as_mut().unwrap();
                    trial.deadline = sent + TRIAL;
                    trial.phase = Phase::AwaitReport(ReportWait {
                        serial,
                        deadline: sent + REPORT_WAIT,
                    });
                    trial.quiet_until = sent + SETTLE;
                    crate::log(&format!(
                        "playback buffer: target {target} ms sent; waiting for fresh delay report"
                    ));
                }
            }
            Phase::AwaitReport(wait) => {
                if let Some(report) =
                    wait.report(session.delay_serial, session.delay_report, now)?
                {
                    let target = if restoring {
                        request.restore
                    } else {
                        request.target
                    };
                    let (interval, delta) = adjustment(report, target);
                    if delta == 0 {
                        self.trial.as_mut().unwrap().phase = if restoring {
                            Phase::RestoreQuiet
                        } else {
                            Phase::Holding
                        };
                    } else {
                        let shared = self.active.shared.lock().unwrap();
                        if shared.closing {
                            return Err(closing_error());
                        }
                        if shared.cancel && !restoring {
                            drop(shared);
                            self.begin_restore(render)?;
                            return Ok(false);
                        }
                        self.trial.as_mut().unwrap().clock_changed = true;
                        let programmed = render
                            .set_clock(interval, delta)
                            .and_then(|_| render.clock());
                        drop(shared);
                        let status = programmed?;
                        self.trial.as_mut().unwrap().phase = Phase::Adjusting {
                            generation: status.generation,
                        };
                        crate::log(&format!(
                            "playback buffer: report {}.{} ms, target {target} ms, interval {interval}/10, drift {delta} ms",
                            report / 10, report % 10
                        ));
                    }
                }
            }
            Phase::Adjusting { generation } => {
                let status = render.clock()?;
                if status.generation != generation {
                    return Err(io::Error::other(
                        "The Windows playback clock changed during the buffer trial.",
                    ));
                }
                match status.phase {
                    1 | 2 => {}
                    3 => {
                        render.cancel_clock()?;
                        let trial = self.trial.as_mut().unwrap();
                        trial.clock_changed = false;
                        trial.phase = if trial.restoring { Phase::RestoreQuiet } else { Phase::Holding };
                    }
                    4 => return Err(io::Error::new(io::ErrorKind::TimedOut,
                        "The Windows playback clock did not reach the requested drift within 10 seconds.")),
                    _ => return Err(io::Error::other("The Windows playback clock adjustment was interrupted.")),
                }
            }
            Phase::Holding => {}
            Phase::RestoreSend => {
                if now >= quiet_until {
                    let serial = session.delay_serial;
                    let restored = self.send(request.restore);
                    let sent = Instant::now();
                    let trial = self.trial.as_mut().unwrap();
                    trial.quiet_until = sent + SETTLE;
                    restored?;
                    trial.phase = Phase::AwaitReport(ReportWait {
                        serial,
                        deadline: sent + REPORT_WAIT,
                    });
                    crate::log(&format!(
                        "playback buffer: restore {} ms sent; waiting for fresh delay report",
                        request.restore
                    ));
                }
            }
            Phase::RestoreQuiet => {
                if now >= quiet_until {
                    session.reopen_media(200)?;
                    self.active.shared.lock().unwrap().started = None;
                    self.trial.as_mut().unwrap().phase = Phase::AwaitRestoredStart;
                    return Ok(true);
                }
            }
            Phase::AwaitRestoredStart => {}
        }
        Ok(false)
    }

    fn begin_restore(&mut self, render: &Render) -> io::Result<()> {
        let trial = self.trial.as_mut().unwrap();
        if trial.restoring {
            return Ok(());
        }
        if trial.clock_changed {
            render.cancel_clock()?;
            trial.clock_changed = false;
        }
        trial.restoring = true;
        if !trial.control_changed {
            trial.phase = Phase::RestoreQuiet;
            trial.quiet_until = Instant::now();
            return Ok(());
        }
        // Retain control_changed until the entire restoration finishes. A later
        // failure still gets a best-effort restore before the AAP channel closes.
        trial.phase = Phase::RestoreSend;
        Ok(())
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let pending = self.active.shared.lock().unwrap().busy;
        if let Some(trial) = self.trial.take() {
            let restored = if trial.control_changed {
                settle_until(trial.quiet_until);
                let result = self.send(trial.request.restore);
                settle_until(Instant::now() + SETTLE);
                result.is_ok()
            } else {
                settle_until(trial.quiet_until);
                true
            };
            // Only explicit cleanup or a restored start can certify the media
            // state. This control-only fallback must retain the trial guard.
            self.finish(
                Some(if !restored || trial.clock_changed {
                    "Buffer cleanup was incomplete. Reconnect the AirPods.".into()
                } else {
                    "Playback restoration did not finish. Reconnect the AirPods.".into()
                }),
                true,
                true,
            );
        } else if pending {
            self.finish(None, false, true);
        }
        {
            let mut shared = self.active.shared.lock().unwrap();
            shared.pending = None;
            shared.busy = false;
            shared.started = None;
            shared.owner_done = true;
            self.active.changed.notify_all();
        }
        let mut slot = self.ctx.playback_buffer.lock().unwrap();
        if slot
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, &self.active))
        {
            *slot = None;
        }
    }
}

fn settle_until(deadline: Instant) {
    std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
}

fn closing_error() -> io::Error {
    io::Error::new(
        io::ErrorKind::ConnectionReset,
        "The AirPods control session is closing.",
    )
}

fn stop_active(active: &Active, timeout: Duration) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    let mut shared = active.shared.lock().unwrap();
    shared.started = None;
    shared.closing = true;
    shared.cancel = true;
    while shared.busy && !shared.owner_done {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            shared.restore_failed = true;
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Playback did not acknowledge buffer cleanup before control-channel teardown.",
            ));
        }
        shared = active.changed.wait_timeout(shared, remaining).unwrap().0;
    }
    if shared.restore_failed {
        Err(io::Error::other(
            "The buffer trial did not finish restoring. Reconnect the AirPods.",
        ))
    } else {
        Ok(())
    }
}

// The receiver must restore before closing AAP. It never holds playback/state
// locks while waiting, and the stream owner never waits for this receiver.
pub fn stop(ctx: &Ctx) {
    let active = ctx.playback_buffer.lock().unwrap().clone();
    if let Some(active) = active {
        if let Err(error) = stop_active(&active, STOP_WAIT) {
            crate::log(&format!("playback buffer: {error}"));
            ctx.shutdown_cleanup_failed.store(true, Ordering::Release);
        }
    }
}

fn validate(target: u16, restore: Option<u16>) -> io::Result<u16> {
    let Some(restore) = restore else {
        return Err(io::Error::other(
            "Supply an explicit buffer restore value for this trial.",
        ));
    };
    if ![60, 80, 100, 120, 140].contains(&target)
        || ![80, 100, 120, 140, 150, 300, 500].contains(&restore)
        || target >= restore
    {
        return Err(io::Error::other(
            "Use a supported buffer target below the restore value.",
        ));
    }
    Ok(restore)
}

// DelayReport is in tenths of a millisecond; the Apple comparison truncates it.
fn adjustment(report: u16, target: u16) -> (u32, u32) {
    let report = report / 10;
    let mut delta = u32::from(report.abs_diff(target));
    if delta > 10 && target != 150 {
        delta -= 10;
    }
    (
        if report > target {
            11
        } else if report < target {
            9
        } else {
            10
        },
        delta,
    )
}

pub fn set(ctx: &Ctx, target: Option<u16>, restore: Option<u16>) {
    if let Err(error) = request(ctx, target, restore) {
        ctx.state.lock().unwrap().playback_buffer_error = Some(error.to_string());
    }
    ctx.push_state();
}

fn request(ctx: &Ctx, target: Option<u16>, restore: Option<u16>) -> io::Result<()> {
    let _preference = ctx.mic_preference.lock().unwrap();
    let active =
        ctx.playback_buffer.lock().unwrap().clone().ok_or_else(|| {
            io::Error::other("Start native AirPods playback before a buffer trial.")
        })?;
    let mut shared = active.shared.lock().unwrap();
    let Some(target) = target else {
        shared.cancel = true;
        return Ok(());
    };
    let restore = validate(target, restore)?;
    if shared.closing || shared.owner_done || shared.restore_failed {
        return Err(io::Error::other(
            "Reconnect the AirPods after playback cleanup.",
        ));
    }
    if shared.busy {
        return Err(io::Error::other(
            "Wait for the current buffer trial to restore.",
        ));
    }
    let mut state = ctx.state.lock().unwrap();
    if !state.connected
        || !state.playback_native_driver
        || state.playback_state != "ready"
        || ctx.mac() != active.address
        || !bt::audio_request_is_current(active.intent)
        || ctx.shutting_down.load(Ordering::Acquire)
        || ctx.user_disconnected.load(Ordering::Acquire)
        || ctx.wants_reconnect.load(Ordering::Acquire)
        || !state.playback_delay_reporting
        || state.playback_delay_tenths_ms.is_none()
        || !shared.started.is_some_and(|time| time.elapsed() >= SETTLE)
    {
        return Err(io::Error::other(
            "Keep audio playing until the AirPods report their delay.",
        ));
    }
    if super::selection::microphone_busy(
        &state,
        ctx.capture_active.load(Ordering::Acquire),
        ctx.mic_on.load(Ordering::Acquire),
    ) {
        return Err(io::Error::other(
            "Stop the microphone before a buffer trial.",
        ));
    }
    shared.pending = Some(Request { target, restore });
    shared.busy = true;
    shared.cancel = false;
    state.playback_buffer_target_ms = Some(target);
    state.playback_buffer_error = None;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changed_trial() -> Trial {
        Trial {
            request: Request {
                target: 60,
                restore: 150,
            },
            phase: Phase::Holding,
            restoring: false,
            control_changed: true,
            clock_changed: true,
            deadline: Instant::now() + TRIAL,
            quiet_until: Instant::now(),
        }
    }

    #[test]
    fn cleanup_restores_control_even_when_clock_cancel_fails() {
        let mut trial = changed_trial();
        let events = std::cell::RefCell::new(Vec::new());
        assert!(cleanup_changes(
            &mut trial,
            || {
                events.borrow_mut().push(10);
                Err(io::Error::other("clock failed"))
            },
            |restore| {
                events.borrow_mut().push(restore);
                Ok(())
            },
        )
        .is_err());
        assert_eq!(*events.borrow(), [10, 150]);
        assert!(trial.clock_changed);
        assert!(!trial.control_changed);
        assert!(cleanup_changes(&mut trial, || Ok(()), |_| panic!("already restored")).is_ok());
        assert!(!trial.clock_changed);
    }

    #[test]
    fn cleanup_retains_failed_control_for_fallback_and_skips_unapplied_changes() {
        let mut trial = changed_trial();
        assert!(cleanup_changes(
            &mut trial,
            || Ok(()),
            |_| Err(io::Error::other("send failed"))
        )
        .is_err());
        assert!(!trial.clock_changed);
        assert!(trial.control_changed);
        assert!(cleanup_changes(
            &mut trial,
            || panic!("clock already nominal"),
            |value| {
                assert_eq!(value, 150);
                Ok(())
            }
        )
        .is_ok());
        assert!(cleanup_changes(
            &mut trial,
            || panic!("no clock change"),
            |_| panic!("no control change")
        )
        .is_ok());
    }

    #[test]
    fn trials_require_an_explicit_higher_restore_value() {
        assert_eq!(validate(60, Some(500)).unwrap(), 500);
        assert_eq!(validate(140, Some(150)).unwrap(), 150);
        for (target, restore) in [
            (60, None),
            (30, Some(150)),
            (61, Some(150)),
            (60, Some(60)),
            (140, Some(100)),
            (60, Some(65535)),
        ] {
            assert!(validate(target, restore).is_err());
        }
        assert_eq!(
            aap::device_control_command(0x0B, 500),
            [4, 0, 4, 0, 9, 0, 11, 0xF4, 1, 0, 0]
        );
    }

    #[test]
    fn apple_adjustment_preserves_direction_rounding_and_restore_exception() {
        assert_eq!(adjustment(1500, 60), (11, 80));
        assert_eq!(adjustment(1499, 100), (11, 39));
        assert_eq!(adjustment(600, 150), (9, 90));
        assert_eq!(adjustment(1500, 150), (10, 0));
        assert_eq!(adjustment(1100, 100), (11, 10));
        assert_eq!(adjustment(1110, 100), (11, 1));
        assert_eq!(adjustment(1500, 300), (9, 140));
    }

    #[test]
    fn identical_numeric_report_is_fresh_only_with_a_new_serial() {
        let now = Instant::now();
        let wait = ReportWait {
            serial: 7,
            deadline: now + REPORT_WAIT,
        };
        assert_eq!(wait.report(7, Some(1500), now).unwrap(), None);
        assert_eq!(wait.report(8, Some(1500), now).unwrap(), Some(1500));
        assert_eq!(wait.report(8, None, now).unwrap(), None);
        assert!(wait
            .report(8, Some(1500), now + REPORT_WAIT + Duration::from_millis(1))
            .is_err());
        let wrapped = ReportWait {
            serial: u64::MAX,
            deadline: now + REPORT_WAIT,
        };
        assert_eq!(wrapped.report(0, Some(1500), now).unwrap(), Some(1500));
    }

    #[test]
    fn external_stop_releases_gate_while_owner_acknowledges() {
        let active = Arc::new(Active::new(1, 2));
        active.shared.lock().unwrap().busy = true;
        let owner = active.clone();
        let worker = std::thread::spawn(move || loop {
            let mut shared = owner.shared.lock().unwrap();
            if shared.closing {
                assert!(shared.cancel);
                shared.busy = false;
                owner.changed.notify_all();
                break;
            }
            drop(shared);
            std::thread::yield_now();
        });
        assert!(stop_active(&active, Duration::from_secs(2)).is_ok());
        worker.join().unwrap();
        assert!(active.shared.lock().unwrap().closing);
    }

    #[test]
    fn failed_or_unacknowledged_cleanup_cannot_report_success() {
        let active = Active::new(1, 2);
        active.shared.lock().unwrap().busy = true;
        assert!(stop_active(&active, Duration::ZERO).is_err());
        let mut shared = active.shared.lock().unwrap();
        assert!(shared.closing && shared.cancel && shared.restore_failed);
        shared.busy = false;
        drop(shared);
        assert!(stop_active(&active, Duration::ZERO).is_err());
    }
}
