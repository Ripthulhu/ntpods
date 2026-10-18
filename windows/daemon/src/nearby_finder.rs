//! A temporary, selected-pair BLE signal view. No advertisement starts audio.

use ntpods_ipc::NearbyFinderState;
use std::collections::VecDeque;

const WINDOW: usize = 7;
const TREND_WINDOW: usize = 5;

#[derive(Default)]
pub struct Finder {
    revision: u64,
    state_revision: u64,
    active: bool,
    paused: bool,
    device_id: String,
    available_identity: bool,
    readings: VecDeque<i16>,
    ema: Option<f32>,
    ema_history: VecDeque<f32>,
    last_heard: Option<u64>,
}

impl Finder {
    pub fn new(device_id: String, available_identity: bool) -> Self {
        Self {
            device_id,
            available_identity,
            ..Self::default()
        }
    }

    pub fn scanning(&self) -> bool {
        self.active && self.available_identity && !self.paused
    }

    pub fn active(&self) -> bool {
        self.active
    }

    pub fn set_paused(&mut self, paused: bool) -> bool {
        if self.paused == paused {
            return false;
        }
        self.paused = paused;
        self.state_revision = self.state_revision.wrapping_add(1);
        true
    }

    pub fn set_identity(&mut self, device_id: &str, available_identity: bool) -> bool {
        if self.device_id == device_id && self.available_identity == available_identity {
            return false;
        }
        if self.device_id != device_id {
            self.clear();
            self.device_id = device_id.into();
        }
        self.available_identity = available_identity;
        if !available_identity {
            self.clear();
        }
        self.state_revision = self.state_revision.wrapping_add(1);
        true
    }

    pub fn start(&mut self, device_id: &str) -> Option<u64> {
        if device_id.is_empty() || self.device_id != device_id || !self.available_identity {
            return None;
        }
        self.clear_readings();
        self.revision = self.revision.wrapping_add(1);
        self.state_revision = self.state_revision.wrapping_add(1);
        self.active = true;
        Some(self.revision)
    }

    pub fn stop(&mut self, device_id: &str, owner: u64) -> bool {
        if !self.active || self.device_id != device_id || self.revision != owner {
            return false;
        }
        self.clear();
        true
    }

    pub fn clear(&mut self) -> bool {
        let changed = self.active || self.paused || !self.readings.is_empty();
        self.revision = self.revision.wrapping_add(1);
        self.active = false;
        self.paused = false;
        self.clear_readings();
        if changed {
            self.state_revision = self.state_revision.wrapping_add(1);
        }
        changed
    }

    fn clear_readings(&mut self) {
        self.readings.clear();
        self.ema = None;
        self.ema_history.clear();
        self.last_heard = None;
    }

    /// Call only after the rotating address resolves with the selected pair's IRK.
    pub fn observe_verified(&mut self, device_id: &str, rssi: i16, now_ms: u64) -> bool {
        // Windows uses -127 for an out-of-range event with the previous advertisement.
        if !self.scanning() || self.device_id != device_id || !(-126..=20).contains(&rssi) {
            return false;
        }
        if self.last_heard.is_some_and(|then| now_ms < then) {
            return false;
        }
        if self
            .last_heard
            .is_some_and(|then| now_ms.saturating_sub(then) > 10_000)
        {
            self.clear_readings();
        }
        self.readings.push_back(rssi);
        if self.readings.len() > WINDOW {
            self.readings.pop_front();
        }
        let mut sorted: Vec<_> = self.readings.iter().copied().collect();
        sorted.sort_unstable();
        let median = f32::from(sorted[sorted.len() / 2]);
        let smoothed = self
            .ema
            .map_or(median, |previous| previous * 0.7 + median * 0.3);
        self.ema = Some(smoothed);
        self.ema_history.push_back(smoothed);
        if self.ema_history.len() > TREND_WINDOW {
            self.ema_history.pop_front();
        }
        self.last_heard = Some(now_ms);
        self.state_revision = self.state_revision.wrapping_add(1);
        true
    }

    pub fn snapshot(&self, now_ms: u64) -> NearbyFinderState {
        let trend = if self.paused
            || self
                .last_heard
                .is_none_or(|last| now_ms.saturating_sub(last) > 10_000)
            || self.ema_history.len() < TREND_WINDOW
        {
            "unknown"
        } else {
            let delta = self.ema.unwrap_or_default() - self.ema_history[0];
            if delta >= 3.0 {
                "stronger"
            } else if delta <= -3.0 {
                "weaker"
            } else {
                "steady"
            }
        };
        NearbyFinderState {
            revision: self.state_revision,
            active: self.active,
            paused: self.paused,
            available_identity: self.available_identity,
            rssi: self
                .last_heard
                .and_then(|_| self.ema.map(|rssi| rssi.round() as i16)),
            trend: trend.into(),
            last_heard_age_ms: self.last_heard.map(|last| now_ms.saturating_sub(last)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_owner_cannot_stop_new_client_or_new_pair() {
        let mut finder = Finder::new("aaa".into(), true);
        let old = finder.start("aaa").unwrap();
        let new = finder.start("aaa").unwrap();
        assert!(!finder.stop("aaa", old));
        assert!(finder.scanning());
        assert!(!finder.stop("bbb", new));
        finder.set_identity("bbb", true);
        assert!(!finder.stop("bbb", new));
        assert!(!finder.scanning());
    }

    #[test]
    fn unavailable_identity_never_activates_scan() {
        let mut finder = Finder::new("aaa".into(), false);
        assert!(finder.start("aaa").is_none());
        assert!(finder.start("bbb").is_none());
        finder.set_identity("aaa", true);
        assert!(finder.start("aaa").is_some());
        assert!(finder.set_paused(true));
        assert!(!finder.scanning());
        assert!(!finder.observe_verified("aaa", -60, 100));
        assert!(finder.set_paused(false));
        assert!(finder.scanning());
        finder.set_identity("aaa", false);
        assert!(!finder.scanning());
    }

    #[test]
    fn median_ema_trend_and_age_use_only_valid_monotonic_samples() {
        let mut finder = Finder::new("aaa".into(), true);
        finder.start("aaa");
        for (i, value) in [-75, -74, -73, -72, -71, -70, -30, -60, -55, -50, -45]
            .into_iter()
            .enumerate()
        {
            assert!(finder.observe_verified("aaa", value, i as u64 * 1_000));
        }
        assert!(!finder.observe_verified("aaa", 21, 11_000));
        assert!(!finder.observe_verified("aaa", -20, 1_000));
        let snap = finder.snapshot(12_000);
        assert_eq!(snap.last_heard_age_ms, Some(2_000));
        assert_eq!(snap.trend, "stronger");
        assert_ne!(snap.rssi, Some(-30));
        assert_eq!(finder.snapshot(25_000).trend, "unknown");
        assert!(finder.observe_verified("aaa", -40, 25_000));
        assert_eq!(finder.snapshot(25_000).trend, "unknown");
        finder.clear();
        assert_eq!(finder.snapshot(25_000).last_heard_age_ms, None);
    }

    #[test]
    fn delayed_advertisement_from_previous_pair_cannot_seed_new_pair_signal() {
        let mut finder = Finder::new("old".into(), true);
        finder.start("old");
        assert!(finder.observe_verified("old", -70, 100));
        finder.set_identity("new", true);
        finder.start("new");
        assert!(!finder.observe_verified("old", -25, 200));
        assert_eq!(finder.snapshot(200).rssi, None);
        assert!(finder.observe_verified("new", -75, 300));
        assert_eq!(finder.snapshot(300).rssi, Some(-75));
    }

    #[test]
    fn snapshot_revision_advances_on_session_and_signal_changes() {
        let mut finder = Finder::new("old".into(), true);
        let initial = finder.snapshot(0).revision;
        let owner = finder.start("old").unwrap();
        let started = finder.snapshot(0).revision;
        assert!(started > initial);
        assert!(finder.observe_verified("old", -70, 100));
        let observed = finder.snapshot(100).revision;
        assert!(observed > started);
        finder.set_paused(true);
        let paused = finder.snapshot(100).revision;
        assert!(paused > observed);
        assert!(finder.stop("old", owner));
        let stopped = finder.snapshot(100).revision;
        assert!(stopped > paused);
        finder.set_identity("new", false);
        assert!(finder.snapshot(100).revision > stopped);
    }

    #[test]
    fn out_of_range_event_does_not_refresh_last_heard() {
        let mut finder = Finder::new("aaa".into(), true);
        finder.start("aaa");
        assert!(!finder.observe_verified("aaa", -127, 100));
        assert_eq!(finder.snapshot(100).last_heard_age_ms, None);
        assert!(finder.observe_verified("aaa", 0, 110));
        assert!(!finder.observe_verified("aaa", 21, 200));
        assert_eq!(finder.snapshot(200).last_heard_age_ms, Some(90));
        assert!(finder.observe_verified("aaa", 20, 300));
        assert!(!finder.observe_verified("aaa", -127, 400));
        assert_eq!(finder.snapshot(400).last_heard_age_ms, Some(100));
    }
}
