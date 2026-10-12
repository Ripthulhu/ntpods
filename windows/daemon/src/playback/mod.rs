//! Opt-in AAC and Apple AAC-ELD playback through the NTPods render endpoint.
mod aac;
mod aac_protocol;
mod codec;
mod encoder;
mod pcm;
mod protocol;
mod render;
mod selection;
mod session;

use crate::{bt, driver::Driver, Ctx};
use codec::{AudioEncoder, MediaPackets, Mode};
use ntpods_ipc::PlaybackMode;
use pcm::{FrameAssembler, FrameClock, MAX_READ_BYTES};
use render::Render;
pub use selection::{load as load_selection, Selection};
use session::Session;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

struct Reset(SyncSender<io::Result<()>>);
struct Control {
    address: u64,
    intent: u64,
    reset: SyncSender<Reset>,
    stop: Arc<AtomicBool>,
}
static CONTROL: Mutex<Option<Control>> = Mutex::new(None);
static OWNED: AtomicBool = AtomicBool::new(false);

pub fn is_active() -> bool {
    OWNED.load(Ordering::Acquire)
}

struct Ownership;
impl Drop for Ownership {
    fn drop(&mut self) {
        OWNED.store(false, Ordering::Release);
    }
}

pub struct Worker {
    stop: Arc<AtomicBool>,
    selection: Arc<Mutex<Selection>>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn start(ctx: Ctx, driver: Driver, address: u64) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let disconnected = stop.clone();
        let selection = ctx.playback_selection.clone();
        let thread = thread::spawn(move || manage(ctx, driver, address, disconnected));
        Self {
            stop,
            selection,
            thread: Some(thread),
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.selection.lock().unwrap().stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn set_mode(ctx: &Ctx, mode: Option<PlaybackMode>) {
    let result = {
        let _preference = ctx.mic_preference.lock().unwrap();
        let mut selected = ctx.playback_selection.lock().unwrap();
        let mut state = ctx.state.lock().unwrap();
        let retry = mode.is_none() || state.playback_state == "error";
        let mode = mode.unwrap_or(selected.mode);
        let busy = selection::microphone_busy(
            &state,
            ctx.capture_active.load(Ordering::Acquire),
            ctx.mic_on.load(Ordering::Acquire),
        );
        match selected.change(mode, retry, busy, selection::save) {
            Ok(true) => {
                state.playback_mode = mode;
                state.playback_error = None;
                state.playback_state = if selected.cancel.is_some() {
                    "stopping"
                } else if mode == PlaybackMode::Windows {
                    "windows"
                } else if state.connected {
                    "starting"
                } else {
                    "disconnected"
                }
                .into();
                Ok(())
            }
            Ok(false) => Ok(()),
            Err(error) => {
                state.playback_error = Some(error.to_string());
                Err(error)
            }
        }
    };
    ctx.push_state();
    if let Err(error) = result {
        crate::log(&format!("playback selection: {error}"));
    }
}

fn publish(ctx: &Ctx, revision: u64, name: &str, error: Option<String>) {
    let current = {
        let selected = ctx.playback_selection.lock().unwrap();
        if selected.revision != revision {
            false
        } else {
            let mut state = ctx.state.lock().unwrap();
            state.playback_state = name.into();
            state.playback_error = error;
            true
        }
    };
    if current {
        ctx.push_state();
    }
}

fn manage(ctx: Ctx, driver: Driver, address: u64, disconnected: Arc<AtomicBool>) {
    let mut attempted = None;
    while !disconnected.load(Ordering::Acquire) && !ctx.shutting_down.load(Ordering::Acquire) {
        let request = {
            let _preference = ctx.mic_preference.lock().unwrap();
            let mut selected = ctx.playback_selection.lock().unwrap();
            let mut state = ctx.state.lock().unwrap();
            if !selected.should_attempt(
                attempted,
                disconnected.load(Ordering::Acquire) || ctx.shutting_down.load(Ordering::Acquire),
                &state,
                ctx.capture_active.load(Ordering::Acquire),
                ctx.mic_on.load(Ordering::Acquire),
            ) {
                None
            } else {
                let cancel = Arc::new(AtomicBool::new(false));
                selected.cancel = Some(cancel.clone());
                attempted = Some(selected.revision);
                state.playback_state = if selected.mode == PlaybackMode::Windows {
                    "stopping"
                } else {
                    "starting"
                }
                .into();
                state.playback_error = None;
                Some((selected.revision, selected.mode, cancel))
            }
        };
        let Some((revision, mode, cancel)) = request else {
            thread::sleep(Duration::from_millis(50));
            continue;
        };
        ctx.push_state();
        let result = match Mode::from_selection(mode) {
            Some(mode) => run(
                driver.clone(),
                address,
                cancel.clone(),
                mode,
                || publish(&ctx, revision, "ready", None),
                || publish(&ctx, revision, "stopping", None),
            ),
            None => bt::restore_experimental_audio(),
        };
        {
            let mut selected = ctx.playback_selection.lock().unwrap();
            selected.cancel = None;
        }
        let (name, error) = match result {
            Err(error) if error.kind() != io::ErrorKind::Interrupted => {
                crate::log(&format!("playback: {error}"));
                ("error", Some(error.to_string()))
            }
            _ if mode == PlaybackMode::Windows => ("windows", None),
            _ => ("disconnected", None),
        };
        publish(&ctx, revision, name, error);
    }
    let revision = ctx.playback_selection.lock().unwrap().revision;
    let (mode, failed) = {
        let state = ctx.state.lock().unwrap();
        (state.playback_mode, state.playback_state == "error")
    };
    if failed {
        return;
    }
    publish(
        &ctx,
        revision,
        if mode == PlaybackMode::Windows {
            "windows"
        } else {
            "disconnected"
        },
        None,
    );
}

struct Registration;
impl Drop for Registration {
    fn drop(&mut self) {
        CONTROL.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}

/// Called before taking the normal audio gate: this worker owns that gate.
pub fn reset_if_active(address: u64, intent: u64) -> Option<(u32, u32)> {
    let (sender, stop) = {
        let active = CONTROL.lock().unwrap_or_else(|e| e.into_inner());
        let control = active.as_ref()?;
        if control.address != address || control.intent != intent {
            control.stop.store(true, Ordering::Release);
            return None;
        }
        (control.reset.clone(), control.stop.clone())
    };
    let (reply, response) = mpsc::sync_channel(1);
    if sender.try_send(Reset(reply)).is_ok()
        && matches!(response.recv_timeout(Duration::from_secs(8)), Ok(Ok(())))
    {
        Some((0, 0))
    } else {
        stop.store(true, Ordering::Release);
        Some((1460, 1460))
    }
}

fn run(
    driver: Driver,
    address: u64,
    stop: Arc<AtomicBool>,
    mode: Mode,
    ready: impl FnOnce(),
    stopping: impl FnOnce(),
) -> io::Result<()> {
    bt::restore_experimental_audio()?;
    if stop.load(Ordering::Acquire) {
        return Ok(());
    }
    session::driver_ready(&driver, address)?;
    let render = Render::open()?;
    let mut encoder = AudioEncoder::new(mode)?;
    let intent = bt::playback_intent();
    // Keep volume on our endpoint until normal audio services are restored.
    OWNED.store(true, Ordering::Release);
    let _owned = Ownership;
    bt::with_experimental_playback(address, intent, || {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        let mut session = Session::open(driver, address, mode, stop.clone())?;
        session.suspend()?;
        let (reset, requests) = mpsc::sync_channel(1);
        {
            let mut active = CONTROL.lock().unwrap_or_else(|e| e.into_inner());
            if active.is_some() {
                return Err(io::Error::other("Playback is already running"));
            }
            *active = Some(Control {
                address,
                intent,
                reset,
                stop: stop.clone(),
            });
        }
        let _registered = Registration;
        crate::log(&format!(
            "experimental playback: {} ready on Speakers (NTPods Experimental)",
            mode.name()
        ));
        ready();
        let result = stream(
            &mut session,
            &render,
            &mut encoder,
            &stop,
            intent,
            &requests,
            mode,
        );
        stopping();
        result
    })
}

#[link(name = "winmm")]
extern "system" {
    fn timeBeginPeriod(period: u32) -> u32;
    fn timeEndPeriod(period: u32) -> u32;
}
struct TimerResolution;
impl TimerResolution {
    fn acquire() -> io::Result<Self> {
        if unsafe { timeBeginPeriod(1) } != 0 {
            return Err(io::Error::other("Playback timer is unavailable"));
        }
        Ok(Self)
    }
}
impl Drop for TimerResolution {
    fn drop(&mut self) {
        unsafe {
            timeEndPeriod(1);
        }
    }
}

fn prime_output(
    samples: usize,
    begin: impl FnOnce() -> io::Result<()>,
    mut read: impl FnMut() -> io::Result<Vec<u8>>,
) -> io::Result<FrameClock> {
    begin()?;
    // Negotiation can outlast the ring's 100 ms. Discard that audio before priming.
    for _ in 0..16 {
        if read()?.is_empty() {
            return FrameClock::new(samples, Instant::now() + Duration::from_millis(50));
        }
    }
    Err(io::Error::other("Could not clear queued playback"))
}

fn stream(
    session: &mut Session,
    render: &Render,
    encoder: &mut AudioEncoder,
    stop: &AtomicBool,
    intent: u64,
    requests: &Receiver<Reset>,
    mode: Mode,
) -> io::Result<()> {
    let _timer = TimerResolution::acquire()?;
    let origin = Instant::now();
    let mut packets = MediaPackets::new(encoder)?;
    let samples = mode.frame_samples();
    let mut pcm = FrameAssembler::new(samples)?;
    let mut active = false;
    let mut clock = FrameClock::new(samples, Instant::now())?;
    let mut next_poll = Instant::now();
    let initial = render.status()?;
    let mut dropped = initial[2];
    let mut generation = initial[3];
    let mut outstanding = 0usize;
    while !stop.load(Ordering::Acquire) && bt::audio_request_is_current(intent) {
        if let Ok(Reset(reply)) = requests.try_recv() {
            let result = (|| {
                session.suspend()?;
                clock = prime_output(
                    samples,
                    || {
                        *encoder = AudioEncoder::new(mode)?;
                        if active {
                            session.start()?;
                        }
                        Ok(())
                    },
                    || render.read(MAX_READ_BYTES),
                )?;
                packets.clear();
                pcm.reset();
                outstanding = 0;
                let status = render.status()?;
                dropped = status[2];
                generation = status[3];
                Ok(())
            })();
            let failed = result.is_err();
            let _ = reply.send(result);
            if failed {
                return Err(io::Error::other("Playback stream restart failed"));
            }
        }
        let now = Instant::now();
        if now >= next_poll {
            session.poll()?;
            next_poll = now + Duration::from_millis(20);
        }
        let status = render.status()?;
        if active && (status[0] == 0 || status[3] != generation) {
            session.suspend()?;
            active = false;
            packets.clear();
            pcm.reset();
            outstanding = 0;
        }
        if status[0] != 0 && !active {
            clock = prime_output(
                samples,
                || {
                    *encoder = AudioEncoder::new(mode)?;
                    session.start()
                },
                || render.read(MAX_READ_BYTES),
            )?;
            let primed = render.status()?;
            dropped = primed[2];
            generation = primed[3];
            active = true;
        } else if active && status[2] != dropped {
            return Err(io::Error::other(
                "Playback reader fell behind the Windows audio clock",
            ));
        }
        if active && now >= clock.deadline() {
            if now.duration_since(clock.deadline()) > Duration::from_millis(100)
                || outstanding * samples >= 4800
            {
                return Err(io::Error::other(
                    "Playback processing missed its audio deadline",
                ));
            }
            let data = render.read(pcm.read_limit())?;
            if render.status()?[3] != generation {
                // Pause/Run can replace the producer between STATUS and READ_PCM.
                continue;
            }
            if let Some(frame) = pcm.push(&data)? {
                encoder.write(frame)?;
                outstanding += 1;
                clock.advance();
            }
        }
        while active {
            let Some(unit) = encoder.read()? else {
                break;
            };
            outstanding = outstanding.saturating_sub(1);
            for packet in packets.push(unit, origin.elapsed().as_millis() as u32, session.mtu)? {
                session.media(&packet)?;
            }
        }
        thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slow_start_discards_backlog_before_priming() {
        use std::cell::RefCell;
        use std::collections::VecDeque;
        let queue = RefCell::new(VecDeque::new());
        let before = Instant::now();
        let clock = prime_output(
            1024,
            || {
                // A 150 ms START response leaves the newest 100 ms in the driver ring.
                queue.borrow_mut().extend(vec![vec![7; MAX_READ_BYTES]; 10]);
                Ok(())
            },
            || Ok(queue.borrow_mut().pop_front().unwrap_or_default()),
        )
        .unwrap();
        assert!(queue.borrow().is_empty());
        assert!(clock.deadline() >= before + Duration::from_millis(50));
        assert!(prime_output(480, || Ok(()), || Ok(vec![1; MAX_READ_BYTES])).is_err());
        let error = prime_output(
            480,
            || Err(io::Error::other("START rejected")),
            || {
                panic!("failed START must not consume PCM");
            },
        );
        assert!(error.is_err());
    }

    #[test]
    fn reset_reaches_worker_and_new_intent_cancels_ownership() {
        let (sender, requests) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        *CONTROL.lock().unwrap() = Some(Control {
            address: 1,
            intent: 3,
            reset: sender,
            stop: stop.clone(),
        });
        let _registration = Registration;
        let worker = thread::spawn(move || {
            let Reset(reply) = requests.recv_timeout(Duration::from_secs(1)).unwrap();
            reply.send(Ok(())).unwrap();
        });
        assert_eq!(reset_if_active(1, 3), Some((0, 0)));
        worker.join().unwrap();
        assert_eq!(reset_if_active(1, 5), None);
        assert!(stop.load(Ordering::Acquire));
        drop(_registration);
        assert_eq!(reset_if_active(1, 3), None);
    }
}
