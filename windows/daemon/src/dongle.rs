//! The HID worker owns USB handles; discovery only reads status.
use super::{dongle_hid as hid, dongle_protocol as protocol, Ctx};
use ntpods_ipc::{Command, DongleState};
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
    initialized: AtomicBool,
    research: Mutex<Option<(u64, u32, Arc<hid::Device>)>>,
    pub(super) rpc: Mutex<()>,
}

pub fn research_device(ctx: &Ctx, session: u64, generation: u32) -> Option<Arc<hid::Device>> {
    let current = ctx.dongle.research.lock().unwrap();
    current
        .as_ref()
        .filter(|(s, g, _)| *s == session && *g == generation)
        .map(|(_, _, d)| d.clone())
}

impl Controller {
    fn inhibits(&self, state: &DongleState, explicit: bool) -> bool {
        !explicit && (!self.initialized.load(Ordering::Acquire) || inhibits_native(state, false))
    }
}

pub fn inhibits_native(state: &DongleState, explicit: bool) -> bool {
    (state.present || state.ambiguous) && !explicit
}

pub fn inhibits(ctx: &Ctx, explicit: bool) -> bool {
    ctx.dongle
        .inhibits(&ctx.state.lock().unwrap().dongle, explicit)
}

pub fn apply(ctx: &Ctx, command: &Command) -> bool {
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
        value.pending = pending || ctx.dongle.queued.lock().unwrap().is_some();
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

/// A new USB session invalidates requests queued before removal, even if firmware restarted at generation zero.
pub fn start(ctx: Ctx) {
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut device: Option<Arc<hid::Device>> = None;
        let mut known_path: Option<Vec<u16>> = None;
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
        let mut ready_tx = Some(ready_tx);
        while !ctx.shutting_down.load(Ordering::Acquire) {
            if last_discovery.elapsed() >= Duration::from_secs(1) {
                last_discovery = Instant::now();
                let discovered = hid::discover(known_path.as_deref());
                match discovered {
                    Ok(hid::Discovery::One(next)) => {
                        known_path = Some(next.path.clone());
                        if device.as_ref().map(|d| &d.path) != Some(&next.path) {
                            session = session.wrapping_add(1).max(1);
                            seed_sequence = true;
                            pending = None;
                            error = None;
                            device = Some(Arc::new(next));
                            state = DongleState {
                                present: true,
                                session,
                                ..Default::default()
                            };
                        }
                    }
                    other => {
                        if matches!(other, Ok(hid::Discovery::None | hid::Discovery::Multiple)) {
                            known_path = None;
                        }
                        device = None;
                        pending = None;
                        state = DongleState {
                            present: other.is_err(),
                            ambiguous: matches!(other, Ok(hid::Discovery::Multiple)),
                            ..Default::default()
                        };
                        error = if state.ambiguous {
                            Some("multiple".into())
                        } else if other.is_err() {
                            Some("discovery".into())
                        } else {
                            None
                        };
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
                        state.pair_token = pair.observe(session, report.address);
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
                        state = DongleState {
                            present: true,
                            session,
                            ..Default::default()
                        };
                        device = None;
                    }
                }
            } else if request.is_some() {
                error = Some("disconnected".into());
            }
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
    });
    // Establish USB ownership before the native connection workers start.
    let _ = ready_rx.recv_timeout(Duration::from_secs(2));
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
