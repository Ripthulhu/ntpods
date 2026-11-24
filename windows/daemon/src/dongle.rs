//! The HID worker owns USB handles; discovery only reads status.
use super::{dongle_hid as hid, dongle_protocol as protocol, Ctx};
use ntpods_ipc::{Command, DongleDevice, DongleState};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
struct Request {
    session: u64,
    generation: u32,
    opcode: u8,
    argument: u8,
}
#[derive(Default)]
struct PairIdentity {
    session: u64,
    address: [u8; 6],
    revision: u64,
}
impl PairIdentity {
    fn observe(&mut self, session: u64, address: [u8; 6]) -> u64 {
        if self.session != session || self.address != address {
            self.session = session;
            self.address = address;
            self.revision = self.revision.wrapping_add(1).max(1);
        }
        if address == [0; 6] {
            0
        } else {
            self.revision
        }
    }
}

#[derive(Default)]
pub struct Controller {
    queued: Mutex<Option<Request>>,
    select: Mutex<Option<String>>,
    initialized: AtomicBool,
    pub(super) stopped: AtomicBool,
    usb_presence: Mutex<UsbPresence>,
    research: Mutex<Option<(u64, u32, Arc<hid::Device>)>>,
    pub(super) rpc: Mutex<()>,
}

#[derive(Default)]
struct UsbPresence {
    present: bool,
    lost_at: Option<Instant>,
}
impl UsbPresence {
    fn observe(&mut self, present: bool, now: Instant) {
        if present {
            self.lost_at = None;
        } else if self.present {
            // A personality change briefly removes both USB interfaces. Repeated
            // empty inventories must still let a real unplug expire.
            self.lost_at = Some(now);
        }
        self.present = present;
    }

    fn inhibits(&self, now: Instant) -> bool {
        self.present || self.lost_at.is_some_and(|lost| now.duration_since(lost) < Duration::from_secs(2))
    }
}

pub fn research_device(ctx: &Ctx, session: u64, generation: u32) -> Option<Arc<hid::Device>> {
    let current = ctx.dongle.research.lock().unwrap();
    current
        .as_ref()
        .filter(|(s, g, _)| *s == session && *g == generation)
        .map(|(_, _, d)| d.clone())
}

impl Controller {
    pub(super) fn inhibits(&self, state: &DongleState, explicit: bool) -> bool {
        self.inhibits_at(state, explicit, Instant::now())
    }

    fn inhibits_at(&self, state: &DongleState, explicit: bool, now: Instant) -> bool {
        !explicit && (!self.initialized.load(Ordering::Acquire) || inhibits_native(state, false)
            || self.usb_presence.lock().unwrap().inhibits(now))
    }
}

pub fn inhibits_native(state: &DongleState, explicit: bool) -> bool {
    (state.present || state.ambiguous || !state.available_devices.is_empty()) && !explicit
}

pub fn inhibits(ctx: &Ctx, explicit: bool) -> bool {
    ctx.dongle
        .inhibits(&ctx.state.lock().unwrap().dongle, explicit)
}

pub fn apply(ctx: &Ctx, command: &Command) -> bool {
    if let Command::SelectDongle { serial } = command {
        let mut state = ctx.state.lock().unwrap();
        let d = &mut state.dongle;
        if !d.available_devices.iter().any(|device| device.serial == *serial) {
            d.error = Some("stale".into());
        } else if d.pending || super::dongle_health::busy(ctx) {
            d.error = Some("busy".into());
        } else {
            *ctx.dongle.select.lock().unwrap() = Some(serial.clone());
            d.pending = true;
            d.error = None;
        }
        drop(state);
        ctx.push_state();
        return true;
    }
    let Command::DongleControl {
        session,
        generation,
        opcode,
        argument,
    } = command
    else {
        return false;
    };
    let routing = matches!(opcode, 1 | 2 | 4);
    let _actions = routing.then(|| ctx.companion_actions.lock().unwrap());
    let mut state = ctx.state.lock().unwrap();
    let d = &mut state.dongle;
    let mut queued = ctx.dongle.queued.lock().unwrap();
    let mut accepted = false;
    if !d.present
        || d.ambiguous
        || *session == 0
        || *session != d.session
        || *generation != d.generation
    {
        d.error = Some("stale".into());
    } else if d.pending || queued.is_some() || super::dongle_health::busy(ctx) {
        d.error = Some("busy".into());
    } else if let Err(reason) = protocol::validate_command(d, *generation, *opcode, *argument) {
        d.error = Some(reason.into());
    } else {
        *queued = Some(Request {
            session: *session,
            generation: *generation,
            opcode: *opcode,
            argument: *argument,
        });
        d.pending = true;
        d.error = None;
        accepted = true;
    }
    drop(queued);
    if accepted && routing {
        ctx.case_return
            .lock()
            .unwrap()
            .revoke(ctx.mac(), ctx.connect_requested.snapshot());
    }
    drop(state);
    if accepted && routing {
        super::companion::local_override(ctx, command);
    }
    ctx.push_state();
    true
}

fn publish(ctx: &Ctx, mut value: DongleState, pending: bool, error: &Option<String>) {
    value.error = error.clone();
    let changed = {
        let mut state = ctx.state.lock().unwrap();
        if state.dongle.session == value.session
            && state.dongle.pair_token == value.pair_token
            && state.dongle.generation == value.generation
        {
            value.health = state.dongle.health.clone();
        } else if value.pair_token != 0
            && state.dongle.session == value.session
            && state.dongle.pair_token == value.pair_token
        {
            value.health.nearby_finder = state.dongle.health.nearby_finder.clone();
        }
        value.pending = pending || ctx.dongle.queued.lock().unwrap().is_some()
            || ctx.dongle.select.lock().unwrap().is_some();
        if state.dongle == value {
            false
        } else {
            state.dongle = value;
            true
        }
    };
    if changed {
        ctx.push_state();
    }
}

fn selection<'a>(preferred: &mut Option<String>, devices: &'a [DongleDevice]) -> Option<&'a DongleDevice> {
    if preferred.is_none() && devices.len() == 1 {
        *preferred = Some(devices[0].serial.clone());
    }
    preferred.as_ref().and_then(|serial| devices.iter().find(|device| device.serial == *serial))
}

fn unavailable(previous: &DongleState, elapsed: Duration) -> DongleState {
    let mut battery = previous.battery.clone();
    battery.left_charging = false;
    battery.right_charging = false;
    battery.case_charging = false;
    DongleState {
        serial: previous.serial.clone(),
        capabilities: previous.capabilities & 64,
        battery,
        battery_age_ms: previous.battery_age_ms.map(|age|
            age.map(|age| age.saturating_add(elapsed.as_millis().min(u128::from(u32::MAX - 1)) as u32).min(u32::MAX - 1))),
        battery_case_source: previous.battery_case_source,
        ..Default::default()
    }
}

/// A new USB session invalidates requests queued before removal, even if firmware restarted at generation zero.
pub fn start(ctx: Ctx) {
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut device: Option<Arc<hid::Device>> = None;
        let mut known_path: Option<Vec<u16>> = None;
        let mut preferred = ctx.companion_preferences.lock().unwrap().selected_dongle().map(str::to_owned);
        let mut available = Vec::new();
        // Also invalidate old IPC commands across daemon restarts while USB stays powered.
        let mut session = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        let mut sequence = 0u32;
        let mut pair = PairIdentity::default();
        let mut seed_sequence = true;
        let mut pending: Option<protocol::Pending> = None;
        let mut error = None;
        let mut state = DongleState::default();
        let mut last_discovery = Instant::now() - Duration::from_secs(2);
        let mut last_observation = Instant::now();
        let mut ready_tx = Some(ready_tx);
        while !ctx.shutting_down.load(Ordering::Acquire) {
            let selected = ctx.dongle.select.lock().unwrap().take();
            if let Some(serial) = selected {
                if ctx.companion_preferences.lock().unwrap().set_selected_dongle(&serial).is_err() {
                    error = Some("io".into());
                } else {
                    if preferred.as_deref() != Some(serial.as_str()) {
                        preferred = Some(serial);
                        known_path = None;
                        device = None;
                        pending = None;
                        ctx.dongle.queued.lock().unwrap().take();
                        *ctx.dongle.research.lock().unwrap() = None;
                        state = DongleState::default();
                    }
                    last_discovery = Instant::now() - Duration::from_secs(2);
                }
            }
            if last_discovery.elapsed() >= if device.is_some() { Duration::from_secs(2) } else { Duration::from_millis(500) } {
                last_discovery = Instant::now();
                let discovered = hid::discover(known_path.as_deref());
                match discovered {
                    Ok(devices) => {
                        ctx.dongle.usb_presence.lock().unwrap().observe(!devices.is_empty(), Instant::now());
                        available = devices.iter().map(|d| DongleDevice {
                            serial: d.serial.clone(), usb_audio_available: d.usb_audio_available,
                        }).collect();
                        let selected = selection(&mut preferred, &available).map(|d| d.serial.clone());
                        let next = devices.into_iter().find(|d| Some(&d.serial) == selected.as_ref());
                        if let Some(next) = next {
                            known_path = Some(next.path.clone());
                            if device.as_ref().map(|d| &d.path) != Some(&next.path) {
                                session = session.wrapping_add(1).max(1);
                                seed_sequence = true;
                                pending = None;
                                error = None;
                                device = Some(Arc::new(next));
                                state.present = true;
                                state.session = session;
                            }
                        } else {
                            known_path = None;
                            device = None;
                            pending = None;
                            state = unavailable(&state, last_observation.elapsed());
                            last_observation = Instant::now();
                            state.serial = preferred.clone();
                            state.ambiguous = preferred.is_none() && available.len() > 1;
                            error = if state.ambiguous { Some("multiple".into()) }
                                else if preferred.is_some() && !available.is_empty() { Some("selected_missing".into()) }
                                else { None };
                            *ctx.dongle.research.lock().unwrap() = None;
                        }
                    }
                    Err(_) => {
                        device = None;
                        pending = None;
                        state = unavailable(&state, last_observation.elapsed());
                        last_observation = Instant::now();
                        state.present = true;
                        error = Some("discovery".into());
                    }
                }
            }
            let request = ctx.dongle.queued.lock().unwrap().take();
            if let Some(usb) = device.as_ref() {
                match usb.read() {
                    Ok(report) => {
                        if seed_sequence {
                            sequence = report.sequence;
                            seed_sequence = false;
                        }
                        state = report.state.clone();
                        state.session = session;
                        state.serial = Some(usb.serial.clone());
                        state.usb_audio_available = usb.usb_audio_available;
                        last_observation = Instant::now();
                        state.pair_token = pair.observe(session, report.address);
                        state.paired_device_id = protocol::paired_device_id(report.address);
                        if let Some(waiting) = pending.as_ref() {
                            if let Some(result) =
                                waiting.poll(Some(&report), session, Instant::now())
                            {
                                error = result.err().map(str::to_string);
                                pending = None;
                            }
                        }
                        if let Some(request) = request {
                            if request.session != session {
                                error = Some("stale".into());
                            } else if pending.is_some() || super::dongle_health::busy(&ctx) {
                                error = Some("busy".into());
                            } else {
                                sequence = sequence.wrapping_add(1).max(1);
                                match protocol::command(
                                    &report,
                                    request.generation,
                                    sequence,
                                    request.opcode,
                                    request.argument,
                                ) {
                                    Err(reason) => error = Some(reason.into()),
                                    Ok(bytes) => {
                                        let since = Instant::now();
                                        if usb.write(bytes).is_ok() {
                                            pending = Some(protocol::Pending {
                                                sequence,
                                                opcode: request.opcode,
                                                session,
                                                since,
                                            });
                                            error = None;
                                        } else {
                                            error = Some("io".into());
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Err(problem) => {
                        pending = None;
                        error = Some(
                            if problem.kind() == std::io::ErrorKind::InvalidData {
                                "protocol"
                            } else {
                                "io"
                            }
                            .into(),
                        );
                        state = unavailable(&state, last_observation.elapsed());
                        last_observation = Instant::now();
                        state.present = true;
                        state.session = session;
                        device = None;
                    }
                }
            } else if request.is_some() {
                error = Some("disconnected".into());
            }
            state.available_devices = available.clone();
            *ctx.dongle.research.lock().unwrap() = device
                .as_ref()
                .map(|d| (session, state.generation, d.clone()));
            publish(&ctx, state.clone(), pending.is_some(), &error);
            ctx.dongle.initialized.store(true, Ordering::Release);
            if let Some(tx) = ready_tx.take() {
                let _ = tx.send(());
            }
            thread::sleep(Duration::from_millis(if pending.is_some() {
                50
            } else {
                500
            }));
        }
        if let Some(usb) = device {
            let _ = crate::dongle_rpc::Session::stop_finder_at_shutdown(
                ctx.clone(),
                usb,
                session,
                state.generation,
            );
        }
        ctx.dongle.queued.lock().unwrap().take();
        ctx.dongle.research.lock().unwrap().take();
        ctx.dongle.stopped.store(true, Ordering::Release);
    });
    // Publish the initial USB inventory before starting the IPC servers.
    let _ = ready_rx.recv_timeout(Duration::from_secs(2));
}

#[cfg(test)]
mod tests {
    use super::*;
    fn board(serial: &str, usb_audio_available: bool) -> DongleDevice {
        DongleDevice { serial: serial.into(), usb_audio_available }
    }
    #[test]
    fn selection_tracks_board_across_personalities_and_never_falls_back_to_another() {
        let mut selected = None;
        assert!(selection(&mut selected, &[board("a", false), board("b", true)]).is_none());
        assert!(selected.is_none());
        assert_eq!(selection(&mut selected, &[board("a", false)]).unwrap().serial, "a");
        assert_eq!(selection(&mut selected, &[board("a", true), board("b", true)]).unwrap().serial, "a");
        assert!(selection(&mut selected, &[board("b", true)]).is_none());
        assert!(selection(&mut selected, &[]).is_none());
        assert_eq!(selected.as_deref(), Some("a"));
        assert!(!selection(&mut selected, &[board("a", false)]).unwrap().usb_audio_available);
    }
    #[test]
    fn reenumeration_retains_case_age_but_clears_presence_charging_and_controls() {
        let previous = DongleState { serial: Some("a".into()), present: true, connected: true,
            capabilities: 127, battery_case_source: 4,
            controls_ready: true, battery_present: 7, usb_audio_available: true,
            battery: ntpods_ipc::Battery { case: Some(61), case_charging: true, ..Default::default() },
            battery_age_ms: [None, Some(u32::MAX - 2), Some(2000)], ..Default::default() };
        let stale = unavailable(&previous, Duration::from_secs(1));
        assert_eq!(stale.battery.case, Some(61));
        assert_eq!(stale.battery_age_ms, [None, Some(u32::MAX - 1), Some(3000)]);
        assert_eq!(stale.serial, previous.serial);
        assert!(!stale.connected && !stale.controls_ready && !stale.usb_audio_available);
        assert!(!stale.battery.case_charging);
        assert_eq!(stale.battery_present, 0);
        assert_eq!((stale.capabilities, stale.battery_case_source), (64, 4));
        assert!(stale.health.model.is_empty());
    }
    #[test]
    fn dongle_selection_requires_a_current_inventory_and_idle_settings() {
        let ctx = Ctx::for_test();
        ctx.state.lock().unwrap().dongle.available_devices = vec![board("a", false), board("b", true)];
        assert!(apply(&ctx, &Command::SelectDongle { serial: "missing".into() }));
        assert_eq!(ctx.state.lock().unwrap().dongle.error.as_deref(), Some("stale"));
        assert!(ctx.dongle.select.lock().unwrap().is_none());
        assert!(apply(&ctx, &Command::SelectDongle { serial: "b".into() }));
        assert_eq!(ctx.dongle.select.lock().unwrap().as_deref(), Some("b"));
        assert!(ctx.state.lock().unwrap().dongle.pending);
        assert!(apply(&ctx, &Command::SelectDongle { serial: "a".into() }));
        assert_eq!(ctx.state.lock().unwrap().dongle.error.as_deref(), Some("busy"));
        assert_eq!(ctx.dongle.select.lock().unwrap().as_deref(), Some("b"));
    }
    #[test]
    fn opaque_pair_identity_is_stable_until_peer_or_usb_session_changes() {
        let mut identity = PairIdentity::default();
        assert_eq!(identity.observe(1, [0; 6]), 0);
        let token = identity.observe(1, [1, 2, 3, 4, 5, 6]);
        assert_ne!(token, 0);
        assert_eq!(identity.observe(1, [1, 2, 3, 4, 5, 6]), token);
        assert_ne!(identity.observe(1, [1, 2, 3, 4, 5, 7]), token);
        assert_ne!(identity.observe(2, [1, 2, 3, 4, 5, 6]), token);
    }
    #[test]
    fn dongle_only_inhibits_automatic_native_connections() {
        for d in [
            DongleState {
                present: true,
                ..Default::default()
            },
            DongleState {
                ambiguous: true,
                ..Default::default()
            },
        ] {
            assert!(inhibits_native(&d, false));
            assert!(!inhibits_native(&d, true));
        }
        assert!(!inhibits_native(&DongleState::default(), false));
    }
    #[test]
    fn initial_discovery_timeout_does_not_authorize_native_acquisition() {
        let controller = Controller::default();
        let absent = DongleState::default();
        assert!(controller.inhibits(&absent, false));
        assert!(!controller.inhibits(&absent, true));
        controller.initialized.store(true, Ordering::Release);
        assert!(!controller.inhibits(&absent, false));
        assert!(controller.inhibits(
            &DongleState {
                present: true,
                ..Default::default()
            },
            false
        ));
    }

    #[test]
    fn usb_reenumeration_blocks_automatic_native_until_return_or_real_unplug() {
        let controller = Controller::default();
        controller.initialized.store(true, Ordering::Release);
        let absent = DongleState::default();
        let now = Instant::now();
        let observe = |present, at| controller.usb_presence.lock().unwrap().observe(present, at);
        assert!(!controller.inhibits_at(&absent, false, now));
        observe(true, now);
        assert!(controller.inhibits_at(&absent, false, now));
        observe(false, now + Duration::from_millis(100));
        assert!(controller.inhibits_at(&absent, false, now + Duration::from_millis(350)));
        assert!(!controller.inhibits_at(&absent, true, now + Duration::from_millis(350)));
        observe(true, now + Duration::from_millis(400));
        assert!(controller.inhibits_at(&absent, false, now + Duration::from_secs(5)));
        observe(false, now + Duration::from_secs(6));
        observe(false, now + Duration::from_millis(7900));
        assert!(controller.inhibits_at(&absent, false, now + Duration::from_millis(7999)));
        assert!(!controller.inhibits_at(&absent, false, now + Duration::from_secs(8)));
    }

    #[test]
    fn only_accepted_dongle_routes_revoke_the_pc_case_return() {
        for (session, generation, opcode, argument, capabilities, pending, revoke) in [
            (7, 1, 1, 0, 7, false, true),
            (7, 1, 2, 0, 7, false, true),
            (7, 1, 4, 0, 7, false, true),
            (8, 1, 1, 0, 7, false, false),
            (7, 2, 1, 0, 7, false, false),
            (7, 1, 1, 1, 7, false, false),
            (7, 1, 1, 0, 0, false, false),
            (7, 1, 1, 0, 7, true, false),
            (7, 1, 3, 2, 7, false, false),
        ] {
            let ctx = Ctx::for_test();
            ctx.mac.store(1, Ordering::Release);
            ctx.connect_requested.store(true, Ordering::Release);
            let request = ctx.connect_requested.snapshot();
            let idle_request = request.wrapping_add(4) & !3;
            ctx.state.lock().unwrap().dongle = DongleState {
                present: true,
                selected: true,
                controls_ready: true,
                session: 7,
                generation: 1,
                capabilities,
                pending,
                ..Default::default()
            };
            {
                let mut policy = ctx.case_return.lock().unwrap();
                policy.record_owned_playback(1, request, 10);
                policy.observe_case(1, request, Some(true), 20);
            }
            assert!(apply(&ctx, &Command::DongleControl {
                session,
                generation,
                opcode,
                argument,
            }));
            let mut policy = ctx.case_return.lock().unwrap();
            policy.record_owned_playback(1, request, 30);
            policy.observe_case(1, request, Some(true), 40);
            assert_eq!(policy.link_lost(1, request, idle_request, 50), !revoke);
            assert_eq!(policy.take_open(1, idle_request, true, 60, 60), !revoke,
                "opcode={opcode} session={session} generation={generation} pending={pending}");
        }
    }
}
