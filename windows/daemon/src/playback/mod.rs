//! AAC and Apple AAC-ELD playback through the AirPods AudioSink driver.
mod aac_protocol;
mod buffer;
mod codec;
mod encoder;
mod engine_period;
mod pcm;
mod protocol;
mod render;
mod scheduling;
mod selection;
mod session;
mod settings;
mod timing;

use crate::{bt, driver::Driver, Ctx};
pub use buffer::{
    begin_automatic_pause, set as set_buffer, stop as stop_buffer, Active as BufferActive,
    AutomaticPause,
};
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
        state.playback_preferred_mode = Some(selected.mode);
        state.playback_aac_bitrate = selected.bitrates[0];
        state.playback_aac_eld_bitrate = selected.bitrates[1];
    }
    thread::spawn(move || {
        manage(&ctx);
        ctx.playback_done.store(true, Ordering::Release);
    });
}

pub fn set_bitrate(
    ctx: &Ctx,
    mode: PlaybackMode,
    bitrate: PlaybackBitrate,
    expected: (u64, u64, [PlaybackBitrate; 2]),
) {
    {
        let _preference = ctx.mic_preference.lock().unwrap();
        let mut selected = ctx.playback_selection.lock().unwrap();
        let mut state = ctx.state.lock().unwrap();
        if ctx.shutting_down.load(Ordering::Acquire)
            || bt::playback_intent() != expected.0
            || selected.revision != expected.1
            || selected.bitrates != expected.2
        {
            return;
        }
        let busy = selection::microphone_busy(
            &state,
            ctx.capture_active.load(Ordering::Acquire),
            ctx.mic_on.load(Ordering::Acquire),
        );
        let active = (state.playback_state == "ready").then_some(state.playback_mode);
        let affects_playback = selected.uses_bitrate(mode, active);
        match if state.playback_buffer_target_ms.is_some() {
            Err(io::Error::other(
                "Restore the buffer trial before changing playback.",
            ))
        } else {
            selected.change_bitrate(mode, bitrate, active, busy, selection::save_bitrates)
        } {
            Ok(changed) => {
                state.playback_aac_bitrate = selected.bitrates[0];
                state.playback_aac_eld_bitrate = selected.bitrates[1];
                if changed && affects_playback {
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

pub fn set_mode(ctx: &Ctx, mode: Option<PlaybackMode>, expected: (u64, u64, [PlaybackBitrate; 2])) {
    let result = {
        let _preference = ctx.mic_preference.lock().unwrap();
        let mut selected = ctx.playback_selection.lock().unwrap();
        let mut state = ctx.state.lock().unwrap();
        if ctx.shutting_down.load(Ordering::Acquire)
            || bt::playback_intent() != expected.0
            || selected.revision != expected.1
            || selected.bitrates != expected.2
        {
            return;
        }
        let retry = mode.is_none()
            || state.playback_state == "error"
            || mode.is_some_and(|mode| mode != state.playback_mode);
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
        } else if state.playback_buffer_target_ms.is_some() {
            Err(io::Error::other(
                "Restore the buffer trial before changing playback.",
            ))
        } else {
            selected.change(mode, retry, busy, selection::save)
        } {
            Ok(true) => {
                state.playback_mode = mode;
                state.playback_preferred_mode = Some(mode);
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
    active: Option<(Mode, Option<u32>)>,
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
            state.playback_bitrate_kbps = active.and_then(|(_, bitrate)| bitrate);
            if name != "ready" {
                state.playback_delay_reporting = false;
                state.playback_delay_tenths_ms = None;
            }
            if let Some((mode, _)) = active {
                state.playback_mode = mode.selection();
            }
            true
        }
    };
    if current {
        ctx.push_state();
    }
}

fn publish_delay(
    ctx: &Ctx,
    address: u64,
    revision: u64,
    intent: u64,
    reporting: bool,
    delay: Option<u16>,
) {
    let changed = {
        let selected = ctx.playback_selection.lock().unwrap();
        let mut state = ctx.state.lock().unwrap();
        if !publication_current(
            selected.revision,
            revision,
            ctx.mac(),
            address,
            bt::audio_request_is_current(intent),
        ) {
            return;
        }
        let changed =
            state.playback_delay_reporting != reporting || state.playback_delay_tenths_ms != delay;
        state.playback_delay_reporting = reporting;
        state.playback_delay_tenths_ms = delay;
        changed
    };
    if changed {
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
                state.playback_mode = selected.mode;
                state.playback_bitrate_kbps = None;
                Some((
                    selected.revision,
                    selected.mode,
                    selected.bitrates,
                    cancel,
                    intent,
                ))
            }
        };
        let Some((revision, mode, bitrates, cancel, intent)) = request else {
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
                bitrates,
                intent,
                |mode, kbps| {
                    publish(
                        ctx,
                        address,
                        revision,
                        intent,
                        if kbps.is_some() { "ready" } else { "starting" },
                        None,
                        Some((mode, kbps)),
                    );
                },
                |reporting, delay| publish_delay(ctx, address, revision, intent, reporting, delay),
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
    bitrates: [PlaybackBitrate; 2],
    intent: u64,
    report_codec: impl Fn(Mode, Option<u32>),
    report_delay: impl Fn(bool, Option<u16>),
    stopping: impl FnOnce(),
) -> io::Result<()> {
    if stop.load(Ordering::Acquire) {
        return Ok(());
    }
    let settings = settings::Settings::load()?;
    session::driver_ready(&driver, address)?;
    render.set_ready(false)?;
    let session_ctx = ctx.clone();
    let buffer_driver = driver.clone();
    let mut session = Session::open(
        driver,
        address,
        mode,
        settings.eld_aus,
        bitrates,
        stop.clone(),
        move || {
            shutdown_complete(
                session_ctx.shutting_down.load(Ordering::Acquire),
                session_ctx.receiver_done.load(Ordering::Acquire),
            ) || session_ctx.user_disconnected.load(Ordering::Acquire)
                || session_ctx.mac() != address
                || !bt::audio_request_is_current(intent)
        },
        |mode| report_codec(mode, None),
    )?;
    let mut buffer = buffer::Stream::new(ctx, address, intent, buffer_driver);
    let mode = session.mode;
    let reserve = settings.reserve(mode)?;
    crate::log(&format!(
        "playback: ELD AUs per packet={}, source reserve={} ms",
        settings.eld_aus,
        reserve.as_millis()
    ));
    if mode == Mode::Eld {
        crate::log(&format!(
            "playback: AAC-ELD RTP clock={} Hz",
            settings.eld_rtp_clock
        ));
    }
    session.suspend()?;
    let mut encoder = AudioEncoder::new(mode, session.bitrate, session.variable)?;
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
    report_codec(mode, Some(session.bitrate / 1000));
    report_delay(session.delay_reporting, session.delay_report);
    let mut next_connection_check = Instant::now();
    let mut timing = timing::Timing::default();
    let result = stream(
        &mut session,
        &mut buffer,
        render,
        address,
        &mut encoder,
        &stop,
        intent,
        &requests,
        mode,
        reserve,
        settings.eld_rtp_clock,
        &mut timing,
        &report_delay,
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
    let restored = buffer.cleanup(render);
    let muted = render.set_ready(false);
    if let Some(summary) = timing.summary() {
        crate::log(&format!("playback timing {}: {summary}", mode.name()));
    }
    result.and(restored).and(muted)
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
    reserve: Duration,
    begin: impl FnOnce() -> io::Result<()>,
    mut read: impl FnMut() -> io::Result<Vec<u8>>,
) -> io::Result<FrameClock> {
    begin()?;
    // Negotiation can outlast the ring's 100 ms. Discard that audio before priming.
    for _ in 0..16 {
        if read()?.is_empty() {
            return FrameClock::new(samples, Instant::now() + reserve);
        }
    }
    Err(io::Error::other("Could not clear queued playback"))
}

fn primed_status(
    render: &Render,
    clock: &mut FrameClock,
    driver_clock: bool,
    reserve: Duration,
) -> io::Result<[u32; 4]> {
    let status = render.status()?;
    if driver_clock {
        let snapshot = render.clock()?;
        if snapshot.generation != status[3] {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "Playback producer changed while priming",
            ));
        }
        clock.use_driver_clock(snapshot.virtual_hns, reserve);
    }
    Ok(status)
}

fn playback_wait(
    notified: bool,
    adjusting: bool,
    control_wait: Duration,
    frame_wait: Option<Duration>,
) -> Duration {
    let wait = frame_wait.map_or(control_wait, |frame| frame.min(control_wait));
    if !notified || adjusting {
        wait.min(Duration::from_millis(1))
    } else {
        wait
    }
}

fn stream(
    session: &mut Session,
    buffer: &mut buffer::Stream,
    render: &Render,
    address: u64,
    encoder: &mut AudioEncoder,
    stop: &AtomicBool,
    intent: u64,
    requests: &Receiver<Reset>,
    mode: Mode,
    reserve: Duration,
    eld_rtp_clock: u32,
    timing: &mut timing::Timing,
    report_delay: impl Fn(bool, Option<u16>),
    mut disconnected: impl FnMut() -> bool,
) -> io::Result<()> {
    let _timer = TimerResolution::acquire()?;
    let _scheduling = scheduling::AudioScheduling::acquire();
    let mut packets = MediaPackets::new(encoder, session.eld_aus, eld_rtp_clock)?;
    let samples = mode.frame_samples();
    let mut pcm = FrameAssembler::new(samples)?;
    let mut active = false;
    let mut clock = FrameClock::new(samples, Instant::now())?;
    let mut next_poll = Instant::now();
    let initial = render.status()?;
    let mut dropped = initial[2];
    let mut generation = initial[3];
    let mut outstanding = 0usize;
    let mut engine_period = engine_period::Lease::default();
    let notifications = match render.notifications(thread::current()) {
        Ok(notifications) => notifications,
        Err(error) => {
            crate::log(&format!("playback: PCM notifications unavailable: {error}"));
            None
        }
    };
    let driver_clock = match render.clock() {
        Ok(_) => true,
        Err(error) if matches!(error.raw_os_error(), Some(1 | 50)) => false,
        Err(error) => return Err(error),
    };
    while !stop.load(Ordering::Acquire) && bt::audio_request_is_current(intent) && !disconnected() {
        if let Ok(Reset(reply)) = requests.try_recv() {
            let result = buffer.cleanup_before(render, || {
                session.restore_media_timeout()?;
                session.suspend()?;
                clock = prime_output(
                    samples,
                    reserve,
                    || {
                        *encoder = AudioEncoder::new(mode, session.bitrate, session.variable)?;
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
                let status = primed_status(render, &mut clock, driver_clock, reserve)?;
                dropped = status[2];
                generation = status[3];
                Ok(())
            });
            let failed = result.is_err();
            if !failed && active {
                buffer.started();
            }
            let _ = reply.send(result);
            if failed {
                return Err(io::Error::other("Playback stream restart failed"));
            }
        }
        let now = Instant::now();
        if now >= next_poll {
            session.poll()?;
            report_delay(session.delay_reporting, session.delay_report);
            next_poll = now + Duration::from_millis(20);
        }
        let mut status = render.status()?;
        if engine_period.update((status[0] != 0).then_some(status[3]), || {
            engine_period::EnginePeriod::acquire(address, || {
                stop.load(Ordering::Acquire) || !bt::audio_request_is_current(intent)
            })
            .map_err(|error| {
                crate::log(&format!(
                    "playback: automatic 5 ms period unavailable: {error}"
                ))
            })
            .ok()
        }) {
            // Changing the engine period can replace Windows' producer and its generation.
            status = render.status()?;
            engine_period.settled((status[0] != 0).then_some(status[3]));
            if stop.load(Ordering::Acquire)
                || !bt::audio_request_is_current(intent)
                || disconnected()
            {
                break;
            }
        }
        if active && (status[0] == 0 || status[3] != generation) {
            buffer.cleanup_before(render, || {
                session.restore_media_timeout()?;
                session.suspend()
            })?;
            active = false;
            packets.clear();
            pcm.reset();
            outstanding = 0;
        }
        if status[0] != 0 && !active {
            clock = prime_output(
                samples,
                reserve,
                || {
                    *encoder = AudioEncoder::new(mode, session.bitrate, session.variable)?;
                    session.start()
                },
                || render.read(MAX_READ_BYTES),
            )?;
            let primed = primed_status(render, &mut clock, driver_clock, reserve)?;
            dropped = primed[2];
            generation = primed[3];
            buffer.started();
            active = true;
        } else if active && status[2] != dropped {
            return Err(io::Error::other(
                "Playback reader fell behind the Windows audio clock",
            ));
        }
        if buffer.poll(session, render)? {
            if !active {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionReset,
                    "Playback stopped during the buffer trial",
                ));
            }
            clock = prime_output(
                samples,
                reserve,
                || {
                    *encoder = AudioEncoder::new(mode, session.bitrate, session.variable)?;
                    session.start()
                },
                || render.read(MAX_READ_BYTES),
            )?;
            packets.clear();
            pcm.reset();
            outstanding = 0;
            let primed = primed_status(render, &mut clock, driver_clock, reserve)?;
            dropped = primed[2];
            generation = primed[3];
            buffer.started();
            continue;
        }
        let observed_at = Instant::now();
        let mut adjusting = false;
        let host_time = if active && driver_clock {
            let snapshot = render.clock()?;
            if snapshot.generation != generation {
                continue;
            }
            adjusting = matches!(snapshot.phase, 1 | 2);
            Some(snapshot.virtual_hns)
        } else {
            None
        };
        let mut waiting_for_pcm = false;
        if let Some(lateness) = if active {
            clock.lateness(Instant::now(), host_time)?
        } else {
            None
        } {
            if lateness > Duration::from_millis(100) || outstanding * samples >= 4800 {
                return Err(io::Error::other(
                    "Playback processing missed its audio deadline",
                ));
            }
            let mut replaced = false;
            let frame = pcm.read_available(
                |limit| render.read(limit),
                || {
                    replaced = render.status()?[3] != generation;
                    Ok(!replaced)
                },
            )?;
            if replaced {
                continue;
            }
            if let Some(frame) = frame {
                encoder.write(frame)?;
                timing.frame(samples, status[1], lateness);
                outstanding += 1;
                clock.advance();
            } else {
                waiting_for_pcm = true;
            }
        }
        while active {
            let Some(unit) = encoder.read()? else {
                break;
            };
            outstanding = outstanding.saturating_sub(1);
            for packet in packets.push(unit, session.mtu)? {
                let started = Instant::now();
                let sent = session.media(&packet);
                timing.send(started.elapsed());
                sent?;
            }
        }
        let frame_wait = if active && !waiting_for_pcm {
            Some(
                clock
                    .until_due(observed_at, host_time)?
                    .saturating_sub(observed_at.elapsed()),
            )
        } else {
            None
        };
        thread::park_timeout(playback_wait(
            notifications
                .as_ref()
                .is_some_and(|notifications| notifications.available()),
            adjusting,
            next_poll.saturating_duration_since(Instant::now()),
            frame_wait,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notification_waits_preserve_frame_and_control_deadlines_and_polling_fallback() {
        let ms = Duration::from_millis;
        assert_eq!(playback_wait(true, false, ms(20), Some(ms(4))), ms(4));
        assert_eq!(playback_wait(true, false, ms(2), Some(ms(9))), ms(2));
        assert_eq!(
            playback_wait(true, false, ms(20), Some(Duration::ZERO)),
            Duration::ZERO
        );
        // Once a due frame has no PCM, wait for data instead of spinning on its deadline.
        assert_eq!(playback_wait(true, false, ms(20), None), ms(20));
        assert_eq!(playback_wait(false, false, ms(20), None), ms(1));
        assert_eq!(playback_wait(true, true, ms(20), Some(ms(4))), ms(1));
        assert_eq!(
            playback_wait(false, true, Duration::ZERO, None),
            Duration::ZERO
        );
    }

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
    fn start_and_restart_discard_backlog_before_priming() {
        use std::cell::RefCell;
        use std::collections::VecDeque;
        for samples in [480, 1024] {
            for reserve_ms in [20, 30, 50] {
                if samples == 1024 && reserve_ms == 20 {
                    continue;
                }
                let reserve = Duration::from_millis(reserve_ms);
                let queue = RefCell::new(VecDeque::new());
                for _ in 0..2 {
                    let before = Instant::now();
                    let clock = prime_output(
                        samples,
                        reserve,
                        || {
                            // A 150 ms START response leaves the newest 100 ms in the driver ring.
                            queue.borrow_mut().extend(vec![vec![7; MAX_READ_BYTES]; 10]);
                            Ok(())
                        },
                        || Ok(queue.borrow_mut().pop_front().unwrap_or_default()),
                    )
                    .unwrap();
                    let after = Instant::now();
                    assert!(queue.borrow().is_empty());
                    assert!(clock.deadline() >= before + reserve);
                    assert!(clock.deadline() <= after + reserve);
                }
            }
        }
        let reserve = Duration::from_millis(50);
        assert!(prime_output(480, reserve, || Ok(()), || Ok(vec![1; MAX_READ_BYTES])).is_err());
        let error = prime_output(
            480,
            reserve,
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
