//! Bounded hearing-test mode checks; no tones, diagnostics or profile writes.

use crate::{att_settings, driver::Driver, hearing};
use ntpods_ipc::HearingTestModeProbeResult;
use std::{cell::Cell, thread, time::Duration};

const CHARACTERISTIC: [u8; 16] = [
    0x0a, 0x7e, 0x45, 0x41, 0x0b, 0x0f, 0x98, 0x90, 0x6f, 0x4d, 0xc5, 0x95, 0x01, 0x00, 0x12, 0xa4,
];
const POLL_LIMIT: usize = 5;
const POLL_INTERVAL: Duration = Duration::from_millis(100);

fn empty_result() -> HearingTestModeProbeResult {
    HearingTestModeProbeResult {
        initial_state: None,
        enabled_state: None,
        final_state: None,
        enable_sent: false,
        disable_sent: false,
        restoration_verified: false,
        error: None,
    }
}

fn state(response: &[u8]) -> Result<u8, String> {
    match response {
        [0x0b, value @ 0..=4] => Ok(*value),
        [0x01, ..] => Err(hearing::att_error(
            "Could not read hearing-test mode.",
            response,
        )),
        _ => Err("AirPods returned an unsupported hearing-test mode value.".into()),
    }
}

fn restore(
    report: &mut HearingTestModeProbeResult,
    disable: &[u8; 4],
    same_session: impl Fn() -> bool,
    exchange: &mut impl FnMut(&[u8]) -> Result<Vec<u8>, String>,
    wait: &mut impl FnMut(Duration),
) -> Result<(), String> {
    // Cancellation must not skip cleanup; the session check still applies.
    if !same_session() {
        return Err("AirPods connection changed before mode restoration.".into());
    }
    let response = exchange(disable)?;
    report.disable_sent = true;
    if !response.is_empty() {
        return Err("Unexpected reply to the hearing-test mode disable command.".into());
    }
    for attempt in 0..POLL_LIMIT {
        if !same_session() {
            return Err("AirPods connection changed during mode restoration.".into());
        }
        let observed = state(&exchange(&[0x0a, disable[1], disable[2]])?)?;
        report.final_state = Some(observed);
        if observed == 0 {
            report.restoration_verified = true;
            return Ok(());
        }
        if attempt + 1 < POLL_LIMIT {
            wait(POLL_INTERVAL);
        }
    }
    Err("AirPods did not confirm the restored off state before the read limit.".into())
}

fn transaction(
    enable_check: bool,
    cancelled: impl Fn() -> bool,
    same_session: impl Fn() -> bool,
    mut exchange: impl FnMut(&[u8]) -> Result<Vec<u8>, String>,
    mut fallback_disable: impl FnMut(&[u8]) -> Result<(), String>,
    mut wait: impl FnMut(Duration),
) -> HearingTestModeProbeResult {
    let mut report = empty_result();
    let mut possible_enable = false;
    let mut disable = [0x52, 0, 0, 0];
    let exchange_failed = Cell::new(false);
    let mut exchange = |request: &[u8]| {
        let result = exchange(request);
        if result.is_err() {
            exchange_failed.set(true);
        }
        result
    };
    let outcome: Result<(), String> = (|| {
        let check = || {
            if !same_session() {
                Err("AirPods connection changed.".to_string())
            } else if cancelled() {
                Err("Hearing-test mode check cancelled.".to_string())
            } else {
                Ok(())
            }
        };
        check()?;
        let characteristic = att_settings::discover(
            &mut |request| {
                check()?;
                exchange(request)
            },
            &CHARACTERISTIC,
            "Hearing-test mode",
        )?
        .ok_or("Hearing-test mode is not available on these AirPods.")?;
        let handle = characteristic.handle.to_le_bytes();
        let read = [0x0a, handle[0], handle[1]];
        check()?;
        let initial = state(&exchange(&read)?)?;
        report.initial_state = Some(initial);
        report.final_state = Some(initial);
        if !enable_check {
            return Ok(());
        }
        if initial != 0 {
            return Err("Hearing-test mode is not off; the check made no changes.".into());
        }
        if characteristic.write_opcode != Some(0x52) {
            return Err(
                "Hearing-test mode does not expose the expected write-without-response property."
                    .into(),
            );
        }
        check()?;
        disable = [0x52, handle[0], handle[1], 0];
        possible_enable = true;
        report.final_state = None;
        let response = exchange(&[0x52, handle[0], handle[1], 1])?;
        report.enable_sent = true;
        if !response.is_empty() {
            return Err("Unexpected reply to the hearing-test mode command.".into());
        }
        for attempt in 0..POLL_LIMIT {
            check()?;
            let observed = state(&exchange(&read)?)?;
            report.enabled_state = Some(observed);
            if observed == 1 {
                break;
            }
            if attempt + 1 < POLL_LIMIT {
                wait(POLL_INTERVAL);
            }
        }
        if report.enabled_state != Some(1) {
            return Err("AirPods did not confirm hearing-test mode before the read limit.".into());
        }
        Ok(())
    })();
    report.error = outcome.err();
    if possible_enable {
        if !exchange_failed.get() {
            if let Err(cleanup) = restore(
                &mut report,
                &disable,
                &same_session,
                &mut exchange,
                &mut wait,
            ) {
                match &mut report.error {
                    Some(error) => error.push_str(&format!(" {cleanup}")),
                    None => report.error = Some(cleanup),
                }
            }
        }
        if !report.restoration_verified {
            let error = report
                .error
                .get_or_insert_with(|| "Hearing-test mode check failed.".into());
            // A failed read may still be pending. Never drain or reuse its reply queue.
            if same_session() {
                match fallback_disable(&disable) {
                    Ok(()) => report.disable_sent = true,
                    Err(cleanup) => error.push_str(&format!(" Disable request failed: {cleanup}")),
                }
            } else {
                error.push_str(" No disable request was sent to the changed connection.");
            }
            error.push_str(" Mode restoration is unverified.");
        }
    }
    if report.error.is_none() && cancelled() {
        report.error = Some("Hearing-test mode check cancelled; off state restored.".into());
    }
    report
}

pub fn probe(
    drv: &Driver,
    enable_check: bool,
    cancelled: impl Fn() -> bool,
) -> HearingTestModeProbeResult {
    let session = hearing::session();
    let same_session = || hearing::session() == session;
    let mut report = empty_result();
    let result = hearing::att_transaction(drv, &same_session, |exchange| {
        report = transaction(
            enable_check,
            cancelled,
            &same_session,
            exchange,
            |request| {
                if !same_session() {
                    return Err("AirPods connection changed.".into());
                }
                drv.att_send(request).map_err(|error| error.to_string())
            },
            thread::sleep,
        );
        Ok(())
    });
    if !matches!(result, Ok(Some(()))) && report.error.is_none() {
        report.error = Some(match result {
            Err(error) => error,
            _ => "AirPods connection changed; the mode check did not complete.".into(),
        });
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, collections::VecDeque};

    fn discovery(properties: u8, uuid: [u8; 16]) -> VecDeque<(Vec<u8>, Result<Vec<u8>, String>)> {
        let mut service = vec![0x06, 1, 0, 0xff, 0xff, 0, 0x28];
        service.extend_from_slice(&att_settings::SERVICE);
        let mut characteristic = vec![0x09, 21, 0x20, 0, properties, 0x21, 0];
        characteristic.extend_from_slice(&uuid);
        VecDeque::from([
            (service, Ok(vec![0x07, 0x12, 0, 0x2b, 0])),
            (vec![0x08, 0x12, 0, 0x2b, 0, 3, 0x28], Ok(characteristic)),
            (
                vec![0x08, 0x21, 0, 0x2b, 0, 3, 0x28],
                Ok(vec![1, 8, 0x21, 0, 0x0a]),
            ),
        ])
    }

    fn run(
        enable: bool,
        mut replies: VecDeque<(Vec<u8>, Result<Vec<u8>, String>)>,
    ) -> (HearingTestModeProbeResult, Vec<Vec<u8>>) {
        let mut cleanup = Vec::new();
        let result = transaction(
            enable,
            || false,
            || true,
            |request| {
                let (expected, reply) = replies.pop_front().expect("unexpected ATT operation");
                assert_eq!(request, expected);
                reply
            },
            |request| {
                cleanup.push(request.to_vec());
                Ok(())
            },
            |_| {},
        );
        assert!(replies.is_empty());
        (result, cleanup)
    }

    fn read(replies: &mut VecDeque<(Vec<u8>, Result<Vec<u8>, String>)>, value: u8) {
        replies.push_back((vec![0x0a, 0x21, 0], Ok(vec![0x0b, value])));
    }

    #[test]
    fn default_probe_reads_only_and_preserves_an_existing_active_mode() {
        let mut replies = discovery(0x16, CHARACTERISTIC);
        read(&mut replies, 1);
        let (result, cleanup) = run(false, replies);
        assert_eq!(
            (result.initial_state, result.final_state),
            (Some(1), Some(1))
        );
        assert!(!result.enable_sent && !result.disable_sent && result.error.is_none());
        assert!(cleanup.is_empty());
    }

    #[test]
    fn explicit_probe_checks_enable_and_restores_with_fresh_off_readback() {
        let mut replies = discovery(0x16, CHARACTERISTIC);
        read(&mut replies, 0);
        replies.push_back((vec![0x52, 0x21, 0, 1], Ok(vec![])));
        read(&mut replies, 0);
        read(&mut replies, 1);
        replies.push_back((vec![0x52, 0x21, 0, 0], Ok(vec![])));
        read(&mut replies, 1);
        read(&mut replies, 0);
        let (result, cleanup) = run(true, replies);
        assert_eq!(
            (
                result.initial_state,
                result.enabled_state,
                result.final_state
            ),
            (Some(0), Some(1), Some(0))
        );
        assert!(result.enable_sent && result.disable_sent && result.restoration_verified);
        assert!(result.error.is_none() && cleanup.is_empty());
    }

    #[test]
    fn refuses_active_unknown_and_malformed_initial_values_without_writes() {
        for value in [vec![0x0b, 1], vec![0x0b, 5], vec![0x0b], vec![0x0b, 0, 0]] {
            let mut replies = discovery(0x16, CHARACTERISTIC);
            replies.push_back((vec![0x0a, 0x21, 0], Ok(value)));
            let (result, cleanup) = run(true, replies);
            assert!(!result.enable_sent && !result.disable_sent);
            assert!(result.error.is_some() && cleanup.is_empty());
        }
    }

    #[test]
    fn changed_uuid_or_properties_never_reach_a_mode_write() {
        let mut other = CHARACTERISTIC;
        other[12] = 3;
        let (result, cleanup) = run(true, discovery(0x16, other));
        assert!(result.error.is_some() && cleanup.is_empty());
        for properties in [0x02, 0x0a] {
            let mut replies = discovery(properties, CHARACTERISTIC);
            read(&mut replies, 0);
            let (result, cleanup) = run(true, replies);
            assert!(result.error.is_some() && !result.enable_sent && cleanup.is_empty());
        }
        let mut replies = discovery(0x04, CHARACTERISTIC);
        replies.pop_back();
        let (result, cleanup) = run(true, replies);
        assert!(result.error.is_some() && cleanup.is_empty());
    }

    #[test]
    fn an_absent_service_stays_read_only() {
        let mut replies = discovery(0x16, CHARACTERISTIC);
        let (request, _) = replies.pop_front().unwrap();
        let (result, cleanup) = run(
            true,
            VecDeque::from([(request, Ok(vec![1, 6, 1, 0, 0x0a]))]),
        );
        assert!(result.error.is_some() && cleanup.is_empty());
        assert!(!result.enable_sent && !result.disable_sent);
    }

    #[test]
    fn only_exchange_failure_prevents_fresh_restoration_readback() {
        for reply in [
            Err("ATT timeout".into()),
            Ok(vec![0x0b, 5]),
            Ok(vec![0x0b, 0, 0]),
        ] {
            let mut replies = discovery(0x16, CHARACTERISTIC);
            read(&mut replies, 0);
            replies.push_back((vec![0x52, 0x21, 0, 1], Ok(vec![])));
            let exchange_failed = reply.is_err();
            replies.push_back((vec![0x0a, 0x21, 0], reply));
            if !exchange_failed {
                replies.push_back((vec![0x52, 0x21, 0, 0], Ok(vec![])));
                read(&mut replies, 0);
            }
            let (result, cleanup) = run(true, replies);
            assert!(result.enable_sent && result.disable_sent && result.error.is_some());
            assert_eq!(result.restoration_verified, !exchange_failed);
            if exchange_failed {
                assert!(result.error.unwrap().contains("restoration is unverified"));
                assert_eq!(cleanup, [vec![0x52, 0x21, 0, 0]]);
            } else {
                assert!(cleanup.is_empty());
            }
        }
    }

    #[test]
    fn intermediate_states_only_pass_after_expected_readbacks() {
        let mut replies = discovery(0x16, CHARACTERISTIC);
        read(&mut replies, 0);
        replies.push_back((vec![0x52, 0x21, 0, 1], Ok(vec![])));
        for value in [3, 2, 1] {
            read(&mut replies, value);
        }
        replies.push_back((vec![0x52, 0x21, 0, 0], Ok(vec![])));
        for value in [3, 1, 0] {
            read(&mut replies, value);
        }
        let (result, cleanup) = run(true, replies);
        assert_eq!(result.enabled_state, Some(1));
        assert_eq!(result.final_state, Some(0));
        assert!(result.restoration_verified && result.error.is_none() && cleanup.is_empty());
    }

    #[test]
    fn a_failed_enable_send_still_attempts_disable() {
        let mut replies = discovery(0x16, CHARACTERISTIC);
        read(&mut replies, 0);
        replies.push_back((
            vec![0x52, 0x21, 0, 1],
            Err("send completion unknown".into()),
        ));
        let (result, cleanup) = run(true, replies);
        assert!(!result.enable_sent && result.disable_sent && !result.restoration_verified);
        assert!(result.error.is_some());
        assert_eq!(cleanup, [vec![0x52, 0x21, 0, 0]]);
    }

    #[test]
    fn cancellation_after_enable_cannot_skip_same_session_disable() {
        let cancelled = Cell::new(false);
        let mut replies = discovery(0x16, CHARACTERISTIC);
        read(&mut replies, 0);
        replies.push_back((vec![0x52, 0x21, 0, 1], Ok(vec![])));
        replies.push_back((vec![0x52, 0x21, 0, 0], Ok(vec![])));
        read(&mut replies, 0);
        let mut cleanup = Vec::new();
        let result = transaction(
            true,
            || cancelled.get(),
            || true,
            |request| {
                let (expected, reply) = replies.pop_front().expect("operation after cancellation");
                assert_eq!(request, expected);
                if request[0] == 0x52 {
                    cancelled.set(true);
                }
                reply
            },
            |request| {
                cleanup.push(request.to_vec());
                Ok(())
            },
            |_| {},
        );
        assert!(replies.is_empty() && result.error.unwrap().contains("cancelled"));
        assert!(result.disable_sent && result.restoration_verified);
        assert!(cleanup.is_empty());
    }

    #[test]
    fn cancellation_before_discovery_sends_nothing() {
        let result = transaction(
            true,
            || true,
            || true,
            |_| panic!("cancelled probe reached ATT"),
            |_| panic!("cancelled probe reached cleanup"),
            |_| {},
        );
        assert!(result.error.unwrap().contains("cancelled"));
        assert!(!result.enable_sent && !result.disable_sent);
    }

    #[test]
    fn cancellation_after_enabled_readback_still_verifies_normal_cleanup() {
        let cancelled = Cell::new(false);
        let mut replies = discovery(0x16, CHARACTERISTIC);
        read(&mut replies, 0);
        replies.push_back((vec![0x52, 0x21, 0, 1], Ok(vec![])));
        read(&mut replies, 1);
        replies.push_back((vec![0x52, 0x21, 0, 0], Ok(vec![])));
        read(&mut replies, 0);
        let result = transaction(
            true,
            || cancelled.get(),
            || true,
            |request| {
                let (expected, reply) = replies.pop_front().expect("extra cleanup operation");
                assert_eq!(request, expected);
                if reply.as_ref().is_ok_and(|value| value == &[0x0b, 1]) {
                    cancelled.set(true);
                }
                reply
            },
            |_| panic!("verified cleanup used fallback"),
            |_| {},
        );
        assert!(replies.is_empty() && result.restoration_verified);
        assert!(result.enable_sent && result.disable_sent);
        assert!(result.error.unwrap().contains("off state restored"));
    }

    #[test]
    fn connection_change_prevents_cleanup_from_targeting_a_new_session() {
        let current = Cell::new(true);
        let mut replies = discovery(0x16, CHARACTERISTIC);
        read(&mut replies, 0);
        replies.push_back((vec![0x52, 0x21, 0, 1], Ok(vec![])));
        let result = transaction(
            true,
            || false,
            || current.get(),
            |request| {
                let (expected, reply) = replies.pop_front().expect("operation on new session");
                assert_eq!(request, expected);
                if request[0] == 0x52 {
                    current.set(false);
                }
                reply
            },
            |_| panic!("cleanup reached new session"),
            |_| {},
        );
        assert!(replies.is_empty() && result.enable_sent && !result.disable_sent);
        assert!(result.error.unwrap().contains("changed connection"));
    }

    #[test]
    fn cleanup_send_failure_and_cleanup_read_timeout_are_not_restoration_success() {
        for fail_disable in [false, true] {
            let mut replies = discovery(0x16, CHARACTERISTIC);
            read(&mut replies, 0);
            replies.push_back((vec![0x52, 0x21, 0, 1], Ok(vec![])));
            read(&mut replies, 1);
            replies.push_back((
                vec![0x52, 0x21, 0, 0],
                if fail_disable {
                    Err("disable failed".into())
                } else {
                    Ok(vec![])
                },
            ));
            if !fail_disable {
                replies.push_back((
                    vec![0x0a, 0x21, 0],
                    Err("restoration read timed out".into()),
                ));
            }
            let result = transaction(
                true,
                || false,
                || true,
                |request| {
                    let (expected, reply) = replies.pop_front().expect("extra cleanup operation");
                    assert_eq!(request, expected);
                    reply
                },
                |_| Err("fallback failed".into()),
                |_| {},
            );
            assert!(replies.is_empty() && !result.restoration_verified);
            assert!(result.error.unwrap().contains("fallback failed"));
        }
    }

    #[test]
    fn poll_exhaustion_is_bounded_and_sends_no_more_enable_requests() {
        let mut replies = discovery(0x16, CHARACTERISTIC);
        read(&mut replies, 0);
        replies.push_back((vec![0x52, 0x21, 0, 1], Ok(vec![])));
        for _ in 0..POLL_LIMIT {
            read(&mut replies, 0);
        }
        replies.push_back((vec![0x52, 0x21, 0, 0], Ok(vec![])));
        read(&mut replies, 0);
        let (result, cleanup) = run(true, replies);
        assert_eq!(result.enabled_state, Some(0));
        assert!(result.restoration_verified && result.error.is_some());
        assert!(cleanup.is_empty());
    }
}
