//! Bounded HID transactions. Only the hearing coordinator can submit these packets.
use crate::{dongle_hid as hid, Ctx};
use std::{
    thread,
    time::{Duration, Instant},
};

const HEADER: [u8; 7] = [3, b'A', b'P', b'D', b'G', 1, 64];
const LIMIT: usize = 512;
const CHUNK: usize = 42;
const LEASE_CAPABILITY: u16 = 0x20;

fn lease_request(action: u8, token: u32) -> [u8; 6] {
    let mut request = [10, action, 0, 0, 0, 0];
    request[2..].copy_from_slice(&token.to_le_bytes());
    request
}

fn lease_token(reply: &[u8]) -> Result<u32, String> {
    if reply.len() != 5 || reply[0] != 1 {
        return Err("Unsupported dongle settings lease.".into());
    }
    let token = u32::from_le_bytes(reply[1..].try_into().unwrap());
    if token == 0 {
        return Err("Dongle settings lease is unavailable.".into());
    }
    Ok(token)
}

fn lock_transaction(
    ctx: &Ctx,
    deadline: Option<Instant>,
) -> Result<std::sync::MutexGuard<'_, ()>, String> {
    if let Some(deadline) = deadline {
        loop {
            match ctx.dongle.rpc.try_lock() {
                Ok(transaction) => return Ok(transaction),
                Err(std::sync::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(std::sync::TryLockError::WouldBlock) => {
                    return Err("Dongle shutdown cleanup timed out.".into());
                }
                Err(error) => panic!("Dongle transaction lock poisoned: {error}"),
            }
        }
    }
    Ok(ctx.dongle.rpc.lock().unwrap())
}

pub struct Cached {
    pub age_ms: u32,
    pub revision: u32,
    pub packet: Vec<u8>,
}
fn cached_report(result: &[u8]) -> Result<Option<Cached>, String> {
    if result.is_empty() {
        return Ok(None);
    }
    if !(9..=LIMIT).contains(&result.len()) {
        return Err("Incomplete dongle report.".into());
    }
    let revision = u32::from_le_bytes(result[4..8].try_into().unwrap());
    if revision == 0 {
        return Err("Dongle report has no reception revision.".into());
    }
    Ok(Some(Cached {
        age_ms: u32::from_le_bytes(result[..4].try_into().unwrap()),
        revision,
        packet: result[8..].to_vec(),
    }))
}
struct Reply {
    op: u8,
    generation: u32,
    sequence: u32,
    total: usize,
    offset: usize,
    phase: u8,
    data: Vec<u8>,
}
fn parse(bytes: &[u8; 64]) -> Result<Reply, String> {
    let total = u16::from_le_bytes([bytes[16], bytes[17]]) as usize;
    let offset = u16::from_le_bytes([bytes[18], bytes[19]]) as usize;
    let size = bytes[20] as usize;
    if bytes[..7] != HEADER
        || !matches!(bytes[7], 0..=3 | 7)
        || bytes[21] > 4
        || total > LIMIT
        || offset > total
        || size > CHUNK
        || size > total - offset
        || bytes[22 + size..].iter().any(|b| *b != 0)
    {
        return Err("Unsupported dongle settings response.".into());
    }
    Ok(Reply {
        op: bytes[7],
        generation: u32::from_le_bytes(bytes[8..12].try_into().unwrap()),
        sequence: u32::from_le_bytes(bytes[12..16].try_into().unwrap()),
        total,
        offset,
        phase: bytes[21],
        data: bytes[22..22 + size].to_vec(),
    })
}
fn packet(
    op: u8,
    generation: u32,
    sequence: u32,
    total: usize,
    offset: usize,
    data: &[u8],
) -> Result<[u8; 64], String> {
    if !(1..=7).contains(&op)
        || sequence == 0
        || total > LIMIT
        || data.len() > CHUNK
        || offset > LIMIT
    {
        return Err("Invalid dongle settings request.".into());
    }
    let mut b = [0; 64];
    b[..7].copy_from_slice(&HEADER);
    b[7] = op;
    b[8..12].copy_from_slice(&generation.to_le_bytes());
    b[12..16].copy_from_slice(&sequence.to_le_bytes());
    b[16..18].copy_from_slice(&(total as u16).to_le_bytes());
    b[18..20].copy_from_slice(&(offset as u16).to_le_bytes());
    b[20] = data.len() as u8;
    b[22..22 + data.len()].copy_from_slice(data);
    Ok(b)
}
pub struct Session {
    ctx: Ctx,
    usb: std::sync::Arc<hid::Device>,
    session: u64,
    generation: u32,
    sequence: u32,
    peer: [u8; 6],
    active: bool,
    local: bool,
    cleanup_deadline: Option<Instant>,
    lease_supported: bool,
    lease_token: Option<u32>,
    lease_failed: bool,
}
impl Session {
    pub fn open(ctx: Ctx, session: u64, generation: u32) -> Result<Self, String> {
        Self::open_kind(ctx, session, generation, false)
    }
    pub fn open_local(ctx: Ctx, session: u64, generation: u32) -> Result<Self, String> {
        Self::open_kind(ctx, session, generation, true)
    }
    fn open_kind(ctx: Ctx, session: u64, generation: u32, local: bool) -> Result<Self, String> {
        {
            let state = ctx.state.lock().unwrap();
            let d = &state.dongle;
            if !d.present
                || d.ambiguous
                || d.session != session
                || d.generation != generation
                || (!local && !d.controls_ready)
                || d.capabilities & if local { 16 } else { 8 } == 0
            {
                return Err("Reconnect the dongle's AirPods, then try again.".into());
            }
        }
        let usb = crate::dongle::research_device(&ctx, session, generation)
            .ok_or("Dongle connection changed.")?;
        Self::connect(ctx, usb, session, generation, local, None)
    }
    fn connect(
        ctx: Ctx,
        usb: std::sync::Arc<hid::Device>,
        session: u64,
        generation: u32,
        local: bool,
        cleanup_deadline: Option<Instant>,
    ) -> Result<Self, String> {
        let report = usb.read().map_err(|_| "Could not read dongle identity.")?;
        if report.state.generation != generation
            || report.state.capabilities & if local { 16 } else { 8 } == 0
            || (!local && !report.state.controls_ready)
        {
            return Err("Dongle connection changed. Read the settings again.".into());
        }
        let initial = parse(
            &usb.read_feature(3)
                .map_err(|_| "Dongle settings firmware is unavailable.")?,
        )?;
        if initial.generation != generation || matches!(initial.phase, 1 | 2) {
            return Err("Dongle settings are busy or the connection changed.".into());
        }
        let result = Self {
            ctx,
            usb,
            session,
            generation,
            sequence: initial.sequence,
            peer: report.address,
            active: false,
            local,
            cleanup_deadline,
            lease_supported: report.state.capabilities & LEASE_CAPABILITY != 0,
            lease_token: None,
            lease_failed: false,
        };
        if !result.current() {
            return Err("Dongle connection changed.".into());
        }
        Ok(result)
    }
    // The USB owner retains the handle until this one bounded Stop has finished.
    pub fn stop_finder_at_shutdown(
        ctx: Ctx,
        usb: std::sync::Arc<hid::Device>,
        session: u64,
        generation: u32,
    ) -> Result<(), String> {
        if !ctx.shutting_down.load(std::sync::atomic::Ordering::Acquire) {
            return Err("Dongle shutdown has not started.".into());
        }
        let mut rpc = Self::connect(
            ctx,
            usb,
            session,
            generation,
            true,
            Some(Instant::now() + Duration::from_secs(1)),
        )?;
        if !rpc.feature(&[8])?.is_empty() {
            return Err("Unsupported finder response.".into());
        }
        Ok(())
    }
    pub fn peer(&self) -> [u8; 6] {
        self.peer
    }
    pub fn extended(&self) -> bool {
        self.ctx.state.lock().unwrap().dongle.capabilities & 16 != 0
    }
    pub fn current(&self) -> bool {
        if self.cleanup_deadline.map_or_else(
            || {
                self.ctx
                    .shutting_down
                    .load(std::sync::atomic::Ordering::Acquire)
            },
            |deadline| Instant::now() >= deadline,
        ) {
            return false;
        }
        let s = self.ctx.state.lock().unwrap();
        let d = &s.dongle;
        d.present
            && !d.ambiguous
            && d.session == self.session
            && d.generation == self.generation
            && (self.local || d.controls_ready)
    }
    fn send(&self, op: u8, total: usize, offset: usize, data: &[u8]) -> Result<(), String> {
        if !self.current() {
            return Err("Dongle connection changed.".into());
        }
        self.usb
            .write(packet(
                op,
                self.generation,
                self.sequence,
                total,
                offset,
                data,
            )?)
            .map_err(|_| "Dongle settings transfer failed; the request was not retried.".into())
    }
    fn wait(
        &self,
        op: u8,
        deadline: Instant,
        accept: impl Fn(&Reply) -> bool,
    ) -> Result<Reply, String> {
        loop {
            if !self.current() {
                return Err("Dongle connection changed.".into());
            }
            if Instant::now() >= deadline
                || self
                    .cleanup_deadline
                    .is_some_and(|end| Instant::now() >= end)
            {
                return Err("Dongle settings timed out; the request was not retried.".into());
            }
            let reply = parse(
                &self
                    .usb
                    .read_feature(3)
                    .map_err(|_| "Could not read dongle settings response.")?,
            )?;
            if reply.generation != self.generation {
                return Err("Dongle connection changed.".into());
            }
            if reply.sequence == self.sequence && reply.op == op {
                if reply.phase == 4 {
                    return Err(match reply.data.first() {
                        Some(3) => "Dongle connection changed.",
                        Some(5) => {
                            "AirPods settings timed out. Reconnect, then read settings again."
                        }
                        Some(8) => "Dongle settings request was cancelled.",
                        _ => "AirPods did not complete the settings request. Read settings again.",
                    }
                    .into());
                }
                if accept(&reply) {
                    return Ok(reply);
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn transact(&mut self, op: u8, payload: &[u8]) -> Result<Vec<u8>, String> {
        if payload.is_empty() || payload.len() > LIMIT {
            return Err("Invalid dongle settings payload length.".into());
        }
        let ctx = self.ctx.clone();
        let _transaction = lock_transaction(&ctx, self.cleanup_deadline)?;
        self.ensure_lease(matches!(op, 1 | 2))?;
        self.transact_locked(op, payload)
    }
    fn ensure_lease(&mut self, raw: bool) -> Result<(), String> {
        if self.lease_failed {
            return Err("Dongle settings lease ended. Read the settings again.".into());
        }
        if let Some(token) = self.lease_token {
            // Keep one lease through read/modify/verify and cached mode-confirmation polling.
            // A rejected renewal must never acquire a replacement halfway through an edit.
            self.lease_failed = true;
            if !self
                .transact_locked(7, &lease_request(1, token))?
                .is_empty()
            {
                return Err("Unsupported dongle settings lease renewal.".into());
            }
            self.lease_failed = false;
        } else if raw {
            if !self.lease_supported {
                return Err(
                    "Update the dongle firmware before reading or changing hearing data.".into(),
                );
            }
            self.lease_failed = true;
            let reply = self.transact_locked(7, &lease_request(0, 0))?;
            self.lease_token = Some(lease_token(&reply)?);
            self.lease_failed = false;
        }
        Ok(())
    }
    pub fn acquire_settings_lease(&mut self) -> Result<(), String> {
        let ctx = self.ctx.clone();
        let _transaction = lock_transaction(&ctx, self.cleanup_deadline)?;
        self.ensure_lease(true)
    }
    fn transact_locked(&mut self, op: u8, payload: &[u8]) -> Result<Vec<u8>, String> {
        if !self.current() {
            return Err("Dongle connection changed.".into());
        }
        if self.active {
            return Err("The previous dongle request has not finished cancelling.".into());
        }
        // Media permission checks share this endpoint with the health worker.
        let initial = parse(
            &self
                .usb
                .read_feature(3)
                .map_err(|_| "Could not read dongle settings response.")?,
        )?;
        if initial.generation != self.generation || matches!(initial.phase, 1 | 2) {
            return Err("Dongle settings are busy or the connection changed.".into());
        }
        self.sequence = initial.sequence.wrapping_add(1).max(1);
        self.active = true;
        let result = (|| {
            let deadline = Instant::now() + Duration::from_secs(4);
            for (index, data) in payload.chunks(CHUNK).enumerate() {
                let offset = index * CHUNK;
                self.send(op, payload.len(), offset, data)?;
                self.wait(op, deadline, |r| {
                    r.phase == 1 && r.total == payload.len() && r.offset == offset + data.len()
                })?;
            }
            self.send(6, 0, 0, &[])?;
            let mut reply = self.wait(op, Instant::now() + Duration::from_secs(4), |r| {
                r.phase == 3 && r.offset == 0
            })?;
            let total = reply.total;
            let mut output = Vec::with_capacity(total);
            loop {
                if reply.total != total
                    || reply.offset != output.len()
                    || (reply.data.is_empty() && output.len() < total)
                {
                    return Err("Dongle returned an incomplete settings response.".into());
                }
                output.extend_from_slice(&reply.data);
                if output.len() == total {
                    return Ok(output);
                }
                self.send(4, 0, output.len(), &[])?;
                reply = self.wait(op, Instant::now() + Duration::from_secs(1), |r| {
                    r.phase == 3 && r.offset == output.len()
                })?;
            }
        })();
        if result.is_err() {
            self.cancel();
        } else {
            self.active = false;
        }
        result
    }
    fn release_lease(&mut self) {
        let Some(token) = self.lease_token.take() else {
            self.cancel();
            return;
        };
        self.lease_failed = true;
        let previous = self.cleanup_deadline;
        self.cleanup_deadline =
            Some(previous.unwrap_or_else(|| Instant::now() + Duration::from_secs(1)));
        let ctx = self.ctx.clone();
        let _ = (|| -> Result<(), String> {
            let _transaction = lock_transaction(&ctx, self.cleanup_deadline)?;
            self.cancel();
            // Cancel is queued; wait for its receipt before staging the release.
            while self.current() {
                let reply = parse(
                    &self
                        .usb
                        .read_feature(3)
                        .map_err(|_| "Could not release dongle settings lease.")?,
                )?;
                if reply.generation != self.generation {
                    break;
                }
                if !matches!(reply.phase, 1 | 2) {
                    self.transact_locked(7, &lease_request(2, token))?;
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
            Ok(())
        })();
        self.cleanup_deadline = previous;
    }
    pub fn exchange(&mut self, pdu: &[u8]) -> Result<Vec<u8>, String> {
        self.transact(1, pdu)
    }
    pub fn send_aacp(&mut self, pdu: &[u8]) -> Result<(), String> {
        self.transact(2, pdu).map(|_| ())
    }
    pub fn feature(&mut self, payload: &[u8]) -> Result<Vec<u8>, String> {
        self.transact(7, payload)
    }
    pub fn cached(&mut self, slot: u8) -> Result<Option<Cached>, String> {
        if !(1..=5).contains(&slot) {
            return Err("Unknown dongle report.".into());
        }
        cached_report(&self.transact(3, &[slot])?)
    }
    pub fn cancel(&mut self) {
        if self.active && self.current() && self.send(5, 0, 0, &[]).is_ok() {
            // SetFeature only queues Cancel. Its receipt must precede cleanup traffic.
            let deadline = Instant::now() + Duration::from_secs(1);
            while self.current() && Instant::now() < deadline {
                let Ok(bytes) = self.usb.read_feature(3) else {
                    break;
                };
                let Ok(reply) = parse(&bytes) else { break };
                if reply.generation != self.generation {
                    break;
                }
                if reply.sequence == self.sequence && reply.phase == 4 && reply.data == [8] {
                    self.active = false;
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.release_lease();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_lease_wire_requires_exact_version_and_a_nonzero_token() {
        assert_eq!(lease_request(0, 0), [10, 0, 0, 0, 0, 0]);
        assert_eq!(
            lease_request(2, 0x7654_3210),
            [10, 2, 0x10, 0x32, 0x54, 0x76]
        );
        assert_eq!(
            lease_token(&[1, 0x10, 0x32, 0x54, 0x76]).unwrap(),
            0x7654_3210
        );
        for reply in [
            vec![],
            vec![1],
            vec![1, 0, 0, 0, 0],
            vec![2, 1, 0, 0, 0],
            vec![1, 1, 0, 0, 0, 0],
        ] {
            assert!(lease_token(&reply).is_err());
        }
    }
    #[test]
    fn cache_envelope_keeps_report_age_and_revision_separate() {
        assert!(cached_report(&[]).unwrap().is_none());
        for size in 1..=8 {
            assert!(cached_report(&vec![1; size]).is_err());
        }
        assert!(cached_report(&[0; 9]).is_err());
        assert!(cached_report(&[1; 513]).is_err());
        let bytes = [
            123u32.to_le_bytes().as_slice(),
            u32::MAX.to_le_bytes().as_slice(),
            &[4, 0, 4, 0, 6],
        ]
        .concat();
        let cached = cached_report(&bytes).unwrap().unwrap();
        assert_eq!(cached.age_ms, 123);
        assert_eq!(cached.revision, u32::MAX);
        assert_eq!(cached.packet, [4, 0, 4, 0, 6]);
    }
    #[test]
    fn reports_reject_unknown_and_inconsistent_bounds() {
        let mut b = [0; 64];
        b[..7].copy_from_slice(&HEADER);
        assert!(parse(&b).is_ok());
        for (i, v) in [(0, 2), (5, 2), (7, 9), (21, 5), (20, 43), (63, 1), (18, 1)] {
            let mut bad = b;
            bad[i] = v;
            assert!(parse(&bad).is_err(), "{i}");
        }
        b[21] = 3;
        b[16] = 255;
        b[17] = 1;
        b[20] = 42;
        assert!(parse(&b).is_ok());
        b[17] = 2;
        assert!(parse(&b).is_err());
    }
    #[test]
    fn fragments_pin_generation_and_do_not_include_unwritten_bytes() {
        for n in 1..=512 {
            let payload = vec![0x77; n];
            for (i, chunk) in payload.chunks(CHUNK).enumerate() {
                let b = packet(1, 9, 11, n, i * CHUNK, chunk).unwrap();
                assert_eq!(&b[8..12], &9u32.to_le_bytes());
                assert_eq!(&b[12..16], &11u32.to_le_bytes());
                assert_eq!(&b[22..22 + chunk.len()], chunk);
                assert!(b[22 + chunk.len()..].iter().all(|b| *b == 0));
            }
        }
        assert!(packet(1, 9, 0, 1, 0, &[1]).is_err());
        assert!(packet(1, 9, 1, 513, 0, &[1]).is_err());
    }
}
