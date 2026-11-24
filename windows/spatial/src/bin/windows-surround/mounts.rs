use super::super::{
    Result,
    pose::{Calibration, CalibrationSample},
};
use super::{Lease, NO_PAIRED_MOTION, PrimaryBud, open_collection};
use hidapi::HidDevice;
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Pair {
    address: [u8; 6],
    session: u32,
}

fn parse_pair(r: &[u8]) -> Option<Pair> {
    if r.len() != 64 || r[..7] != *b"\x02APDG\x01\x40" || r[7] & 3 != 3 {
        return None;
    }
    let address: [u8; 6] = r[12..18].try_into().ok()?;
    let session = u32::from_le_bytes(r[8..12].try_into().ok()?);
    (session != 0 && address != [0; 6] && address != [255; 6]).then_some(Pair { address, session })
}

fn read_pair(control: &HidDevice) -> hidapi::HidResult<Option<Pair>> {
    let mut r = [0u8; 64];
    r[0] = 2;
    let n = control.get_feature_report(&mut r)?;
    Ok(r.get(..n).and_then(parse_pair))
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Source {
    role: [u8; 4],
    generation: u32,
}

fn source(r: &[u8]) -> Option<Source> {
    let (role, generation) = CalibrationSample::source(r)?;
    matches!(role[0], 1 | 2).then_some(Source { role, generation })
}

fn directory() -> Option<PathBuf> {
    Some(
        PathBuf::from(std::env::var_os("LOCALAPPDATA")?)
            .join("AirPods Spatial Audio")
            .join("mounts-v1"),
    )
}

fn key(pair: Pair, role: [u8; 4]) -> String {
    pair.address
        .into_iter()
        .chain(role)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn decode(text: &str, pair: Pair, role: [u8; 4]) -> Option<Calibration> {
    let mut fields = text.split_whitespace();
    if fields.next() != Some("mount-v1") || fields.next()? != key(pair, role) {
        return None;
    }
    let mut rows = [[0.; 3]; 3];
    for n in rows.iter_mut().flatten() {
        *n = fields.next()?.parse().ok()?;
    }
    if fields.next().is_some() {
        return None;
    }
    Calibration::from_rows(role, rows)
}

fn load(root: &Path, pair: Pair, role: [u8; 4]) -> Option<Calibration> {
    let mut text = String::new();
    File::open(root.join(key(pair, role)))
        .ok()?
        .take(1025)
        .read_to_string(&mut text)
        .ok()?;
    (text.len() <= 1024)
        .then(|| decode(&text, pair, role))
        .flatten()
}

fn save(root: &Path, pair: Pair, calibration: &Calibration) -> Result<()> {
    fs::create_dir_all(root)?;
    let id = key(pair, calibration.role);
    let target = root.join(&id);
    let temp = target.with_extension(format!("{}.tmp", std::process::id()));
    let mut file = File::options()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temp)?;
    let write = (|| -> Result<()> {
        writeln!(file, "mount-v1 {id}")?;
        for [x, y, z] in calibration.rows() {
            writeln!(file, "{x} {y} {z}")?;
        }
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, &target)?;
        Ok(())
    })();
    if write.is_err() {
        let _ = fs::remove_file(temp);
    }
    write
}

pub(super) struct Mounts {
    control: Option<HidDevice>,
    retry_control: Instant,
    root: Option<PathBuf>,
    binding: Binding,
}

pub(super) struct BoundMount<'a> {
    pub(super) role: [u8; 4],
    pub(super) profile: Option<&'a Calibration>,
}

#[derive(Default)]
struct Binding {
    scope: Option<Source>,
    observed: Option<Pair>,
    checked: Option<Instant>,
    confirmed: bool,
    profile: Option<Calibration>,
}

impl Mounts {
    pub(super) fn new() -> Self {
        Self {
            control: None,
            retry_control: Instant::now(),
            root: directory(),
            binding: Binding::default(),
        }
    }

    pub(super) fn for_report(
        &mut self,
        serial: &str,
        report: &[u8],
        now: Instant,
    ) -> Option<BoundMount<'_>> {
        let root = self.root.as_deref()?;
        self.binding.for_source(
            source(report),
            now,
            || {
                if self.control.is_none() && now >= self.retry_control {
                    self.retry_control = now + Duration::from_secs(1);
                    self.control = open_collection(serial, 1).ok();
                }
                match read_pair(self.control.as_ref()?) {
                    Ok(pair) => pair,
                    Err(_) => {
                        self.control = None;
                        self.retry_control = now + Duration::from_secs(1);
                        None
                    }
                }
            },
            |pair, role| load(root, pair, role),
        )
    }
}

impl Binding {
    // Motion and control generations are independent. Bracket fresh motion with
    // matching peer/session reads before applying a saved mounting.
    fn for_source(
        &mut self,
        src: Option<Source>,
        now: Instant,
        read: impl FnOnce() -> Option<Pair>,
        lookup: impl FnOnce(Pair, [u8; 4]) -> Option<Calibration>,
    ) -> Option<BoundMount<'_>> {
        if self.scope != src {
            *self = Self {
                scope: src,
                ..Self::default()
            };
        }
        let src = src?;
        let period = if self.confirmed || self.observed.is_none() {
            Duration::from_secs(1)
        } else {
            Duration::from_millis(200)
        };
        if self
            .checked
            .is_none_or(|last| now.duration_since(last) >= period)
        {
            let current = read();
            match current {
                Some(pair) if Some(pair) == self.observed => {
                    if self.profile.is_none() {
                        self.profile = lookup(pair, src.role);
                    }
                    self.confirmed = true;
                }
                _ => {
                    self.profile = None;
                    self.confirmed = false;
                }
            }
            self.observed = current;
            self.checked = Some(now);
        }
        self.confirmed.then(|| BoundMount {
            role: src.role,
            profile: self.profile.as_ref(),
        })
    }
}

pub(super) fn calibrate(
    serial: &str,
    prompt: impl Fn(u8, u64),
    cancel: impl Fn() -> bool,
) -> Result<Option<PrimaryBud>> {
    let root = directory().ok_or("Windows local application data is unavailable")?;
    let control = open_collection(serial, 1)?;
    println!(
        "Close the head-tracking player first. Wear the AirPods and face the center of your screen. Follow three short prompts without turning sideways."
    );
    println!(
        "Waiting for motion. If needed, press the dongle button once for audio ownership, or play briefly to its headphone output."
    );
    let mut lease = Lease::open(serial)?;
    let mut report = [0; 64];
    let deadline = Instant::now() + Duration::from_secs(18);
    let (pair, origin) = loop {
        if cancel() {
            return Ok(None);
        }
        lease.renew()?;
        let n = lease.device.read_timeout(&mut report, 50)?;
        if let Some(origin) = source(&report[..n])
            && let Ok(Some(pair)) = read_pair(&control)
        {
            break (pair, origin);
        }
        if Instant::now() >= deadline {
            return Err(NO_PAIRED_MOTION.into());
        }
    };
    let mut phases = [Vec::new(), Vec::new(), Vec::new()];
    let mut last = Instant::now();
    let mut accepted = None;
    for (index, (instruction, seconds)) in [
        ("Face the screen, keep your chin level and hold still.", 3),
        (
            "Raise your chin about 25 degrees, straight up, and hold.",
            5,
        ),
        (
            "Return to the same level posture and hold until the guide closes.",
            10,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let (before, current) = phases.split_at_mut(index);
        let phase = &mut current[0];
        prompt(index as u8 + 1, seconds);
        println!("{instruction}");
        let started = Instant::now();
        let end = started + Duration::from_secs(seconds);
        let mut check_return = started + Duration::from_secs(3);
        while Instant::now() < end {
            if cancel() {
                return Ok(None);
            }
            lease.renew()?;
            let n = lease.device.read_timeout(&mut report, 35)?;
            if last.elapsed() >= Duration::from_millis(250) {
                return Err(
                    "Motion paused during calibration. Keep the buds connected and try again."
                        .into(),
                );
            }
            if n == 0 {
                continue;
            }
            last = Instant::now();
            if source(&report[..n]) != Some(origin) {
                return Err(
                    "The motion source changed during calibration. Face forward and retry.".into(),
                );
            }
            let sample = CalibrationSample::from_report(&report[..n])
                .ok_or("Invalid motion sample during calibration")?;
            if phase.len() >= 120 * seconds as usize {
                return Err("Unexpected motion sample rate during calibration".into());
            }
            phase.push(sample);
            if index == 2 && Instant::now() >= check_return {
                check_return += Duration::from_millis(250);
                if let Ok(calibration) = Calibration::from_holds(&before[0], &before[1], phase) {
                    accepted = Some(calibration);
                    break;
                }
            }
        }
        if read_pair(&control)? != Some(pair) {
            return Err(
                "The AirPods connection changed during calibration. Reconnect and retry.".into(),
            );
        }
    }
    let calibration = match accepted {
        Some(calibration) => calibration,
        None => Calibration::from_holds(&phases[0], &phases[1], &phases[2])?,
    };
    if cancel() {
        return Ok(None);
    }
    save(&root, pair, &calibration)?;
    println!(
        "Saved three-axis calibration for the {} primary bud. Start head tracking and face forward.",
        if origin.role[0] == 1 { "left" } else { "right" }
    );
    Ok(PrimaryBud::from_role(origin.role))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_requires_a_current_connected_selected_address() {
        let mut status = [0; 64];
        status[..7].copy_from_slice(b"\x02APDG\x01\x40");
        status[7] = 3;
        status[8] = 1;
        assert!(parse_pair(&status).is_none());
        status[15] = 42;
        assert_eq!(parse_pair(&status).unwrap().session, 1);
        assert!(parse_pair(&status[..63]).is_none());
        for i in [0, 5, 6, 7, 8] {
            let mut bad = status;
            bad[i] = 0;
            assert!(parse_pair(&bad).is_none());
        }
    }

    #[test]
    fn profiles_bind_to_pair_and_all_role_bytes_but_survive_a_new_session() {
        let pair = Pair {
            address: [1, 2, 3, 4, 5, 6],
            session: 2,
        };
        let role = [1, 3, 4, 5];
        let text = format!("mount-v1 {}\n1 0 0\n0 1 0\n0 0 1", key(pair, role));
        assert!(decode(&text, Pair { session: 6, ..pair }, role).is_some());
        assert!(decode(&text, pair, [1, 3, 4, 6]).is_none());
        assert!(
            decode(
                &text,
                Pair {
                    address: [6; 6],
                    ..pair
                },
                role
            )
            .is_none()
        );
        for broken in [
            text.replace("mount-v1", "mount-v2"),
            text.replace("0 0 1", "0 0 -1"),
            text.replace("0 1 0", "NaN 1 0"),
            text + " extra",
        ] {
            assert!(decode(&broken, pair, role).is_none());
        }
    }

    #[test]
    fn source_or_connection_change_retires_profile_until_stable_again() {
        let start = Instant::now();
        let pair = Pair {
            address: [3; 6],
            session: 41,
        };
        let mut src = Source {
            role: [1, 0, 0, 0],
            generation: 9,
        };
        let mut b = Binding::default();
        let mut bound = |ms, src, peer| {
            b.for_source(
                src,
                start + Duration::from_millis(ms),
                || peer,
                |p, role| {
                    (p.address == pair.address).then(|| {
                        Calibration::from_rows(role, [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]])
                            .unwrap()
                    })
                },
            )
            .and_then(|mount| mount.profile)
            .is_some()
        };
        assert!(!bound(0, Some(src), Some(pair)));
        assert!(bound(240, Some(src), Some(pair)));
        src.generation += 1;
        assert!(!bound(260, Some(src), Some(pair)));
        assert!(bound(470, Some(src), Some(pair)));
        src.role[0] = 2;
        assert!(!bound(500, Some(src), Some(pair)));
        assert!(bound(710, Some(src), Some(pair)));
        let changed = Pair {
            session: 42,
            ..pair
        };
        assert!(!bound(1750, Some(src), Some(changed)));
        assert!(bound(1970, Some(src), Some(changed)));
        let other = Pair {
            address: [4; 6],
            ..changed
        };
        assert!(!bound(3000, Some(src), Some(other)));
        assert!(!bound(3220, Some(src), Some(other)));
        assert!(!bound(3240, None, Some(pair)));
        assert!(!bound(3300, Some(src), None));
    }

    #[test]
    fn missing_mount_is_rechecked_after_control_recovers_without_reopening_motion() {
        let start = Instant::now();
        let src = Source {
            role: [2, 0, 1, 0],
            generation: 3,
        };
        let pair = Pair {
            address: [9; 6],
            session: 7,
        };
        let mut binding = Binding::default();
        let mut lookups = 0;
        let mut check = |ms, peer, saved| {
            binding
                .for_source(
                    Some(src),
                    start + Duration::from_millis(ms),
                    || peer,
                    |p, role| {
                        assert_eq!((p, role), (pair, src.role));
                        lookups += 1;
                        if saved {
                            Calibration::from_rows(role, [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]])
                        } else {
                            None
                        }
                    },
                )
                .map(|mount| mount.profile.is_some())
        };
        assert_eq!(check(0, None, false), None);
        assert_eq!(check(1000, Some(pair), false), None);
        assert_eq!(check(1200, Some(pair), false), Some(false));
        assert_eq!(check(1500, Some(pair), true), Some(false));
        assert_eq!(check(2200, Some(pair), true), Some(true));
        assert_eq!(check(3200, Some(pair), true), Some(true));
        assert_eq!(check(4200, None, true), None);
        assert_eq!(lookups, 2);
    }

    #[test]
    fn round_trips_and_replaces_a_complete_mount_without_exposing_partial_state() {
        let root = std::env::temp_dir().join(format!(
            "windows-surround-mount-test-{}",
            std::process::id()
        ));
        let pair = Pair {
            address: [9; 6],
            session: 1,
        };
        let role = [2, 1, 2, 3];
        let axes = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
        let turned = [[0., 1., 0.], [-1., 0., 0.], [0., 0., 1.]];
        for rows in [axes, turned] {
            save(&root, pair, &Calibration::from_rows(role, rows).unwrap()).unwrap();
            assert_eq!(load(&root, pair, role).unwrap().rows(), rows);
        }
        fs::remove_file(root.join(key(pair, role))).unwrap();
        fs::remove_dir(root).unwrap();
    }
}
