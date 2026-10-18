//! Exercise the production buffer owner without Windows devices or Bluetooth.
#![cfg(feature = "experimental-playback")]
#![allow(dead_code)]

use ntpods_ipc::Snapshot;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[path = "../src/playback/buffer.rs"]
mod buffer;
#[path = "../src/connection_request.rs"]
mod connection_request;
#[path = "../src/mic_lifecycle.rs"]
mod mic_lifecycle;
#[path = "../src/playback/selection.rs"]
mod selection;

#[derive(Debug, PartialEq)]
enum Event {
    Preflight,
    Control(u16),
    Clock(u32, u32),
    CancelClock,
    Reopen(u16),
    Start,
    Published(Option<u16>, bool, String),
}

#[derive(Default)]
struct Hardware {
    events: Vec<Event>,
    phase: u32,
    fail_reopen: bool,
    fail_start: bool,
    fail_control: Option<u16>,
    fail_apply_reopen: bool,
    fail_preflight: bool,
    queued_delay: Option<u16>,
    reopen_delay: Option<u16>,
    poll_busy: bool,
    poll_error: Option<io::ErrorKind>,
}

type SharedHardware = Arc<Mutex<Hardware>>;

#[derive(Clone)]
struct Ctx {
    state: Arc<Mutex<Snapshot>>,
    playback_buffer: Arc<Mutex<Option<Arc<buffer::Active>>>>,
    mic_preference: Arc<Mutex<()>>,
    mic_demand: Arc<Mutex<mic_lifecycle::Demand>>,
    auto_mode: Arc<AtomicBool>,
    shutting_down: Arc<AtomicBool>,
    user_disconnected: Arc<AtomicBool>,
    wants_reconnect: Arc<AtomicBool>,
    capture_active: Arc<AtomicBool>,
    mic_on: Arc<AtomicBool>,
    shutdown_cleanup_failed: Arc<AtomicBool>,
    hardware: SharedHardware,
}

impl Ctx {
    fn new(hardware: SharedHardware) -> Self {
        Self {
            state: Arc::new(Mutex::new(Snapshot {
                connected: true,
                playback_native_driver: true,
                playback_state: "ready".into(),
                playback_delay_reporting: true,
                playback_delay_tenths_ms: Some(1500),
                ..Snapshot::default()
            })),
            playback_buffer: Arc::default(),
            mic_preference: Arc::default(),
            mic_demand: Arc::default(),
            auto_mode: Arc::default(),
            shutting_down: Arc::default(),
            user_disconnected: Arc::default(),
            wants_reconnect: Arc::default(),
            capture_active: Arc::default(),
            mic_on: Arc::default(),
            shutdown_cleanup_failed: Arc::default(),
            hardware,
        }
    }

    fn mac(&self) -> u64 {
        1
    }

    fn push_state(&self) {
        let state = self.state.lock().unwrap();
        self.hardware.lock().unwrap().events.push(Event::Published(
            state.playback_buffer_target_ms,
            state.playback_buffer_error.is_some(),
            state.playback_state.clone(),
        ));
    }
}

fn log(_: &str) {}

fn persist_preference(_: &str, _: &[u8]) -> io::Result<()> {
    panic!("Playback buffer tests must inject persistence instead of writing user preferences")
}

mod bt {
    pub fn audio_request_is_current(intent: u64) -> bool {
        intent == 7
    }
}

mod aap {
    pub fn device_control_command(id: u8, value: u32) -> [u8; 11] {
        let value = value.to_le_bytes();
        [4, 0, 4, 0, 9, 0, id, value[0], value[1], value[2], value[3]]
    }
}

mod driver {
    use super::*;

    pub struct Driver(pub SharedHardware);

    impl Driver {
        pub fn send_playback_control(&self, data: &[u8]) -> io::Result<()> {
            assert_eq!(&data[..7], &[4, 0, 4, 0, 9, 0, 0x0B]);
            let value = u16::from_le_bytes(data[7..9].try_into().unwrap());
            let mut hardware = self.0.lock().unwrap();
            hardware.events.push(Event::Control(value));
            if hardware.fail_control == Some(value) {
                Err(io::Error::other("control write failed"))
            } else {
                Ok(())
            }
        }
    }
}

mod render {
    use super::*;

    pub struct Render(pub SharedHardware);
    pub struct ClockStatus {
        pub generation: u32,
        pub phase: u32,
    }

    impl Render {
        pub fn clock(&self) -> io::Result<ClockStatus> {
            Ok(ClockStatus {
                generation: 1,
                phase: self.0.lock().unwrap().phase,
            })
        }

        pub fn set_clock(&self, interval: u32, delta: u32) -> io::Result<()> {
            let mut hardware = self.0.lock().unwrap();
            hardware.events.push(Event::Clock(interval, delta));
            hardware.phase = 2;
            Ok(())
        }

        pub fn cancel_clock(&self) -> io::Result<()> {
            let mut hardware = self.0.lock().unwrap();
            hardware.events.push(Event::CancelClock);
            hardware.phase = 5;
            Ok(())
        }
    }
}

mod codec {
    #[derive(Clone, Copy, PartialEq, Eq)]
    pub enum Mode {
        Aac,
        Eld,
    }
}

mod session {
    use super::*;

    pub struct Session {
        pub gaming_supported: Option<bool>,
        pub mode: super::codec::Mode,
        pub delay_serial: u64,
        pub delay_report: Option<u16>,
        pub delay_reporting: bool,
        pub flush_ms: u16,
        pub hardware: SharedHardware,
    }

    impl Session {
        pub fn poll(&mut self) -> io::Result<bool> {
            let mut hardware = self.hardware.lock().unwrap();
            if let Some(kind) = hardware.poll_error {
                return Err(io::Error::from(kind));
            }
            if let Some(delay) = hardware.queued_delay.take() {
                self.delay_serial = self.delay_serial.wrapping_add(1);
                self.delay_report = Some(delay);
            }
            Ok(!hardware.poll_busy)
        }

        pub fn check_flush_support(&self) -> io::Result<()> {
            let mut hardware = self.hardware.lock().unwrap();
            hardware.events.push(Event::Preflight);
            if hardware.fail_preflight {
                Err(io::Error::new(io::ErrorKind::Unsupported, "older driver"))
            } else {
                Ok(())
            }
        }

        pub fn reopen_media(&mut self, timeout: u16) -> io::Result<()> {
            let mut hardware = self.hardware.lock().unwrap();
            hardware.events.push(Event::Reopen(timeout));
            if (timeout == 200 && hardware.fail_reopen)
                || (timeout != 200 && hardware.fail_apply_reopen)
            {
                Err(io::Error::new(
                    io::ErrorKind::ConnectionReset,
                    "reopen failed",
                ))
            } else {
                self.delay_report = hardware.reopen_delay;
                if self.delay_report.is_some() {
                    self.delay_serial = self.delay_serial.wrapping_add(1);
                }
                self.flush_ms = timeout;
                Ok(())
            }
        }

        pub fn restore_media_timeout(&mut self) -> io::Result<()> {
            if self.flush_ms != 200 {
                self.reopen_media(200)?;
            }
            Ok(())
        }

        pub(super) fn start(&mut self) -> io::Result<()> {
            let mut hardware = self.hardware.lock().unwrap();
            hardware.events.push(Event::Start);
            if hardware.fail_start {
                Err(io::Error::other("restored START failed"))
            } else {
                Ok(())
            }
        }
    }
}

#[test]
fn capture_during_adjustment_keeps_gate_until_restored_start_or_cleanup() {
    for failure in [None, Some("reopen"), Some("start"), Some("cleanup")] {
        let hardware = Arc::new(Mutex::new(Hardware::default()));
        let ctx = Ctx::new(hardware.clone());
        let render = render::Render(hardware.clone());
        let mut session = session::Session {
            gaming_supported: None,
            mode: codec::Mode::Eld,
            delay_serial: 1,
            delay_report: Some(1500),
            delay_reporting: true,
            flush_ms: 200,
            hardware: hardware.clone(),
        };
        let mut stream = buffer::Stream::new(&ctx, 1, 7, driver::Driver(hardware.clone()));
        stream.started();
        std::thread::sleep(Duration::from_millis(501));
        buffer::set(&ctx, Some(60), Some(150), || true);
        assert_eq!(
            ctx.state.lock().unwrap().playback_buffer_target_ms,
            Some(60)
        );
        assert!(!mic_lifecycle::playback_allows_microphone(
            &ctx.state.lock().unwrap()
        ));
        assert!(stream.poll(&mut session, &render).unwrap());
        session.start().unwrap();
        stream.started();
        std::thread::sleep(Duration::from_millis(501));
        assert!(!stream.poll(&mut session, &render).unwrap());
        session.delay_report = Some(1500);
        session.delay_serial += 1;
        assert!(!stream.poll(&mut session, &render).unwrap());
        assert_eq!(hardware.lock().unwrap().phase, 2);

        ctx.capture_active.store(true, Ordering::Release);
        assert!(!stream.poll(&mut session, &render).unwrap());
        assert_eq!(hardware.lock().unwrap().phase, 5);
        assert!(!mic_lifecycle::playback_allows_microphone(
            &ctx.state.lock().unwrap()
        ));
        std::thread::sleep(Duration::from_millis(501));
        assert!(!stream.poll(&mut session, &render).unwrap());
        session.delay_serial += 1;
        assert!(!stream.poll(&mut session, &render).unwrap());
        std::thread::sleep(Duration::from_millis(501));

        {
            let mut hardware = hardware.lock().unwrap();
            assert!(hardware.events.contains(&Event::Control(150)));
            hardware.fail_reopen = failure == Some("reopen");
            hardware.fail_start = matches!(failure, Some("start" | "cleanup"));
            assert!(!hardware
                .events
                .iter()
                .any(|event| matches!(event, Event::Published(None, false, _))));
        }
        assert_eq!(
            ctx.state.lock().unwrap().playback_buffer_target_ms,
            Some(60)
        );
        assert!(!mic_lifecycle::playback_allows_microphone(
            &ctx.state.lock().unwrap()
        ));
        let restored = stream.poll(&mut session, &render).and_then(|reopened| {
            assert!(reopened);
            assert!(!mic_lifecycle::playback_allows_microphone(
                &ctx.state.lock().unwrap()
            ));
            session.start()?;
            assert!(!mic_lifecycle::playback_allows_microphone(
                &ctx.state.lock().unwrap()
            ));
            stream.started();
            Ok(())
        });
        if failure.is_none() {
            restored.unwrap();
            let state = ctx.state.lock().unwrap();
            assert_eq!(state.playback_buffer_target_ms, None);
            assert_eq!(state.playback_buffer_error, None);
            assert_eq!(state.playback_state, "ready");
            assert!(mic_lifecycle::playback_allows_microphone(&state));
        } else {
            assert!(restored.is_err());
            assert_eq!(
                ctx.state.lock().unwrap().playback_buffer_target_ms,
                Some(60)
            );
            assert!(!mic_lifecycle::playback_allows_microphone(
                &ctx.state.lock().unwrap()
            ));
            ctx.state.lock().unwrap().playback_state = "stopping".into();
            let cleanup = stream.cleanup_before(&render, || {
                assert_eq!(
                    ctx.state.lock().unwrap().playback_buffer_target_ms,
                    Some(60)
                );
                assert!(!mic_lifecycle::playback_allows_microphone(
                    &ctx.state.lock().unwrap()
                ));
                if failure == Some("cleanup") {
                    Err(io::Error::other("transport cleanup failed"))
                } else {
                    Ok(())
                }
            });
            assert_eq!(cleanup.is_err(), failure == Some("cleanup"));
            let mut state = ctx.state.lock().unwrap();
            assert_eq!(state.playback_state, "stopping");
            assert!(!mic_lifecycle::playback_allows_microphone(&state));
            assert_eq!(
                state.playback_buffer_target_ms,
                (failure == Some("cleanup")).then_some(60)
            );
            if failure == Some("cleanup") {
                state.playback_state = "ready".into();
                assert!(!mic_lifecycle::playback_allows_microphone(&state));
                state.playback_state = "stopping".into();
            }
            let events = &hardware.lock().unwrap().events;
            assert_eq!(
                events
                    .iter()
                    .filter(|event| **event == Event::Control(150))
                    .count(),
                2
            );
            assert!(!events.iter().any(
                |event| matches!(event, Event::Published(None, false, name) if name == "ready")
            ));
        }
        if failure == Some("cleanup") {
            assert!(stream.cleanup(&render).is_err());
            buffer::stop(&ctx);
            assert!(ctx.shutdown_cleanup_failed.load(Ordering::Acquire));
        } else {
            stream.cleanup(&render).unwrap();
        }
    }
}

#[test]
fn automatic_policy_restores_for_capture_then_reapplies_afterward() {
    let hardware = Arc::new(Mutex::new(Hardware::default()));
    let ctx = Ctx::new(hardware.clone());
    ctx.state.lock().unwrap().model = "A3064".into();
    let render = render::Render(hardware.clone());
    let mut session = session::Session {
        gaming_supported: None,
        mode: codec::Mode::Eld,
        delay_serial: 1,
        delay_report: Some(1500),
        delay_reporting: true,
        flush_ms: 200,
        hardware: hardware.clone(),
    };
    let mut stream = buffer::Stream::new(&ctx, 1, 7, driver::Driver(hardware.clone()));
    stream.started();
    std::thread::sleep(Duration::from_millis(501));
    assert!(stream.poll(&mut session, &render).unwrap());
    assert_eq!(session.flush_ms, 60);
    session.start().unwrap();
    stream.started();
    std::thread::sleep(Duration::from_millis(501));
    {
        let mut hardware = hardware.lock().unwrap();
        hardware.queued_delay = Some(1400);
        hardware.poll_busy = true;
    }
    assert!(!stream.poll(&mut session, &render).unwrap());
    assert!(!hardware
        .lock()
        .unwrap()
        .events
        .contains(&Event::Control(60)));
    hardware.lock().unwrap().poll_busy = false;
    assert!(!stream.poll(&mut session, &render).unwrap());
    assert!(hardware
        .lock()
        .unwrap()
        .events
        .contains(&Event::Control(60)));
    assert!(!stream.poll(&mut session, &render).unwrap());
    assert_eq!(hardware.lock().unwrap().phase, 0);
    session.delay_report = Some(0);
    session.delay_serial += 1;
    assert!(!stream.poll(&mut session, &render).unwrap());
    assert_eq!(hardware.lock().unwrap().phase, 0);
    session.delay_report = Some(1490);
    session.delay_serial += 1;
    assert!(!stream.poll(&mut session, &render).unwrap());
    assert!(hardware
        .lock()
        .unwrap()
        .events
        .contains(&Event::Clock(11, 79)));
    hardware.lock().unwrap().phase = 3;
    assert!(!stream.poll(&mut session, &render).unwrap());
    assert_eq!(
        ctx.state.lock().unwrap().playback_buffer_target_ms,
        Some(60)
    );

    ctx.capture_active.store(true, Ordering::Release);
    assert!(!stream.poll(&mut session, &render).unwrap());
    std::thread::sleep(Duration::from_millis(501));
    {
        let mut hardware = hardware.lock().unwrap();
        hardware.queued_delay = Some(1400);
        hardware.poll_busy = true;
    }
    assert!(!stream.poll(&mut session, &render).unwrap());
    assert!(!hardware
        .lock()
        .unwrap()
        .events
        .contains(&Event::Control(150)));
    hardware.lock().unwrap().poll_busy = false;
    assert!(!stream.poll(&mut session, &render).unwrap());
    assert!(hardware
        .lock()
        .unwrap()
        .events
        .contains(&Event::Control(150)));
    assert!(!stream.poll(&mut session, &render).unwrap());
    assert_eq!(hardware.lock().unwrap().phase, 5);
    session.delay_report = Some(1290);
    session.delay_serial += 1;
    assert!(!stream.poll(&mut session, &render).unwrap());
    assert!(hardware
        .lock()
        .unwrap()
        .events
        .contains(&Event::Clock(9, 21)));
    hardware.lock().unwrap().phase = 3;
    assert!(!stream.poll(&mut session, &render).unwrap());
    std::thread::sleep(Duration::from_millis(501));
    hardware.lock().unwrap().reopen_delay = Some(0);
    assert!(stream.poll(&mut session, &render).unwrap());
    assert_eq!(session.flush_ms, 200);
    assert!(!mic_lifecycle::playback_allows_microphone(
        &ctx.state.lock().unwrap()
    ));
    session.start().unwrap();
    stream.started();
    assert!(mic_lifecycle::playback_allows_microphone(
        &ctx.state.lock().unwrap()
    ));
    assert!(!stream.poll(&mut session, &render).unwrap());

    ctx.capture_active.store(false, Ordering::Release);
    std::thread::sleep(Duration::from_millis(501));
    assert!(stream.poll(&mut session, &render).unwrap());
    assert_eq!(session.flush_ms, 60);
    stream
        .cleanup_before(&render, || session.restore_media_timeout())
        .unwrap();
}

#[test]
fn automatic_policy_reapplies_after_zero_report_on_pause() {
    for outcome in [
        "fresh_report",
        "zero_timeout",
        "restore_failed",
        "shorter_delay",
    ] {
        let hardware = Arc::new(Mutex::new(Hardware::default()));
        let ctx = Ctx::new(hardware.clone());
        ctx.state.lock().unwrap().model = "A3063".into();
        let render = render::Render(hardware.clone());
        let mut session = session::Session {
            gaming_supported: None,
            mode: codec::Mode::Eld,
            delay_serial: 1,
            delay_report: Some(0),
            delay_reporting: true,
            flush_ms: 200,
            hardware: hardware.clone(),
        };
        let mut stream = buffer::Stream::new(&ctx, 1, 7, driver::Driver(hardware.clone()));
        stream.started();
        std::thread::sleep(Duration::from_millis(501));
        assert!(!stream.poll(&mut session, &render).unwrap());
        assert!(hardware.lock().unwrap().events.is_empty());

        session.delay_report = Some(1500);
        session.delay_serial += 1;
        std::thread::sleep(Duration::from_millis(501));
        assert!(stream.poll(&mut session, &render).unwrap());
        assert_eq!(session.delay_report, None);
        session.start().unwrap();
        stream.started();
        std::thread::sleep(Duration::from_millis(501));
        assert!(!stream.poll(&mut session, &render).unwrap());
        session.delay_report = Some(1380);
        session.delay_serial += 1;
        assert!(!stream.poll(&mut session, &render).unwrap());
        assert!(hardware
            .lock()
            .unwrap()
            .events
            .contains(&Event::Clock(11, 68)));
        hardware.lock().unwrap().phase = 3;
        assert!(!stream.poll(&mut session, &render).unwrap());

        {
            let mut hardware = hardware.lock().unwrap();
            hardware.reopen_delay = Some(0);
            hardware.fail_reopen = outcome == "restore_failed";
        }
        let restored = stream.cleanup_before(&render, || session.restore_media_timeout());
        assert_eq!(restored.is_err(), outcome == "restore_failed");
        if outcome == "restore_failed" {
            assert!(ctx.state.lock().unwrap().playback_buffer_error.is_some());
            session.delay_report = Some(1500);
            session.start().unwrap();
            stream.started();
            std::thread::sleep(Duration::from_millis(501));
            assert!(!stream.poll(&mut session, &render).unwrap());
            assert_eq!(
                hardware
                    .lock()
                    .unwrap()
                    .events
                    .iter()
                    .filter(|event| **event == Event::Reopen(60))
                    .count(),
                1
            );
            continue;
        }
        assert_eq!(session.delay_report, Some(0));
        assert_eq!(session.flush_ms, 200);
        assert_eq!(ctx.state.lock().unwrap().playback_buffer_target_ms, None);
        assert_eq!(ctx.state.lock().unwrap().playback_buffer_error, None);
        session.start().unwrap();
        stream.started();
        std::thread::sleep(Duration::from_millis(501));
        if outcome == "shorter_delay" {
            session.delay_report = Some(600);
            session.delay_serial += 1;
            assert!(!stream.poll(&mut session, &render).unwrap());
            session.delay_report = Some(0);
            session.delay_serial += 1;
            std::thread::sleep(Duration::from_millis(501));
            assert!(!stream.poll(&mut session, &render).unwrap());
            assert_eq!(
                hardware
                    .lock()
                    .unwrap()
                    .events
                    .iter()
                    .filter(|event| **event == Event::Reopen(60))
                    .count(),
                1
            );
            continue;
        }

        assert!(stream.poll(&mut session, &render).unwrap());
        assert_eq!(session.flush_ms, 60);
        session.start().unwrap();
        stream.started();
        std::thread::sleep(Duration::from_millis(501));
        assert!(!stream.poll(&mut session, &render).unwrap());
        session.delay_report = Some(0);
        session.delay_serial += 1;
        assert!(!stream.poll(&mut session, &render).unwrap());
        assert_eq!(
            hardware
                .lock()
                .unwrap()
                .events
                .iter()
                .filter(|event| matches!(event, Event::Clock(_, _)))
                .count(),
            1
        );

        if outcome == "zero_timeout" {
            std::thread::sleep(Duration::from_millis(2001));
            assert!(stream.poll(&mut session, &render).unwrap());
            assert_eq!(session.flush_ms, 200);
            assert!(!mic_lifecycle::playback_allows_microphone(
                &ctx.state.lock().unwrap()
            ));
            session.start().unwrap();
            stream.started();
            assert_eq!(ctx.state.lock().unwrap().playback_buffer_target_ms, None);
            assert!(ctx.state.lock().unwrap().playback_buffer_error.is_some());
            assert!(mic_lifecycle::playback_allows_microphone(
                &ctx.state.lock().unwrap()
            ));
            session.delay_report = Some(1500);
            session.delay_serial += 1;
            std::thread::sleep(Duration::from_millis(501));
            assert!(!stream.poll(&mut session, &render).unwrap());
            assert_eq!(
                hardware
                    .lock()
                    .unwrap()
                    .events
                    .iter()
                    .filter(|event| **event == Event::Reopen(60))
                    .count(),
                2
            );
            assert_eq!(
                hardware
                    .lock()
                    .unwrap()
                    .events
                    .iter()
                    .filter(|event| matches!(event, Event::Clock(_, _)))
                    .count(),
                1
            );
        } else {
            session.delay_report = Some(1380);
            session.delay_serial += 1;
            assert!(!stream.poll(&mut session, &render).unwrap());
            assert_eq!(
                hardware
                    .lock()
                    .unwrap()
                    .events
                    .iter()
                    .filter(|event| **event == Event::Clock(11, 68))
                    .count(),
                2
            );
            hardware.lock().unwrap().phase = 3;
            assert!(!stream.poll(&mut session, &render).unwrap());
            stream
                .cleanup_before(&render, || session.restore_media_timeout())
                .unwrap();
        }
    }
}

#[test]
fn diagnostic_cancel_does_not_wait_or_allow_a_deferred_start() {
    let hardware = Arc::new(Mutex::new(Hardware::default()));
    let ctx = Ctx::new(hardware.clone());
    ctx.state.lock().unwrap().model = "A3064".into();
    let render = render::Render(hardware.clone());
    let mut session = session::Session {
        gaming_supported: None,
        mode: codec::Mode::Eld,
        delay_serial: 1,
        delay_report: Some(1500),
        delay_reporting: true,
        flush_ms: 200,
        hardware: hardware.clone(),
    };
    let mut stream = buffer::Stream::new(&ctx, 1, 7, driver::Driver(hardware.clone()));
    stream.started();
    std::thread::sleep(Duration::from_millis(501));
    assert!(stream.poll(&mut session, &render).unwrap());
    let pause = buffer::begin_automatic_pause(&ctx).unwrap().unwrap();
    assert!(pause.is_pending() && pause.is_current());
    buffer::set(&ctx, None, None, || true);
    assert!(pause.is_pending());
    assert!(!pause.is_current());
    assert!(!ctx.shutdown_cleanup_failed.load(Ordering::Acquire));
    stream
        .cleanup_before(&render, || session.restore_media_timeout())
        .unwrap();
    pause.wait().unwrap();
    buffer::set(&ctx, Some(60), Some(150), || pause.is_current());
    let state = ctx.state.lock().unwrap();
    assert_eq!(state.playback_buffer_target_ms, None);
    assert_eq!(state.playback_buffer_error, None);
    assert!(!hardware
        .lock()
        .unwrap()
        .events
        .contains(&Event::Control(60)));
}

#[test]
fn signaling_errors_prevent_target_and_restore_writes() {
    for (automatic, restoring) in [(false, false), (false, true), (true, false), (true, true)] {
        for kind in [
            io::ErrorKind::ConnectionAborted,
            io::ErrorKind::Interrupted,
            io::ErrorKind::NotConnected,
        ] {
            let hardware = Arc::new(Mutex::new(Hardware::default()));
            let ctx = Ctx::new(hardware.clone());
            if automatic {
                ctx.state.lock().unwrap().model = "A3064".into();
            }
            let render = render::Render(hardware.clone());
            let mut session = session::Session {
                gaming_supported: None,
                mode: codec::Mode::Eld,
                delay_serial: 1,
                delay_report: Some(1500),
                delay_reporting: true,
                flush_ms: 200,
                hardware: hardware.clone(),
            };
            let mut stream = buffer::Stream::new(&ctx, 1, 7, driver::Driver(hardware.clone()));
            stream.started();
            std::thread::sleep(Duration::from_millis(501));
            let target = 60;
            if !automatic {
                buffer::set(&ctx, Some(target), Some(150), || true);
            }
            assert!(stream.poll(&mut session, &render).unwrap());
            session.start().unwrap();
            stream.started();
            std::thread::sleep(Duration::from_millis(501));
            if restoring {
                assert!(!stream.poll(&mut session, &render).unwrap());
                buffer::set(&ctx, None, None, || true);
                std::thread::sleep(Duration::from_millis(501));
            }
            hardware.lock().unwrap().poll_error = Some(kind);
            assert_eq!(stream.poll(&mut session, &render).unwrap_err().kind(), kind);
            assert_eq!(
                ctx.state.lock().unwrap().playback_buffer_target_ms,
                Some(target)
            );
            {
                let hardware = hardware.lock().unwrap();
                assert_eq!(hardware.events.contains(&Event::Control(target)), restoring);
                assert!(!hardware.events.contains(&Event::Control(150)));
                assert!(!hardware.events.contains(&Event::Reopen(200)));
                assert!(!hardware
                    .events
                    .iter()
                    .any(|event| matches!(event, Event::Clock(_, _))));
            }
            ctx.state.lock().unwrap().playback_state = "stopping".into();
            stream.cleanup(&render).unwrap();
        }
    }
}

#[test]
fn automatic_errors_fall_back_once_or_stop_without_reconnect_loops() {
    for failure in ["control", "apply_reopen", "restore_reopen"] {
        let hardware = Arc::new(Mutex::new(Hardware {
            fail_control: Some(60),
            fail_apply_reopen: failure == "apply_reopen",
            fail_reopen: failure == "restore_reopen",
            ..Default::default()
        }));
        let ctx = Ctx::new(hardware.clone());
        ctx.state.lock().unwrap().model = "A3064".into();
        let render = render::Render(hardware.clone());
        let mut session = session::Session {
            gaming_supported: None,
            mode: codec::Mode::Eld,
            delay_serial: 1,
            delay_report: Some(1500),
            delay_reporting: true,
            flush_ms: 200,
            hardware: hardware.clone(),
        };
        let mut stream = buffer::Stream::new(&ctx, 1, 7, driver::Driver(hardware.clone()));
        stream.started();
        std::thread::sleep(Duration::from_millis(501));
        let first = stream.poll(&mut session, &render);
        if failure == "apply_reopen" {
            assert_eq!(first.unwrap_err().kind(), io::ErrorKind::ConnectionReset);
            assert_eq!(
                ctx.state.lock().unwrap().playback_buffer_target_ms,
                Some(60)
            );
            ctx.state.lock().unwrap().playback_state = "stopping".into();
            stream.cleanup(&render).unwrap();
            continue;
        }
        assert!(first.unwrap());
        session.start().unwrap();
        stream.started();
        std::thread::sleep(Duration::from_millis(501));
        let fallback = stream.poll(&mut session, &render);
        assert_eq!(
            ctx.state.lock().unwrap().playback_buffer_target_ms,
            Some(60)
        );
        assert!(!mic_lifecycle::playback_allows_microphone(
            &ctx.state.lock().unwrap()
        ));
        assert!(hardware
            .lock()
            .unwrap()
            .events
            .contains(&Event::Control(150)));
        if failure == "restore_reopen" {
            assert_eq!(fallback.unwrap_err().kind(), io::ErrorKind::Other);
            assert!(buffer::pause_automatic(&ctx).is_err());
            ctx.state.lock().unwrap().playback_state = "stopping".into();
            assert!(stream.cleanup(&render).is_err());
        } else {
            assert!(fallback.unwrap());
            assert_eq!(session.flush_ms, 200);
            session.start().unwrap();
            stream.started();
            assert!(mic_lifecycle::playback_allows_microphone(
                &ctx.state.lock().unwrap()
            ));
            assert!(ctx.state.lock().unwrap().playback_buffer_error.is_some());
            std::thread::sleep(Duration::from_millis(501));
            assert!(!stream.poll(&mut session, &render).unwrap());
            assert_eq!(
                hardware
                    .lock()
                    .unwrap()
                    .events
                    .iter()
                    .filter(|event| **event == Event::Reopen(60))
                    .count(),
                1
            );
            stream.cleanup(&render).unwrap();
        }
    }
}

#[test]
fn automatic_setup_reopens_before_start_and_does_not_restart_audible_playback() {
    for mode in [codec::Mode::Aac, codec::Mode::Eld] {
        let hardware = Arc::new(Mutex::new(Hardware::default()));
        let ctx = Ctx::new(hardware.clone());
        ctx.state.lock().unwrap().model = "A3064".into();
        let render = render::Render(hardware.clone());
        let mut session = session::Session {
            gaming_supported: None,
            mode,
            delay_serial: 1,
            delay_report: Some(1500),
            delay_reporting: true,
            flush_ms: 200,
            hardware: hardware.clone(),
        };
        let mut stream = buffer::Stream::new(&ctx, 1, 7, driver::Driver(hardware.clone()));
        for resumed in [false, true] {
            hardware.lock().unwrap().events.clear();
            if resumed {
                session.delay_report = Some(0);
            }
            stream.prepare(&mut session, &render).unwrap();
            assert_eq!(session.flush_ms, 60);
            assert!(hardware.lock().unwrap().events.contains(&Event::Reopen(60)));
            assert!(!hardware
                .lock()
                .unwrap()
                .events
                .contains(&Event::Control(60)));
            assert!(!mic_lifecycle::playback_allows_microphone(
                &ctx.state.lock().unwrap()
            ));
            session.start().unwrap();
            stream.started();
            assert!(!stream.poll(&mut session, &render).unwrap());
            assert!(!hardware
                .lock()
                .unwrap()
                .events
                .contains(&Event::Control(60)));
            std::thread::sleep(Duration::from_millis(501));
            assert!(!stream.poll(&mut session, &render).unwrap());
            assert!(hardware
                .lock()
                .unwrap()
                .events
                .contains(&Event::Control(60)));
            session.delay_serial += 1;
            session.delay_report = Some(1400);
            assert!(!stream.poll(&mut session, &render).unwrap());
            hardware.lock().unwrap().phase = 3;
            assert!(!stream.poll(&mut session, &render).unwrap());
            {
                let hardware = hardware.lock().unwrap();
                let transitions: Vec<_> = hardware
                    .events
                    .iter()
                    .filter(|event| matches!(event, Event::Reopen(_) | Event::Start))
                    .collect();
                assert_eq!(transitions, [&Event::Reopen(60), &Event::Start]);
            }
            stream
                .cleanup_before(&render, || session.restore_media_timeout())
                .unwrap();
            assert_eq!(session.flush_ms, 200);
            assert_eq!(ctx.state.lock().unwrap().playback_buffer_target_ms, None);
        }
    }
}

#[test]
fn automatic_policy_waits_for_microphone_demand_and_suppresses_unsupported_drivers() {
    let hardware = Arc::new(Mutex::new(Hardware {
        fail_preflight: true,
        ..Default::default()
    }));
    let ctx = Ctx::new(hardware.clone());
    ctx.state.lock().unwrap().model = "A3064".into();
    ctx.auto_mode.store(true, Ordering::Release);
    let render = render::Render(hardware.clone());
    let mut session = session::Session {
        gaming_supported: None,
        mode: codec::Mode::Eld,
        delay_serial: 1,
        delay_report: Some(1500),
        delay_reporting: true,
        flush_ms: 200,
        hardware: hardware.clone(),
    };
    let mut stream = buffer::Stream::new(&ctx, 1, 7, driver::Driver(hardware.clone()));
    stream.started();
    for mode in [ntpods_ipc::MicMode::Manual, ntpods_ipc::MicMode::KeepActive] {
        {
            let mut demand = ctx.mic_demand.lock().unwrap();
            demand.mode = mode;
            demand.manual_active = true;
        }
        std::thread::sleep(Duration::from_millis(501));
        assert!(!stream.poll(&mut session, &render).unwrap());
        assert!(!hardware.lock().unwrap().events.contains(&Event::Preflight));
    }
    ctx.auto_mode.store(false, Ordering::Release);
    std::thread::sleep(Duration::from_millis(501));
    assert!(!stream.poll(&mut session, &render).unwrap());
    assert_eq!(ctx.state.lock().unwrap().playback_buffer_target_ms, None);
    assert!(ctx.state.lock().unwrap().playback_buffer_error.is_some());
    std::thread::sleep(Duration::from_millis(501));
    assert!(!stream.poll(&mut session, &render).unwrap());
    let events = &hardware.lock().unwrap().events;
    assert_eq!(
        events
            .iter()
            .filter(|event| **event == Event::Preflight)
            .count(),
        1
    );
    assert!(!events
        .iter()
        .any(|event| matches!(event, Event::Control(_) | Event::Reopen(_))));
}
