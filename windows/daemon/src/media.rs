//! Auto-pause through SMTC, with local Plexamp control for its keyboard-only player.
//! Ear removal pauses the current media session without changing its A2DP profile.
//! Resume applies only to the session and pause owned by NTPods.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::Foundation::{
    AsyncStatus, EventRegistrationToken, IAsyncOperation, TypedEventHandler,
};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as SessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
    MediaPropertiesChangedEventArgs, PlaybackInfoChangedEventArgs,
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
    // Session wrappers have manager-local identity, even for the same player.
    manager: SessionManager,
    session: Session,
    changed: EventRegistrationToken,
    media_changed: Option<EventRegistrationToken>,
    claim: Arc<PauseClaim>,
}

pub enum PausedPlayback {
    Session(PausedSession),
    Plexamp(crate::plexamp::Paused),
}

impl PausedPlayback {
    pub fn resume(self) -> bool {
        match self {
            Self::Session(session) => session.resume(),
            Self::Plexamp(session) => session.resume(),
        }
    }
}

pub fn pause_playing(address: Option<u64>) -> Option<PausedPlayback> {
    if is_playing() {
        return pause_playing_session().map(PausedPlayback::Session);
    }
    crate::plexamp::pause(address).map(PausedPlayback::Plexamp)
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

    fn revoke(&self) {
        self.0.store(2, Ordering::Release);
    }
}

pub fn pause_playing_session() -> Option<PausedSession> {
    let manager = SessionManager::RequestAsync().ok()?.get().ok()?;
    let session = manager.GetCurrentSession().ok()?;
    pause_session(manager, session, false)
}

fn pause_session(
    manager: SessionManager,
    session: Session,
    confirm: bool,
) -> Option<PausedSession> {
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
    let mut paused = PausedSession {
        manager,
        session,
        changed,
        media_changed: None,
        claim,
    };
    let observed = paused.claim.clone();
    let media_handler =
        TypedEventHandler::<Session, MediaPropertiesChangedEventArgs>::new(move |_, _| {
            observed.revoke();
            Ok(())
        });
    paused.media_changed = Some(paused.session.MediaPropertiesChanged(&media_handler).ok()?);
    let operation = paused.session.TryPauseAsync().ok()?;
    if !(if confirm {
        bounded_result(operation)?
    } else {
        operation.get().ok()?
    }) {
        return None;
    }
    if confirm {
        let deadline = Instant::now() + Duration::from_millis(750);
        loop {
            if paused
                .session
                .GetPlaybackInfo()
                .ok()?
                .PlaybackStatus()
                .ok()?
                == PlaybackStatus::Paused
            {
                break;
            }
            if Instant::now() >= deadline || !paused.claim.may_resume() {
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    Some(paused)
}

fn bounded_result<T: windows::core::RuntimeType + 'static>(
    operation: IAsyncOperation<T>,
) -> Option<T> {
    let deadline = Instant::now() + Duration::from_millis(750);
    loop {
        match operation.Status().ok()? {
            AsyncStatus::Completed => return operation.GetResults().ok(),
            AsyncStatus::Started if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10))
            }
            _ => {
                let _ = operation.Cancel();
                return None;
            }
        }
    }
}

impl PausedSession {
    pub fn resume(self) -> bool {
        let Ok(current) = self.manager.GetCurrentSession() else {
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
            .and_then(bounded_result)
            .unwrap_or(false)
    }
}

impl Drop for PausedSession {
    fn drop(&mut self) {
        let _ = self.session.RemovePlaybackInfoChanged(self.changed);
        if let Some(token) = self.media_changed {
            let _ = self.session.RemoveMediaPropertiesChanged(token);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseReceipt {
    Paused,
    Quiet,
    Failed,
    Resumed(bool),
}

enum SelectedPauseCommand {
    Resume(Box<dyn FnOnce() -> bool + Send>),
    Stop,
}

/// The COM player stays on its worker; dropping this ticket never presses Play.
pub struct SelectedPause {
    receipts: Receiver<PauseReceipt>,
    commands: Sender<SelectedPauseCommand>,
    cancelled: Arc<AtomicBool>,
    resume_requested: bool,
}

impl SelectedPause {
    pub fn start(address: u64) -> Self {
        Self::start_output(Some(address))
    }

    pub fn start_dongle() -> Self {
        Self::start_output(None)
    }

    fn start_output(address: Option<u64>) -> Self {
        let (send_receipt, receipts) = mpsc::channel();
        let (commands, receive_command) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        std::thread::spawn(move || {
            init();
            let route_ready = || address.map_or_else(
                || crate::volume::default_dongle_output().is_ok(),
                |address| crate::volume::output_device_for(Some(address)).is_ok(),
            );
            let owns_player = |player: &str| address.map_or_else(
                || crate::volume::player_owns_dongle_output(player),
                |address| crate::volume::player_owns_output(address, player),
            );
            let active = if route_ready() { crate::volume::render_activity(address) } else { Err(windows::core::Error::from(windows::Win32::Foundation::E_FAIL)) };
            let playback = match active {
                Ok(false) => None,
                Ok(true) if !worker_cancelled.load(Ordering::Acquire) => {
                    let selected = (|| {
                        let manager = bounded_result(SessionManager::RequestAsync().ok()?)?;
                        let session = manager.GetCurrentSession().ok()?;
                        let player = session.SourceAppUserModelId().ok()?.to_string();
                        let sessions = manager.GetSessions().ok()?;
                        let same_player = (0..sessions.Size().ok()?)
                            .filter(|index| {
                                sessions
                                    .GetAt(*index)
                                    .ok()
                                    .and_then(|session| session.SourceAppUserModelId().ok())
                                    .is_some_and(|source| source.to_string() == player)
                            })
                            .count();
                        if worker_cancelled.load(Ordering::Acquire)
                            || same_player != 1
                            || !owns_player(&player)
                        {
                            return None;
                        }
                        pause_session(manager, session, true).map(PausedPlayback::Session)
                    })()
                    .or_else(|| {
                        if worker_cancelled.load(Ordering::Acquire)
                            || !owns_player("Plexamp.exe")
                        {
                            return None;
                        }
                        crate::plexamp::pause(address).map(PausedPlayback::Plexamp)
                    });
                    if selected.is_none() {
                        let _ = send_receipt.send(PauseReceipt::Failed);
                        return;
                    }
                    selected
                }
                _ => {
                    let _ = send_receipt.send(PauseReceipt::Failed);
                    return;
                }
            };
            if worker_cancelled.load(Ordering::Acquire) {
                return;
            }
            if send_receipt
                .send(if playback.is_some() {
                    PauseReceipt::Paused
                } else {
                    PauseReceipt::Quiet
                })
                .is_err()
            {
                return;
            }
            // No controller can retain a forgotten resume claim indefinitely.
            match receive_command.recv_timeout(Duration::from_secs(24 * 60 * 60)) {
                Ok(SelectedPauseCommand::Resume(permitted)) => {
                    let resumed = resume_owned(
                        &worker_cancelled,
                        || permitted() && route_ready(),
                        || playback.is_some_and(PausedPlayback::resume),
                    );
                    let _ = send_receipt.send(PauseReceipt::Resumed(resumed));
                }
                _ => (),
            }
        });
        Self {
            receipts,
            commands,
            cancelled,
            resume_requested: false,
        }
    }

    pub fn poll(&self) -> Option<PauseReceipt> {
        match self.receipts.try_recv() {
            Ok(receipt) => Some(receipt),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }

    pub fn resume(&mut self, permitted: impl FnOnce() -> bool + Send + 'static) -> bool {
        if self.resume_requested || self.cancelled.load(Ordering::Acquire) {
            return false;
        }
        self.resume_requested = true;
        self.commands
            .send(SelectedPauseCommand::Resume(Box::new(permitted)))
            .is_ok()
    }
}

fn resume_owned(
    cancelled: &AtomicBool,
    permitted: impl FnOnce() -> bool,
    play: impl FnOnce() -> bool,
) -> bool {
    !cancelled.load(Ordering::Acquire)
        && permitted()
        && !cancelled.load(Ordering::Acquire)
        && play()
}

impl Drop for SelectedPause {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        let _ = self.commands.send(SelectedPauseCommand::Stop);
    }
}

#[cfg(test)]
mod tests {
    use super::{AutoPausePolicy, PauseClaim};

    #[test]
    #[ignore = "Requires a running Windows media player; does not change playback"]
    fn retained_manager_preserves_current_session_identity() {
        super::init();
        let manager = super::SessionManager::RequestAsync()
            .unwrap()
            .get()
            .unwrap();
        let session = manager.GetCurrentSession().unwrap();
        assert!(manager.GetCurrentSession().unwrap() == session);
    }

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
    fn changing_track_revokes_the_owned_pause_without_a_playback_notification() {
        let claim = PauseClaim::default();
        claim.observe(true);
        claim.revoke();
        claim.observe(true);
        assert!(!claim.may_resume());
    }

    #[test]
    fn cancelled_or_superseded_call_never_plays_the_owned_player() {
        use std::cell::Cell;
        use std::sync::atomic::{AtomicBool, Ordering};
        let cancelled = AtomicBool::new(false);
        let played = Cell::new(false);
        assert!(!super::resume_owned(
            &cancelled,
            || false,
            || {
                played.set(true);
                true
            }
        ));
        assert!(!played.get());
        assert!(!super::resume_owned(
            &cancelled,
            || {
                cancelled.store(true, Ordering::Release);
                true
            },
            || {
                played.set(true);
                true
            }
        ));
        assert!(!played.get());
        assert!(!super::resume_owned(
            &cancelled,
            || panic!("cancelled guard must not run"),
            || panic!("cancelled play")
        ));
    }

    #[test]
    fn resume_is_queued_once_and_drop_cancels_even_a_queued_resume() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{mpsc, Arc};
        let (_send, receipts) = mpsc::channel();
        let (commands, receive) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let mut ticket = super::SelectedPause {
            receipts,
            commands,
            cancelled: cancelled.clone(),
            resume_requested: false,
        };
        assert!(ticket.resume(|| true));
        assert!(!ticket.resume(|| panic!("duplicate resume")));
        drop(ticket);
        assert!(cancelled.load(Ordering::Acquire));
        let super::SelectedPauseCommand::Resume(guard) = receive.try_recv().unwrap() else {
            panic!("resume command")
        };
        assert!(!super::resume_owned(&cancelled, guard, || panic!(
            "dropped ticket must not play"
        )));
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
