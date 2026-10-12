//! Live movement follows LibrePods android/rewrite, 34076c85:
//! AppleDevice.startHeadTracking and AACPManager.handleSensorData.
//! Values are signed sensor units, not calibrated angles or spatial-audio poses.

const PREFIX: [u8; 10] = [4, 0, 4, 0, 0x17, 0, 0, 0, 0x10, 0];
const MAX_PAYLOAD: usize = 16 * 1024;
const PERIOD_MS: u64 = 40;
const TIMEOUT_MS: u64 = 5_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    pub horizontal: i16,
    pub vertical: i16,
}

fn service(firmware: &str) -> Option<u8> {
    let major = crate::device_models::firmware_major(firmware);
    (major > 0).then_some(if major >= 8 { 16 } else { 14 })
}

/// SensorDataWX.service_settings: setting 2, configuration mode 1 + interval µs.
pub fn command(sequence: u16, service: u8, on: bool) -> Vec<u8> {
    let mut payload = vec![8];
    let mut seq = u32::from(sequence);
    loop {
        let byte = (seq & 0x7f) as u8;
        seq >>= 7;
        payload.push(byte | if seq != 0 { 0x80 } else { 0 });
        if seq == 0 {
            break;
        }
    }
    payload.extend_from_slice(&[0x42, 11, 8, service, 0x10, 2, 0x1a, 5, 1]);
    payload.extend_from_slice(&(if on { 40_000u32 } else { 0 }).to_le_bytes());
    let mut frame = PREFIX.to_vec();
    frame.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    frame.extend_from_slice(&payload);
    frame
}

#[derive(Clone, Copy)]
enum Value<'a> {
    Number(u64),
    Bytes(&'a [u8]),
    Other,
}

fn varint(data: &[u8], pos: &mut usize) -> Option<u64> {
    let mut result = 0;
    for shift in (0..=63).step_by(7) {
        let byte = *data.get(*pos)?;
        *pos += 1;
        if shift == 63 && byte > 1 {
            return None;
        }
        result |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Some(result);
        }
    }
    None
}

fn fields(data: &[u8]) -> Option<Vec<(u64, Value<'_>)>> {
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

fn unique<'a>(fields: &[(u64, Value<'a>)], id: u64) -> Option<Value<'a>> {
    let mut matches = fields.iter().filter(|(number, _)| *number == id);
    let value = matches.next()?.1;
    matches.next().is_none().then_some(value)
}

fn parse(payload: &[u8], expected_service: u8) -> Option<Sample> {
    let top = fields(payload)?;
    if !matches!(unique(&top, 2)?, Value::Number(1 | 3)) {
        return None;
    }
    let Value::Bytes(command) = unique(&top, 7)? else {
        return None;
    };
    let command = fields(command)?;
    if !matches!(unique(&command, 1)?, Value::Number(n) if n == u64::from(expected_service)) {
        return None;
    }
    let Value::Bytes(record) = unique(&command, 3)? else {
        return None;
    };
    if record.len() != 58 {
        return None;
    }
    Some(Sample {
        horizontal: i16::from_le_bytes([record[28], record[29]]),
        vertical: i16::from_le_bytes([record[30], record[31]]),
    })
}

#[derive(Default)]
struct Decoder {
    carry: Vec<u8>,
}

impl Decoder {
    fn feed(&mut self, chunk: &[u8], service: u8) -> Option<Sample> {
        if chunk.len() > MAX_PAYLOAD || self.carry.len() + chunk.len() > 2 * MAX_PAYLOAD {
            self.carry.clear();
            return None;
        }
        self.carry.extend_from_slice(chunk);
        let mut latest = None;
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
            if let Some(sample) = parse(&self.carry[consumed + 12..end], service) {
                latest = Some(sample);
            }
            consumed = end;
        }
        self.carry.drain(..consumed);
        latest
    }
}

#[derive(Default)]
pub struct Tracker {
    pub enabled: bool,
    pub state: &'static str,
    pub error: Option<&'static str>,
    stream_error: Option<&'static str>,
    revision: u64,
    active_service: Option<u8>,
    stop_pending: bool,
    stop_attempts: u8,
    stop_retry_at: u64,
    wait_started: Option<u64>,
    last_sample: Option<u64>,
    last_published: Option<u64>,
    decoder: Decoder,
}

impl Tracker {
    pub fn request(&mut self, enabled: bool) -> u64 {
        self.revision = self.revision.wrapping_add(1);
        self.enabled = enabled;
        self.stream_error = None;
        if !self.stop_pending {
            self.state = if enabled { "starting" } else { "idle" };
            self.error = None;
        }
        self.stop_attempts = 0;
        self.stop_retry_at = 0;
        self.wait_started = None;
        self.last_sample = None;
        self.last_published = None;
        self.decoder.carry.clear();
        self.revision
    }

    pub fn release(&mut self, owner: u64) -> bool {
        if self.revision != owner {
            return false;
        }
        self.request(false);
        true
    }

    fn stop(&mut self, now: u64, send: &mut impl FnMut(u8, bool) -> bool) -> bool {
        if let Some(service) = self.active_service {
            if self.stop_attempts >= 2 || now < self.stop_retry_at {
                return false;
            }
            self.stop_attempts += 1;
            if !send(service, false) {
                self.stop_pending = true;
                self.stop_retry_at = now.saturating_add(500);
                self.state = "error";
                self.error = Some("Could not stop head tracking. Reconnect the AirPods.");
                return false;
            }
            self.active_service = None;
        }
        if self.stop_pending {
            self.error = self.stream_error;
        }
        self.stop_pending = false;
        self.stop_attempts = 0;
        self.decoder.carry.clear();
        self.last_sample = None;
        self.last_published = None;
        self.wait_started = None;
        true
    }

    pub fn update(
        &mut self,
        now: u64,
        worn: bool,
        firmware: &str,
        mut send: impl FnMut(u8, bool) -> bool,
    ) {
        if self.stop_pending && !self.stop(now, &mut send) {
            return;
        }
        if !self.enabled || !worn {
            self.stop(now, &mut send);
            if self.error.is_none() {
                self.state = if self.enabled {
                    "waiting_for_ear"
                } else {
                    "idle"
                };
            }
            return;
        }
        if self.error.is_some() {
            return;
        }
        let since = *self.wait_started.get_or_insert(now);
        if now.saturating_sub(self.last_sample.unwrap_or(since)) >= TIMEOUT_MS {
            self.stream_error = Some("No head-tracking data. Pause and start to try again.");
            self.stop(now, &mut send);
            self.state = "error";
            if !self.stop_pending {
                self.error = self.stream_error;
            }
            return;
        }
        if self.active_service.is_none() {
            self.state = "starting";
            if let Some(service) = service(firmware) {
                self.active_service = Some(service);
                if !send(service, true) {
                    self.stream_error = Some("Could not start head tracking. Try again.");
                    self.stop(now, &mut send);
                    self.state = "error";
                    if !self.stop_pending {
                        self.error = self.stream_error;
                    }
                }
            }
        }
    }

    pub fn receive(&mut self, data: &[u8], now: u64) -> Option<Sample> {
        if !self.enabled || self.error.is_some() {
            return None;
        }
        let sample = self.decoder.feed(data, self.active_service?)?;
        self.last_sample = Some(now);
        self.state = "streaming";
        if self
            .last_published
            .is_some_and(|t| now.saturating_sub(t) < PERIOD_MS)
        {
            return None;
        }
        self.last_published = Some(now);
        Some(sample)
    }

    pub fn disconnected(&mut self) {
        self.request(false);
        self.active_service = None;
        self.stop_pending = false;
        self.error = None;
        self.state = "idle";
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(service: u8, horizontal: i16, vertical: i16) -> Vec<u8> {
        let mut record = [0; 58];
        record[28..30].copy_from_slice(&horizontal.to_le_bytes());
        record[30..32].copy_from_slice(&vertical.to_le_bytes());
        let mut payload = vec![8, 1, 0x10, 1, 0x3a, 62, 8, service, 0x1a, 58];
        payload.extend_from_slice(&record);
        let mut out = PREFIX.to_vec();
        out.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        out.extend_from_slice(&payload);
        out
    }

    #[test]
    fn commands_match_android_sensor_settings_and_public_firmware_selection() {
        assert_eq!(service("9A348"), Some(16));
        assert_eq!(service("8B34"), Some(16));
        assert_eq!(service("7E93"), Some(14));
        assert_eq!(service(""), None);
        assert_eq!(service("unknown"), None);
        assert_eq!(service("6619472"), None);
        assert_eq!(service("6F21"), Some(14));
        assert_eq!(
            &command(1, 16, true)[10..],
            &[15, 0, 8, 1, 0x42, 11, 8, 16, 0x10, 2, 0x1a, 5, 1, 0x40, 0x9c, 0, 0]
        );
        assert_eq!(
            &command(1, 14, false)[10..],
            &[15, 0, 8, 1, 0x42, 11, 8, 14, 0x10, 2, 0x1a, 5, 1, 0, 0, 0, 0]
        );
        assert_eq!(&command(128, 16, true)[10..15], &[16, 0, 8, 0x80, 1]);
    }

    #[test]
    fn signed_axes_decode_across_every_fragment_boundary_and_batched_frames() {
        for service in [14, 16] {
            let frame = frame(service, -32768, 32767);
            let expected = Sample {
                horizontal: -32768,
                vertical: 32767,
            };
            for split in 0..frame.len() {
                let mut decoder = Decoder::default();
                assert_eq!(decoder.feed(&frame[..split], service), None);
                assert_eq!(decoder.feed(&frame[split..], service), Some(expected));
            }
            let mut batch = frame;
            batch.extend_from_slice(&super::tests::frame(service, 123, -456));
            assert_eq!(
                Decoder::default().feed(&batch, service),
                Some(Sample {
                    horizontal: 123,
                    vertical: -456
                })
            );
        }
    }

    #[test]
    fn only_live_exact_motion_records_are_accepted() {
        let good = frame(16, 123, -456);
        assert_eq!(Decoder::default().feed(&good, 14), None);
        for (offset, byte) in [
            (0, 0),
            (4, 0x58),
            (8, 0),
            (15, 2),
            (16, 0x42),
            (19, 19),
            (21, 57),
        ] {
            let mut bad = good.clone();
            bad[offset] = byte;
            assert_eq!(Decoder::default().feed(&bad, 16), None, "offset {offset}");
        }
        let mut duplicate = good[12..].to_vec();
        duplicate.extend_from_slice(&[0x10, 1]);
        assert_eq!(parse(&duplicate, 16), None);
        let mut overflow = vec![0x80; 10];
        overflow.push(2);
        assert!(fields(&overflow).is_none());
        assert!(fields(&[8, 0].repeat(97)).is_none());
        let mut oversized = PREFIX.to_vec();
        oversized.extend_from_slice(&[0xff, 0xff]);
        oversized.extend_from_slice(&good);
        assert!(Decoder::default().feed(&oversized, 16).is_some());
    }

    #[test]
    fn start_timeout_stops_once_and_requires_an_explicit_retry() {
        let mut tracker = Tracker::default();
        let mut writes = Vec::new();
        tracker.request(true);
        tracker.update(0, true, "9A348", |s, on| {
            writes.push((s, on));
            true
        });
        tracker.update(4999, true, "9A348", |s, on| {
            writes.push((s, on));
            true
        });
        assert_eq!(tracker.state, "starting");
        tracker.update(5000, true, "9A348", |s, on| {
            writes.push((s, on));
            true
        });
        for now in [10_000, 20_000] {
            tracker.update(now, true, "9A348", |s, on| {
                writes.push((s, on));
                true
            });
        }
        assert_eq!(writes, [(16, true), (16, false)]);
        assert_eq!(tracker.state, "error");
        assert!(tracker.receive(&frame(16, 1, 2), 20_000).is_none());
        tracker.request(true);
        tracker.update(21_000, true, "9A348", |s, on| {
            writes.push((s, on));
            true
        });
        assert_eq!(writes.last(), Some(&(16, true)));
    }

    #[test]
    fn movement_is_rate_limited_and_ear_removal_suspends_then_resumes() {
        let mut tracker = Tracker::default();
        tracker.request(true);
        tracker.update(0, true, "9A348", |_, _| true);
        let sample = frame(16, 100, -200);
        assert!(tracker.receive(&sample, 1).is_some());
        assert_eq!(tracker.state, "streaming");
        assert!(tracker.receive(&sample, 40).is_none());
        assert!(tracker.receive(&sample, 41).is_some());
        let mut writes = Vec::new();
        tracker.update(42, false, "9A348", |s, on| {
            writes.push((s, on));
            true
        });
        assert_eq!(tracker.state, "waiting_for_ear");
        assert!(tracker.enabled);
        assert!(tracker.receive(&sample, 43).is_none());
        tracker.update(50_000, false, "9A348", |s, on| {
            writes.push((s, on));
            true
        });
        tracker.update(50_040, true, "9A348", |s, on| {
            writes.push((s, on));
            true
        });
        assert_eq!(writes, [(16, false), (16, true)]);
        assert!(tracker.receive(&sample, 50_041).is_some());
        tracker.request(false);
        assert!(tracker.receive(&sample, 50_100).is_none());
        tracker.update(50_100, true, "9A348", |s, on| {
            writes.push((s, on));
            true
        });
        assert_eq!(writes.last(), Some(&(16, false)));
    }

    #[test]
    fn old_client_disconnect_cannot_stop_new_owner_and_session_end_never_rearms() {
        let mut tracker = Tracker::default();
        let old = tracker.request(true);
        let current = tracker.request(true);
        assert!(!tracker.release(old));
        assert!(tracker.enabled);
        assert!(tracker.release(current));
        assert!(!tracker.enabled);
        tracker.request(true);
        tracker.update(0, true, "9A348", |_, _| true);
        tracker.disconnected();
        tracker.update(1000, true, "9A348", |_, _| panic!("must not restart"));
        assert!(!tracker.enabled);
        assert_eq!(tracker.state, "idle");
    }

    #[test]
    fn failed_start_and_missing_firmware_never_retry_unboundedly() {
        let mut tracker = Tracker::default();
        tracker.request(true);
        let mut writes = 0;
        tracker.update(0, true, "9A348", |_, _| {
            writes += 1;
            false
        });
        tracker.update(10_000, true, "9A348", |_, _| {
            writes += 1;
            false
        });
        assert_eq!(writes, 3); // One start and at most two stop attempts.
        assert_eq!(tracker.state, "error");
        tracker.disconnected();
        tracker.request(true);
        tracker.update(0, true, "", |_, _| panic!("unknown service"));
        tracker.update(5000, true, "", |_, _| panic!("unknown service"));
        assert_eq!(tracker.state, "error");
    }

    #[test]
    fn failed_stop_keeps_cleanup_obligation_and_retries_only_once_without_a_new_request() {
        let mut tracker = Tracker::default();
        tracker.request(true);
        tracker.update(0, true, "9A348", |_, _| true);
        tracker.request(false);
        let mut writes = 0;
        for now in [1, 100, 500, 501, 1000, 10_000] {
            tracker.update(now, true, "9A348", |service, on| {
                assert_eq!((service, on), (16, false));
                writes += 1;
                false
            });
            assert_eq!(tracker.state, "error");
            assert_eq!(tracker.active_service, Some(16));
            assert!(tracker.stop_pending);
        }
        assert_eq!(writes, 2);
        tracker.request(false);
        assert_eq!(tracker.state, "error");
        assert!(tracker.error.is_some());
        tracker.update(10_001, true, "9A348", |service, on| {
            assert_eq!((service, on), (16, false));
            true
        });
        assert_eq!(tracker.active_service, None);
        assert_eq!(tracker.state, "idle");
        assert!(tracker.error.is_none());
    }

    #[test]
    fn successful_stop_retry_does_not_restart_a_timed_out_stream() {
        let mut tracker = Tracker::default();
        tracker.request(true);
        tracker.update(0, true, "9A348", |_, _| true);
        tracker.update(5000, true, "9A348", |_, _| false);
        assert!(tracker.stop_pending);
        tracker.update(5500, true, "9A348", |_, on| {
            assert!(!on);
            true
        });
        assert!(!tracker.stop_pending);
        assert!(tracker.error.is_some());
        tracker.update(20_000, true, "9A348", |_, _| panic!("must not restart"));
    }
}
