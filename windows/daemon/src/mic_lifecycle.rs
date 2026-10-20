//! Pure microphone policy. All Bluetooth actions run on the receive thread.
const PCM_TIMEOUT_MS: u64 = 5_000;
const QUIET_MS: u64 = 500;
const DRAIN_TIMEOUT_MS: u64 = 2_000;
use ntpods_ipc::{MicMode, Snapshot};

pub fn playback_allows_microphone(state: &Snapshot) -> bool {
    if state.playback_buffer_target_ms.is_some() {
        return false;
    }
    // Hearing Off remains available during capture; pending edits only block a new uplink.
    if (state.hearing_aid_pending
        || state.hardware_diagnostics_pending
        || state.hearing_metadata_pending
        || state
            .hardware_check
            .as_ref()
            .is_some_and(|job| job.is_active()))
        && !state.mic_uplink_active
        && !matches!(
            state.mic_state.as_str(),
            "starting" | "capturing" | "stopping" | "recovering"
        )
    {
        return false;
    }
    if state.playback_native_driver {
        state.playback_state == "ready"
    } else {
        !matches!(state.playback_state.as_str(), "starting" | "stopping")
    }
}

/// User intent is separate from Windows clients and Bluetooth recovery.
#[derive(Default)]
pub struct Demand {
    pub mode: MicMode,
    pub stop_delay_secs: u32,
    pub manual_active: bool,
    captured: bool,
    unused_since_ms: Option<u64>,
}

impl Demand {
    pub fn configure(&mut self, mode: MicMode, stop_delay_secs: u32) -> bool {
        if stop_delay_secs > 3600 {
            return false;
        }
        if self.mode != mode {
            self.clear();
        }
        self.mode = mode;
        self.stop_delay_secs = stop_delay_secs;
        true
    }

    pub fn set_manual(&mut self, active: bool) {
        self.manual_active = self.mode == MicMode::Manual && active;
    }

    pub fn toggle_manual(
        &mut self,
        now: u64,
        enabled: bool,
        capture: bool,
        stream_active: bool,
        start_allowed: Result<(), &'static str>,
    ) -> Result<bool, &'static str> {
        let active = !(self.wanted(now, enabled, capture, true) || stream_active);
        if active {
            start_allowed?;
        }
        self.configure(MicMode::Manual, self.stop_delay_secs);
        self.set_manual(active);
        Ok(active)
    }

    pub fn clear(&mut self) {
        self.manual_active = false;
        self.captured = false;
        self.unused_since_ms = None;
    }

    pub fn observe_capture(&mut self, now: u64, capture: bool) {
        if capture {
            self.captured = true;
            self.unused_since_ms = None;
        } else if self.captured {
            self.captured = false;
            self.unused_since_ms = Some(now);
        }
    }

    pub fn wanted(&mut self, now: u64, permitted: bool, capture: bool, ear_allowed: bool) -> bool {
        if !permitted {
            self.clear();
            return false;
        }
        if !ear_allowed {
            self.clear();
            // Lifecycle gates wear separately. Preserve the underlying request
            // so removing an earbud cannot clear a failed-start latch.
            return match self.mode {
                MicMode::Automatic => capture,
                MicMode::KeepActive => true,
                MicMode::Manual => false,
            };
        }
        self.observe_capture(now, capture);
        match self.mode {
            MicMode::Automatic => {
                capture
                    || self.unused_since_ms.is_some_and(|since| {
                        now.saturating_sub(since) < u64::from(self.stop_delay_secs) * 1000
                    })
            }
            MicMode::KeepActive => true,
            MicMode::Manual => self.manual_active,
        }
    }

    pub fn preferences_json(&self) -> String {
        serde_json::json!({"mode": self.mode, "stop_delay_secs": self.stop_delay_secs}).to_string()
    }

    pub fn from_preferences_json(text: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(text).ok()?;
        let mode = serde_json::from_value(value.get("mode")?.clone()).ok()?;
        let stop_delay_secs = u32::try_from(value.get("stop_delay_secs")?.as_u64()?).ok()?;
        let mut demand = Self::default();
        demand.configure(mode, stop_delay_secs).then_some(demand)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    Starting,
    Capturing,
    Stopping,
    Recovering,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Start,
    Stop,
    RetryStop,
    ResetAudio,
    Reconnect,
}

/// Retain confirmed wear state when audio recovery reopens the control channel.
#[derive(Default)]
pub struct WearState {
    stable: [Option<bool>; 2],
}

impl WearState {
    pub fn observe(&mut self, current: [Option<bool>; 2]) {
        for (previous, reported) in self.stable.iter_mut().zip(current) {
            if reported.is_some() {
                *previous = reported;
            }
        }
    }

    pub fn allows_capture(&self) -> bool {
        self.stable != [Some(false); 2]
    }
}

#[derive(Default)]
pub struct Lifecycle {
    pub phase: Phase,
    pub uplink_active: bool,
    audio_restore_pending: bool,
    deadline_ms: u64,
    last_audio_ms: Option<u64>,
    reset_finished: bool,
    reset_failed: bool,
    recovery_expires_ms: u64,
    stop_started_ms: Option<u64>,
    stop_packets: u32,
    stop_retried: bool,
    failed: bool,
    unexpected_audio: bool,
}

impl Lifecycle {
    pub fn tick(&mut self, now: u64, wanted: bool, ear_allowed: bool) -> Action {
        let capture_allowed = wanted && ear_allowed;
        if self.unexpected_audio {
            self.unexpected_audio = false;
            self.begin_stop(now);
            return Action::Stop;
        }
        match self.phase {
            Phase::Idle if capture_allowed => {
                self.phase = Phase::Starting;
                self.deadline_ms = now.saturating_add(PCM_TIMEOUT_MS);
                self.stop_started_ms = None;
                self.stop_packets = 0;
                Action::Start
            }
            Phase::Starting | Phase::Capturing
                if !capture_allowed || self.capture_timed_out(now) =>
            {
                self.failed |= capture_allowed;
                self.begin_stop(now);
                Action::Stop
            }
            Phase::Stopping if now >= self.deadline_ms => {
                if self.quiet_for_ms(now) >= QUIET_MS {
                    self.uplink_active = false;
                    if self.audio_restore_pending {
                        self.phase = Phase::Recovering;
                        Action::ResetAudio
                    } else {
                        self.finish_stop();
                        Action::None
                    }
                } else if now < self.recovery_expires_ms {
                    Action::None // allow ordinary queued SDUs the full drain window
                } else if !self.stop_retried {
                    // START may complete after a short capture already sent
                    // STOP. Repeat only STOP once before touching playback.
                    self.stop_retried = true;
                    self.deadline_ms = now + QUIET_MS;
                    self.recovery_expires_ms = now + DRAIN_TIMEOUT_MS;
                    Action::RetryStop
                } else {
                    self.phase = Phase::Recovering;
                    Action::ResetAudio
                }
            }
            Phase::Recovering if self.reset_finished && now >= self.deadline_ms => {
                let still_arriving = self
                    .last_audio_ms
                    .is_some_and(|t| now.saturating_sub(t) < QUIET_MS);
                if still_arriving && !self.reset_failed && now < self.recovery_expires_ms {
                    return Action::None; // drain queued SDUs before judging the stop
                }
                if self.reset_failed || still_arriving {
                    self.phase = Phase::Error;
                    Action::Reconnect
                } else {
                    self.finish_stop();
                    Action::None
                }
            }
            // Taking the AirPods out does not revoke the app's capture request.
            Phase::Error if !wanted && !self.cleanup_pending() => {
                *self = Self::default();
                Action::None
            }
            _ => Action::None,
        }
    }

    pub fn audio_packet(&mut self, now: u64) {
        self.last_audio_ms = Some(now);
        self.uplink_active = true;
        self.audio_restore_pending = true;
        if matches!(self.phase, Phase::Stopping | Phase::Recovering) {
            self.stop_packets = self.stop_packets.saturating_add(1);
        }
        if matches!(self.phase, Phase::Idle | Phase::Error) {
            self.unexpected_audio = true;
        }
    }
    pub fn request_stop(&mut self, now: u64) {
        if self.cleanup_pending() {
            self.begin_stop(now);
        } else {
            // A cancelled START that never reached Bluetooth needs no drain window.
            self.finish_stop();
        }
    }
    pub fn start_attempted(&mut self) {
        self.uplink_active = true;
        self.audio_restore_pending = true;
    }
    /// The first PCM deadline starts after the Bluetooth command leaves its queue.
    pub fn start_sent(&mut self, now: u64) {
        if self.phase == Phase::Starting {
            self.deadline_ms = now.saturating_add(PCM_TIMEOUT_MS);
        }
    }
    pub fn cleanup_pending(&self) -> bool {
        self.uplink_active || self.audio_restore_pending
    }
    /// Retry after permission was revoked and microphone cleanup finished.
    pub fn retry_after_revocation(&mut self) {
        if self.phase == Phase::Error && !self.cleanup_pending() {
            *self = Self::default();
        }
    }
    /// Decoded audio renews stream health even when no app wants its samples.
    pub fn pcm_received(&mut self, now: u64) {
        if self.accepts_pcm() {
            self.phase = Phase::Capturing;
            self.deadline_ms = now.saturating_add(PCM_TIMEOUT_MS);
        }
    }
    pub fn capture_timed_out(&self, now: u64) -> bool {
        self.accepts_pcm() && now >= self.deadline_ms
    }
    pub fn start_failed(&mut self, now: u64) {
        self.failed = true;
        self.begin_stop(now);
    }
    fn begin_stop(&mut self, now: u64) {
        self.phase = Phase::Stopping;
        self.deadline_ms = now + QUIET_MS;
        self.recovery_expires_ms = now + DRAIN_TIMEOUT_MS;
        self.stop_started_ms = Some(now);
        self.stop_packets = 0;
        self.stop_retried = false;
        self.reset_finished = false;
        self.reset_failed = false;
    }
    fn quiet_for_ms(&self, now: u64) -> u64 {
        now.saturating_sub(
            self.last_audio_ms
                .unwrap_or(0)
                .max(self.stop_started_ms.unwrap_or(0)),
        )
    }
    fn finish_stop(&mut self) {
        self.uplink_active = false;
        self.audio_restore_pending = false;
        self.phase = if self.failed {
            Phase::Error
        } else {
            Phase::Idle
        };
    }
    /// Timing and counts only; used to diagnose STOP without logging audio.
    pub fn stop_progress(&self, now: u64) -> (u64, u32, Option<u64>) {
        (
            now.saturating_sub(self.stop_started_ms.unwrap_or(now)),
            self.stop_packets,
            self.last_audio_ms.map(|t| now.saturating_sub(t)),
        )
    }
    pub fn reset_completed(&mut self, now: u64, success: bool) {
        self.reset_finished = true;
        self.reset_failed = !success;
        self.deadline_ms = now + QUIET_MS;
        self.recovery_expires_ms = now + DRAIN_TIMEOUT_MS;
    }
    /// Receiver polling must not add another idle wait after a recovery deadline.
    pub fn wait_ms(&self, now: u64, maximum: u32) -> u32 {
        match self.phase {
            Phase::Stopping => {}
            Phase::Recovering if self.reset_finished => {}
            _ => return maximum,
        }
        let deadline = if self.reset_failed {
            self.deadline_ms
        } else {
            self.deadline_ms.max(
                self.last_audio_ms
                    .unwrap_or(0)
                    .saturating_add(QUIET_MS)
                    .min(self.recovery_expires_ms),
            )
        };
        deadline.saturating_sub(now).min(u64::from(maximum)) as u32
    }
    pub fn accepts_pcm(&self) -> bool {
        matches!(self.phase, Phase::Starting | Phase::Capturing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_delay_starts_at_last_client_close_and_reopen_cancels_it() {
        let mut demand = Demand::default();
        demand.configure(MicMode::Automatic, 30);
        let mut mic = Lifecycle::default();
        assert!(!demand.wanted(0, true, false, true));
        assert_eq!(
            mic.tick(1, demand.wanted(1, true, true, true), true),
            Action::Start
        );
        mic.start_attempted();
        mic.pcm_received(1);
        demand.observe_capture(100, false);
        for now in [5_000, 10_000, 15_000, 20_000, 25_000, 30_099] {
            // Warm uplink PCM is healthy; no Windows client receives these samples.
            mic.pcm_received(now);
            assert_eq!(
                mic.tick(now, demand.wanted(now, true, false, true), true),
                Action::None
            );
        }
        assert_eq!(
            mic.tick(30_099, demand.wanted(30_099, true, true, true), true),
            Action::None
        );
        demand.observe_capture(40_000, false);
        assert!(demand.wanted(69_999, true, false, true));
        mic.pcm_received(69_999);
        assert_eq!(
            mic.tick(70_000, demand.wanted(70_000, true, false, true), true),
            Action::Stop
        );
    }

    #[test]
    fn default_automatic_stops_immediately_and_multiple_clients_keep_it_wanted() {
        let mut demand = Demand::default();
        assert!(demand.wanted(0, true, true, true));
        // The caller passes streams > 0, so one of several clients closing stays true.
        assert!(demand.wanted(100, true, true, true));
        assert!(!demand.wanted(200, true, false, true));
    }

    #[test]
    fn keep_active_starts_without_clients_and_safety_preempts_every_policy() {
        for mode in [MicMode::Automatic, MicMode::KeepActive, MicMode::Manual] {
            let mut demand = Demand::default();
            demand.configure(mode, 3600);
            demand.set_manual(true);
            assert!(demand.wanted(0, true, true, true));
            assert!(demand.wanted(1, true, false, true));
            let mut mic = Lifecycle::default();
            assert_eq!(
                mic.tick(1, demand.wanted(1, true, false, true), true),
                Action::Start
            );
            mic.start_attempted();
            mic.pcm_received(1);
            // Disabled, disconnected and shutting down all revoke permission.
            assert_eq!(
                mic.tick(2, demand.wanted(2, false, false, true), true),
                Action::Stop
            );
            assert!(!demand.manual_active);
            assert!(!mic.accepts_pcm());
            demand.set_manual(true);
            assert_eq!(
                mic.tick(3, demand.wanted(3, true, true, false), false),
                Action::None
            );
            assert!(!mic.accepts_pcm());
            assert!(!demand.manual_active);
        }
        let mut keep = Demand::default();
        keep.configure(MicMode::KeepActive, 0);
        assert!(keep.wanted(0, true, false, true));
    }

    #[test]
    fn manual_stop_stays_stopped_while_an_app_keeps_recording() {
        let mut demand = Demand::default();
        demand.configure(MicMode::Manual, 30);
        assert!(!demand.wanted(0, true, true, true));
        demand.set_manual(true);
        assert!(demand.wanted(1, true, false, true));
        demand.set_manual(false);
        for now in [2, 30_000, 3_600_000] {
            assert!(!demand.wanted(now, true, true, true));
        }
        demand.set_manual(true);
        assert!(demand.wanted(3_600_001, true, true, true));
        demand.clear(); // session closed or both earbuds removed
        assert!(!demand.wanted(3_600_002, true, true, true));
    }

    #[test]
    fn shortcut_starts_manual_without_clients_and_retains_only_preferences() {
        let mut demand = Demand::default();
        demand.configure(MicMode::Automatic, 30);
        assert_eq!(
            demand.toggle_manual(0, false, false, false, Ok(())),
            Ok(true)
        );
        assert_eq!(demand.mode, MicMode::Manual);
        assert!(demand.wanted(1, true, false, true));
        let restored = Demand::from_preferences_json(&demand.preferences_json()).unwrap();
        assert_eq!(restored.mode, MicMode::Manual);
        assert_eq!(restored.stop_delay_secs, 30);
        assert!(!restored.manual_active);
        assert_eq!(
            demand.toggle_manual(2, true, false, false, Ok(())),
            Ok(false)
        );
        assert!(!demand.wanted(3, true, true, true));
    }

    #[test]
    fn shortcut_stops_every_policy_and_armed_stream_despite_start_rejection() {
        for mode in [MicMode::Automatic, MicMode::KeepActive, MicMode::Manual] {
            let mut demand = Demand::default();
            demand.configure(mode, 30);
            demand.set_manual(true);
            assert_eq!(
                demand.toggle_manual(0, true, true, false, Err("disconnected")),
                Ok(false)
            );
            assert_eq!(demand.mode, MicMode::Manual);
            assert!(!demand.wanted(1, true, true, true));
        }
        let mut demand = Demand::default();
        assert_eq!(
            demand.toggle_manual(0, false, false, true, Err("unsupported")),
            Ok(false)
        );
        assert!(!demand.manual_active);
        assert_eq!(
            demand.toggle_manual(1, true, false, false, Err("recovering")),
            Err("recovering")
        );
        assert!(!demand.manual_active);
    }

    #[test]
    fn shortcut_stops_an_automatic_idle_hold_without_restarting_for_capture() {
        let mut demand = Demand::default();
        demand.configure(MicMode::Automatic, 30);
        demand.observe_capture(0, true);
        demand.observe_capture(1, false);
        assert_eq!(
            demand.toggle_manual(2, true, false, false, Ok(())),
            Ok(false)
        );
        for now in [3, 30_002, 3_600_000] {
            assert!(!demand.wanted(now, true, true, true));
        }
    }

    #[test]
    fn rejected_shortcut_start_does_not_change_policy() {
        let mut demand = Demand::default();
        demand.configure(MicMode::Automatic, 30);
        assert_eq!(
            demand.toggle_manual(0, false, false, false, Err("unavailable")),
            Err("unavailable")
        );
        assert_eq!(demand.mode, MicMode::Automatic);
        assert_eq!(demand.stop_delay_secs, 30);
        assert!(!demand.manual_active);
    }

    #[test]
    fn policy_wear_changes_do_not_retry_a_failed_automatic_or_kept_stream() {
        for mode in [MicMode::Automatic, MicMode::KeepActive] {
            let mut demand = Demand::default();
            demand.configure(mode, 30);
            let mut mic = Lifecycle::default();
            assert_eq!(
                mic.tick(0, demand.wanted(0, true, true, true), true),
                Action::Start
            );
            mic.start_attempted();
            mic.start_failed(1);
            assert_eq!(
                mic.tick(501, demand.wanted(501, true, true, false), false),
                Action::ResetAudio
            );
            mic.reset_completed(1_000, true);
            assert_eq!(
                mic.tick(1_500, demand.wanted(1_500, true, true, false), false),
                Action::None
            );
            assert_eq!(mic.phase, Phase::Error);
            for worn in [false, true, false, true] {
                assert_eq!(
                    mic.tick(2_000, demand.wanted(2_000, true, true, worn), worn),
                    Action::None
                );
                assert_eq!(mic.phase, Phase::Error);
            }
            assert_eq!(
                mic.tick(2_001, demand.wanted(2_001, false, true, false), false),
                Action::None
            );
            assert_eq!(mic.phase, Phase::Idle);
        }
    }

    #[test]
    fn policy_persists_without_start_intent_and_rejects_bad_delays() {
        let mut demand = Demand::default();
        demand.configure(MicMode::Manual, 3600);
        demand.set_manual(true);
        let saved = demand.preferences_json();
        assert!(!saved.contains("manual_active"));
        let restored = Demand::from_preferences_json(&saved).unwrap();
        assert_eq!(restored.mode, MicMode::Manual);
        assert_eq!(restored.stop_delay_secs, 3600);
        assert!(!restored.manual_active);
        for text in [
            r#"{"mode":"manual","stop_delay_secs":-1}"#,
            r#"{"mode":"manual","stop_delay_secs":3601}"#,
            r#"{"mode":"manual","stop_delay_secs":4294967296}"#,
            r#"{"mode":"unknown","stop_delay_secs":0}"#,
        ] {
            assert!(Demand::from_preferences_json(text).is_none());
        }
    }

    #[test]
    fn a_queued_start_gets_a_full_pcm_window_without_overriding_stop() {
        let mut mic = Lifecycle::default();
        assert_eq!(mic.tick(0, true, true), Action::Start);
        mic.start_attempted();
        mic.start_sent(10_000);
        assert_eq!(mic.tick(10_001, true, true), Action::None);
        assert_eq!(mic.tick(15_000, true, true), Action::Stop);
        mic.start_sent(16_000);
        assert_eq!(mic.phase, Phase::Stopping);
    }

    #[test]
    fn stalled_decoding_stops_and_latches_even_if_packets_keep_arriving() {
        let mut m = Lifecycle::default();
        assert_eq!(m.tick(0, true, true), Action::Start);
        m.start_attempted();
        m.audio_packet(100);
        m.pcm_received(100);
        for now in [1_000, 2_000, 5_099] {
            m.audio_packet(now); // malformed/undecodable payload is not progress
            assert_eq!(m.tick(now, true, true), Action::None);
        }
        assert!(m.capture_timed_out(5_100));
        assert_eq!(m.tick(5_100, true, true), Action::Stop);
        assert!(!m.accepts_pcm());
        m.pcm_received(5_101); // a late delivery cannot reopen the PCM gate
        assert_eq!(m.phase, Phase::Stopping);
        assert_eq!(m.tick(5_600, true, true), Action::ResetAudio);
        m.reset_completed(20_000, true);
        assert_eq!(m.tick(20_500, true, true), Action::None);
        assert_eq!(m.phase, Phase::Error);
        for now in [20_501, 60_000, 3_600_000] {
            assert_eq!(m.tick(now, true, true), Action::None);
        }
        assert_eq!(m.tick(3_600_001, false, true), Action::None);
        assert_eq!(m.tick(3_600_002, true, true), Action::Start);
    }

    #[test]
    fn delivered_pcm_renews_deadline_but_no_packets_times_out() {
        let mut m = Lifecycle::default();
        m.tick(0, true, true);
        m.start_attempted();
        for now in [4_999, 9_998, 14_997] {
            assert_eq!(m.tick(now, true, true), Action::None);
            m.pcm_received(now); // all-zero decoded samples also count
        }
        assert_eq!(m.tick(19_996, true, true), Action::None);
        assert_eq!(m.tick(19_997, true, true), Action::Stop);
        assert_eq!(m.tick(20_497, true, true), Action::ResetAudio);
    }

    #[test]
    fn unknown_and_transitional_wear_reports_preserve_confirmed_state() {
        let mut wear = WearState::default();
        assert!(wear.allows_capture());
        wear.observe([None, None]);
        assert!(wear.allows_capture());
        wear.observe([Some(false), None]);
        assert!(wear.allows_capture());
        wear.observe([None, Some(false)]);
        assert!(!wear.allows_capture());
        wear.observe([None, None]);
        assert!(!wear.allows_capture());

        // A fresh AAP session must not forget that both AirPods are still out.
        for now in [0, 20_000] {
            let mut fresh = Lifecycle::default();
            assert_eq!(fresh.tick(now, true, wear.allows_capture()), Action::None);
            assert_eq!(fresh.phase, Phase::Idle);
        }
        wear.observe([Some(true), None]);
        assert!(wear.allows_capture());
        let mut fresh = Lifecycle::default();
        assert_eq!(
            fresh.tick(20_001, true, wear.allows_capture()),
            Action::Start
        );
    }

    #[test]
    fn one_worn_airpod_keeps_capture_allowed() {
        let mut wear = WearState::default();
        let mut m = Lifecycle::default();
        wear.observe([Some(true), Some(true)]);
        assert_eq!(m.tick(0, true, wear.allows_capture()), Action::Start);
        m.start_attempted();
        m.pcm_received(100);
        for reported in [[Some(false), Some(true)], [Some(true), Some(false)]] {
            wear.observe(reported);
            assert!(wear.allows_capture());
            assert_eq!(m.tick(100, true, wear.allows_capture()), Action::None);
            assert_eq!(m.phase, Phase::Capturing);
        }
    }

    #[test]
    fn both_out_stops_once_and_waits_for_reinsertion_after_recovery() {
        let mut m = Lifecycle::default();
        assert_eq!(m.tick(0, true, true), Action::Start);
        m.start_attempted();
        m.audio_packet(100);
        m.pcm_received(100);
        assert_eq!(m.tick(200, true, false), Action::Stop);
        assert!(!m.accepts_pcm());
        assert_eq!(m.tick(300, true, false), Action::None);
        assert_eq!(m.tick(700, true, false), Action::ResetAudio);
        m.reset_completed(17_000, true);
        assert_eq!(m.tick(17_500, true, false), Action::None);
        assert_eq!(m.phase, Phase::Idle);
        assert!(!m.cleanup_pending());
        for now in [17_501, 18_000, 60_000] {
            assert_eq!(m.tick(now, true, false), Action::None);
            assert_eq!(m.phase, Phase::Idle);
        }
        assert_eq!(m.tick(60_001, true, true), Action::Start);
    }

    #[test]
    fn quick_reinsertion_waits_for_the_entire_existing_recovery() {
        let mut m = Lifecycle::default();
        m.tick(0, true, true);
        m.start_attempted();
        m.pcm_received(100);
        assert_eq!(m.tick(100, true, false), Action::Stop);
        assert_eq!(m.tick(101, true, true), Action::None);
        assert!(!m.accepts_pcm());
        assert_eq!(m.tick(600, true, true), Action::ResetAudio);
        for now in [601, 5_000, 16_999] {
            assert_eq!(m.tick(now, true, true), Action::None);
            assert_eq!(m.phase, Phase::Recovering);
            assert!(!m.accepts_pcm());
        }
        m.reset_completed(17_000, true);
        assert_eq!(m.tick(17_499, true, true), Action::None);
        assert_eq!(m.tick(17_500, true, true), Action::None);
        assert_eq!(m.phase, Phase::Idle);
        assert_eq!(m.tick(17_501, true, true), Action::Start);
    }

    #[test]
    fn ending_capture_or_disabling_while_out_prevents_restart() {
        let mut m = Lifecycle::default();
        m.tick(0, true, true);
        m.start_attempted();
        m.pcm_received(100);
        assert_eq!(m.tick(100, true, false), Action::Stop);
        assert_eq!(m.tick(200, false, false), Action::None);
        assert_eq!(m.tick(600, false, false), Action::ResetAudio);
        m.reset_completed(17_000, true);
        assert_eq!(m.tick(17_500, false, false), Action::None);
        for now in [17_501, 60_000] {
            assert_eq!(m.tick(now, false, true), Action::None);
            assert_eq!(m.phase, Phase::Idle);
        }
    }

    #[test]
    fn removal_cancels_a_start_without_latching_failure() {
        let mut m = Lifecycle::default();
        m.tick(0, true, true);
        m.start_attempted();
        assert_eq!(m.tick(100, true, false), Action::Stop);
        assert_eq!(m.tick(600, true, false), Action::ResetAudio);
        m.reset_completed(17_000, true);
        assert_eq!(m.tick(17_500, true, false), Action::None);
        assert_eq!(m.phase, Phase::Idle);
        assert_eq!(m.tick(17_501, true, true), Action::Start);
    }

    #[test]
    fn ear_removal_cannot_clear_a_failed_capture_latch() {
        let mut m = Lifecycle::default();
        m.tick(0, true, true);
        m.start_attempted();
        m.start_failed(100);
        assert_eq!(m.tick(600, true, false), Action::ResetAudio);
        m.reset_completed(17_000, true);
        assert_eq!(m.tick(17_500, true, false), Action::None);
        assert_eq!(m.phase, Phase::Error);
        for ear_allowed in [false, true, false, true] {
            assert_eq!(m.tick(18_000, true, ear_allowed), Action::None);
            assert_eq!(m.phase, Phase::Error);
        }
        assert_eq!(m.tick(18_001, false, false), Action::None);
        assert_eq!(m.phase, Phase::Idle);
        assert_eq!(m.tick(18_002, true, false), Action::None);
        assert_eq!(m.tick(18_003, true, true), Action::Start);
    }

    #[test]
    fn idle_never_arms_a_microphone_without_capture() {
        let mut m = Lifecycle::default();
        for now in [0, 500, 60_000] {
            assert_eq!(m.tick(now, false, true), Action::None);
        }
        assert!(!m.uplink_active);
        assert!(!m.cleanup_pending());
    }
    #[test]
    fn cancelled_start_needs_no_stop_or_audio_reset() {
        let mut m = Lifecycle::default();
        assert_eq!(m.tick(0, true, true), Action::Start);
        assert!(!m.uplink_active);
        m.request_stop(1);
        assert_eq!(m.phase, Phase::Idle);
        assert_eq!(m.tick(1, false, true), Action::None);
        assert!(!m.cleanup_pending());
        assert!(!m.uplink_active);
        assert_eq!(m.tick(2, true, true), Action::Start);
    }
    #[test]
    fn cancellation_after_start_was_attempted_keeps_the_full_recovery() {
        let mut m = Lifecycle::default();
        assert_eq!(m.tick(0, true, true), Action::Start);
        m.start_attempted();
        m.request_stop(100);
        assert_eq!(m.phase, Phase::Stopping);
        assert!(m.cleanup_pending());
        assert_eq!(m.tick(599, true, true), Action::None);
        assert_eq!(m.wait_ms(599, 100), 1);
        assert_eq!(m.tick(600, true, true), Action::ResetAudio);
        assert!(!m.accepts_pcm());
    }
    #[test]
    fn receiver_wait_tracks_quiet_deadline_and_late_packets() {
        let mut m = Lifecycle::default();
        assert_eq!(m.wait_ms(0, 150), 150);
        assert_eq!(m.tick(0, true, true), Action::Start);
        m.start_attempted();
        assert_eq!(m.wait_ms(50, 150), 150);
        m.request_stop(100);
        assert_eq!(m.wait_ms(501, 150), 99);
        m.audio_packet(550);
        assert_eq!(m.tick(600, true, true), Action::None);
        assert_eq!(m.wait_ms(600, 150), 150);
        assert_eq!(m.wait_ms(1000, 100), 50);
        assert_eq!(m.wait_ms(1050, 150), 0);
        assert_eq!(m.tick(1050, true, true), Action::ResetAudio);
        // Waiting for playback completion has no expired polling deadline.
        assert_eq!(m.wait_ms(10_000, 150), 150);
        m.reset_completed(10_000, true);
        assert_eq!(m.wait_ms(10_499, 150), 1);
        assert_eq!(m.tick(10_499, true, true), Action::None);
        assert_eq!(m.tick(10_500, true, true), Action::None);
        assert_eq!(m.phase, Phase::Idle);
        assert_eq!(m.wait_ms(10_500, 150), 150);
        assert_eq!(m.tick(10_501, true, true), Action::Start);
    }
    #[test]
    fn receiver_wait_respects_retry_and_recovery_drain_limits() {
        let mut m = Lifecycle::default();
        m.tick(0, true, true);
        m.start_attempted();
        m.request_stop(100);
        for (audio_at, deadline, action) in [
            (2050, 2100, Action::RetryStop),
            (4050, 4100, Action::ResetAudio),
        ] {
            m.audio_packet(audio_at);
            assert_eq!(m.wait_ms(deadline - 1, 150), 1);
            assert_eq!(m.tick(deadline, false, true), action);
        }
        m.reset_completed(5000, true);
        m.audio_packet(5400);
        assert_eq!(m.wait_ms(5500, 150), 150);
        assert_eq!(m.tick(5500, true, true), Action::None);
        m.audio_packet(6950);
        assert_eq!(m.wait_ms(6999, 150), 1);
        assert_eq!(m.tick(7000, true, true), Action::Reconnect);
        assert_eq!(m.wait_ms(7000, 150), 150);
        assert_eq!(m.tick(7001, true, true), Action::None);
    }
    #[test]
    fn failed_reset_waits_once_then_stays_latched_despite_new_demand() {
        let mut m = Lifecycle::default();
        m.tick(0, true, true);
        m.start_attempted();
        m.request_stop(100);
        assert_eq!(m.tick(600, false, true), Action::ResetAudio);
        m.reset_completed(1000, false);
        m.audio_packet(1450);
        assert_eq!(m.wait_ms(1499, 150), 1);
        assert_eq!(m.tick(1499, true, true), Action::None);
        assert_eq!(m.tick(1500, true, true), Action::Reconnect);
        assert_eq!(m.wait_ms(1500, 150), 150);
        m.retry_after_revocation();
        assert_eq!(m.tick(1501, true, true), Action::None);
        assert_eq!(m.phase, Phase::Error);
    }
    #[test]
    fn quiet_stop_requires_one_restore_before_new_capture_or_shutdown_completion() {
        let mut m = Lifecycle::default();
        assert_eq!(m.tick(0, true, true), Action::Start);
        m.start_attempted();
        m.audio_packet(100);
        m.pcm_received(100);
        assert_eq!(m.phase, Phase::Capturing);
        assert_eq!(m.tick(200, false, true), Action::Stop);
        assert!(!m.accepts_pcm());
        assert_eq!(m.tick(300, true, true), Action::None);
        assert_eq!(m.tick(699, true, true), Action::None);
        assert_eq!(m.phase, Phase::Stopping);
        assert_eq!(m.tick(700, true, true), Action::ResetAudio);
        assert_eq!(m.phase, Phase::Recovering);
        assert!(!m.uplink_active);
        assert!(m.cleanup_pending());
        assert!(!m.accepts_pcm());
        assert_eq!(m.stop_progress(700), (500, 0, Some(600)));
        for now in [701, 900, 1000] {
            m.retry_after_revocation();
            assert_eq!(m.tick(now, true, true), Action::None);
            assert_eq!(m.phase, Phase::Recovering);
        }
        m.reset_completed(16_000, true);
        assert_eq!(m.tick(16_499, true, true), Action::None);
        assert!(m.cleanup_pending());
        assert_eq!(m.tick(16_500, true, true), Action::None);
        assert_eq!(m.phase, Phase::Idle);
        assert!(!m.cleanup_pending());
        assert!(!m.uplink_active); // session cleanup must not send another STOP
        assert_eq!(m.tick(16_501, true, true), Action::Start);
    }
    #[test]
    fn continued_audio_after_stop_requires_reconnection() {
        let mut m = Lifecycle::default();
        m.tick(0, true, true);
        m.tick(100, false, true);
        m.audio_packet(550);
        assert_eq!(m.tick(600, false, true), Action::None);
        for now in [1000, 1500, 2000] {
            m.audio_packet(now);
            assert_eq!(m.tick(now, false, true), Action::None);
        }
        assert_eq!(m.tick(2100, false, true), Action::RetryStop);
        for now in [2500, 3000, 3500, 4000] {
            m.audio_packet(now);
            assert_eq!(m.tick(now, false, true), Action::None); // no second retry
        }
        assert_eq!(m.tick(4100, false, true), Action::ResetAudio);
        assert_eq!(m.tick(4200, false, true), Action::None); // only one reset
        m.reset_completed(5000, true);
        m.audio_packet(5400);
        assert_eq!(m.tick(5500, false, true), Action::None);
        m.audio_packet(7000);
        assert_eq!(m.tick(7000, false, true), Action::Reconnect);
        assert!(!m.accepts_pcm());
        assert_eq!(m.tick(7100, true, true), Action::None);
    }
    #[test]
    fn failed_start_stops_then_latches_error_instead_of_retrying_forever() {
        let mut m = Lifecycle::default();
        m.tick(0, true, true);
        m.start_attempted();
        assert_eq!(m.tick(5000, true, true), Action::Stop);
        assert_eq!(m.tick(5500, true, true), Action::ResetAudio);
        m.reset_completed(6000, true);
        assert_eq!(m.tick(6500, true, true), Action::None);
        assert_eq!(m.phase, Phase::Error);
        assert_eq!(m.tick(9000, true, true), Action::None);
        m.tick(9001, false, true);
        assert_eq!(m.tick(9002, true, true), Action::Start);
    }
    #[test]
    fn failed_start_stays_latched_after_successful_audio_reset() {
        let mut m = Lifecycle::default();
        assert_eq!(m.tick(0, true, true), Action::Start);
        m.start_failed(100);
        m.audio_packet(2100);
        assert_eq!(m.tick(2100, true, true), Action::RetryStop);
        m.audio_packet(4100);
        assert_eq!(m.tick(4100, true, true), Action::ResetAudio);
        m.reset_completed(4500, true);
        assert_eq!(m.tick(5000, true, true), Action::None);
        assert_eq!(m.phase, Phase::Error);
        assert!(!m.uplink_active);
        assert_eq!(m.tick(6000, true, true), Action::None);
        assert_eq!(m.phase, Phase::Error);
    }
    #[test]
    fn quick_permission_revocation_retries_only_a_stopped_error() {
        let mut m = Lifecycle::default();
        assert_eq!(m.tick(0, true, true), Action::Start);
        m.start_attempted();
        assert_eq!(m.tick(5000, true, true), Action::Stop);
        assert_eq!(m.tick(5500, true, true), Action::ResetAudio);
        m.reset_completed(5500, true);
        assert_eq!(m.tick(6000, true, true), Action::None);
        assert_eq!(m.phase, Phase::Error);
        assert!(!m.cleanup_pending());

        m.retry_after_revocation();
        assert_eq!(m.tick(6001, true, true), Action::Start);

        m.phase = Phase::Error;
        m.uplink_active = true;
        m.retry_after_revocation();
        assert_eq!(m.phase, Phase::Error);
        assert!(m.uplink_active);
        assert_eq!(m.tick(6002, true, true), Action::None);
        m.uplink_active = false;
        m.audio_restore_pending = true;
        m.retry_after_revocation();
        assert_eq!(m.phase, Phase::Error);
        assert_eq!(m.tick(6003, false, true), Action::None);
        assert_eq!(m.phase, Phase::Error);
    }
    #[test]
    fn unsolicited_audio_after_crash_is_stopped_without_a_capture_client() {
        let mut m = Lifecycle::default();
        m.audio_packet(10);
        assert!(m.uplink_active);
        assert_eq!(m.tick(11, false, true), Action::Stop);
        assert!(!m.accepts_pcm());
        assert_eq!(m.tick(511, false, true), Action::ResetAudio);
        assert!(!m.uplink_active);
        assert!(m.cleanup_pending());
    }
    #[test]
    fn queued_audio_after_reset_does_not_force_a_reconnect_once_quiet() {
        let mut m = Lifecycle::default();
        m.tick(0, true, true);
        m.tick(100, false, true);
        m.audio_packet(2050);
        assert_eq!(m.tick(2100, false, true), Action::RetryStop);
        m.audio_packet(4050);
        assert_eq!(m.tick(4100, false, true), Action::ResetAudio);
        m.reset_completed(5000, true);
        m.audio_packet(5400);
        assert_eq!(m.tick(5500, false, true), Action::None);
        assert_eq!(m.phase, Phase::Recovering);
        assert_eq!(m.tick(5900, false, true), Action::None);
        assert_eq!(m.phase, Phase::Idle);
        assert!(!m.uplink_active);
    }
    #[test]
    fn delayed_start_after_stop_gets_one_stop_retry_then_restores_audio() {
        let mut m = Lifecycle::default();
        m.tick(0, true, true);
        assert_eq!(m.tick(100, false, true), Action::Stop);
        for now in [450, 850, 1250, 1650, 2050] {
            m.audio_packet(now);
            assert_eq!(m.tick(now, false, true), Action::None);
        }
        assert_eq!(m.tick(2100, false, true), Action::RetryStop);
        assert_eq!(m.phase, Phase::Stopping);
        assert_eq!(m.tick(2599, false, true), Action::None);
        assert_eq!(m.phase, Phase::Stopping);
        assert_eq!(m.tick(2600, false, true), Action::ResetAudio);
        assert_eq!(m.phase, Phase::Recovering);
        assert_eq!(m.stop_progress(2600), (2500, 5, Some(550)));
        assert!(!m.uplink_active);
        assert!(m.cleanup_pending());
        m.reset_completed(3000, true);
        assert_eq!(m.tick(3500, false, true), Action::None);
        assert_eq!(m.phase, Phase::Idle);
    }
    #[test]
    fn normal_late_packet_drains_without_retrying_stop() {
        let mut m = Lifecycle::default();
        m.tick(0, true, true);
        assert_eq!(m.tick(0, false, true), Action::Stop);
        m.audio_packet(150);
        assert_eq!(m.tick(600, false, true), Action::None);
        assert_eq!(m.phase, Phase::Stopping);
        assert_eq!(m.tick(650, false, true), Action::ResetAudio);
        assert_eq!(m.phase, Phase::Recovering);
        assert!(!m.uplink_active);
    }
    #[test]
    fn quiet_at_drain_deadline_restores_audio_without_stop_retry() {
        let mut m = Lifecycle::default();
        m.tick(0, true, true);
        m.tick(100, false, true);
        m.audio_packet(1600);
        assert_eq!(m.tick(2100, false, true), Action::ResetAudio);
        assert_eq!(m.phase, Phase::Recovering);
        assert!(!m.uplink_active);
    }
    #[test]
    fn brief_disable_is_not_lost_when_capture_is_requested_again() {
        let mut m = Lifecycle::default();
        m.tick(0, true, true);
        m.start_attempted();
        m.pcm_received(100);
        m.request_stop(100);
        assert_eq!(m.tick(101, true, true), Action::None);
        assert!(!m.accepts_pcm());
        assert_eq!(m.tick(600, true, true), Action::ResetAudio);
        assert_eq!(m.tick(601, true, true), Action::None);
        m.reset_completed(1000, true);
        assert_eq!(m.tick(1500, true, true), Action::None);
        assert_eq!(m.phase, Phase::Idle);
        assert_eq!(m.tick(1501, true, true), Action::Start);
    }
    #[test]
    fn failed_service_reset_requests_one_escalation() {
        let mut m = Lifecycle::default();
        m.tick(0, true, true);
        m.tick(100, false, true);
        m.audio_packet(2050);
        assert_eq!(m.tick(2100, false, true), Action::RetryStop);
        m.audio_packet(4050);
        assert_eq!(m.tick(4100, false, true), Action::ResetAudio);
        m.reset_completed(5000, false);
        assert_eq!(m.tick(5500, false, true), Action::Reconnect);
    }

    #[test]
    fn quiet_uplink_does_not_hide_failed_playback_restoration() {
        let mut m = Lifecycle::default();
        m.tick(0, true, true);
        m.start_attempted();
        assert_eq!(m.tick(100, false, true), Action::Stop);
        assert_eq!(m.tick(600, false, true), Action::ResetAudio);
        assert!(!m.uplink_active);
        assert!(m.cleanup_pending());
        m.reset_completed(16_000, false);
        assert_eq!(m.tick(16_500, false, true), Action::Reconnect);
        assert_eq!(m.phase, Phase::Error);
        assert!(m.cleanup_pending());
        assert!(!m.accepts_pcm());
        for wanted in [false, true] {
            m.retry_after_revocation();
            assert_eq!(m.tick(17_000, wanted, true), Action::None);
            assert_eq!(m.phase, Phase::Error);
        }
    }
}
