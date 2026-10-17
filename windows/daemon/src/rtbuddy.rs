//! Bounded RTBuddy framing and device-supplied sensor descriptions.

pub const PREFIX: [u8; 10] = [4, 0, 4, 0, 0x17, 0, 0, 0, 0x10, 0];
const MAX_PAYLOAD: usize = 16 * 1024;

#[derive(Clone, Copy, Debug)]
pub enum Value<'a> {
    Number(u64),
    Bytes(&'a [u8]),
    Other,
}

fn varint(data: &[u8], pos: &mut usize) -> Option<u64> {
    let mut value = 0;
    for shift in (0..=63).step_by(7) {
        let byte = *data.get(*pos)?;
        *pos += 1;
        if shift == 63 && byte > 1 {
            return None;
        }
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

pub fn fields(data: &[u8]) -> Option<Vec<(u64, Value<'_>)>> {
    if data.len() > MAX_PAYLOAD {
        return None;
    }
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        if out.len() == 96 {
            return None;
        }
        let key = varint(data, &mut pos)?;
        if key >> 3 == 0 || key >> 3 > 4096 {
            return None;
        }
        let value = match key & 7 {
            0 => Value::Number(varint(data, &mut pos)?),
            2 => {
                let length = usize::try_from(varint(data, &mut pos)?).ok()?;
                let end = pos.checked_add(length)?;
                let bytes = data.get(pos..end)?;
                pos = end;
                Value::Bytes(bytes)
            }
            wire @ (1 | 5) => {
                pos = pos.checked_add(if wire == 1 { 8 } else { 4 })?;
                if pos > data.len() {
                    return None;
                }
                Value::Other
            }
            _ => return None,
        };
        out.push((key >> 3, value));
    }
    Some(out)
}

pub fn unique<'a>(fields: &[(u64, Value<'a>)], id: u64) -> Option<Value<'a>> {
    let mut matches = fields.iter().filter(|(number, _)| *number == id);
    let value = matches.next()?.1;
    matches.next().is_none().then_some(value)
}

pub fn sensor_fields(payload: &[u8]) -> Option<Vec<(u64, Value<'_>)>> {
    if let Some(parsed) = fields(payload) {
        return (!matches!(unique(&parsed, 2), Some(Value::Number(5)))).then_some(parsed);
    }
    // The two-byte trailer belongs to the log-5 envelope, not its protobuf.
    let body = payload.strip_suffix(&[0xff, 0xff])?;
    let parsed = fields(body)?;
    matches!(unique(&parsed, 2), Some(Value::Number(5))).then_some(parsed)
}

#[derive(Default)]
pub struct Frames {
    carry: Vec<u8>,
}

impl Frames {
    pub fn reset(&mut self) {
        self.carry.clear();
    }

    pub fn feed(&mut self, chunk: &[u8], mut visit: impl FnMut(&[u8])) {
        if chunk.len() > MAX_PAYLOAD || self.carry.len() + chunk.len() > 2 * MAX_PAYLOAD {
            self.reset();
            return;
        }
        self.carry.extend_from_slice(chunk);
        let mut consumed = 0;
        while self.carry.len() - consumed >= PREFIX.len() {
            if !self.carry[consumed..].starts_with(&PREFIX) {
                consumed += 1;
                continue;
            }
            if self.carry.len() - consumed < 12 {
                break;
            }
            let size = usize::from(u16::from_le_bytes([
                self.carry[consumed + 10],
                self.carry[consumed + 11],
            ]));
            if size > MAX_PAYLOAD {
                consumed += PREFIX.len();
                continue;
            }
            let end = consumed + 12 + size;
            if end > self.carry.len() {
                break;
            }
            visit(&self.carry[consumed + 12..end]);
            consumed = end;
        }
        self.carry.drain(..consumed);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MotionService {
    pub id: u8,
}

#[derive(Default)]
pub struct PendingMotion(std::sync::atomic::AtomicU8);

impl PendingMotion {
    pub fn publish(&self, motion: Option<MotionService>) {
        self.0.store(
            motion.map_or(128, |s| s.id),
            std::sync::atomic::Ordering::Release,
        );
    }

    pub fn take(&self) -> Option<Option<MotionService>> {
        match self.0.swap(0, std::sync::atomic::Ordering::AcqRel) {
            0 => None,
            128 => Some(None),
            id => Some(Some(MotionService { id })),
        }
    }

    pub fn is_pending(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Acquire) != 0
    }
}

// Exact observed layouts gate writes to vendor-defined feature reports.
const MOTION_DESCRIPTOR: &[u8] = &[
    0x06, 0x0c, 0xff, 0x09, 0x11, 0xa1, 0x01, 0x06, 0x00, 0xff, 0x09, 0x23, 0xa1, 0x00, 0x85, 0x01,
    0x06, 0x15, 0xff, 0x09, 0x04, 0x95, 0x01, 0x75, 0x40, 0x81, 0x02, 0x06, 0x0c, 0xff, 0x09, 0x0d,
    0x76, 0xb0, 0x05, 0x81, 0x02, 0x85, 0x02, 0x09, 0x0e, 0x81, 0x02, 0xc0, 0x85, 0x01, 0x05, 0x20,
    0x0a, 0x0e, 0x03, 0x75, 0x20, 0xb1, 0x02, 0x85, 0x02, 0x0a, 0x1b, 0x03, 0xb1, 0x02, 0x06, 0x15,
    0xff, 0x0a, 0x01, 0x00, 0x85, 0x03, 0xb1, 0x02, 0x85, 0x04, 0x05, 0x20, 0x0a, 0x1a, 0x03, 0xb1,
    0x02, 0x85, 0xff, 0x06, 0x15, 0xff, 0x0a, 0x20, 0x00, 0x75, 0x08, 0x95, 0xe0, 0xb1, 0x02, 0xc0,
];
const HR_DESCRIPTOR: &[u8] = &[
    0x05, 0x20, 0x09, 0x16, 0xa1, 0x01, 0x85, 0x01, 0x0a, 0x0e, 0x03, 0x14, 0x27, 0xff, 0xff, 0xff,
    0x7f, 0x75, 0x20, 0x95, 0x01, 0xb1, 0x02, 0x0a, 0xb8, 0x04, 0x26, 0xff, 0x00, 0x75, 0x08, 0x95,
    0x01, 0x81, 0x02, 0x06, 0x15, 0xff, 0x0a, 0x20, 0x01, 0x95, 0x01, 0x75, 0x08, 0x81, 0x02, 0x26,
    0xff, 0x7f, 0x06, 0x15, 0xff, 0x0a, 0x21, 0x01, 0x95, 0x01, 0x75, 0x10, 0x81, 0x02, 0x75, 0x08,
    0x95, 0x01, 0x15, 0x01, 0x25, 0x02, 0x1a, 0x04, 0x01, 0x2a, 0x05, 0x01, 0x81, 0x00, 0x06, 0x00,
    0xff, 0x09, 0x23, 0xa1, 0x00, 0x15, 0x00, 0x24, 0x06, 0x15, 0xff, 0x09, 0x04, 0x95, 0x08, 0x75,
    0x08, 0x81, 0x02, 0x06, 0x0a, 0xff, 0x09, 0x12, 0x95, 0x04, 0x75, 0x08, 0x81, 0x02, 0x85, 0x02,
    0x06, 0x0a, 0xff, 0x09, 0x13, 0x96, 0x58, 0x02, 0x75, 0x08, 0x81, 0x02, 0xc0, 0xc0,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sensor {
    Motion,
    HeartRate,
}

fn description(data: &[u8]) -> Option<Sensor> {
    fn take<'a>(data: &'a [u8], pos: &mut usize, size: usize) -> Option<&'a [u8]> {
        let end = pos.checked_add(size)?;
        let out = data.get(*pos..end)?;
        *pos = end.checked_add(3)? & !3;
        (*pos <= data.len()).then_some(out)
    }
    fn word(data: &[u8], pos: &mut usize) -> Option<u32> {
        Some(u32::from_le_bytes(take(data, pos, 4)?.try_into().ok()?))
    }
    fn item<'a>(data: &'a [u8], pos: &mut usize) -> Option<(u32, &'a [u8], u32)> {
        let header = word(data, pos)?;
        let kind = (header >> 24) & 0x7f;
        let count = header & 0xffffff;
        let size = match kind {
            4 if count <= 64 => 8,
            8..=10 => usize::try_from(count).ok()?,
            11 if count <= 1 => 0,
            _ => return None,
        };
        Some((kind, take(data, pos, size)?, count))
    }
    if data.len() > MAX_PAYLOAD || data.get(..4)? != [0xd3, 0, 0, 0] {
        return None;
    }
    let mut pos = 4;
    let header = word(data, &mut pos)?;
    let count = header & 0xffffff;
    if header & 0x7f000000 != 0x01000000 || count > 96 {
        return None;
    }
    let mut keys = Vec::new();
    let (mut name, mut vendor, mut maximum, mut descriptor) = (None, None, None, None);
    for _ in 0..count {
        let (kind, key, _) = item(data, &mut pos)?;
        if !matches!(kind, 8 | 9) || key.len() > 128 {
            return None;
        }
        let key = key.strip_suffix(&[0]).unwrap_or(key);
        if keys.contains(&key) {
            return None;
        }
        keys.push(key);
        let (kind, value, _) = item(data, &mut pos)?;
        match key {
            b"AccessoryService" | b"HeartRateService" if matches!(kind, 8 | 9) => {
                if name.is_some() {
                    return None;
                }
                name = Some(value.strip_suffix(&[0]).unwrap_or(value));
            }
            b"VendorID" if kind == 4 => vendor = Some(u64::from_le_bytes(value.try_into().ok()?)),
            b"MaxReportSize" if kind == 4 => {
                maximum = Some(u64::from_le_bytes(value.try_into().ok()?))
            }
            b"ReportDescriptor" if kind == 10 => descriptor = Some(value),
            _ => {}
        }
    }
    if pos != data.len() || vendor != Some(1452) {
        return None;
    }
    match (name?, maximum?, descriptor?) {
        (b"devmotion6", 191, MOTION_DESCRIPTOR) => Some(Sensor::Motion),
        (b"HeartRate", 601, HR_DESCRIPTOR) => Some(Sensor::HeartRate),
        _ => None,
    }
}

#[derive(Default)]
pub struct Discovery {
    pub motion: Option<MotionService>,
    pub heart_rate: Option<u8>,
    pub generation: u64,
    frames: Frames,
    last_sequence: Option<u64>,
}

impl Discovery {
    /// Returns true when a known mapping changes or announcements restart after a handoff.
    pub fn feed(&mut self, chunk: &[u8]) -> bool {
        let mut changed = false;
        self.frames.feed(chunk, |payload| {
            let Some(top) = fields(payload) else {
                return;
            };
            if !matches!(unique(&top, 2), Some(Value::Number(1 | 3))) {
                return;
            }
            let mut announcements = Vec::new();
            let mut has_descriptors = false;
            for (id, value) in &top {
                if *id != 5 {
                    continue;
                }
                let Value::Bytes(value) = value else {
                    return;
                };
                let Some(nested) = fields(value) else {
                    return;
                };
                let (Some(Value::Number(service)), Some(Value::Bytes(dictionary))) =
                    (unique(&nested, 1), unique(&nested, 2))
                else {
                    return;
                };
                let Ok(service @ 1..=127) = u8::try_from(service) else {
                    return;
                };
                has_descriptors = true;
                if let Some(sensor) = description(dictionary) {
                    if announcements
                        .iter()
                        .any(|&(id, kind)| (id == service) != (kind == sensor))
                    {
                        return;
                    }
                    announcements.push((service, sensor));
                }
            }
            if !has_descriptors {
                return;
            }
            let Some(Value::Number(sequence)) = unique(&top, 1) else {
                return;
            };
            let restarted = sequence == 1 && self.last_sequence.is_some_and(|old| old > 1);
            let mut motion = if restarted { None } else { self.motion };
            let mut heart_rate = if restarted { None } else { self.heart_rate };
            for (service, sensor) in announcements {
                match sensor {
                    Sensor::Motion => {
                        if heart_rate == Some(service) {
                            return;
                        }
                        motion = Some(MotionService { id: service });
                    }
                    Sensor::HeartRate => {
                        if motion.is_some_and(|s| s.id == service) {
                            return;
                        }
                        heart_rate = Some(service);
                    }
                }
            }
            if restarted {
                self.generation = self.generation.wrapping_add(1);
            }
            changed |= restarted || self.motion != motion || self.heart_rate != heart_rate;
            self.last_sequence = Some(sequence);
            self.motion = motion;
            self.heart_rate = heart_rate;
        });
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(out: &mut Vec<u8>, kind: u32, count: usize, data: &[u8]) {
        out.extend_from_slice(&((kind << 24) | count as u32).to_le_bytes());
        out.extend_from_slice(data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }

    fn dictionary(name: &[u8], descriptor: &[u8], maximum: u64) -> Vec<u8> {
        let mut out = vec![0xd3, 0, 0, 0, 4, 0, 0, 0x81];
        for (key, kind, count, value) in [
            (&b"VendorID\0"[..], 4, 64, &1452u64.to_le_bytes()[..]),
            (&b"AccessoryService\0"[..], 9, name.len(), name),
            (&b"MaxReportSize\0"[..], 4, 64, &maximum.to_le_bytes()[..]),
            (&b"ReportDescriptor\0"[..], 10, descriptor.len(), descriptor),
        ] {
            item(&mut out, 9, key.len(), key);
            item(&mut out, kind, count, value);
        }
        out
    }

    fn bytes(out: &mut Vec<u8>, field: u8, data: &[u8]) {
        out.push(field * 8 + 2);
        let mut size = data.len();
        while size >= 128 {
            out.push(size as u8 | 0x80);
            size >>= 7;
        }
        out.push(size as u8);
        out.extend_from_slice(data);
    }

    fn announcement(sequence: u8, service: u8, dictionary: &[u8]) -> Vec<u8> {
        let mut nested = vec![8, service];
        bytes(&mut nested, 2, dictionary);
        let mut payload = vec![8, sequence, 16, 3];
        bytes(&mut payload, 5, &nested);
        let mut frame = PREFIX.to_vec();
        frame.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        frame.extend_from_slice(&payload);
        frame
    }

    #[test]
    fn captured_layouts_match_only_named_sensors_and_survive_fragmentation() {
        let motion = dictionary(b"devmotion6\0", MOTION_DESCRIPTOR, 191);
        let hr = dictionary(b"HeartRate\0", HR_DESCRIPTOR, 601);
        assert_eq!(description(&motion), Some(Sensor::Motion));
        assert_eq!(description(&hr), Some(Sensor::HeartRate));
        assert!(description(&dictionary(b"SPL0\0", MOTION_DESCRIPTOR, 191)).is_none());
        assert!(description(&dictionary(b"devmotion6\0", HR_DESCRIPTOR, 191)).is_none());
        assert!(description(&dictionary(b"devmotion6\0", MOTION_DESCRIPTOR, 57)).is_none());
        let frame = announcement(1, 27, &motion);
        for split in 0..frame.len() {
            let mut discovery = Discovery::default();
            assert!(!discovery.feed(&frame[..split]));
            assert!(discovery.feed(&frame[split..]));
            assert_eq!(discovery.motion, Some(MotionService { id: 27 }));
        }
    }

    #[test]
    fn descriptor_generation_restarts_once_and_duplicate_frames_do_not_rearm() {
        let motion = announcement(1, 16, &dictionary(b"devmotion6\0", MOTION_DESCRIPTOR, 191));
        let hr = announcement(3, 20, &dictionary(b"HeartRate\0", HR_DESCRIPTOR, 601));
        let mut discovery = Discovery::default();
        assert!(discovery.feed(&motion));
        assert!(!discovery.feed(&motion));
        assert!(discovery.feed(&hr));
        assert!(!discovery.feed(&hr));
        assert_eq!(discovery.generation, 0);
        assert!(discovery.feed(&motion));
        assert_eq!(discovery.generation, 1);
        assert!(!discovery.feed(&motion));
        assert!(discovery.feed(&hr));
        assert_eq!(discovery.heart_rate, Some(20));
    }

    #[test]
    fn malformed_descriptions_do_not_change_known_mapping() {
        let dictionary = dictionary(b"devmotion6\0", MOTION_DESCRIPTOR, 191);
        let mut discovery = Discovery::default();
        assert!(discovery.feed(&announcement(1, 16, &dictionary)));
        for end in 0..dictionary.len() {
            assert!(description(&dictionary[..end]).is_none());
            assert!(!discovery.feed(&announcement(1, 27, &dictionary[..end])));
            assert_eq!(discovery.motion, Some(MotionService { id: 16 }));
        }
        let mut oversized = dictionary.clone();
        oversized[4..8].copy_from_slice(&0x81000061u32.to_le_bytes());
        assert!(description(&oversized).is_none());
        let mut trailing = dictionary;
        trailing.extend_from_slice(&[0; 4]);
        assert!(description(&trailing).is_none());
        assert!(fields(&[0x80; 10]).is_none());
        assert!(fields(&[8, 0].repeat(97)).is_none());
    }

    #[test]
    fn only_captured_log_five_trailer_is_removed() {
        assert!(sensor_fields(&[8, 1, 16, 5, 0xff, 0xff]).is_some());
        assert!(sensor_fields(&[8, 1, 16, 1, 0xff, 0xff]).is_none());
        assert!(sensor_fields(&[8, 1, 16, 5, 0xff]).is_none());
        assert!(sensor_fields(&[8, 1, 16, 5, 0x12, 0xff]).is_none());
    }

    #[test]
    fn mapping_conflicts_are_rejected_across_packets_until_a_generation_reset() {
        let motion = dictionary(b"devmotion6\0", MOTION_DESCRIPTOR, 191);
        let hr = dictionary(b"HeartRate\0", HR_DESCRIPTOR, 601);
        let mut discovery = Discovery::default();
        assert!(discovery.feed(&announcement(1, 16, &motion)));
        assert!(!discovery.feed(&announcement(3, 16, &hr)));
        assert_eq!(discovery.motion, Some(MotionService { id: 16 }));
        assert_eq!(discovery.heart_rate, None);
        assert!(discovery.feed(&announcement(3, 20, &hr)));
        assert!(!discovery.feed(&announcement(4, 20, &motion)));
        assert!(discovery.feed(&announcement(1, 20, &motion)));
        assert_eq!(discovery.motion, Some(MotionService { id: 20 }));
        assert_eq!(discovery.heart_rate, None);
        assert!(discovery.feed(&announcement(3, 16, &hr)));
        assert_eq!(discovery.heart_rate, Some(16));
    }

    #[test]
    fn unsupported_restart_retires_known_services_without_accepting_the_unknown_layout() {
        let motion = dictionary(b"devmotion6\0", MOTION_DESCRIPTOR, 191);
        let hr = dictionary(b"HeartRate\0", HR_DESCRIPTOR, 601);
        let unknown = dictionary(b"SPL0\0", &[0], 57);
        let mut discovery = Discovery::default();
        assert!(discovery.feed(&announcement(3, 16, &motion)));
        assert!(discovery.feed(&announcement(4, 20, &hr)));
        assert!(!discovery.feed(&announcement(5, 18, &unknown)));
        assert_eq!(discovery.motion, Some(MotionService { id: 16 }));
        assert_eq!(discovery.heart_rate, Some(20));
        assert!(!discovery.feed(&announcement(1, 0, &unknown)));
        let mut not_a_descriptor = announcement(1, 18, &unknown);
        not_a_descriptor[16] = 0x32;
        assert!(!discovery.feed(&not_a_descriptor));
        assert_eq!(discovery.motion, Some(MotionService { id: 16 }));
        assert_eq!(discovery.heart_rate, Some(20));

        let restart = announcement(1, 18, &unknown);
        assert!(discovery.feed(&restart));
        assert_eq!(discovery.generation, 1);
        assert_eq!(discovery.motion, None);
        assert_eq!(discovery.heart_rate, None);
        assert!(!discovery.feed(&restart));
        assert_eq!(discovery.generation, 1);

        assert!(discovery.feed(&announcement(2, 27, &motion)));
        assert_eq!(discovery.motion, Some(MotionService { id: 27 }));
        assert_eq!(discovery.heart_rate, None);

        let mut discovery = Discovery::default();
        assert!(discovery.feed(&announcement(1, 16, &motion)));
        assert!(!discovery.feed(&announcement(2, 18, &unknown)));
        assert!(discovery.feed(&restart));
        assert_eq!(discovery.motion, None);
        assert_eq!(discovery.generation, 1);
    }

    #[test]
    fn a_busy_control_worker_does_not_block_or_lose_the_latest_refresh() {
        use std::sync::{mpsc, Arc, Mutex};
        use std::time::Duration;
        let control = Arc::new(Mutex::new(()));
        let held = control.lock().unwrap();
        let pending = Arc::new(PendingMotion::default());
        let (sent, received) = mpsc::channel();
        let queue = pending.clone();
        let producer = std::thread::spawn(move || {
            queue.publish(Some(MotionService { id: 16 }));
            queue.publish(Some(MotionService { id: 27 }));
            sent.send(()).unwrap();
        });
        received
            .recv_timeout(Duration::from_secs(1))
            .expect("receiver waited on sensor control");
        assert!(pending.is_pending());
        drop(held);
        producer.join().unwrap();
        assert_eq!(pending.take(), Some(Some(MotionService { id: 27 })));
        assert_eq!(pending.take(), None);
        pending.publish(None);
        assert_eq!(pending.take(), Some(None));
    }
}
