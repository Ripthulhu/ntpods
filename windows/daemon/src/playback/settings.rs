use std::{env, io, time::Duration};

pub(super) struct Settings {
    pub eld_aus: usize,
    pub reserve: Duration,
}

impl Settings {
    pub fn load() -> io::Result<Self> {
        let units = read("NTPODS_ELD_AUS_PER_PACKET")?;
        let reserve = read("NTPODS_PLAYBACK_RESERVE_MS")?;
        Self::parse(units.as_deref(), reserve.as_deref())
    }

    fn parse(units: Option<&str>, reserve: Option<&str>) -> io::Result<Self> {
        let eld_aus = match units {
            None | Some("1") => 1,
            Some("2") => 2,
            _ => return Err(invalid("NTPODS_ELD_AUS_PER_PACKET must be 1 or 2")),
        };
        let reserve = match reserve {
            None | Some("30") => Duration::from_millis(30),
            Some("50") => Duration::from_millis(50),
            _ => return Err(invalid("NTPODS_PLAYBACK_RESERVE_MS must be 30 or 50")),
        };
        Ok(Self { eld_aus, reserve })
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
        for (units, reserve, expected_units, expected_ms) in [
            (None, None, 1, 30),
            (Some("1"), None, 1, 30),
            (Some("2"), None, 2, 30),
            (None, Some("30"), 1, 30),
            (None, Some("50"), 1, 50),
            (Some("1"), Some("30"), 1, 30),
            (Some("1"), Some("50"), 1, 50),
            (Some("2"), Some("30"), 2, 30),
            (Some("2"), Some("50"), 2, 50),
        ] {
            let settings = Settings::parse(units, reserve).unwrap();
            assert_eq!(settings.eld_aus, expected_units);
            assert_eq!(settings.reserve, Duration::from_millis(expected_ms));
        }
    }

    #[test]
    fn invalid_values_do_not_silently_select_another_profile() {
        for units in ["", "0", "3", "01", "-1", "1 ", "1.0"] {
            assert_eq!(
                Settings::parse(Some(units), None)
                    .err()
                    .unwrap()
                    .kind(),
                io::ErrorKind::InvalidInput
            );
        }
        for reserve in ["", "0", "10", "40", "500", "30 ", "30.0"] {
            assert_eq!(
                Settings::parse(None, Some(reserve))
                    .err()
                    .unwrap()
                    .kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }
}
