use ntpods_ipc::{PlaybackMode, Snapshot};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct Selection {
    pub mode: PlaybackMode,
    pub revision: u64,
    pub cancel: Option<Arc<AtomicBool>>,
}

impl Selection {
    pub fn new(mode: PlaybackMode) -> Self {
        Self {
            mode,
            revision: 0,
            cancel: None,
        }
    }

    pub fn change(
        &mut self,
        mode: PlaybackMode,
        retry: bool,
        microphone_busy: bool,
        persist: impl FnOnce(PlaybackMode) -> io::Result<()>,
    ) -> io::Result<bool> {
        if !retry && self.mode == mode {
            return Ok(false);
        }
        if microphone_busy {
            return Err(io::Error::other(
                "Stop the microphone before changing playback.",
            ));
        }
        persist(mode)?;
        self.stop();
        self.mode = mode;
        self.revision = self.revision.wrapping_add(1);
        Ok(true)
    }

    pub fn stop(&self) {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Release);
        }
    }

    pub fn should_attempt(
        &self,
        attempted: Option<u64>,
        stopped: bool,
        state: &Snapshot,
        capture_active: bool,
        mic_on: bool,
    ) -> bool {
        !stopped && attempted != Some(self.revision)
            // A command reserves the transition before a new capture client can start.
            && (matches!(state.playback_state.as_str(), "starting" | "stopping")
                || !microphone_busy(state, capture_active, mic_on))
    }
}

pub fn microphone_busy(state: &Snapshot, capture_active: bool, mic_on: bool) -> bool {
    capture_active
        || mic_on
        || state.mic_capture_active
        || state.mic_uplink_active
        || matches!(
            state.mic_state.as_str(),
            "starting" | "capturing" | "stopping" | "recovering"
        )
}

fn path() -> io::Result<std::path::PathBuf> {
    Ok(std::path::PathBuf::from(
        std::env::var_os("LOCALAPPDATA")
            .ok_or_else(|| io::Error::other("Local app data is unavailable"))?,
    )
    .join("NTPods")
    .join("playback-mode.json"))
}

pub fn load() -> PlaybackMode {
    path()
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save(mode: PlaybackMode) -> io::Result<()> {
    std::fs::write(path()?, serde_json::to_vec(&mode)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selecting_mode_cancels_only_after_preference_is_saved() {
        let mut selected = Selection::new(PlaybackMode::Windows);
        let cancel = Arc::new(AtomicBool::new(false));
        selected.cancel = Some(cancel.clone());
        assert!(selected
            .change(PlaybackMode::Aac, false, false, |_| Err(io::Error::other(
                "disk full"
            )))
            .is_err());
        assert_eq!(selected.mode, PlaybackMode::Windows);
        assert_eq!(selected.revision, 0);
        assert!(!cancel.load(Ordering::Acquire));
        assert!(selected
            .change(PlaybackMode::Aac, false, false, |_| Ok(()))
            .unwrap());
        assert_eq!(selected.mode, PlaybackMode::Aac);
        assert_eq!(selected.revision, 1);
        assert!(cancel.load(Ordering::Acquire));
        assert!(!selected
            .change(PlaybackMode::Aac, false, false, |_| panic!(
                "unchanged preference"
            ))
            .unwrap());
        assert!(selected
            .change(PlaybackMode::Aac, true, false, |_| Ok(()))
            .unwrap());
        assert_eq!(selected.revision, 2);
        assert!(selected
            .change(PlaybackMode::Windows, false, false, |_| Ok(()))
            .unwrap());
    }

    #[test]
    fn mode_changes_and_retries_preserve_active_capture() {
        let mut selected = Selection::new(PlaybackMode::Aac);
        let cancel = Arc::new(AtomicBool::new(false));
        selected.cancel = Some(cancel.clone());
        for (mode, retry) in [
            (PlaybackMode::Windows, false),
            (PlaybackMode::AacEld, false),
            (PlaybackMode::Aac, true),
        ] {
            assert!(selected
                .change(mode, retry, true, |_| panic!("must not persist"))
                .is_err());
            assert_eq!(selected.mode, PlaybackMode::Aac);
            assert_eq!(selected.revision, 0);
            assert!(!cancel.load(Ordering::Acquire));
        }
    }

    #[test]
    fn microphone_and_playback_transitions_are_mutually_exclusive() {
        let mut state = Snapshot::default();
        assert!(!microphone_busy(&state, false, false));
        assert!(microphone_busy(&state, true, false));
        assert!(microphone_busy(&state, false, true));
        for phase in ["starting", "capturing", "stopping", "recovering"] {
            state.mic_state = phase.into();
            assert!(microphone_busy(&state, false, false));
        }
        state.mic_state = "idle".into();
        state.mic_uplink_active = true;
        assert!(microphone_busy(&state, false, false));
    }

    #[test]
    fn failure_waits_for_retry_and_saved_choices_wait_for_existing_capture() {
        let mut selected = Selection::new(PlaybackMode::Aac);
        let mut state = Snapshot {
            playback_mode: PlaybackMode::Aac,
            playback_state: "disconnected".into(),
            ..Default::default()
        };
        assert!(!selected.should_attempt(None, false, &state, true, false));
        assert!(!selected.should_attempt(None, false, &state, false, true));
        assert!(selected.should_attempt(None, false, &state, false, false));
        let attempted = Some(selected.revision);
        state.playback_state = "error".into();
        assert!(!selected.should_attempt(attempted, false, &state, false, false));
        selected
            .change(PlaybackMode::Aac, true, false, |_| Ok(()))
            .unwrap();
        state.playback_state = "starting".into();
        assert!(selected.should_attempt(attempted, false, &state, true, false));
        state.playback_state = "stopping".into();
        assert!(selected.should_attempt(attempted, false, &state, true, false));
        assert!(!selected.should_attempt(attempted, true, &state, false, false));
        selected
            .change(PlaybackMode::Windows, false, false, |_| Ok(()))
            .unwrap();
        selected
            .change(PlaybackMode::Windows, true, false, |_| Ok(()))
            .unwrap();
        assert_eq!(selected.mode, PlaybackMode::Windows);
    }
}
