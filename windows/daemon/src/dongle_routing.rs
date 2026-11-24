//! Confirmed companion routing through the existing, generation-checked HID transport.
use crate::{dongle_rpc::Session, Ctx};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub known: bool,
    pub owned: bool,
    pub pending: bool,
    pub released: bool,
    pub releasing: bool,
    pub connected: bool,
    pub controls_ready: bool,
    pub busy: bool,
    pub revision: u32,
    pub control_session: u32,
    pub peer: [u8; 6],
    pub received: Instant,
    action: u8,
    nonce: u32,
}
impl Status {
    pub fn receipt(&self) -> Option<(u32, u32, u8)> {
        (self.nonce != 0 && self.action != 0).then_some((self.revision, self.nonce, self.action))
    }
    pub(crate) fn decode(b: &[u8]) -> Result<Self, &'static str> {
        if b.len() != 32 || b[0] != 1 || b[1] != 32 || b[3] > 3 || b[18] > 2 || b[19] != 0 {
            return Err("unavailable");
        }
        let field = |i| u32::from_le_bytes(b[i..i + 4].try_into().unwrap());
        let peer: [u8; 6] = b[12..18].try_into().unwrap();
        if field(4) == 0
            || peer == [0; 6]
            || peer == [255; 6]
            || (b[18] == 0) != (field(28) == 0)
            || b[2] & 8 != 0 && b[2] & (2 | 4 | 16 | 32) != 0
        {
            return Err("unavailable");
        }
        let flags = b[2];
        Ok(Self {
            known: flags & 1 != 0,
            owned: flags & 2 != 0,
            pending: flags & 4 != 0,
            released: flags & 8 != 0,
            releasing: flags & 16 != 0,
            connected: flags & 32 != 0,
            controls_ready: flags & 64 != 0,
            busy: flags & 128 != 0,
            revision: field(4),
            control_session: field(8),
            peer,
            received: Instant::now(),
            action: b[18],
            nonce: field(28),
        })
    }
    fn completes(&self, revision: u32, nonce: u32, acquire: bool) -> Result<bool, &'static str> {
        if self.revision != revision
            || self.nonce != nonce
            || self.action != if acquire { 2 } else { 1 }
        {
            return Err("superseded");
        }
        Ok(!self.busy
            && if acquire {
                self.connected && self.controls_ready && self.owned && !self.pending
            } else {
                self.released && !self.connected && !self.owned && !self.pending && !self.releasing
            })
    }
}

#[derive(Clone, Debug)]
pub struct Ticket {
    binding: Arc<Mutex<Binding>>,
    serial: String,
    peer: [u8; 6],
}
#[derive(Debug)]
struct Binding {
    session: u64,
    pair: u64,
    transition: Option<Transition>,
}
#[derive(Debug)]
struct Transition {
    revision: u32,
    nonce: u32,
    action: u8,
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
}
impl Transition {
    fn active(&self) -> bool {
        Instant::now() < self.deadline && !self.cancelled.load(Ordering::Acquire)
    }
    fn matches(&self, status: Status) -> bool {
        status.revision == self.revision
            && status.nonce == self.nonce
            && status.action == self.action
    }
}
impl Ticket {
    pub fn pending_receipt(&self) -> Option<(u32, u32, u8)> {
        self.binding
            .lock()
            .unwrap()
            .transition
            .as_ref()
            .filter(|t| t.active())
            .map(|t| (t.revision, t.nonce, t.action))
    }
    // Physical identity is only a filter; a replacement handle still needs its exact action receipt.
    pub fn same_physical(&self, other: &Self) -> bool {
        self.serial == other.serial && self.peer == other.peer
    }
    pub fn current(&self, ctx: &Ctx) -> bool {
        let d = ctx.state.lock().unwrap().dongle.clone();
        let binding = self.binding.lock().unwrap();
        if binding.transition.as_ref().is_some_and(Transition::active) {
            // Only wait through our own USB personality change; no new action is authorized here.
            return !ctx.shutting_down.load(Ordering::Acquire)
                && !d.ambiguous
                && d.serial.as_ref().is_none_or(|s| s == &self.serial)
                && d.paired_device_id.as_ref().is_none_or(|id| {
                    Some(id) == crate::dongle_protocol::paired_device_id(self.peer).as_ref()
                });
        }
        !ctx.shutting_down.load(Ordering::Acquire)
            && d.present
            && !d.ambiguous
            && d.capabilities & 512 != 0
            && d.session == binding.session
            && d.pair_token == binding.pair
            && d.serial.as_deref() == Some(self.serial.as_str())
            && d.paired_device_id == crate::dongle_protocol::paired_device_id(self.peer)
    }
    pub fn transitioning(&self, ctx: &Ctx) -> bool {
        let waiting = {
            self.binding
                .lock()
                .unwrap()
                .transition
                .as_ref()
                .is_some_and(Transition::active)
        };
        waiting && self.current(ctx)
    }
    fn observe(&self, ctx: &Ctx, status: Status) -> Result<(), &'static str> {
        let d = ctx.state.lock().unwrap().dongle.clone();
        if !d.present
            || d.ambiguous
            || d.serial.as_deref() != Some(&self.serial)
            || d.paired_device_id != crate::dongle_protocol::paired_device_id(self.peer)
            || status.peer != self.peer
        {
            return Err("superseded");
        }
        let mut binding = self.binding.lock().unwrap();
        if let Some(pending) = binding.transition.as_ref().filter(|t| t.active()) {
            if !pending.matches(status) {
                return Err("superseded");
            }
            // A reboot clears the echoed nonce; only this accepted operation can rebind USB handles.
            binding.session = d.session;
            binding.pair = d.pair_token;
        } else if binding.session != d.session || binding.pair != d.pair_token {
            return Err("superseded");
        }
        Ok(())
    }
}
// A present but unidentified dongle must inhibit fallback to native Bluetooth.
pub fn selected(ctx: &Ctx, address: u64) -> Option<Result<Ticket, &'static str>> {
    let d = &ctx.state.lock().unwrap().dongle;
    if !d.present && !d.ambiguous && d.available_devices.is_empty() {
        return None;
    }
    let peer: [u8; 6] = address.to_be_bytes()[2..].try_into().unwrap();
    if address == 0
        || address > 0xffff_ffff_ffff
        || !d.present
        || d.ambiguous
        || d.session == 0
        || d.pair_token == 0
        || d.generation == 0
        || d.capabilities & 512 == 0
        || d.paired_device_id != crate::dongle_protocol::paired_device_id(peer)
    {
        return Some(Err("unavailable"));
    }
    Some(
        d.serial
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(|serial| Ticket {
                binding: Arc::new(Mutex::new(Binding {
                    session: d.session,
                    pair: d.pair_token,
                    transition: None,
                })),
                serial: serial.clone(),
                peer,
            })
            .ok_or("unavailable"),
    )
}

pub struct Job {
    reply: mpsc::Receiver<Result<Status, &'static str>>,
    cancelled: Arc<AtomicBool>,
}
impl Job {
    pub fn poll(&self) -> Option<Result<Status, &'static str>> {
        match self.reply.try_recv() {
            Ok(Ok(status)) if status.received.elapsed() > Duration::from_secs(1) => {
                Some(Err("superseded"))
            }
            Ok(value) => Some(value),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err("failed")),
        }
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

fn exchange(
    ctx: &Ctx,
    ticket: &Ticket,
    payload: &[u8],
    allowed: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<Status, &'static str> {
    if !allowed() || !ticket.current(ctx) {
        return Err("superseded");
    }
    let d = ctx.state.lock().unwrap().dongle.clone();
    if !d.present || d.serial.as_deref() != Some(&ticket.serial) || d.capabilities & 512 == 0 {
        return Err("unavailable");
    }
    if d.pending || crate::dongle_health::busy(ctx) {
        return Err("busy");
    }
    if payload[0] != 20 {
        let binding = ticket.binding.lock().unwrap();
        if d.session != binding.session || d.pair_token != binding.pair {
            return Err("superseded");
        }
    }
    let mut rpc =
        Session::open_local(ctx.clone(), d.session, d.generation).map_err(|_| "unavailable")?;
    rpc.restrict(allowed.clone());
    if rpc.peer() != ticket.peer || !allowed() || !ticket.current(ctx) {
        return Err("superseded");
    }
    let bytes = rpc
        .feature_until(payload, Instant::now() + Duration::from_secs(2))
        .map_err(|_| "unavailable")?;
    let result = Status::decode(&bytes)?;
    if result.peer != ticket.peer || !allowed() || !ticket.current(ctx) {
        return Err("superseded");
    }
    ticket.observe(ctx, result)?;
    Ok(result)
}

pub fn probe(ctx: Ctx, ticket: Ticket) -> Job {
    start(ctx, ticket, None, || true)
}
pub fn act(
    ctx: Ctx,
    ticket: Ticket,
    expected_revision: u32,
    acquire: bool,
    current: impl Fn() -> bool + Send + Sync + 'static,
) -> Job {
    start(ctx, ticket, Some((expected_revision, acquire)), current)
}
fn start(
    ctx: Ctx,
    ticket: Ticket,
    action: Option<(u32, bool)>,
    current: impl Fn() -> bool + Send + Sync + 'static,
) -> Job {
    static NONCE: AtomicU32 = AtomicU32::new(0);
    let (send, reply) = mpsc::sync_channel(1);
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancel = cancelled.clone();
    let transition_cancel = cancelled.clone();
    let deadline = Instant::now() + Duration::from_secs(15);
    let allowed: Arc<dyn Fn() -> bool + Send + Sync> =
        Arc::new(move || !cancel.load(Ordering::Acquire) && Instant::now() < deadline && current());
    thread::spawn(move || {
        crate::volume::init();
        let result = (|| {
            let before = exchange(&ctx, &ticket, &[20], allowed.clone())?;
            let Some((expected, acquire)) = action else {
                return Ok(before);
            };
            if expected == 0 || before.revision != expected {
                return Err("superseded");
            }
            if before.busy {
                return Err("busy");
            }
            if !acquire {
                crate::companion::retain_dongle_output(&ctx, &before, allowed.clone())?;
            }
            let nonce = NONCE
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
                    Some(v.wrapping_add(1).max(1))
                })
                .unwrap()
                .wrapping_add(1)
                .max(1);
            let next_revision = expected.wrapping_add(1).max(1);
            let mut payload = [21, u8::from(acquire), 0, 0, 0, 0, 0, 0, 0, 0];
            payload[2..6].copy_from_slice(&expected.to_le_bytes());
            payload[6..].copy_from_slice(&nonce.to_le_bytes());
            ticket.binding.lock().unwrap().transition = Some(Transition {
                revision: next_revision,
                nonce,
                action: if acquire { 2 } else { 1 },
                deadline,
                cancelled: transition_cancel,
            });
            // Teardown can advance Bluetooth generation before its RPC receipt is read.
            // A fresh snapshot must echo this exact operation; never resend the action.
            let mut status = exchange(&ctx, &ticket, &payload, allowed.clone()).ok();
            while allowed() && ticket.current(&ctx) {
                if let Some(value) = status.take() {
                    if value.completes(next_revision, nonce, acquire)? {
                        ticket.binding.lock().unwrap().transition = None;
                        return Ok(value);
                    }
                }
                thread::sleep(Duration::from_millis(40));
                status = match exchange(&ctx, &ticket, &[20], allowed.clone()) {
                    Ok(value) => Some(value),
                    Err("unavailable" | "busy") => None,
                    Err(error) => return Err(error),
                };
            }
            Err("superseded")
        })();
        let _ = send.send(result);
    });
    Job { reply, cancelled }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot() -> [u8; 32] {
        let mut b = [0; 32];
        b[0] = 1;
        b[1] = 32;
        b[4] = 1;
        b[12..18].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        b
    }
    #[test]
    fn rejects_missing_identity_and_malformed_receipts() {
        let b = snapshot();
        assert!(Status::decode(&b).is_ok());
        for (at, value) in [(0, 2), (1, 31), (3, 4), (4, 0), (18, 3), (19, 1), (28, 1)] {
            let mut invalid = b;
            invalid[at] = value;
            assert!(Status::decode(&invalid).is_err());
        }
        let mut invalid = b;
        invalid[12..18].fill(0);
        assert!(Status::decode(&invalid).is_err());
        assert!(Status::decode(&b[..31]).is_err());
    }
    #[test]
    fn only_exact_confirmed_action_completes_after_generation_changes() {
        let mut b = snapshot();
        b[18] = 1;
        b[28] = 7;
        let mut s = Status::decode(&b).unwrap();
        assert_eq!(s.completes(1, 7, false), Ok(false));
        s.released = true;
        assert_eq!(s.completes(1, 7, false), Ok(true));
        s.control_session += 1;
        assert_eq!(s.completes(1, 7, false), Ok(true));
        assert_eq!(s.completes(2, 7, false), Err("superseded"));
        assert_eq!(s.completes(1, 8, false), Err("superseded"));
        assert_eq!(s.completes(1, 7, true), Err("superseded"));
        s.nonce = 0;
        s.action = 0;
        assert_eq!(s.completes(1, 7, false), Err("superseded"));
        b[18] = 2;
        b[2] = 2 | 32 | 64;
        let mut s = Status::decode(&b).unwrap();
        assert_eq!(s.completes(1, 7, true), Ok(true));
        s.pending = true;
        assert_eq!(s.completes(1, 7, true), Ok(false));
    }
    #[test]
    fn local_ticket_never_survives_usb_replacement_pair_change_or_shutdown() {
        let ctx = Ctx::for_test();
        assert!(selected(&ctx, 0x010203040506).is_none());
        {
            let mut s = ctx.state.lock().unwrap();
            let d = &mut s.dongle;
            d.present = true;
            d.session = 1;
            d.pair_token = 1;
            d.generation = 2;
            d.capabilities = 16 | 512;
            d.serial = Some("pico".into());
            d.paired_device_id = Some("010203040506".into());
        }
        let ticket = selected(&ctx, 0x010203040506).unwrap().unwrap();
        assert!(ticket.current(&ctx));
        ctx.state.lock().unwrap().dongle.capabilities = 16;
        assert!(selected(&ctx, 0x010203040506).unwrap().is_err());
        assert!(!ticket.current(&ctx));
        ctx.state.lock().unwrap().dongle.capabilities = 16 | 512;
        ctx.state.lock().unwrap().dongle.generation = 3;
        assert!(ticket.current(&ctx));
        ctx.state.lock().unwrap().dongle.pair_token = 2;
        assert!(!ticket.current(&ctx));
        assert!(selected(&ctx, 0x010203040507).unwrap().is_err());
        ctx.state.lock().unwrap().dongle.pair_token = 1;
        ctx.state.lock().unwrap().dongle.serial = Some("replacement".into());
        assert!(!ticket.current(&ctx));
        ctx.state.lock().unwrap().dongle.serial = Some("pico".into());
        ctx.shutting_down.store(true, Ordering::Release);
        assert!(!ticket.current(&ctx));
    }
    #[test]
    fn revoked_operation_finishes_without_opening_any_usb_device() {
        let ctx = Ctx::for_test();
        let ticket = Ticket {
            binding: Arc::new(Mutex::new(Binding {
                session: 1,
                pair: 1,
                transition: None,
            })),
            serial: "absent".into(),
            peer: [1, 2, 3, 4, 5, 6],
        };
        let job = act(ctx, ticket, 1, false, || false);
        assert_eq!(
            job.reply.recv_timeout(Duration::from_secs(1)).unwrap(),
            Err("superseded")
        );
    }
    #[test]
    fn dropping_a_job_revokes_its_worker_permission() {
        let (_send, reply) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        let job = Job {
            reply,
            cancelled: cancelled.clone(),
        };
        drop(job);
        assert!(cancelled.load(Ordering::Acquire));
    }
    #[test]
    fn a_queued_receipt_does_not_become_fresh_when_the_coordinator_wakes() {
        let (send, reply) = mpsc::sync_channel(1);
        let mut status = Status::decode(&snapshot()).unwrap();
        status.received = Instant::now() - Duration::from_secs(2);
        send.send(Ok(status)).unwrap();
        let job = Job {
            reply,
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        assert_eq!(job.poll(), Some(Err("superseded")));
    }
    #[test]
    fn usb_personality_change_requires_exact_accepted_operation_before_rebinding() {
        let ctx = Ctx::for_test();
        {
            let mut s = ctx.state.lock().unwrap();
            let d = &mut s.dongle;
            d.present = true;
            d.session = 1;
            d.pair_token = 1;
            d.generation = 1;
            d.capabilities = 512;
            d.serial = Some("pico".into());
            d.paired_device_id = Some("010203040506".into());
        }
        let ticket = selected(&ctx, 0x010203040506).unwrap().unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        ticket.binding.lock().unwrap().transition = Some(Transition {
            revision: 2,
            nonce: 7,
            action: 1,
            deadline: Instant::now() + Duration::from_secs(2),
            cancelled: cancelled.clone(),
        });
        {
            let mut s = ctx.state.lock().unwrap();
            let d = &mut s.dongle;
            d.present = false;
            d.paired_device_id = None;
            d.session = 2;
            d.pair_token = 2;
        }
        assert!(ticket.transitioning(&ctx));
        let mut b = snapshot();
        b[4] = 2;
        b[18] = 1;
        b[28] = 7;
        b[2] = 8;
        let status = Status::decode(&b).unwrap();
        assert_eq!(ticket.observe(&ctx, status), Err("superseded"));
        {
            let mut s = ctx.state.lock().unwrap();
            let d = &mut s.dongle;
            d.present = true;
            d.paired_device_id = Some("010203040506".into());
        }
        let mut reboot = status;
        reboot.nonce = 0;
        reboot.action = 0;
        assert_eq!(ticket.observe(&ctx, reboot), Err("superseded"));
        assert_eq!(ticket.binding.lock().unwrap().session, 1);
        assert_eq!(ticket.observe(&ctx, status), Ok(()));
        assert_eq!(ticket.binding.lock().unwrap().session, 2);
        cancelled.store(true, Ordering::Release);
        assert!(!ticket.transitioning(&ctx));
        assert!(ticket.current(&ctx));
        ctx.state.lock().unwrap().dongle.serial = Some("different".into());
        assert!(!ticket.current(&ctx));
    }
}
