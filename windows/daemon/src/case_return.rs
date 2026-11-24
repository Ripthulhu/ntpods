use serde::{Deserialize, Serialize};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use windows_sys::Win32::System::SystemInformation::GetTickCount64;

// shortcut: unobserved phone use cannot revoke this local ticket; add verified host-ownership reports when decoded.
const RETURN_WINDOW_MS: u64 = 24 * 60 * 60 * 1_000;
const OPEN_FRESH_MS: u64 = 5_000;
const RESTART_GRACE_MS: u64 = 5 * 60 * 1_000;
const CLOCK_SKEW_MS: u64 = 2_000;

#[derive(Clone, Copy)]
struct Clock {
    utc_ms: u64,
    uptime_ms: u64,
}

impl Clock {
    fn now() -> Option<Self> {
        Some(Self {
            utc_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()?
                .as_millis()
                .try_into()
                .ok()?,
            uptime_ms: unsafe { GetTickCount64() },
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RestartTicket {
    version: u8,
    address: u64,
    saved_utc_ms: u64,
    saved_uptime_ms: u64,
    expires_utc_ms: u64,
    expires_uptime_ms: u64,
}

impl RestartTicket {
    fn remaining(&self, address: u64, clock: Clock) -> Option<u64> {
        let utc_age = clock.utc_ms.checked_sub(self.saved_utc_ms)?;
        let uptime_age = clock.uptime_ms.checked_sub(self.saved_uptime_ms)?;
        let duration = self.expires_utc_ms.checked_sub(self.saved_utc_ms)?;
        let uptime_duration = self.expires_uptime_ms.checked_sub(self.saved_uptime_ms)?;
        let remaining = self
            .expires_utc_ms
            .checked_sub(clock.utc_ms)?
            .min(self.expires_uptime_ms.checked_sub(clock.uptime_ms)?);
        (self.version == 1
            && self.address == address
            && address != 0
            && address <= 0xffff_ffff_ffff
            && duration > 0
            && duration <= RESTART_GRACE_MS
            && duration == uptime_duration
            && remaining > 0
            && remaining <= RESTART_GRACE_MS
            && utc_age.abs_diff(uptime_age) <= CLOCK_SKEW_MS)
            .then_some(remaining)
    }
}

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
/// New commands and ownership loss revoke it; a brief same-boot restart requires fresh case evidence.
#[derive(Default)]
pub struct CaseReturn {
    state: State,
    storage: Option<PathBuf>,
    restart_deadline_ms: Option<u64>,
    stopped: bool,
}

impl CaseReturn {
    pub fn load(address: u64, request: u64) -> Self {
        let path = std::env::var_os("LOCALAPPDATA")
            .map(|root| PathBuf::from(root).join("NTPods/case-return.json"));
        match (path, Clock::now()) {
            (Some(path), Some(clock)) => Self::load_at(path, address, request, clock),
            (storage, _) => Self {
                storage,
                ..Self::default()
            },
        }
    }

    fn load_at(path: PathBuf, address: u64, request: u64, clock: Clock) -> Self {
        let mut policy = Self {
            storage: Some(path.clone()),
            ..Self::default()
        };
        let revoked = revoked_path(&path).try_exists().unwrap_or(true);
        let ticket = (|| -> Option<RestartTicket> {
            let file = std::fs::File::open(&path).ok()?;
            if file.metadata().ok()?.len() > 1024 {
                return None;
            }
            serde_json::from_reader(file.take(1025)).ok()
        })();
        // Consume before restoring, so a failed revocation cannot reuse the same disk record.
        if !remove_ticket(&path) {
            return policy;
        }
        if revoked || request & 3 != 0 {
            return policy;
        }
        if let Some(remaining) = ticket.and_then(|ticket| ticket.remaining(address, clock)) {
            policy.restart_deadline_ms = Some(remaining);
            policy.state = State::Waiting {
                address,
                request,
                remembered_at: 0,
                in_case_at: None,
                lost_at: 0,
            };
        }
        policy
    }

    fn persist_armed(&self, now_ms: u64) {
        if let Some(clock) = Clock::now() {
            self.persist_armed_at(now_ms, clock);
        }
    }

    fn persist_armed_at(&self, now_ms: u64, clock: Clock) {
        let Some(path) = &self.storage else {
            return;
        };
        let (address, remembered_at) = match self.state {
            State::Owned {
                address,
                in_case_at: Some(at),
                ..
            } => (address, at),
            State::Waiting {
                address,
                remembered_at,
                in_case_at: Some(_),
                ..
            } => (address, remembered_at),
            _ => return,
        };
        let Some(age) = now_ms.checked_sub(remembered_at) else {
            return;
        };
        let Some(remaining) = RETURN_WINDOW_MS.checked_sub(age) else {
            return;
        };
        let remaining = remaining.min(RESTART_GRACE_MS).min(
            self.restart_deadline_ms
                .map_or(u64::MAX, |deadline| deadline.saturating_sub(now_ms)),
        );
        if remaining == 0 {
            return;
        }
        let Some(expires_utc_ms) = clock.utc_ms.checked_add(remaining) else {
            return;
        };
        let Some(expires_uptime_ms) = clock.uptime_ms.checked_add(remaining) else {
            return;
        };
        let ticket = RestartTicket {
            version: 1,
            address,
            saved_utc_ms: clock.utc_ms,
            saved_uptime_ms: clock.uptime_ms,
            expires_utc_ms,
            expires_uptime_ms,
        };
        let result = serde_json::to_vec(&ticket)
            .map_err(io::Error::other)
            .and_then(|bytes| crate::atomic_file::write(path, &bytes));
        match result {
            Ok(()) => {
                remove_ticket(&revoked_path(path));
            }
            Err(error) => crate::log(&format!(
                "case return: restart ticket could not be saved: {error}"
            )),
        }
    }

    fn revoke_ticket(&self) {
        if let Some(path) = &self.storage {
            // Revocation must survive a sharing lock on the previous ticket.
            if let Err(error) = crate::atomic_file::write(&revoked_path(path), b"revoked") {
                crate::log(&format!(
                    "case return: restart revocation could not be saved: {error}"
                ));
            }
            remove_ticket(path);
        }
    }

    pub fn shutdown(&mut self, now_ms: u64) {
        self.persist_armed(now_ms);
        self.state = State::Idle;
        self.restart_deadline_ms = None;
        self.stopped = true;
    }

    /// Record only successful explicitly owned native playback, never nearby presence.
    pub fn record_owned_playback(&mut self, address: u64, request: u64, now_ms: u64) {
        if self.stopped {
            return;
        }
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
        self.cancel();
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
        if self.stopped {
            return;
        }
        let mut closed = false;
        let mut opened = false;
        match &mut self.state {
            State::Owned {
                address: owner,
                request: generation,
                ready_at,
                in_case_at,
            } if *owner == address && *generation == request && now_ms >= *ready_at => {
                match both_in_case {
                    Some(true) => {
                        closed = in_case_at.is_none();
                        in_case_at.get_or_insert(now_ms);
                    }
                    Some(false) => {
                        opened = in_case_at.take().is_some();
                    }
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
                    closed = in_case_at.is_none();
                    in_case_at.get_or_insert(now_ms);
                }
            }
            _ => {}
        }
        if closed {
            self.persist_armed(now_ms);
        }
        if opened {
            self.revoke_ticket();
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
        if self.stopped {
            return false;
        }
        if let State::Waiting {
            address: owner,
            request,
            remembered_at,
            lost_at,
            ..
        } = &mut self.state
        {
            if *owner == address
                && *request == completed_request
                && completed_request & 3 == 1
                && idle_request == (completed_request.wrapping_add(4) & !3)
                && now_ms >= *lost_at
                && within(*remembered_at, now_ms, RETURN_WINDOW_MS)
            {
                *request = idle_request;
                *lost_at = now_ms;
                return true;
            }
            return false;
        }
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
        if self.stopped {
            return false;
        }
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
        if changed {
            self.persist_armed(now_ms);
        }
        changed
    }

    /// Preserve the return through an exact idle-to-controls-only Windows claim.
    pub fn passive_claim(&mut self, address: u64, before: u64, after: u64) -> bool {
        if self.stopped {
            return false;
        }
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
        if self.stopped {
            return false;
        }
        if self
            .restart_deadline_ms
            .is_some_and(|deadline| now_ms >= deadline)
        {
            self.cancel();
            return false;
        }
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
        self.restart_deadline_ms = None;
        self.revoke_ticket();
    }

    /// A delayed ready publication cannot restore a request whose ownership was revoked.
    pub fn revoke(&mut self, address: u64, request: u64) {
        self.cancel();
        self.state = State::Revoked { address, request };
    }
}

fn revoked_path(path: &Path) -> PathBuf {
    path.with_extension("revoked")
}

fn remove_ticket(path: &Path) -> bool {
    match std::fs::remove_file(path) {
        Ok(()) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => true,
        Err(error) => {
            crate::log(&format!(
                "case return: restart ticket could not be revoked: {error}"
            ));
            false
        }
    }
}

fn within(then: u64, now: u64, limit: u64) -> bool {
    now.checked_sub(then).is_some_and(|age| age <= limit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn restart_path() -> PathBuf {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "ntpods-case-return-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }

    fn restart_clock() -> Clock {
        Clock {
            utc_ms: 1_000_000,
            uptime_ms: 500_000,
        }
    }

    fn write_restart(path: &Path, clock: Clock) {
        let ticket = RestartTicket {
            version: 1,
            address: 1,
            saved_utc_ms: clock.utc_ms,
            saved_uptime_ms: clock.uptime_ms,
            expires_utc_ms: clock.utc_ms + RESTART_GRACE_MS,
            expires_uptime_ms: clock.uptime_ms + RESTART_GRACE_MS,
        };
        crate::atomic_file::write(path, &serde_json::to_vec(&ticket).unwrap()).unwrap();
    }

    #[test]
    fn restart_requires_owned_close_and_fresh_post_restart_case_evidence() {
        let path = restart_path();
        let mut policy = CaseReturn {
            storage: Some(path.clone()),
            ..CaseReturn::default()
        };
        policy.record_owned_playback(1, 1, 10);
        policy.observe_case(1, 1, Some(true), 20);
        assert!(!path.exists());
        policy.record_owned_playback(1, 3, 10);
        assert!(!path.exists());
        policy.observe_case(1, 3, Some(true), 20);
        assert!(path.exists());
        policy.persist_armed_at(20, restart_clock());
        let mut resumed = CaseReturn::load_at(
            path.clone(),
            1,
            0,
            Clock {
                utc_ms: 1_001_000,
                uptime_ms: 501_000,
            },
        );
        assert!(!path.exists());
        assert!(!resumed.take_open(1, 0, true, 10, 10));
        assert!(!resumed.observe_closed(2, 0, true, 10, 10));
        assert!(!resumed.observe_closed(1, 0, false, 10, 10));
        assert!(resumed.observe_closed(1, 0, true, 10, 10));
        assert!(!resumed.take_open(1, 0, true, 9, 11));
        assert!(resumed.take_open(1, 0, true, 11, 11));
        assert!(!path.exists());
        assert!(!resumed.take_open(1, 0, true, 12, 12));
        remove_ticket(&revoked_path(&path));
    }

    #[test]
    fn restart_rejects_expiry_clock_changes_reboot_other_pair_and_active_request() {
        let path = restart_path();
        for (address, request, utc_ms, uptime_ms) in [
            (1, 0, 999_999, 500_001),
            (1, 0, 1_000_001, 499_999),
            (1, 0, 1_000_001, 1),
            (1, 0, 1_005_000, 501_000),
            (1, 0, 1_300_000, 800_000),
            (2, 0, 1_001_000, 501_000),
            (1, 3, 1_001_000, 501_000),
        ] {
            write_restart(&path, restart_clock());
            let resumed =
                CaseReturn::load_at(path.clone(), address, request, Clock { utc_ms, uptime_ms });
            assert!(matches!(resumed.state, State::Idle));
            assert!(!path.exists());
        }
        for bytes in [
            b"{".as_slice(),
            br#"{"version":2}"#.as_slice(),
            &[b' '; 1025],
        ] {
            crate::atomic_file::write(&path, bytes).unwrap();
            let resumed = CaseReturn::load_at(path.clone(), 1, 0, restart_clock());
            assert!(matches!(resumed.state, State::Idle));
            assert!(!path.exists());
        }
        write_restart(&path, restart_clock());
        let mut ticket: RestartTicket =
            serde_json::from_reader(std::fs::File::open(&path).unwrap()).unwrap();
        ticket.expires_utc_ms += 1;
        crate::atomic_file::write(&path, &serde_json::to_vec(&ticket).unwrap()).unwrap();
        assert!(matches!(
            CaseReturn::load_at(path, 1, 0, restart_clock()).state,
            State::Idle
        ));
    }

    #[test]
    fn repeated_restart_and_passive_retirement_preserve_the_original_deadline() {
        let path = restart_path();
        write_restart(&path, restart_clock());
        let mut resumed = CaseReturn::load_at(
            path.clone(),
            1,
            0,
            Clock {
                utc_ms: 1_001_000,
                uptime_ms: 501_000,
            },
        );
        assert!(resumed.passive_claim(1, 0, 5));
        assert!(!resumed.link_lost(2, 5, 8, 20));
        assert!(!resumed.link_lost(1, 5, 12, 20));
        assert!(resumed.link_lost(1, 5, 8, 20));
        assert!(resumed.passive_claim(1, 8, 13));
        resumed.observe_case(1, 13, Some(true), 30);
        resumed.persist_armed_at(
            5_000,
            Clock {
                utc_ms: 1_006_000,
                uptime_ms: 506_000,
            },
        );
        let ticket: RestartTicket =
            serde_json::from_reader(std::fs::File::open(&path).unwrap()).unwrap();
        assert_eq!(ticket.expires_utc_ms, 1_300_000);
        let mut again = CaseReturn::load_at(
            path.clone(),
            1,
            0,
            Clock {
                utc_ms: 1_007_000,
                uptime_ms: 507_000,
            },
        );
        again.observe_case(1, 0, Some(true), 10);
        assert!(!again.take_open(1, 0, true, 293_000, 293_000));
        assert!(!path.exists());
        remove_ticket(&revoked_path(&path));
    }

    #[test]
    fn cancellation_revocation_and_case_removal_delete_the_restart_ticket() {
        for action in 0..4 {
            let path = restart_path();
            let mut policy = CaseReturn {
                storage: Some(path.clone()),
                ..CaseReturn::default()
            };
            policy.record_owned_playback(1, 3, 10);
            policy.observe_case(1, 3, Some(true), 20);
            assert!(path.exists());
            match action {
                0 => policy.cancel(),
                1 => {
                    policy.revoke(1, 3);
                    policy.record_owned_playback(1, 3, 30);
                }
                2 => policy.observe_case(1, 3, Some(false), 30),
                _ => policy.record_owned_playback(2, 7, 30),
            }
            assert!(!path.exists());
            remove_ticket(&revoked_path(&path));
        }
    }

    #[test]
    fn small_clock_skews_cannot_extend_the_original_uptime_expiry() {
        let path = restart_path();
        write_restart(&path, restart_clock());
        for restart in 1..=3 {
            let clock = Clock {
                utc_ms: 1_000_000 + restart * 100,
                uptime_ms: 500_000 + restart * 1_000,
            };
            let mut policy = CaseReturn::load_at(path.clone(), 1, 0, clock);
            policy.observe_case(1, 0, Some(true), 0);
            policy.persist_armed_at(0, clock);
            let ticket: RestartTicket =
                serde_json::from_reader(std::fs::File::open(&path).unwrap()).unwrap();
            assert_eq!(ticket.expires_uptime_ms, 800_000);
        }
        remove_ticket(&path);
        remove_ticket(&revoked_path(&path));
    }

    #[test]
    fn locked_ticket_revocation_cannot_restore_on_the_next_start() {
        use std::os::windows::fs::OpenOptionsExt;
        let path = restart_path();
        write_restart(&path, restart_clock());
        let locked = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(&path)
            .unwrap();
        let mut policy = CaseReturn {
            storage: Some(path.clone()),
            ..CaseReturn::default()
        };
        policy.cancel();
        assert!(path.exists());
        assert!(revoked_path(&path).exists());
        drop(locked);
        let resumed = CaseReturn::load_at(
            path.clone(),
            1,
            0,
            Clock {
                utc_ms: 1_001_000,
                uptime_ms: 501_000,
            },
        );
        assert!(matches!(resumed.state, State::Idle));
        assert!(!path.exists());
        remove_ticket(&revoked_path(&path));
    }

    #[test]
    fn shutdown_preserves_only_the_armed_ticket_and_cannot_rearm_after_late_ready() {
        let path = restart_path();
        let mut policy = CaseReturn {
            storage: Some(path.clone()),
            ..CaseReturn::default()
        };
        policy.record_owned_playback(1, 3, 10);
        policy.observe_case(1, 3, Some(true), 20);
        policy.shutdown(30);
        let saved = std::fs::read(&path).unwrap();
        policy.record_owned_playback(1, 3, 31);
        policy.observe_case(1, 3, Some(true), 32);
        assert!(!policy.link_lost(1, 3, 4, 33));
        assert!(!policy.observe_closed(1, 0, true, 34, 34));
        assert_eq!(std::fs::read(&path).unwrap(), saved);
        policy.cancel();
        assert!(!path.exists());
        remove_ticket(&revoked_path(&path));
    }

    #[test]
    fn late_close_cannot_extend_the_original_owner_window_on_disk() {
        let path = restart_path();
        let mut policy = CaseReturn {
            storage: Some(path.clone()),
            ..CaseReturn::default()
        };
        policy.record_owned_playback(1, 3, 10);
        assert!(policy.link_lost(1, 3, 4, 30));
        let almost_expired = 10 + RETURN_WINDOW_MS - 20;
        policy.observe_case(1, 4, Some(true), almost_expired);
        policy.persist_armed_at(almost_expired, restart_clock());
        let ticket: RestartTicket =
            serde_json::from_reader(std::fs::File::open(&path).unwrap()).unwrap();
        assert_eq!(ticket.expires_utc_ms, restart_clock().utc_ms + 20);
        policy.cancel();
        remove_ticket(&revoked_path(&path));
    }

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
