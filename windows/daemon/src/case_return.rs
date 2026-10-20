// shortcut: unobserved phone use cannot revoke this local ticket; add verified host-ownership reports when decoded.
const RETURN_WINDOW_MS: u64 = 24 * 60 * 60 * 1_000;
const OPEN_FRESH_MS: u64 = 5_000;

#[derive(Default)]
enum State {
    #[default]
    Idle,
    Owned {
        address: u64,
        request: u64,
        ready_at: u64,
        in_case_at: Option<u64>,
    },
    Waiting {
        address: u64,
        request: u64,
        remembered_at: u64,
        in_case_at: Option<u64>,
        lost_at: u64,
    },
    Revoked {
        address: u64,
        request: u64,
    },
}

/// A single return after this PC's verified playback and a verified case closure.
/// Callers revoke on ownership loss and cancel on new commands, pairing/selection changes and shutdown.
#[derive(Default)]
pub struct CaseReturn {
    state: State,
}

impl CaseReturn {
    /// Record only successful explicitly owned native playback, never nearby presence.
    pub fn record_owned_playback(&mut self, address: u64, request: u64, now_ms: u64) {
        if matches!(self.state, State::Revoked { address: a, request: r } if a == address && r == request)
        {
            return;
        }
        if address == 0 || address > 0xffff_ffff_ffff || request & 3 != 3 {
            return;
        }
        if matches!(self.state, State::Owned { address: a, request: r, .. } if a == address && r == request)
        {
            return;
        }
        self.state = State::Owned {
            address,
            request,
            ready_at: now_ms,
            in_case_at: None,
        };
    }

    /// Supply the complete, verified AAP ear report for the same established request.
    pub fn observe_case(
        &mut self,
        address: u64,
        request: u64,
        both_in_case: Option<bool>,
        now_ms: u64,
    ) {
        match &mut self.state {
            State::Owned {
                address: owner,
                request: generation,
                ready_at,
                in_case_at,
            } if *owner == address && *generation == request && now_ms >= *ready_at => {
                match both_in_case {
                    Some(true) => {
                        in_case_at.get_or_insert(now_ms);
                    }
                    Some(false) => *in_case_at = None,
                    None => {}
                }
            }
            State::Waiting {
                address: owner,
                request: generation,
                remembered_at,
                in_case_at,
                lost_at,
            } if *owner == address
                && *generation == request
                && now_ms >= *lost_at
                && within(*remembered_at, now_ms, RETURN_WINDOW_MS) =>
            {
                if both_in_case == Some(true) {
                    in_case_at.get_or_insert(now_ms);
                }
            }
            _ => {}
        }
    }

    /// Remember the owner after an exact release; returning still requires case evidence.
    pub fn link_lost(
        &mut self,
        address: u64,
        completed_request: u64,
        idle_request: u64,
        now_ms: u64,
    ) -> bool {
        if !matches!(self.state, State::Owned { address: a, request: r, .. } if a == address && r == completed_request)
        {
            return false;
        }
        let State::Owned {
            ready_at,
            in_case_at,
            ..
        } = std::mem::take(&mut self.state)
        else {
            return false;
        };
        let remembered_at = in_case_at.unwrap_or(ready_at);
        if idle_request != (completed_request.wrapping_add(4) & !3)
            || !within(remembered_at, now_ms, RETURN_WINDOW_MS)
        {
            return false;
        }
        self.state = State::Waiting {
            address,
            request: idle_request,
            remembered_at,
            in_case_at,
            lost_at: now_ms,
        };
        true
    }

    /// Supply only a fresh IRK-verified report with both buds inside the closed case.
    pub fn observe_closed(
        &mut self,
        address: u64,
        current_request: u64,
        verified: bool,
        received_ms: u64,
        now_ms: u64,
    ) -> bool {
        let State::Waiting {
            address: owner,
            request,
            remembered_at,
            in_case_at,
            lost_at,
        } = &mut self.state
        else {
            return false;
        };
        if !verified
            || address != *owner
            || current_request != *request
            || received_ms < *lost_at
            || !within(received_ms, now_ms, OPEN_FRESH_MS)
            || !within(*remembered_at, now_ms, RETURN_WINDOW_MS)
        {
            return false;
        }
        let changed = in_case_at.is_none();
        in_case_at.get_or_insert(received_ms);
        changed
    }

    /// Preserve the return through an exact idle-to-controls-only Windows claim.
    pub fn passive_claim(&mut self, address: u64, before: u64, after: u64) -> bool {
        let State::Waiting {
            address: owner,
            request,
            ..
        } = &mut self.state
        else {
            return false;
        };
        if *owner != address
            || *request != before
            || before & 3 != 0
            || after != ((before.wrapping_add(4) & !3) | 1)
        {
            return false;
        }
        *request = after;
        true
    }

    /// Consume once for a fresh IRK-verified open report from the still-paired selected pair.
    pub fn take_open(
        &mut self,
        address: u64,
        current_request: u64,
        verified: bool,
        received_ms: u64,
        now_ms: u64,
    ) -> bool {
        let State::Waiting {
            address: owner,
            request,
            remembered_at,
            in_case_at,
            lost_at,
        } = self.state
        else {
            return false;
        };
        // An overnight return is useful, but stale ownership never survives a daemon restart.
        if !within(remembered_at, now_ms, RETURN_WINDOW_MS) {
            self.cancel();
            return false;
        }
        let Some(in_case_at) = in_case_at else {
            return false;
        };
        if !verified
            || address != owner
            || current_request != request
            || received_ms < lost_at
            || received_ms < in_case_at
            || !within(received_ms, now_ms, OPEN_FRESH_MS)
        {
            return false;
        }
        self.cancel();
        true
    }

    pub fn cancel(&mut self) {
        self.state = State::Idle;
    }

    /// A delayed ready publication cannot restore a request whose ownership was revoked.
    pub fn revoke(&mut self, address: u64, request: u64) {
        self.state = State::Revoked { address, request };
    }
}

fn within(then: u64, now: u64, limit: u64) -> bool {
    now.checked_sub(then).is_some_and(|age| age <= limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn waiting() -> CaseReturn {
        let mut policy = CaseReturn::default();
        policy.record_owned_playback(1, 3, 10);
        policy.observe_case(1, 3, Some(true), 20);
        assert!(policy.link_lost(1, 3, 4, 30));
        policy
    }

    #[test]
    fn owned_case_return_is_consumed_once_across_repeated_advertisements() {
        let mut policy = waiting();
        assert!(policy.take_open(1, 4, true, 40, 40));
        assert!(!policy.take_open(1, 4, true, 41, 41));
        assert!(!policy.link_lost(1, 3, 4, 42));
    }

    #[test]
    fn presence_passive_controls_and_generic_link_loss_never_arm_return() {
        let mut policy = CaseReturn::default();
        policy.observe_case(1, 3, Some(true), 20);
        assert!(!policy.link_lost(1, 3, 4, 30));
        for request in [0, 1, 2] {
            policy.record_owned_playback(1, request, 10);
            policy.observe_case(1, request, Some(true), 20);
            assert!(!policy.link_lost(1, request, 4, 30));
        }
        policy.record_owned_playback(1, 3, 10);
        assert!(policy.link_lost(1, 3, 4, 30));
        assert!(!policy.take_open(1, 4, true, 40, 40));
    }

    #[test]
    fn unrelated_identity_and_stale_generation_cannot_arm_or_consume() {
        let mut policy = CaseReturn::default();
        policy.record_owned_playback(1, 3, 10);
        policy.observe_case(2, 3, Some(true), 20);
        policy.observe_case(1, 7, Some(true), 20);
        assert!(policy.link_lost(1, 3, 4, 30));
        assert!(!policy.take_open(1, 4, true, 40, 40));
        let mut policy = waiting();
        for (address, request, verified) in [(2, 4, true), (1, 8, true), (1, 4, false)] {
            assert!(!policy.take_open(address, request, verified, 40, 40));
        }
        assert!(policy.take_open(1, 4, true, 40, 40));
    }

    #[test]
    fn removing_buds_before_loss_cancels_the_case_qualification() {
        let mut policy = CaseReturn::default();
        policy.record_owned_playback(1, 3, 10);
        policy.observe_case(1, 3, Some(true), 20);
        policy.observe_case(1, 3, Some(false), 21);
        assert!(policy.link_lost(1, 3, 4, 30));
        assert!(!policy.take_open(1, 4, true, 40, 40));
    }

    #[test]
    fn fresh_open_must_follow_loss_and_monotonic_time_cannot_go_backwards() {
        let mut policy = waiting();
        for (received, now) in [(29, 40), (40, 39), (40, 40 + OPEN_FRESH_MS + 1)] {
            assert!(!policy.take_open(1, 4, true, received, now));
        }
        assert!(policy.take_open(1, 4, true, 50, 50));
        let mut policy = waiting();
        assert!(!policy.take_open(1, 4, true, 19, 19));
        assert!(!policy.take_open(1, 4, true, 40, 40));
    }

    #[test]
    fn duplicate_case_reports_do_not_extend_the_overnight_window() {
        let mut policy = CaseReturn::default();
        policy.record_owned_playback(1, 3, 10);
        policy.observe_case(1, 3, Some(true), 20);
        policy.record_owned_playback(1, 3, 25);
        policy.observe_case(1, 3, Some(true), 29);
        assert!(policy.link_lost(1, 3, 4, 30));
        let expiry = 20 + RETURN_WINDOW_MS;
        assert!(policy.take_open(1, 4, true, expiry, expiry));
        let mut policy = waiting();
        assert!(!policy.take_open(1, 4, true, expiry + 1, expiry + 1));
        assert!(!policy.take_open(1, 4, true, 40, 40));
    }

    #[test]
    fn supersession_cancellation_and_restart_do_not_reuse_an_old_owner() {
        for mut policy in [waiting(), CaseReturn::default()] {
            policy.cancel();
            assert!(!policy.take_open(1, 4, true, 40, 40));
        }
        let mut policy = waiting();
        policy.record_owned_playback(2, 7, 40);
        assert!(!policy.take_open(1, 4, true, 50, 50));
        policy.observe_case(2, 7, Some(true), 50);
        assert!(!policy.link_lost(2, 7, 16, 60));
        assert!(!policy.take_open(2, 16, true, 70, 70));
    }

    #[test]
    fn stale_loss_cannot_cancel_a_newer_owned_session() {
        let mut policy = CaseReturn::default();
        policy.record_owned_playback(1, 7, 10);
        policy.observe_case(1, 7, Some(true), 20);
        assert!(!policy.link_lost(1, 3, 4, 30));
        assert!(policy.link_lost(1, 7, 8, 30));
        assert!(policy.take_open(1, 8, true, 40, 40));
    }

    #[test]
    fn passive_controls_preserve_return_without_consuming_or_granting_audio() {
        let mut policy = waiting();
        assert!(policy.passive_claim(1, 4, 9));
        assert!(!policy.passive_claim(1, 4, 9));
        assert!(!policy.take_open(1, 4, true, 40, 40));
        assert!(!policy.take_open(1, 9, false, 40, 40));
        assert!(policy.take_open(1, 9, true, 40, 40));
        assert!(!policy.take_open(1, 9, true, 41, 41));
    }

    #[test]
    fn only_the_exact_selected_passive_claim_can_carry_the_waiting_generation() {
        let mut policy = waiting();
        for (address, before, after) in [
            (2, 4, 9),
            (1, 0, 5),
            (1, 8, 13),
            (1, 4, 8),
            (1, 4, 11),
            (1, 4, 13),
            (1, 5, 9),
        ] {
            assert!(!policy.passive_claim(address, before, after));
        }
        assert!(policy.take_open(1, 4, true, 40, 40));
        assert!(!CaseReturn::default().passive_claim(1, 4, 9));
    }

    #[test]
    fn revoked_ownership_cannot_relatch_from_a_delayed_ready_publication() {
        let mut policy = CaseReturn::default();
        policy.record_owned_playback(1, 3, 10);
        policy.observe_case(1, 3, Some(true), 20);
        policy.revoke(1, 3);
        policy.record_owned_playback(1, 1, 21);
        policy.record_owned_playback(1, 3, 22);
        policy.observe_case(1, 3, Some(true), 23);
        assert!(!policy.link_lost(1, 3, 4, 30));
        assert!(!policy.take_open(1, 4, true, 40, 40));

        policy.cancel();
        policy.record_owned_playback(1, 7, 50);
        policy.observe_case(1, 7, Some(true), 60);
        assert!(policy.link_lost(1, 7, 8, 70));
        assert!(policy.take_open(1, 8, true, 80, 80));
    }

    #[test]
    fn unknown_ear_reports_neither_arm_nor_remove_case_qualification() {
        let mut policy = CaseReturn::default();
        policy.record_owned_playback(1, 3, 10);
        policy.observe_case(1, 3, None, 20);
        assert!(policy.link_lost(1, 3, 4, 30));
        assert!(!policy.take_open(1, 4, true, 40, 40));

        policy.record_owned_playback(1, 7, 40);
        policy.observe_case(1, 7, Some(true), 50);
        policy.observe_case(1, 7, None, 60);
        assert!(policy.link_lost(1, 7, 8, 70));
        assert!(policy.take_open(1, 8, true, 80, 80));

        policy.record_owned_playback(1, 11, 90);
        policy.observe_case(1, 11, Some(true), 100);
        policy.observe_case(1, 11, None, 110);
        policy.observe_case(1, 11, Some(false), 120);
        assert!(policy.link_lost(1, 11, 12, 130));
        assert!(!policy.take_open(1, 12, true, 140, 140));
    }

    fn unqualified() -> CaseReturn {
        let mut policy = CaseReturn::default();
        policy.record_owned_playback(1, 3, 10);
        assert!(policy.link_lost(1, 3, 4, 30));
        policy
    }

    #[test]
    fn fresh_verified_closed_case_can_qualify_a_remembered_owner_after_loss() {
        let mut policy = unqualified();
        assert!(!policy.take_open(1, 4, true, 40, 40));
        assert!(policy.observe_closed(1, 4, true, 50, 50));
        assert!(!policy.observe_closed(1, 4, true, 51, 51));
        assert!(!policy.take_open(1, 4, true, 49, 51));
        assert!(policy.take_open(1, 4, true, 60, 60));
        assert!(!policy.take_open(1, 4, true, 61, 61));
    }

    #[test]
    fn aap_case_report_on_passive_controls_can_qualify_the_same_remembered_owner() {
        let mut policy = unqualified();
        assert!(policy.passive_claim(1, 4, 9));
        policy.observe_case(1, 3, Some(true), 40);
        policy.observe_case(1, 9, None, 40);
        assert!(!policy.take_open(1, 9, true, 41, 41));
        policy.observe_case(1, 9, Some(true), 50);
        // In-ear is also a return trigger, so it must preserve the prior closed-case evidence.
        policy.observe_case(1, 9, Some(false), 60);
        assert!(policy.take_open(1, 9, true, 60, 60));
    }

    #[test]
    fn late_case_qualification_rejects_other_peers_generations_and_stale_reports() {
        let mut policy = unqualified();
        for (address, request, verified, received, now) in [
            (2, 4, true, 40, 40),
            (1, 8, true, 40, 40),
            (1, 4, false, 40, 40),
            (1, 4, true, 29, 40),
            (1, 4, true, 40, 39),
            (1, 4, true, 40, 40 + OPEN_FRESH_MS + 1),
        ] {
            assert!(!policy.observe_closed(address, request, verified, received, now));
        }
        policy.observe_case(2, 4, Some(true), 40);
        policy.observe_case(1, 8, Some(true), 40);
        policy.observe_case(1, 4, Some(true), 29);
        assert!(!policy.take_open(1, 4, true, 50, 50));
        policy.revoke(1, 4);
        assert!(!policy.observe_closed(1, 4, true, 60, 60));
        policy.observe_case(1, 4, Some(true), 60);
        assert!(!policy.take_open(1, 4, true, 70, 70));
    }

    #[test]
    fn late_qualification_and_duplicates_cannot_extend_the_remembered_owner_window() {
        let mut policy = unqualified();
        let expiry = 10 + RETURN_WINDOW_MS;
        assert!(policy.observe_closed(1, 4, true, expiry - 1, expiry - 1));
        assert!(!policy.observe_closed(1, 4, true, expiry, expiry));
        policy.observe_case(1, 4, Some(true), expiry);
        assert!(!policy.take_open(1, 4, true, expiry + 1, expiry + 1));
    }
}
