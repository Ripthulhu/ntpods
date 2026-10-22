//! AirPods ATT settings, including Loud Sound Reduction and hearing assistance.
//! Hearing assistance uses AAP controls 0x2C/0x33 and its discovered ATT characteristic.
//! Settings follow Linux `hearing-aid-adjustments.py`: a header and little-endian
//! f32 fields for each ear's 8-band audiogram (dB HL), gain, tone and noise controls.

use crate::driver::Driver;
use crate::{
    aap,
    att_settings::{self, Characteristic},
};
use ntpods_ipc::HearingAidSettings;
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
static HEARING_EDIT_SESSION: AtomicU64 = AtomicU64::new(u64::MAX);
static HEARING_READ_SESSION: AtomicU64 = AtomicU64::new(u64::MAX);
static ATT_FAILED_SESSION: AtomicU64 = AtomicU64::new(u64::MAX);
// Low two bits: unknown/off/on. Upper bits: revision of the last device report.
static HEARING_REPORTED: AtomicU64 = AtomicU64::new(0);
// Off and unknown reports invalidate cached and in-flight enables without blocking audio receive.
static HEARING_OFF_REVISION: AtomicU64 = AtomicU64::new(0);
static MODES: Mutex<ModeCache> = Mutex::new(ModeCache {
    session: 0,
    revision: 0,
    hearing: None,
    hearing_template: None,
    assist: None,
    pme: None,
    pme_template: None,
});

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    HearingAid,
    MediaAssist,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModeState {
    pub hearing_enrolled: Option<bool>,
    pub hearing_enabled: Option<bool>,
    pub assist_enabled: Option<bool>,
    pub pme_voice_enrolled: Option<bool>,
    pub pme_voice_enabled: Option<bool>,
    pub pme_media_enabled: Option<bool>,
    pub media_enabled: Option<bool>,
    pub revision: u64,
}

#[derive(Clone, Default)]
pub(super) struct ModeCache {
    session: u64,
    revision: u64,
    hearing: Option<[u8; 11]>,
    hearing_template: Option<[u8; 11]>,
    assist: Option<bool>,
    pme: Option<[u8; 140]>,
    pme_template: Option<[u8; 140]>,
}

fn flag(value: u8) -> Option<bool> {
    match value {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

impl ModeCache {
    /// Preserve validated templates for an Off recovery, but require a new report.
    pub(super) fn invalidate_report(&mut self, slot: u8) {
        let changed = match slot {
            2 => self.hearing.take().is_some(),
            3 => self.assist.take().is_some(),
            4 => self.pme.take().is_some(),
            _ => false,
        };
        if changed {
            self.revision = self.revision.wrapping_add(1);
        }
    }

    pub(super) fn state(&self) -> ModeState {
        let voice = self.pme.as_ref().and_then(|packet| flag(packet[10]));
        let media = self.pme.as_ref().and_then(|packet| flag(packet[11]));
        ModeState {
            hearing_enrolled: self
                .hearing
                .as_ref()
                .or(self.hearing_template.as_ref())
                .and_then(|packet| flag(packet[7])),
            hearing_enabled: self
                .hearing
                .as_ref()
                .and_then(|packet| reported_hearing_mode(packet)),
            assist_enabled: self.assist,
            pme_voice_enrolled: self
                .pme
                .as_ref()
                .or(self.pme_template.as_ref())
                .and_then(|packet| flag(packet[9])),
            pme_voice_enabled: voice,
            pme_media_enabled: media,
            media_enabled: match (self.assist, voice, media) {
                (Some(gate), Some(voice), Some(media)) => Some(gate && (voice || media)),
                _ => None,
            },
            revision: self.revision,
        }
    }

    /// USB owns a separate cache; observing its reports must not alter native state.
    pub(super) fn observe(&mut self, packet: &[u8]) -> bool {
        let changed = if packet.starts_with(&[4, 0, 4, 0, 9, 0, 0x2c]) {
            let value = hearing_report(packet);
            let changed = self.hearing != value;
            self.hearing = value;
            self.hearing_template = value;
            changed
        } else if packet.starts_with(&[4, 0, 4, 0, 9, 0, 0x33]) {
            let value = match packet {
                [4, 0, 4, 0, 9, 0, 0x33, value @ (1 | 2), 0, 0, 0] => flag(*value),
                _ => None,
            };
            let changed = self.assist != value;
            self.assist = value;
            changed
        } else if packet.starts_with(&[4, 0, 4, 0, 0x53, 0]) {
            let value = pme_report(packet);
            let changed = self.pme != value;
            self.pme = value;
            self.pme_template = value;
            changed
        } else {
            false
        };
        if changed {
            self.revision = self.revision.wrapping_add(1);
        }
        changed
    }
}

pub fn mode_state() -> ModeState {
    let modes = MODES.lock().unwrap_or_else(|error| error.into_inner());
    if modes.session == session() {
        modes.state()
    } else {
        ModeState {
            revision: modes.revision,
            ..ModeState::default()
        }
    }
}

pub fn cached_pme_packet() -> Option<Vec<u8>> {
    let modes = MODES.lock().unwrap_or_else(|error| error.into_inner());
    (modes.session == session())
        .then_some(modes.pme_template)
        .flatten()
        .map(|packet| packet.to_vec())
}

pub fn has_stored_profile() -> bool {
    has_current_read() && mode_state().hearing_enrolled == Some(true)
}

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
    let session = HEARING_SESSION
        .fetch_add(1, Ordering::SeqCst)
        .wrapping_add(1);
    let mut modes = MODES.lock().unwrap_or_else(|error| error.into_inner());
    *modes = ModeCache {
        session,
        revision: modes.revision.wrapping_add(1),
        hearing: None,
        hearing_template: None,
        assist: None,
        pme: None,
        pme_template: None,
    };
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
    MODES
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .hearing = None;
}

fn hearing_report(packet: &[u8]) -> Option<[u8; 11]> {
    match packet {
        [4, 0, 4, 0, 9, 0, 0x2c, 0..=2, 0..=2, 0, 0] => packet.try_into().ok(),
        _ => None,
    }
}

fn pme_report(packet: &[u8]) -> Option<[u8; 140]> {
    if packet.len() == 140
        && packet[..9] == [4, 0, 4, 0, 0x53, 0, 0x84, 0, 2]
        && packet[9..12].iter().all(|value| *value <= 2)
    {
        packet.try_into().ok()
    } else {
        None
    }
}

fn reported_hearing_mode(packet: &[u8]) -> Option<bool> {
    match packet {
        // Pro 3 reports an unset enabled byte when hearing assistance is not enrolled.
        [4, 0, 4, 0, 9, 0, 0x2c, 2, 0, 0, 0] => Some(false),
        [4, 0, 4, 0, 9, 0, 0x2c, enrolled @ (1 | 2), enabled @ (1 | 2), 0, 0] => {
            Some(*enrolled == 1 && *enabled == 1)
        }
        _ => None,
    }
}

pub fn observe_packet(packet: &[u8]) -> bool {
    if packet.starts_with(&[4, 0, 4, 0, 9, 0, 0x2c]) {
        let mut modes = MODES.lock().unwrap_or_else(|error| error.into_inner());
        modes.session = session();
        modes.hearing = hearing_report(packet);
        modes.hearing_template = modes.hearing;
        modes.revision = modes.revision.wrapping_add(1);
        let state = match reported_hearing_mode(packet) {
            Some(true) => 2,
            Some(false) => 1,
            None => 0,
        };
        if state != 2 {
            // Unknown and off reports invalidate enables without blocking audio receive.
            HEARING_OFF_REVISION.fetch_add(1, Ordering::SeqCst);
        }
        publish_report(state);
        return true;
    }
    if packet.starts_with(&[4, 0, 4, 0, 9, 0, 0x33]) {
        let mut modes = MODES.lock().unwrap_or_else(|error| error.into_inner());
        modes.session = session();
        modes.assist = match packet {
            [4, 0, 4, 0, 9, 0, 0x33, value @ (1 | 2), 0, 0, 0] => flag(*value),
            _ => None,
        };
        modes.revision = modes.revision.wrapping_add(1);
        return true;
    }
    if packet.starts_with(&[4, 0, 4, 0, 0x53, 0]) {
        // The two-byte on-bud gate is a separate message from the full curve configuration.
        if matches!(packet, [4, 0, 4, 0, 0x53, 0, 2, 0, 1, 1 | 2]) {
            return false;
        }
        let mut modes = MODES.lock().unwrap_or_else(|error| error.into_inner());
        modes.session = session();
        modes.pme = pme_report(packet);
        modes.pme_template = modes.pme;
        modes.revision = modes.revision.wrapping_add(1);
        return true;
    }
    false
}

pub(super) fn custom_transparency_allowed(hearing_supported: bool) -> bool {
    custom_transparency_modes_allowed(
        hearing_supported,
        reported_state().0,
        mode_state().media_enabled,
    )
}

fn custom_transparency_modes_allowed(
    supported: bool,
    hearing: Option<bool>,
    media: Option<bool>,
) -> bool {
    !supported || (hearing == Some(false) && media == Some(false))
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

// A4120005-95C5-4D6F-9098-0F0B41457E0A, little-endian ATT UUID.
const CHARACTERISTIC: [u8; 16] = [
    0x0a, 0x7e, 0x45, 0x41, 0x0b, 0x0f, 0x98, 0x90, 0x6f, 0x4d, 0xc5, 0x95, 0x05, 0x00, 0x12, 0xa4,
];

// ATT responses have their own receive queue; bound how long this settings worker waits.
const ATT_TIMEOUT_MS: u32 = 900;

// f32 offsets into the settings value (bytes after the ATT opcode) — mirror the
// Linux reference exactly.
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
    media_enabled: bool,
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
    let packets = if on {
        &enable[..]
    } else if media_enabled {
        &disable[1..]
    } else {
        &disable[..]
    };
    for &(packet, delay) in packets {
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

fn parse_settings(value: &[u8]) -> Result<HearingAidSettings, String> {
    if value.len() != 104 || !matches!(value[..4], [2, 0 | 2, 0x60 | 0x64, 0]) {
        return Err("The AirPods returned an unsupported hearing-settings format.".into());
    }
    let read = |offset| f32::from_le_bytes(value[offset..offset + 4].try_into().unwrap());
    let left_conversation = read(OFF_LEFT_CONV);
    let right_conversation = read(OFF_RIGHT_CONV);
    if !matches!(left_conversation, 0.0 | 1.0) || !matches!(right_conversation, 0.0 | 1.0) {
        return Err("The AirPods returned unsupported hearing-settings values.".into());
    }
    let settings = HearingAidSettings {
        left_eq: std::array::from_fn(|i| read(OFF_LEFT_EQ + 4 * i)),
        right_eq: std::array::from_fn(|i| read(OFF_RIGHT_EQ + 4 * i)),
        left_amplification: read(OFF_LEFT_AMP),
        right_amplification: read(OFF_RIGHT_AMP),
        left_tone: read(OFF_LEFT_TONE),
        right_tone: read(OFF_RIGHT_TONE),
        left_conversation_boost: left_conversation == 1.0,
        right_conversation_boost: right_conversation == 1.0,
        left_ambient_noise_reduction: read(OFF_LEFT_ANR),
        right_ambient_noise_reduction: read(OFF_RIGHT_ANR),
        own_voice: read(OFF_OWN_VOICE),
    };
    if !settings.is_valid() {
        return Err("The AirPods returned unsupported hearing-settings values.".into());
    }
    Ok(settings)
}

pub(super) fn att_error(context: &str, response: &[u8]) -> String {
    match response {
        [0x01, request, low, high, error] => format!(
            "{context} ATT request 0x{request:02X}, handle 0x{:04X}, error 0x{error:02X}.",
            u16::from_le_bytes([*low, *high]),
        ),
        _ => context.into(),
    }
}

fn read_value(
    exchange: &mut impl FnMut(&[u8]) -> Result<Vec<u8>, String>,
    handle: u16,
) -> Result<(Vec<u8>, HearingAidSettings), String> {
    let response = exchange(&att_read_req(handle))?;
    let Some(value) = response.strip_prefix(&[0x0b]) else {
        return Err(att_error(
            "Could not read the existing audiograms.",
            &response,
        ));
    };
    let settings = parse_settings(value)?;
    Ok((value.to_vec(), settings))
}

fn discover(
    exchange: &mut impl FnMut(&[u8]) -> Result<Vec<u8>, String>,
) -> Result<Characteristic, String> {
    att_settings::discover(exchange, &CHARACTERISTIC, "Hearing assistance")?
        .ok_or("Hearing assistance is not available on these AirPods.".into())
}

pub(super) fn read_settings(
    exchange: &mut impl FnMut(&[u8]) -> Result<Vec<u8>, String>,
) -> Result<(Characteristic, Vec<u8>, HearingAidSettings), String> {
    let characteristic = discover(exchange)?;
    let (value, settings) = read_value(exchange, characteristic.handle)?;
    Ok((characteristic, value, settings))
}

pub(super) fn editable_value(value: &[u8], characteristic: Characteristic) -> bool {
    characteristic.write_opcode.is_some()
        && value.len() == 104
        && matches!(value[..4], [2, 2, 0x60 | 0x64, 0])
}

/// Capture report identity before a request waits for the shared ATT lock.
#[derive(Clone, Copy)]
pub struct Authorization {
    session: u64,
    off_revision: u64,
    report: u64,
    mode_revision: u64,
}

pub fn authorization() -> Authorization {
    let modes = MODES.lock().unwrap_or_else(|error| error.into_inner());
    Authorization {
        session: self::session(),
        off_revision: HEARING_OFF_REVISION.load(Ordering::SeqCst),
        report: HEARING_REPORTED.load(Ordering::SeqCst),
        mode_revision: modes.revision,
    }
}

impl Authorization {
    fn is_current(self) -> bool {
        self.session == self::session()
            && self.off_revision == HEARING_OFF_REVISION.load(Ordering::SeqCst)
            && self.report == HEARING_REPORTED.load(Ordering::SeqCst)
            && self.mode_revision == mode_state().revision
    }
}

pub fn has_current_read() -> bool {
    HEARING_READ_SESSION.load(Ordering::SeqCst) == self::session()
}

pub fn can_edit_current_read() -> bool {
    has_current_read() && HEARING_EDIT_SESSION.load(Ordering::SeqCst) == self::session()
}

/// A read must not enable hearing assistance or write its notification subscriptions.
pub fn read(drv: &Driver, gen: u64) -> Result<Option<HearingAidSettings>, String> {
    let session = self::session();
    let current = || is_current(gen) && self::session() == session;
    if current() {
        HEARING_READ_SESSION.store(u64::MAX, Ordering::SeqCst);
        HEARING_EDIT_SESSION.store(u64::MAX, Ordering::SeqCst);
    }
    att_transaction(drv, &current, |exchange| {
        let result = read_settings(&mut |request| exchange(request));
        // Publish while holding the ATT lock so an older read cannot overwrite a newer one.
        if current() {
            HEARING_EDIT_SESSION.store(
                match &result {
                    Ok((characteristic, value, _)) if editable_value(value, *characteristic) => {
                        session
                    }
                    _ => u64::MAX,
                },
                Ordering::SeqCst,
            );
            HEARING_READ_SESSION.store(
                if result.is_ok() { session } else { u64::MAX },
                Ordering::SeqCst,
            );
        }
        result.map(|(_, _, settings)| settings)
    })
}

pub(super) fn prepare_settings(
    mut exchange: impl FnMut(&[u8]) -> Result<Vec<u8>, String>,
    expected: &HearingAidSettings,
) -> Result<(Characteristic, Vec<u8>), String> {
    let (characteristic, value, actual) = read_settings(&mut exchange)?;
    if !editable_value(&value, characteristic) {
        return Err("These hearing settings can be read but not edited yet.".into());
    }
    if actual != *expected {
        return Err(
            "Hearing settings changed on the AirPods. Read them again before editing.".into(),
        );
    }
    Ok((characteristic, value))
}

fn patch_settings(value: &mut [u8], requested: &HearingAidSettings) {
    for i in 0..8 {
        put_f32(value, OFF_LEFT_EQ + i * 4, requested.left_eq[i]);
        put_f32(value, OFF_RIGHT_EQ + i * 4, requested.right_eq[i]);
    }
    for (offset, v) in [
        (OFF_LEFT_AMP, requested.left_amplification),
        (OFF_RIGHT_AMP, requested.right_amplification),
        (OFF_LEFT_TONE, requested.left_tone),
        (OFF_RIGHT_TONE, requested.right_tone),
        (OFF_LEFT_ANR, requested.left_ambient_noise_reduction),
        (OFF_RIGHT_ANR, requested.right_ambient_noise_reduction),
        (OFF_OWN_VOICE, requested.own_voice),
    ] {
        put_f32(value, offset, v);
    }
    for (offset, on) in [
        (OFF_LEFT_CONV, requested.left_conversation_boost),
        (OFF_RIGHT_CONV, requested.right_conversation_boost),
    ] {
        let current = f32::from_le_bytes(value[offset..offset + 4].try_into().unwrap());
        // Both signs of zero mean false; retain untouched device bytes.
        if (current == 1.0) != on {
            put_f32(value, offset, if on { 1.0 } else { 0.0 });
        }
    }
}

pub(super) fn write_settings(
    mut exchange: impl FnMut(&[u8]) -> Result<Vec<u8>, String>,
    characteristic: Characteristic,
    mut preserved: Vec<u8>,
    requested: &HearingAidSettings,
) -> Result<HearingAidSettings, String> {
    if !requested.is_valid() {
        return Err("Complete, valid audiograms and hearing settings are required.".into());
    }
    parse_settings(&preserved)?;
    if !editable_value(&preserved, characteristic) {
        return Err("These hearing settings can be read but not edited yet.".into());
    }
    let original = preserved.clone();
    patch_settings(&mut preserved, requested);
    // Apple's v2 writer counts the 100-byte prefix; device reads may report 96.
    preserved[..4].copy_from_slice(&[2, 2, 0x64, 0]);
    let mut write = att_write_pdu(characteristic.handle, &preserved);
    write[0] = characteristic.write_opcode.unwrap();
    let response = exchange(&write)?;
    if write[0] == 0x12 {
        require_write_response(&response)?;
    }
    confirm_settings(&mut exchange, characteristic.handle, &original, &preserved, thread::sleep)
}

fn confirm_settings(
    exchange: &mut impl FnMut(&[u8]) -> Result<Vec<u8>, String>,
    handle: u16,
    original: &[u8],
    written: &[u8],
    mut wait: impl FnMut(Duration),
) -> Result<HearingAidSettings, String> {
    for attempt in 0..5 {
        let (value, settings) = read_value(exchange, handle)?;
        if !matches!(value[..4], [2, 2, 0x60 | 0x64, 0]) {
            return Err("AirPods returned a different hearing-settings format after the write.".into());
        }
        if value[4..] == written[4..] {
            return Ok(settings);
        }
        // A Write Command may finish after the first read; never resend it or accept other changes.
        if value[4..] != original[4..] || attempt == 4 {
            break;
        }
        wait(Duration::from_millis(75));
    }
    Err("AirPods did not apply the hearing settings. Read them again before editing.".into())
}

fn update_settings(
    mut exchange: impl FnMut(&[u8]) -> Result<Vec<u8>, String>,
    expected: &HearingAidSettings,
    requested: &HearingAidSettings,
    enable: impl FnOnce() -> Result<bool, String>,
) -> Result<HearingAidSettings, String> {
    if !requested.is_valid() {
        return Err("Complete, valid audiograms and hearing settings are required.".into());
    }
    // Enable commands can alter the ATT server; preserve the original value first.
    let (mut characteristic, preserved) = prepare_settings(&mut exchange, expected)?;
    if enable()? {
        characteristic = discover(&mut exchange)?;
        if !editable_value(&preserved, characteristic) {
            return Err("These hearing settings can be read but not edited yet.".into());
        }
        let (enabled_value, _) = read_value(&mut exchange, characteristic.handle)?;
        if enabled_value != preserved {
            return Err(
                "AirPods changed the hearing settings while enabling assistance. Read them again before editing.".into(),
            );
        }
    }
    // No notification consumer is needed: every write is confirmed by an explicit read.
    write_settings(&mut exchange, characteristic, preserved, requested)
}

fn recover_failed_enable<T>(
    result: Result<T, String>,
    started_enable: bool,
    previously_enabled: bool,
    current: impl Fn() -> bool,
    disable: impl FnOnce() -> Result<(), String>,
) -> Result<T, String> {
    match result {
        Err(error) if started_enable && !previously_enabled && current() => {
            match disable() {
                Ok(()) => Err(format!("{error} Hearing-assistance disable was requested; wait for the AirPods to confirm it is off.")),
                Err(cleanup) => Err(format!("{error} Hearing assistance may still be active: disabling it failed ({cleanup}).")),
            }
        }
        other => other,
    }
}

pub enum Applied {
    Disabled,
    Settings(HearingAidSettings),
    Requested(ModeVerification),
}

pub struct ModeVerification {
    plan: ModePlan,
    original_profile: Option<Vec<u8>>,
    send_error: Option<String>,
    pub session: u64,
}

pub(super) struct ModePlan {
    enrollment: u8,
    hearing: bool,
    voice: bool,
    media: bool,
    gate: bool,
    original_pme: [u8; 140],
    pub(super) packets: Vec<Vec<u8>>,
}

pub(super) fn mode_plan(cache: &ModeCache, mode: Mode, on: bool) -> Result<ModePlan, String> {
    let state = cache.state();
    if on && state.hearing_enrolled != Some(true) {
        return Err("Set up Hearing Assistance on an iPhone or iPad first.".into());
    }
    let unknown =
        || "Wait for the AirPods to report the other hearing mode before changing it.".to_owned();
    if on {
        let (Some(hearing), Some(voice), Some(media), Some(gate)) = (
            state.hearing_enabled,
            state.pme_voice_enabled,
            state.pme_media_enabled,
            state.assist_enabled,
        ) else {
            return Err(unknown());
        };
        if gate != (hearing || voice || media) {
            return Err(
                "Hearing modes disagree. Reconnect the AirPods before changing them.".into(),
            );
        }
    }
    let desired_hearing = if mode == Mode::HearingAid {
        on
    } else {
        state.hearing_enabled.ok_or_else(unknown)?
    };
    let (desired_voice, desired_media) = if mode == Mode::MediaAssist {
        (on, on)
    } else {
        (
            state.pme_voice_enabled.ok_or_else(unknown)?,
            state.pme_media_enabled.ok_or_else(unknown)?,
        )
    };
    let pme = cache.pme.or(cache.pme_template).ok_or_else(unknown)?;
    let enrollment = cache
        .hearing
        .or(cache.hearing_template)
        .ok_or_else(unknown)?[7];
    let desired_gate = desired_hearing || desired_voice || desired_media;
    let mut packets = Vec::new();
    if state.assist_enabled != Some(desired_gate) {
        let mut packet = HA_OFF_33;
        packet[7] = if desired_gate { 1 } else { 2 };
        packets.push(packet.to_vec());
    }
    match mode {
        Mode::HearingAid if state.hearing_enabled != Some(on) => {
            let mut packet = cache
                .hearing
                .or(cache.hearing_template)
                .ok_or("Read the current Hearing Aid state first.")?;
            packet[8] = if on { 1 } else { 2 };
            packets.push(packet.to_vec());
            if on {
                packets.push(aap::anc_command(3).to_vec());
            }
        }
        Mode::MediaAssist
            if state.pme_voice_enabled != Some(on) || state.pme_media_enabled != Some(on) =>
        {
            let mut packet = pme;
            // Apple clones this configuration and changes only its two enable flags.
            packet[10] = if on { 1 } else { 2 };
            packet[11] = if on { 1 } else { 2 };
            packets.push(packet.to_vec());
        }
        _ => {}
    }
    Ok(ModePlan {
        enrollment,
        hearing: desired_hearing,
        voice: desired_voice,
        media: desired_media,
        gate: desired_gate,
        original_pme: pme,
        packets,
    })
}

pub fn next_mode_gen(mode: Mode, on: bool) -> Result<u64, String> {
    let cache = MODES.lock().unwrap_or_else(|error| error.into_inner());
    if cache.session != session() {
        return Err("AirPods connection changed.".into());
    }
    mode_plan(&cache, mode, on)?;
    // Reject an unsafe Off before cancelling the other mode's confirmation.
    Ok(next_gen())
}

pub(super) fn mode_confirmed(plan: &ModePlan, cache: &ModeCache) -> bool {
    let state = cache.state();
    cache
        .hearing
        .is_some_and(|packet| packet[7] == plan.enrollment)
        && state.hearing_enabled == Some(plan.hearing)
        && state.assist_enabled == Some(plan.gate)
        && state.pme_voice_enabled == Some(plan.voice)
        && state.pme_media_enabled == Some(plan.media)
        && cache.pme.is_some_and(|pme| {
            pme[8..10] == plan.original_pme[8..10] && pme[12..] == plan.original_pme[12..]
        })
}

fn send_mode_packets(
    packets: &[Vec<u8>],
    current: impl Fn() -> bool,
    mut send: impl FnMut(&[u8]) -> std::io::Result<()>,
) -> Result<(), String> {
    for packet in packets {
        if !current() {
            return Err("AirPods connection changed.".into());
        }
        send(packet).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn reserve_mode_change(
    original: &ModeCache,
    current: &mut ModeCache,
    plan: &ModePlan,
) -> Result<bool, String> {
    if current.session != original.session || current.revision != original.revision {
        return Err(
            "Hearing modes changed before the request started. Read their state again.".into(),
        );
    }
    let mut hearing_changed = false;
    for packet in &plan.packets {
        match (packet[4], packet[6]) {
            (9, 0x2c) => {
                current.hearing = None;
                hearing_changed = true;
            }
            (9, 0x33) => current.assist = None,
            (9, 0x0d) => {}
            (0x53, _) => current.pme = None,
            _ => unreachable!(),
        }
    }
    current.revision = current.revision.wrapping_add(1);
    Ok(hearing_changed)
}

fn stored_profile_read(
    mut exchange: impl FnMut(&[u8]) -> Result<Vec<u8>, String>,
) -> Result<(Characteristic, Vec<u8>, HearingAidSettings), String> {
    read_settings(&mut |request| {
        if !matches!(request.first(), Some(0x06 | 0x08 | 0x0a)) {
            return Err("Stored hearing profiles can only be read.".into());
        }
        exchange(request)
    })
}

pub fn mode_reports_complete() -> bool {
    let state = mode_state();
    state.hearing_enabled.is_some()
        && state.assist_enabled.is_some()
        && state.pme_voice_enabled.is_some()
        && state.pme_media_enabled.is_some()
}

pub(super) fn verify_profile(
    original: &[u8],
    mut exchange: impl FnMut(&[u8]) -> Result<Vec<u8>, String>,
) -> Result<(HearingAidSettings, bool), String> {
    let (characteristic, confirmed, settings) = stored_profile_read(|request| exchange(request))?;
    if confirmed != original {
        return Err("The stored hearing profile changed. Read it again before continuing.".into());
    }
    Ok((settings, editable_value(&confirmed, characteristic)))
}

/// Firmware may report mode changes only on a new AACP session.
pub fn verify_mode(
    drv: &Driver,
    gen: u64,
    verification: &ModeVerification,
) -> Result<Option<Applied>, String> {
    let session = self::session();
    let current = || is_current(gen) && self::session() == session;
    let check_modes = || {
        let modes = MODES.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(error) = &verification.send_error {
            return Err(format!("The mode change was incomplete ({error}). The AirPods state was refreshed; check the switches before trying again."));
        }
        if modes.session != session || !mode_confirmed(&verification.plan, &modes) {
            return Err("The AirPods reported different hearing modes. Read their state again.".into());
        }
        Ok(())
    };
    // Turning a mode off must remain possible when the ATT channel needs recovery.
    let Some(original) = &verification.original_profile else {
        if !current() {
            return Ok(None);
        }
        check_modes()?;
        return Ok(current().then_some(Applied::Disabled));
    };
    att_transaction(drv, &current, |exchange| {
        check_modes()?;
        let (settings, editable) = verify_profile(original, |request| exchange(request))?;
        check_modes()?;
        HEARING_READ_SESSION.store(session, Ordering::SeqCst);
        HEARING_EDIT_SESSION.store(if editable { session } else { u64::MAX }, Ordering::SeqCst);
        Ok(Applied::Settings(settings))
    })
}

/// Change stored-profile modes without writing audiograms, gain curves or subscriptions.
pub fn set_mode(
    drv: &Driver,
    gen: u64,
    mode: Mode,
    on: bool,
    authorized: Authorization,
) -> Result<Option<Applied>, String> {
    let mut transaction = HEARING_STATE
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let session = self::session();
    let current = || is_current(gen) && self::session() == session;
    if !current() {
        return Ok(None);
    }
    if !authorized.is_current() {
        return Err("Hearing modes changed. Read their current state and try again.".into());
    }
    let cache = MODES
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    let plan = mode_plan(&cache, mode, on)?;
    let mut reply_pending = false;
    let result = (|| {
        let baseline = if on {
            require_clean_att_session(session)?;
            let baseline = stored_profile_read(|request| {
                att_exchange(drv, &current, session, request, &mut reply_pending)
            })?;
            if (mode == Mode::MediaAssist || cache.state().hearing_enabled != Some(true))
                && crate::transparency::read_enabled(|request| {
                    att_exchange(drv, &current, session, request, &mut reply_pending)
                })?
            {
                return Err(
                    "Turn off Custom Transparency before enabling hearing assistance.".into(),
                );
            }
            Some(baseline)
        } else {
            None
        };
        if !current() || !authorized.is_current() {
            return Err("Hearing modes changed before the request started.".into());
        }
        if !plan.packets.is_empty() {
            let mut modes = MODES.lock().unwrap_or_else(|error| error.into_inner());
            if !current() || modes.session != session {
                return Err("AirPods connection changed.".into());
            }
            if reserve_mode_change(&cache, &mut modes, &plan)? {
                HEARING_REPORTED.fetch_and(!3, Ordering::SeqCst);
            }
            *transaction = None;
            drop(modes);
            // A failed send can follow accepted controls; refresh the actual state either way.
            let send_error =
                send_mode_packets(&plan.packets, &current, |packet| drv.send(packet)).err();
            return Ok(Applied::Requested(ModeVerification {
                plan,
                original_profile: baseline.map(|(_, value, _)| value),
                send_error,
                session,
            }));
        }
        if let Some((_, original, _)) = baseline {
            let (characteristic, confirmed, settings) = stored_profile_read(|request| {
                att_exchange(drv, &current, session, request, &mut reply_pending)
            })?;
            if confirmed != original {
                return Err(
                    "The stored hearing profile changed. Read it again before continuing.".into(),
                );
            }
            let modes = MODES.lock().unwrap_or_else(|error| error.into_inner());
            if modes.session != session || !mode_confirmed(&plan, &modes) {
                return Err(
                    "Hearing modes changed before the request finished. Read their state again."
                        .into(),
                );
            }
            HEARING_READ_SESSION.store(session, Ordering::SeqCst);
            HEARING_EDIT_SESSION.store(
                if editable_value(&confirmed, characteristic) { session } else { u64::MAX },
                Ordering::SeqCst,
            );
            if plan.hearing {
                *transaction = Some((session, HEARING_OFF_REVISION.load(Ordering::SeqCst)));
            }
            Ok(Applied::Settings(settings))
        } else {
            Ok(Applied::Disabled)
        }
    })();
    if reply_pending && self::session() == session {
        ATT_FAILED_SESSION.store(session, Ordering::SeqCst);
        *transaction = None;
    }
    if !current() {
        return Ok(None);
    }
    result.map(Some)
}

/// Apply settings through AAP and lazy-opened ATT; superseded requests return `None`.
pub fn apply(
    drv: &Driver,
    gen: u64,
    on: bool,
    authorized: Authorization,
    expected: Option<HearingAidSettings>,
    requested: Option<HearingAidSettings>,
) -> Result<Option<Applied>, String> {
    if !on {
        return set_mode(drv, gen, Mode::HearingAid, false, authorized);
    }
    // Keep the enable state and ATT transaction under one lock to prevent interleaved replies.
    let mut state = HEARING_STATE.lock().unwrap_or_else(|p| p.into_inner());
    let session = HEARING_SESSION.load(Ordering::SeqCst);
    let off_revision = HEARING_OFF_REVISION.load(Ordering::SeqCst);
    let current = || is_current(gen) && self::session() == session;

    // Discard superseded work before touching the shared driver.
    if !current() {
        return Ok(None);
    }

    let media_enabled = mode_state()
        .media_enabled
        .ok_or("Wait for the AirPods to report Media Assist before changing hearing assistance.")?;

    if !authorized.is_current() || authorized.report & 3 == 0 {
        return Err(
            "Hearing assistance changed before the request started. Read the settings again."
                .into(),
        );
    }
    let (Some(expected), Some(requested)) = (expected, requested) else {
        return Err(
            "Read the existing audiograms before enabling or editing hearing assistance.".into(),
        );
    };
    if HEARING_READ_SESSION.load(Ordering::SeqCst) != session {
        return Err("Read the existing audiograms again after reconnecting.".into());
    }
    if !can_edit_current_read() {
        return Err("These hearing settings can be read but not edited yet.".into());
    }
    if !expected.is_valid() || !requested.is_valid() {
        return Err("Complete, valid audiograms and hearing settings are required.".into());
    }

    // Mode-only disable remains available even when ATT needs a reconnect.
    require_clean_att_session(session)?;
    let enabled = enabled_in_session(*state, session, off_revision) || authorized.report & 3 == 2;
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
    let started_enable = std::cell::Cell::new(false);
    let result = update_settings(
        |request| att_exchange(drv, &enable_current, session, request, &mut reply_pending),
        &expected,
        &requested,
        || {
            if !enabled {
                started_enable.set(true);
                clear_reported_mode();
                hearing_controls(
                    true,
                    media_enabled,
                    &enable_current,
                    |packet| drv.send(packet),
                    thread::sleep,
                )?;
            }
            Ok(!enabled)
        },
    );
    let result = recover_failed_enable(
        result,
        started_enable.get(),
        authorized.report & 3 == 2,
        &current,
        || {
            clear_reported_mode();
            hearing_controls(
                false,
                media_enabled,
                &current,
                |packet| drv.send(packet),
                thread::sleep,
            )
        },
    );
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
        HEARING_READ_SESSION.store(u64::MAX, Ordering::SeqCst);
        // The recovery Off report also advances this revision. Keep its original
        // failure/cleanup detail instead of replacing it with a generic error.
        return match result {
            Err(error) => Err(error),
            Ok(_) => Err("Hearing assistance changed on the AirPods. Try again.".into()),
        };
    }
    match result {
        Ok(settings) => {
            *state = Some((session, off_revision));
            Ok(Some(Applied::Settings(settings)))
        }
        Err(error) => {
            *state = None;
            HEARING_READ_SESSION.store(u64::MAX, Ordering::SeqCst);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode_fixture(hearing: bool, voice: bool, media: bool) -> ModeCache {
        let mut pme = [0u8; 140];
        pme[..12].copy_from_slice(&[
            4,
            0,
            4,
            0,
            0x53,
            0,
            0x84,
            0,
            2,
            0,
            if voice { 1 } else { 2 },
            if media { 1 } else { 2 },
        ]);
        for (i, value) in pme[12..].iter_mut().enumerate() {
            *value = i as u8;
        }
        let hearing = Some(if hearing { HA_ON_2C } else { HA_OFF_2C });
        ModeCache {
            session: 0,
            revision: 1,
            hearing,
            hearing_template: hearing,
            assist: Some(hearing == Some(HA_ON_2C) || voice || media),
            pme: Some(pme),
            pme_template: Some(pme),
        }
    }

    #[test]
    fn independent_mode_cache_uses_reports_without_touching_other_sessions() {
        let original = mode_fixture(false, false, false);
        let mut cache = ModeCache::default();
        assert!(cache.observe(&original.hearing.unwrap()));
        assert!(cache.observe(&HA_OFF_33));
        assert!(cache.observe(&original.pme.unwrap()));
        let plan = mode_plan(&cache, Mode::HearingAid, true).unwrap();
        assert!(!mode_confirmed(&plan, &cache));
        for packet in &plan.packets {
            cache.observe(packet);
        }
        assert!(mode_confirmed(&plan, &cache));
        assert_eq!(original.state().hearing_enabled, Some(false));
        assert_eq!(cache.state().hearing_enabled, Some(true));
        let revision = cache.state().revision;
        assert!(!cache.observe(&HA_ON_2C));
        assert_eq!(cache.state().revision, revision);
        cache.invalidate_report(2);
        assert_eq!(cache.state().hearing_enabled, None);
        assert!(!mode_confirmed(&plan, &cache));
        assert!(mode_plan(&cache, Mode::HearingAid, false).is_ok());
        assert!(cache.observe(&HA_ON_2C));
        assert!(mode_confirmed(&plan, &cache));
    }

    #[test]
    fn malformed_mode_reports_remove_edit_authorization_and_templates() {
        let mut cache = mode_fixture(false, false, false);
        for packet in [
            vec![4, 0, 4, 0, 9, 0, 0x2c],
            vec![4, 0, 4, 0, 9, 0, 0x33],
            vec![4, 0, 4, 0, 0x53, 0],
        ] {
            assert!(cache.observe(&packet));
        }
        assert!(cache.state().hearing_enabled.is_none());
        assert!(cache.state().media_enabled.is_none());
        for mode in [Mode::HearingAid, Mode::MediaAssist] {
            for on in [true, false] {
                assert!(mode_plan(&cache, mode, on).is_err());
            }
        }
    }

    #[test]
    fn stored_mode_packets_preserve_the_other_mode_enrollment_and_every_curve_byte() {
        for hearing in [false, true] {
            for voice in [false, true] {
                for media in [false, true] {
                    let cache = mode_fixture(hearing, voice, media);
                    for mode in [Mode::HearingAid, Mode::MediaAssist] {
                        for on in [false, true] {
                            let plan = mode_plan(&cache, mode, on).unwrap();
                            assert_eq!(
                                plan.hearing,
                                if mode == Mode::HearingAid {
                                    on
                                } else {
                                    hearing
                                }
                            );
                            assert_eq!(
                                plan.voice,
                                if mode == Mode::MediaAssist { on } else { voice }
                            );
                            assert_eq!(
                                plan.media,
                                if mode == Mode::MediaAssist { on } else { media }
                            );
                            assert_eq!(plan.gate, plan.hearing || plan.voice || plan.media);
                            let mut confirmed = cache.clone();
                            for packet in &plan.packets {
                                match (packet[4], packet[6]) {
                                    (9, 0x33) => {
                                        assert_eq!(packet.len(), 11);
                                        assert_eq!(flag(packet[7]), Some(plan.gate));
                                        confirmed.assist = flag(packet[7]);
                                    }
                                    (9, 0x2c) => {
                                        assert_eq!(mode, Mode::HearingAid);
                                        assert_eq!(packet[7], cache.hearing.unwrap()[7]);
                                        assert_eq!(&packet[9..], &[0, 0]);
                                        confirmed.hearing = hearing_report(packet);
                                    }
                                    (9, 0x0d) => {
                                        assert_eq!(mode, Mode::HearingAid);
                                        assert!(on && !hearing);
                                        assert_eq!(packet, &aap::anc_command(3));
                                    }
                                    (0x53, _) => {
                                        assert_eq!(mode, Mode::MediaAssist);
                                        let original = cache.pme.unwrap();
                                        assert_eq!(&packet[..10], &original[..10]);
                                        assert_eq!(&packet[12..], &original[12..]);
                                        confirmed.pme = pme_report(packet);
                                    }
                                    _ => panic!("Mode switch wrote a profile or unrelated control"),
                                }
                            }
                            assert!(mode_confirmed(&plan, &confirmed));
                            confirmed.pme.as_mut().unwrap()[12] ^= 1;
                            assert!(!mode_confirmed(&plan, &confirmed));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn mode_confirmation_rejects_changed_enrollment_even_when_off_flags_match() {
        let cache = mode_fixture(true, false, false);
        let plan = mode_plan(&cache, Mode::HearingAid, false).unwrap();
        let mut confirmed = mode_fixture(false, false, false);
        assert!(mode_confirmed(&plan, &confirmed));
        confirmed.hearing.as_mut().unwrap()[7] = 2;
        assert!(!mode_confirmed(&plan, &confirmed));
    }

    #[test]
    fn pending_other_mode_off_is_rejected_without_cancelling_its_confirmation() {
        let _guard = HEARING_STATE.lock().unwrap();
        for (pending_mode, other_mode, hearing, media) in [
            (Mode::MediaAssist, Mode::HearingAid, true, false),
            (Mode::HearingAid, Mode::MediaAssist, false, true),
        ] {
            invalidate_session();
            let mut original = mode_fixture(hearing, media, media);
            original.session = session();
            let plan = mode_plan(&original, pending_mode, true).unwrap();
            let mut reserved = original.clone();
            reserve_mode_change(&original, &mut reserved, &plan).unwrap();
            *MODES.lock().unwrap() = reserved;
            let generation = HEARING_GEN.load(Ordering::SeqCst);
            assert!(next_mode_gen(other_mode, false).is_err());
            assert!(is_current(generation));
            assert_eq!(next_mode_gen(pending_mode, false).unwrap(), generation.wrapping_add(1));
        }
        invalidate_session();
    }

    #[test]
    fn partial_mode_send_stops_at_the_failure_and_keeps_it_for_verification() {
        let plan = mode_plan(&mode_fixture(false, false, false), Mode::HearingAid, true).unwrap();
        assert_eq!(plan.packets.len(), 3);
        for failed_at in 0..plan.packets.len() {
            let mut attempts = 0;
            let result = send_mode_packets(
                &plan.packets,
                || true,
                |packet| {
                    assert_eq!(packet, plan.packets[attempts]);
                    attempts += 1;
                    if attempts == failed_at + 1 {
                        Err(std::io::Error::other("injected send failure"))
                    } else {
                        Ok(())
                    }
                },
            );
            assert!(result.is_err());
            assert_eq!(attempts, failed_at + 1);
            assert!(result.err().unwrap().contains("injected send failure"));
        }
        assert!(send_mode_packets(&plan.packets, || false, |_| panic!("cancelled send")).is_err());
    }

    #[test]
    fn stored_modes_fail_closed_on_unknown_or_inconsistent_policy_and_unenrolled_enables() {
        let original = mode_fixture(false, false, false);
        for missing in 0..5 {
            let mut cache = original.clone();
            match missing {
                0 => cache.hearing = None,
                1 => cache.assist = None,
                2 => cache.pme = None,
                3 => cache.pme.as_mut().unwrap()[10] = 0,
                _ => cache.pme.as_mut().unwrap()[11] = 0,
            }
            for mode in [Mode::HearingAid, Mode::MediaAssist] {
                assert!(mode_plan(&cache, mode, true).is_err());
            }
        }
        let mut inconsistent = original.clone();
        inconsistent.assist = Some(true);
        assert!(mode_plan(&inconsistent, Mode::HearingAid, true).is_err());
        let mut unenrolled = original;
        unenrolled.hearing = Some([4, 0, 4, 0, 9, 0, 0x2c, 2, 0, 0, 0]);
        for mode in [Mode::HearingAid, Mode::MediaAssist] {
            assert!(mode_plan(&unenrolled, mode, true).is_err());
            assert!(mode_plan(&unenrolled, mode, false)
                .unwrap()
                .packets
                .is_empty());
        }
    }

    #[test]
    fn full_pme_cache_requires_exact_configuration_and_is_cleared_by_bad_reports_or_reconnect() {
        let _guard = HEARING_STATE.lock().unwrap();
        invalidate_session();
        let pme = mode_fixture(false, true, false).pme.unwrap();
        assert!(observe_packet(&HA_OFF_2C));
        assert!(observe_packet(&HA_ON_33));
        assert!(observe_packet(&pme));
        assert_eq!(mode_state().media_enabled, Some(true));
        assert_eq!(mode_state().pme_voice_enrolled, None);
        assert_eq!(cached_pme_packet(), Some(pme.to_vec()));
        let authorized = authorization();
        assert!(!observe_packet(&[4, 0, 4, 0, 0x53, 0, 2, 0, 1, 1]));
        assert_eq!(cached_pme_packet(), Some(pme.to_vec()));
        for offset in [6, 8, 9, 10, 11] {
            let mut malformed = pme;
            malformed[offset] = 9;
            assert!(observe_packet(&malformed));
            assert!(cached_pme_packet().is_none());
            assert_eq!(mode_state().media_enabled, None);
            assert!(observe_packet(&pme));
        }
        assert!(!authorized.is_current());
        assert!(observe_packet(&pme[..139]));
        assert!(cached_pme_packet().is_none());
        observe_packet(&pme);
        invalidate_session();
        assert!(cached_pme_packet().is_none());
        assert_eq!(mode_state().hearing_enrolled, None);
        assert_eq!(mode_state().assist_enabled, None);
    }

    #[test]
    fn stored_profile_mode_read_accepts_known_readonly_format_without_any_attribute_write() {
        let _guard = HEARING_STATE.lock().unwrap();
        invalidate_session();
        assert!(!has_current_read());
        assert!(!can_edit_current_read());
        assert!(mode_plan(&mode_fixture(false, false, false), Mode::HearingAid, true).is_ok());
        let (mut original, settings) = fixture();
        original[..4].copy_from_slice(&[2, 2, 0x60, 0]);
        let mut replies = discovery(0x02, 0x39);
        replies.push((vec![0x0a, 0x39, 0], [vec![0x0b], original.clone()].concat()));
        let mut replies = replies.into_iter();
        let result = stored_profile_read(|request| {
            assert!(matches!(request[0], 0x06 | 0x08 | 0x0a));
            let (expected, response) = replies.next().expect("Unexpected profile write");
            assert_eq!(request, expected);
            Ok(response)
        })
        .unwrap();
        assert_eq!(result.0, Characteristic { handle: 0x39, write_opcode: None });
        assert_eq!((result.1, result.2), (original, settings));
        assert!(replies.next().is_none());
    }

    #[test]
    fn refreshed_mode_verification_checks_every_profile_byte_and_never_writes() {
        let (mut original, expected) = fixture();
        original[..4].copy_from_slice(&[2, 2, 0x60, 0]);
        for changed in [None, Some(1), Some(4), Some(100)] {
            let mut value = original.clone();
            if let Some(offset) = changed {
                value[offset] ^= 1;
            }
            let mut replies = discovery(0x02, 0x39);
            replies.push((vec![0x0a, 0x39, 0], [vec![0x0b], value].concat()));
            let mut replies = replies.into_iter();
            let result = verify_profile(&original, |request| {
                let (expected, response) = replies.next().expect("Unexpected profile write");
                assert_eq!(request, expected);
                Ok(response)
            });
            if changed.is_none() {
                assert_eq!(result.unwrap(), (expected, false));
            } else {
                assert!(result.is_err());
            }
            assert!(replies.next().is_none());
        }
    }

    #[test]
    fn legacy_cleanup_leaves_shared_assist_enabled_for_existing_media_assist() {
        let mut packets = Vec::new();
        hearing_controls(
            false,
            true,
            || true,
            |packet| {
                packets.push(packet.to_vec());
                Ok(())
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(packets, [HA_OFF_2C.to_vec()]);
    }

    #[test]
    fn explicit_off_recovers_unconfirmed_target_without_turning_off_the_other_mode() {
        for other_on in [false, true] {
            let mut pending_hearing = mode_fixture(false, other_on, other_on);
            pending_hearing.hearing = None;
            pending_hearing.assist = None;
            let plan = mode_plan(&pending_hearing, Mode::HearingAid, false).unwrap();
            assert_eq!(
                plan.packets,
                [
                    if other_on {
                        HA_ON_33.to_vec()
                    } else {
                        HA_OFF_33.to_vec()
                    },
                    HA_OFF_2C.to_vec()
                ]
            );
            assert!(mode_plan(&pending_hearing, Mode::HearingAid, true).is_err());

            let mut pending_media = mode_fixture(other_on, false, false);
            pending_media.pme = None;
            pending_media.assist = None;
            let original = pending_media.pme_template.unwrap();
            let plan = mode_plan(&pending_media, Mode::MediaAssist, false).unwrap();
            assert_eq!(
                plan.packets[0],
                if other_on {
                    HA_ON_33.to_vec()
                } else {
                    HA_OFF_33.to_vec()
                }
            );
            assert_eq!(plan.packets[1][10..12], [2, 2]);
            assert_eq!(&plan.packets[1][..10], &original[..10]);
            assert_eq!(&plan.packets[1][12..], &original[12..]);
            assert!(mode_plan(&pending_media, Mode::MediaAssist, true).is_err());
            pending_media.hearing = None;
            assert!(mode_plan(&pending_media, Mode::MediaAssist, false).is_err());
            pending_hearing.pme = None;
            assert!(mode_plan(&pending_hearing, Mode::HearingAid, false).is_err());
        }
    }

    #[test]
    fn a_new_pme_report_during_profile_read_prevents_writing_the_old_curve_snapshot() {
        let original = mode_fixture(false, false, false);
        let plan = mode_plan(&original, Mode::MediaAssist, true).unwrap();
        for changed in 0..3 {
            let mut current = original.clone();
            match changed {
                0 => current.pme.as_mut().unwrap()[12] ^= 1,
                1 => current.assist = Some(true),
                _ => current.session += 1,
            }
            current.revision += 1;
            let before = current.clone();
            assert!(reserve_mode_change(&original, &mut current, &plan).is_err());
            assert_eq!(current.pme, before.pme);
            assert_eq!(current.hearing, before.hearing);
            assert_eq!(current.assist, before.assist);
            assert_eq!(current.revision, before.revision);
        }
    }

    #[test]
    fn custom_transparency_requires_both_hearing_modes_confirmed_off() {
        for hearing in [None, Some(false), Some(true)] {
            for media in [None, Some(false), Some(true)] {
                assert_eq!(
                    custom_transparency_modes_allowed(true, hearing, media),
                    hearing == Some(false) && media == Some(false)
                );
                assert!(custom_transparency_modes_allowed(false, hearing, media));
            }
        }
    }

    const TEST_SETTING: Characteristic = Characteristic {
        handle: 0x35,
        write_opcode: Some(0x12),
    };

    fn discovery(properties: u8, handle: u16) -> Vec<(Vec<u8>, Vec<u8>)> {
        let mut service = vec![0x06, 1, 0, 0xff, 0xff, 0, 0x28];
        service.extend_from_slice(&att_settings::SERVICE);
        let mut declaration = vec![9, 21];
        declaration.extend_from_slice(&(handle - 1).to_le_bytes());
        declaration.push(properties);
        declaration.extend_from_slice(&handle.to_le_bytes());
        declaration.extend_from_slice(&CHARACTERISTIC);
        vec![
            (service, vec![7, 0x30, 0, 0x40, 0]),
            (vec![8, 0x30, 0, 0x40, 0, 3, 0x28], declaration),
            (
                vec![8, handle as u8, (handle >> 8) as u8, 0x40, 0, 3, 0x28],
                vec![1, 8, 0x40, 0, 0x0a],
            ),
        ]
    }

    fn update_settings(
        mut exchange: impl FnMut(&[u8]) -> Result<Vec<u8>, String>,
        expected: &HearingAidSettings,
        requested: &HearingAidSettings,
        enable: impl FnOnce() -> Result<bool, String>,
    ) -> Result<HearingAidSettings, String> {
        let mut pending_discovery = discovery(0x0a, TEST_SETTING.handle).into_iter();
        super::update_settings(
            |request| {
                if matches!(request[0], 0x06 | 0x08) {
                    if request[0] == 0x06 {
                        pending_discovery = discovery(0x0a, TEST_SETTING.handle).into_iter();
                    }
                    let (expected, response) = pending_discovery
                        .next()
                        .expect("unexpected discovery request");
                    assert_eq!(request, expected);
                    Ok(response)
                } else {
                    assert!(pending_discovery.next().is_none());
                    exchange(request)
                }
            },
            expected,
            requested,
            enable,
        )
    }

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
                vec![
                    HA_ON_2C.to_vec(),
                    aap::anc_command(3).to_vec(),
                    HA_ON_33.to_vec(),
                ]
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
                        false,
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

    fn fixture() -> (Vec<u8>, HearingAidSettings) {
        let settings = HearingAidSettings {
            left_eq: [5., 10., 15., 20., 25., 30., 35., 40.],
            right_eq: [10., 15., 20., 25., 30., 35., 40., 45.],
            left_amplification: -0.25,
            right_amplification: 0.5,
            left_tone: -0.5,
            right_tone: 0.25,
            left_conversation_boost: true,
            right_conversation_boost: false,
            left_ambient_noise_reduction: 0.25,
            right_ambient_noise_reduction: 0.75,
            own_voice: 0.5,
        };
        let mut value = vec![0; 104];
        value[..4].copy_from_slice(&[2, 2, 0x64, 0]);
        patch_settings(&mut value, &settings);
        (value, settings)
    }

    #[test]
    fn reading_audiograms_only_reads_and_preserves_distinct_values() {
        let (value, settings) = fixture();
        let mut requests = Vec::new();
        let result = read_value(
            &mut |request| {
                requests.push(request.to_vec());
                Ok([vec![0x0b], value.clone()].concat())
            },
            TEST_SETTING.handle,
        )
        .unwrap();
        assert_eq!(result, (value, settings));
        assert_eq!(requests, [vec![0x0a, 0x35, 0]]);
    }

    #[test]
    fn hearing_reads_require_a_complete_known_v2_header() {
        let (mut value, settings) = fixture();
        for enrollment in [0, 2] {
            for format in [0x60, 0x64] {
                value[..4].copy_from_slice(&[2, enrollment, format, 0]);
                assert_eq!(parse_settings(&value).unwrap(), settings);
            }
        }
        for header in [
            [0, 0, 0, 0],
            [7, 9, 0, 11],
            [1, 2, 0x64, 0],
            [3, 2, 0x64, 0],
            [2, 1, 0x64, 0],
            [2, 3, 0x64, 0],
            [2, 2, 0x64, 1],
            [2, 2, 0x60, 1],
            [2, 2, 0, 0],
            [2, 2, 0x61, 0],
        ] {
            value[..4].copy_from_slice(&header);
            assert!(parse_settings(&value).is_err(), "header {header:02x?}");
        }
        value[..4].copy_from_slice(&[1, 2, 0x64, 0]);
        assert!(parse_settings(&value[..100]).is_err());
        value[..4].copy_from_slice(&[2, 2, 0x64, 0]);
        assert!(parse_settings(&value[..103]).is_err());
        value.push(0);
        assert!(parse_settings(&value).is_err());
    }

    #[test]
    fn fitted_v2_header_is_editable_only_with_advertised_write_permission() {
        let (mut value, settings) = fixture();
        for header in [[2, 0, 0x60, 0], [2, 2, 0x60, 0]] {
            value[..4].copy_from_slice(&header);
            let mut replies = discovery(0x16, 0x39);
            replies.push((vec![0x0a, 0x39, 0], [vec![0x0b], value.clone()].concat()));
            let mut replies = replies.into_iter();
            let (characteristic, original, actual) = read_settings(&mut |request| {
                let (expected, response) = replies.next().expect("read sent a write");
                assert_eq!(request, expected);
                Ok(response)
            })
            .unwrap();
            assert_eq!(actual, settings);
            assert_eq!(original, value);
            assert_eq!(editable_value(&original, characteristic), header[1] == 2);
            assert!(replies.next().is_none());
            assert!(!editable_value(&original, Characteristic { write_opcode: None, ..characteristic }));
        }
        for header in [
            [0, 0, 0x60, 0],
            [2, 1, 0x60, 0],
            [2, 3, 0x60, 0],
            [2, 0, 0x60, 1],
            [2, 2, 0x60, 1],
            [2, 0, 0x61, 0],
            [2, 2, 0x61, 0],
        ] {
            value[..4].copy_from_slice(&header);
            assert!(parse_settings(&value).is_err());
            assert!(!editable_value(&value, TEST_SETTING));
        }
        for header in [[7, 9, 0, 11], [1, 2, 0x64, 0], [2, 0, 0x64, 0], [2, 2, 0x64, 1]] {
            value[..4].copy_from_slice(&header);
            assert!(write_settings(|_| panic!("unknown format was written"), TEST_SETTING, value.clone(), &settings).is_err());
        }
    }

    #[test]
    fn discovered_readonly_characteristic_can_be_read_but_not_used_for_legacy_edits() {
        let (value, baseline) = fixture();
        for read_only in [true, false] {
            let mut replies = discovery(0x02, 0x38);
            replies.push((vec![0x0a, 0x38, 0], [vec![0x0b], value.clone()].concat()));
            let mut replies = replies.into_iter();
            let exchange = &mut |request: &[u8]| {
                let (expected, response) = replies
                    .next()
                    .expect("read-only characteristic was written");
                assert_eq!(request, expected);
                Ok(response)
            };
            if read_only {
                let (characteristic, value, settings) = read_settings(exchange).unwrap();
                assert_eq!(settings, baseline);
                assert!(!editable_value(&value, characteristic));
            } else {
                assert!(
                    super::update_settings(exchange, &baseline, &baseline, || panic!(
                        "read-only characteristic enabled hearing"
                    ))
                    .is_err()
                );
            }
            assert!(replies.next().is_none());
        }
    }

    #[test]
    fn hearing_writes_use_the_apple_v2_header_and_discovered_write_method() {
        let (original, baseline) = fixture();
        let requested = HearingAidSettings {
            left_tone: 0.75,
            ..baseline
        };
        for (properties, opcode) in [(0x16, 0x52), (0x0a, 0x12)] {
            for header in [[2, 2, 0x60, 0], [2, 2, 0x64, 0]] {
                let mut original = original.clone();
                original[..4].copy_from_slice(&header);
                let mut changed = original.clone();
                patch_settings(&mut changed, &requested);
                changed[..4].copy_from_slice(&[2, 2, 0x64, 0]);
                let mut write = vec![opcode, 0x39, 0];
                write.extend_from_slice(&changed);
                let mut replies = discovery(properties, 0x39);
                replies.extend([
                    (vec![0x0a, 0x39, 0], [vec![0x0b], original].concat()),
                    (write, if opcode == 0x12 { vec![0x13] } else { vec![] }),
                    (vec![0x0a, 0x39, 0], [vec![0x0b], changed].concat()),
                ]);
                let mut replies = replies.into_iter();
                assert_eq!(
                    super::update_settings(
                        |request| {
                            let (expected, response) =
                                replies.next().expect("unexpected ATT request");
                            assert_eq!(request, expected);
                            Ok(response)
                        },
                        &baseline,
                        &requested,
                        || Ok(false)
                    )
                    .unwrap(),
                    requested
                );
                assert!(replies.next().is_none());
            }
        }
    }

    #[test]
    fn write_command_does_not_wait_for_acknowledgment() {
        let mut reply_pending = true;
        let mut sent = 0;
        let result = exchange_att(
            &[0x52, 0x39, 0, 1],
            &mut reply_pending,
            || true,
            || true,
            |packet| {
                assert_eq!(packet, [0x52, 0x39, 0, 1]);
                sent += 1;
                Ok(())
            },
            |timeout, _| {
                assert_eq!(timeout, 0, "Write Command waited for a response");
                Ok(0)
            },
        );
        assert_eq!(result, Ok(vec![]));
        assert_eq!(sent, 1);
        assert!(!reply_pending);
    }

    #[test]
    fn fitted_write_accepts_only_exact_body_readback_with_known_header_normalization() {
        let (mut original, baseline) = fixture();
        original[..4].copy_from_slice(&[2, 2, 0x60, 0]);
        let requested = HearingAidSettings { left_eq: [12.0; 8], ..baseline };
        for header in [[2, 2, 0x60, 0], [2, 2, 0x64, 0], [2, 0, 0x60, 0]] {
            for changed_byte in [None, Some(OFF_RIGHT_EQ), Some(OFF_OWN_VOICE)] {
                let mut readback = original.clone();
                patch_settings(&mut readback, &requested);
                readback[..4].copy_from_slice(&header);
                if let Some(offset) = changed_byte { readback[offset] ^= 1; }
                let mut writes = 0;
                let result = write_settings(|request| {
                    if request[0] == 0x12 {
                        writes += 1;
                        assert_eq!(&request[3..7], &[2, 2, 0x64, 0]);
                        assert_eq!(&request[3 + OFF_LEFT_AMP..], &original[OFF_LEFT_AMP..]);
                        Ok(vec![0x13])
                    } else {
                        assert_eq!(request[0], 0x0a);
                        Ok([vec![0x0b], readback.clone()].concat())
                    }
                }, TEST_SETTING, original.clone(), &requested);
                assert_eq!(result.is_ok(), header[1] == 2 && changed_byte.is_none());
                assert_eq!(writes, 1);
            }
        }
    }

    #[test]
    fn command_readback_must_match_settings_and_header() {
        let (original, baseline) = fixture();
        let requested = HearingAidSettings {
            own_voice: 0.25,
            ..baseline
        };
        for changed_header in [false, true] {
            let mut confirmed = original.clone();
            if changed_header {
                patch_settings(&mut confirmed, &requested);
                confirmed[..4].copy_from_slice(&[2, 0, 0x60, 0]);
            }
            let mut calls = 0;
            let result = write_settings(
                |request| {
                    calls += 1;
                    if calls == 1 {
                        assert_eq!(request[0], 0x52);
                        Ok(vec![])
                    } else {
                        assert_eq!(request, [0x0a, 0x39, 0]);
                        Ok([vec![0x0b], confirmed.clone()].concat())
                    }
                },
                Characteristic {
                    handle: 0x39,
                    write_opcode: Some(0x52),
                },
                original.clone(),
                &requested,
            );
            assert!(result.is_err());
            assert_eq!(calls, if changed_header { 2 } else { 6 });
        }
    }

    #[test]
    fn delayed_write_readback_retries_only_the_unchanged_original_value() {
        let (original, baseline) = fixture();
        let mut written = original.clone();
        let requested = HearingAidSettings { left_tone: 0.75, ..baseline };
        patch_settings(&mut written, &requested);
        for stale_reads in [0, 1, 4, 5] {
            let mut reads = 0;
            let mut waits = 0;
            let result = confirm_settings(&mut |request| {
                assert_eq!(request, att_read_req(TEST_SETTING.handle));
                reads += 1;
                Ok([vec![0x0b], if reads <= stale_reads { original.clone() } else { written.clone() }].concat())
            }, TEST_SETTING.handle, &original, &written, |delay| {
                assert_eq!(delay, Duration::from_millis(75));
                waits += 1;
            });
            assert_eq!(result.is_ok(), stale_reads < 5);
            assert_eq!(reads, (stale_reads + 1).min(5));
            assert_eq!(waits, stale_reads.min(4));
        }
        let mut unexpected = original.clone();
        put_f32(&mut unexpected, OFF_RIGHT_EQ, 50.0);
        assert!(confirm_settings(&mut |_| Ok([vec![0x0b], unexpected.clone()].concat()),
            TEST_SETTING.handle, &original, &written, |_| panic!("unrelated change retried")).is_err());
        assert!(confirm_settings(&mut |_| Err("session changed".into()),
            TEST_SETTING.handle, &original, &written, |_| panic!("failed read retried")).is_err());
    }

    #[test]
    fn a_new_enable_rediscovers_the_handle_before_writing_unchanged_pre_enable_curves() {
        let (original, baseline) = fixture();
        let mut write = vec![0x52, 0x3c, 0];
        write.extend_from_slice(&original);
        let mut replies = discovery(0x0a, 0x35);
        replies.push((vec![0x0a, 0x35, 0], [vec![0x0b], original.clone()].concat()));
        replies.extend(discovery(0x16, 0x3c));
        replies.extend([
            (vec![0x0a, 0x3c, 0], [vec![0x0b], original.clone()].concat()),
            (write, vec![]),
            (vec![0x0a, 0x3c, 0], [vec![0x0b], original].concat()),
        ]);
        let mut replies = replies.into_iter();
        assert_eq!(
            super::update_settings(
                |request| {
                    let (expected, response) = replies.next().expect("unexpected request");
                    assert_eq!(request, expected);
                    Ok(response)
                },
                &baseline,
                &baseline,
                || Ok(true)
            )
            .unwrap(),
            baseline
        );
        assert!(replies.next().is_none());
    }

    #[test]
    fn enable_cannot_overwrite_a_profile_that_changed_during_the_request() {
        let (original, baseline) = fixture();
        for offset in [OFF_LEFT_EQ, OFF_RIGHT_EQ, OFF_LEFT_TONE, OFF_OWN_VOICE] {
            let mut changed = original.clone();
            changed[offset] ^= 1;
            let mut replies = discovery(0x0a, 0x35);
            replies.push((vec![0x0a, 0x35, 0], [vec![0x0b], original.clone()].concat()));
            replies.extend(discovery(0x16, 0x3c));
            replies.push((vec![0x0a, 0x3c, 0], [vec![0x0b], changed].concat()));
            let mut replies = replies.into_iter();
            assert!(super::update_settings(
                |request| {
                    let (expected, response) = replies.next().expect("changed profile was overwritten");
                    assert_eq!(request, expected);
                    Ok(response)
                },
                &baseline,
                &baseline,
                || Ok(true),
            ).is_err());
            assert!(replies.next().is_none());
        }
    }

    #[test]
    fn unrelated_edits_preserve_signed_zero_boost_flags() {
        let (mut value, baseline) = fixture();
        put_f32(&mut value, OFF_LEFT_CONV, -0.0);
        put_f32(&mut value, OFF_RIGHT_CONV, -0.0);
        let requested = HearingAidSettings {
            left_tone: 0.75,
            left_conversation_boost: false,
            right_conversation_boost: false,
            ..baseline
        };
        patch_settings(&mut value, &requested);
        for offset in [OFF_LEFT_CONV, OFF_RIGHT_CONV] {
            assert_eq!(&value[offset..offset + 4], &(-0.0f32).to_le_bytes());
        }
        patch_settings(&mut value, &HearingAidSettings {
            right_conversation_boost: true,
            ..requested
        });
        assert_eq!(&value[OFF_LEFT_CONV..OFF_LEFT_CONV + 4], &(-0.0f32).to_le_bytes());
        assert_eq!(&value[OFF_RIGHT_CONV..OFF_RIGHT_CONV + 4], &1.0f32.to_le_bytes());
    }

    #[test]
    fn a_new_enable_never_writes_if_the_format_or_write_permission_changed() {
        let (mut original, baseline) = fixture();
        original[..4].copy_from_slice(&[2, 2, 0x64, 0]);
        for properties in [0x02, 0x16] {
            let mut changed = original.clone();
            changed[..4].copy_from_slice(&[2, 0, 0x60, 0]);
            let mut replies = discovery(0x0a, 0x35);
            replies.push((vec![0x0a, 0x35, 0], [vec![0x0b], original.clone()].concat()));
            replies.extend(discovery(properties, 0x3c));
            if properties == 0x16 {
                replies.push((vec![0x0a, 0x3c, 0], [vec![0x0b], changed].concat()));
            }
            let mut replies = replies.into_iter();
            assert!(super::update_settings(
                |request| {
                    let (expected, response) =
                        replies.next().expect("changed characteristic was written");
                    assert_eq!(request, expected);
                    Ok(response)
                },
                &baseline,
                &baseline,
                || Ok(true)
            )
            .is_err());
            assert!(replies.next().is_none());
        }
    }

    #[test]
    fn hearing_discovery_failure_never_reads_a_fixed_handle_or_enables() {
        let (_, baseline) = fixture();
        for response in [
            vec![1, 6, 1, 0, 0x0a],
            vec![7, 0x40, 0, 0x30, 0],
            vec![1, 6, 1, 0, 0x0e],
        ] {
            let mut calls = 0;
            assert!(super::update_settings(
                |request| {
                    calls += 1;
                    assert_eq!(request[0], 0x06);
                    Ok(response.clone())
                },
                &baseline,
                &baseline,
                || panic!("failed discovery enabled hearing")
            )
            .is_err());
            assert_eq!(calls, 1);
        }
    }

    #[test]
    fn scalar_edit_preserves_audiograms_and_unrelated_per_ear_settings() {
        let (original, baseline) = fixture();
        let requested = HearingAidSettings {
            left_tone: 0.75,
            ..baseline
        };
        let mut device = original.clone();
        let mut sent = Vec::new();
        let result = update_settings(
            |request| {
                sent.push(request.to_vec());
                match request[0] {
                    0x0a => Ok([vec![0x0b], device.clone()].concat()),
                    0x12 => {
                        device = request[3..].to_vec();
                        Ok(vec![0x13])
                    }
                    _ => panic!("unexpected operation"),
                }
            },
            &baseline,
            &requested,
            || Ok(false),
        )
        .unwrap();
        assert_eq!(result, requested);
        assert_eq!(sent.len(), 3);
        for (index, (&before, &after)) in original.iter().zip(&device).enumerate() {
            if !(OFF_LEFT_TONE..OFF_LEFT_TONE + 4).contains(&index) {
                assert_eq!(before, after, "unrelated byte {index} changed");
            }
        }
    }

    #[test]
    fn initial_enable_reads_original_curves_before_any_control_or_write() {
        use std::cell::Cell;
        let (original, baseline) = fixture();
        let read = Cell::new(false);
        let enabled = Cell::new(false);
        let mut settings_writes = 0;
        let result = update_settings(
            |request| {
                if request[0] == 0x0a {
                    read.set(true);
                    Ok([vec![0x0b], original.clone()].concat())
                } else {
                    assert!(enabled.get());
                    if request[1] == TEST_SETTING.handle as u8 {
                        settings_writes += 1;
                    }
                    Ok(vec![0x13])
                }
            },
            &baseline,
            &baseline,
            || {
                assert!(read.get());
                enabled.set(true);
                Ok(true)
            },
        );
        assert_eq!(result.unwrap(), baseline);
        assert_eq!(settings_writes, 1);
    }

    #[test]
    fn failed_or_stale_reads_never_enable_or_write() {
        let (original, baseline) = fixture();
        let mut changed = original.clone();
        put_f32(&mut changed, OFF_LEFT_EQ, 45.);
        for reply in [
            vec![1, 0x0a, 0x2a, 0, 0x0e],
            vec![0x0b; 104],
            [vec![0x0b], changed].concat(),
        ] {
            let mut calls = 0;
            let result = update_settings(
                |request| {
                    calls += 1;
                    assert_eq!(request, [0x0a, 0x35, 0]);
                    Ok(reply.clone())
                },
                &baseline,
                &baseline,
                || panic!("read failure must not enable"),
            );
            assert!(result.is_err());
            assert_eq!(calls, 1);
        }
    }

    #[test]
    fn invalid_or_unconfirmed_settings_cannot_report_success() {
        let (original, baseline) = fixture();
        for bad in [f32::NAN, f32::INFINITY, -1., 91.] {
            let mut requested = baseline;
            requested.right_eq[4] = bad;
            assert!(update_settings(
                |_| panic!("invalid settings reached device"),
                &baseline,
                &requested,
                || panic!("invalid settings enabled")
            )
            .is_err());
        }
        let requested = HearingAidSettings {
            own_voice: 0.25,
            ..baseline
        };
        let mut writes = 0;
        let result = write_settings(
            |request| {
                if request[0] == 0x12 {
                    writes += 1;
                    Ok(vec![0x13])
                } else {
                    Ok([vec![0x0b], original.clone()].concat())
                }
            },
            TEST_SETTING,
            original.clone(),
            &requested,
        );
        assert!(
            result.is_err(),
            "a write ACK alone must not confirm a setting"
        );
        assert_eq!(writes, 1);
        for size in [0, 103, 105] {
            assert!(parse_settings(&vec![0; size]).is_err());
        }
        let mut malformed = original;
        put_f32(&mut malformed, OFF_RIGHT_EQ, f32::NAN);
        assert!(parse_settings(&malformed).is_err());
        put_f32(&mut malformed, OFF_RIGHT_EQ, 20.);
        put_f32(&mut malformed, OFF_LEFT_CONV, 0.5);
        assert!(parse_settings(&malformed).is_err());
    }

    #[test]
    fn hearing_read_and_write_accept_notifications_before_each_reply() {
        let (mut device, baseline) = fixture();
        let requested = HearingAidSettings {
            left_tone: 0.5,
            ..baseline
        };
        let mut sent = Vec::new();
        let mut pending = false;
        let mut exchange = |request: &[u8]| {
            let reply = if request[0] == 0x0a {
                [vec![0x0b], device.clone()].concat()
            } else {
                if request[1] == TEST_SETTING.handle as u8 {
                    device = request[3..].to_vec();
                }
                vec![0x13]
            };
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
        assert_eq!(
            update_settings(&mut exchange, &baseline, &requested, || Ok(false)).unwrap(),
            requested
        );
        assert_eq!(sent[0], [0x0a, 0x35, 0]);
        assert_eq!(&sent[1][..3], [0x12, 0x35, 0]);
        assert_eq!(sent[2], [0x0a, 0x35, 0]);
        assert_eq!(sent.len(), 3);
    }

    #[test]
    fn hearing_settings_cancelled_during_read_consume_reply_without_writing() {
        use std::cell::Cell;
        for reconnect in [false, true] {
            let (original, baseline) = fixture();
            let current = Cell::new(true);
            let same_session = Cell::new(true);
            let mut pending = false;
            let mut sent = Vec::new();
            let result = update_settings(
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
                            response[0] = 0x0b;
                            response[1..105].copy_from_slice(&original);
                            Ok(105)
                        },
                    )
                },
                &baseline,
                &baseline,
                || {
                    if current.get() {
                        Ok(true)
                    } else {
                        Err("Cancelled".into())
                    }
                },
            );
            assert!(result.is_err());
            assert_eq!(sent, [vec![0x0a, 0x35, 0]]);
            assert_eq!(pending, reconnect);
        }
    }

    #[test]
    fn failed_initial_enable_disables_without_overwriting_curves() {
        use std::cell::Cell;
        let (original, baseline) = fixture();
        // Fail each enable-control step, the post-enable read, the write and readback.
        for fail_at in 0..6 {
            let started = Cell::new(false);
            let current = || true;
            let mut controls = Vec::new();
            let mut setting_writes = 0;
            let mut reads = 0;
            let result = update_settings(
                |request| match request[0] {
                    0x0a => {
                        reads += 1;
                        if (fail_at == 3 && reads == 2) || (fail_at == 5 && reads == 3) {
                            Err("read failed".into())
                        } else {
                            Ok([vec![0x0b], original.clone()].concat())
                        }
                    }
                    0x12 => {
                        setting_writes += 1;
                        if fail_at == 4 {
                            Err("settings write failed".into())
                        } else {
                            Ok(vec![0x13])
                        }
                    }
                    _ => panic!("unexpected ATT request"),
                },
                &baseline,
                &baseline,
                || {
                    started.set(true);
                    let mut step = 0;
                    hearing_controls(
                        true,
                        false,
                        current,
                        |packet| {
                            controls.push(packet.to_vec());
                            let failed = step == fail_at;
                            step += 1;
                            if failed {
                                Err(std::io::Error::other("enable failed"))
                            } else {
                                Ok(())
                            }
                        },
                        |_| {},
                    )?;
                    Ok(true)
                },
            );
            assert!(result.is_err());
            let result = recover_failed_enable(result, started.get(), false, current, || {
                hearing_controls(
                    false,
                    false,
                    current,
                    |packet| {
                        controls.push(packet.to_vec());
                        Ok(())
                    },
                    |_| {},
                )
            });
            assert!(result.unwrap_err().contains("disable was requested"));
            assert_eq!(
                &controls[controls.len() - 2..],
                [HA_OFF_33.to_vec(), HA_OFF_2C.to_vec()]
            );
            assert_eq!(setting_writes, usize::from(fail_at >= 4));
        }
    }

    #[test]
    fn enable_recovery_never_overrides_newer_requests_or_existing_assistance() {
        for (started, previously_on, current) in [
            (false, false, true),
            (true, true, true),
            (true, false, false),
        ] {
            let result = recover_failed_enable::<()>(
                Err("failed".into()),
                started,
                previously_on,
                || current,
                || panic!("must not disable existing or superseding intent"),
            );
            assert_eq!(result.unwrap_err(), "failed");
        }
        let result = recover_failed_enable::<()>(
            Err("readback failed".into()),
            true,
            false,
            || true,
            || Err("connection lost".into()),
        );
        assert!(result.unwrap_err().contains("may still be active"));
    }

    #[test]
    fn queued_enables_reject_intervening_unknown_off_or_reconnected_reports() {
        let _guard = HEARING_STATE.lock().unwrap();
        for report in [HA_OFF_2C.to_vec(), vec![4, 0, 4, 0, 9, 0, 0x2c, 9]] {
            observe_packet(&HA_ON_2C);
            let authorized = authorization();
            assert!(authorized.is_current());
            observe_packet(&report);
            assert!(!authorized.is_current());
            assert_ne!(
                authorized.off_revision,
                HEARING_OFF_REVISION.load(Ordering::SeqCst)
            );
        }
        observe_packet(&HA_ON_2C);
        let authorized = authorization();
        invalidate_session();
        assert!(!authorized.is_current());
    }

    #[test]
    fn att_errors_report_metadata_without_setting_payloads() {
        assert_eq!(
            att_error("Read failed.", &[1, 0x0a, 0x18, 0, 0x0e]),
            "Read failed. ATT request 0x0A, handle 0x0018, error 0x0E."
        );
        assert_eq!(
            att_error("Read failed.", &[0x0b, 10, 20, 30, 40, 50]),
            "Read failed."
        );
    }

    #[test]
    fn session_invalidation_does_not_wait_for_a_worker_or_keep_its_handshake() {
        let mut enabled = HEARING_STATE.lock().unwrap();
        let old_session = HEARING_SESSION.load(Ordering::SeqCst);
        HEARING_READ_SESSION.store(old_session, Ordering::SeqCst);
        HEARING_EDIT_SESSION.store(u64::MAX, Ordering::SeqCst);
        assert!(has_current_read());
        assert!(!can_edit_current_read());
        HEARING_EDIT_SESSION.store(old_session, Ordering::SeqCst);
        assert!(can_edit_current_read());
        *enabled = Some((old_session, HEARING_OFF_REVISION.load(Ordering::SeqCst)));
        let generation = next_gen();
        let loud_sound = loud_sound_token();
        let transparency = crate::transparency::token();
        ATT_FAILED_SESSION.store(old_session, Ordering::SeqCst);
        assert!(require_clean_att_session(old_session).is_err());
        observe_packet(&HA_OFF_33);
        observe_packet(&mode_fixture(false, false, false).pme.unwrap());
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
        assert!(!has_current_read());
        assert!(!can_edit_current_read());
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
        observe_packet(&HA_OFF_33);
        observe_packet(&mode_fixture(false, false, false).pme.unwrap());
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
    fn captured_unenrolled_hearing_report_is_off_and_rejects_unknown_neighbors() {
        let captured = [4, 0, 4, 0, 9, 0, 0x2c, 2, 0, 0, 0];
        assert_eq!(reported_hearing_mode(&captured), Some(false));
        for length in 0..captured.len() {
            assert_eq!(reported_hearing_mode(&captured[..length]), None);
        }
        let mut extended = captured.to_vec();
        extended.push(0);
        assert_eq!(reported_hearing_mode(&extended), None);
        for index in 0..captured.len() {
            let mut packet = captured;
            packet[index] = 3;
            assert_eq!(reported_hearing_mode(&packet), None);
        }
        for enrolled in [0, 1] {
            let mut packet = captured;
            packet[7] = enrolled;
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
    fn hearing_write_requires_exact_ack_before_readback() {
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
