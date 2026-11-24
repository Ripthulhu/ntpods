//! Production USB and health workers with an in-memory HID endpoint and media player.
#![allow(dead_code)]

use ntpods_ipc::{Event, Snapshot};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

#[path = "../src/aap.rs"]
mod aap;
#[path = "../src/accommodations.rs"]
mod accommodations;
#[path = "../src/accommodations_control.rs"]
mod accommodations_control;
#[path = "../src/atomic_file.rs"]
mod atomic_file;
#[path = "../src/att_settings.rs"]
mod att_settings;
#[path = "../src/battery_ble.rs"]
mod battery_ble;
#[path = "../src/case_return.rs"]
mod case_return;
#[path = "../src/connection_request.rs"]
mod connection_request;
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
    companion_preferences: Arc<Mutex<companion_prefs::Store>>,
    dongle: Arc<dongle::Controller>,
    dongle_health: Arc<dongle_health::Controller>,
    companion_actions: Arc<Mutex<()>>,
    case_return: Arc<Mutex<case_return::CaseReturn>>,
    connect_requested: Arc<connection_request::ConnectionRequest>,
    mac: Arc<AtomicU64>,
    state: Arc<Mutex<Snapshot>>,
    auto_pause: Arc<Mutex<media::AutoPausePolicy>>,
    shutting_down: Arc<AtomicBool>,
}

impl Ctx {
    fn for_test() -> Self {
        Self {
            companion_preferences: Arc::default(),
            dongle: Arc::default(),
            dongle_health: Arc::default(),
            companion_actions: Arc::default(),
            case_return: Arc::default(),
            connect_requested: Arc::new(connection_request::ConnectionRequest::new(false)),
            mac: Arc::default(),
            state: Arc::new(Mutex::new(Snapshot::default())),
            auto_pause: Arc::new(Mutex::new(media::AutoPausePolicy)),
            shutting_down: Arc::default(),
        }
    }
    fn mac(&self) -> u64 {
        self.mac.load(Ordering::Acquire)
    }
    fn push_state(&self) {}
    fn send_event(&self, _: &Event) {}
}

mod companion_prefs {
    #[derive(Default)]
    pub struct Store { selected: Option<String> }
    impl Store {
        pub fn selected_dongle(&self) -> Option<&str> { self.selected.as_deref() }
        pub fn set_selected_dongle(&mut self, serial: &str) -> std::io::Result<()> {
            self.selected = Some(serial.into()); Ok(())
        }
    }
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
    path: u16,
    usb_audio_available: bool,
    connected: bool,
    generation: u32,
    capabilities: u16,
    usb_playback: bool,
    input: u8,
    audio_error: bool,
    stop_status_error: bool,
    ears: [u8; 2],
    revision: u32,
    ear_reads: usize,
    features: Vec<u8>,
    reconnect_automatically: bool,
    finder: bool,
    reply: [u8; 64],
    request: Vec<u8>,
    response: Vec<u8>,
    metadata: Option<Vec<u8>>,
    transparency: Option<Vec<u8>>,
    clinical: Option<Vec<u8>>,
    cached: std::collections::BTreeMap<u8, Vec<u8>>,
    op: u8,
    commit_block: Option<Arc<CommitBlock>>,
    sequences: Vec<u32>,
    commits: Vec<(u8, Vec<u8>)>,
    lease: Option<u32>,
    lease_calls: Vec<(u8, u32)>,
    lease_error_action: Option<u8>,
    lease_bad_reply: bool,
    raw_requests: Vec<(u8, Vec<u8>)>,
    raw_error: bool,
    failure: u8,
    cancel_delay: u8,
    cancel_pending: bool,
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
            path: 1,
            usb_audio_available: true,
            connected: true,
            generation: 1,
            capabilities: 56,
            usb_playback: true,
            input: 1,
            audio_error: false,
            stop_status_error: false,
            ears: [0, 0],
            revision: 1,
            ear_reads: 0,
            features: Vec::new(),
            reconnect_automatically: false,
            finder: false,
            reply,
            request: Vec::new(),
            response: Vec::new(),
            metadata: None,
            transparency: None,
            clinical: None,
            cached: Default::default(),
            op: 0,
            commit_block: None,
            sequences: Vec::new(),
            commits: Vec::new(),
            lease: None,
            lease_calls: Vec::new(),
            lease_error_action: None,
            lease_bad_reply: false,
            raw_requests: Vec::new(),
            raw_error: false,
            failure: 0,
            cancel_delay: 0,
            cancel_pending: false,
        }
    }
    fn response(&mut self) -> Vec<u8> {
        if matches!(self.op, 1 | 2) {
            assert!(
                self.lease.is_some(),
                "Raw requests require the settings lease"
            );
            self.raw_requests.push((self.op, self.request.clone()));
            if self.raw_error {
                self.raw_error = false;
                self.failure = 6;
                return Vec::new();
            }
            if self.op == 1 {
                if self.transparency.is_some() {
                    return match self.request[0] {
                        0x06 => vec![0x07, 0x30, 0, 0x40, 0],
                        0x08 if self.request[1] == 0x30 => {
                            let mut response = vec![0x09, 21, 0x34, 0, 0x0a, 0x35, 0];
                            response.extend_from_slice(&[0x90, 0x53, 0x4d, 0x74, 0xaf, 0x9e, 0x03, 0x94,
                                0xdb, 0x45, 0xab, 0xf7, 0xc1, 0x1c, 0x62, 0xd5]);
                            if self.clinical.is_some() {
                                response.extend_from_slice(&[0x38, 0, 0x0a, 0x39, 0]);
                                response.extend_from_slice(&[0x0a, 0x7e, 0x45, 0x41, 0x0b, 0x0f, 0x98, 0x90,
                                    0x6f, 0x4d, 0xc5, 0x95, 5, 0, 0x12, 0xa4]);
                            }
                            response
                        }
                        0x08 => vec![1, 8, self.request[1], self.request[2], 0x0a],
                        0x0a => [vec![0x0b], if self.request[1] == 0x35 {
                            self.transparency.clone().unwrap() } else { self.clinical.clone().unwrap() }].concat(),
                        0x12 => {
                            assert_eq!(self.request[1], 0x35, "Clinical profile writes are forbidden");
                            self.transparency = Some(self.request[3..].to_vec()); vec![0x13]
                        },
                        _ => panic!("Unexpected Custom Transparency request"),
                    };
                }
            }
            return if self.op == 1 {
                vec![0x0b, 7]
            } else {
                Vec::new()
            };
        }
        if self.op == 3 {
            if let Some(packet) = self.cached.get(&self.request[0]) {
                return [0u32.to_le_bytes().as_slice(), self.revision.to_le_bytes().as_slice(), packet].concat();
            }
            if self.request == [1] {
                if let Some(packet) = self.metadata.as_ref() {
                    return [0u32.to_le_bytes().as_slice(), 1u32.to_le_bytes().as_slice(), packet].concat();
                }
            }
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
            10 => {
                assert_eq!(self.request.len(), 6);
                let action = self.request[1];
                let token = u32::from_le_bytes(self.request[2..].try_into().unwrap());
                self.lease_calls.push((action, token));
                if self.lease_error_action == Some(action) {
                    self.lease_error_action = None;
                    self.lease = None;
                    self.failure = 3;
                    return Vec::new();
                }
                match action {
                    0 => {
                        assert_eq!(token, 0);
                        assert!(
                            self.lease.is_none(),
                            "A coordinator must not reacquire its lease"
                        );
                        self.lease = Some(0x7654_3210);
                        if self.lease_bad_reply {
                            return vec![1, 0, 0, 0, 0];
                        }
                        [vec![1], self.lease.unwrap().to_le_bytes().to_vec()].concat()
                    }
                    1 | 2 => {
                        if self.lease != Some(token) {
                            self.failure = 3;
                        } else if action == 2 {
                            self.lease = None;
                        }
                        Vec::new()
                    }
                    _ => panic!("Unexpected lease action"),
                }
            }
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
            23 => {
                match self.request.as_slice() {
                    [23] => {}
                    [23, value @ (0 | 1)] => self.reconnect_automatically = *value == 1,
                    _ => panic!("Unexpected reconnect policy request"),
                }
                vec![1, u8::from(self.reconnect_automatically)]
            }
            _ => panic!("Unexpected feature {feature}"),
        }
    }
    fn cancelled(&mut self) {
        self.reply[16..18].copy_from_slice(&1u16.to_le_bytes());
        self.reply[18..20].fill(0);
        self.reply[20] = 1;
        self.reply[21] = 4;
        self.reply[22..].fill(0);
        self.reply[22] = 8;
        self.cancel_pending = false;
    }
}

mod dongle_hid {
    use super::*;
    pub struct Device {
        pub path: Vec<u16>,
        pub serial: String,
        pub usb_audio_available: bool,
        hardware: Arc<Mutex<Hardware>>,
    }
    pub fn discover(_: Option<&[u16]>) -> std::io::Result<Vec<Device>> {
        let hardware = USB.lock().unwrap().as_ref().unwrap().clone();
        let h = hardware.lock().unwrap();
        Ok(vec![Device {
            path: vec![h.path],
            serial: "test-board".into(),
            usb_audio_available: h.usb_audio_available,
            hardware: hardware.clone(),
        }])
    }
    impl Device {
        pub fn read(&self) -> std::io::Result<dongle_protocol::Report> {
            let h = self.hardware.lock().unwrap();
            if self.path != [h.path] {
                return Err(std::io::Error::new(std::io::ErrorKind::NotConnected, "USB re-enumerated"));
            }
            Ok(dongle_protocol::Report {
                state: ntpods_ipc::DongleState {
                    present: true,
                    connected: h.connected,
                    controls_ready: h.connected,
                    selected: true,
                    usb_playback: h.usb_playback,
                    generation: h.generation,
                    capabilities: h.capabilities,
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
            if h.cancel_pending {
                if h.cancel_delay == 0 {
                    h.cancelled();
                } else {
                    h.cancel_delay -= 1;
                }
            }
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
            assert!(
                !h.cancel_pending,
                "A queued Cancel must finish before the next request"
            );
            match op {
                1 | 2 | 3 | 7 => {
                    h.op = op;
                    h.failure = 0;
                    let offset = usize::from(u16::from_le_bytes(bytes[18..20].try_into().unwrap()));
                    if offset == 0 { h.request.clear(); }
                    assert_eq!(offset, h.request.len());
                    h.request.extend_from_slice(&bytes[22..22 + usize::from(bytes[20])]);
                    h.reply = bytes;
                    let received = h.request.len() as u16;
                    h.reply[18..20].copy_from_slice(&received.to_le_bytes());
                    h.reply[20] = 0;
                    h.reply[21] = 1;
                    h.reply[22..].fill(0);
                }
                6 => {
                    h.sequences
                        .push(u32::from_le_bytes(bytes[12..16].try_into().unwrap()));
                    let request = (h.op, h.request.clone());
                    h.commits.push(request);
                    let response = h.response();
                    h.response = response.clone();
                    h.reply[16..18].copy_from_slice(&(response.len() as u16).to_le_bytes());
                    h.reply[18..20].fill(0);
                    let count = response.len().min(42);
                    h.reply[20] = count as u8;
                    h.reply[21] = 3;
                    h.reply[22..].fill(0);
                    h.reply[22..22 + count].copy_from_slice(&response[..count]);
                    if h.failure != 0 {
                        h.reply[16..18].copy_from_slice(&1u16.to_le_bytes());
                        h.reply[20] = 1;
                        h.reply[21] = 4;
                        h.reply[22] = h.failure;
                    }
                }
                4 => {
                    let offset = usize::from(u16::from_le_bytes(bytes[18..20].try_into().unwrap()));
                    let page = h.response[offset..h.response.len().min(offset + 42)].to_vec();
                    h.reply[18..20].copy_from_slice(&bytes[18..20]);
                    h.reply[20] = page.len() as u8;
                    h.reply[22..].fill(0);
                    h.reply[22..22 + page.len()].copy_from_slice(&page);
                }
                5 => {
                    if h.cancel_delay == 0 {
                        h.cancelled();
                    } else {
                        h.cancel_pending = true;
                    }
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
        Self::start_config(health_worker, |_| {})
    }
    fn start_config(health_worker: bool, configure: impl FnOnce(&mut Hardware)) -> Self {
        let gate = TEST_GATE.lock().unwrap();
        *PLAYER.lock().unwrap() = Player::default();
        let hardware = Arc::new(Mutex::new(Hardware::new()));
        configure(&mut hardware.lock().unwrap());
        *USB.lock().unwrap() = Some(hardware.clone());
        let ctx = Ctx::for_test();
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
        self.wear([0, 0]);
    }
    fn wear(&self, statuses: [u8; 2]) {
        let mut h = self.hardware.lock().unwrap();
        h.ears = statuses;
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
fn control_only_board_reads_and_changes_its_reconnect_policy_without_airpods() {
    use ntpods_ipc::Command;
    let fixture = Fixture::start_config(false, |h| {
        h.connected = false;
        h.usb_audio_available = false;
        h.usb_playback = false;
    });
    dongle_health::start(fixture.ctx.clone());
    until(|| fixture.ctx.state.lock().unwrap().dongle.health.reconnect_automatically == Some(false));
    let d = fixture.ctx.state.lock().unwrap().dongle.clone();
    let set = Command::DongleHealth {
        session: d.session,
        generation: d.generation,
        command: Box::new(Command::SetDongleReconnectPolicy { automatic: true }),
    };
    assert!(dongle_health::apply(&fixture.ctx, &set));
    until(|| fixture.ctx.state.lock().unwrap().dongle.health.reconnect_automatically == Some(true));
    assert!(fixture.hardware.lock().unwrap().reconnect_automatically);
    assert!(!fixture.ctx.state.lock().unwrap().dongle.connected);
}

#[test]
fn control_only_boot_and_audio_reenumeration_retire_old_sessions_without_reconnecting() {
    let fixture = Fixture::start_config(false, |h| {
        h.usb_audio_available = false;
        h.usb_playback = false;
        h.connected = false;
    });
    let before = fixture.ctx.state.lock().unwrap().dongle.clone();
    assert!(before.present && !before.connected && !before.usb_audio_available);
    assert_eq!(before.serial.as_deref(), Some("test-board"));
    assert_eq!(before.available_devices.len(), 1);
    {
        let mut h = fixture.hardware.lock().unwrap();
        h.path += 1;
        h.generation += 1;
        h.connected = true;
        h.usb_audio_available = true;
    }
    until(|| {
        let d = &fixture.ctx.state.lock().unwrap().dongle;
        d.session != before.session && d.connected && d.usb_audio_available
    });
    assert!(dongle::research_device(&fixture.ctx, before.session, before.generation).is_none());
    dongle::apply(&fixture.ctx, &ntpods_ipc::Command::DongleControl {
        session: before.session, generation: before.generation, opcode: 1, argument: 0,
    });
    assert_eq!(fixture.ctx.state.lock().unwrap().dongle.error.as_deref(), Some("stale"));
    assert!(fixture.hardware.lock().unwrap().commits.is_empty());
}

#[test]
fn custom_transparency_loads_automatically_and_live_writes_preserve_the_complete_record() {
    let fixture = Fixture::start_config(false, |h| {
        let mut metadata = vec![4, 0, 4, 0, 0x1d, 0, 2, 0, 0, 4, 0];
        for field in ["", "A2084", "", "", "6F21", "", "", "", "", "", ""] {
            metadata.extend_from_slice(field.as_bytes()); metadata.push(0);
        }
        h.metadata = Some(metadata);
        let mut record = vec![0; 104];
        record[4..8].copy_from_slice(&12.5f32.to_le_bytes());
        record[52..56].copy_from_slice(&8.25f32.to_le_bytes());
        record[100..104].copy_from_slice(&0.5f32.to_le_bytes());
        h.transparency = Some(record);
    });
    dongle_health::start(fixture.ctx.clone());
    until(|| fixture.ctx.state.lock().unwrap().dongle.health.transparency.is_some());
    let d = fixture.ctx.state.lock().unwrap().dongle.clone();
    let expected = d.health.transparency.unwrap();
    assert_eq!(expected.left_eq[0], 12.5);
    assert!(!expected.enabled);
    let original = fixture.hardware.lock().unwrap().transparency.clone().unwrap();
    assert!(fixture.hardware.lock().unwrap().raw_requests.iter().all(|(_, p)| p[0] != 0x12));
    let mut requested = expected;
    requested.enabled = true;
    requested.left_amplification = 1.25;
    requested.right_amplification = 1.25;
    dongle_health::apply(&fixture.ctx, &ntpods_ipc::Command::DongleHealth {
        session: d.session, generation: d.generation,
        command: Box::new(ntpods_ipc::Command::SetTransparency { settings: requested, expected: Some(expected) }),
    });
    until(|| fixture.ctx.state.lock().unwrap().dongle.health.transparency == Some(requested));
    let h = fixture.hardware.lock().unwrap();
    assert_eq!(h.raw_requests.iter().filter(|(_, p)| p[0] == 0x12).count(), 1);
    let record = h.transparency.as_ref().unwrap();
    assert_eq!(&record[4..36], &original[4..36]);
    assert_eq!(&record[52..84], &original[52..84]);
    assert_eq!(&record[100..104], &original[100..104]);
    assert!(h.lease.is_none());
}

#[test]
fn dongle_audiogram_uses_the_saved_profile_and_rejects_a_stale_live_edit_version() {
    let fixture = Fixture::start_config(false, |h| {
        let mut metadata = vec![4, 0, 4, 0, 0x1d, 0, 2, 0, 0, 4, 0];
        for field in ["", "A3063", "", "", "9A348", "", "", "", "", "", ""] {
            metadata.extend_from_slice(field.as_bytes()); metadata.push(0);
        }
        h.metadata = Some(metadata);
        let mut record = vec![0; 104];
        record[100..104].copy_from_slice(&0.5f32.to_le_bytes());
        h.transparency = Some(record);
        let fixture: serde_json::Value = serde_json::from_str(include_str!("fixtures/legacy-audiogram-synthetic-v1.json")).unwrap();
        let hex = fixture["cases"][0]["ahps_hex"].as_str().unwrap();
        h.clinical = Some(hex.as_bytes().chunks_exact(2).map(|pair|
            u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap()).collect());
        h.cached.insert(2, vec![4, 0, 4, 0, 9, 0, 0x2c, 1, 2, 0, 0]);
        h.cached.insert(3, vec![4, 0, 4, 0, 9, 0, 0x33, 2, 0, 0, 0]);
        let mut pme = vec![0; 140];
        pme[..12].copy_from_slice(&[4, 0, 4, 0, 0x53, 0, 0x84, 0, 2, 2, 2, 2]);
        pme[12..].copy_from_slice(accommodations::Preset::ALL[0].curves());
        h.cached.insert(4, pme);
    });
    dongle_health::start(fixture.ctx.clone());
    until(|| fixture.ctx.state.lock().unwrap().dongle.health.headphone_accommodations.as_ref()
        .is_some_and(|view| view.audiogram_available && view.editable));
    let d = fixture.ctx.state.lock().unwrap().dongle.clone();
    let view = d.health.headphone_accommodations.unwrap();
    let original = fixture.hardware.lock().unwrap().clinical.clone().unwrap();
    let command = ntpods_ipc::Command::DongleHealth { session: d.session, generation: d.generation,
        command: Box::new(ntpods_ipc::Command::SetHeadphoneAccommodations {
            request_id: 1, device_id: format!("dongle:{}:{}", d.session, d.generation),
            session: view.session, revision: view.revision, edit_version: view.edit_version,
            audiogram: true, phone: Some(true), media: Some(true), preset: None,
        }) };
    dongle_health::apply(&fixture.ctx, &command);
    until(|| fixture.ctx.state.lock().unwrap().dongle.health.headphone_accommodations.as_ref()
        .is_some_and(|view| view.edit_version == 1 && !view.pending));
    {
        let h = fixture.hardware.lock().unwrap();
        assert_eq!(h.clinical.as_ref(), Some(&original));
        assert!(h.raw_requests.iter().all(|(op, p)| *op != 1 || !matches!(p[0], 0x12 | 0x52) || p[1] == 0x35));
        assert_eq!(h.raw_requests.iter().filter(|(op, p)| *op == 2 && p[4] == 0x53).count(), 1);
        let curves = hearing::legacy_audiogram_curves(&original).unwrap();
        assert_eq!(&h.raw_requests.iter().find(|(op, p)| *op == 2 && p[4] == 0x53).unwrap().1[12..], &curves);
    }
    let writes = || fixture.hardware.lock().unwrap().raw_requests.iter()
        .filter(|(op, p)| *op == 2 || (*op == 1 && matches!(p[0], 0x12 | 0x52))).count();
    let sends = writes();
    dongle_health::apply(&fixture.ctx, &command);
    until(|| fixture.ctx.state.lock().unwrap().dongle.health.headphone_accommodations.as_ref()
        .is_some_and(|view| !view.pending && view.error.as_ref().is_some_and(|error| error.contains("changed"))));
    assert_eq!(writes(), sends);
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
    fixture.wear([2, 0]);
    until(|| !PLAYER.lock().unwrap().resumes.is_empty());
    let (_, permission) = PLAYER.lock().unwrap().resumes.pop().unwrap();
    fixture.ctx.state.lock().unwrap().dongle.generation += 1;
    fixture.hardware.lock().unwrap().generation += 1;
    assert!(!permission());
}

#[test]
fn regression_either_solo_bud_stays_usable_and_resumes_only_its_owned_case_transition() {
    for solo in [[0, 2], [2, 0]] {
        let fixture = Fixture::new();
        let reads = fixture.hardware.lock().unwrap().ear_reads;
        fixture.wear(solo);
        until(|| fixture.hardware.lock().unwrap().ear_reads >= reads + 2);
        assert_eq!(PLAYER.lock().unwrap().pauses, 0);
        assert!(PLAYER.lock().unwrap().resumes.is_empty());
        fixture.wear(solo.map(|status| if status == 2 { 1 } else { status }));
        until(|| PLAYER.lock().unwrap().pauses == 1);
        fixture.hardware.lock().unwrap().usb_playback = false;
        until(|| !fixture.ctx.state.lock().unwrap().dongle.usb_playback);
        fixture.wear(solo);
        until(|| !PLAYER.lock().unwrap().resumes.is_empty());
        let (_, permission) = PLAYER.lock().unwrap().resumes.pop().unwrap();
        assert!(permission());
    }
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

fn settings_session(fixture: &Fixture) -> dongle_rpc::Session {
    let d = fixture.ctx.state.lock().unwrap().dongle.clone();
    dongle_rpc::Session::open(fixture.ctx.clone(), d.session, d.generation).unwrap()
}

#[test]
fn settings_lease_is_lazy_and_required_before_old_firmware_raw_requests() {
    let fixture = Fixture::start(false);
    fixture.hardware.lock().unwrap().capabilities = 24;
    let mut rpc = settings_session(&fixture);
    assert!(rpc.cached(1).unwrap().is_none());
    assert_eq!(rpc.feature(&[2]).unwrap(), [1, 0]);
    assert!(rpc.exchange(&[0x0a, 1, 0]).unwrap_err().contains("Update"));
    assert!(rpc.send_aacp(&[4, 0, 4, 0, 9]).is_err());
    drop(rpc);
    let h = fixture.hardware.lock().unwrap();
    assert!(h.lease_calls.is_empty() && h.raw_requests.is_empty());
}

#[test]
fn settings_lease_covers_read_modify_verify_and_cached_confirmation_until_drop() {
    let fixture = Fixture::start(false);
    let mut rpc = settings_session(&fixture);
    assert!(rpc.cached(1).unwrap().is_none());
    assert!(fixture.hardware.lock().unwrap().lease_calls.is_empty());
    assert_eq!(rpc.exchange(&[0x0a, 1, 0]).unwrap(), [0x0b, 7]);
    rpc.send_aacp(&[4, 0, 4, 0, 9, 0, 0x2c, 1, 1, 0, 0])
        .unwrap();
    assert!(rpc.cached(2).unwrap().is_none());
    rpc.exchange(&[0x12, 1, 0, 8]).unwrap();
    rpc.exchange(&[0x0a, 1, 0]).unwrap();
    {
        let h = fixture.hardware.lock().unwrap();
        assert_eq!(
            h.raw_requests.iter().map(|(op, _)| *op).collect::<Vec<_>>(),
            [1, 2, 1, 1]
        );
        assert_eq!(
            h.lease_calls
                .iter()
                .map(|(action, _)| *action)
                .collect::<Vec<_>>(),
            [0, 1, 1, 1, 1]
        );
        assert_eq!(h.lease, Some(0x7654_3210));
    }
    drop(rpc);
    let h = fixture.hardware.lock().unwrap();
    assert_eq!(h.lease_calls.last(), Some(&(2, 0x7654_3210)));
    assert!(h.lease.is_none());
}

#[test]
fn settings_lease_precedes_hearing_mode_profile_and_self_check_baselines() {
    use ntpods_ipc::Command;
    let fixture = Fixture::new();
    for command in [
        Command::SetHearingAidMode {
            request_id: 1,
            on: false,
        },
        Command::SetMediaAssist {
            request_id: 2,
            on: true,
        },
        Command::SetHearingAid {
            request_id: 3,
            on: true,
            expected: None,
            settings: None,
        },
        Command::StartHardwareCheck {
            device_id: format!(
                "dongle:{}:{}",
                fixture.ctx.state.lock().unwrap().dongle.session,
                1
            ),
            request_id: 4,
            buds_in_case_confirmed: true,
        },
    ] {
        until(|| !dongle_health::busy(&fixture.ctx));
        {
            let mut h = fixture.hardware.lock().unwrap();
            h.commits.clear();
            h.lease_calls.clear();
        }
        let d = fixture.ctx.state.lock().unwrap().dongle.clone();
        assert!(dongle_health::apply(
            &fixture.ctx,
            &Command::DongleHealth {
                session: d.session,
                generation: d.generation,
                command: Box::new(command),
            }
        ));
        until(|| fixture.hardware.lock().unwrap().lease_calls.last() == Some(&(2, 0x7654_3210)));
        let h = fixture.hardware.lock().unwrap();
        let acquired = h
            .commits
            .iter()
            .position(|(op, p)| *op == 7 && p == &[10, 0, 0, 0, 0, 0])
            .unwrap();
        let baselines: Vec<_> = h
            .commits
            .iter()
            .enumerate()
            .filter(|(_, (op, p))| *op == 3 && matches!(p.as_slice(), [1..=4]))
            .map(|(index, _)| index)
            .collect();
        assert_eq!(baselines.len(), 4);
        assert!(baselines.iter().all(|index| *index > acquired));
        assert!(h.lease.is_none() && h.raw_requests.is_empty()); // Unknown model still fails safely.
    }
}

#[test]
fn settings_lease_expiry_or_rejected_renewal_never_reacquires_mid_action() {
    for expired in [false, true] {
        let fixture = Fixture::start(false);
        let mut rpc = settings_session(&fixture);
        rpc.exchange(&[0x0a, 1, 0]).unwrap();
        {
            let mut h = fixture.hardware.lock().unwrap();
            if expired {
                h.lease = None;
            } else {
                h.lease_error_action = Some(1);
            }
        }
        assert!(rpc.send_aacp(&[4, 0, 4, 0, 9]).is_err());
        assert!(rpc.exchange(&[0x12, 1, 0, 8]).is_err());
        assert!(rpc.cached(2).is_err());
        drop(rpc);
        {
            let h = fixture.hardware.lock().unwrap();
            assert_eq!(h.raw_requests.len(), 1);
            assert_eq!(h.lease_calls, [(0, 0), (1, 0x7654_3210), (2, 0x7654_3210)]);
        }
        // A new user action starts over with fresh reads and a new lease.
        let mut next = settings_session(&fixture);
        next.exchange(&[0x0a, 1, 0]).unwrap();
        drop(next);
        assert_eq!(fixture.hardware.lock().unwrap().raw_requests.len(), 2);
    }
}

#[test]
fn settings_lease_unknown_acquire_result_cannot_fall_back_or_retry() {
    for malformed in [false, true] {
        let fixture = Fixture::start(false);
        {
            let mut h = fixture.hardware.lock().unwrap();
            h.lease_bad_reply = malformed;
            if !malformed {
                h.lease_error_action = Some(0);
            }
        }
        let mut rpc = settings_session(&fixture);
        assert!(rpc.exchange(&[0x0a, 1, 0]).is_err());
        assert!(rpc.send_aacp(&[4, 0, 4, 0, 9]).is_err());
        drop(rpc);
        let h = fixture.hardware.lock().unwrap();
        assert_eq!(h.lease_calls, [(0, 0)]);
        assert!(h.raw_requests.is_empty());
    }
}

#[test]
fn settings_lease_raw_error_retains_cleanup_ownership_and_releases_on_drop() {
    for mode_cleanup in [false, true] {
        let fixture = Fixture::start(false);
        let mut rpc = settings_session(&fixture);
        fixture.hardware.lock().unwrap().raw_error = true;
        fixture.hardware.lock().unwrap().cancel_delay = 2;
        assert!(rpc.exchange(&[0x12, 1, 0, 8]).is_err());
        if mode_cleanup {
            rpc.send_aacp(&[4, 0, 4, 0, 9]).unwrap();
        }
        drop(rpc);
        let h = fixture.hardware.lock().unwrap();
        assert_eq!(h.raw_requests.len(), if mode_cleanup { 2 } else { 1 });
        assert_eq!(h.lease_calls.first(), Some(&(0, 0)));
        assert_eq!(h.lease_calls.last(), Some(&(2, 0x7654_3210)));
        assert!(h.lease.is_none());
    }
}

#[test]
fn settings_lease_generation_change_never_releases_or_writes_to_the_new_target() {
    let fixture = Fixture::start(false);
    let mut rpc = settings_session(&fixture);
    rpc.exchange(&[0x0a, 1, 0]).unwrap();
    fixture.ctx.state.lock().unwrap().dongle.generation += 1;
    {
        let mut h = fixture.hardware.lock().unwrap();
        h.generation += 1;
        h.lease = None; // Firmware drops leases with their connection generation.
    }
    assert!(rpc.exchange(&[0x12, 1, 0, 8]).is_err());
    drop(rpc);
    let h = fixture.hardware.lock().unwrap();
    assert_eq!(h.lease_calls, [(0, 0)]);
    assert_eq!(h.raw_requests.len(), 1);
}

#[test]
fn settings_lease_drop_can_release_during_shutdown_without_authorizing_other_commands() {
    let fixture = Fixture::start(false);
    let mut rpc = settings_session(&fixture);
    rpc.exchange(&[0x0a, 1, 0]).unwrap();
    fixture.ctx.shutting_down.store(true, Ordering::Release);
    assert!(!rpc.current());
    assert!(rpc.send_aacp(&[4, 0, 4, 0, 9]).is_err());
    drop(rpc);
    let h = fixture.hardware.lock().unwrap();
    assert_eq!(h.lease_calls.last(), Some(&(2, 0x7654_3210)));
    assert_eq!(h.raw_requests.len(), 1);
    assert!(h.lease.is_none());
}
