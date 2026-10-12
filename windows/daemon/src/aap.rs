//! AAP protocol: outgoing commands + parsers for the packets the AirPods push.

pub const PSM_AACP: u16 = 0x1001;

pub const HANDSHAKE: [u8; 16] = [
    0x00, 0x00, 0x04, 0x00, 0x01, 0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];
pub const SET_FEATURES: [u8; 14] = [
    0x04, 0x00, 0x04, 0x00, 0x4D, 0x00, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];
pub const REQUEST_NOTIFS: [u8; 10] = [0x04, 0x00, 0x04, 0x00, 0x0F, 0x00, 0xFF, 0xFF, 0xFF, 0xFF];

/// Enable the hi-res (AAC-ELD) microphone stream — the AirPods start pushing
/// 0x58 uplink audio packets. Taken from LibrePods PR #655.
pub const START_AUDIO: [u8; 19] = [
    0x04, 0x00, 0x04, 0x00, 0x58, 0x00, 0x00, 0x00, 0x09, 0x00, 0x00, 0x01, 0x82, 0x00, 0x00, 0x00,
    0x04, 0x96, 0x00,
];
/// Stop the hi-res microphone stream.
pub const STOP_AUDIO: [u8; 12] = [
    0x04, 0x00, 0x04, 0x00, 0x58, 0x00, 0x00, 0x00, 0x02, 0x00, 0x03, 0x01,
];

// ---- AirPods Pro 3 RTBuddy heart-rate (PR #702) ----
//
// Enable = the AACP 1.3 init handshake (the four CONNECT/CAPABILITIES packets,
// sent RAW via the driver like `sendPacket` on Android) then a `sensor_stream`
// frame for the heart-rate stream. The init packets carry the `04 00 04 00`
// header inline; `sensor_stream` bakes it in too, so every constant here is a
// ready-to-send driver packet.
//
// The init packets are **unrefuted rather than confirmed**: every iOS capture
// so far began with the AACP session already established, so they were never
// seen on the wire. They are kept because the Android implementation sends them
// and nothing contradicts that.

/// AACP 1.3 init, service 0 — CONNECT (raw `sendPacket`).
pub const HR_CONNECT_SERVICE_0: [u8; 16] = [
    0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];
/// AACP 1.3 init, service 0 — CAPABILITIES (raw `sendPacket`).
pub const HR_CAPABILITIES_SERVICE_0: [u8; 7] = [0x04, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00];
/// AACP 1.3 init, service 4 — CONNECT (raw `sendPacket`).
pub const HR_CONNECT_SERVICE_4: [u8; 16] = [
    0x00, 0x00, 0x04, 0x00, 0x01, 0x00, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
];
/// AACP 1.3 init, service 4 — CAPABILITIES (raw `sendPacket`).
pub const HR_CAPABILITIES_SERVICE_4: [u8; 7] = [0x04, 0x00, 0x04, 0x00, 0x01, 0x00, 0x00];

/// HRM_STATE control command (id 0x30), value 0x01 = on — the switch that powers
/// the PPG measurement engine.
///
/// The enable step the working Android client (upstream PR #702, produces real BPM
/// on AirPods Pro 3) sends: `sendControlCommand(HRM_STATE=0x30, true)` right after the
/// AACP 1.3 session init and before the stream start. iOS reaches the engine by another
/// (hidden) path, so the iOS PacketLogger captures never showed 0x30; the Android path
/// is the reproducible one. NOTE: sending this makes our enable byte-match Android, but
/// on the A3063 test unit the AirPods still ACK service 19 and emit no data frames — so
/// it is necessary but, standalone, not sufficient here (see hr_retry_campaign).
pub const HR_ENABLE: [u8; 11] = [
    0x04, 0x00, 0x04, 0x00, 0x09, 0x00, 0x30, 0x01, 0x00, 0x00, 0x00,
];

// ---- Sensor stream control ----
//
// Streams are started and stopped with the `0x17` … `42 0B` frame family that
// `AAP Definitions.md` already documents for Head Tracking. The frame carries a
// stream id and a sampling period in microseconds; **a period of zero stops the
// stream**. Verified against four captures — see `AAP Definitions.md` →
// "Starting and Stopping Sensor Streams" for the alignment.
//
// There are **two forms** of this frame, and mixing them produces a packet that
// appears in no capture. Across 24 observed control frames the correlation is
// exact, with no exceptions:
//
//   form A — no `10 02` after the sequence, bare stream id (0x10, 0x12, 0x13)
//   form B — `10 02` after the sequence, stream id with bit 0x40 set (0x50, 0x52, 0x53)
//
// The payload length is not a constant: it is the real byte count, so it moves
// with both the form and the width of the sequence varint (0x10 for form A with a
// two-byte varint, 0x11 / 0x12 for form B).
//
// A session captured from the connection onwards uses **form A** to start heart
// rate:
//
//   t=364.35  ->  08 13 ... period 1000000     heart rate at 1 Hz
//   t=365.92  <-  first type-19 frame          (+1.57 s)
//
// Captures that begin mid-session show only form B, which is where an earlier
// revision here got 0x53 and the `10 02` field from. That revision also fixed the
// length at 17, which is only right for form B with a one-byte varint. This is
// form A, matching both the fresh-session capture and the original constant.
//
// The Windows symptom that prompted the recheck fits: raw PPG came up but heart
// rate never did, on a daemon sending form B into a freshly established session.

/// Stream id for heart rate — data type 19.
pub const STREAM_HEART_RATE: u8 = 0x54; // HEARTRATE_COMMAND — newer firmware (version3 first digit >= 8)
pub const STREAM_HEART_RATE_LEGACY: u8 = 0x13; // HEARTRATE — older firmware
/// Stream id for 6-axis device motion — data type 16 (DEVMOTION6). This was
/// mislabelled "raw PPG": the RTBuddy schema's ServiceType enum is 16=DEVMOTION6,
/// 19=HEARTRATE. It is motion, unrelated to heart rate (Android never sends it),
/// so it is NOT part of the HR enable. LibrePods uses it for head tracking on
/// firmware 8 and newer; the motion decoder lives in head_tracking.rs.
pub const STREAM_DEVMOTION6: u8 = 0x10;
/// Stream id 14 is the ACTIVITY classifier on current firmware. LibrePods also
/// uses this service for 58-byte head-tracking records on firmware older than 8;
/// its 23-byte activity records must never be decoded as movement.
pub const STREAM_ACTIVITY: u8 = 0x0E;

/// One-second sampling period, in microseconds — the cadence iOS uses for heart rate.
pub const PERIOD_HEART_RATE_US: u32 = 1_000_000;
/// 50 Hz sampling period, in microseconds — the cadence iOS uses for raw PPG.
pub const PERIOD_PPG_US: u32 = 20_000;

/// Build a sensor stream control frame, ready to send via the driver.
///
/// `seq` is the sequence number the phone increments per control frame. It is
/// encoded as a **two-byte varint**, which is what both the captures and the
/// original constant here used, and what makes the payload length come out at
/// 16 — so pass a value that stays inside 14 bits and just count up. Whether the
/// AirPods validate it is untested.
///
/// Pass `period_us = 0` to stop the stream.
///
/// Reproduces the captured heart-rate start frame byte-for-byte (`seq = 152`
/// there, encoding as `98 01`):
/// `04 00 04 00 17 00 00 00 10 00 10 00 08 98 01 42 0b 08 13 10 02 1a 05 01 40 42 0f 00`
pub fn sensor_stream(seq: u16, stream_id: u8, period_us: u32) -> [u8; 28] {
    // Two-byte varint: low 7 bits with the continuation bit, then the next 7.
    let s0 = 0x80 | (seq & 0x7F) as u8;
    let s1 = ((seq >> 7) & 0x7F) as u8;
    let p = period_us.to_le_bytes();
    [
        0x04, 0x00, 0x04, 0x00, // header
        0x17, 0x00, 0x00, 0x00, // opcode
        0x10, 0x00, // service
        0x10, 0x00, // payload length = 16
        0x08, s0, s1, // sequence, two-byte varint
        0x42, 0x0B, // field 8, 11 bytes
        0x08, stream_id, //   stream id
        0x10, 0x02, //   field 2 = 2
        0x1A, 0x05, //   field 3, 5 bytes
        0x01, p[0], p[1], p[2], p[3], // mode 1 + period µs, little-endian
    ]
}

/// Kavish's confirmed-working heart-rate START frame (from the LibrePods maintainer, Discord
/// 2026-08-13: "this is what finally worked for me"). On newer AirPods Pro 3 firmware
/// the HR service id MOVED: it's now **84 (0x54)**, not 19 (0x13) — and the top-level
/// message carries an extra field-2=2 vs the generic `sensor_stream`. 1 Hz (period
/// 1 000 000 µs, the last 4 bytes = UINT32 LE). `seq` is a plain request counter (its
/// value is irrelevant); keep it < 128 so it stays a single-byte varint like his.
/// The maintainer's `setSensorServiceReportInterval` frame — sets a sensor service's
/// report interval (START = 1 Hz / period 1e6 µs, STOP = period 0). `service` is
/// HEARTRATE_COMMAND (84) or HEARTRATE (19) per firmware. Matches his
/// `SensorDataWX{ ServiceSettings{ service, setting=2, config=0x01+interval_µs_LE } }`.
pub fn hr_stream(seq: u8, service: u8, period_us: u32) -> [u8; 29] {
    let p = period_us.to_le_bytes();
    [
        0x04,
        0x00,
        0x04,
        0x00, // header
        0x17,
        0x00,
        0x00,
        0x00, // opcode (BuddyCommand)
        0x10,
        0x00, // descriptor (SensorDataWX)
        0x11,
        0x00, // payload length = 17
        0x08,
        seq & 0x7F, // sequence, single-byte varint
        0x10,
        0x02, // top-level field 2 = 2
        0x42,
        0x0B, // field 8 (ServiceSettings), 11 bytes
        0x08,
        service, //   service (84 = HEARTRATE_COMMAND / 19 = HEARTRATE)
        0x10,
        0x02, //   setting = 2
        0x1A,
        0x05, //   config, 5 bytes
        0x01,
        p[0],
        p[1],
        p[2],
        p[3], // 0x01 + interval µs, little-endian
    ]
}

/// SensorDataWX `request_all_descriptors` (protobuf field 4, `22 00` = empty
/// message) on the Sensor Data WX service. A *named discovery call* from the
/// RTBuddy schema (pabloaul/apple-wireshark): iOS sends it twice at session open —
/// once without `log_type`, once with `log_type=2` — before any stream. The daemon
/// never sent these. Two-byte varint seq → length field 5 (no log_type) or 7.
pub fn request_all_descriptors(seq: u16, log_type: bool) -> Vec<u8> {
    let s0 = 0x80 | (seq & 0x7F) as u8;
    let s1 = ((seq >> 7) & 0x7F) as u8;
    let mut payload = vec![0x08, s0, s1];
    if log_type {
        payload.extend_from_slice(&[0x10, 0x02]); // log_type = 2
    }
    payload.extend_from_slice(&[0x22, 0x00]); // field 4 (request_all_descriptors), empty
    let len = (payload.len() as u16).to_le_bytes();
    let mut f = vec![
        0x04, 0x00, 0x04, 0x00, 0x17, 0x00, 0x00, 0x00, 0x10, 0x00, len[0], len[1],
    ];
    f.extend_from_slice(&payload);
    f
}

/// Parse the `0x2E` **Connected Devices** packet: the list of hosts the AirPods
/// are currently connected to. Returns each device's 48-bit address.
///
/// This is the multipoint ground truth, and the only party that has it is the
/// buds themselves — measured 2026-08-31, during a Teams call the AirPods moved
/// the audio to the phone while the Windows endpoint, the Bluetooth link and our
/// AAP channel all still reported healthy. `0x2E` announced the phone **3.2
/// seconds before** Windows noticed anything.
///
/// Layout, from two captures (one device, then two):
///
/// ```text
/// 04 00 04 00 2e 00 | 01 00 01 | 20 bd 1d 72 90 9c 02 06
/// 04 00 04 00 2e 00 | 01 22 02 | 68 44 65 83 8a 74 01 00 | 20 bd 1d 72 90 9c 02 06
///                      |  |  `- count                        `- this PC's radio
///                      |  `- flags
///                      `- constant 0x01
/// ```
///
/// Each entry is 8 bytes: a 6-byte address then 2 bytes of per-device flags
/// (this host showed `02 06`, the phone `01 00`). The flags are NOT interpreted
/// here — two samples is not enough to know what they mean, and the count plus
/// the addresses already answer the question we care about.
///
/// The address is **big-endian**, i.e. it reads in the same order as a
/// `BLUETOOTH_ADDRESS.ullLong`, so the two compare directly. (First written as
/// little-endian, which produced this machine's radio byte-reversed —
/// `9c90721dbd20` against the real `20bd1d72909c` — and so never matched.)
pub fn parse_connected_devices(data: &[u8]) -> Option<Vec<(u64, [u8; 2])>> {
    if data.len() < 9 || data[..4] != HEADER || data[4] != 0x2E {
        return None;
    }
    let count = data[8] as usize;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let base = 9 + i * 8;
        if base + 8 > data.len() {
            break;
        }
        let mut addr = 0u64;
        for b in &data[base..base + 6] {
            addr = (addr << 8) | *b as u64; // big-endian, matching BLUETOOTH_ADDRESS
        }
        out.push((addr, [data[base + 6], data[base + 7]]));
    }
    Some(out)
}

/// The two per-device flag bytes of an 0x2E entry are **not** a device class.
///
/// A first version of this guessed a device type from them, on one capture where
/// this PC was `02 06` and a phone `01 00`. More data killed it: over 23 packets
/// this same PC appeared as `02 06` (15x), `01 06` (4x) and `02 04` (4x). A value
/// that changes for a fixed device cannot be its class — it looks like connection
/// or stream state. Until we know what it means, no label is inferred from it: a
/// wrong "Computer" next to the user's iPhone is worse than an honest "Unknown".
///
/// Kept as a formatter so the bytes still reach the log, where more samples can
/// eventually settle what they are.
pub fn flags_hex(flags: [u8; 2]) -> String {
    format!("{:02x} {:02x}", flags[0], flags[1])
}

/// Audio-route diagnostics: the opcodes `AAP Definitions.md` names as carrying
/// routing/stream state, which the daemon does not yet interpret.
///
/// Why they matter (measured 2026-08-31, a Teams call): the AirPods can move the
/// audio to the phone while the AAP channel keeps flowing, the Bluetooth link
/// stays connected and the Windows endpoint still reports OK — so *every* signal
/// the daemon watches today reads healthy and it never notices. The buds are the
/// only party that knows, and if they announce it at all it is on one of these:
///
///   0x2B Stream State Info · 0x44 Send Smart Routing 2.0 Info
///   0x52 Source Context    · 0x2E Connected Devices (carries BT addresses)
///
/// Returns the opcode and a hex dump so a reproduction shows which one fires and
/// what it says. Once that is known, this becomes the reclaim trigger.
pub fn route_diag(data: &[u8]) -> Option<(u8, String)> {
    if data.len() < 6 || data[..4] != HEADER {
        return None;
    }
    let op = data[4];
    if !matches!(op, 0x2B | 0x2E | 0x44 | 0x52) {
        return None;
    }
    let dump = data
        .iter()
        .take(64)
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    Some((op, dump))
}

/// Human-readable name for the opcodes `route_diag` reports.
pub fn route_opcode_name(op: u8) -> &'static str {
    match op {
        0x2B => "Stream State Info",
        0x2E => "Connected Devices",
        0x44 => "Smart Routing 2.0 Info",
        0x52 => "Source Context",
        _ => "?",
    }
}

/// All microphone protocol messages stay out of general IPC and diagnostics,
/// including malformed audio and control acknowledgements.
pub fn is_microphone_message(data: &[u8]) -> bool {
    data.len() >= 6 && data[..4] == HEADER && data[4..6] == [0x58, 0]
}

pub fn is_audio_packet(data: &[u8]) -> bool {
    data.len() >= 22 && is_microphone_message(data) && data[6..8] == [1, 0]
}

/// Validate the whole SDU before decoding any access unit. Each record is a
/// little-endian timestamp, an 8-bit length and AAC-ELD bytes.
pub fn for_each_au(sdu: &[u8], mut f: impl FnMut(u32, &[u8])) -> bool {
    if !is_audio_packet(sdu) || sdu.len() == 22 {
        return false;
    }
    let mut off = 22;
    while off < sdu.len() {
        if sdu.len() - off < 5 {
            return false;
        }
        let n = sdu[off + 4] as usize;
        if n == 0 || n > sdu.len() - off - 5 {
            return false;
        }
        off += 5 + n;
    }
    off = 22;
    while off < sdu.len() {
        let end = off + 5 + sdu[off + 4] as usize;
        let timestamp = u32::from_le_bytes(sdu[off..off + 4].try_into().unwrap());
        f(timestamp, &sdu[off + 5..end]);
        off = end;
    }
    true
}

/// Listening-mode (ANC) control command. value = mode (1 off, 2 anc, 3 transparency, 4 adaptive).
pub fn anc_command(mode: u8) -> [u8; 11] {
    control_command(0x0D, mode)
}

/// Generic AAP control command (opcode 0x09): `[HEADER, 0x09, 0x00, id, value, 0,0,0]`.
/// ANC and the boolean feature toggles are all this shape.
pub fn control_command(id: u8, value: u8) -> [u8; 11] {
    device_control_command(id, value.into())
}

pub fn device_control_command(id: u8, value: u32) -> [u8; 11] {
    let value = value.to_le_bytes();
    [
        0x04, 0x00, 0x04, 0x00, 0x09, 0x00, id, value[0], value[1], value[2], value[3],
    ]
}

pub fn parse_device_control(data: &[u8]) -> Option<(u8, u32)> {
    if data.len() != 11 || data[..6] != [0x04, 0x00, 0x04, 0x00, 0x09, 0x00] {
        return None;
    }
    let value = u32::from_le_bytes(data[7..11].try_into().ok()?);
    super::device_settings::valid_control(data[6], value).then_some((data[6], value))
}

pub fn eq_command(eq: ntpods_ipc::Equalizer) -> [u8; 13] {
    [
        0x04, 0x00, 0x04, 0x00, 0x63, 0x00, 0x05, 0x00, 0x01, eq.mode, eq.low, eq.mid, eq.high,
    ]
}

pub fn parse_eq(data: &[u8]) -> Option<ntpods_ipc::Equalizer> {
    if data.len() != 13 || data[..9] != [0x04, 0x00, 0x04, 0x00, 0x63, 0x00, 0x05, 0x00, 0x01] {
        return None;
    }
    let eq = ntpods_ipc::Equalizer {
        mode: data[9],
        low: data[10],
        mid: data[11],
        high: data[12],
    };
    super::device_settings::valid_equalizer(eq).then_some(eq)
}

/// Boolean feature toggle: 0x01 = on, 0x02 = off (matches the AirPods encoding).
pub fn feature_command(id: u8, on: bool) -> [u8; 11] {
    control_command(id, if on { 0x01 } else { 0x02 })
}

/// If `data` is a control-command status for `id` (opcode 0x09), return its value byte.
pub fn parse_control_value(data: &[u8], id: u8) -> Option<u8> {
    if data.len() >= 8 && data[..4] == HEADER && data[4] == 0x09 && data[6] == id {
        Some(data[7])
    } else {
        None
    }
}

/// Boolean status is confirmed only by a complete control record with value 1/2.
pub fn parse_feature_status(data: &[u8], id: u8) -> Option<bool> {
    if data.len() < 11 || data[5] != 0 {
        return None;
    }
    match parse_control_value(data, id)? {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

/// Parse a rename packet the app sends over the proxy: `[HEADER, 0x1A, 0x00,
/// 0x01, size, 0x00, ...name]`. Returns the new device name.
pub fn parse_rename(data: &[u8]) -> Option<String> {
    if data.len() >= 9 && data[..4] == HEADER && data[4] == 0x1A {
        let size = data[7] as usize;
        if data.len() >= 9 + size {
            return String::from_utf8(data[9..9 + size].to_vec()).ok();
        }
    }
    None
}

/// Build a rename command for the AirPods (the inverse of `parse_rename`):
/// `[HEADER, 0x1A, 0x00, 0x01, size, 0x00, ...name]`.
pub fn build_rename(name: &str) -> Vec<u8> {
    let bytes = name.as_bytes();
    let mut f = vec![
        0x04,
        0x00,
        0x04,
        0x00,
        0x1A,
        0x00,
        0x01,
        bytes.len() as u8,
        0x00,
    ];
    f.extend_from_slice(bytes);
    f
}

/// Conversational Awareness event (opcode 0x4B): the AirPods signal that you
/// started/stopped speaking; the status byte drives the host-side volume duck.
/// 1 = start, 2 = reduce, 3 = partial, 4/6/7/8/9 = end.
pub fn parse_conversational_awareness(data: &[u8]) -> Option<u8> {
    if data.len() >= 10 && data[..4] == HEADER && data[4..6] == [0x4B, 0] {
        Some(data[9])
    } else {
        None
    }
}

pub fn anc_name(mode: u8) -> &'static str {
    match mode {
        1 => "Off",
        2 => "Noise Cancellation",
        3 => "Transparency",
        4 => "Adaptive",
        _ => "?",
    }
}

#[derive(Default, Clone, Copy)]
pub struct Battery {
    reported: u8,
    pub headphone: Option<u8>,
    pub left: Option<u8>,
    pub right: Option<u8>,
    pub case: Option<u8>,
    // Per-component charging flag (status byte 0x01 charging, 0x05 charging in case).
    pub headphone_charging: bool,
    pub left_charging: bool,
    pub right_charging: bool,
    pub case_charging: bool,
}

impl Battery {
    pub fn apply_to_state(self, state: &mut ntpods_ipc::Snapshot, now_ms: u64) {
        if self.reported == 0 {
            return;
        }
        self.apply_to(&mut state.battery);
        let freshness = state.battery_freshness.get_or_insert_with(Default::default);
        for (bit, value, age) in [
            (1, self.headphone, &mut freshness.headphone),
            (2, self.right, &mut freshness.right),
            (4, self.left, &mut freshness.left),
            (8, self.case, &mut freshness.case),
        ] {
            if self.reported & bit != 0 {
                age.present = Some(value.is_some());
                if value.is_some() {
                    age.timestamp_ms = Some(now_ms);
                }
                age.live = value.is_some();
            }
        }
        state.battery_source = "aap".into();
        state.battery_timestamp_ms = Some(now_ms);
    }

    pub fn apply_to(self, battery: &mut ntpods_ipc::Battery) {
        for (bit, level, charging, dest, dest_charging) in [
            (
                1,
                self.headphone,
                self.headphone_charging,
                &mut battery.headphone,
                &mut battery.headphone_charging,
            ),
            (
                2,
                self.right,
                self.right_charging,
                &mut battery.right,
                &mut battery.right_charging,
            ),
            (
                4,
                self.left,
                self.left_charging,
                &mut battery.left,
                &mut battery.left_charging,
            ),
            (
                8,
                self.case,
                self.case_charging,
                &mut battery.case,
                &mut battery.case_charging,
            ),
        ] {
            if self.reported & bit != 0 {
                if level.is_some() {
                    *dest = level;
                }
                *dest_charging = level.is_some() && charging;
            }
        }
    }
}

const HEADER: [u8; 4] = [0x04, 0x00, 0x04, 0x00];

/// If `data` is a battery packet (opcode 0x04), return the parsed levels.
pub fn parse_battery(data: &[u8]) -> Option<Battery> {
    if data.len() < 7 || data[..4] != HEADER || data[4..6] != [0x04, 0] {
        return None;
    }
    let payload = &data[4..]; // starts at opcode
    let count = payload[2] as usize;
    if count == 0 || payload.len() < 3 + count * 5 {
        return None;
    }
    let mut b = Battery::default();
    for i in 0..count {
        let base = 3 + i * 5;
        // status 0x04 = component not connected/present; level 0xFF (255) = the
        // slot exists but is absent (e.g. the headphone slot on earbuds) -> both
        // report as unknown. (0x05 = charging in case, level valid.)
        let status = payload[base + 3];
        let raw = payload[base + 2];
        if raw > 100 && raw != 0xFF {
            return None;
        }
        let level = if status == 0x04 || raw == 0xFF {
            None
        } else {
            Some(raw)
        };
        // 0x01 = charging, 0x05 = charging while in the case; both mean "charging".
        let charging = level.is_some() && matches!(status, 0x01 | 0x05);
        if matches!(payload[base], 1 | 2 | 4 | 8) {
            b.reported |= payload[base];
        }
        match payload[base] {
            0x01 => {
                b.headphone = level;
                b.headphone_charging = charging;
            }
            0x02 => {
                b.right = level;
                b.right_charging = charging;
            }
            0x04 => {
                b.left = level;
                b.left_charging = charging;
            }
            0x08 => {
                b.case = level;
                b.case_charging = charging;
            }
            _ => {}
        }
    }
    Some(b)
}

/// In-ear state of one earbud, as reported by the AAP ear-detection packet.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EarStatus {
    InEar,
    OutOfEar,
    InCase,
    Disconnected,
    /// 0x04 — a transitional value the iOS capture found: emitted only while a bud
    /// is *in motion* between resting states, never at rest. Callers should hold
    /// the previous state rather than act on it, to avoid false auto-pauses.
    Transitional,
}

impl EarStatus {
    fn from_byte(b: u8) -> EarStatus {
        match b {
            0x00 => EarStatus::InEar,
            0x01 => EarStatus::OutOfEar,
            0x02 => EarStatus::InCase,
            0x03 => EarStatus::Disconnected,
            0x04 => EarStatus::Transitional,
            _ => EarStatus::OutOfEar, // anything unexpected
        }
    }
    pub fn in_ear(self) -> bool {
        self == EarStatus::InEar
    }
    /// True for the 0x04 in-motion value — callers should keep the prior state.
    pub fn is_transitional(self) -> bool {
        self == EarStatus::Transitional
    }
}

/// If `data` is an ear-detection packet (opcode 0x06), return the (primary,
/// secondary) earbud statuses. Which physical bud is "primary" varies, so
/// callers should treat them symmetrically (e.g. "is any bud in ear").
pub fn parse_ear_detection(data: &[u8]) -> Option<(EarStatus, EarStatus)> {
    if data.len() >= 8
        && data[..4] == HEADER
        && data[4..6] == [0x06, 0]
        && data[6] <= 4
        && data[7] <= 4
    {
        Some((EarStatus::from_byte(data[6]), EarStatus::from_byte(data[7])))
    } else {
        None
    }
}

/// In-motion reports retain the last stable state for playback and mic decoding.
pub fn in_ear_state(previous: [bool; 2], current: [EarStatus; 2]) -> [bool; 2] {
    std::array::from_fn(|i| {
        if current[i].is_transitional() {
            previous[i]
        } else {
            current[i].in_ear()
        }
    })
}

/// Model, public firmware version and serial from the information packet.
pub fn parse_metadata(data: &[u8]) -> Option<(String, String, String)> {
    if data.len() < 11 || data[..4] != HEADER || data[4..6] != [0x1D, 0] {
        return None;
    }
    // The five-byte information header precedes UTF-8 fields; UUIDs follow field 10.
    let mut remaining = &data[11..];
    let mut strings = Vec::with_capacity(11);
    for _ in 0..11 {
        let end = remaining.iter().position(|&b| b == 0)?;
        let value = std::str::from_utf8(&remaining[..end]).ok()?;
        if value.chars().any(char::is_control) {
            return None;
        }
        strings.push(value);
        remaining = &remaining[end + 1..];
    }
    let model = strings[1].to_owned();
    let serial = strings[3].to_owned();
    let firmware = [strings[10], strings[4]]
        .into_iter()
        .find(|value| crate::device_models::firmware_major(value) > 0)
        .map(str::to_owned)
        .or_else(|| packed_release_firmware(strings[10]))
        .unwrap_or_default();
    if model.is_empty() && firmware.is_empty() {
        return None;
    }
    Some((model, firmware, serial))
}

fn packed_release_firmware(value: &str) -> Option<String> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let packed: u32 = value.parse().ok()?;
    let major = packed >> 20;
    if !(1..=99).contains(&major) || packed & 0xF != 0 {
        return None;
    }
    // Inferred from release reports 6357536=6B34, 6619472=6F21 and 9442752=9A348.
    // Unknown prerelease low-nibble values are deliberately not decoded.
    let branch = char::from(b'A' + ((packed >> 16) & 0xF) as u8);
    let build = (packed >> 4) & 0xFFF;
    Some(format!("{major}{branch}{build}"))
}

/// If `data` reports the listening mode (control command 0x09, id 0x0D),
/// return the mode value.
pub fn parse_anc_mode(data: &[u8]) -> Option<u8> {
    if data.len() >= 8 && data[..4] == HEADER && data[4] == 0x09 && data[6] == 0x0D {
        Some(data[7])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_reports_keep_absent_components_without_erasing_omitted_ones() {
        let mut state = ntpods_ipc::Battery {
            left: Some(60),
            right: Some(61),
            case: Some(90),
            case_charging: true,
            ..Default::default()
        };
        let packet = [4, 0, 4, 0, 4, 0, 2, 4, 1, 55, 2, 0, 8, 1, 255, 4, 0];
        parse_battery(&packet).unwrap().apply_to(&mut state);
        assert_eq!(state.left, Some(55));
        assert_eq!(state.right, Some(61));
        assert_eq!(state.case, Some(90));
        assert!(!state.case_charging);
        for length in 0..packet.len() {
            assert!(parse_battery(&packet[..length]).is_none());
        }
        for invalid in 101..255 {
            let mut bad = packet;
            bad[9] = invalid;
            assert!(parse_battery(&bad).is_none());
        }
        let mut bad_opcode = packet;
        bad_opcode[5] = 1;
        assert!(parse_battery(&bad_opcode).is_none());
        let unknown = [4, 0, 4, 0, 4, 0, 1, 1, 1, 255, 1, 0];
        let battery = parse_battery(&unknown).unwrap();
        assert_eq!(battery.headphone, None);
        assert!(!battery.headphone_charging);
    }

    #[test]
    fn metadata_uses_public_firmware_and_preserves_utf8_short_and_empty_fields() {
        let fields = [
            "Écouteurs",
            "A3063",
            "Apple Inc.",
            "EXAMPLE",
            "90.3431.6807",
            "",
            "1",
            "com.apple.accessory.updater",
            "LEFT",
            "RIGHT",
            "9A348",
        ];
        let mut packet = vec![4, 0, 4, 0, 0x1D, 0, 2, 0, 0, 4, 0];
        for field in fields {
            packet.extend_from_slice(field.as_bytes());
            packet.push(0);
        }
        let length = (packet.len() - 9) as u16;
        packet[7..9].copy_from_slice(&length.to_le_bytes());
        assert_eq!(
            parse_metadata(&packet),
            Some(("A3063".into(), "9A348".into(), "EXAMPLE".into()))
        );
        for length in 0..packet.len() {
            assert!(parse_metadata(&packet[..length]).is_none());
        }
        packet[5] = 1;
        assert!(parse_metadata(&packet).is_none());
    }

    #[test]
    fn metadata_normalizes_release_builds_without_using_internal_versions_as_a_major() {
        for (model, version1, version3, expected) in [
            ("A2084", "90.3431.6807", "6619472", "6F21"),
            ("A2084", "6F21", "6619472", "6F21"),
            ("A3063", "90.3431.6807", "9442752", "9A348"),
            ("A2084", "", "6357536", "6B34"),
            ("A3063", "9A348", "", "9A348"),
            ("A3063", "9A348", "9A5347a", "9A5347a"),
            ("A1523", "6.8.8", "", "6.8.8"),
            ("A3063", "90.3431.6807", "", ""),
            ("A3063", "6619472", "unknown", ""),
            ("A3063", "90.3431.6807", "6619473", ""),
            ("A3063", "90.3431.6807", "4294967296", ""),
            ("A3063", "90.3431.6807", "0", ""),
        ] {
            let fields = [
                "AirPods",
                model,
                "Apple Inc.",
                "EXAMPLE",
                version1,
                "",
                "1",
                "com.apple.accessory.updater",
                "LEFT",
                "RIGHT",
                version3,
            ];
            let mut packet = vec![4, 0, 4, 0, 0x1D, 0, 2, 0, 0, 4, 0];
            for field in fields {
                packet.extend_from_slice(field.as_bytes());
                packet.push(0);
            }
            assert_eq!(
                parse_metadata(&packet),
                Some((model.into(), expected.into(), "EXAMPLE".into()))
            );
        }
    }

    #[test]
    fn malformed_ear_reports_cannot_pause_playback_or_restart_the_microphone() {
        let mut packet = [4, 0, 4, 0, 6, 0, 0, 1];
        assert_eq!(
            parse_ear_detection(&packet),
            Some((EarStatus::InEar, EarStatus::OutOfEar))
        );
        for position in [6, 7] {
            for value in 5..=255 {
                let mut invalid = packet;
                invalid[position] = value;
                assert_eq!(parse_ear_detection(&invalid), None);
            }
        }
        packet[5] = 1;
        assert_eq!(parse_ear_detection(&packet), None);
    }

    #[test]
    fn adaptive_controls_match_android_rewrite_wire_format() {
        for strength in [0, 25, 50, 100] {
            let packet = device_control_command(0x2E, strength);
            assert_eq!(packet, [4, 0, 4, 0, 9, 0, 0x2E, strength as u8, 0, 0, 0]);
            assert_eq!(parse_device_control(&packet), Some((0x2E, strength)));
        }
    }

    #[test]
    fn ear_handoffs_reset_only_on_stable_state_changes() {
        use EarStatus::*;
        let mut previous = [true, true];
        for (reported, expected, changed) in [
            ([Transitional, InEar], [true, true], false),
            ([OutOfEar, InEar], [false, true], true),
            ([OutOfEar, InEar], [false, true], false),
            ([Transitional, InEar], [false, true], false),
            ([InEar, InEar], [true, true], true),
            ([InEar, OutOfEar], [true, false], true),
            ([OutOfEar, OutOfEar], [false, false], true),
            ([InCase, Disconnected], [false, false], false),
            ([Transitional, Transitional], [false, false], false),
            ([InEar, InEar], [true, true], true),
        ] {
            let current = in_ear_state(previous, reported);
            assert_eq!(current, expected);
            assert_eq!(current != previous, changed);
            previous = current;
        }
    }
    #[test]
    fn feature_status_requires_a_complete_known_reply() {
        let enabled = [4, 0, 4, 0, 9, 0, 0x28, 1, 0, 0, 0];
        let mut reply = enabled;
        assert_eq!(parse_feature_status(&reply, 0x28), Some(true));
        reply[7] = 2;
        assert_eq!(parse_feature_status(&reply, 0x28), Some(false));
        for value in [0, 3, 0xff] {
            reply[7] = value;
            assert_eq!(parse_feature_status(&reply, 0x28), None);
        }
        assert_eq!(parse_feature_status(&enabled, 0x26), None);
        for len in 0..enabled.len() {
            assert_eq!(parse_feature_status(&enabled[..len], 0x28), None);
        }
        reply = enabled;
        reply[5] = 1;
        assert_eq!(parse_feature_status(&reply, 0x28), None);
        reply = enabled;
        reply[4] = 0x58;
        assert_eq!(parse_feature_status(&reply, 0x28), None);
    }

    #[test]
    fn device_controls_require_complete_valid_records_and_keep_both_value_bytes() {
        let record = device_control_command(0x24, 0x0200);
        assert_eq!(record, [4, 0, 4, 0, 9, 0, 0x24, 0, 2, 0, 0]);
        assert_eq!(parse_device_control(&record), Some((0x24, 0x0200)));
        for length in 0..record.len() {
            assert_eq!(parse_device_control(&record[..length]), None);
        }
        let mut extra = record.to_vec();
        extra.push(0);
        assert_eq!(parse_device_control(&extra), None);
        for offset in 0..6 {
            let mut wrong_header = record;
            wrong_header[offset] ^= 0xFF;
            assert_eq!(parse_device_control(&wrong_header), None);
        }
        assert_eq!(parse_device_control(&device_control_command(0x03, 1)), None);
        assert_eq!(parse_device_control(&device_control_command(0x23, 0)), None);
        assert_eq!(
            parse_device_control(&device_control_command(0x28, 0x0101)),
            None
        );
    }

    #[test]
    fn native_eq_matches_librepods_and_rejects_truncated_or_unknown_forms() {
        let eq = ntpods_ipc::Equalizer {
            mode: 2,
            low: 0,
            mid: 50,
            high: 100,
        };
        let packet = eq_command(eq);
        assert_eq!(packet, [4, 0, 4, 0, 0x63, 0, 5, 0, 1, 2, 0, 50, 100]);
        assert_eq!(parse_eq(&packet), Some(eq));
        for length in 0..packet.len() {
            assert_eq!(parse_eq(&packet[..length]), None);
        }
        for offset in 0..packet.len() {
            let mut malformed = packet;
            malformed[offset] = 0xFF;
            assert_eq!(parse_eq(&malformed), None, "offset={offset}");
        }
        let mut extra = packet.to_vec();
        extra.push(0);
        assert_eq!(parse_eq(&extra), None);
    }

    fn audio() -> Vec<u8> {
        let mut p = vec![0u8; 22];
        p[..8].copy_from_slice(&[4, 0, 4, 0, 0x58, 0, 1, 0]);
        p.extend_from_slice(&[180, 0, 0, 0, 3, 10, 11, 12]);
        p
    }
    #[test]
    fn microphone_payload_never_becomes_route_diagnostics() {
        let p = audio();
        assert!(is_microphone_message(&p));
        assert!(route_diag(&p).is_none());
        for n in 0..6 {
            assert!(!is_microphone_message(&p[..n]));
        }
    }
    #[test]
    fn rejects_every_truncated_audio_record_without_partial_delivery() {
        let p = audio();
        for n in 0..p.len() {
            let mut calls = 0;
            assert!(!for_each_au(&p[..n], |_, _| calls += 1));
            assert_eq!(calls, 0);
        }
        let mut received = Vec::new();
        assert!(for_each_au(&p, |stamp, au| received.push((stamp, au.to_vec()))));
        assert_eq!(received, vec![(180, vec![10, 11, 12])]);
        let mut bad = p.clone();
        bad.extend_from_slice(&[0, 0, 0, 0, 255, 1]);
        assert!(!for_each_au(&bad, |_, _| panic!(
            "partially validated packet"
        )));
    }
    #[test]
    fn arbitrary_packet_lengths_do_not_panic() {
        let mut seed = 7u32;
        for n in 0..8193 {
            let p: Vec<u8> = (0..n)
                .map(|_| {
                    seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                    (seed >> 24) as u8
                })
                .collect();
            let _ = for_each_au(&p, |_, _| {});
            let _ = parse_battery(&p);
            let _ = parse_ear_detection(&p);
            let _ = parse_anc_mode(&p);
        }
    }
}
