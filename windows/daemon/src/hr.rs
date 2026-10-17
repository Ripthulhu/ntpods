//! Bounded heart-rate decoding with device-announced report IDs and legacy fallback.

const AACP_RTBUDDY_HEADER_LENGTH: usize = 12;
const MAX_RTBUDDY_PAYLOAD_LENGTH: usize = 16 * 1024;

/// Firmware has emitted live records with both log types.
const LIVE_SENSOR_DATA_LOG_TYPES: [u64; 2] = [1, 3];

/// Different exact status trailers depending on whether one or both earbuds
/// participate in the session. `10 00 80` / `20 80 00` arrived with the iOS 27
/// firmware (upstream PR #702, 5e20986).
const KNOWN_HEART_RATE_STATUS_TAILS: [[u8; 3]; 6] = [
    [0x10, 0x00, 0x00],
    [0x10, 0x00, 0x80],
    [0x20, 0x00, 0x00],
    [0x20, 0x80, 0x00],
    [0x20, 0x02, 0x80],
    [0x20, 0x82, 0x80],
];

const FIELD_LOG_TYPE: u32 = 2;
const FIELD_SERVICE: u32 = 1;
const FIELD_COMMAND_PAYLOAD: u32 = 3;
// Legacy report IDs from LibrePods PR #702 and the maintainer's firmware notes:
// 8* uses service 19; 9* enables on 84 and reports on 20. Discovery takes precedence.
const HEART_RATE_REPORT_SERVICES: [u64; 3] = [84, 20, 19];
const HEART_RATE_PAYLOAD_LENGTH: usize = 18;
const HEART_RATE_BPM_OFFSET: usize = 1;
/// Confidence is about 20 during warm-up and 160..240 once locked (LibrePods PR #702
/// and Windows captures). Warm-up BPM can pass the range check, so filter on confidence too.
const HEART_RATE_CONFIDENCE_OFFSET: usize = 2;
const MIN_HEART_RATE_CONFIDENCE: u8 = 0x80;
const HEART_RATE_STATUS_TAIL_OFFSET: usize = 15;
const MIN_BPM: u8 = 30;
const MAX_BPM: u8 = 220;

const SENSOR_DATA_COMMAND_FIELDS: [u32; 5] = [5, 7, 8, 9, 12];
const MAX_COMMAND_ENVELOPE_DEPTH: u32 = 3;
const MAX_PAYLOAD_WRAPPER_DEPTH: u32 = 3;
const MAX_COMMANDS_PER_FRAME: usize = 16;
const MAX_PAYLOAD_CANDIDATES_PER_COMMAND: usize = 12;
const MAX_PROTO_MESSAGE_LENGTH: usize = MAX_RTBUDDY_PAYLOAD_LENGTH;
const MAX_PROTO_FIELDS: usize = 96;
const MAX_PROTO_FIELD_NUMBER: u32 = 4_096;

const WIRE_VARINT: u8 = 0;
const WIRE_FIXED64: u8 = 1;
const WIRE_LENGTH_DELIMITED: u8 = 2;
const WIRE_FIXED32: u8 = 5;

/// type=0x0004, service=0x0004, opcode=0x0017, descriptor=0x00100000 — the
/// AACP/RTBuddy SensorDataWX frame prefix (first 10 bytes of the 12-byte header).
const RTBUDDY_FRAME_PREFIX: [u8; 10] = [0x04, 0x00, 0x04, 0x00, 0x17, 0x00, 0x00, 0x00, 0x10, 0x00];

struct ProtoField {
    number: u32,
    wire_type: u8,
    varint_value: Option<u64>,
    value_start: usize,
    value_end: usize,
}

struct ProtoMessage {
    fields: Vec<ProtoField>,
}

impl ProtoMessage {
    fn first_varint(&self, field_number: u32) -> Option<u64> {
        self.fields
            .iter()
            .find(|f| f.number == field_number && f.wire_type == WIRE_VARINT)
            .and_then(|f| f.varint_value)
    }
}

struct VarintRead {
    value: u64,
    next_index: usize,
}

/// Candidate payloads from one heart-rate command, independent of its service ID.
struct HeartRateCommand {
    payload_candidates: Vec<Vec<u8>>,
}

/// Reassembles RTBuddy frames and extracts verified HEARTRATE samples.
pub struct RtBuddyHeartRateDecoder {
    carry: Vec<u8>,
    report_service: Option<u8>,
    awaiting_descriptor: bool,
}

/// One validated reading: BPM plus the sensor's confidence byte (payload[2]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HrSample {
    pub bpm: u16,
    pub confidence: u8,
}

impl Default for RtBuddyHeartRateDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl RtBuddyHeartRateDecoder {
    pub fn new() -> Self {
        RtBuddyHeartRateDecoder {
            carry: Vec::new(),
            report_service: None,
            awaiting_descriptor: false,
        }
    }

    /// Report discovery must not change the separate firmware-specific activation IDs.
    pub fn set_report_service(&mut self, service: Option<u8>) {
        self.awaiting_descriptor =
            (self.report_service.is_some() || self.awaiting_descriptor) && service.is_none();
        self.report_service = service;
        self.carry.clear();
    }

    /// Device-supplied mappings and partial frames belong to one connection.
    pub fn reset(&mut self) {
        self.carry.clear();
        self.report_service = None;
        self.awaiting_descriptor = false;
    }

    /// Feed one received chunk; returns any newly-decoded samples.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<HrSample> {
        let mut samples = Vec::new();
        if chunk.is_empty() || self.awaiting_descriptor {
            return samples;
        }

        let data: Vec<u8> = if self.carry.is_empty() {
            chunk.to_vec()
        } else {
            let mut d = std::mem::take(&mut self.carry);
            d.extend_from_slice(chunk);
            d
        };
        self.carry.clear();

        let mut cursor = 0usize;
        while cursor < data.len() {
            let frame_offset = match index_of_prefix(&data, &RTBUDDY_FRAME_PREFIX, cursor) {
                Some(off) => off,
                None => {
                    // No further frame starts; keep a partial prefix at the tail
                    // so it can complete with the next chunk.
                    let suffix_length =
                        longest_suffix_matching_prefix(&data, &RTBUDDY_FRAME_PREFIX, cursor);
                    if suffix_length > 0 {
                        let passthrough_end = data.len() - suffix_length;
                        self.carry = data[passthrough_end..].to_vec();
                    }
                    break;
                }
            };

            if data.len() - frame_offset < AACP_RTBUDDY_HEADER_LENGTH {
                self.carry = data[frame_offset..].to_vec();
                break;
            }

            let payload_length = read_le16(&data, frame_offset + 10);
            if payload_length > MAX_RTBUDDY_PAYLOAD_LENGTH {
                // The declared length is untrusted — drop the rest.
                break;
            }

            let frame_length = AACP_RTBUDDY_HEADER_LENGTH + payload_length;
            if data.len() - frame_offset < frame_length {
                self.carry = data[frame_offset..].to_vec();
                break;
            }

            let frame = &data[frame_offset..frame_offset + frame_length];
            if let Some(sample) = classify_frame(frame, self.report_service) {
                samples.push(sample);
            }
            cursor = frame_offset + frame_length;
        }

        samples
    }
}

/// Accept only live heart-rate frames with a valid payload, BPM and confidence.
fn classify_frame(frame: &[u8], report_service: Option<u8>) -> Option<HrSample> {
    let top_level = parse_proto_message(frame, AACP_RTBUDDY_HEADER_LENGTH, frame.len())?;

    let log_type = top_level
        .first_varint(FIELD_LOG_TYPE)
        .map(|v| v as i64)
        .unwrap_or(-1);
    let mut commands: Vec<HeartRateCommand> = Vec::new();

    for field in &top_level.fields {
        if field.wire_type == WIRE_LENGTH_DELIMITED
            && SENSOR_DATA_COMMAND_FIELDS.contains(&field.number)
            && commands.len() < MAX_COMMANDS_PER_FRAME
        {
            collect_heart_rate_commands(
                frame,
                field.value_start,
                field.value_end,
                0,
                report_service,
                &mut commands,
            );
        }
    }

    if commands.is_empty() {
        return None;
    }
    if !LIVE_SENSOR_DATA_LOG_TYPES.contains(&(log_type as u64)) {
        // Related but the wrong log type — reject (control/startup frame).
        return None;
    }

    let payloads: Vec<&Vec<u8>> = commands
        .iter()
        .flat_map(|c| c.payload_candidates.iter())
        .collect();

    payloads
        .iter()
        .find(|p| is_valid_heart_rate_payload(p))
        .map(|p| HrSample {
            bpm: p[HEART_RATE_BPM_OFFSET] as u16,
            confidence: p[HEART_RATE_CONFIDENCE_OFFSET],
        })
}

fn collect_heart_rate_commands(
    data: &[u8],
    start: usize,
    end: usize,
    depth: u32,
    report_service: Option<u8>,
    commands: &mut Vec<HeartRateCommand>,
) {
    if depth > MAX_COMMAND_ENVELOPE_DEPTH || commands.len() >= MAX_COMMANDS_PER_FRAME {
        return;
    }
    let message = match parse_proto_message(data, start, end) {
        Some(m) => m,
        None => return,
    };
    let service = message.first_varint(FIELD_SERVICE);

    if service.is_some_and(|s| {
        report_service.map_or_else(
            || HEART_RATE_REPORT_SERVICES.contains(&s),
            |id| s == u64::from(id),
        )
    }) {
        let mut payloads: Vec<Vec<u8>> = Vec::new();
        for field in &message.fields {
            if field.number == FIELD_COMMAND_PAYLOAD && field.wire_type == WIRE_LENGTH_DELIMITED {
                collect_payload_candidates(
                    data,
                    field.value_start,
                    field.value_end,
                    0,
                    &mut payloads,
                );
            }
        }
        commands.push(HeartRateCommand {
            payload_candidates: payloads,
        });
    }

    if depth == MAX_COMMAND_ENVELOPE_DEPTH {
        return;
    }
    for field in &message.fields {
        if field.wire_type == WIRE_LENGTH_DELIMITED && commands.len() < MAX_COMMANDS_PER_FRAME {
            collect_heart_rate_commands(
                data,
                field.value_start,
                field.value_end,
                depth + 1,
                report_service,
                commands,
            );
        }
    }
}

fn collect_payload_candidates(
    data: &[u8],
    start: usize,
    end: usize,
    depth: u32,
    candidates: &mut Vec<Vec<u8>>,
) {
    if candidates.len() >= MAX_PAYLOAD_CANDIDATES_PER_COMMAND {
        return;
    }

    let direct = data[start..end].to_vec();
    if !candidates.iter().any(|c| *c == direct) {
        candidates.push(direct);
    }
    if depth >= MAX_PAYLOAD_WRAPPER_DEPTH {
        return;
    }

    let wrapper = match parse_proto_message(data, start, end) {
        Some(m) => m,
        None => return,
    };
    for field in &wrapper.fields {
        if field.wire_type == WIRE_LENGTH_DELIMITED
            && candidates.len() < MAX_PAYLOAD_CANDIDATES_PER_COMMAND
        {
            collect_payload_candidates(
                data,
                field.value_start,
                field.value_end,
                depth + 1,
                candidates,
            );
        }
    }
}

fn is_valid_heart_rate_payload(payload: &[u8]) -> bool {
    if payload.len() != HEART_RATE_PAYLOAD_LENGTH {
        return false;
    }
    let bpm = payload[HEART_RATE_BPM_OFFSET];
    if bpm < MIN_BPM || bpm > MAX_BPM {
        return false;
    }
    if payload[HEART_RATE_CONFIDENCE_OFFSET] < MIN_HEART_RATE_CONFIDENCE {
        return false;
    }
    KNOWN_HEART_RATE_STATUS_TAILS.iter().any(|tail| {
        tail.iter()
            .enumerate()
            .all(|(i, b)| payload[HEART_RATE_STATUS_TAIL_OFFSET + i] == *b)
    })
}

fn parse_proto_message(data: &[u8], start: usize, end: usize) -> Option<ProtoMessage> {
    if end < start || end > data.len() || end - start > MAX_PROTO_MESSAGE_LENGTH {
        return None;
    }

    let mut fields: Vec<ProtoField> = Vec::new();
    let mut index = start;
    while index < end {
        if fields.len() >= MAX_PROTO_FIELDS {
            return None;
        }
        let key = read_varint(data, index, end)?;
        index = key.next_index;

        let field_number = (key.value >> 3) as u32;
        if field_number == 0 || field_number as u64 > MAX_PROTO_FIELD_NUMBER as u64 {
            return None;
        }
        let wire_type = (key.value & 0x07) as u8;

        match wire_type {
            WIRE_VARINT => {
                let value = read_varint(data, index, end)?;
                fields.push(ProtoField {
                    number: field_number,
                    wire_type,
                    varint_value: Some(value.value),
                    value_start: index,
                    value_end: value.next_index,
                });
                index = value.next_index;
            }
            WIRE_LENGTH_DELIMITED => {
                let length = read_varint(data, index, end)?;
                if length.value > usize::MAX as u64 {
                    return None;
                }
                let value_end = length.next_index.checked_add(length.value as usize)?;
                if value_end < length.next_index || value_end > end {
                    return None;
                }
                fields.push(ProtoField {
                    number: field_number,
                    wire_type,
                    varint_value: None,
                    value_start: length.next_index,
                    value_end,
                });
                index = value_end;
            }
            WIRE_FIXED64 => {
                if end - index < 8 {
                    return None;
                }
                fields.push(ProtoField {
                    number: field_number,
                    wire_type,
                    varint_value: None,
                    value_start: index,
                    value_end: index + 8,
                });
                index += 8;
            }
            WIRE_FIXED32 => {
                if end - index < 4 {
                    return None;
                }
                fields.push(ProtoField {
                    number: field_number,
                    wire_type,
                    varint_value: None,
                    value_start: index,
                    value_end: index + 4,
                });
                index += 4;
            }
            _ => return None,
        }
    }
    Some(ProtoMessage { fields })
}

fn read_varint(data: &[u8], start: usize, end: usize) -> Option<VarintRead> {
    let mut value: u64 = 0;
    let mut shift: u32 = 0;
    let mut index = start;

    while index < end && shift < 64 {
        let byte = data[index] as u64;
        index += 1;
        value |= (byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Some(VarintRead {
                value,
                next_index: index,
            });
        }
        shift += 7;
    }
    None
}

fn read_le16(data: &[u8], offset: usize) -> usize {
    (data[offset] as usize) | ((data[offset + 1] as usize) << 8)
}

/// Diagnostic prefix check; it does not validate a heart-rate reading.
pub fn contains_frame_prefix(data: &[u8]) -> bool {
    index_of_prefix(data, &RTBUDDY_FRAME_PREFIX, 0).is_some()
}

fn index_of_prefix(data: &[u8], prefix: &[u8], start_index: usize) -> Option<usize> {
    if prefix.is_empty() {
        return Some(start_index.min(data.len()));
    }
    if data.len() < prefix.len() {
        return None;
    }
    let last_start = data.len() - prefix.len();
    if start_index > last_start {
        return None;
    }
    for start in start_index..=last_start {
        if prefix
            .iter()
            .enumerate()
            .all(|(i, b)| data[start + i] == *b)
        {
            return Some(start);
        }
    }
    None
}

/// The longest suffix of `data[start_index..]` that matches a *prefix* of
/// `prefix` (i.e. a truncated frame header at the tail we should carry forward).
fn longest_suffix_matching_prefix(data: &[u8], prefix: &[u8], start_index: usize) -> usize {
    let start_index = start_index.min(data.len());
    let available = data.len() - start_index;
    let max_length = available.min(prefix.len().saturating_sub(1));
    for length in (1..=max_length).rev() {
        let start = data.len() - length;
        if (0..length).all(|i| data[start + i] == prefix[i]) {
            return length;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(service: u8) -> Vec<u8> {
        let mut report = [0; 18];
        report[0] = 1;
        report[1] = 72;
        report[2] = 160;
        report[15] = 0x10;
        let mut payload = vec![8, 1, 16, 1, 0x3a, 22, 8, service, 0x1a, 18];
        payload.extend_from_slice(&report);
        let mut frame = RTBUDDY_FRAME_PREFIX.to_vec();
        frame.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        frame.extend_from_slice(&payload);
        frame
    }

    #[test]
    fn discovery_changes_reports_without_guessing_activation_aliases() {
        let expected = vec![HrSample {
            bpm: 72,
            confidence: 160,
        }];
        let mut decoder = RtBuddyHeartRateDecoder::new();
        for id in [19, 20, 84] {
            assert_eq!(decoder.feed(&frame(id)), expected);
        }
        assert!(decoder.feed(&frame(27)).is_empty());
        decoder.set_report_service(Some(27));
        assert_eq!(decoder.feed(&frame(27)), expected);
        for id in [19, 20, 84] {
            assert!(decoder.feed(&frame(id)).is_empty());
        }
        decoder.set_report_service(None);
        for id in [19, 20, 27, 84] {
            assert!(decoder.feed(&frame(id)).is_empty());
        }
        decoder.set_report_service(None);
        assert!(decoder.feed(&frame(20)).is_empty());
        decoder.set_report_service(Some(28));
        assert_eq!(decoder.feed(&frame(28)), expected);
        assert!(decoder.feed(&frame(20)).is_empty());
        decoder.reset();
        assert_eq!(decoder.feed(&frame(20)), expected);
        assert!(decoder.feed(&frame(28)).is_empty());
    }

    #[test]
    fn descriptor_refresh_discards_partial_samples_and_keeps_payload_validation() {
        let mut decoder = RtBuddyHeartRateDecoder::new();
        let sample = frame(27);
        decoder.set_report_service(Some(27));
        assert!(decoder.feed(&sample[..24]).is_empty());
        decoder.set_report_service(Some(27));
        assert!(decoder.feed(&sample[24..]).is_empty());
        assert_eq!(decoder.feed(&sample).len(), 1);
        let mut warmup = sample.clone();
        warmup[24] = 1;
        assert!(decoder.feed(&warmup).is_empty());
        let mut control = sample;
        control[15] = 2;
        assert!(decoder.feed(&control).is_empty());
    }
}
