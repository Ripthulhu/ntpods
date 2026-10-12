//! AirPods ATT settings, including Loud Sound Reduction and hearing assistance.
//! Hearing assistance: enable it over AAP (control commands 0x2C /
//! 0x33), switch to Transparency, then write the full settings to the ATT/GATT
//! (PSM 0x001F, handle 0x2A) via a read-modify-write. Layout + semantics are ported
//! from the Linux `hearing-aid-adjustments.py`: an 8-band audiogram (hearing loss in
//! dB HL) per ear, per-ear amplification, tone, conversation-boost, ambient-noise-
//! reduction and own-voice — all little-endian f32 at fixed offsets.

use crate::aap;
use crate::driver::Driver;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::{
    thread,
    time::{Duration, Instant},
};
use windows::Win32::Foundation::ERROR_DEVICE_NOT_CONNECTED;

// One ATT transaction at a time. The saved value identifies the enabled session.
static HEARING_STATE: Mutex<Option<u64>> = Mutex::new(None);
static HEARING_SESSION: AtomicU64 = AtomicU64::new(0);
// Coalesce rapid settings edits before they reach the shared ATT channel.
static HEARING_GEN: AtomicU64 = AtomicU64::new(0);
static LOUD_SOUND_GEN: AtomicU64 = AtomicU64::new(0);
static ATT_FAILED_SESSION: AtomicU64 = AtomicU64::new(u64::MAX);

fn require_clean_att_session(session: u64) -> Result<(), String> {
    if ATT_FAILED_SESSION.load(Ordering::SeqCst) == session {
        Err("Reconnect your headphones, then try again.".into())
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub struct LoudSoundToken {
    generation: u64,
    session: u64,
}

impl LoudSoundToken {
    pub fn is_current(self) -> bool {
        self.generation == LOUD_SOUND_GEN.load(Ordering::SeqCst)
            && self.session == HEARING_SESSION.load(Ordering::SeqCst)
    }
}

pub fn loud_sound_token() -> LoudSoundToken {
    LoudSoundToken {
        generation: LOUD_SOUND_GEN
            .fetch_add(1, Ordering::SeqCst)
            .wrapping_add(1),
        session: HEARING_SESSION.load(Ordering::SeqCst),
    }
}

pub fn is_current(gen: u64) -> bool {
    HEARING_GEN.load(Ordering::SeqCst) == gen
}

pub fn next_gen() -> u64 {
    HEARING_GEN.fetch_add(1, Ordering::SeqCst).wrapping_add(1)
}

/// Session teardown cannot wait for an ATT worker to release its transaction lock.
pub fn invalidate_session() {
    HEARING_SESSION.fetch_add(1, Ordering::SeqCst);
    HEARING_GEN.fetch_add(1, Ordering::SeqCst);
    LOUD_SOUND_GEN.fetch_add(1, Ordering::SeqCst);
}

// AAP hearing-assist enable/disable (0x09 control commands 0x2C / 0x33).
const HA_ON_2C: [u8; 11] = [
    0x04, 0x00, 0x04, 0x00, 0x09, 0x00, 0x2C, 0x01, 0x01, 0x00, 0x00,
];
const HA_ON_33: [u8; 11] = [
    0x04, 0x00, 0x04, 0x00, 0x09, 0x00, 0x33, 0x01, 0x00, 0x00, 0x00,
];
const HA_OFF_2C: [u8; 11] = [
    0x04, 0x00, 0x04, 0x00, 0x09, 0x00, 0x2C, 0x01, 0x02, 0x00, 0x00,
];
const HA_OFF_33: [u8; 11] = [
    0x04, 0x00, 0x04, 0x00, 0x09, 0x00, 0x33, 0x02, 0x00, 0x00, 0x00,
];

const H_SETTINGS: u16 = 0x002A; // hearing-aid settings characteristic
const H_CCCD: u16 = 0x002B; // its client-config descriptor

// ATT responses have their own receive queue; bound how long this settings worker waits.
const ATT_TIMEOUT_MS: u32 = 900;

// f32 offsets into the settings value (bytes after the ATT opcode) — mirror the
// Linux reference exactly.
const OFF_MODE: usize = 2;
const OFF_LEFT_EQ: usize = 4; // + i*4, 8 bands
const OFF_LEFT_AMP: usize = 36;
const OFF_LEFT_TONE: usize = 40;
const OFF_LEFT_CONV: usize = 44;
const OFF_LEFT_ANR: usize = 48;
const OFF_RIGHT_EQ: usize = 52; // + i*4, 8 bands
const OFF_RIGHT_AMP: usize = 84;
const OFF_RIGHT_TONE: usize = 88;
const OFF_RIGHT_CONV: usize = 92;
const OFF_RIGHT_ANR: usize = 96;
const OFF_OWN_VOICE: usize = 100;

fn put_f32(buf: &mut [u8], off: usize, v: f32) {
    if off + 4 <= buf.len() {
        buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }
}

fn att_read_req(handle: u16) -> [u8; 3] {
    [0x0A, (handle & 0xff) as u8, (handle >> 8) as u8]
}

fn att_write_pdu(handle: u16, value: &[u8]) -> Vec<u8> {
    let mut p = vec![0x12u8, (handle & 0xff) as u8, (handle >> 8) as u8];
    p.extend_from_slice(value);
    p
}

// LibrePods main efccc449 ATTManager.kt + AccessibilitySettingsScreen: handle
// 0x1B holds one byte, 0 = off / 1 = on. It needs no hearing-aid enable command.
const H_LOUD_SOUND_REDUCTION: u16 = 0x001B;

fn loud_sound_value(response: &[u8]) -> Result<bool, String> {
    match response {
        [0x0B, value @ (0 | 1)] => Ok(*value == 1),
        _ => Err("Could not read Loud Sound Reduction. Reconnect the AirPods to retry.".into()),
    }
}

fn loud_sound_transaction(
    requested: Option<bool>,
    mut exchange: impl FnMut(&[u8]) -> Result<Vec<u8>, String>,
) -> Result<bool, String> {
    let read = att_read_req(H_LOUD_SOUND_REDUCTION);
    let current = loud_sound_value(&exchange(&read)?)?;
    let Some(requested) = requested.filter(|value| *value != current) else {
        return Ok(current);
    };
    let response = exchange(&att_write_pdu(
        H_LOUD_SOUND_REDUCTION,
        &[u8::from(requested)],
    ))?;
    if response != [0x13] {
        return Err(
            "Loud Sound Reduction was not acknowledged. Reconnect the AirPods to retry.".into(),
        );
    }
    // A write acknowledgment confirms receipt; only readback confirms the value.
    loud_sound_value(&exchange(&read)?)
}

fn att_queue_drained(received: std::io::Result<usize>) -> Result<bool, String> {
    match received {
        Ok(length) => Ok(length == 0),
        // ATT opens lazily on the first send; there is no queue to drain yet.
        Err(error) if error.raw_os_error() == Some(ERROR_DEVICE_NOT_CONNECTED.0 as i32) => Ok(true),
        Err(_) => Err("Could not read the AirPods settings channel.".into()),
    }
}

fn drain_att(
    mut receive: impl FnMut() -> std::io::Result<usize>,
    current: impl Fn() -> bool,
) -> Result<(), String> {
    for _ in 0..16 {
        if !current() {
            return Err("AirPods connection changed.".into());
        }
        let drained = att_queue_drained(receive())?;
        if !current() {
            return Err("AirPods connection changed.".into());
        }
        if drained {
            return Ok(());
        }
    }
    Err("AirPods settings channel is busy. Reconnect the AirPods to retry.".into())
}

fn att_response(
    mut receive: impl FnMut(u32) -> Result<Vec<u8>, String>,
    current: impl Fn() -> bool,
) -> Result<Vec<u8>, String> {
    let deadline = Instant::now() + Duration::from_millis(ATT_TIMEOUT_MS.into());
    for _ in 0..16 {
        if !current() {
            return Err("AirPods connection changed.".into());
        }
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .as_millis() as u32;
        if remaining == 0 {
            break;
        }
        let response = receive(remaining)?;
        if !current() {
            return Err("AirPods connection changed.".into());
        }
        // Hearing assistance can notify on this same ATT channel between replies.
        if matches!(response.as_slice(), [0x1B, low, high, ..] if *low != 0 || *high != 0) {
            continue;
        }
        return Ok(response);
    }
    Err("AirPods settings response timed out. Reconnect the AirPods to retry.".into())
}

pub fn loud_sound_reduction(
    drv: &Driver,
    token: LoudSoundToken,
    requested: Option<bool>,
) -> Result<Option<bool>, String> {
    let mut transaction = HEARING_STATE.lock().unwrap_or_else(|p| p.into_inner());
    if !token.is_current() {
        return Ok(None);
    }
    require_clean_att_session(token.session)?;
    let result = loud_sound_transaction(requested, |request| {
        if !token.is_current() {
            return Err("AirPods connection changed.".into());
        }
        // Discard queued notifications/late replies before starting a new request.
        let mut response = [0u8; 512];
        drain_att(|| drv.att_recv(0, &mut response), || token.is_current())?;
        drv.att_send(request)
            .map_err(|_| "Could not reach the AirPods settings channel.")?;
        att_response(
            |timeout| {
                let length = drv
                    .att_recv(timeout, &mut response)
                    .map_err(|_| "Could not read the AirPods settings channel.")?;
                Ok(response[..length].to_vec())
            },
            || token.is_current(),
        )
    });
    if !token.is_current() {
        return Ok(None);
    }
    if result.is_err() {
        // ATT read responses have no handle: after a timeout a late reply cannot
        // be attributed safely. Retry only on a new connection, whose queue resets.
        ATT_FAILED_SESSION.store(token.session, Ordering::SeqCst);
        *transaction = None;
    }
    result.map(Some).map_err(|_| {
        "Could not read or change Loud Sound Reduction. Reconnect your headphones, then try again.".into()
    })
}

/// Wake the buds' hearing-aid ATT server (dormant until enabled), switch to
/// Transparency (mode 3) so ambient sound passes through to be amplified, and enable
/// notifications on the settings CCCD. Only needed on the off→on transition (or to
/// recover a dropped ATT channel) — NOT on every slider tweak, since re-sending the
/// AAP enable makes the buds reset their ATT server and drop our channel.
fn enable_handshake(drv: &Driver) -> Result<(), String> {
    drv.send(&HA_ON_2C).map_err(|e| e.to_string())?;
    thread::sleep(Duration::from_millis(300));
    drv.send(&aap::anc_command(3)).map_err(|e| e.to_string())?;
    thread::sleep(Duration::from_millis(200));
    drv.send(&HA_ON_33).map_err(|e| e.to_string())?;
    thread::sleep(Duration::from_millis(900));

    let mut b = [0u8; 512];
    drv.att_send(&att_write_pdu(H_CCCD, &[0x01, 0x00]))
        .map_err(|e| e.to_string())?;
    let n = drv
        .att_recv(ATT_TIMEOUT_MS, &mut b)
        .map_err(|e| e.to_string())?;
    require_write_response(&b[..n])
}

fn require_write_response(response: &[u8]) -> Result<(), String> {
    if response == [0x13] {
        Ok(())
    } else {
        Err("AirPods did not acknowledge the hearing-settings write.".into())
    }
}

/// Read-modify-write the settings characteristic over the (already-open) ATT channel.
/// This is the only step a settings tweak needs — no AAP re-enable, so the ATT
/// channel stays up.
#[allow(clippy::too_many_arguments)]
fn write_settings(
    drv: &Driver,
    left_eq: &[f32],
    right_eq: &[f32],
    amplification: f32,
    balance: f32,
    tone: f32,
    conv_boost: bool,
    anr: f32,
    own_voice: f32,
) -> Result<String, String> {
    let mut b = [0u8; 512];

    // 1) Read the current settings value (read-modify-write).
    drv.att_send(&att_read_req(H_SETTINGS))
        .map_err(|e| format!("ATT read request: {e}"))?;
    let n = drv
        .att_recv(ATT_TIMEOUT_MS, &mut b)
        .map_err(|e| format!("ATT read err: {e}"))?;
    if n < 105 || b[0] != 0x0B {
        return Err(format!("bad ATT read resp [{n}]"));
    }
    let mut val = b[1..n].to_vec(); // the characteristic value (~104 bytes)

    // 2) Patch: audiogram (per-frequency loss in dB HL) + per-ear amplification from
    // amplification/balance + tone/conversation-boost/ANR/own-voice.
    let amp = amplification.clamp(0.0, 1.0);
    let bal = balance.clamp(-1.0, 1.0);
    let left_amp = (amp - bal / 2.0).clamp(-1.0, 1.0);
    let right_amp = (amp + bal / 2.0).clamp(-1.0, 1.0);
    let cb = if conv_boost { 1.0f32 } else { 0.0f32 };
    let tone = tone.clamp(-1.0, 1.0);
    let anr = anr.clamp(0.0, 1.0);
    let own_voice = own_voice.clamp(0.0, 1.0);

    if val.len() > OFF_MODE {
        val[OFF_MODE] = 0x64;
    }
    for i in 0..8usize {
        put_f32(
            &mut val,
            OFF_LEFT_EQ + i * 4,
            *left_eq.get(i).unwrap_or(&0.0),
        );
        put_f32(
            &mut val,
            OFF_RIGHT_EQ + i * 4,
            *right_eq.get(i).unwrap_or(&0.0),
        );
    }
    put_f32(&mut val, OFF_LEFT_AMP, left_amp);
    put_f32(&mut val, OFF_LEFT_TONE, tone);
    put_f32(&mut val, OFF_LEFT_CONV, cb);
    put_f32(&mut val, OFF_LEFT_ANR, anr);
    put_f32(&mut val, OFF_RIGHT_AMP, right_amp);
    put_f32(&mut val, OFF_RIGHT_TONE, tone);
    put_f32(&mut val, OFF_RIGHT_CONV, cb);
    put_f32(&mut val, OFF_RIGHT_ANR, anr);
    put_f32(&mut val, OFF_OWN_VOICE, own_voice);

    // 3) Write it back.
    drv.att_send(&att_write_pdu(H_SETTINGS, &val))
        .map_err(|e| format!("ATT write: {e}"))?;
    let wn = drv
        .att_recv(ATT_TIMEOUT_MS, &mut b)
        .map_err(|e| format!("ATT write response: {e}"))?;
    require_write_response(&b[..wn])?;

    Ok(format!(
        "hearing settings acknowledged: {} bytes",
        val.len()
    ))
}

/// Apply hearing-assist settings. Requires the AAP + ATT channels to be up (the
/// driver opens ATT on demand). Returns a summary only for work that was applied.
#[allow(clippy::too_many_arguments)]
pub fn apply(
    drv: &Driver,
    gen: u64,
    on: bool,
    left_eq: &[f32],
    right_eq: &[f32],
    amplification: f32,
    balance: f32,
    tone: f32,
    conv_boost: bool,
    anr: f32,
    own_voice: f32,
) -> Result<Option<String>, String> {
    // Serialize applies and read the "already enabled" flag under the same lock, so
    // overlapping applies can't interleave ATT I/O on the shared channel.
    let mut state = HEARING_STATE.lock().unwrap_or_else(|p| p.into_inner());
    let session = HEARING_SESSION.load(Ordering::SeqCst);

    // A newer command arrived while we waited for the lock — drop this stale apply
    // before touching the driver (latest-wins coalescing).
    if HEARING_GEN.load(Ordering::SeqCst) != gen {
        return Ok(None);
    }

    if !on {
        drv.send(&HA_OFF_33).map_err(|e| e.to_string())?;
        thread::sleep(Duration::from_millis(300));
        drv.send(&HA_OFF_2C).map_err(|e| e.to_string())?;
        *state = None;
        return Ok(Some("Hearing assistance disable request sent.".into()));
    }

    // AAP-only disable above remains available even when ATT needs a reconnect.
    require_clean_att_session(session)?;
    let result = (|| {
        if *state != Some(session) {
            enable_handshake(drv)?;
        }
        write_settings(
            drv,
            left_eq,
            right_eq,
            amplification,
            balance,
            tone,
            conv_boost,
            anr,
            own_voice,
        )
    })();
    match result {
        Ok(s) => {
            *state = Some(session);
            Ok(Some(s))
        }
        Err(_) => {
            *state = None;
            // ACKs carry no handle. A same-session retry could consume a late
            // reply from either feature; only reconnecting discards that queue.
            ATT_FAILED_SESSION.store(session, Ordering::SeqCst);
            if !is_current(gen) {
                return Ok(None);
            }
            Err(
                "Hearing settings were not confirmed. Reconnect your headphones, then try again."
                    .into(),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_invalidation_does_not_wait_for_a_worker_or_keep_its_handshake() {
        let mut enabled = HEARING_STATE.lock().unwrap();
        let old_session = HEARING_SESSION.load(Ordering::SeqCst);
        *enabled = Some(old_session);
        let generation = next_gen();
        let loud_sound = loud_sound_token();
        ATT_FAILED_SESSION.store(old_session, Ordering::SeqCst);
        assert!(require_clean_att_session(old_session).is_err());
        invalidate_session();
        assert!(!is_current(generation));
        assert!(!loud_sound.is_current());
        assert_ne!(*enabled, Some(HEARING_SESSION.load(Ordering::SeqCst)));
        assert!(require_clean_att_session(HEARING_SESSION.load(Ordering::SeqCst)).is_ok());
        *enabled = None;
    }

    #[test]
    fn only_the_write_acknowledgment_confirms_hearing_settings() {
        assert!(require_write_response(&[0x13]).is_ok());
        for response in [
            &[][..],
            &[0x01, 0x12, 0x2a, 0, 3][..],
            &[0x1b, 0x2a, 0][..],
            &[0x13, 0][..],
        ] {
            assert!(require_write_response(response).is_err());
        }
    }

    #[test]
    fn loud_sound_reads_and_unchanged_values_never_write() {
        for requested in [None, Some(true)] {
            let mut calls = 0;
            assert_eq!(
                loud_sound_transaction(requested, |request| {
                    calls += 1;
                    assert_eq!(request, [0x0A, 0x1B, 0]);
                    Ok(vec![0x0B, 1])
                }),
                Ok(true)
            );
            assert_eq!(calls, 1);
        }
    }

    #[test]
    fn unopened_att_can_start_but_other_receive_failures_cannot() {
        assert_eq!(att_queue_drained(Ok(0)), Ok(true));
        assert_eq!(att_queue_drained(Ok(2)), Ok(false));
        assert_eq!(
            att_queue_drained(Err(std::io::Error::from_raw_os_error(
                ERROR_DEVICE_NOT_CONNECTED.0 as i32
            ))),
            Ok(true)
        );
        for error in [5, 6, 995, 1168] {
            assert!(att_queue_drained(Err(std::io::Error::from_raw_os_error(error))).is_err());
        }
    }

    #[test]
    fn att_drain_is_bounded_and_cancellation_after_a_wait_prevents_the_send() {
        let current = std::cell::Cell::new(true);
        let drained = drain_att(
            || {
                current.set(false);
                Ok(0)
            },
            || current.get(),
        );
        let mut sent = false;
        let _ = drained.map(|_| sent = true);
        assert!(!sent);
        let mut calls = 0;
        assert!(drain_att(
            || {
                calls += 1;
                Ok(2)
            },
            || true
        )
        .is_err());
        assert_eq!(calls, 16);
    }

    #[test]
    fn att_notifications_do_not_consume_the_read_or_write_reply() {
        for reply in [vec![0x0B, 1], vec![0x13]] {
            let mut replies = [vec![0x1B, 0x2A, 0, 1], reply.clone()].into_iter();
            assert_eq!(
                att_response(
                    |timeout| {
                        assert!(timeout > 0 && timeout <= ATT_TIMEOUT_MS);
                        Ok(replies.next().expect("unexpected receive"))
                    },
                    || true
                ),
                Ok(reply)
            );
            assert!(replies.next().is_none());
        }
        let mut calls = 0;
        assert!(att_response(
            |_| {
                calls += 1;
                Ok(vec![0x1B, 0x2A, 0, 1])
            },
            || true
        )
        .is_err());
        assert_eq!(calls, 16);
        // Invalid zero-handle notifications remain errors for the transaction parser.
        assert_eq!(
            att_response(|_| Ok(vec![0x1B, 0, 0]), || true),
            Ok(vec![0x1B, 0, 0])
        );
    }

    #[test]
    fn loud_sound_writes_only_the_characteristic_and_confirms_readback() {
        for (requested, readback) in [(false, false), (true, true), (true, false)] {
            let mut exchanges = [
                (vec![0x0A, 0x1B, 0], vec![0x0B, u8::from(!requested)]),
                (vec![0x12, 0x1B, 0, u8::from(requested)], vec![0x13]),
                (vec![0x0A, 0x1B, 0], vec![0x0B, u8::from(readback)]),
            ]
            .into_iter();
            assert_eq!(
                loud_sound_transaction(Some(requested), |request| {
                    let (expected, response) = exchanges.next().expect("unexpected request");
                    assert_eq!(request, expected);
                    Ok(response)
                }),
                Ok(readback)
            );
            assert!(exchanges.next().is_none());
        }
    }

    #[test]
    fn loud_sound_rejects_missing_malformed_or_unacknowledged_replies() {
        for response in [
            vec![],
            vec![0x0B],
            vec![0x0B, 2],
            vec![0x0B, 1, 0],
            vec![0x01, 0x0A, 0x1B, 0, 3],
        ] {
            let mut calls = 0;
            assert!(loud_sound_transaction(Some(true), |_| {
                calls += 1;
                Ok(response.clone())
            })
            .is_err());
            assert_eq!(calls, 1, "invalid read must not send a write");
        }
        let mut calls = 0;
        assert!(loud_sound_transaction(Some(true), |_| {
            calls += 1;
            match calls {
                1 => Ok(vec![0x0B, 0]),
                2 => Ok(vec![0x13, 0]),
                _ => panic!("invalid write acknowledgment must stop the transaction"),
            }
        })
        .is_err());
        assert_eq!(calls, 2);
        assert!(loud_sound_transaction(Some(true), |_| Err("timeout".into())).is_err());
    }
}
