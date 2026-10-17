use super::codec::{EldRtpClock, Mode};
use std::{env, io, time::Duration};

pub(super) struct Settings {
    pub eld_aus: usize,
    pub eld_rtp_clock: EldRtpClock,
    pub eld_rtp_ssrc: u32,
    reserve: Option<Duration>,
}

impl Settings {
    pub fn load() -> io::Result<Self> {
        let units = read("NTPODS_ELD_AUS_PER_PACKET")?;
        let reserve = read("NTPODS_PLAYBACK_RESERVE_MS")?;
        let clock = read("NTPODS_ELD_RTP_CLOCK")?;
        let ssrc = read("NTPODS_ELD_RTP_SSRC")?;
        Self::parse(
            units.as_deref(),
            reserve.as_deref(),
            clock.as_deref(),
            ssrc.as_deref(),
        )
    }

    pub fn reserve(&self, mode: Mode) -> io::Result<Duration> {
        match (self.reserve, mode) {
            (Some(reserve), Mode::Aac) if reserve < Duration::from_millis(30) => Err(invalid(
                "NTPODS_PLAYBACK_RESERVE_MS=20 requires AAC-ELD; use 30 or 50 for AAC",
            )),
            (Some(reserve), _) => Ok(reserve),
            (None, Mode::Aac) => Ok(Duration::from_millis(30)),
            (None, Mode::Eld) => Ok(Duration::from_millis(20)),
        }
    }

    fn parse(
        units: Option<&str>,
        reserve: Option<&str>,
        clock: Option<&str>,
        ssrc: Option<&str>,
    ) -> io::Result<Self> {
        let eld_aus = match units {
            None | Some("1") => 1,
            Some("2") => 2,
            _ => return Err(invalid("NTPODS_ELD_AUS_PER_PACKET must be 1 or 2")),
        };
        let reserve = match reserve {
            None => None,
            Some("20") => Some(Duration::from_millis(20)),
            Some("30") => Some(Duration::from_millis(30)),
            Some("50") => Some(Duration::from_millis(50)),
            _ => return Err(invalid("NTPODS_PLAYBACK_RESERVE_MS must be 20, 30 or 50")),
        };
        let eld_rtp_clock = match clock {
            None | Some("48000") => EldRtpClock::Samples48000,
            Some("monotonic") => EldRtpClock::Monotonic,
            Some("1000") => EldRtpClock::Samples1000,
            _ => {
                return Err(invalid(
                    "NTPODS_ELD_RTP_CLOCK must be monotonic, 48000 or 1000",
                ))
            }
        };
        let eld_rtp_ssrc = match ssrc {
            None if eld_rtp_clock == EldRtpClock::Monotonic => 0,
            None | Some("0x11223344") => 0x11223344,
            Some("0") => 0,
            _ => return Err(invalid("NTPODS_ELD_RTP_SSRC must be 0 or 0x11223344")),
        };
        Ok(Self {
            eld_aus,
            eld_rtp_clock,
            eld_rtp_ssrc,
            reserve,
        })
    }
}

fn read(name: &str) -> io::Result<Option<String>> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(env::VarError::NotUnicode(_)) => Err(invalid("Invalid playback settings environment")),
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_each_override_are_independent() {
        for (units, expected_units) in [(None, 1), (Some("1"), 1), (Some("2"), 2)] {
            for (reserve, aac_ms, eld_ms) in
                [(None, 30, 20), (Some("30"), 30, 30), (Some("50"), 50, 50)]
            {
                for (clock, expected_clock) in [
                    (None, EldRtpClock::Samples48000),
                    (Some("monotonic"), EldRtpClock::Monotonic),
                    (Some("48000"), EldRtpClock::Samples48000),
                    (Some("1000"), EldRtpClock::Samples1000),
                ] {
                    let settings = Settings::parse(units, reserve, clock, None).unwrap();
                    assert_eq!(settings.eld_aus, expected_units);
                    assert_eq!(settings.eld_rtp_clock, expected_clock);
                    assert_eq!(
                        settings.eld_rtp_ssrc,
                        if expected_clock == EldRtpClock::Monotonic {
                            0
                        } else {
                            0x11223344
                        }
                    );
                    assert_eq!(
                        settings.reserve(Mode::Eld).unwrap(),
                        Duration::from_millis(eld_ms)
                    );
                    assert_eq!(
                        settings.reserve(Mode::Aac).unwrap(),
                        Duration::from_millis(aac_ms)
                    );
                }
            }
        }
    }

    #[test]
    fn twenty_milliseconds_requires_the_negotiated_eld_codec() {
        for units in [None, Some("1"), Some("2")] {
            let settings = Settings::parse(units, Some("20"), None, None).unwrap();
            assert_eq!(
                settings.reserve(Mode::Eld).unwrap(),
                Duration::from_millis(20)
            );
            let error = settings.reserve(Mode::Aac).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
            assert!(error.to_string().contains("requires AAC-ELD"));
        }
    }

    #[test]
    fn invalid_values_do_not_silently_select_another_profile() {
        for units in ["", "0", "3", "01", "-1", "1 ", "1.0"] {
            assert_eq!(
                Settings::parse(Some(units), None, None, None)
                    .err()
                    .unwrap()
                    .kind(),
                io::ErrorKind::InvalidInput
            );
        }
        for reserve in ["", "0", "10", "40", "500", "30 ", "30.0"] {
            assert_eq!(
                Settings::parse(None, Some(reserve), None, None)
                    .err()
                    .unwrap()
                    .kind(),
                io::ErrorKind::InvalidInput
            );
        }
        for clock in ["", "0", "90000", "048000", "1", "1000 ", "1000.0"] {
            assert_eq!(
                Settings::parse(None, None, Some(clock), None)
                    .err()
                    .unwrap()
                    .kind(),
                io::ErrorKind::InvalidInput
            );
        }
        for ssrc in [
            "",
            "00",
            "0x0",
            "11223344",
            "0x11223344 ",
            "0xffffffff",
            "-1",
        ] {
            assert_eq!(
                Settings::parse(None, None, None, Some(ssrc))
                    .err()
                    .unwrap()
                    .kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }

    #[test]
    fn explicit_source_identifier_is_independent_of_clock() {
        for clock in [None, Some("monotonic"), Some("48000"), Some("1000")] {
            for (ssrc, expected) in [("0", 0), ("0x11223344", 0x11223344)] {
                let settings = Settings::parse(None, None, clock, Some(ssrc)).unwrap();
                assert_eq!(settings.eld_rtp_ssrc, expected);
            }
        }
    }
}
