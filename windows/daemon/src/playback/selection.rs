use ntpods_ipc::{PlaybackBitrate, PlaybackLinkSettings, PlaybackMode, Snapshot};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct Selection {
    pub mode: PlaybackMode,
    pub revision: u64,
    pub cancel: Option<Arc<AtomicBool>>,
    pub bitrates: [PlaybackBitrate; 2],
    pub link: PlaybackLinkSettings,
    pub game_mode: bool,
}

impl Selection {
    pub fn new(mode: PlaybackMode) -> Self {
        Self {
            mode,
            revision: 0,
            cancel: None,
            bitrates: [PlaybackBitrate::default(); 2],
            link: PlaybackLinkSettings::default(),
            game_mode: true,
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
        explicit_connection: bool,
    ) -> bool {
        explicit_connection && state.connected && !state.link_stale
            && !stopped && attempted != Some(self.revision)
            // A command reserves the transition before a new capture client can start.
            && (matches!(state.playback_state.as_str(), "starting" | "stopping")
                || (!mic_on && !state.mic_uplink_active
                    && !matches!(state.mic_state.as_str(), "starting" | "capturing" | "stopping" | "recovering")))
    }

    pub fn uses_bitrate(&self, mode: PlaybackMode, active: Option<PlaybackMode>) -> bool {
        mode == self.mode
            || active == Some(mode)
            || (active.is_none() && self.mode == PlaybackMode::AacEld && mode == PlaybackMode::Aac)
    }

    pub fn change_bitrate(
        &mut self,
        mode: PlaybackMode,
        bitrate: PlaybackBitrate,
        active: Option<PlaybackMode>,
        busy: bool,
        persist: impl FnOnce([PlaybackBitrate; 2]) -> io::Result<()>,
    ) -> io::Result<bool> {
        validate_bitrate(mode, bitrate)?;
        let index = usize::from(mode == PlaybackMode::AacEld);
        if self.bitrates[index] == bitrate {
            return Ok(false);
        }
        let affects_playback = self.uses_bitrate(mode, active);
        if busy && affects_playback {
            return Err(io::Error::other(
                "Stop the microphone before changing playback.",
            ));
        }
        let mut next = self.bitrates;
        next[index] = bitrate;
        persist(next)?;
        self.bitrates = next;
        if affects_playback {
            self.stop();
            self.revision = self.revision.wrapping_add(1);
        }
        Ok(true)
    }

    pub fn uses_link(&self, active: Option<PlaybackMode>) -> bool {
        active.unwrap_or(self.mode) == PlaybackMode::AacEld && self.bitrates[1].variable
    }

    pub fn change_game_mode(
        &mut self,
        enabled: bool,
        busy: bool,
        persist: impl FnOnce(bool) -> io::Result<()>,
    ) -> io::Result<bool> {
        if self.game_mode == enabled {
            return Ok(false);
        }
        if busy {
            return Err(io::Error::other(
                "Stop the microphone before changing playback.",
            ));
        }
        persist(enabled)?;
        // The stream owns the buffer transition; keep its existing PCM and session.
        self.game_mode = enabled;
        Ok(true)
    }

    pub fn change_link(
        &mut self,
        settings: PlaybackLinkSettings,
        active: Option<PlaybackMode>,
        busy: bool,
        persist: impl FnOnce(PlaybackLinkSettings) -> io::Result<()>,
    ) -> io::Result<bool> {
        if !settings.is_valid() {
            return Err(io::Error::other(
                "Frame limit must be between 128 and 256 kbps.",
            ));
        }
        if settings == self.link {
            return Ok(false);
        }
        let affects_playback = self.uses_link(active);
        if busy && affects_playback {
            return Err(io::Error::other(
                "Stop the microphone before changing playback.",
            ));
        }
        persist(settings)?;
        self.link = settings;
        if affects_playback {
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
    if bitrate.variable
        && (mode != PlaybackMode::AacEld || !bitrate.best_effort || bitrate.kbps != 256)
    {
        return Err(io::Error::other(
            "Variable bitrate requires the AAC-ELD profile.",
        ));
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
        PlaybackMode::Windows => PlaybackMode::AacEld,
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
    crate::persist_preference("playback-bitrates.json", &serde_json::to_vec(&values)?)
}

pub fn load_link() -> PlaybackLinkSettings {
    path()
        .ok()
        .and_then(|path| std::fs::read(path.with_file_name("playback-link.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<PlaybackLinkSettings>(&bytes).ok())
        .filter(|settings| settings.is_valid())
        .unwrap_or_default()
}

pub fn save_link(settings: PlaybackLinkSettings) -> io::Result<()> {
    crate::persist_preference("playback-link.json", &serde_json::to_vec(&settings)?)
}

pub fn load_game_mode() -> bool {
    path()
        .ok()
        .and_then(|path| std::fs::read(path.with_file_name("playback-game-mode.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<bool>(&bytes).ok())
        .unwrap_or(true)
}

pub fn save_game_mode(enabled: bool) -> io::Result<()> {
    crate::persist_preference("playback-game-mode.json", &serde_json::to_vec(&enabled)?)
}

pub fn save(mode: PlaybackMode) -> io::Result<()> {
    crate::persist_preference("playback-mode.json", &serde_json::to_vec(&mode)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn game_mode_persists_before_publishing_without_replacing_the_stream() {
        let mut selected = Selection::new(PlaybackMode::AacEld);
        let cancel = Arc::new(AtomicBool::new(false));
        selected.cancel = Some(cancel.clone());
        assert!(selected.game_mode);
        assert!(selected
            .change_game_mode(false, true, |_| panic!("busy"))
            .is_err());
        assert!(selected
            .change_game_mode(false, false, |_| Err(io::Error::other("disk full")))
            .is_err());
        assert!(selected.game_mode);
        assert!(selected
            .change_game_mode(true, true, |_| panic!("no change"))
            .is_ok_and(|changed| !changed));
        assert!(selected
            .change_game_mode(false, false, |enabled| {
                assert!(!enabled);
                Ok(())
            })
            .unwrap());
        assert!(!selected.game_mode);
        assert_eq!(selected.revision, 0);
        assert!(!cancel.load(Ordering::Acquire));
        assert!(selected.change_game_mode(true, false, |_| Ok(())).unwrap());
        assert!(selected.game_mode);
    }

    #[test]
    fn link_preferences_are_bounded_and_persist_before_cancellation() {
        let mut selected = Selection::new(PlaybackMode::AacEld);
        selected.bitrates[1].variable = true;
        let cancel = Arc::new(AtomicBool::new(false));
        selected.cancel = Some(cancel.clone());
        let manual = PlaybackLinkSettings {
            mode: ntpods_ipc::PlaybackLinkMode::Manual,
            kbps: 174,
        };
        for invalid in [127, 257, u16::MAX] {
            assert!(selected
                .change_link(
                    PlaybackLinkSettings {
                        kbps: invalid,
                        ..manual
                    },
                    None,
                    false,
                    |_| panic!("invalid value persisted")
                )
                .is_err());
        }
        assert!(selected
            .change_link(manual, None, true, |_| panic!("busy stream changed"))
            .is_err());
        assert!(selected
            .change_link(manual, None, false, |_| Err(io::Error::other("disk full")))
            .is_err());
        assert_eq!(selected.link, PlaybackLinkSettings::default());
        assert_eq!(selected.revision, 0);
        assert!(!cancel.load(Ordering::Acquire));
        assert!(selected
            .change_link(manual, None, false, |value| {
                assert_eq!(value, manual);
                Ok(())
            })
            .unwrap());
        assert_eq!(selected.link, manual);
        assert_eq!(selected.revision, 1);
        assert!(cancel.load(Ordering::Acquire));
        assert!(!selected
            .change_link(manual, None, true, |_| panic!(
                "unchanged preference written"
            ))
            .unwrap());
    }

    #[test]
    fn dormant_link_choices_do_not_restart_aac_or_fixed_bitrate() {
        for (mode, active, variable) in [
            (PlaybackMode::Aac, Some(PlaybackMode::Aac), false),
            (PlaybackMode::AacEld, Some(PlaybackMode::Aac), true),
            (PlaybackMode::AacEld, Some(PlaybackMode::AacEld), false),
        ] {
            let mut selected = Selection::new(mode);
            selected.bitrates[1].variable = variable;
            let cancel = Arc::new(AtomicBool::new(false));
            selected.cancel = Some(cancel.clone());
            let value = PlaybackLinkSettings {
                mode: ntpods_ipc::PlaybackLinkMode::Automatic,
                ..PlaybackLinkSettings::default()
            };
            assert!(selected
                .change_link(value, active, true, |_| Ok(()))
                .unwrap());
            assert_eq!(selected.link, value);
            assert_eq!(selected.revision, 0);
            assert!(!cancel.load(Ordering::Acquire));
        }
    }

    #[test]
    fn variable_profile_is_opt_in_and_restarts_only_eld() {
        let variable = PlaybackBitrate {
            variable: true,
            ..PlaybackBitrate::default()
        };
        assert!(!PlaybackBitrate::default().variable);
        assert!(validate_bitrate(PlaybackMode::AacEld, variable).is_ok());
        assert!(validate_bitrate(PlaybackMode::Aac, variable).is_err());
        assert!(validate_bitrate(
            PlaybackMode::AacEld,
            PlaybackBitrate {
                best_effort: false,
                ..variable
            }
        )
        .is_err());
        assert!(validate_bitrate(
            PlaybackMode::AacEld,
            PlaybackBitrate {
                kbps: 192,
                ..variable
            }
        )
        .is_err());

        let mut selection = Selection::new(PlaybackMode::AacEld);
        let cancel = Arc::new(AtomicBool::new(false));
        selection.cancel = Some(cancel.clone());
        assert!(selection
            .change_bitrate(PlaybackMode::AacEld, variable, None, false, |values| {
                assert_eq!(values, [PlaybackBitrate::default(), variable]);
                Ok(())
            })
            .unwrap());
        assert_eq!(selection.revision, 1);
        assert!(cancel.load(Ordering::Acquire));
    }

    #[test]
    fn new_audio_intent_allows_reconnect_but_same_failure_requires_retry() {
        assert_eq!(attempted_revision(Some((4, 8)), 8), Some(4));
        assert_eq!(attempted_revision(Some((4, 8)), 9), None);
        assert_eq!(attempted_revision(None, 8), None);
    }

    #[test]
    fn legacy_and_missing_selections_use_eld_without_driver_rebinding() {
        assert_eq!(
            normalize_mode(PlaybackMode::default()),
            PlaybackMode::AacEld
        );
        assert_eq!(normalize_mode(PlaybackMode::Windows), PlaybackMode::AacEld);
    }

    #[test]
    fn saved_codec_preferences_are_preserved() {
        for (saved, expected) in [
            (r#""aac""#, PlaybackMode::Aac),
            (r#""aac_eld""#, PlaybackMode::AacEld),
        ] {
            assert_eq!(
                normalize_mode(serde_json::from_str(saved).unwrap()),
                expected
            );
        }
    }

    #[test]
    fn bitrate_preferences_are_separate_and_preserve_active_recording() {
        let mut selected = Selection::new(PlaybackMode::Aac);
        let cancel = Arc::new(AtomicBool::new(false));
        selected.cancel = Some(cancel.clone());
        let manual = PlaybackBitrate {
            variable: false,
            best_effort: false,
            kbps: 128,
        };
        assert!(selected
            .change_bitrate(PlaybackMode::AacEld, manual, None, true, |values| {
                assert_eq!(values[0], PlaybackBitrate::default());
                assert_eq!(values[1], manual);
                Ok(())
            })
            .unwrap());
        assert_eq!(selected.revision, 0);
        assert!(!cancel.load(Ordering::Acquire));
        assert!(selected
            .change_bitrate(PlaybackMode::Aac, manual, None, true, |_| panic!(
                "recording"
            ))
            .is_err());
        assert!(selected
            .change_bitrate(PlaybackMode::Aac, manual, None, false, |_| Err(
                io::Error::other("disk full")
            ))
            .is_err());
        assert_eq!(selected.bitrates[0], PlaybackBitrate::default());
        assert!(!cancel.load(Ordering::Acquire));
        assert!(selected
            .change_bitrate(PlaybackMode::Aac, manual, None, false, |_| Ok(()))
            .unwrap());
        assert_eq!(selected.revision, 1);
        assert!(cancel.load(Ordering::Acquire));
        assert!(!selected
            .change_bitrate(PlaybackMode::Aac, manual, None, false, |_| panic!(
                "unchanged"
            ))
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
                    variable: false,
                    best_effort: true,
                    kbps
                }
            )
            .is_err());
        }
    }

    #[test]
    fn fallback_bitrate_edits_preserve_recording_and_invalidate_pending_preferences() {
        let manual = PlaybackBitrate {
            variable: false,
            best_effort: false,
            kbps: 192,
        };
        for active in [None, Some(PlaybackMode::Aac)] {
            let mut selected = Selection::new(PlaybackMode::AacEld);
            let cancel = Arc::new(AtomicBool::new(false));
            selected.cancel = Some(cancel.clone());
            assert!(selected
                .change_bitrate(PlaybackMode::Aac, manual, active, true, |_| panic!(
                    "recording"
                ))
                .is_err());
            assert_eq!(selected.bitrates[0], PlaybackBitrate::default());
            assert_eq!(selected.revision, 0);
            assert!(!cancel.load(Ordering::Acquire));
            assert!(selected
                .change_bitrate(PlaybackMode::Aac, manual, active, false, |_| Ok(()))
                .unwrap());
            assert_eq!(selected.mode, PlaybackMode::AacEld);
            assert_eq!(selected.revision, 1);
            assert!(cancel.load(Ordering::Acquire));
        }
        let mut selected = Selection::new(PlaybackMode::AacEld);
        assert!(selected
            .change_bitrate(
                PlaybackMode::Aac,
                manual,
                Some(PlaybackMode::AacEld),
                true,
                |_| Ok(())
            )
            .unwrap());
        assert_eq!(selected.revision, 0);
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
    fn control_only_acl_cannot_start_avdtp_or_reuse_completed_audio_intent() {
        use crate::connection_request::ConnectionRequest;
        let selected = Selection::new(PlaybackMode::AacEld);
        let requested = ConnectionRequest::new(false);
        let state = Snapshot {
            connected: true,
            playback_native_driver: true,
            playback_mode: PlaybackMode::AacEld,
            playback_state: "disconnected".into(),
            ..Default::default()
        };
        assert!(requested.claim());
        for capture in [false, true] {
            assert!(!selected.should_attempt(
                None,
                false,
                &state,
                capture,
                false,
                requested.explicit()
            ));
        }
        requested.store(true, Ordering::Release);
        let completed = requested.snapshot();
        assert!(selected.should_attempt(None, false, &state, false, false, requested.explicit()));
        assert!(requested.release(completed));
        assert!(!selected.should_attempt(None, false, &state, true, false, requested.explicit()));
        assert!(requested.claim());
        assert!(!selected.should_attempt(None, false, &state, false, false, requested.explicit()));
        requested.store(true, Ordering::Release);
        assert!(!requested.release(completed));
        assert!(selected.should_attempt(None, false, &state, false, false, requested.explicit()));
    }

    #[test]
    fn playback_waits_for_fresh_control_handshake_after_connect_or_repair() {
        let selected = Selection::new(PlaybackMode::AacEld);
        let mut state = Snapshot {
            playback_state: "starting".into(),
            ..Default::default()
        };
        assert!(!selected.should_attempt(None, false, &state, false, false, true));
        state.connected = true;
        assert!(selected.should_attempt(None, false, &state, false, false, true));
        state.link_stale = true;
        assert!(!selected.should_attempt(None, false, &state, false, false, true));
        state.connected = false;
        state.link_stale = false;
        assert!(!selected.should_attempt(None, false, &state, false, false, true));
        state.connected = true;
        assert!(selected.should_attempt(None, false, &state, false, false, true));
    }

    #[test]
    fn failure_waits_for_retry_and_saved_choices_wait_for_existing_capture() {
        let mut selected = Selection::new(PlaybackMode::Aac);
        let mut state = Snapshot {
            connected: true,
            playback_mode: PlaybackMode::Aac,
            playback_state: "disconnected".into(),
            ..Default::default()
        };
        assert!(selected.should_attempt(None, false, &state, true, false, true));
        assert!(!selected.should_attempt(None, false, &state, false, true, true));
        assert!(selected.should_attempt(None, false, &state, false, false, true));
        let attempted = Some(selected.revision);
        state.playback_state = "error".into();
        assert!(!selected.should_attempt(attempted, false, &state, false, false, true));
        selected
            .change(PlaybackMode::Aac, true, false, |_| Ok(()))
            .unwrap();
        state.playback_state = "starting".into();
        assert!(selected.should_attempt(attempted, false, &state, true, false, true));
        state.playback_state = "stopping".into();
        assert!(selected.should_attempt(attempted, false, &state, true, false, true));
        assert!(!selected.should_attempt(attempted, true, &state, false, false, true));
        assert!(selected
            .change(PlaybackMode::Windows, true, false, |_| panic!(
                "no driver rebinding"
            ))
            .is_err());
        assert_eq!(selected.mode, PlaybackMode::Aac);
    }
}
