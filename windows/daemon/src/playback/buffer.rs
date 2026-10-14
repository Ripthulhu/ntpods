//! Stream-owned receive-buffer policy and bounded diagnostic trials.
use super::{render::Render, session::Session};
use crate::{aap, bt, driver::Driver, Ctx};
use std::io;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

const SETTLE: Duration = Duration::from_millis(500);
const REPORT_WAIT: Duration = Duration::from_secs(2);
const TRIAL: Duration = Duration::from_secs(30);
const TRANSITION: Duration = Duration::from_secs(15);
const STOP_WAIT: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lifetime {
    Trial,
    Playback,
}

#[derive(Clone, Copy)]
struct Request {
    target: u16,
    restore: u16,
    lifetime: Lifetime,
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
    automatic: bool,
    automatic_disabled: bool,
    automatic_holds: usize,
    cancel_revision: u64,
}

impl Shared {
    fn can_automate(&self, now: Instant) -> bool {
        !self.busy
            && !self.closing
            && !self.owner_done
            && !self.restore_failed
            && !self.automatic_disabled
            && self.automatic_holds == 0
            && self
                .started
                .is_some_and(|started| now.duration_since(started) >= SETTLE)
    }

    fn queue(&mut self, request: Request) {
        self.pending = Some(request);
        self.busy = true;
        self.cancel = false;
        self.automatic = request.lifetime == Lifetime::Playback;
    }
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
    fn report(
        self,
        serial: u64,
        delay: Option<u16>,
        lifetime: Lifetime,
        now: Instant,
    ) -> io::Result<Option<u16>> {
        if now > self.deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "The AirPods did not send a fresh delay report for the buffer change.",
            ));
        }
        let delay = delay.filter(|delay| lifetime == Lifetime::Trial || *delay != 0);
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
    media_uncertain: bool,
    deadline: Instant,
    hold_until: Option<Instant>,
    quiet_until: Instant,
}

impl Trial {
    fn new(request: Request, now: Instant) -> Self {
        Self {
            request,
            phase: Phase::AwaitStart,
            restoring: false,
            control_changed: false,
            clock_changed: false,
            media_uncertain: false,
            deadline: now + TRANSITION,
            hold_until: (request.lifetime == Lifetime::Trial).then_some(now + TRIAL),
            quiet_until: now,
        }
    }

    fn check_deadline(&self, now: Instant) -> io::Result<()> {
        if !matches!(self.phase, Phase::Holding) && now >= self.deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "The playback buffer transition did not finish within 15 seconds.",
            ));
        }
        Ok(())
    }

    fn expired(&self, now: Instant) -> bool {
        !self.restoring && self.hold_until.is_some_and(|deadline| now >= deadline)
    }
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
    next_automatic: Instant,
    fallback_reprime: bool,
    automatic_held: bool,
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
            next_automatic: Instant::now(),
            fallback_reprime: false,
            automatic_held: false,
        }
    }

    pub fn started(&mut self) {
        let mut shared = self.active.shared.lock().unwrap();
        if shared.closing {
            return;
        }
        shared.started.get_or_insert_with(Instant::now);
        drop(shared);
        if self.fallback_reprime {
            self.fallback_reprime = false;
            let error = self.pending_error.take();
            self.finish(error, false, false);
            return;
        }
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
        shared.automatic = false;
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
        self.cleanup_transport(render, finish_transport, false)
    }

    fn cleanup_transport(
        &mut self,
        render: &Render,
        finish_transport: impl FnOnce() -> io::Result<()>,
        defer_finish: bool,
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
        if defer_finish && first_error.is_none() {
            // Keep microphone startup blocked until the caller re-primes media.
            self.fallback_reprime = true;
            return Ok(());
        }
        self.fallback_reprime = false;
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
        self.queue_automatic(session);
        let result = self.poll_inner(session, render);
        if let Err(error) = &result {
            // The owner publishes stopping before cleanup. Completing here
            // would unblock microphone startup while altered media is open.
            self.pending_error = Some(error.to_string());
            let automatic = {
                let mut shared = self.active.shared.lock().unwrap();
                let automatic = shared.automatic && !shared.closing;
                if automatic {
                    shared.automatic_disabled = true;
                }
                automatic
            };
            if automatic {
                crate::log(&format!(
                    "playback buffer: automatic request failed: {error}"
                ));
                // The owner must tear down an ended or cancelled session, not restart it.
                if matches!(
                    error.kind(),
                    io::ErrorKind::Interrupted
                        | io::ErrorKind::ConnectionAborted
                        | io::ErrorKind::ConnectionReset
                        | io::ErrorKind::NotConnected
                        | io::ErrorKind::BrokenPipe
                ) {
                    return result;
                }
                if self
                    .trial
                    .as_ref()
                    .is_some_and(|trial| trial.media_uncertain)
                {
                    return Err(io::Error::other(format!(
                        "The automatic buffer change interrupted the media channel. Reconnect the AirPods: {error}"
                    )));
                }
                self.cleanup_transport(render, || session.restore_media_timeout(), true)
                    .map_err(|error| {
                        io::Error::other(format!(
                            "Automatic buffer restoration failed. Reconnect the AirPods: {error}"
                        ))
                    })?;
                return Ok(true);
            }
        }
        result
    }

    fn queue_automatic(&mut self, session: &Session) {
        let now = Instant::now();
        if now < self.next_automatic {
            return;
        }
        self.next_automatic = now + SETTLE;
        let _preference = self.ctx.mic_preference.lock().unwrap();
        let mut shared = self.active.shared.lock().unwrap();
        if !shared.can_automate(now) || !self.current() {
            return;
        }
        let mut state = self.ctx.state.lock().unwrap();
        let mic_requested = {
            let demand = self.ctx.mic_demand.lock().unwrap();
            self.ctx.auto_mode.load(Ordering::Acquire)
                && match demand.mode {
                    ntpods_ipc::MicMode::KeepActive => true,
                    ntpods_ipc::MicMode::Manual => demand.manual_active,
                    ntpods_ipc::MicMode::Automatic => false,
                }
        };
        if mic_requested
            || super::selection::microphone_busy(
                &state,
                self.ctx.capture_active.load(Ordering::Acquire),
                self.ctx.mic_on.load(Ordering::Acquire),
            )
        {
            return;
        }
        if session
            .delay_report
            .is_some_and(|delay| delay != 0 && delay <= 1200)
        {
            self.automatic_held = false;
        }
        let Some(request) = automatic_request(
            &state,
            session.delay_reporting,
            session.delay_report,
            self.automatic_held,
        ) else {
            return;
        };
        self.automatic_held = false;
        shared.queue(request);
        state.playback_buffer_target_ms = Some(request.target);
        state.playback_buffer_error = None;
        drop(state);
        drop(shared);
        self.ctx.push_state();
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
                if request.lifetime == Lifetime::Playback {
                    self.active.shared.lock().unwrap().automatic_disabled = true;
                }
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
            self.trial = Some(Trial::new(request, now));
            self.active.shared.lock().unwrap().started = None;
            self.reopen(session, request.target)?;
            return Ok(true);
        }
        let Some(trial) = self.trial.as_ref() else {
            return Ok(false);
        };
        let now = Instant::now();
        trial.check_deadline(now)?;
        if !trial.restoring && (cancel || !self.current() || trial.expired(now)) {
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
                    // Queued reports must precede the serial saved for this write.
                    if !session.poll()? {
                        return Ok(false);
                    }
                    self.trial
                        .as_ref()
                        .unwrap()
                        .check_deadline(Instant::now())?;
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
                    trial.hold_until =
                        (request.lifetime == Lifetime::Trial).then_some(sent + TRIAL);
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
                if let Some(report) = wait.report(
                    session.delay_serial,
                    session.delay_report,
                    request.lifetime,
                    now,
                )? {
                    let target = if restoring {
                        request.restore
                    } else {
                        request.target
                    };
                    let (interval, delta) = adjustment(report, target);
                    if delta == 0 {
                        if !restoring && request.lifetime == Lifetime::Playback {
                            self.automatic_held = true;
                        }
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
                        if !restoring && request.lifetime == Lifetime::Playback {
                            self.automatic_held = true;
                        }
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
                    if !session.poll()? {
                        return Ok(false);
                    }
                    self.trial
                        .as_ref()
                        .unwrap()
                        .check_deadline(Instant::now())?;
                    let shared = self.active.shared.lock().unwrap();
                    if shared.closing {
                        return Err(closing_error());
                    }
                    let serial = session.delay_serial;
                    let restored = self.send(request.restore);
                    let sent = Instant::now();
                    drop(shared);
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
                    self.reopen(session, 200)?;
                    self.active.shared.lock().unwrap().started = None;
                    self.trial.as_mut().unwrap().phase = Phase::AwaitRestoredStart;
                    return Ok(true);
                }
            }
            Phase::AwaitRestoredStart => {}
        }
        Ok(false)
    }

    fn reopen(&mut self, session: &mut Session, flush_ms: u16) -> io::Result<()> {
        self.trial.as_mut().unwrap().media_uncertain = true;
        session.reopen_media(flush_ms)?;
        self.trial.as_mut().unwrap().media_uncertain = false;
        Ok(())
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
        trial.deadline = Instant::now() + TRANSITION;
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

// This policy has coordinated clock/flush listening coverage only on Pro 3.
fn automatic_request(
    state: &ntpods_ipc::Snapshot,
    reporting: bool,
    delay: Option<u16>,
    previously_held: bool,
) -> Option<Request> {
    let supported = ["A3063", "A3064", "A3065"]
        .iter()
        .any(|known| state.model.trim().eq_ignore_ascii_case(known));
    (supported
        && state.connected
        && state.playback_native_driver
        && state.playback_state == "ready"
        && reporting
        // A resumed configuration can report zero after a successful hold/restore.
        && delay.is_some_and(|delay| delay > 1200 || (delay == 0 && previously_held)))
    .then_some(Request {
        target: 120,
        // Explicit policy restore value; DelayReport is not the prior setting.
        restore: 150,
        lifetime: Lifetime::Playback,
    })
}

pub struct AutomaticPause {
    active: Arc<Active>,
    cancel_revision: u64,
}

impl AutomaticPause {
    pub fn is_current(&self) -> bool {
        let shared = self.active.shared.lock().unwrap();
        !shared.closing && !shared.owner_done && shared.cancel_revision == self.cancel_revision
    }

    pub fn is_pending(&self) -> bool {
        let shared = self.active.shared.lock().unwrap();
        shared.busy && shared.automatic && !shared.owner_done
    }

    pub fn wait(&self) -> io::Result<()> {
        self.wait_for(STOP_WAIT)
    }

    fn wait_for(&self, timeout: Duration) -> io::Result<()> {
        let deadline = Instant::now() + timeout;
        let mut shared = self.active.shared.lock().unwrap();
        while shared.busy && shared.automatic && !shared.owner_done {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                shared.restore_failed = true;
                shared.automatic_disabled = true;
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Playback did not finish restoring its buffer. Reconnect the AirPods.",
                ));
            }
            shared = self
                .active
                .changed
                .wait_timeout(shared, remaining)
                .unwrap()
                .0;
        }
        if shared.restore_failed {
            Err(io::Error::other(
                "Playback buffer restoration failed. Reconnect the AirPods.",
            ))
        } else {
            Ok(())
        }
    }
}

impl Drop for AutomaticPause {
    fn drop(&mut self) {
        let mut shared = self.active.shared.lock().unwrap();
        shared.automatic_holds -= 1;
        self.active.changed.notify_all();
    }
}

fn begin_pause(active: Arc<Active>) -> io::Result<AutomaticPause> {
    let mut shared = active.shared.lock().unwrap();
    if shared.restore_failed {
        return Err(io::Error::other(
            "Playback buffer restoration failed. Reconnect the AirPods.",
        ));
    }
    shared.automatic_holds += 1;
    if shared.automatic {
        shared.cancel = true;
    }
    let cancel_revision = shared.cancel_revision;
    drop(shared);
    Ok(AutomaticPause {
        active,
        cancel_revision,
    })
}

/// Reserve before deferring a command so automatic work cannot slip past it.
pub fn begin_automatic_pause(ctx: &Ctx) -> io::Result<Option<AutomaticPause>> {
    let active = ctx.playback_buffer.lock().unwrap().clone();
    active.map(begin_pause).transpose()
}

/// Wait outside preference/state locks; keep the guard through the new command.
pub fn pause_automatic(ctx: &Ctx) -> io::Result<Option<AutomaticPause>> {
    let pause = begin_automatic_pause(ctx)?;
    if let Some(pause) = &pause {
        pause.wait()?;
    }
    Ok(pause)
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

pub fn set(ctx: &Ctx, target: Option<u16>, restore: Option<u16>, current: impl FnOnce() -> bool) {
    let result = (|| {
        if target.is_none() {
            return request(ctx, target, restore, current);
        }
        if let Some(target) = target {
            validate(target, restore)?;
        }
        let pause = pause_automatic(ctx)?;
        if let Some(pause) = &pause {
            let started = pause.active.shared.lock().unwrap().started;
            if target.is_some() {
                if let Some(started) = started {
                    settle_until(started + SETTLE);
                }
            }
        }
        request(ctx, target, restore, current)
    })();
    if let Err(error) = result {
        ctx.state.lock().unwrap().playback_buffer_error = Some(error.to_string());
    }
    ctx.push_state();
}

fn request(
    ctx: &Ctx,
    target: Option<u16>,
    restore: Option<u16>,
    current: impl FnOnce() -> bool,
) -> io::Result<()> {
    let _preference = ctx.mic_preference.lock().unwrap();
    if !current() {
        return Ok(());
    }
    let active =
        ctx.playback_buffer.lock().unwrap().clone().ok_or_else(|| {
            io::Error::other("Start native AirPods playback before a buffer trial.")
        })?;
    let mut shared = active.shared.lock().unwrap();
    let Some(target) = target else {
        shared.cancel_revision = shared.cancel_revision.wrapping_add(1);
        shared.automatic_disabled = true;
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
    shared.automatic_disabled = true;
    shared.queue(Request {
        target,
        restore,
        lifetime: Lifetime::Trial,
    });
    state.playback_buffer_target_ms = Some(target);
    state.playback_buffer_error = None;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn automatic() -> Request {
        Request {
            target: 120,
            restore: 150,
            lifetime: Lifetime::Playback,
        }
    }

    fn changed_trial() -> Trial {
        Trial {
            request: Request {
                target: 60,
                restore: 150,
                lifetime: Lifetime::Trial,
            },
            phase: Phase::Holding,
            restoring: false,
            control_changed: true,
            clock_changed: true,
            media_uncertain: false,
            deadline: Instant::now() + TRANSITION,
            hold_until: Some(Instant::now() + TRIAL),
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
        assert_eq!(
            wait.report(7, Some(1500), Lifetime::Trial, now).unwrap(),
            None
        );
        assert_eq!(
            wait.report(8, Some(1500), Lifetime::Trial, now).unwrap(),
            Some(1500)
        );
        assert_eq!(wait.report(8, None, Lifetime::Trial, now).unwrap(), None);
        assert!(wait
            .report(
                8,
                Some(1500),
                Lifetime::Trial,
                now + REPORT_WAIT + Duration::from_millis(1)
            )
            .is_err());
        let wrapped = ReportWait {
            serial: u64::MAX,
            deadline: now + REPORT_WAIT,
        };
        assert_eq!(
            wrapped.report(0, Some(1500), Lifetime::Trial, now).unwrap(),
            Some(1500)
        );
    }

    #[test]
    fn automatic_zero_reports_wait_for_a_nonzero_report_without_extending_the_deadline() {
        let now = Instant::now();
        let wait = ReportWait {
            serial: 7,
            deadline: now + REPORT_WAIT,
        };
        assert_eq!(
            wait.report(8, Some(0), Lifetime::Playback, now).unwrap(),
            None
        );
        assert_eq!(
            wait.report(9, Some(0), Lifetime::Playback, now + REPORT_WAIT)
                .unwrap(),
            None
        );
        assert!(wait
            .report(
                10,
                Some(0),
                Lifetime::Playback,
                now + REPORT_WAIT + Duration::from_millis(1)
            )
            .is_err());
        assert_eq!(
            wait.report(9, Some(1490), Lifetime::Playback, now).unwrap(),
            Some(1490)
        );
        assert_eq!(
            wait.report(8, Some(0), Lifetime::Trial, now).unwrap(),
            Some(0)
        );
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

    #[test]
    fn automatic_policy_requires_a_known_model_and_streaming_delay() {
        let mut state = ntpods_ipc::Snapshot {
            model: "A3064".into(),
            connected: true,
            playback_native_driver: true,
            playback_state: "ready".into(),
            ..Default::default()
        };
        let request = automatic_request(&state, true, Some(1490), false).unwrap();
        assert_eq!((request.target, request.restore), (120, 150));
        assert!(request.lifetime == Lifetime::Playback);
        for report in [None, Some(0), Some(600), Some(1000), Some(1200)] {
            assert!(automatic_request(&state, true, report, false).is_none());
        }
        assert!(automatic_request(&state, true, Some(0), true).is_some());
        for report in [None, Some(600), Some(1000), Some(1200)] {
            assert!(automatic_request(&state, true, report, true).is_none());
        }
        assert!(automatic_request(&state, false, Some(1490), true).is_none());
        for model in ["", "unknown", "A2084", "A3048", "A3055", "A2096"] {
            state.model = model.into();
            assert!(automatic_request(&state, true, Some(1490), true).is_none());
        }
        for model in ["A3063", "a3064", " A3065 "] {
            state.model = model.into();
            assert!(automatic_request(&state, true, Some(1490), false).is_some());
        }
        state.connected = false;
        assert!(automatic_request(&state, true, Some(1490), true).is_none());
        state.connected = true;
        state.playback_native_driver = false;
        assert!(automatic_request(&state, true, Some(1490), true).is_none());
        state.playback_native_driver = true;
        state.playback_state = "starting".into();
        assert!(automatic_request(&state, true, Some(1490), true).is_none());
    }

    #[test]
    fn sustained_hold_has_no_timer_but_every_transition_is_bounded() {
        let now = Instant::now();
        let mut sustained = Trial::new(automatic(), now);
        assert!(sustained.check_deadline(now + TRANSITION).is_err());
        sustained.phase = Phase::Holding;
        let later = now + Duration::from_secs(3600);
        assert!(sustained.check_deadline(later).is_ok());
        assert!(!sustained.expired(later));
        sustained.restoring = true;
        sustained.phase = Phase::RestoreSend;
        sustained.deadline = later + TRANSITION;
        assert!(sustained.check_deadline(later).is_ok());
        assert!(sustained.check_deadline(later + TRANSITION).is_err());
        let mut manual = Trial::new(
            Request {
                lifetime: Lifetime::Trial,
                ..automatic()
            },
            now,
        );
        manual.phase = Phase::Holding;
        assert!(!manual.expired(now + TRIAL - Duration::from_millis(1)));
        assert!(manual.expired(now + TRIAL));
    }

    #[test]
    fn automatic_work_reapplies_after_stop_but_not_after_failure() {
        let now = Instant::now();
        let mut shared = Shared::default();
        assert!(!shared.can_automate(now));
        shared.started = Some(now);
        assert!(!shared.can_automate(now + SETTLE - Duration::from_millis(1)));
        assert!(shared.can_automate(now + SETTLE));
        shared.queue(automatic());
        assert!(!shared.can_automate(now + SETTLE));
        shared.busy = false;
        shared.started = None;
        assert!(!shared.can_automate(now + SETTLE));
        shared.started = Some(now + SETTLE);
        assert!(shared.can_automate(now + SETTLE * 2));
        shared.automatic_disabled = true;
        assert!(!shared.can_automate(now + Duration::from_secs(3600)));
        let fresh = Shared {
            started: Some(now),
            ..Default::default()
        };
        assert!(fresh.can_automate(now + SETTLE));
    }

    #[test]
    fn automatic_pause_waits_for_restore_and_holds_off_reapply() {
        let active = Arc::new(Active::new(1, 2));
        {
            let mut shared = active.shared.lock().unwrap();
            shared.started = Some(Instant::now() - SETTLE);
            shared.queue(automatic());
        }
        let pause = begin_pause(active.clone()).unwrap();
        assert!(pause.is_pending());
        assert!(active.shared.lock().unwrap().cancel);
        let owner = active.clone();
        let worker = std::thread::spawn(move || loop {
            let mut shared = owner.shared.lock().unwrap();
            if shared.cancel {
                assert!(!shared.closing);
                assert_eq!(shared.automatic_holds, 1);
                shared.busy = false;
                shared.automatic = false;
                owner.changed.notify_all();
                break;
            }
            drop(shared);
            std::thread::yield_now();
        });
        pause.wait_for(Duration::from_secs(2)).unwrap();
        worker.join().unwrap();
        assert!(!pause.is_pending());
        assert!(!active.shared.lock().unwrap().can_automate(Instant::now()));
        let nested = begin_pause(active.clone()).unwrap();
        assert!(!nested.is_pending());
        nested.wait_for(Duration::ZERO).unwrap();
        drop(pause);
        assert!(!active.shared.lock().unwrap().can_automate(Instant::now()));
        drop(nested);
        assert!(active.shared.lock().unwrap().can_automate(Instant::now()));
    }

    #[test]
    fn automatic_pause_failure_does_not_clear_the_restore_guard() {
        let active = Arc::new(Active::new(1, 2));
        active.shared.lock().unwrap().queue(automatic());
        let pause = begin_pause(active.clone()).unwrap();
        assert!(pause.wait_for(Duration::ZERO).is_err());
        let mut shared = active.shared.lock().unwrap();
        assert!(shared.busy && shared.restore_failed && shared.automatic_disabled);
        assert_eq!(shared.automatic_holds, 1);
        shared.busy = false;
        shared.automatic = false;
        drop(shared);
        assert!(pause.wait_for(Duration::ZERO).is_err());
        drop(pause);
        assert_eq!(active.shared.lock().unwrap().automatic_holds, 0);
        assert!(begin_pause(active).is_err());
    }

    #[test]
    fn automatic_pause_leaves_explicit_trials_under_their_existing_guard() {
        let active = Arc::new(Active::new(1, 2));
        active.shared.lock().unwrap().queue(Request {
            lifetime: Lifetime::Trial,
            ..automatic()
        });
        let pause = begin_pause(active.clone()).unwrap();
        assert!(!pause.is_pending());
        pause.wait_for(Duration::ZERO).unwrap();
        let shared = active.shared.lock().unwrap();
        assert!(shared.busy && !shared.cancel && !shared.automatic);
        drop(shared);
        drop(pause);
    }

    #[test]
    fn automatic_pause_cannot_apply_to_a_closed_stream() {
        let active = Arc::new(Active::new(1, 2));
        let pause = begin_pause(active.clone()).unwrap();
        assert!(pause.is_current());
        active.shared.lock().unwrap().closing = true;
        assert!(!pause.is_current());
        let mut shared = active.shared.lock().unwrap();
        shared.closing = false;
        shared.owner_done = true;
        drop(shared);
        assert!(!pause.is_current());
    }
}
