//! Bounded receive-buffer trials on the current native playback stream.
use crate::{aap, bt, driver::Driver, Ctx};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const SETTLE: Duration = Duration::from_millis(500);
const TRIAL: Duration = Duration::from_secs(30);

pub struct Active {
    address: u64,
    intent: u64,
    driver: Driver,
    started: Mutex<Option<Instant>>,
    cancel: AtomicBool,
    worker: Mutex<Option<JoinHandle<()>>>,
    restore_failed: AtomicBool,
}

pub struct Stream {
    ctx: Ctx,
    active: Arc<Active>,
}

impl Stream {
    pub fn new(ctx: &Ctx, address: u64, intent: u64, driver: Driver) -> Self {
        let active = Arc::new(Active {
            address,
            intent,
            driver,
            started: Mutex::new(None),
            cancel: AtomicBool::new(false),
            worker: Mutex::new(None),
            restore_failed: AtomicBool::new(false),
        });
        *ctx.playback_buffer.lock().unwrap() = Some(active.clone());
        Self {
            ctx: ctx.clone(),
            active,
        }
    }

    pub fn started(&mut self) {
        self.active
            .started
            .lock()
            .unwrap()
            .get_or_insert_with(Instant::now);
    }

    pub fn stop(&mut self) -> io::Result<()> {
        stop_active(&self.active)
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let _ = self.stop();
        let mut slot = self.ctx.playback_buffer.lock().unwrap();
        if slot
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, &self.active))
        {
            *slot = None;
        }
    }
}

fn stop_active(active: &Active) -> io::Result<()> {
    let mut worker = active.worker.lock().unwrap();
    *active.started.lock().unwrap() = None;
    active.cancel.store(true, Ordering::Release);
    if let Some(worker) = worker.take() {
        if worker.join().is_err() {
            active.restore_failed.store(true, Ordering::Release);
        }
    }
    if active.restore_failed.load(Ordering::Acquire) {
        Err(io::Error::other(
            "The earbud buffer restore could not be sent. Reconnect the AirPods.",
        ))
    } else {
        Ok(())
    }
}

// AAP teardown must restore before closing the control channel, without waiting for readback.
pub fn stop(ctx: &Ctx) {
    let active = ctx.playback_buffer.lock().unwrap().clone();
    if let Some(active) = active {
        if let Err(error) = stop_active(&active) {
            crate::log(&format!("playback buffer: {error}"));
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

pub fn set(ctx: &Ctx, target: Option<u16>, restore: Option<u16>) {
    let result = request(ctx, target, restore);
    if let Err(error) = result {
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
    let Some(target) = target else {
        active.cancel.store(true, Ordering::Release);
        return Ok(());
    };
    let restore = validate(target, restore)?;
    let mut worker = active.worker.lock().unwrap();
    if worker.as_ref().is_some_and(|worker| !worker.is_finished()) {
        return Err(io::Error::other(
            "Wait for the current buffer trial to restore.",
        ));
    }
    if active.restore_failed.load(Ordering::Acquire) {
        return Err(io::Error::other(
            "Reconnect the AirPods after the failed buffer restore.",
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
        || !active
            .started
            .lock()
            .unwrap()
            .is_some_and(|time| time.elapsed() >= SETTLE)
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
    // This is an operator-supplied baseline, never inferred from an AVDTP delay report.
    state.playback_buffer_target_ms = Some(target);
    state.playback_buffer_error = None;
    active.cancel.store(false, Ordering::Release);
    drop(state);
    let task_ctx = ctx.clone();
    let task_active = active.clone();
    *worker = Some(thread::spawn(move || {
        run(&task_ctx, &task_active, target, restore)
    }));
    Ok(())
}

fn run(ctx: &Ctx, active: &Active, target: u16, restore: u16) {
    crate::log(&format!(
        "playback buffer: trial requested {target} ms, supplied restore {restore} ms, 30 s limit"
    ));
    let mut restore_failed = false;
    let result = trial(
        target,
        restore,
        || {
            !active.cancel.load(Ordering::Acquire)
                && ctx.mac() == active.address
                && bt::audio_request_is_current(active.intent)
                && !ctx.shutting_down.load(Ordering::Acquire)
                && !ctx.user_disconnected.load(Ordering::Acquire)
                && !ctx.wants_reconnect.load(Ordering::Acquire)
                && !ctx.capture_active.load(Ordering::Acquire)
        },
        |value| {
            active
                .driver
                .send_playback_control(&aap::device_control_command(0x0B, value.into()))
        },
        || {
            let deadline = Instant::now() + TRIAL;
            while Instant::now() < deadline
                && !active.cancel.load(Ordering::Acquire)
                && ctx.mac() == active.address
                && bt::audio_request_is_current(active.intent)
                && !ctx.shutting_down.load(Ordering::Acquire)
                && !ctx.user_disconnected.load(Ordering::Acquire)
                && !ctx.wants_reconnect.load(Ordering::Acquire)
                && !ctx.capture_active.load(Ordering::Acquire)
            {
                thread::sleep(Duration::from_millis(20));
            }
        },
        || thread::sleep(SETTLE),
        &mut restore_failed,
    );
    active
        .restore_failed
        .store(restore_failed, Ordering::Release);
    let cancelled = matches!(&result, Ok(false));
    let error = result.err().map(|error| error.to_string());
    crate::log(&format!(
        "playback buffer: {}",
        error.as_deref().unwrap_or(if cancelled {
            "cancelled before sending"
        } else {
            "restore sent; device application unconfirmed"
        })
    ));
    let current = ctx
        .playback_buffer
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|current| std::ptr::eq(current.as_ref(), active));
    if current && ctx.mac() == active.address {
        let mut state = ctx.state.lock().unwrap();
        if !restore_failed {
            state.playback_buffer_target_ms = None;
        }
        state.playback_buffer_error = error;
        drop(state);
        ctx.push_state();
    }
}

fn trial(
    target: u16,
    restore: u16,
    current: impl FnOnce() -> bool,
    mut send: impl FnMut(u16) -> io::Result<()>,
    wait: impl FnOnce(),
    mut settle: impl FnMut(),
    restore_failed: &mut bool,
) -> io::Result<bool> {
    if !current() {
        return Ok(false);
    }
    let applied = send(target);
    // Keep control changes apart and leave a quiet interval before AVDTP suspension.
    settle();
    if applied.is_ok() {
        wait();
    }
    let restored = send(restore);
    settle();
    *restore_failed = restored.is_err();
    restored.map_err(|_| {
        io::Error::other("The earbud buffer restore could not be sent. Reconnect the AirPods.")
    })?;
    applied.map(|_| true).map_err(|_| {
        io::Error::other("The buffer trial could not be sent; the restore value was sent.")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

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
    fn cancellation_and_failed_apply_still_restore_the_supplied_value() {
        for fail_apply in [false, true] {
            let events = RefCell::new(Vec::new());
            let mut failed = false;
            let result = trial(
                60,
                300,
                || true,
                |value| {
                    events.borrow_mut().push(value);
                    if fail_apply && value == 60 {
                        Err(io::Error::other("timeout"))
                    } else {
                        Ok(())
                    }
                },
                || events.borrow_mut().push(1),
                || events.borrow_mut().push(0),
                &mut failed,
            );
            assert_eq!(result.is_err(), fail_apply);
            assert!(!failed);
            assert_eq!(
                *events.borrow(),
                if fail_apply {
                    vec![60, 0, 300, 0]
                } else {
                    vec![60, 0, 1, 300, 0]
                }
            );
        }
    }

    #[test]
    fn failed_restore_remains_an_error() {
        let mut failed = false;
        assert!(trial(
            60,
            150,
            || true,
            |value| if value == 150 {
                Err(io::Error::other("disconnected"))
            } else {
                Ok(())
            },
            || {},
            || {},
            &mut failed
        )
        .is_err());
        assert!(failed);
    }

    #[test]
    fn cancellation_before_apply_sends_neither_target_nor_restore() {
        let mut failed = false;
        assert!(trial(
            60,
            150,
            || false,
            |_| panic!("cancelled write"),
            || panic!("cancelled wait"),
            || panic!("cancelled settle"),
            &mut failed
        )
        .is_ok());
        assert!(!failed);
    }
}
