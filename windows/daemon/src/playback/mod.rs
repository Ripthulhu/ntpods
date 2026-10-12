//! Opt-in AAC and Apple AAC-ELD playback through the NTPods render endpoint.
mod aac;
mod aac_protocol;
mod codec;
mod encoder;
mod pcm;
mod protocol;
mod render;
mod session;

use crate::{bt, driver::Driver};
use codec::{AudioEncoder, MediaPackets, Mode};
use pcm::{FrameAssembler, FrameClock, MAX_READ_BYTES};
use render::Render;
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
static ATTEMPTED: AtomicBool = AtomicBool::new(false);

pub fn is_active() -> bool {
    CONTROL.lock().unwrap_or_else(|e| e.into_inner()).is_some()
}

pub struct Worker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn start(driver: Driver, address: u64) -> Option<Self> {
        let mode = match Mode::requested(std::env::args()) {
            Ok(Some(mode)) => mode,
            Ok(None) => return None,
            Err(error) => {
                crate::log(&format!("experimental playback: {error}"));
                return None;
            }
        };
        if ATTEMPTED.swap(true, Ordering::AcqRel) {
            return None;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let cancelled = stop.clone();
        let thread = thread::spawn(move || {
            let result = run(driver, address, cancelled, mode);
            match result {
                Ok(()) => crate::log("experimental playback: stopped"),
                Err(error) => crate::log(&format!("experimental playback: {error}")),
            }
        });
        Some(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
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

fn run(driver: Driver, address: u64, stop: Arc<AtomicBool>, mode: Mode) -> io::Result<()> {
    session::driver_ready(&driver, address)?;
    let render = Render::open()?;
    let mut encoder = AudioEncoder::new(mode)?;
    let intent = bt::playback_intent();
    bt::with_experimental_playback(address, intent, || {
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        let mut session = Session::open(driver, address, mode)?;
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
        stream(
            &mut session,
            &render,
            &mut encoder,
            &stop,
            intent,
            &requests,
            mode,
        )
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
