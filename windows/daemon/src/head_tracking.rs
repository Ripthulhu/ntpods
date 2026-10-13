//! Live movement follows LibrePods android/rewrite, 34076c85:
//! AppleDevice.startHeadTracking and AACPManager.handleSensorData.
//! Values are signed sensor units, not calibrated angles or spatial-audio poses.

use crate::rtbuddy::{
    fields, sensor_fields, unique, Frames, MotionService, PendingMotion, Value, PREFIX,
};
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
    feature_command(sequence, service, 1, if on { 40_000 } else { 0 })
}

fn feature_command(sequence: u16, service: u8, report: u8, value: u32) -> Vec<u8> {
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
    payload.extend_from_slice(&[0x42, 11, 8, service, 0x10, 2, 0x1a, 5, report]);
    payload.extend_from_slice(&value.to_le_bytes());
    let mut frame = PREFIX.to_vec();
    frame.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    frame.extend_from_slice(&payload);
    frame
}

pub fn control(
    mut sequence: impl FnMut() -> u16,
    service: u8,
    on: bool,
    native: bool,
    refresh: &PendingMotion,
    mut send: impl FnMut(&[u8]) -> bool,
) -> bool {
    // A descriptor can change while the previous write is in flight.
    let mut send_current = |packet: &[u8]| !refresh.is_pending() && send(packet);
    if !native {
        return send_current(&command(sequence(), service, on));
    }
    let fifo = send_current(&feature_command(
        sequence(),
        service,
        4,
        if on { 1 } else { 10 },
    ));
    // A failed FIFO restore must not prevent the zero-interval stop request.
    let interval = send_current(&feature_command(
        sequence(),
        service,
        1,
        if on { 20_000 } else { 0 },
    ));
    fifo && interval
}

fn parse(payload: &[u8], expected_service: u8, native: bool) -> Option<Sample> {
    let top = sensor_fields(payload)?;
    let log = match unique(&top, 2)? {
        Value::Number(log @ (1 | 3 | 5)) => log,
        _ => return None,
    };
    if top.iter().filter(|(id, _)| matches!(id, 3 | 7)).count() != 1 {
        return None;
    }
    let record = match (unique(&top, 3), unique(&top, 7)) {
        (Some(Value::Bytes(packed)), None) if native => {
            // Windows receives log-1 or log-3 packets with a 58-byte report.
            if !matches!((log, packed.len()), (1 | 3, 58 | 60) | (5, 58))
                || packed[..2] != [expected_service, 0]
            {
                return None;
            }
            &packed[2..]
        }
        (None, Some(Value::Bytes(command))) if matches!(log, 1 | 3) => {
            let command = fields(command)?;
            if !matches!(unique(&command, 1)?, Value::Number(n) if n == u64::from(expected_service))
            {
                return None;
            }
            let Value::Bytes(record) = unique(&command, 3)? else {
                return None;
            };
            record
        }
        _ => return None,
    };
    if !matches!(record.len(), 56 | 58) || (record.len() == 56 && (!native || record[0] != 1)) {
        return None;
    }
    // LibrePods' raw channel offsets are relative to the report, never the service prefix.
    Some(Sample {
        horizontal: i16::from_le_bytes([record[28], record[29]]),
        vertical: i16::from_le_bytes([record[30], record[31]]),
    })
}

#[derive(Default)]
struct Decoder {
    frames: Frames,
}

impl Decoder {
    fn feed(&mut self, chunk: &[u8], service: u8, native: bool) -> Option<Sample> {
        let mut latest = None;
        self.frames.feed(chunk, |payload| {
            if let Some(sample) = parse(payload, service, native) {
                latest = Some(sample);
            }
        });
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
    active_native: bool,
    discovered: Option<MotionService>,
    awaiting_descriptor: bool,
    stop_pending: bool,
    stop_attempts: u8,
    stop_retry_at: u64,
    wait_started: Option<u64>,
    last_sample: Option<u64>,
    last_published: Option<u64>,
    decoder: Decoder,
}

impl Tracker {
    /// Keep the user's stream ownership across a sensor-service handoff.
    pub fn refresh_services(&mut self, motion: Option<MotionService>) {
        let had_mapping = self.discovered.is_some() || self.awaiting_descriptor;
        self.discovered = motion;
        self.awaiting_descriptor = had_mapping && motion.is_none();
        self.decoder.frames.reset();
        self.last_sample = None;
        self.last_published = None;
        self.wait_started = None;
        self.stream_error = None;
        if self.awaiting_descriptor || motion.is_some_and(|s| self.active_service != Some(s.id)) {
            // The old address may now belong to another sensor.
            self.active_service = None;
            self.active_native = false;
            self.stop_pending = false;
            self.stop_attempts = 0;
            self.stop_retry_at = 0;
        }
        if self.active_service.is_some() {
            self.stop_pending = true;
            // A failed stop remains a cleanup obligation; announcements cannot extend retries.
            if self.stop_attempts == 0 {
                self.stop_retry_at = 0;
            }
        } else {
            self.error = None;
        }
        self.state = if self.enabled { "starting" } else { "idle" };
    }

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
        self.decoder.frames.reset();
        self.revision
    }

    pub fn release(&mut self, owner: u64) -> bool {
        if self.revision != owner {
            return false;
        }
        self.request(false);
        true
    }

    fn stop(&mut self, now: u64, send: &mut impl FnMut(u8, bool, bool) -> bool) -> bool {
        if let Some(service) = self.active_service {
            if self.stop_attempts >= 2 || now < self.stop_retry_at {
                return false;
            }
            self.stop_attempts += 1;
            if !send(service, false, self.active_native) {
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
        self.decoder.frames.reset();
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
        mut send: impl FnMut(u8, bool, bool) -> bool,
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
        if self.active_service.is_none() && !self.awaiting_descriptor {
            self.state = "starting";
            if let Some(service) = self.discovered.map(|s| s.id).or_else(|| service(firmware)) {
                self.active_service = Some(service);
                self.active_native = self.discovered.is_some();
                if !send(service, true, self.active_native) {
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
        if !self.enabled || self.error.is_some() || self.stop_pending {
            return None;
        }
        let sample = self
            .decoder
            .feed(data, self.active_service?, self.active_native)?;
        self.last_sample = Some(now);
        self.state = "streaming";
        if self.last_published.is_some_and(|t| {
            now.saturating_sub(t) < if self.active_native { 20 } else { PERIOD_MS }
        }) {
            return None;
        }
        self.last_published = Some(now);
        Some(sample)
    }

    pub fn disconnected(&mut self) {
        self.request(false);
        self.active_service = None;
        self.active_native = false;
        self.discovered = None;
        self.awaiting_descriptor = false;
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

    fn native_frame(service: u8, packed: bool, log: u8) -> Vec<u8> {
        native_frame_with_size(service, packed, log, 56)
    }

    fn native_frame_with_size(service: u8, packed: bool, log: u8, size: u8) -> Vec<u8> {
        let mut report = vec![0; size.into()];
        report[0] = 1;
        report[26..28].copy_from_slice(&1234i16.to_le_bytes());
        report[28..30].copy_from_slice(&(-2345i16).to_le_bytes());
        report[30..32].copy_from_slice(&3456i16.to_le_bytes());
        let mut payload = vec![8, 1, 16, log];
        if packed {
            payload.extend_from_slice(&[0x1a, size + 2, service, 0]);
        } else {
            payload.extend_from_slice(&[0x3a, size + 4, 8, service, 0x1a, size]);
        }
        payload.extend_from_slice(&report);
        if log == 5 {
            payload.extend_from_slice(&[0xff, 0xff]);
        }
        let mut out = PREFIX.to_vec();
        out.extend_from_slice(&(payload.len() as u16).to_le_bytes());
        out.extend_from_slice(&payload);
        out
    }

    #[test]
    fn native_packed_and_command_envelopes_use_the_same_report_offsets() {
        let expected = Some(Sample {
            horizontal: -2345,
            vertical: 3456,
        });
        for frame in [
            native_frame(16, true, 5),
            native_frame(16, true, 1),
            native_frame(16, true, 3),
            native_frame_with_size(16, true, 1, 58),
            native_frame_with_size(16, true, 3, 58),
            native_frame(16, false, 1),
            native_frame(16, false, 3),
        ] {
            for split in 0..frame.len() {
                let mut decoder = Decoder::default();
                assert_eq!(decoder.feed(&frame[..split], 16, true), None);
                assert_eq!(decoder.feed(&frame[split..], 16, true), expected);
            }
            assert_eq!(Decoder::default().feed(&frame, 14, true), None);
            assert_eq!(Decoder::default().feed(&frame, 16, false), None);
        }
        let frame = native_frame(16, true, 5);
        let mut bad_trailer = frame.clone();
        *bad_trailer.last_mut().unwrap() = 0;
        assert_eq!(Decoder::default().feed(&bad_trailer, 16, true), None);
        let mut missing_trailer = frame[..frame.len() - 2].to_vec();
        missing_trailer[10] -= 2;
        assert_eq!(Decoder::default().feed(&missing_trailer, 16, true), None);
        let mut wrong_report = native_frame(16, false, 1);
        wrong_report[22] = 2;
        assert_eq!(Decoder::default().feed(&wrong_report, 16, true), None);
    }

    #[test]
    fn windows_packed_motion_rejects_other_sizes_logs_and_service_prefixes() {
        for size in [55, 57, 59, 60] {
            let frame = native_frame_with_size(16, true, 1, size);
            assert_eq!(Decoder::default().feed(&frame, 16, true), None);
        }
        for log in [0, 2, 4, 5, 7] {
            let frame = native_frame_with_size(16, true, log, 58);
            assert_eq!(Decoder::default().feed(&frame, 16, true), None);
        }
        let mut frame = native_frame_with_size(16, true, 1, 58);
        frame[19] = 1; // High byte of the little-endian service ID.
        assert_eq!(Decoder::default().feed(&frame, 16, true), None);
        for log in [1, 3] {
            let mut frame = native_frame_with_size(16, true, log, 58);
            frame.extend_from_slice(&[0xff, 0xff]);
            frame[10] += 2;
            assert_eq!(Decoder::default().feed(&frame, 16, true), None);
        }
    }

    #[test]
    fn captured_native_control_order_restores_fifo_and_always_attempts_interval_stop() {
        let mut sequence = 0;
        let mut writes = Vec::new();
        let refresh = PendingMotion::default();
        for on in [true, false] {
            assert!(control(
                || {
                    sequence += 1;
                    sequence
                },
                16,
                on,
                true,
                &refresh,
                |p| {
                    writes.push(p.to_vec());
                    true
                }
            ));
        }
        assert_eq!(
            writes.iter().map(|p| &p[p.len() - 5..]).collect::<Vec<_>>(),
            [
                &[4, 1, 0, 0, 0][..],
                &[1, 0x20, 0x4e, 0, 0],
                &[4, 10, 0, 0, 0],
                &[1, 0, 0, 0, 0],
            ]
        );
        writes.clear();
        assert!(!control(
            || 5,
            16,
            false,
            true,
            &refresh,
            |p| {
                writes.push(p.to_vec());
                false
            }
        ));
        assert_eq!(writes.len(), 2);
        assert_eq!(&writes[1][writes[1].len() - 5..], &[1, 0, 0, 0, 0]);
    }

    #[test]
    fn handoff_restarts_only_enabled_streams_and_preserves_owner_and_stop_cleanup() {
        let mut tracker = Tracker::default();
        let owner = tracker.request(true);
        tracker.update(0, true, "9A348", |_, _, _| true);
        tracker.refresh_services(Some(MotionService { id: 27 }));
        let mut writes = Vec::new();
        tracker.update(1, true, "9A348", |s, on, native| {
            writes.push((s, on, native));
            true
        });
        assert_eq!(writes, [(27, true, true)]);
        assert!(tracker.receive(&native_frame(27, true, 5), 2).is_some());
        assert!(tracker.receive(&native_frame(27, true, 5), 21).is_none());
        assert!(tracker.receive(&native_frame(27, true, 5), 22).is_some());
        assert!(tracker.release(owner));
        tracker.refresh_services(Some(MotionService { id: 27 }));
        tracker.update(23, true, "9A348", |s, on, native| {
            assert_eq!((s, on, native), (27, false, true));
            false
        });
        tracker.refresh_services(Some(MotionService { id: 27 }));
        tracker.update(24, true, "9A348", |_, _, _| {
            panic!("must keep bounded stop backoff")
        });
        tracker.update(523, true, "9A348", |s, on, native| {
            assert_eq!((s, on, native), (27, false, true));
            true
        });
        assert!(!tracker.enabled);
        assert_eq!(tracker.state, "idle");
    }

    #[test]
    fn refresh_during_native_start_blocks_remaining_writes_and_stale_cleanup() {
        for resume in [false, true] {
            let mut tracker = Tracker::default();
            let refresh = PendingMotion::default();
            tracker.refresh_services(Some(MotionService { id: 16 }));
            tracker.request(true);
            let mut writes = Vec::new();
            tracker.update(0, true, "9A348", |service, on, native| {
                control(
                    || 1,
                    service,
                    on,
                    native,
                    &refresh,
                    |packet| {
                        writes.push((service, on, packet.to_vec()));
                        refresh.publish(Some(MotionService { id: 20 }));
                        true
                    },
                )
            });
            assert_eq!(writes.len(), 1); // FIFO only; no old interval or failure cleanup.
            assert_eq!((writes[0].0, writes[0].1), (16, true));
            assert!(tracker.stop_pending);

            tracker.refresh_services(refresh.take().unwrap());
            if !resume {
                tracker.request(false); // Session-close cleanup after the queued remap.
            }
            tracker.update(1, true, "9A348", |service, on, native| {
                control(
                    || 2,
                    service,
                    on,
                    native,
                    &refresh,
                    |packet| {
                        writes.push((service, on, packet.to_vec()));
                        true
                    },
                )
            });
            assert_eq!(writes.len(), if resume { 3 } else { 1 });
            assert!(writes[1..].iter().all(|w| (w.0, w.1) == (20, true)));
            assert!(!tracker.stop_pending && tracker.error.is_none());
        }
    }

    #[test]
    fn native_remap_retires_the_old_address_but_same_address_refresh_stops_it() {
        let mut tracker = Tracker::default();
        tracker.refresh_services(Some(MotionService { id: 16 }));
        tracker.request(true);
        tracker.update(0, true, "9A348", |_, _, _| true);
        assert!(tracker.receive(&native_frame(16, true, 5), 1).is_some());

        tracker.refresh_services(Some(MotionService { id: 20 }));
        let mut writes = Vec::new();
        tracker.update(2, true, "9A348", |s, on, native| {
            writes.push((s, on, native));
            true
        });
        assert_eq!(writes, [(20, true, true)]);
        assert!(tracker.receive(&native_frame(16, true, 5), 3).is_none());
        assert!(tracker.receive(&native_frame(20, true, 5), 3).is_some());

        writes.clear();
        tracker.refresh_services(Some(MotionService { id: 20 }));
        tracker.update(4, true, "9A348", |s, on, native| {
            writes.push((s, on, native));
            true
        });
        assert_eq!(writes, [(20, false, true), (20, true, true)]);
    }

    #[test]
    fn incomplete_discovery_cannot_stop_or_restart_a_stale_address() {
        let mut tracker = Tracker::default();
        tracker.refresh_services(Some(MotionService { id: 16 }));
        tracker.request(true);
        tracker.update(0, true, "9A348", |_, _, _| true);
        tracker.request(false);
        tracker.update(1, true, "9A348", |_, _, _| false);
        assert!(tracker.stop_pending);

        tracker.refresh_services(None);
        tracker.update(501, true, "9A348", |_, _, _| panic!("stale stop"));
        assert!(!tracker.stop_pending);
        tracker.request(true);
        tracker.refresh_services(None);
        tracker.update(502, true, "9A348", |_, _, _| panic!("guessed start"));
        assert!(tracker.receive(&native_frame(16, true, 5), 503).is_none());
        tracker.refresh_services(Some(MotionService { id: 27 }));
        tracker.update(504, true, "9A348", |s, on, native| {
            assert_eq!((s, on, native), (27, true, true));
            true
        });

        tracker.refresh_services(None);
        tracker.disconnected();
        tracker.request(true);
        tracker.update(505, true, "9A348", |s, on, native| {
            assert_eq!((s, on, native), (16, true, false));
            true
        });
    }

    #[test]
    fn legacy_motion_keeps_firmware_fallback_without_a_known_motion_descriptor() {
        let mut tracker = Tracker::default();
        tracker.refresh_services(None);
        tracker.request(true);
        let mut writes = Vec::new();
        tracker.update(0, true, "7E93", |s, on, native| {
            writes.push((s, on, native));
            true
        });
        assert_eq!(writes, [(14, true, false)]);
        assert!(tracker.receive(&frame(14, 100, -200), 1).is_some());

        writes.clear();
        tracker.refresh_services(None);
        tracker.update(2, true, "7E93", |s, on, native| {
            writes.push((s, on, native));
            true
        });
        assert_eq!(writes, [(14, false, false), (14, true, false)]);
        assert!(tracker.receive(&frame(14, 100, -200), 3).is_some());
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
                assert_eq!(decoder.feed(&frame[..split], service, false), None);
                assert_eq!(
                    decoder.feed(&frame[split..], service, false),
                    Some(expected)
                );
            }
            let mut batch = frame;
            batch.extend_from_slice(&super::tests::frame(service, 123, -456));
            assert_eq!(
                Decoder::default().feed(&batch, service, false),
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
        assert_eq!(Decoder::default().feed(&good, 14, false), None);
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
            assert_eq!(
                Decoder::default().feed(&bad, 16, false),
                None,
                "offset {offset}"
            );
        }
        let mut duplicate = good[12..].to_vec();
        duplicate.extend_from_slice(&[0x10, 1]);
        assert_eq!(parse(&duplicate, 16, false), None);
        let mut overflow = vec![0x80; 10];
        overflow.push(2);
        assert!(fields(&overflow).is_none());
        assert!(fields(&[8, 0].repeat(97)).is_none());
        let mut oversized = PREFIX.to_vec();
        oversized.extend_from_slice(&[0xff, 0xff]);
        oversized.extend_from_slice(&good);
        assert!(Decoder::default().feed(&oversized, 16, false).is_some());
    }

    #[test]
    fn start_timeout_stops_once_and_requires_an_explicit_retry() {
        let mut tracker = Tracker::default();
        let mut writes = Vec::new();
        tracker.request(true);
        tracker.update(0, true, "9A348", |s, on, _| {
            writes.push((s, on));
            true
        });
        tracker.update(4999, true, "9A348", |s, on, _| {
            writes.push((s, on));
            true
        });
        assert_eq!(tracker.state, "starting");
        tracker.update(5000, true, "9A348", |s, on, _| {
            writes.push((s, on));
            true
        });
        for now in [10_000, 20_000] {
            tracker.update(now, true, "9A348", |s, on, _| {
                writes.push((s, on));
                true
            });
        }
        assert_eq!(writes, [(16, true), (16, false)]);
        assert_eq!(tracker.state, "error");
        assert!(tracker.receive(&frame(16, 1, 2), 20_000).is_none());
        tracker.request(true);
        tracker.update(21_000, true, "9A348", |s, on, _| {
            writes.push((s, on));
            true
        });
        assert_eq!(writes.last(), Some(&(16, true)));
    }

    #[test]
    fn movement_is_rate_limited_and_ear_removal_suspends_then_resumes() {
        let mut tracker = Tracker::default();
        tracker.request(true);
        tracker.update(0, true, "9A348", |_, _, _| true);
        let sample = frame(16, 100, -200);
        assert!(tracker.receive(&sample, 1).is_some());
        assert_eq!(tracker.state, "streaming");
        assert!(tracker.receive(&sample, 40).is_none());
        assert!(tracker.receive(&sample, 41).is_some());
        let mut writes = Vec::new();
        tracker.update(42, false, "9A348", |s, on, _| {
            writes.push((s, on));
            true
        });
        assert_eq!(tracker.state, "waiting_for_ear");
        assert!(tracker.enabled);
        assert!(tracker.receive(&sample, 43).is_none());
        tracker.update(50_000, false, "9A348", |s, on, _| {
            writes.push((s, on));
            true
        });
        tracker.update(50_040, true, "9A348", |s, on, _| {
            writes.push((s, on));
            true
        });
        assert_eq!(writes, [(16, false), (16, true)]);
        assert!(tracker.receive(&sample, 50_041).is_some());
        tracker.request(false);
        assert!(tracker.receive(&sample, 50_100).is_none());
        tracker.update(50_100, true, "9A348", |s, on, _| {
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
        tracker.update(0, true, "9A348", |_, _, _| true);
        tracker.disconnected();
        tracker.update(1000, true, "9A348", |_, _, _| panic!("must not restart"));
        assert!(!tracker.enabled);
        assert_eq!(tracker.state, "idle");
    }

    #[test]
    fn failed_start_and_missing_firmware_never_retry_unboundedly() {
        let mut tracker = Tracker::default();
        tracker.request(true);
        let mut writes = 0;
        tracker.update(0, true, "9A348", |_, _, _| {
            writes += 1;
            false
        });
        tracker.update(10_000, true, "9A348", |_, _, _| {
            writes += 1;
            false
        });
        assert_eq!(writes, 3); // One start and at most two stop attempts.
        assert_eq!(tracker.state, "error");
        tracker.disconnected();
        tracker.request(true);
        tracker.update(0, true, "", |_, _, _| panic!("unknown service"));
        tracker.update(5000, true, "", |_, _, _| panic!("unknown service"));
        assert_eq!(tracker.state, "error");
    }

    #[test]
    fn failed_stop_keeps_cleanup_obligation_and_retries_only_once_without_a_new_request() {
        let mut tracker = Tracker::default();
        tracker.request(true);
        tracker.update(0, true, "9A348", |_, _, _| true);
        tracker.request(false);
        let mut writes = 0;
        for now in [1, 100, 500, 501, 1000, 10_000] {
            tracker.update(now, true, "9A348", |service, on, _| {
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
        tracker.update(10_001, true, "9A348", |service, on, _| {
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
        tracker.update(0, true, "9A348", |_, _, _| true);
        tracker.update(5000, true, "9A348", |_, _, _| false);
        assert!(tracker.stop_pending);
        tracker.update(5500, true, "9A348", |_, on, _| {
            assert!(!on);
            true
        });
        assert!(!tracker.stop_pending);
        assert!(tracker.error.is_some());
        tracker.update(20_000, true, "9A348", |_, _, _| panic!("must not restart"));
    }
}
