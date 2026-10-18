//! Auto-pause through Windows System Media Transport Controls (SMTC).
//! Ear removal pauses the current media session without changing its A2DP profile.
//! Resume applies only to the session and pause owned by NTPods.

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use windows::Foundation::{EventRegistrationToken, TypedEventHandler};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as SessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
    PlaybackInfoChangedEventArgs,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

pub struct AutoPausePolicy {
    enabled: bool,
    revision: u64,
}

impl AutoPausePolicy {
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            revision: 0,
        }
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        if !enabled {
            // A quick off/on must still revoke the old resume permission.
            self.revision = self.revision.wrapping_add(1);
        }
        self.enabled = enabled;
    }

    pub fn permission(&self, local_audio_route: bool) -> Option<u64> {
        (self.enabled && local_audio_route).then_some(self.revision)
    }
}

/// Initialize COM (MTA) for the calling thread — required before any SMTC call.
/// Call once, from the thread that will drive the media calls.
pub fn init() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

fn current_session() -> windows::core::Result<Session> {
    let manager = SessionManager::RequestAsync()?.get()?;
    manager.GetCurrentSession()
}

/// True if the system's current media session is actively playing. False when
/// there is no session or the call fails.
pub fn is_playing() -> bool {
    (|| -> windows::core::Result<bool> {
        let status = current_session()?.GetPlaybackInfo()?.PlaybackStatus()?;
        Ok(status == PlaybackStatus::Playing)
    })()
    .unwrap_or(false)
}

/// The exact session whose pause request succeeded. Never resume a different
/// application or a session that was already paused before ear removal.
pub struct PausedSession {
    session: Session,
    changed: EventRegistrationToken,
    claim: Arc<PauseClaim>,
}

#[derive(Default)]
struct PauseClaim(AtomicU8);

impl PauseClaim {
    fn observe(&self, paused: bool) {
        // Allow our one pause notification. Further changes revoke ownership,
        // even if a fast Play/Pause made both callbacks observe Paused.
        let _ = self
            .0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |state| {
                Some(if paused && state == 0 { 1 } else { 2 })
            });
    }

    fn may_resume(&self) -> bool {
        self.0.load(Ordering::Acquire) != 2
    }
}

pub fn pause_playing_session() -> Option<PausedSession> {
    let session = current_session().ok()?;
    if session.GetPlaybackInfo().ok()?.PlaybackStatus().ok()? != PlaybackStatus::Playing {
        return None;
    }
    let claim = Arc::new(PauseClaim::default());
    let observed = claim.clone();
    let handler =
        TypedEventHandler::<Session, PlaybackInfoChangedEventArgs>::new(move |sender, _| {
            let paused = sender
                .as_ref()
                .and_then(|session| session.GetPlaybackInfo().ok())
                .and_then(|info| info.PlaybackStatus().ok())
                == Some(PlaybackStatus::Paused);
            observed.observe(paused);
            Ok(())
        });
    let changed = session.PlaybackInfoChanged(&handler).ok()?;
    let paused = PausedSession {
        session,
        changed,
        claim,
    };
    paused
        .session
        .TryPauseAsync()
        .ok()?
        .get()
        .ok()?
        .then_some(paused)
}

impl PausedSession {
    pub fn resume(self) -> bool {
        let Ok(current) = current_session() else {
            return false;
        };
        if current != self.session {
            return false;
        }
        let Ok(info) = self.session.GetPlaybackInfo() else {
            return false;
        };
        if info.PlaybackStatus().ok() != Some(PlaybackStatus::Paused) || !self.claim.may_resume() {
            return false;
        }
        self.session
            .TryPlayAsync()
            .ok()
            .and_then(|op| op.get().ok())
            .unwrap_or(false)
    }
}

impl Drop for PausedSession {
    fn drop(&mut self) {
        let _ = self.session.RemovePlaybackInfoChanged(self.changed);
    }
}

#[cfg(test)]
mod tests {
    use super::{AutoPausePolicy, PauseClaim};

    #[test]
    fn own_pause_allows_resume_until_an_intervening_playback_change() {
        let claim = PauseClaim::default();
        assert!(claim.may_resume());
        claim.observe(true); // Our accepted pause notification.
        assert!(claim.may_resume());
        claim.observe(false); // User presses Play while an earbud stays out.
        claim.observe(true); // User then presses Pause.
        assert!(!claim.may_resume());
    }

    #[test]
    fn fast_play_pause_cannot_reclaim_ownership_when_both_callbacks_see_paused() {
        let claim = PauseClaim::default();
        claim.observe(true);
        claim.observe(true);
        claim.observe(true);
        assert!(!claim.may_resume());
    }

    #[test]
    fn unknown_or_changed_playback_before_the_pause_ack_revokes_ownership() {
        let claim = PauseClaim::default();
        claim.observe(false);
        claim.observe(true);
        assert!(!claim.may_resume());
    }

    #[test]
    fn passive_ear_reports_cannot_pause_or_resume_local_media() {
        let policy = AutoPausePolicy::new(true);
        assert!(policy.permission(false).is_none());
        assert!(policy.permission(true).is_some());
        assert!(policy.permission(false).is_none());
    }

    #[test]
    fn disabling_auto_pause_revokes_a_previous_pause_even_after_reenabling() {
        let mut policy = AutoPausePolicy::new(true);
        let paused_under = policy.permission(true).unwrap();
        policy.set_enabled(false);
        assert!(policy.permission(true).is_none());
        policy.set_enabled(true);
        assert_ne!(policy.permission(true), Some(paused_under));
        let current = policy.permission(true);
        policy.set_enabled(true);
        assert_eq!(policy.permission(true), current);
    }
}
