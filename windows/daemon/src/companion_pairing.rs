//! One-time provisioning over the bonded setup service; ordinary sessions stay v2.
use super::*;
use ntpods_ipc::CompanionPairingStatus;

const SETUP_SERVICE: GUID = GUID::from_u128(0xbbc1e9f2_6f73_4c61_ae15_6e74706f6474);
const WINDOW: Duration = Duration::from_secs(120);
const ATTEMPTS: u8 = 5;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Ticket {
    request: u64,
    peer: u64,
    device: u64,
    authorization: u64,
    generation: u64,
}
struct Window {
    ticket: Ticket,
    code: [u8; 8],
    since: Instant,
    attempts: u8,
    claimed: bool,
}
impl Drop for Window {
    fn drop(&mut self) {
        wipe(&mut self.code);
    }
}
impl Window {
    fn current(&self, ticket: Ticket, authorization: u64, generation: u64, now: Instant) -> bool {
        self.ticket == ticket
            && ticket.authorization == authorization
            && ticket.generation == generation
            && now.saturating_duration_since(self.since) < WINDOW
            && self.attempts < ATTEMPTS
    }
    fn claim(&mut self, code: &str, now: Instant) -> bool {
        if self.claimed
            || now.saturating_duration_since(self.since) >= WINDOW
            || self.attempts >= ATTEMPTS
        {
            return false;
        }
        let valid = code.len() == 8
            && code
                .bytes()
                .zip(self.code)
                .fold(0u8, |diff, (a, b)| diff | (a ^ b))
                == 0;
        if valid {
            self.claimed = true;
            wipe(&mut self.code);
        } else {
            self.attempts += 1;
        }
        valid
    }
}

#[derive(Default)]
pub(super) struct Pairing {
    active: Option<Window>,
    terminal: Option<CompanionPairingStatus>,
    socket: Option<(u64, SOCKET)>,
    used: HashSet<u64>,
}
impl Pairing {
    pub(super) fn status(&self) -> Option<CompanionPairingStatus> {
        self.active
            .as_ref()
            .map(|w| CompanionPairingStatus {
                request_id: w.ticket.request,
                device_id: format!("{:012x}", w.ticket.device),
                peer_address: mac(w.ticket.peer),
                phase: if w.claimed {
                    "authenticating"
                } else {
                    "waiting"
                }
                .into(),
                remaining_ms: WINDOW.saturating_sub(w.since.elapsed()).as_millis() as u32,
            })
            .or_else(|| self.terminal.clone())
    }
    fn finish(&mut self, phase: &str, close: bool) {
        if let Some(w) = self.active.take() {
            self.terminal = Some(CompanionPairingStatus {
                request_id: w.ticket.request,
                device_id: format!("{:012x}", w.ticket.device),
                peer_address: mac(w.ticket.peer),
                phase: phase.into(),
                remaining_ms: 0,
            });
        }
        if close {
            self.close_socket();
        }
    }
    fn close_socket(&mut self) {
        if let Some((_, socket)) = self.socket {
            unsafe {
                shutdown(socket, SD_BOTH);
            }
        }
    }
    fn cancel(&mut self, request: u64) {
        if self
            .active
            .as_ref()
            .is_some_and(|w| w.ticket.request == request)
        {
            self.finish("cancelled", true);
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    v: u32,
    #[serde(rename = "type")]
    kind: String,
    tx: String,
    device: String,
    nonce: String,
    code: String,
}
impl Drop for Request {
    fn drop(&mut self) {
        unsafe {
            wipe(self.code.as_bytes_mut());
        }
    }
}
fn request(line: &[u8], device: u64) -> io::Result<Request> {
    if line.first() != Some(&b'{')
        || line.last() != Some(&b'}')
        || line.contains(&b'\r')
        || line.len() > MAX_FRAME
    {
        return Err(invalid());
    }
    let value: Request = serde_json::from_slice(line).map_err(|_| invalid())?;
    if value.v != 2
        || value.kind != "pair_request"
        || value.tx != ZERO_TX
        || value.device != mac(device)
        || decode_hex::<32>(&value.nonce).is_none()
        || !valid_code(&value.code)
    {
        return Err(invalid());
    }
    Ok(value)
}
fn valid_code(code: &str) -> bool {
    code.len() == 8 && code.bytes().all(|b| b.is_ascii_digit())
}
fn save_candidate(
    state: &mut State,
    candidate: Link,
    revoke_consent: impl FnOnce() -> io::Result<()>,
    save: impl FnOnce(&State) -> io::Result<()>,
) -> io::Result<()> {
    revoke_consent()?;
    let old = state.link.replace(candidate);
    if let Err(error) = save(state) {
        state.link = old;
        return Err(error);
    }
    Ok(())
}
fn current(ctx: &Ctx, ticket: Ticket) -> bool {
    if ctx.shutting_down.load(Ordering::Acquire)
        || !selected_pair(ctx, ticket.device)
        || !bonded(ticket.peer)
    {
        return false;
    }
    let state = ctx.companion.lock().unwrap();
    state.pairing.active.as_ref().is_some_and(|w| {
        w.current(
            ticket,
            state.authorization,
            state.generation,
            Instant::now(),
        )
    })
}

pub(super) fn observe_local(ctx: &Ctx, command: &Command) {
    if !matches!(
        command,
        Command::CompanionLink { .. }
            | Command::CompanionForget { .. }
            | Command::Shutdown
            | Command::SelectDongle { .. }
            | Command::Connect { .. }
            | Command::Disconnect
            | Command::RepairConnection
            | Command::PairingCompleted { .. }
            | Command::DongleControl {
                opcode: 1 | 2 | 4,
                ..
            }
    ) {
        return;
    }
    ctx.companion
        .lock()
        .unwrap()
        .pairing
        .finish("cancelled", true);
}

pub(super) fn apply_local(ctx: &Ctx, command: &Command) -> bool {
    let (name, id, outcome) = match command {
        Command::CompanionBeginPairing {
            peer_address,
            device_id,
            code,
            request_id,
        } => {
            let result = (|| {
                let peer = parse_mac(peer_address).ok_or("Choose the bonded phone.")?;
                let device = parse_device_id(device_id).ok_or("Select the paired AirPods.")?;
                if *request_id == 0
                    || !valid_code(code)
                    || peer == device
                    || !selected_pair(ctx, device)
                    || !bonded(peer)
                {
                    return Err("Pair the phone and select the AirPods before linking.");
                }
                let _actions = ctx.companion_actions.lock().unwrap();
                if !selected_pair(ctx, device) {
                    return Err("The selected AirPods changed.");
                }
                let mut state = ctx.companion.lock().unwrap();
                if state.pairing.used.contains(request_id) || state.pairing.used.len() >= 128 {
                    return Err("Create a new setup request.");
                }
                let ticket = Ticket {
                    request: *request_id,
                    peer,
                    device,
                    authorization: state.authorization,
                    generation: state.generation,
                };
                state.pairing.finish("cancelled", true);
                state.pairing.used.insert(*request_id);
                state.pairing.terminal = None;
                state.pairing.active = Some(Window {
                    ticket,
                    code: code.as_bytes().try_into().unwrap(),
                    since: Instant::now(),
                    attempts: 0,
                    claimed: false,
                });
                Ok(())
            })();
            ("companion_begin_pairing", *request_id, result)
        }
        Command::CompanionCancelPairing { request_id } => {
            let result = if *request_id == 0 {
                Err("Use the setup request identifier.")
            } else {
                ctx.companion.lock().unwrap().pairing.cancel(*request_id);
                Ok(())
            };
            ("companion_cancel_pairing", *request_id, result)
        }
        _ => return false,
    };
    ctx.send_event(&Event::CommandResult {
        command: name.into(),
        request_id: id,
        ok: outcome.is_ok(),
        message: outcome.err().unwrap_or("").into(),
    });
    publish(ctx);
    true
}

fn commit(ctx: &Ctx, ticket: Ticket, candidate: &Link) -> io::Result<()> {
    let _actions = ctx.companion_actions.lock().unwrap();
    if !current(ctx, ticket) {
        return Err(invalid());
    }
    let mut preferences = ctx.companion_preferences.lock().unwrap();
    let mut state = ctx.companion.lock().unwrap();
    if !state.pairing.active.as_ref().is_some_and(|w| {
        w.claimed
            && w.current(
                ticket,
                state.authorization,
                state.generation,
                Instant::now(),
            )
    }) {
        return Err(invalid());
    }
    // Revoke consent first: a failed key save retains the old approval without granting new call access.
    let mut candidate = candidate.clone();
    candidate.held_away = state.holds(ticket.device);
    save_candidate(
        &mut state,
        candidate,
        || preferences.set_consent(false),
        State::save,
    )?;
    state.authorization = state.authorization.wrapping_add(1);
    state.presentation_peer = presence::Peer::default();
    state.generation = state.generation.wrapping_add(1);
    state.connected = false;
    state.queued = None;
    state.release = None;
    state.error = None;
    state.phase = "waiting".into();
    state.pairing.finish("completed", false);
    drop(state);
    drop(preferences);
    publish(ctx);
    Ok(())
}

fn provision(ctx: &Ctx, ticket: Ticket, stream: &mut Framed) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let request = loop {
        if Instant::now() >= deadline || !current(ctx, ticket) {
            return Err(invalid());
        }
        if let Some(line) = stream.poll_line()? {
            break request(&line, ticket.device)?;
        }
        thread::sleep(Duration::from_millis(20));
    };
    let accepted = {
        let mut state = ctx.companion.lock().unwrap();
        let authorization = state.authorization;
        let generation = state.generation;
        let w = state.pairing.active.as_mut().ok_or_else(invalid)?;
        if !w.current(ticket, authorization, generation, Instant::now()) {
            return Err(invalid());
        }
        w.claim(&request.code, Instant::now())
    };
    if !accepted {
        stream.send_json(&serde_json::json!({"v":2,"type":"pair_failed","tx":ZERO_TX,"device":mac(ticket.device),"nonce":request.nonce,"code":"failed"}))?;
        return Err(invalid());
    }
    publish(ctx);
    let held_away = ctx.companion.lock().unwrap().holds(ticket.device);
    let candidate = Link {
        peer: ticket.peer,
        device: ticket.device,
        secret: random::<32>()?,
        held_away,
    };
    if !current(ctx, ticket) {
        return Err(invalid());
    }
    let mut reply = serde_json::json!({"v":2,"type":"pair_key","tx":ZERO_TX,"device":mac(ticket.device),"nonce":request.nonce,"key":hex(&candidate.secret)});
    let sent = stream.send_json(&reply);
    if let Some(serde_json::Value::String(key)) = reply.get_mut("key") {
        unsafe {
            wipe(key.as_bytes_mut());
        }
    }
    sent?;
    let result = authenticate_committing(
        stream,
        &candidate,
        || current(ctx, ticket),
        || commit(ctx, ticket, &candidate),
    );
    crate::companion_prefs::publish(ctx);
    result.map(|_| ())
}

pub(super) fn start(ctx: Ctx) {
    thread::spawn(move || loop {
        if ctx.shutting_down.load(Ordering::Acquire) {
            ctx.companion
                .lock()
                .unwrap()
                .pairing
                .finish("cancelled", true);
            return;
        }
        let ticket = ctx
            .companion
            .lock()
            .unwrap()
            .pairing
            .active
            .as_ref()
            .map(|w| w.ticket);
        let Some(ticket) = ticket else {
            thread::sleep(Duration::from_millis(100));
            continue;
        };
        let result = (|| -> io::Result<()> {
            let server = Listener::open_service(SETUP_SERVICE)?;
            while current(&ctx, ticket) {
                if let Some(mut stream) = server.accept(ticket.peer)? {
                    let attempts = {
                        let mut state = ctx.companion.lock().unwrap();
                        if !state.pairing.active.as_ref().is_some_and(|w| {
                            w.current(
                                ticket,
                                state.authorization,
                                state.generation,
                                Instant::now(),
                            )
                        }) {
                            return Err(invalid());
                        }
                        state.pairing.socket = Some((ticket.request, stream.socket.0));
                        state.pairing.active.as_ref().unwrap().attempts
                    };
                    let result = provision(&ctx, ticket, &mut stream);
                    let mut state = ctx.companion.lock().unwrap();
                    if state.pairing.socket == Some((ticket.request, stream.socket.0)) {
                        state.pairing.socket = None;
                    }
                    if result.is_err() {
                        if let Some(w) =
                            state.pairing.active.as_mut().filter(|w| {
                                w.ticket == ticket && !w.claimed && w.attempts == attempts
                            })
                        {
                            w.attempts += 1;
                        }
                    }
                    if state.pairing.active.as_ref().is_some_and(|w| {
                        w.ticket == ticket && (w.claimed || w.attempts >= ATTEMPTS)
                    }) {
                        let phase =
                            if state.pairing.active.as_ref().unwrap().since.elapsed() >= WINDOW {
                                "expired"
                            } else {
                                "failed"
                            };
                        state.pairing.finish(phase, false);
                    }
                    drop(state);
                    publish(&ctx);
                    // A successful setup socket never enters the operational session loop.
                    if result.is_ok() {
                        return Ok(());
                    }
                }
                thread::sleep(Duration::from_millis(50));
            }
            Ok(())
        })();
        let mut state = ctx.companion.lock().unwrap();
        if state
            .pairing
            .active
            .as_ref()
            .is_some_and(|w| w.ticket == ticket)
        {
            let phase = if result.is_err() {
                "failed"
            } else if state.pairing.active.as_ref().unwrap().since.elapsed() >= WINDOW {
                "expired"
            } else {
                "cancelled"
            };
            state.pairing.finish(phase, true);
        }
        drop(state);
        publish(&ctx);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn status_capture_is_serialized_through_nonblocking_publication() {
        let state = Mutex::new(State::default());
        publish_status(&state, |_| {
            assert!(
                state.try_lock().is_err(),
                "Publication must retain the state reservation until the event is enqueued"
            )
        });
        assert!(state.try_lock().is_ok());
    }
    fn ticket() -> Ticket {
        Ticket {
            request: 1,
            peer: 0x112233445566,
            device: 0xaabbccddeeff,
            authorization: 7,
            generation: 9,
        }
    }
    fn window() -> Window {
        Window {
            ticket: ticket(),
            code: *b"01234567",
            since: Instant::now(),
            attempts: 0,
            claimed: false,
        }
    }
    #[test]
    fn consent_failure_does_not_write_and_key_save_failure_preserves_old_approval_without_restoring_consent(
    ) {
        let link = |secret| Link {
            peer: ticket().peer,
            device: ticket().device,
            secret: [secret; 32],
            held_away: true,
        };
        let mut state = State {
            link: Some(link(1)),
            authorization: 7,
            generation: 9,
            ..Default::default()
        };
        let wrote = std::cell::Cell::new(false);
        assert!(save_candidate(
            &mut state,
            link(2),
            || Err(invalid()),
            |_| {
                wrote.set(true);
                Ok(())
            }
        )
        .is_err());
        assert!(!wrote.get());
        assert_eq!(state.link.as_ref().unwrap().secret, [1; 32]);
        let consent = std::cell::Cell::new(true);
        assert!(save_candidate(
            &mut state,
            link(2),
            || {
                consent.set(false);
                Ok(())
            },
            |_| {
                assert!(!consent.get());
                Err(invalid())
            }
        )
        .is_err());
        assert!(!consent.get());
        assert_eq!(state.link.as_ref().unwrap().secret, [1; 32]);
        assert_eq!((state.authorization, state.generation), (7, 9));
        save_candidate(&mut state, link(2), || Ok(()), |_| Ok(())).unwrap();
        assert_eq!(state.link.as_ref().unwrap().secret, [2; 32]);
    }
    #[test]
    fn published_pairing_status_contains_only_correlated_window_metadata() {
        let p = Pairing {
            active: Some(window()),
            ..Default::default()
        };
        let value = serde_json::to_value(p.status()).unwrap();
        assert_eq!(value["request_id"], 1);
        assert_eq!(value["peer_address"], "11:22:33:44:55:66");
        assert!(value.get("code").is_none() && value.get("key").is_none());
        assert!(value["remaining_ms"].as_u64().unwrap() <= 120_000);
    }
    #[test]
    fn code_is_one_time_and_fenced_by_epoch_pair_and_deadline() {
        let mut w = window();
        let now = w.since;
        assert!(w.current(ticket(), 7, 9, now));
        assert!(!w.current(ticket(), 8, 9, now));
        assert!(!w.current(ticket(), 7, 10, now));
        assert!(!w.current(
            Ticket {
                peer: 2,
                ..ticket()
            },
            7,
            9,
            now
        ));
        assert!(!w.current(ticket(), 7, 9, now + WINDOW));
        assert!(w.claim("01234567", now));
        assert_eq!(w.code, [0; 8]);
        assert!(!w.claim("01234567", now));
    }
    #[test]
    fn guesses_are_bounded_across_connections_and_old_cancel_does_not_touch_new_window() {
        let mut w = window();
        for _ in 0..ATTEMPTS {
            assert!(!w.claim("99999999", w.since));
        }
        assert!(!w.claim("01234567", w.since));
        let mut p = Pairing {
            active: Some(window()),
            ..Default::default()
        };
        p.cancel(9);
        assert!(p.active.is_some());
        p.cancel(1);
        assert!(p.active.is_none());
        assert_eq!(p.status().unwrap().phase, "cancelled");
        p.terminal.as_mut().unwrap().phase = "completed".into();
        p.cancel(1);
        assert_eq!(p.status().unwrap().phase, "completed");
    }
    #[test]
    fn strict_request_vectors_and_normal_protocol_separation() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/companion-pairing-v2.json"))
                .unwrap();
        let device = parse_mac(fixture["device"].as_str().unwrap()).unwrap();
        for case in fixture["cases"].as_array().unwrap().iter() {
            let line = case["frame"].as_str().unwrap().as_bytes();
            if case["stage"] == "request" {
                assert_eq!(
                    request(line, device).is_ok(),
                    case["accepted"].as_bool().unwrap(),
                    "{}",
                    case["name"]
                );
            }
            if serde_json::from_slice::<serde_json::Value>(line)
                .ok()
                .and_then(|v| v["type"].as_str().map(str::to_owned))
                .is_some_and(|kind| kind.starts_with("pair_"))
            {
                assert!(Frame::parse(line, device, None).is_err());
            }
        }
    }
}
