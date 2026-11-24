//! A USB presentation lease keeps an explicitly released output selectable, never its radio owned.
use super::*;
use std::sync::{atomic::AtomicBool, Arc};

const CAPABILITY: u16 = 1024;
const TTL_MS: u32 = 10_000;

#[derive(Default)]
pub(super) struct Peer {
    last_frame: Option<Instant>,
    output: Option<(bool, Instant)>,
    transition_until: Option<Instant>,
}
impl Peer {
    pub(super) fn activity(&mut self, now: Instant) {
        self.last_frame = Some(now);
    }
    pub(super) fn output(&mut self, connected: bool, now: Instant) {
        self.output = Some((connected, now));
    }
    pub(super) fn transition(&mut self, until: Option<Instant>) {
        self.transition_until = until;
    }
    fn permits(&self, pc_connected: bool, now: Instant) -> bool {
        self.last_frame
            .is_some_and(|at| now.saturating_duration_since(at) <= PEER_LIMIT)
            && (pc_connected
                || self.transition_until.is_some_and(|until| now < until)
                || self.output.is_some_and(|(connected, at)| {
                    connected && now.saturating_duration_since(at) <= PEER_LIMIT
                }))
    }
}

fn sole_board(d: &ntpods_ipc::DongleState) -> bool {
    d.present
        && d.selected
        && !d.ambiguous
        && d.available_devices.len() == 1
        && d.serial.as_deref() == Some(d.available_devices[0].serial.as_str())
}
pub(super) fn supported(ctx: &Ctx) -> bool {
    let d = &ctx.state.lock().unwrap().dongle;
    // Require one control owner as well as the exact output's composite USB serial.
    sole_board(d) && d.capabilities & CAPABILITY != 0
}

pub(super) fn selected_output(ctx: &Ctx) -> bool {
    let selected = supported(ctx) && {
        let serial = ctx.state.lock().unwrap().dongle.serial.clone();
        serial
            .as_deref()
            .and_then(crate::volume::default_dongle_output_id)
            .is_some()
    };
    if !selected {
        revoke_output(&mut ctx.companion.lock().unwrap());
    }
    selected
}

fn revoke_output(state: &mut State) {
    if state.presentation.as_ref().is_some_and(|lease| {
        lease.alive.swap(false, Ordering::AcqRel)
            && state.authorization == lease.authorization
            && state.output_override == lease.output_override
    }) {
        state.output_override = state.output_override.wrapping_add(1);
        state.generation = state.generation.wrapping_add(1);
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Snapshot {
    active: bool,
    token: u32,
    nonce: u32,
    peer: [u8; 6],
    usb_generation: u32,
}
impl Snapshot {
    fn decode(b: &[u8]) -> Result<Self, &'static str> {
        if b.len() != 32 || b[..2] != [1, 32] || b[2] & !15 != 0 || b[3] != 0 || b[22..24] != [0, 0]
        {
            return Err("unavailable");
        }
        let word = |i| u32::from_le_bytes(b[i..i + 4].try_into().unwrap());
        let active = b[2] & 1 != 0;
        let peer: [u8; 6] = b[16..22].try_into().unwrap();
        if peer == [0; 6]
            || peer == [255; 6]
            || word(24) == 0
            || word(28) == 0
            || active && (word(4) == 0 || word(8) == 0 || word(8) > TTL_MS || word(12) == 0)
            || !active && (word(4) != 0 || word(8) != 0 || b[2] & 6 != 0)
        {
            return Err("unavailable");
        }
        Ok(Self {
            active,
            token: word(4),
            nonce: word(12),
            peer,
            usb_generation: word(24),
        })
    }
    fn confirms(&self, token: Option<u32>, nonce: u32, peer: [u8; 6], generation: u32) -> bool {
        self.active
            && self.nonce == nonce
            && self.peer == peer
            && self.usb_generation == generation
            && token.is_none_or(|token| self.token == token)
    }
}
fn request(action: u8, token: u32, nonce: u32) -> [u8; 10] {
    let mut bytes = [22, action, 0, 0, 0, 0, 0, 0, 0, 0];
    bytes[2..6].copy_from_slice(&token.to_le_bytes());
    bytes[6..].copy_from_slice(&nonce.to_le_bytes());
    bytes
}
fn next_nonce(nonce: u32) -> u32 {
    nonce.wrapping_add(1).max(1)
}

pub(super) struct Lease {
    alive: AtomicBool,
    authorization: u64,
    output_override: u64,
    companion_peer: u64,
    serial: String,
    session: u64,
    pair: u64,
    peer: [u8; 6],
    endpoint: String,
}
impl Lease {
    fn identity_current(
        &self,
        state: &State,
        d: &ntpods_ipc::DongleState,
        shutting_down: bool,
    ) -> bool {
        self.alive.load(Ordering::Acquire)
            && !shutting_down
            && state.authorization == self.authorization
            && state.output_override == self.output_override
            && state.presentation_peer.permits(d.connected, Instant::now())
            && state.link.as_ref().is_some_and(|link| {
                link.peer == self.companion_peer && link.device.to_be_bytes()[2..] == self.peer
            })
            && sole_board(d)
            && d.capabilities & CAPABILITY != 0
            && d.session == self.session
            && d.pair_token == self.pair
            && d.serial.as_deref() == Some(&self.serial)
            && d.paired_device_id == crate::dongle_protocol::paired_device_id(self.peer)
    }
    fn same_usb(&self, ctx: &Ctx) -> bool {
        let d = &ctx.state.lock().unwrap().dongle;
        d.present
            && !d.ambiguous
            && d.session == self.session
            && d.pair_token == self.pair
            && d.serial.as_deref() == Some(&self.serial)
            && d.paired_device_id == crate::dongle_protocol::paired_device_id(self.peer)
    }
    fn bound(&self, ctx: &Ctx) -> bool {
        let d = ctx.state.lock().unwrap().dongle.clone();
        self.identity_current(
            &ctx.companion.lock().unwrap(),
            &d,
            ctx.shutting_down.load(Ordering::Acquire),
        )
    }
    fn cleanup_current(&self, ctx: &Ctx, deadline: Instant) -> bool {
        self.same_usb(ctx) && Instant::now() < deadline
    }
    fn current(&self, ctx: &Ctx) -> bool {
        self.bound(ctx)
            && bonded(self.companion_peer)
            && crate::volume::default_dongle_output_id(&self.serial).as_ref()
                == Some(&self.endpoint)
    }
}

fn exchange(
    ctx: &Ctx,
    lease: &Arc<Lease>,
    bytes: &[u8],
    allowed: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<Snapshot, &'static str> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let permission: Arc<dyn Fn() -> bool + Send + Sync> =
        Arc::new(move || allowed() && Instant::now() < deadline);
    if !permission() || !lease.current(ctx) || Instant::now() >= deadline {
        return Err("superseded");
    }
    let d = ctx.state.lock().unwrap().dongle.clone();
    let mut rpc = crate::dongle_rpc::Session::open_local(ctx.clone(), d.session, d.generation)
        .map_err(|_| "unavailable")?;
    rpc.restrict(permission.clone());
    if rpc.peer() != lease.peer {
        return Err("superseded");
    }
    let reply = rpc
        .feature_until(bytes, deadline)
        .map_err(|_| "unavailable")?;
    if !permission() || !lease.current(ctx) || Instant::now() >= deadline {
        return Err("superseded");
    }
    Snapshot::decode(&reply)
}

pub(super) fn arm(
    ctx: &Ctx,
    routing: &crate::dongle_routing::Status,
    allowed: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<(), &'static str> {
    if !supported(ctx) {
        return Ok(());
    }
    let existing = ctx.companion.lock().unwrap().presentation.clone();
    if existing.is_some_and(|lease| lease.current(ctx)) {
        return Ok(());
    }
    let d = ctx.state.lock().unwrap().dongle.clone();
    let Some(endpoint) = d
        .serial
        .as_deref()
        .and_then(crate::volume::default_dongle_output_id)
    else {
        return Ok(());
    };
    if !allowed()
        || !d.usb_audio_available
        || !routing.known
        || !routing.controls_ready
        || !routing.owned
        || !routing.connected
        || routing.busy
        || routing.released
        || routing.releasing
        || routing.pending
    {
        return Err("unavailable");
    }
    let lease = {
        let state = ctx.companion.lock().unwrap();
        if !state
            .link
            .as_ref()
            .is_some_and(|link| link.device.to_be_bytes()[2..] == routing.peer)
        {
            return Err("superseded");
        }
        Arc::new(Lease {
            alive: AtomicBool::new(true),
            authorization: state.authorization,
            companion_peer: state.link.as_ref().unwrap().peer,
            output_override: state.output_override,
            serial: d.serial.clone().ok_or("unavailable")?,
            session: d.session,
            pair: d.pair_token,
            peer: routing.peer,
            endpoint,
        })
    };
    let context = ctx.clone();
    let owner = lease.clone();
    let permission: Arc<dyn Fn() -> bool + Send + Sync> =
        Arc::new(move || allowed() && owner.bound(&context));
    let before = exchange(ctx, &lease, &[22], permission.clone())?;
    if before.active || before.peer != routing.peer {
        return Err("busy");
    }
    let nonce = next_nonce(before.nonce);
    let armed = exchange(ctx, &lease, &request(0, 0, nonce), permission)?;
    if !armed.confirms(None, nonce, lease.peer, before.usb_generation) {
        return Err("unavailable");
    }
    ctx.companion.lock().unwrap().presentation = Some(lease.clone());
    let ctx = ctx.clone();
    thread::spawn(move || {
        crate::volume::init();
        let mut nonce = nonce;
        let context = ctx.clone();
        let owner = lease.clone();
        let permission: Arc<dyn Fn() -> bool + Send + Sync> =
            Arc::new(move || owner.bound(&context));
        loop {
            for _ in 0..10 {
                if ctx.shutting_down.load(Ordering::Acquire) {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
            if !permission() {
                break;
            }
            nonce = next_nonce(nonce);
            let result = exchange(
                &ctx,
                &lease,
                &request(1, armed.token, nonce),
                permission.clone(),
            );
            if !result.is_ok_and(|s| {
                s.confirms(Some(armed.token), nonce, lease.peer, armed.usb_generation)
            }) {
                break;
            }
        }
        {
            let mut state = ctx.companion.lock().unwrap();
            if state
                .presentation
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, &lease))
            {
                revoke_output(&mut state);
            }
        }
        lease.alive.store(false, Ordering::Release);
        // Cleanup may only reach the same USB session and token; expiry is the fallback after unplug/crash.
        let context = ctx.clone();
        let owner = lease.clone();
        let deadline = Instant::now() + Duration::from_secs(1);
        let cleanup = Arc::new(move || owner.cleanup_current(&context, deadline));
        if cleanup() && !ctx.shutting_down.load(Ordering::Acquire) {
            let d = ctx.state.lock().unwrap().dongle.clone();
            if let Ok(mut rpc) =
                crate::dongle_rpc::Session::open_local(ctx.clone(), d.session, d.generation)
            {
                rpc.restrict(cleanup);
                if rpc.peer() == lease.peer {
                    let _ =
                        rpc.feature_until(&request(2, armed.token, next_nonce(nonce)), deadline);
                }
            }
        }
        let mut state = ctx.companion.lock().unwrap();
        if state
            .presentation
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, &lease))
        {
            state.presentation = None;
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn response() -> [u8; 32] {
        let mut b = [0; 32];
        b[..3].copy_from_slice(&[1, 32, 1]);
        b[4] = 7;
        b[8..12].copy_from_slice(&TTL_MS.to_le_bytes());
        b[12] = 9;
        b[16..22].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        b[24] = 1;
        b[28] = 3;
        b
    }
    #[test]
    fn only_exact_versioned_bounded_owner_receipts_confirm_a_lease() {
        let b = response();
        let s = Snapshot::decode(&b).unwrap();
        assert!(s.confirms(None, 9, s.peer, 1));
        for (token, nonce, peer, generation) in [
            (Some(8), 9, s.peer, 1),
            (None, 8, s.peer, 1),
            (None, 9, [2; 6], 1),
            (None, 9, s.peer, 2),
        ] {
            assert!(!s.confirms(token, nonce, peer, generation));
        }
        for (at, value) in [
            (0, 2),
            (1, 31),
            (2, 16),
            (3, 1),
            (4, 0),
            (8, 255),
            (12, 0),
            (22, 1),
            (24, 0),
            (28, 0),
        ] {
            let mut bad = b;
            bad[at] = value;
            if at == 8 {
                bad[8..12].copy_from_slice(&(TTL_MS + 1).to_le_bytes());
            }
            assert!(Snapshot::decode(&bad).is_err(), "{at}");
        }
        assert!(Snapshot::decode(&b[..31]).is_err());
        let mut inactive = b;
        inactive[2] = 8;
        inactive[4..12].fill(0);
        assert!(!Snapshot::decode(&inactive).unwrap().active);
        inactive[2] |= 2;
        assert!(Snapshot::decode(&inactive).is_err());
        assert_eq!(
            request(1, 0x12345678, u32::MAX),
            [22, 1, 0x78, 0x56, 0x34, 0x12, 255, 255, 255, 255]
        );
        assert_eq!(next_nonce(u32::MAX), 1);
    }
    #[test]
    fn transport_reconnect_keeps_lease_but_manual_output_pair_board_or_approval_does_not() {
        let mut state = State::default();
        state.authorization = 3;
        state.presentation_peer.activity(Instant::now());
        state.presentation_peer.output(true, Instant::now());
        state.link = Some(Link {
            peer: 9,
            device: 0x010203040506,
            secret: [0; 32],
            held_away: true,
        });
        let lease = Lease {
            alive: AtomicBool::new(true),
            authorization: 3,
            output_override: 0,
            companion_peer: 9,
            serial: "board".into(),
            session: 7,
            pair: 8,
            peer: [1, 2, 3, 4, 5, 6],
            endpoint: "output".into(),
        };
        let mut d = ntpods_ipc::DongleState {
            present: true,
            selected: true,
            capabilities: CAPABILITY,
            serial: Some("board".into()),
            session: 7,
            pair_token: 8,
            paired_device_id: Some("010203040506".into()),
            available_devices: vec![ntpods_ipc::DongleDevice {
                serial: "board".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(lease.identity_current(&state, &d, false));
        state.connected = false;
        state.generation += 1;
        assert!(lease.identity_current(&state, &d, false));
        state.output_override += 1;
        assert!(!lease.identity_current(&state, &d, false));
        state.output_override = 0;
        state.authorization += 1;
        assert!(!lease.identity_current(&state, &d, false));
        state.authorization = 3;
        for field in 0..5 {
            let mut changed = d.clone();
            match field {
                0 => changed.session += 1,
                1 => changed.pair_token += 1,
                2 => changed.serial = Some("replacement".into()),
                3 => changed.paired_device_id = Some("020304050607".into()),
                _ => changed.available_devices.push(ntpods_ipc::DongleDevice {
                    serial: "second".into(),
                    ..Default::default()
                }),
            }
            assert!(!lease.identity_current(&state, &changed, false));
        }
        assert!(!lease.identity_current(&state, &d, true));
        d.capabilities = 512;
        assert!(!lease.identity_current(&state, &d, false));
    }
    #[test]
    fn an_observed_output_override_cancels_once_and_cannot_silently_revive() {
        let mut state = State::default();
        let lease = Arc::new(Lease {
            alive: AtomicBool::new(true),
            authorization: 0,
            output_override: 0,
            companion_peer: 1,
            serial: "board".into(),
            session: 7,
            pair: 8,
            peer: [1, 2, 3, 4, 5, 6],
            endpoint: "pico-output".into(),
        });
        state.presentation = Some(lease.clone());
        revoke_output(&mut state);
        assert_eq!((state.generation, state.output_override), (1, 1));
        assert!(!lease.alive.load(Ordering::Acquire));
        let newer_manual = state.output_override + 1;
        state
            .presentation
            .as_ref()
            .unwrap()
            .alive
            .store(true, Ordering::Release);
        state.output_override = newer_manual;
        revoke_output(&mut state);
        assert_eq!(state.output_override, newer_manual);
        assert_eq!(state.generation, 1);
        state.connected = true;
        revoke_output(&mut state);
        assert_eq!((state.generation, state.output_override), (1, newer_manual));
        assert!(!lease.alive.load(Ordering::Acquire));
    }
    #[test]
    fn retained_output_requires_recent_phone_ownership_or_a_bounded_owned_transition() {
        let now = Instant::now();
        let mut peer = Peer::default();
        assert!(!peer.permits(false, now));
        peer.activity(now);
        assert!(peer.permits(true, now));
        assert!(!peer.permits(false, now));
        peer.transition(Some(now + Duration::from_secs(1)));
        peer.output(false, now);
        assert!(peer.permits(false, now));
        assert!(!peer.permits(false, now + Duration::from_secs(1)));
        peer.transition(None);
        peer.output(true, now);
        assert!(peer.permits(false, now + Duration::from_secs(10)));
        assert!(!peer.permits(false, now + PEER_LIMIT + Duration::from_millis(1)));
        peer.activity(now + PEER_LIMIT);
        assert!(!peer.permits(false, now + PEER_LIMIT + Duration::from_millis(1)));
        peer.output(false, now + PEER_LIMIT);
        assert!(!peer.permits(false, now + PEER_LIMIT));
    }
    #[test]
    fn cleanup_can_release_its_retired_token_but_never_a_replacement_usb_owner() {
        let ctx = Ctx::for_test();
        let lease = Lease {
            alive: AtomicBool::new(false),
            authorization: 0,
            output_override: 0,
            companion_peer: 1,
            serial: "board".into(),
            session: 7,
            pair: 8,
            peer: [1, 2, 3, 4, 5, 6],
            endpoint: "old-default-output".into(),
        };
        ctx.companion.lock().unwrap().output_override = 9;
        ctx.state.lock().unwrap().dongle = ntpods_ipc::DongleState {
            present: true,
            serial: Some("board".into()),
            session: 7,
            pair_token: 8,
            paired_device_id: Some("010203040506".into()),
            ..Default::default()
        };
        let deadline = Instant::now() + Duration::from_secs(1);
        assert!(lease.cleanup_current(&ctx, deadline));
        assert!(!lease.bound(&ctx));
        assert!(!lease.cleanup_current(&ctx, Instant::now()));
        ctx.state.lock().unwrap().dongle.pair_token += 1;
        assert!(!lease.cleanup_current(&ctx, deadline));
        ctx.state.lock().unwrap().dongle.pair_token = 8;
        ctx.state.lock().unwrap().dongle.session += 1;
        assert!(!lease.cleanup_current(&ctx, deadline));
    }
}
