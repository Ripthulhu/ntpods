//! Production USB and health workers with an in-memory HID endpoint and media player.
#![allow(dead_code)]

use ntpods_ipc::{Event, Snapshot};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

#[path = "../src/aap.rs"]
mod aap;
#[path = "../src/atomic_file.rs"]
mod atomic_file;
#[path = "../src/att_settings.rs"]
mod att_settings;
#[path = "../src/battery_ble.rs"]
mod battery_ble;
#[path = "../src/device_models.rs"]
mod device_models;
#[path = "../src/device_settings.rs"]
mod device_settings;
#[path = "../src/dongle.rs"]
mod dongle;
#[path = "../src/dongle_features.rs"]
mod dongle_features;
#[path = "../src/dongle_health.rs"]
mod dongle_health;
#[path = "../src/dongle_protocol.rs"]
mod dongle_protocol;
#[path = "../src/dongle_rpc.rs"]
mod dongle_rpc;
#[path = "../src/hardware_check.rs"]
mod hardware_check;
#[path = "../src/hardware_diagnostics.rs"]
mod hardware_diagnostics;
#[path = "../src/hearing.rs"]
mod hearing;
#[path = "../src/hearing_diagnostics.rs"]
mod hearing_diagnostics;
#[path = "../src/hearing_metadata.rs"]
mod hearing_metadata;
#[path = "../src/transparency.rs"]
mod transparency;

fn log(_: &str) {}

#[derive(Clone)]
struct Ctx {
    dongle: Arc<dongle::Controller>,
    dongle_health: Arc<dongle_health::Controller>,
    companion_actions: Arc<Mutex<()>>,
    state: Arc<Mutex<Snapshot>>,
    auto_pause: Arc<Mutex<media::AutoPausePolicy>>,
    shutting_down: Arc<AtomicBool>,
}

impl Ctx {
    fn push_state(&self) {}
    fn send_event(&self, _: &Event) {}
}

mod driver {
    pub struct Driver;
    impl Driver {
        pub fn att_send(&self, _: &[u8]) -> std::io::Result<()> {
            unreachable!()
        }
        pub fn att_recv(&self, _: u32, _: &mut [u8]) -> std::io::Result<usize> {
            unreachable!()
        }
        pub fn send(&self, _: &[u8]) -> std::io::Result<()> {
            unreachable!()
        }
    }
}

mod diagnostic_controls {
    pub fn in_case(_: [crate::aap::EarStatus; 2]) -> Option<bool> {
        None
    }
}

mod companion {
    pub fn local_override(_: &super::Ctx, _: &ntpods_ipc::Command) {}
}

mod media {
    use super::*;
    pub struct AutoPausePolicy;
    impl AutoPausePolicy {
        pub fn permission(&self, route: bool) -> Option<u64> {
            route.then_some(1)
        }
    }
    pub fn init() {}
    #[derive(Clone, Copy)]
    pub enum PauseReceipt {
        Paused,
        Quiet,
        Failed,
        Resumed(bool),
    }
    pub struct SelectedPause {
        cancelled: Arc<AtomicBool>,
    }
    impl SelectedPause {
        pub fn start_dongle() -> Self {
            PLAYER.lock().unwrap().pauses += 1;
            Self {
                cancelled: Arc::default(),
            }
        }
        pub fn resume(&mut self, permitted: impl FnOnce() -> bool + Send + 'static) -> bool {
            PLAYER
                .lock()
                .unwrap()
                .resumes
                .push((self.cancelled.clone(), Box::new(permitted)));
            true
        }
        pub fn poll(&self) -> Option<PauseReceipt> {
            None
        }
    }
    impl Drop for SelectedPause {
        fn drop(&mut self) {
            self.cancelled.store(true, Ordering::Release);
        }
    }
}

#[derive(Default)]
struct Player {
    pauses: usize,
    resumes: Vec<(Arc<AtomicBool>, Box<dyn FnOnce() -> bool + Send>)>,
}
static PLAYER: Mutex<Player> = Mutex::new(Player {
    pauses: 0,
    resumes: Vec::new(),
});
static TEST_GATE: Mutex<()> = Mutex::new(());
static USB: Mutex<Option<Arc<Mutex<Hardware>>>> = Mutex::new(None);

struct Hardware {
    generation: u32,
    usb_playback: bool,
    input: u8,
    audio_error: bool,
    stop_status_error: bool,
    ears: [u8; 2],
    revision: u32,
    ear_reads: usize,
    features: Vec<u8>,
    finder: bool,
    reply: [u8; 64],
    request: Vec<u8>,
    op: u8,
    commit_block: Option<Arc<CommitBlock>>,
    sequences: Vec<u32>,
}

#[derive(Default)]
struct CommitBlock {
    entered: AtomicBool,
    release: AtomicBool,
}

impl Hardware {
    fn new() -> Self {
        let mut reply = [0; 64];
        reply[..7].copy_from_slice(&[3, b'A', b'P', b'D', b'G', 1, 64]);
        reply[8..12].copy_from_slice(&1u32.to_le_bytes());
        Self {
            generation: 1,
            usb_playback: true,
            input: 1,
            audio_error: false,
            stop_status_error: false,
            ears: [0, 0],
            revision: 1,
            ear_reads: 0,
            features: Vec::new(),
            finder: false,
            reply,
            request: Vec::new(),
            op: 0,
            commit_block: None,
            sequences: Vec::new(),
        }
    }
    fn response(&mut self) -> Vec<u8> {
        if self.op == 3 {
            if self.request == [5] {
                self.ear_reads += 1;
                return [
                    0u32.to_le_bytes().as_slice(),
                    self.revision.to_le_bytes().as_slice(),
                    &[4, 0, 4, 0, 6, 0, self.ears[0], self.ears[1]],
                ]
                .concat();
            }
            return Vec::new();
        }
        let feature = self.request[0];
        self.features.push(feature);
        match feature {
            2 => vec![1, 0],
            6 | 7 => {
                if feature == 6 {
                    self.finder = true;
                }
                let mut bytes = vec![0; 24];
                bytes[0] = 1;
                bytes[1] = 0x80 | u8::from(self.finder);
                bytes[2] = 127;
                bytes[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
                bytes[20..24].copy_from_slice(&i32::MIN.to_le_bytes());
                bytes
            }
            8 => {
                self.finder = false;
                Vec::new()
            }
            9 => {
                let mut bytes = vec![0; 20];
                bytes[0] = 1;
                bytes[14] = self.input;
                bytes[16..18].copy_from_slice(&u16::MAX.to_le_bytes());
                bytes[18..20].copy_from_slice(&100u16.to_le_bytes());
                bytes
            }
            _ => panic!("Unexpected feature {feature}"),
        }
    }
}

mod dongle_hid {
    use super::*;
    pub struct Device {
        pub path: Vec<u16>,
        hardware: Arc<Mutex<Hardware>>,
    }
    pub enum Discovery {
        None,
        Multiple,
        One(Device),
    }
    pub fn discover(_: Option<&[u16]>) -> std::io::Result<Discovery> {
        Ok(Discovery::One(Device {
            path: vec![1],
            hardware: USB.lock().unwrap().as_ref().unwrap().clone(),
        }))
    }
    impl Device {
        pub fn read(&self) -> std::io::Result<dongle_protocol::Report> {
            let h = self.hardware.lock().unwrap();
            Ok(dongle_protocol::Report {
                state: ntpods_ipc::DongleState {
                    present: true,
                    connected: true,
                    controls_ready: true,
                    selected: true,
                    usb_playback: h.usb_playback,
                    generation: h.generation,
                    capabilities: 24,
                    ..Default::default()
                },
                address: [1, 2, 3, 4, 5, 6],
                sequence: 0,
                opcode: 0,
                result: 0,
            })
        }
        pub fn read_feature(&self, id: u8) -> std::io::Result<[u8; 64]> {
            assert_eq!(id, 3);
            let mut h = self.hardware.lock().unwrap();
            if h.stop_status_error
                && h.op == 7
                && h.request == [7]
                && h.reply[21] == 3
                && h.features.last() == Some(&7)
                && h.features.contains(&8)
            {
                h.stop_status_error = false;
                return Err(std::io::Error::other("Simulated post-Stop status error"));
            }
            if h.audio_error && h.op == 7 && h.request == [9] && h.reply[21] == 3 {
                return Err(std::io::Error::other("Simulated USB error"));
            }
            Ok(h.reply)
        }
        pub fn write(&self, bytes: [u8; 64]) -> std::io::Result<()> {
            let mut h = self.hardware.lock().unwrap();
            assert_eq!(bytes[0], 3);
            let op = bytes[7];
            match op {
                3 | 7 => {
                    h.op = op;
                    h.request = bytes[22..22 + usize::from(bytes[20])].to_vec();
                    h.reply = bytes;
                    h.reply[18..20].copy_from_slice(&bytes[16..18]);
                    h.reply[20] = 0;
                    h.reply[21] = 1;
                    h.reply[22..].fill(0);
                }
                6 => {
                    h.sequences
                        .push(u32::from_le_bytes(bytes[12..16].try_into().unwrap()));
                    let response = h.response();
                    assert!(response.len() <= 42);
                    h.reply[16..18].copy_from_slice(&(response.len() as u16).to_le_bytes());
                    h.reply[18..20].fill(0);
                    h.reply[20] = response.len() as u8;
                    h.reply[21] = 3;
                    h.reply[22..].fill(0);
                    h.reply[22..22 + response.len()].copy_from_slice(&response);
                }
                5 => {
                    h.reply[21] = 0;
                }
                _ => panic!("Unexpected RPC operation {op}"),
            }
            let block = if op == 6 { h.commit_block.take() } else { None };
            drop(h);
            if let Some(block) = block {
                block.entered.store(true, Ordering::Release);
                until(|| block.release.load(Ordering::Acquire));
            }
            Ok(())
        }
    }
}

struct Fixture {
    ctx: Ctx,
    hardware: Arc<Mutex<Hardware>>,
    _gate: MutexGuard<'static, ()>,
}
impl Fixture {
    fn new() -> Self {
        Self::start(true)
    }
    fn start(health_worker: bool) -> Self {
        let gate = TEST_GATE.lock().unwrap();
        *PLAYER.lock().unwrap() = Player::default();
        let hardware = Arc::new(Mutex::new(Hardware::new()));
        *USB.lock().unwrap() = Some(hardware.clone());
        let ctx = Ctx {
            dongle: Arc::default(),
            dongle_health: Arc::default(),
            companion_actions: Arc::default(),
            state: Arc::new(Mutex::new(Snapshot::default())),
            auto_pause: Arc::new(Mutex::new(media::AutoPausePolicy)),
            shutting_down: Arc::default(),
        };
        dongle::start(ctx.clone());
        if health_worker {
            dongle_health::start(ctx.clone());
        }
        let fixture = Self {
            ctx,
            hardware,
            _gate: gate,
        };
        if health_worker {
            until(|| fixture.hardware.lock().unwrap().ear_reads >= 2);
        }
        fixture
    }
    fn pause(&self) {
        let mut h = self.hardware.lock().unwrap();
        h.ears = [1, 0];
        h.revision += 1;
        drop(h);
        until(|| PLAYER.lock().unwrap().pauses == 1);
        self.hardware.lock().unwrap().usb_playback = false;
        until(|| !self.ctx.state.lock().unwrap().dongle.usb_playback);
    }
    fn reinsert(&self) {
        let mut h = self.hardware.lock().unwrap();
        h.ears = [0, 0];
        h.revision += 1;
    }
    fn audio_queries(&self) -> usize {
        self.hardware
            .lock()
            .unwrap()
            .features
            .iter()
            .filter(|feature| **feature == 9)
            .count()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.ctx.shutting_down.store(true, Ordering::Release);
        let d = self.ctx.state.lock().unwrap().dongle.clone();
        until(|| dongle::research_device(&self.ctx, d.session, d.generation).is_none());
        *USB.lock().unwrap() = None;
    }
}
fn until(mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "Worker did not reach the expected state"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn regression_phone_takes_input_before_the_scheduled_audio_refresh() {
    let fixture = Fixture::new();
    let started = Instant::now();
    fixture.pause();
    assert_eq!(
        fixture
            .ctx
            .state
            .lock()
            .unwrap()
            .dongle
            .health
            .audio
            .as_ref()
            .unwrap()
            .input,
        1
    );
    let before = fixture.audio_queries();
    fixture.hardware.lock().unwrap().input = 2;
    fixture.reinsert();
    until(|| {
        fixture.audio_queries() > before
            && fixture
                .ctx
                .state
                .lock()
                .unwrap()
                .dongle
                .health
                .audio
                .as_ref()
                .is_some_and(|audio| audio.input == 2)
    });
    assert!(started.elapsed() < Duration::from_millis(1500));
    assert!(PLAYER.lock().unwrap().resumes.is_empty());
}

#[test]
fn regression_async_resume_rechecks_ownership_after_it_was_queued() {
    let fixture = Fixture::new();
    fixture.pause();
    fixture.hardware.lock().unwrap().input = 0;
    fixture.reinsert();
    until(|| !PLAYER.lock().unwrap().resumes.is_empty());
    let (_, permission) = PLAYER.lock().unwrap().resumes.pop().unwrap();
    assert_ne!(
        fixture
            .ctx
            .state
            .lock()
            .unwrap()
            .dongle
            .health
            .audio
            .as_ref()
            .unwrap()
            .input,
        2
    );
    fixture.hardware.lock().unwrap().input = 2;
    let before = fixture.audio_queries();
    assert!(!permission());
    assert!(fixture.audio_queries() > before);
}

#[test]
fn regression_async_resume_rejects_a_new_usb_generation() {
    let fixture = Fixture::new();
    fixture.pause();
    fixture.reinsert();
    until(|| !PLAYER.lock().unwrap().resumes.is_empty());
    let (_, permission) = PLAYER.lock().unwrap().resumes.pop().unwrap();
    fixture.ctx.state.lock().unwrap().dongle.generation += 1;
    fixture.hardware.lock().unwrap().generation += 1;
    assert!(!permission());
}

#[test]
fn regression_media_permission_and_health_transactions_share_one_usb_exchange() {
    let fixture = Fixture::start(false);
    let d = fixture.ctx.state.lock().unwrap().dongle.clone();
    let mut media =
        dongle_rpc::Session::open(fixture.ctx.clone(), d.session, d.generation).unwrap();
    let mut health =
        dongle_rpc::Session::open(fixture.ctx.clone(), d.session, d.generation).unwrap();
    let block = Arc::new(CommitBlock::default());
    fixture.hardware.lock().unwrap().commit_block = Some(block.clone());
    let first = std::thread::spawn(move || media.feature(&[9]));
    until(|| block.entered.load(Ordering::Acquire));
    let entered = Arc::new(AtomicBool::new(false));
    let second_entered = entered.clone();
    let second = std::thread::spawn(move || {
        second_entered.store(true, Ordering::Release);
        health.feature(&[2])
    });
    until(|| entered.load(Ordering::Acquire));
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(fixture.hardware.lock().unwrap().features, [9]);
    block.release.store(true, Ordering::Release);
    assert_eq!(first.join().unwrap().unwrap().len(), 20);
    assert_eq!(second.join().unwrap().unwrap(), [1, 0]);
    let h = fixture.hardware.lock().unwrap();
    assert_eq!(h.features, [9, 2]);
    assert_eq!(h.sequences, [1, 2]);
}

#[test]
fn regression_uncertain_ownership_revokes_the_retained_pause() {
    for usb_error in [false, true] {
        let fixture = Fixture::new();
        fixture.pause();
        let before = fixture.audio_queries();
        {
            let mut h = fixture.hardware.lock().unwrap();
            h.input = 3;
            h.audio_error = usb_error;
        }
        fixture.reinsert();
        until(|| {
            fixture.audio_queries() > before
                && fixture
                    .ctx
                    .state
                    .lock()
                    .unwrap()
                    .dongle
                    .health
                    .audio
                    .is_none()
        });
        assert!(PLAYER.lock().unwrap().resumes.is_empty());
    }
}

#[test]
fn regression_usb_shutdown_stops_finder_and_keeps_normal_commands_blocked() {
    let fixture = Fixture::new();
    let d = fixture.ctx.state.lock().unwrap().dongle.clone();
    let mut normal =
        dongle_rpc::Session::open(fixture.ctx.clone(), d.session, d.generation).unwrap();
    let mut local =
        dongle_rpc::Session::open_local(fixture.ctx.clone(), d.session, d.generation).unwrap();
    let finder = format!("dongle-finder:{}:{}", d.session, d.pair_token);
    assert!(dongle_health::start_finder(&fixture.ctx, &finder).is_some());
    until(|| {
        fixture
            .ctx
            .state
            .lock()
            .unwrap()
            .dongle
            .health
            .nearby_finder
            .active
    });
    fixture.ctx.shutting_down.store(true, Ordering::Release);
    assert!(!normal.current());
    assert!(normal.feature(&[9]).is_err());
    assert!(normal.send_aacp(&[4, 0, 4, 0, 9]).is_err());
    assert!(local.feature(&[6]).is_err());
    assert!(dongle_rpc::Session::open_local(fixture.ctx.clone(), d.session, d.generation).is_err());
    until(|| fixture.hardware.lock().unwrap().features.contains(&8));
    assert!(!fixture.hardware.lock().unwrap().finder);
}

#[test]
fn regression_old_stop_status_failure_cannot_stop_a_new_finder_owner() {
    let fixture = Fixture::new();
    let d = fixture.ctx.state.lock().unwrap().dongle.clone();
    let finder = format!("dongle-finder:{}:{}", d.session, d.pair_token);
    let first = dongle_health::start_finder(&fixture.ctx, &finder).unwrap();
    until(|| {
        fixture
            .ctx
            .state
            .lock()
            .unwrap()
            .dongle
            .health
            .nearby_finder
            .active
    });
    let transaction = fixture.ctx.dongle.rpc.lock().unwrap();
    fixture.hardware.lock().unwrap().stop_status_error = true;
    assert!(dongle_health::stop_finder(&fixture.ctx, &finder, first));
    let second = dongle_health::start_finder(&fixture.ctx, &finder).unwrap();
    assert_ne!(first, second);
    drop(transaction);
    until(|| {
        fixture
            .hardware
            .lock()
            .unwrap()
            .features
            .iter()
            .filter(|op| **op == 6)
            .count()
            == 2
    });
    let reads = fixture.hardware.lock().unwrap().ear_reads;
    until(|| fixture.hardware.lock().unwrap().ear_reads >= reads + 2);
    let h = fixture.hardware.lock().unwrap();
    assert!(!h.stop_status_error);
    assert_eq!(h.features.iter().filter(|op| **op == 8).count(), 1);
    assert!(h.finder);
    drop(h);
    assert!(dongle_health::stop_finder(&fixture.ctx, &finder, second));
}

#[test]
fn regression_shutdown_stop_does_not_wait_indefinitely_for_another_transaction() {
    let fixture = Fixture::start(false);
    let d = fixture.ctx.state.lock().unwrap().dongle.clone();
    let usb = dongle::research_device(&fixture.ctx, d.session, d.generation).unwrap();
    let transaction = fixture.ctx.dongle.rpc.lock().unwrap();
    fixture.ctx.shutting_down.store(true, Ordering::Release);
    let started = Instant::now();
    assert!(dongle_rpc::Session::stop_finder_at_shutdown(
        fixture.ctx.clone(),
        usb,
        d.session,
        d.generation,
    )
    .unwrap_err()
    .contains("timed out"));
    assert!(started.elapsed() < Duration::from_millis(1500));
    assert!(!fixture.hardware.lock().unwrap().features.contains(&8));
    drop(transaction);
}
