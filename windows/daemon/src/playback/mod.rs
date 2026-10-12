//! AAC and Apple AAC-ELD playback through the AirPods AudioSink driver.
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
use ntpods_ipc::{PlaybackBitrate, PlaybackMode};
use pcm::{FrameAssembler, FrameClock, MAX_READ_BYTES};
use render::Render;
pub use selection::{load as load_selection, Selection};
use session::Session;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

struct Reset(SyncSender<io::Result<()>>);
struct Control {
    address: u64,
    intent: u64,
    reset: SyncSender<Reset>,
    stop: Arc<AtomicBool>,
}
static CONTROL: Mutex<Option<Control>> = Mutex::new(None);
static NATIVE_ADDRESS: AtomicU64 = AtomicU64::new(0);

pub fn active_address() -> Option<u64> {
    let address = NATIVE_ADDRESS.load(Ordering::Acquire);
    (address != 0).then_some(address)
}

pub fn owns_sink(address: u64) -> bool {
    address != 0
        && (NATIVE_ADDRESS.load(Ordering::Acquire) == address
            || crate::driver::native_audio_bound(address))
}

pub fn start(ctx: Ctx) {
    let bound = crate::driver::native_audio_bound(ctx.mac());
    {
        let mut selected = ctx.playback_selection.lock().unwrap();
        selected.bitrates = selection::load_bitrates();
        let mut state = ctx.state.lock().unwrap();
        state.playback_available = bound;
        state.playback_native_driver = bound;
        state.playback_aac_bitrate = selected.bitrates[0];
        state.playback_aac_eld_bitrate = selected.bitrates[1];
    }
    thread::spawn(move || {
        manage(&ctx);
        ctx.playback_done.store(true, Ordering::Release);
    });
}

pub fn set_bitrate(ctx: &Ctx, mode: PlaybackMode, bitrate: PlaybackBitrate) {
    {
        let _preference = ctx.mic_preference.lock().unwrap();
        let mut selected = ctx.playback_selection.lock().unwrap();
        let mut state = ctx.state.lock().unwrap();
        let busy = selection::microphone_busy(
            &state,
            ctx.capture_active.load(Ordering::Acquire),
            ctx.mic_on.load(Ordering::Acquire),
        );
        match selected.change_bitrate(mode, bitrate, busy, selection::save_bitrates) {
            Ok(changed) => {
                state.playback_aac_bitrate = selected.bitrates[0];
                state.playback_aac_eld_bitrate = selected.bitrates[1];
                if changed && mode == selected.mode {
                    state.playback_error = None;
                    state.playback_bitrate_kbps = None;
                    state.playback_state = if selected.cancel.is_some() {
                        "stopping"
                    } else {
                        "starting"
                    }
                    .into();
                }
            }
            Err(error) => state.playback_error = Some(error.to_string()),
        }
    }
    ctx.push_state();
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
        match if !state.playback_native_driver {
            Err(io::Error::other(
                "The NTPods playback driver is not installed for these AirPods.",
            ))
        } else {
            selected.change(mode, retry, busy, selection::save)
        } {
            Ok(true) => {
                state.playback_mode = mode;
                state.playback_error = None;
                state.playback_state = if selected.cancel.is_some() {
                    "stopping"
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

fn publication_current(
    current_revision: u64,
    revision: u64,
    current_address: u64,
    address: u64,
    current_intent: bool,
) -> bool {
    current_revision == revision && current_address == address && current_intent
}

fn publish(
    ctx: &Ctx,
    address: u64,
    revision: u64,
    intent: u64,
    name: &str,
    error: Option<String>,
    bitrate: Option<u32>,
) {
    let current = {
        let selected = ctx.playback_selection.lock().unwrap();
        let mut state = ctx.state.lock().unwrap();
        if !publication_current(
            selected.revision,
            revision,
            ctx.mac(),
            address,
            bt::audio_request_is_current(intent),
        ) {
            false
        } else {
            state.playback_state = name.into();
            state.playback_error = error;
            state.playback_bitrate_kbps = bitrate;
            true
        }
    };
    if current {
        ctx.push_state();
    }
}

fn availability(ctx: &Ctx, address: u64, bound: bool) {
    let changed = {
        let mut state = ctx.state.lock().unwrap();
        if ctx.mac() != address {
            return;
        }
        let changed = state.playback_available != bound || state.playback_native_driver != bound;
        state.playback_available = bound;
        state.playback_native_driver = bound;
        if !bound {
            state.playback_state = "unavailable".into();
        }
        changed
    };
    if changed {
        ctx.push_state();
    }
}

fn manage(ctx: &Ctx) {
    let mut address = 0;
    let mut render: Option<Render> = None;
    let mut attempted = None;
    let mut next_poll = Instant::now();
    let mut connected = false;
    while !ctx.shutting_down.load(Ordering::Acquire) {
        if ctx.mac() != address {
            render.take();
            NATIVE_ADDRESS.store(0, Ordering::Release);
            address = ctx.mac();
            attempted = None;
            next_poll = Instant::now();
        }
        if Instant::now() >= next_poll {
            if render
                .as_ref()
                .is_some_and(|render| render.status().is_err())
            {
                render.take();
                NATIVE_ADDRESS.store(0, Ordering::Release);
                attempted = None;
            }
            let bound = address != 0 && crate::driver::native_audio_bound(address);
            availability(ctx, address, bound);
            connected = bound
                && !ctx.user_disconnected.load(Ordering::Acquire)
                && bt::is_connected(address);
            if !connected {
                render.take();
                NATIVE_ADDRESS.store(0, Ordering::Release);
                attempted = None;
                if bound {
                    let revision = ctx.playback_selection.lock().unwrap().revision;
                    publish(
                        ctx,
                        address,
                        revision,
                        bt::playback_intent(),
                        "disconnected",
                        None,
                        None,
                    );
                }
            }
            next_poll = Instant::now() + Duration::from_secs(1);
        }
        if !connected || ctx.user_disconnected.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(50));
            continue;
        }
        let request = {
            let _preference = ctx.mic_preference.lock().unwrap();
            let mut selected = ctx.playback_selection.lock().unwrap();
            let mut state = ctx.state.lock().unwrap();
            let intent = bt::playback_intent();
            if ctx.mac() != address
                || !selected.should_attempt(
                    selection::attempted_revision(attempted, intent),
                    ctx.shutting_down.load(Ordering::Acquire),
                    &state,
                    ctx.capture_active.load(Ordering::Acquire),
                    ctx.mic_on.load(Ordering::Acquire),
                )
            {
                None
            } else {
                let cancel = Arc::new(AtomicBool::new(false));
                selected.cancel = Some(cancel.clone());
                attempted = Some((selected.revision, intent));
                state.playback_state = "starting".into();
                state.playback_error = None;
                Some((
                    selected.revision,
                    selected.mode,
                    selected.bitrate(selected.mode),
                    cancel,
                    intent,
                ))
            }
        };
        let Some((revision, mode, bitrate, cancel, intent)) = request else {
            thread::sleep(Duration::from_millis(50));
            continue;
        };
        ctx.push_state();
        let result = (|| {
            if render.is_none() {
                render = Some(Render::open(address)?);
            }
            let render = render.as_ref().unwrap();
            NATIVE_ADDRESS.store(address, Ordering::Release);
            let driver = ctx.shared_driver.open(address)?;
            let mode = Mode::from_selection(mode)
                .ok_or_else(|| io::Error::other("Select AAC or AAC-ELD."))?;
            run(
                ctx,
                driver,
                address,
                render,
                cancel,
                mode,
                bitrate,
                intent,
                |kbps| {
                    publish(ctx, address, revision, intent, "ready", None, Some(kbps));
                },
                || publish(ctx, address, revision, intent, "stopping", None, None),
            )
        })();
        ctx.playback_selection.lock().unwrap().cancel = None;
        if result.as_ref().is_err_and(|error| {
            matches!(
                error.kind(),
                io::ErrorKind::NotConnected | io::ErrorKind::ConnectionReset
            )
        }) {
            render.take();
            NATIVE_ADDRESS.store(0, Ordering::Release);
            attempted = None;
            connected = false;
            next_poll = Instant::now() + Duration::from_secs(1);
        }
        let (name, error) = match result {
            Err(error) if error.kind() != io::ErrorKind::Interrupted => {
                crate::log(&format!("playback: {error}"));
                ("error", Some(error.to_string()))
            }
            _ => ("disconnected", None),
        };
        publish(ctx, address, revision, intent, name, error, None);
    }
    render.take();
    NATIVE_ADDRESS.store(0, Ordering::Release);
}

struct Registration;
impl Drop for Registration {
    fn drop(&mut self) {
        CONTROL.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}

/// Native playback resets its transport without removing the AudioSink devnode.
pub fn reset_if_active(address: u64, intent: u64) -> Option<(u32, u32)> {
    let (sender, stop) = {
        let active = CONTROL.lock().unwrap_or_else(|e| e.into_inner());
        let Some(control) = active.as_ref() else {
            return owns_sink(address).then_some((1167, 1167));
        };
        if control.address != address || control.intent != intent {
            control.stop.store(true, Ordering::Release);
            return Some((1223, 1223));
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

fn shutdown_complete(shutting_down: bool, receiver_done: bool) -> bool {
    // Microphone cleanup still needs the native transport's reset handler.
    shutting_down && receiver_done
}

fn run(
    ctx: &Ctx,
    driver: Driver,
    address: u64,
    render: &Render,
    stop: Arc<AtomicBool>,
    mode: Mode,
    bitrate: PlaybackBitrate,
    intent: u64,
    ready: impl FnOnce(u32),
    stopping: impl FnOnce(),
) -> io::Result<()> {
    if stop.load(Ordering::Acquire) {
        return Ok(());
    }
    session::driver_ready(&driver, address)?;
    render.set_ready(false)?;
    let session_ctx = ctx.clone();
    let mut session = Session::open(driver, address, mode, bitrate, stop.clone(), move || {
        shutdown_complete(
            session_ctx.shutting_down.load(Ordering::Acquire),
            session_ctx.receiver_done.load(Ordering::Acquire),
        ) || session_ctx.user_disconnected.load(Ordering::Acquire)
            || session_ctx.mac() != address
            || !bt::audio_request_is_current(intent)
    })?;
    session.suspend()?;
    let mut encoder = AudioEncoder::new(mode, session.bitrate)?;
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
    render.set_ready(true)?;
    ready(session.bitrate / 1000);
    let mut next_connection_check = Instant::now();
    let result = stream(
        &mut session,
        render,
        &mut encoder,
        &stop,
        intent,
        &requests,
        mode,
        || {
            if shutdown_complete(
                ctx.shutting_down.load(Ordering::Acquire),
                ctx.receiver_done.load(Ordering::Acquire),
            ) || ctx.user_disconnected.load(Ordering::Acquire)
                || ctx.mac() != address
            {
                return true;
            }
            if Instant::now() >= next_connection_check {
                next_connection_check = Instant::now() + Duration::from_secs(1);
                return !bt::is_connected(address);
            }
            false
        },
    );
    stopping();
    let muted = render.set_ready(false);
    result.and(muted)
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
    mut disconnected: impl FnMut() -> bool,
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
    while !stop.load(Ordering::Acquire) && bt::audio_request_is_current(intent) && !disconnected() {
        if let Ok(Reset(reply)) = requests.try_recv() {
            let result = (|| {
                session.suspend()?;
                clock = prime_output(
                    samples,
                    || {
                        *encoder = AudioEncoder::new(mode, session.bitrate)?;
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
                    *encoder = AudioEncoder::new(mode, session.bitrate)?;
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
    fn old_transport_cannot_publish_ready_over_a_new_reconnect() {
        assert!(publication_current(4, 4, 7, 7, true));
        assert!(!publication_current(4, 4, 7, 7, false));
        assert!(!publication_current(5, 4, 7, 7, true));
        assert!(!publication_current(4, 4, 8, 7, true));
    }

    #[test]
    fn playback_shutdown_waits_for_microphone_cleanup() {
        assert!(!shutdown_complete(false, false));
        assert!(!shutdown_complete(true, false));
        assert!(shutdown_complete(true, true));
    }

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
        assert_eq!(reset_if_active(1, 5), Some((1223, 1223)));
        assert!(stop.load(Ordering::Acquire));
        drop(_registration);
        assert_eq!(reset_if_active(1, 3), None);
    }
}
