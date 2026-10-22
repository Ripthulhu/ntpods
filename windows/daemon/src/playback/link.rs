use super::codec::Mode;
use ntpods_ipc::{PlaybackLinkMode, PlaybackLinkSettings};
use std::io;

pub(super) struct LinkBudget {
    automatic: bool,
    current: Option<u32>,
    unavailable_reported: bool,
}

impl LinkBudget {
    pub fn for_settings(settings: PlaybackLinkSettings, mode: Mode, variable: bool) -> Self {
        let eligible = mode == Mode::Eld && variable;
        Self::new(
            eligible && settings.mode == PlaybackLinkMode::Automatic,
            (eligible && settings.mode == PlaybackLinkMode::Manual)
                .then_some(u32::from(settings.kbps)),
        )
    }

    pub fn new(automatic: bool, explicit: Option<u32>) -> Self {
        Self {
            automatic,
            current: explicit,
            unavailable_reported: false,
        }
    }

    pub fn current(&self) -> Option<u32> {
        self.current
    }

    pub fn refresh(
        &mut self,
        mode: Mode,
        bitrate: u32,
        variable: bool,
        read: impl FnOnce() -> io::Result<bool>,
    ) -> Option<u32> {
        if !self.automatic {
            return self.current;
        }
        if mode != Mode::Eld || !variable || bitrate != 256_000 {
            self.current = None;
            return None;
        }
        let active_24ghz = match read() {
            Ok(active) => {
                self.unavailable_reported = false;
                active
            }
            Err(error) => {
                if !self.unavailable_reported {
                    crate::log(&format!(
                        "playback: Wi-Fi band unavailable; retaining encoder budget: {error}"
                    ));
                    self.unavailable_reported = true;
                }
                return self.current;
            }
        };
        let next = active_24ghz.then_some(174);
        if next != self.current {
            crate::log(if active_24ghz {
                "playback: 2.4 GHz Wi-Fi active; selecting 174 kbps AAC-ELD frame budget"
            } else {
                "playback: 2.4 GHz Wi-Fi inactive; restoring normal AAC-ELD Variable mode"
            });
        }
        self.current = next;
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_policies_only_apply_to_variable_eld_and_off_restores_the_budget() {
        for policy in [
            PlaybackLinkMode::Off,
            PlaybackLinkMode::Automatic,
            PlaybackLinkMode::Manual,
        ] {
            let settings = PlaybackLinkSettings {
                mode: policy,
                kbps: 174,
            };
            for (mode, variable) in [(Mode::Aac, false), (Mode::Aac, true), (Mode::Eld, false)] {
                let mut link = LinkBudget::for_settings(settings, mode, variable);
                assert_eq!(
                    link.refresh(mode, 256_000, variable, || panic!(
                        "ineligible stream queried Wi-Fi"
                    )),
                    None
                );
            }
            let mut link = LinkBudget::for_settings(settings, Mode::Eld, true);
            assert_eq!(
                link.refresh(Mode::Eld, 256_000, true, || Ok(true)),
                if policy == PlaybackLinkMode::Off {
                    None
                } else {
                    Some(174)
                }
            );
            assert_eq!(
                link.refresh(Mode::Eld, 256_000, true, || Ok(false)),
                if policy == PlaybackLinkMode::Manual {
                    Some(174)
                } else {
                    None
                }
            );
        }
    }

    #[test]
    fn opt_in_follows_verified_wifi_and_restores_without_rewriting_preferences() {
        let mut link = LinkBudget::new(true, None);
        assert_eq!(link.current(), None);
        for (active, budget) in [
            (false, None),
            (true, Some(174)),
            (true, Some(174)),
            (false, None),
        ] {
            assert_eq!(
                link.refresh(Mode::Eld, 256_000, true, || Ok(active)),
                budget
            );
            assert_eq!(link.current(), budget);
        }
    }

    #[test]
    fn defaults_explicit_budgets_and_fixed_bitrates_never_query_wifi() {
        for explicit in [None, Some(128), Some(174), Some(256)] {
            let mut link = LinkBudget::new(false, explicit);
            assert_eq!(
                link.refresh(Mode::Eld, 256_000, true, || panic!("disabled")),
                explicit
            );
        }
        let mut link = LinkBudget::new(true, None);
        for (mode, bitrate, variable) in [
            (Mode::Aac, 256_000, false),
            (Mode::Aac, 256_000, true),
            (Mode::Eld, 256_000, false),
            (Mode::Eld, 192_000, false),
            (Mode::Eld, 192_000, true),
        ] {
            assert_eq!(
                link.refresh(mode, bitrate, variable, || panic!("ineligible")),
                None
            );
        }
    }

    #[test]
    fn unavailable_or_invalid_band_never_raises_a_reduced_budget() {
        let mut link = LinkBudget::new(true, None);
        for kind in [
            io::ErrorKind::Unsupported,
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::InvalidData,
        ] {
            assert_eq!(
                link.refresh(Mode::Eld, 256_000, true, || Err(kind.into())),
                None
            );
        }
        assert_eq!(
            link.refresh(Mode::Eld, 256_000, true, || Ok(true)),
            Some(174)
        );
        assert_eq!(
            link.refresh(Mode::Eld, 256_000, true, || Err(
                io::ErrorKind::TimedOut.into()
            )),
            Some(174)
        );
        assert_eq!(link.refresh(Mode::Eld, 256_000, true, || Ok(false)), None);
    }

    #[test]
    fn switching_out_of_variable_mode_cannot_retain_an_automatic_override() {
        let mut link = LinkBudget::new(true, None);
        link.refresh(Mode::Eld, 256_000, true, || Ok(true));
        assert_eq!(
            link.refresh(Mode::Eld, 256_000, false, || panic!("manual bitrate")),
            None
        );
        assert_eq!(link.current(), None);
    }
}
