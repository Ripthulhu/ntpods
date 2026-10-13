//! AirPods ATT settings, including Loud Sound Reduction and hearing assistance.
//! Hearing assistance uses AAP controls 0x2C/0x33, Transparency mode and ATT handle 0x2A.
//! Settings follow Linux `hearing-aid-adjustments.py`: a mode byte and little-endian
//! f32 fields for each ear's 8-band audiogram (dB HL), gain, tone and noise controls.

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
pub(super) static HEARING_STATE: Mutex<Option<(u64, u64)>> = Mutex::new(None);
static HEARING_SESSION: AtomicU64 = AtomicU64::new(0);
// Coalesce rapid settings edits before they reach the shared ATT channel.
static HEARING_GEN: AtomicU64 = AtomicU64::new(0);
static LOUD_SOUND_GEN: AtomicU64 = AtomicU64::new(0);
static ATT_FAILED_SESSION: AtomicU64 = AtomicU64::new(u64::MAX);
// Low two bits: unknown/off/on. Upper bits: revision of the last device report.
static HEARING_REPORTED: AtomicU64 = AtomicU64::new(0);
// An off report invalidates cached and in-flight enables without blocking audio receive.
static HEARING_OFF_REVISION: AtomicU64 = AtomicU64::new(0);

fn enabled_in_session(state: Option<(u64, u64)>, session: u64, off_revision: u64) -> bool {
    state == Some((session, off_revision))
}

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
    publish_report(0);
    HEARING_SESSION.fetch_add(1, Ordering::SeqCst);
    HEARING_GEN.fetch_add(1, Ordering::SeqCst);
    LOUD_SOUND_GEN.fetch_add(1, Ordering::SeqCst);
}

pub(super) fn session() -> u64 {
    HEARING_SESSION.load(Ordering::SeqCst)
}

pub fn reported_state() -> (Option<bool>, u64) {
    let report = HEARING_REPORTED.load(Ordering::SeqCst);
    let enabled = match report & 3 {
        1 => Some(false),
        2 => Some(true),
        _ => None,
    };
    (enabled, report >> 2)
}

fn publish_report(state: u64) {
    let _ = HEARING_REPORTED.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |report| {
        Some((report & !3).wrapping_add(4) | state)
    });
}

fn clear_reported_mode() {
    // A local request is not a fresh report from the AirPods.
    HEARING_REPORTED.fetch_and(!3, Ordering::SeqCst);
}

fn reported_hearing_mode(packet: &[u8]) -> Option<bool> {
    match packet {
        [4, 0, 4, 0, 9, 0, 0x2c, enrolled @ (1 | 2), enabled @ (1 | 2), 0, 0] => {
            Some(*enrolled == 1 && *enabled == 1)
        }
        _ => None,
    }
}

pub fn observe_packet(packet: &[u8]) -> bool {
    if packet.starts_with(&[4, 0, 4, 0, 9, 0, 0x2c]) {
        let state = match reported_hearing_mode(packet) {
            Some(true) => 2,
            Some(false) => 1,
            None => 0,
        };
        if state == 1 {
            // Reports arrive on the audio receive thread; never take the ATT lock here.
            HEARING_OFF_REVISION.fetch_add(1, Ordering::SeqCst);
        }
        publish_report(state);
        return true;
    }
    false
}

pub(super) fn custom_transparency_allowed(hearing_supported: bool) -> bool {
    !hearing_supported || reported_state().0 == Some(false)
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
        if response.is_empty() {
            break;
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
    att_transaction(
        drv,
        || token.is_current(),
        |exchange| loud_sound_transaction(requested, exchange),
    )
    .map_err(|_| {
        "Could not read or change Loud Sound Reduction. Reconnect your headphones, then try again."
            .into()
    })
}

pub(super) fn att_transaction<T>(
    drv: &Driver,
    current: impl Fn() -> bool,
    operation: impl FnOnce(&mut dyn FnMut(&[u8]) -> Result<Vec<u8>, String>) -> Result<T, String>,
) -> Result<Option<T>, String> {
    let mut transaction = HEARING_STATE.lock().unwrap_or_else(|p| p.into_inner());
    if !current() {
        return Ok(None);
    }
    let session = session();
    require_clean_att_session(session)?;
    let mut reply_pending = false;
    let result =
        operation(&mut |request| att_exchange(drv, &current, session, request, &mut reply_pending));
    if !current() {
        if reply_pending && self::session() == session {
            ATT_FAILED_SESSION.store(session, Ordering::SeqCst);
            *transaction = None;
        }
        return Ok(None);
    }
    if reply_pending {
        // ATT read responses have no handle: after a timeout a late reply cannot
        // be attributed safely. Retry only on a new connection, whose queue resets.
        ATT_FAILED_SESSION.store(session, Ordering::SeqCst);
        *transaction = None;
    }
    result.map(Some)
}

fn att_exchange(
    drv: &Driver,
    current: impl Fn() -> bool,
    session: u64,
    request: &[u8],
    reply_pending: &mut bool,
) -> Result<Vec<u8>, String> {
    exchange_att(
        request,
        reply_pending,
        current,
        || self::session() == session,
        |packet| drv.att_send(packet),
        |timeout, response| drv.att_recv(timeout, response),
    )
}

fn exchange_att(
    request: &[u8],
    reply_pending: &mut bool,
    current: impl Fn() -> bool,
    same_session: impl Fn() -> bool,
    mut send: impl FnMut(&[u8]) -> std::io::Result<()>,
    mut receive: impl FnMut(u32, &mut [u8]) -> std::io::Result<usize>,
) -> Result<Vec<u8>, String> {
    if !current() {
        return Err("AirPods connection changed.".into());
    }
    let mut response = [0u8; 512];
    drain_att(|| receive(0, &mut response), &current)?;
    *reply_pending = true;
    send(request).map_err(|_| "Could not reach the AirPods settings channel.")?;
    if request.first() == Some(&0x52) {
        // ATT Write Command has no acknowledgment; its caller must read back.
        *reply_pending = false;
        return Ok(Vec::new());
    }
    let result = att_response(
        |timeout| {
            let length = receive(timeout, &mut response)
                .map_err(|_| "Could not read the AirPods settings channel.")?;
            Ok(response[..length].to_vec())
        },
        // Consume an in-flight reply even when a newer slider edit supersedes it.
        same_session,
    );
    if let Ok(response) = &result {
        *reply_pending = !reply_complete(request, response);
        if *reply_pending {
            return Err(
                "AirPods returned an unexpected settings response. Reconnect to retry.".into(),
            );
        }
    }
    result
}

fn reply_complete(request: &[u8], response: &[u8]) -> bool {
    let Some(&opcode) = request.first() else {
        return false;
    };
    if matches!(response, [1, failed, _, _, _] if *failed == opcode) {
        return true;
    }
    match opcode {
        0x06 => response.first() == Some(&0x07),
        0x08 => response.first() == Some(&0x09),
        0x0a => response.first() == Some(&0x0b),
        0x12 => response == [0x13],
        _ => false,
    }
}

// Re-enabling resets the ATT server, so only send this sequence for a new enable.
fn hearing_controls(
    on: bool,
    current: impl Fn() -> bool,
    mut send: impl FnMut(&[u8]) -> std::io::Result<()>,
    mut wait: impl FnMut(Duration),
) -> Result<(), String> {
    let transparency = aap::anc_command(3);
    let enable = [
        (HA_ON_2C.as_slice(), 300),
        (transparency.as_slice(), 200),
        (HA_ON_33.as_slice(), 900),
    ];
    let disable = [(HA_OFF_33.as_slice(), 300), (HA_OFF_2C.as_slice(), 0)];
    for &(packet, delay) in if on { &enable[..] } else { &disable[..] } {
        if !current() {
            return Err("AirPods connection changed.".into());
        }
        send(packet).map_err(|e| e.to_string())?;
        if delay != 0 {
            wait(Duration::from_millis(delay));
        }
    }
    if !current() {
        return Err("AirPods connection changed.".into());
    }
    Ok(())
}

fn require_write_response(response: &[u8]) -> Result<(), String> {
    if response == [0x13] {
        Ok(())
    } else {
        Err("AirPods did not acknowledge the hearing-settings write.".into())
    }
}

/// Preserve unrelated fields and keep ATT open while updating hearing settings.
#[allow(clippy::too_many_arguments)]
fn write_settings(
    mut exchange: impl FnMut(&[u8]) -> Result<Vec<u8>, String>,
    left_eq: &[f32],
    right_eq: &[f32],
    amplification: f32,
    balance: f32,
    tone: f32,
    conv_boost: bool,
    anr: f32,
    own_voice: f32,
) -> Result<String, String> {
    let response = exchange(&att_read_req(H_SETTINGS))?;
    if response.len() < 105 || response[0] != 0x0B {
        return Err(format!("bad ATT read resp [{}]", response.len()));
    }
    let mut val = response[1..].to_vec(); // the characteristic value (~104 bytes)

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

    require_write_response(&exchange(&att_write_pdu(H_SETTINGS, &val))?)?;

    Ok(format!(
        "hearing settings acknowledged: {} bytes",
        val.len()
    ))
}

/// Apply settings through AAP and lazy-opened ATT; superseded requests return `None`.
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
    // Keep the enable state and ATT transaction under one lock to prevent interleaved replies.
    let mut state = HEARING_STATE.lock().unwrap_or_else(|p| p.into_inner());
    let session = HEARING_SESSION.load(Ordering::SeqCst);
    let off_revision = HEARING_OFF_REVISION.load(Ordering::SeqCst);
    let current = || is_current(gen) && self::session() == session;

    // Discard superseded work before touching the shared driver.
    if !current() {
        return Ok(None);
    }

    if !on {
        *state = None;
        clear_reported_mode();
        let result = hearing_controls(false, &current, |packet| drv.send(packet), thread::sleep);
        if !current() {
            return Ok(None);
        }
        result?;
        return Ok(Some("Hearing assistance disable request sent.".into()));
    }

    // AAP-only disable above remains available even when ATT needs a reconnect.
    require_clean_att_session(session)?;
    let enabled = enabled_in_session(*state, session, off_revision);
    let mut reply_pending = false;
    if !enabled {
        let custom = crate::transparency::read_enabled(|request| {
            att_exchange(drv, &current, session, request, &mut reply_pending)
        });
        if !current() {
            if reply_pending && self::session() == session {
                ATT_FAILED_SESSION.store(session, Ordering::SeqCst);
            }
            return Ok(None);
        }
        match custom {
            Ok(true) => {
                return Err(
                    "Turn off Custom Transparency before enabling hearing assistance.".into(),
                )
            }
            Ok(false) => {}
            Err(error) => {
                if reply_pending && self::session() == session {
                    ATT_FAILED_SESSION.store(session, Ordering::SeqCst);
                }
                return Err(error);
            }
        }
    }
    let enable_current =
        || current() && HEARING_OFF_REVISION.load(Ordering::SeqCst) == off_revision;
    let result = (|| {
        let mut exchange = |request: &[u8]| {
            att_exchange(drv, &enable_current, session, request, &mut reply_pending)
        };
        if !enabled {
            clear_reported_mode();
            hearing_controls(
                true,
                &enable_current,
                |packet| drv.send(packet),
                thread::sleep,
            )?;
            require_write_response(&exchange(&att_write_pdu(H_CCCD, &[0x01, 0x00]))?)?;
        }
        write_settings(
            &mut exchange,
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
    // Only an unanswered ATT request can leave a late reply in this connection's queue.
    if reply_pending && self::session() == session {
        ATT_FAILED_SESSION.store(session, Ordering::SeqCst);
        *state = None;
    }
    if !current() {
        if !enabled || self::session() != session {
            *state = None;
        }
        return Ok(None);
    }
    if HEARING_OFF_REVISION.load(Ordering::SeqCst) != off_revision {
        *state = None;
        return Err("Hearing assistance changed on the AirPods. Try again.".into());
    }
    match result {
        Ok(s) => {
            *state = Some((session, off_revision));
            Ok(Some(s))
        }
        Err(_) => {
            *state = None;
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
    fn reported_mode_and_revision_distinguish_device_reports_from_local_requests() {
        let _guard = HEARING_STATE.lock().unwrap();
        let (_, revision) = reported_state();
        assert!(!observe_packet(&[4, 0, 4, 0, 9, 0, 0x0d, 3]));
        assert_eq!(reported_state().1, revision);
        assert!(observe_packet(&HA_OFF_2C));
        let off = reported_state();
        assert_eq!(off, (Some(false), revision + 1));
        clear_reported_mode();
        assert_eq!(reported_state(), (None, off.1));
        observe_packet(&HA_OFF_2C);
        assert_eq!(reported_state(), (Some(false), off.1 + 1));
        observe_packet(&HA_ON_2C);
        assert_eq!(reported_state(), (Some(true), off.1 + 2));
        invalidate_session();
        assert_eq!(reported_state(), (None, off.1 + 3));
    }

    #[test]
    fn cancelled_hearing_controls_never_send_the_next_stage() {
        use std::cell::Cell;
        for on in [false, true] {
            let expected = if on {
                vec![HA_ON_2C.to_vec(), aap::anc_command(3).to_vec(), HA_ON_33.to_vec()]
            } else {
                vec![HA_OFF_33.to_vec(), HA_OFF_2C.to_vec()]
            };
            for cancel_after in 0..=expected.len() {
                for during_wait in [false, true] {
                    let current = Cell::new(cancel_after != 0);
                    let count = Cell::new(0);
                    let mut sent = Vec::new();
                    let result = hearing_controls(
                        on,
                        || current.get(),
                        |packet| {
                            sent.push(packet.to_vec());
                            count.set(count.get() + 1);
                            if !during_wait && count.get() == cancel_after {
                                current.set(false);
                            }
                            Ok(())
                        },
                        |_| {
                            if during_wait && count.get() == cancel_after {
                                current.set(false);
                            }
                        },
                    );
                    assert_eq!(sent, expected[..cancel_after]);
                    if !on && during_wait && cancel_after == expected.len() {
                        assert!(result.is_ok()); // The final disable has no settle wait.
                    } else {
                        assert!(result.is_err());
                    }
                }
            }
        }
    }

    #[test]
    fn hearing_subscribe_read_and_write_accept_notifications_before_each_reply() {
        let mut sent = Vec::new();
        let mut pending = false;
        let mut exchange = |request: &[u8]| {
            let mut reply = vec![0x13];
            if request[0] == 0x0a {
                reply = vec![0x0b; 105];
            }
            let mut replies = [vec![0x1b, 0x2a, 0, 1], reply].into_iter();
            let result = exchange_att(
                request,
                &mut pending,
                || true,
                || true,
                |packet| {
                    sent.push(packet.to_vec());
                    Ok(())
                },
                |timeout, response| {
                    if timeout == 0 {
                        return Ok(0);
                    }
                    let reply = replies.next().expect("unexpected receive");
                    response[..reply.len()].copy_from_slice(&reply);
                    Ok(reply.len())
                },
            );
            assert!(replies.next().is_none());
            assert!(!pending);
            result
        };
        require_write_response(&exchange(&att_write_pdu(H_CCCD, &[1, 0])).unwrap()).unwrap();
        write_settings(&mut exchange, &[1.; 8], &[2.; 8], 0.5, 0., 0., true, 0., 0.).unwrap();
        assert_eq!(sent[0], [0x12, 0x2b, 0, 1, 0]);
        assert_eq!(sent[1], [0x0a, 0x2a, 0]);
        let write = &sent[2];
        assert_eq!(&write[..3], [0x12, 0x2a, 0]);
        assert_eq!(write.len(), 107);
        assert_eq!(&write[3..5], [0x0b; 2]);
        assert_eq!(write[5], 0x64);
        assert_eq!(write[6], 0x0b);
        assert_eq!(
            &write[3 + OFF_LEFT_AMP..7 + OFF_LEFT_AMP],
            0.5f32.to_le_bytes()
        );
        assert_eq!(sent.len(), 3);
    }

    #[test]
    fn hearing_settings_cancelled_during_read_consume_reply_without_writing() {
        use std::cell::Cell;
        for reconnect in [false, true] {
            let current = Cell::new(true);
            let same_session = Cell::new(true);
            let mut pending = false;
            let mut sent = Vec::new();
            let result = write_settings(
                |request| {
                    exchange_att(
                        request,
                        &mut pending,
                        || current.get(),
                        || same_session.get(),
                        |packet| {
                            sent.push(packet.to_vec());
                            Ok(())
                        },
                        |timeout, response| {
                            if timeout == 0 {
                                return Ok(0);
                            }
                            current.set(false);
                            same_session.set(!reconnect);
                            response[..105].fill(0);
                            response[0] = 0x0b;
                            Ok(105)
                        },
                    )
                },
                &[0.; 8],
                &[0.; 8],
                0.5,
                0.,
                0.,
                false,
                0.,
                0.,
            );
            assert!(result.is_err());
            assert_eq!(sent, [vec![0x0a, 0x2a, 0]]);
            assert_eq!(pending, reconnect);
        }
    }

    #[test]
    fn session_invalidation_does_not_wait_for_a_worker_or_keep_its_handshake() {
        let mut enabled = HEARING_STATE.lock().unwrap();
        let old_session = HEARING_SESSION.load(Ordering::SeqCst);
        *enabled = Some((old_session, HEARING_OFF_REVISION.load(Ordering::SeqCst)));
        let generation = next_gen();
        let loud_sound = loud_sound_token();
        let transparency = crate::transparency::token();
        ATT_FAILED_SESSION.store(old_session, Ordering::SeqCst);
        assert!(require_clean_att_session(old_session).is_err());
        observe_packet(&HA_OFF_2C);
        assert!(custom_transparency_allowed(true));
        observe_packet(&HA_ON_2C);
        assert!(!custom_transparency_allowed(true));
        assert!(custom_transparency_allowed(false));
        observe_packet(&HA_OFF_2C);
        observe_packet(&[4, 0, 4, 0, 9, 0, 0x2c, 1, 3, 0, 0]);
        assert!(!custom_transparency_allowed(true));
        observe_packet(&HA_OFF_2C);
        invalidate_session();
        assert!(!custom_transparency_allowed(true));
        assert!(!is_current(generation));
        assert!(!loud_sound.is_current());
        assert!(!transparency.is_current());
        assert!(!enabled_in_session(
            *enabled,
            HEARING_SESSION.load(Ordering::SeqCst),
            HEARING_OFF_REVISION.load(Ordering::SeqCst)
        ));
        assert!(require_clean_att_session(HEARING_SESSION.load(Ordering::SeqCst)).is_ok());
        *enabled = None;
    }

    #[test]
    fn a_confirmed_off_report_invalidates_the_cached_enable_without_locking() {
        let _guard = HEARING_STATE.lock().unwrap();
        let session = session();
        let enabled = Some((session, HEARING_OFF_REVISION.load(Ordering::SeqCst)));
        assert!(enabled_in_session(
            enabled,
            session,
            HEARING_OFF_REVISION.load(Ordering::SeqCst)
        ));
        observe_packet(&HA_OFF_2C);
        assert!(custom_transparency_allowed(true));
        assert!(!enabled_in_session(
            enabled,
            session,
            HEARING_OFF_REVISION.load(Ordering::SeqCst)
        ));
        // Even a later on report cannot revive the cached ATT enable or skip its conflict check.
        observe_packet(&HA_ON_2C);
        assert!(!enabled_in_session(
            enabled,
            session,
            HEARING_OFF_REVISION.load(Ordering::SeqCst)
        ));
        // A worker completing after the off report must store its original revision.
        let worker_revision = HEARING_OFF_REVISION.load(Ordering::SeqCst);
        observe_packet(&HA_OFF_2C);
        let completed_worker = Some((session, worker_revision));
        assert!(!enabled_in_session(
            completed_worker,
            session,
            HEARING_OFF_REVISION.load(Ordering::SeqCst)
        ));
    }

    #[test]
    fn hearing_mode_requires_a_complete_recognized_report() {
        assert_eq!(reported_hearing_mode(&HA_ON_2C), Some(true));
        assert_eq!(reported_hearing_mode(&HA_OFF_2C), Some(false));
        assert_eq!(
            reported_hearing_mode(&[4, 0, 4, 0, 9, 0, 0x2c, 2, 1, 0, 0]),
            Some(false)
        );
        for length in 0..HA_ON_2C.len() {
            assert_eq!(reported_hearing_mode(&HA_ON_2C[..length]), None);
        }
        for index in [0, 5, 6, 7, 8, 9, 10] {
            let mut packet = HA_ON_2C;
            packet[index] = 3;
            assert_eq!(reported_hearing_mode(&packet), None);
        }
    }

    #[test]
    fn consumed_att_errors_and_bad_values_do_not_leave_a_reply_outstanding() {
        for (request, response) in [
            (vec![0x0a, 0x18, 0], vec![1, 0x0a, 0x18, 0, 3]),
            (vec![0x0a, 0x18, 0], vec![0x0b, 0]),
            (vec![0x06, 1, 0], vec![1, 0x06, 1, 0, 0x0a]),
            (vec![0x12, 0x18, 0, 0], vec![1, 0x12, 0x18, 0, 3]),
        ] {
            assert!(reply_complete(&request, &response));
            assert!(reply_complete(&[0x0a, 0x18, 0], &[0x0b, 1]));
        }
        for response in [
            vec![],
            vec![0x0b, 1],
            vec![0x13, 0],
            vec![1, 0x0a, 0x18, 0, 3],
        ] {
            assert!(!reply_complete(&[0x12, 0x18, 0, 0], &response));
        }
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
        assert!(att_response(|_| Ok(vec![]), || true).is_err());
        let current = std::cell::Cell::new(true);
        assert!(att_response(
            |_| {
                current.set(false);
                Ok(vec![0x0B, 1])
            },
            || current.get()
        )
        .is_err());
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
