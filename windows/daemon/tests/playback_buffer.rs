//! Exercise the production trial owner without Windows devices or Bluetooth.
#![cfg(feature = "experimental-playback")]
#![allow(dead_code)]

use ntpods_ipc::Snapshot;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[path = "../src/playback/buffer.rs"]
mod buffer;
#[path = "../src/mic_lifecycle.rs"]
mod mic_lifecycle;
#[path = "../src/playback/selection.rs"]
mod selection;

#[derive(Debug, PartialEq)]
enum Event {
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
}

type SharedHardware = Arc<Mutex<Hardware>>;

#[derive(Clone)]
struct Ctx {
    state: Arc<Mutex<Snapshot>>,
    playback_buffer: Arc<Mutex<Option<Arc<buffer::Active>>>>,
    mic_preference: Arc<Mutex<()>>,
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
            self.0
                .lock()
                .unwrap()
                .events
                .push(Event::Control(u16::from_le_bytes(
                    data[7..9].try_into().unwrap(),
                )));
            Ok(())
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

mod session {
    use super::*;

    pub struct Session {
        pub delay_serial: u64,
        pub delay_report: Option<u16>,
        pub hardware: SharedHardware,
    }

    impl Session {
        pub fn check_flush_support(&self) -> io::Result<()> {
            Ok(())
        }

        pub fn reopen_media(&mut self, timeout: u16) -> io::Result<()> {
            let mut hardware = self.hardware.lock().unwrap();
            hardware.events.push(Event::Reopen(timeout));
            if timeout == 200 && hardware.fail_reopen {
                Err(io::Error::other("restore reopen failed"))
            } else {
                Ok(())
            }
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
            delay_serial: 1,
            delay_report: Some(1500),
            hardware: hardware.clone(),
        };
        let mut stream = buffer::Stream::new(&ctx, 1, 7, driver::Driver(hardware.clone()));
        stream.started();
        std::thread::sleep(Duration::from_millis(501));
        buffer::set(&ctx, Some(60), Some(150));
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
