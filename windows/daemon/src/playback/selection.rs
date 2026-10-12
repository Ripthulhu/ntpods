use ntpods_ipc::{PlaybackBitrate, PlaybackMode, Snapshot};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct Selection {
    pub mode: PlaybackMode,
    pub revision: u64,
    pub cancel: Option<Arc<AtomicBool>>,
    pub bitrates: [PlaybackBitrate; 2],
}

impl Selection {
    pub fn new(mode: PlaybackMode) -> Self {
        Self {
            mode,
            revision: 0,
            cancel: None,
            bitrates: [PlaybackBitrate::default(); 2],
        }
    }

    pub fn change(
        &mut self,
        mode: PlaybackMode,
        retry: bool,
        microphone_busy: bool,
        persist: impl FnOnce(PlaybackMode) -> io::Result<()>,
    ) -> io::Result<bool> {
        if mode == PlaybackMode::Windows {
            return Err(io::Error::other(
                "Restore the Windows driver from the installer.",
            ));
        }
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
        _capture_active: bool,
        mic_on: bool,
    ) -> bool {
        !stopped && attempted != Some(self.revision)
            // A command reserves the transition before a new capture client can start.
            && (matches!(state.playback_state.as_str(), "starting" | "stopping")
                || (!mic_on && !state.mic_uplink_active
                    && !matches!(state.mic_state.as_str(), "starting" | "capturing" | "stopping" | "recovering")))
    }

    pub fn bitrate(&self, mode: PlaybackMode) -> PlaybackBitrate {
        self.bitrates[usize::from(mode == PlaybackMode::AacEld)]
    }

    pub fn change_bitrate(
        &mut self,
        mode: PlaybackMode,
        bitrate: PlaybackBitrate,
        busy: bool,
        persist: impl FnOnce([PlaybackBitrate; 2]) -> io::Result<()>,
    ) -> io::Result<bool> {
        validate_bitrate(mode, bitrate)?;
        let index = usize::from(mode == PlaybackMode::AacEld);
        if self.bitrates[index] == bitrate {
            return Ok(false);
        }
        if busy && mode == self.mode {
            return Err(io::Error::other(
                "Stop the microphone before changing playback.",
            ));
        }
        let mut next = self.bitrates;
        next[index] = bitrate;
        persist(next)?;
        self.bitrates = next;
        if mode == self.mode {
            self.stop();
            self.revision = self.revision.wrapping_add(1);
        }
        Ok(true)
    }
}

pub fn validate_bitrate(mode: PlaybackMode, bitrate: PlaybackBitrate) -> io::Result<()> {
    let range = match mode {
        PlaybackMode::Aac => 96..=512,
        PlaybackMode::AacEld => 128..=256,
        PlaybackMode::Windows => return Err(io::Error::other("Select AAC or AAC-ELD.")),
    };
    if !range.contains(&bitrate.kbps) {
        return Err(io::Error::other("Bitrate is outside the supported range."));
    }
    Ok(())
}

pub fn attempted_revision(attempted: Option<(u64, u64)>, intent: u64) -> Option<u64> {
    attempted
        .filter(|(_, previous)| *previous == intent)
        .map(|(revision, _)| revision)
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
    normalize_mode(
        path()
            .ok()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default(),
    )
}

fn normalize_mode(mode: PlaybackMode) -> PlaybackMode {
    match mode {
        PlaybackMode::Windows => PlaybackMode::Aac,
        mode => mode,
    }
}

pub fn load_bitrates() -> [PlaybackBitrate; 2] {
    path()
        .ok()
        .and_then(|path| std::fs::read(path.with_file_name("playback-bitrates.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<[PlaybackBitrate; 2]>(&bytes).ok())
        .filter(|values| {
            validate_bitrate(PlaybackMode::Aac, values[0]).is_ok()
                && validate_bitrate(PlaybackMode::AacEld, values[1]).is_ok()
        })
        .unwrap_or([PlaybackBitrate::default(); 2])
}

pub fn save_bitrates(values: [PlaybackBitrate; 2]) -> io::Result<()> {
    std::fs::write(
        path()?.with_file_name("playback-bitrates.json"),
        serde_json::to_vec(&values)?,
    )
}

pub fn save(mode: PlaybackMode) -> io::Result<()> {
    std::fs::write(path()?, serde_json::to_vec(&mode)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_audio_intent_allows_reconnect_but_same_failure_requires_retry() {
        assert_eq!(attempted_revision(Some((4, 8)), 8), Some(4));
        assert_eq!(attempted_revision(Some((4, 8)), 9), None);
        assert_eq!(attempted_revision(None, 8), None);
    }

    #[test]
    fn legacy_and_missing_selections_use_aac_without_driver_rebinding() {
        assert_eq!(normalize_mode(PlaybackMode::default()), PlaybackMode::Aac);
        assert_eq!(normalize_mode(PlaybackMode::AacEld), PlaybackMode::AacEld);
    }

    #[test]
    fn bitrate_preferences_are_separate_and_preserve_active_recording() {
        let mut selected = Selection::new(PlaybackMode::Aac);
        let cancel = Arc::new(AtomicBool::new(false));
        selected.cancel = Some(cancel.clone());
        let manual = PlaybackBitrate {
            best_effort: false,
            kbps: 128,
        };
        assert!(selected
            .change_bitrate(PlaybackMode::AacEld, manual, true, |values| {
                assert_eq!(values[0], PlaybackBitrate::default());
                assert_eq!(values[1], manual);
                Ok(())
            })
            .unwrap());
        assert_eq!(selected.revision, 0);
        assert!(!cancel.load(Ordering::Acquire));
        assert!(selected
            .change_bitrate(PlaybackMode::Aac, manual, true, |_| panic!("recording"))
            .is_err());
        assert!(selected
            .change_bitrate(PlaybackMode::Aac, manual, false, |_| Err(io::Error::other(
                "disk full"
            )))
            .is_err());
        assert_eq!(
            selected.bitrate(PlaybackMode::Aac),
            PlaybackBitrate::default()
        );
        assert!(!cancel.load(Ordering::Acquire));
        assert!(selected
            .change_bitrate(PlaybackMode::Aac, manual, false, |_| Ok(()))
            .unwrap());
        assert_eq!(selected.revision, 1);
        assert!(cancel.load(Ordering::Acquire));
        assert!(!selected
            .change_bitrate(PlaybackMode::Aac, manual, false, |_| panic!("unchanged"))
            .unwrap());
        for (mode, kbps) in [
            (PlaybackMode::Windows, 256),
            (PlaybackMode::Aac, 95),
            (PlaybackMode::Aac, 513),
            (PlaybackMode::AacEld, 127),
            (PlaybackMode::AacEld, 257),
        ] {
            assert!(validate_bitrate(
                mode,
                PlaybackBitrate {
                    best_effort: true,
                    kbps
                }
            )
            .is_err());
        }
    }

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
            .change(PlaybackMode::Windows, false, false, |_| panic!(
                "no driver rebinding"
            ))
            .is_err());
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
        assert!(selected.should_attempt(None, false, &state, true, false));
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
        assert!(selected
            .change(PlaybackMode::Windows, true, false, |_| panic!(
                "no driver rebinding"
            ))
            .is_err());
        assert_eq!(selected.mode, PlaybackMode::Aac);
    }
}
