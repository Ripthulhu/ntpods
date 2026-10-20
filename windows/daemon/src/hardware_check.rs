//! One requested in-case check; device reports remain separate from a health diagnosis.

use crate::{att_settings, driver::Driver, hearing};
use ntpods_ipc::{HardwareCheckState, HardwareDiagnostics};

const REQUEST_UUID: [u8; 16] = 0xa4120004_95c5_4d6f_9098_0f0b41457e0au128.to_le_bytes();
const WAIT_MS: u64 = 120_000;

fn transaction(mut exchange: impl FnMut(&[u8]) -> Result<Vec<u8>, String>) -> Result<(), String> {
    let characteristic =
        att_settings::discover_write_request(&mut exchange, &REQUEST_UUID, "Hardware check")?
            .ok_or("These AirPods do not expose the hardware-check request.")?;
    if characteristic.write_opcode != Some(0x12) {
        return Err("The hardware-check request does not support a response.".into());
    }
    let handle = characteristic.handle.to_le_bytes();
    // A lost reply leaves execution uncertain, so this operation never retries.
    let response = exchange(&[0x12, handle[0], handle[1], b'1'])?;
    if response != [0x13] {
        return Err("The AirPods did not acknowledge the hardware-check request.".into());
    }
    Ok(())
}

pub fn trigger(drv: &Driver, current: impl Fn() -> bool) -> Result<Option<()>, String> {
    hearing::att_transaction(drv, current, |exchange| transaction(exchange))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    WaitingCaseClose,
    WaitingResult,
    Completed,
    TimedOut,
    Cancelled,
    Error,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Self::WaitingCaseClose => "waiting_case_close",
            Self::WaitingResult => "waiting_result",
            Self::Completed => "completed",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
            Self::Error => "error",
        }
    }
}

pub struct Coordinator {
    request_id: u64,
    target: String,
    baseline: [u64; 2],
    deadline_ms: u64,
    attempted: bool,
    phase: Phase,
    updated: [bool; 2],
    error: Option<String>,
}

impl Coordinator {
    pub fn start(
        request_id: u64,
        target: String,
        baseline: HardwareDiagnostics,
        now_ms: u64,
    ) -> Result<Self, String> {
        if request_id == 0 || target.is_empty() {
            return Err("The hardware-check request has no device identity.".into());
        }
        let deadline_ms = now_ms
            .checked_add(WAIT_MS)
            .ok_or("The hardware-check deadline is unavailable.")?;
        Ok(Self {
            request_id,
            target,
            baseline: [
                baseline.left.measured_at_unix_seconds,
                baseline.right.measured_at_unix_seconds,
            ],
            deadline_ms,
            attempted: false,
            phase: Phase::WaitingCaseClose,
            updated: [false; 2],
            error: None,
        })
    }

    pub fn matches(&self, request_id: u64, target: &str) -> bool {
        self.request_id == request_id && self.target == target
    }

    pub fn is_active(&self) -> bool {
        matches!(self.phase, Phase::WaitingCaseClose | Phase::WaitingResult)
    }

    pub fn claim_trigger(&mut self, request_id: u64, target: &str, now_ms: u64) -> bool {
        if !self.matches(request_id, target) {
            return false;
        }
        self.tick(now_ms);
        if !self.is_active() || self.attempted {
            return false;
        }
        self.attempted = true;
        true
    }

    pub fn waiting_for_result(&mut self, request_id: u64, target: &str, now_ms: u64) {
        if !self.matches(request_id, target) {
            return;
        }
        self.tick(now_ms);
        if self.is_active() && self.attempted {
            self.phase = Phase::WaitingResult;
        }
    }

    pub fn observe(
        &mut self,
        request_id: u64,
        target: &str,
        record: &HardwareDiagnostics,
        now_ms: u64,
    ) {
        if !self.matches(request_id, target) {
            return;
        }
        self.tick(now_ms);
        if !self.is_active() || !self.attempted {
            return;
        }
        // Firmware clock alignment is unknown; compare each ear's exact timestamp.
        let timestamps = [
            record.left.measured_at_unix_seconds,
            record.right.measured_at_unix_seconds,
        ];
        self.updated = std::array::from_fn(|index| {
            timestamps[index] != 0 && timestamps[index] != self.baseline[index]
        });
        if self.updated == [true; 2] {
            self.phase = Phase::Completed;
        } else if self.updated.contains(&true) {
            self.phase = Phase::WaitingResult;
        }
    }

    pub fn tick(&mut self, now_ms: u64) {
        if self.is_active() && now_ms >= self.deadline_ms {
            self.phase = Phase::TimedOut;
        }
    }

    pub fn cancel(&mut self, request_id: u64, target: &str) {
        if self.matches(request_id, target) && self.is_active() {
            // No firmware cancellation command is established.
            self.phase = Phase::Cancelled;
        }
    }

    pub fn fail(&mut self, request_id: u64, target: &str, message: String, now_ms: u64) {
        if !self.matches(request_id, target) {
            return;
        }
        self.tick(now_ms);
        if self.is_active() {
            self.phase = Phase::Error;
            self.error = Some(message);
        }
    }

    pub fn status(&self) -> HardwareCheckState {
        HardwareCheckState {
            request_id: self.request_id,
            phase: self.phase.name().into(),
            left_updated: self.updated[0],
            right_updated: self.updated[1],
            error: self.error.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ntpods_ipc::HardwareDiagnosticEar;

    fn record(left: u64, right: u64) -> HardwareDiagnostics {
        let ear = |measured_at_unix_seconds| HardwareDiagnosticEar {
            measured_at_unix_seconds,
            first_use_unix_seconds: None,
            general: 0,
            measurement: 0,
            component_failures: Some(0),
            unknown_bits: 0,
        };
        HardwareDiagnostics {
            read_at_unix_seconds: 900,
            left: ear(left),
            right: ear(right),
        }
    }

    fn job() -> Coordinator {
        let mut job = Coordinator::start(1, "pair".into(), record(100, 200), 1_000).unwrap();
        assert!(job.claim_trigger(1, "pair", 1_001));
        job
    }

    #[test]
    fn start_requires_identity_and_one_trigger_attempt() {
        assert!(Coordinator::start(0, "pair".into(), record(0, 0), 0).is_err());
        assert!(Coordinator::start(1, "".into(), record(0, 0), 0).is_err());
        assert!(Coordinator::start(1, "pair".into(), record(0, 0), u64::MAX).is_err());
        let mut job = job();
        assert!(!job.claim_trigger(1, "pair", 2_000));
        job.waiting_for_result(1, "pair", 3_000);
        assert!(!job.claim_trigger(1, "pair", 4_000));
    }

    #[test]
    fn wrong_pair_or_job_cannot_update_cancel_or_fail_current_work() {
        let mut job = job();
        for (id, pair) in [(2, "pair"), (1, "other")] {
            job.observe(id, pair, &record(101, 201), 2_000);
            job.waiting_for_result(id, pair, 2_000);
            job.cancel(id, pair);
            job.fail(id, pair, "wrong job".into(), 2_000);
        }
        assert_eq!(job.status().phase, "waiting_case_close");
        assert!(!job.status().left_updated && !job.status().right_updated);
    }

    #[test]
    fn expected_case_disconnect_and_reconnect_keep_the_job() {
        let mut job = job();
        job.waiting_for_result(1, "pair", 2_000);
        job.tick(30_000);
        assert_eq!(job.status().phase, "waiting_result");
        assert!(!job.claim_trigger(1, "pair", 31_000));
        job.observe(1, "pair", &record(101, 201), 32_000);
        assert_eq!(job.status().phase, "completed");
    }

    #[test]
    fn stale_results_and_host_read_times_do_not_establish_a_new_measurement() {
        let mut job = job();
        let mut stale = record(100, 200);
        stale.read_at_unix_seconds = u64::MAX;
        stale.left.general = 1;
        stale.right.unknown_bits = 0x8000;
        job.observe(1, "pair", &stale, 2_000);
        job.observe(1, "pair", &record(0, 0), 3_000);
        assert!(!job.status().left_updated && !job.status().right_updated);
        assert!(job.is_active());
    }

    #[test]
    fn ears_update_independently_without_requiring_a_later_device_time() {
        let mut job = job();
        job.observe(1, "pair", &record(99, 200), 2_000);
        assert!(job.status().left_updated && !job.status().right_updated);
        assert_eq!(job.status().phase, "waiting_result");
        job.observe(1, "pair", &record(99, 199), 3_000);
        assert_eq!(job.status().phase, "completed");
    }

    #[test]
    fn a_replayed_baseline_cannot_complete_a_partially_updated_check() {
        let mut job = job();
        job.observe(1, "pair", &record(101, 200), 2_000);
        job.observe(1, "pair", &record(100, 201), 3_000);
        assert!(!job.status().left_updated && job.status().right_updated);
        assert!(job.is_active());
    }

    #[test]
    fn timeout_preserves_partial_progress_and_rejects_late_results() {
        let mut job = job();
        job.observe(1, "pair", &record(101, 200), 2_000);
        job.tick(121_000);
        job.observe(1, "pair", &record(101, 201), 121_001);
        assert_eq!(job.status().phase, "timed_out");
        assert!(job.status().left_updated && !job.status().right_updated);
        assert!(!job.claim_trigger(1, "pair", 122_000));
    }

    #[test]
    fn cancellation_and_errors_do_not_claim_to_stop_firmware_or_accept_late_reads() {
        let mut cancelled = job();
        cancelled.cancel(1, "pair");
        cancelled.observe(1, "pair", &record(101, 201), 2_000);
        assert_eq!(cancelled.status().phase, "cancelled");
        let mut failed = job();
        failed.fail(1, "pair", "No reply; execution is uncertain.".into(), 2_000);
        failed.observe(1, "pair", &record(101, 201), 3_000);
        assert_eq!(failed.status().phase, "error");
        assert!(failed.status().error.is_some());
        assert!(!failed.claim_trigger(1, "pair", 3_000));
    }

    fn discovery(properties: u8) -> Vec<(Vec<u8>, Vec<u8>)> {
        let mut service = vec![0x06, 1, 0, 0xff, 0xff, 0, 0x28];
        service.extend_from_slice(&att_settings::SERVICE);
        let mut declaration = vec![0x09, 21, 0x34, 0, properties, 0x35, 0];
        declaration.extend_from_slice(&REQUEST_UUID);
        vec![
            (service, vec![0x07, 0x30, 0, 0x40, 0]),
            (vec![0x08, 0x30, 0, 0x40, 0, 3, 0x28], declaration),
            (
                vec![0x08, 0x35, 0, 0x40, 0, 3, 0x28],
                vec![1, 8, 0x35, 0, 0x0a],
            ),
        ]
    }

    #[test]
    fn trigger_uses_the_discovered_write_request_once_and_no_other_write() {
        let mut expected = discovery(0x08);
        expected.push((vec![0x12, 0x35, 0, 0x31], vec![0x13]));
        let mut expected = expected.into_iter();
        transaction(|request| {
            let (wanted, response) = expected.next().expect("unexpected ATT operation");
            assert_eq!(request, wanted);
            Ok(response)
        })
        .unwrap();
        assert!(expected.next().is_none());
    }

    #[test]
    fn trigger_rejects_unacknowledged_or_ambiguous_writes_without_retry() {
        for reply in [
            Ok(vec![]),
            Ok(vec![1, 0x12, 0x35, 0, 3]),
            Ok(vec![0x13, 0]),
            Err("timeout".into()),
        ] {
            let mut expected = discovery(0x08).into_iter();
            let mut writes = 0;
            assert!(transaction(|request| {
                if let Some((wanted, response)) = expected.next() {
                    assert_eq!(request, wanted);
                    return Ok(response);
                }
                writes += 1;
                assert_eq!(request, [0x12, 0x35, 0, 0x31]);
                reply.clone()
            })
            .is_err());
            assert_eq!(writes, 1);
        }
    }

    #[test]
    fn trigger_requires_response_property_and_an_existing_service() {
        for properties in [0x02, 0x04, 0x06] {
            let mut expected = discovery(properties).into_iter();
            assert!(transaction(|request| {
                let (wanted, response) = expected.next().expect("unexpected operation");
                assert_eq!(request, wanted);
                Ok(response)
            })
            .is_err());
        }
        assert!(transaction(|request| {
            assert_eq!(request[0], 0x06);
            Ok(vec![1, 6, 1, 0, 0x0a])
        })
        .is_err());
    }
}
