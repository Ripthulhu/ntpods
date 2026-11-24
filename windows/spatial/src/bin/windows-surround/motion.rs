use super::Result;
use super::pose::{Orientation, Pose};
use hidapi::{HidApi, HidDevice};
use std::{
    cell::Cell,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

#[path = "mounts.rs"]
mod mounts;
pub(super) const NO_PAIRED_MOTION: &str = "No paired AirPods motion source";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PrimaryBud {
    Left,
    Right,
}

impl PrimaryBud {
    fn from_role(role: [u8; 4]) -> Option<Self> {
        match role[0] {
            1 => Some(Self::Left),
            2 => Some(Self::Right),
            _ => None,
        }
    }
}

pub(super) fn calibrate(
    serial: &str,
    prompt: impl Fn(u8, u64),
    cancel: impl Fn() -> bool,
) -> Result<Option<PrimaryBud>> {
    mounts::calibrate(serial, prompt, cancel)
}

const REQUEST_HZ: u8 = 100;
const TTL: Duration = Duration::from_secs(3);

fn requested_rate(device: &HidDevice) -> Result<u8> {
    let mut report = [2, 0];
    if device.get_feature_report(&mut report)? != 2 || report[0] != 2 {
        return Err("Invalid motion lease response".into());
    }
    Ok(report[1])
}

struct Lease {
    device: HidDevice,
    renewed: Instant,
    release_on_drop: bool,
}

fn open_collection(serial: &str, usage: u16) -> Result<HidDevice> {
    let api = HidApi::new()?;
    let mut found = api.device_list().filter(|d| {
        d.vendor_id() == 0xcafe
            && d.product_id() == 0x4015
            && d.usage_page() == 0xff31
            && d.usage() == usage
            && d.serial_number() == Some(serial)
    });
    let info = found
        .next()
        .ok_or("Selected dongle lacks the required HID collection")?;
    if found.next().is_some() {
        return Err("Multiple matching interfaces have this dongle serial".into());
    }
    Ok(info.open_device(&api)?)
}

impl Lease {
    fn open(serial: &str) -> Result<Self> {
        let device = open_collection(serial, 3)?;
        if requested_rate(&device)? != 0 {
            return Err("A motion reader already holds this dongle".into());
        }
        device.send_feature_report(&[2, REQUEST_HZ])?;
        Ok(Self {
            device,
            renewed: Instant::now(),
            release_on_drop: true,
        })
    }

    fn renew(&mut self) -> Result<()> {
        if self.renewed.elapsed() >= Duration::from_secs(1) {
            if self.renewed.elapsed() >= TTL || requested_rate(&self.device)? != REQUEST_HZ {
                return Err("Dongle motion lease expired or changed owners".into());
            }
            self.device.send_feature_report(&[2, REQUEST_HZ])?;
            self.renewed = Instant::now();
        }
        Ok(())
    }

    fn read_motion(&mut self, serial: &str, state: &Shared) -> Result<()> {
        let mut profiles = mounts::Mounts::new();
        let mut pose = Pose::default();
        let mut ticket = 0;
        let mut report = [0u8; 64];
        while !state.stop.load(Ordering::Relaxed) {
            self.renew()?;
            let requested = state.requested_center.load(Ordering::Acquire);
            if requested != ticket {
                pose.recenter();
                // Flush Windows' queued pre-resume input before using a new reference.
                for _ in 0..64 {
                    if self.device.read_timeout(&mut report, 0)? == 0 {
                        ticket = requested;
                        break;
                    }
                }
                continue;
            }
            let count = self.device.read_timeout(&mut report, 25)?;
            if count != 0 {
                let now = Instant::now();
                let mount = profiles.for_report(serial, &report[..count], now);
                state.record(&mut pose, ticket, &report[..count], mount, now);
            }
        }
        Ok(())
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if self.release_on_drop
            && self.renewed.elapsed() < TTL
            && requested_rate(&self.device).ok() == Some(REQUEST_HZ)
        {
            let _ = self.device.send_feature_report(&[2, 0]);
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct MotionStats {
    pub samples: u64,
    pub ignored: u64,
    pub error: Option<String>,
    first: Option<Instant>,
    last: Option<Instant>,
    min_yaw: f32,
    max_yaw: f32,
    last_anchor: u64,
    anchors_confirmed: u64,
    full_tracking: bool,
}

impl fmt::Display for MotionStats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} accepted poses, {} ignored reports",
            self.samples, self.ignored
        )?;
        if let (Some(first), Some(last)) = (self.first, self.last) {
            write!(
                f,
                ", {} tracking",
                if self.full_tracking {
                    "three-axis"
                } else {
                    "horizontal"
                }
            )?;
            let span = (last - first).as_secs_f32();
            if span > 0.0 {
                write!(f, ", {:.1} Hz delivered", (self.samples - 1) as f32 / span)?;
            }
            write!(
                f,
                ", yaw {:+.1}..{:+.1} deg, last pose {} ms ago",
                self.min_yaw,
                self.max_yaw,
                last.elapsed().as_millis()
            )?;
        }
        if let Some(error) = &self.error {
            write!(f, ", waiting for motion reader: {error}")?;
        }
        if self.anchors_confirmed > 0 {
            write!(
                f,
                ", {} new forward references confirmed by motion",
                self.anchors_confirmed
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Sample {
    ticket: u64,
    primary: Option<PrimaryBud>,
    calibrated: bool,
    orientation: Orientation,
    arrival: Instant,
}

impl Sample {
    fn calibration_status(sample: Option<Self>) -> &'static str {
        match sample.map(|s| (s.primary, s.calibrated)) {
            Some((Some(PrimaryBud::Left), true)) => "CAL_STATUS_LEFT_READY",
            Some((Some(PrimaryBud::Right), true)) => "CAL_STATUS_RIGHT_READY",
            Some((Some(PrimaryBud::Left), false)) => "CAL_STATUS_LEFT_MISSING",
            Some((Some(PrimaryBud::Right), false)) => "CAL_STATUS_RIGHT_MISSING",
            _ => "CAL_STATUS_PENDING",
        }
    }
}

#[derive(Default)]
struct Shared {
    managed: bool,
    stop: AtomicBool,
    latest: Mutex<Option<Sample>>,
    requested_center: AtomicU64,
    applied_center: AtomicU64,
    stats: Mutex<MotionStats>,
}

impl Shared {
    fn record(
        &self,
        pose: &mut Pose,
        ticket: u64,
        report: &[u8],
        mount: Option<mounts::BoundMount<'_>>,
        now: Instant,
    ) {
        let mut stats = self.stats.lock().unwrap();
        if !pose.update(report, now) {
            // A repeated USB sample leaves the last good pose in effect until it ages out.
            if pose.role().is_none() {
                self.applied_center.store(0, Ordering::Release);
            }
            stats.ignored += 1;
            return;
        }
        let mount = mount.filter(|m| Some(m.role) == pose.role());
        let profile = mount.as_ref().and_then(|m| m.profile);
        let orientation = pose.orientation(profile);
        let yaw = orientation.yaw_degrees();
        let calibrated = profile.is_some_and(|p| pose.role() == Some(p.role));
        *self.latest.lock().unwrap() = Some(Sample {
            orientation,
            ticket,
            calibrated,
            primary: mount.and_then(|m| PrimaryBud::from_role(m.role)),
            arrival: now,
        });
        stats.error = None;
        if stats.first.is_none() || stats.full_tracking != calibrated {
            if !self.managed {
                println!(
                    "{}",
                    if calibrated {
                        "Three-axis tracking is active for the current bud."
                    } else {
                        "Horizontal tracking is active. Three-axis tracking uses this pair's saved bud calibration."
                    }
                );
            }
            stats.full_tracking = calibrated;
        }
        // Zero denotes no valid published pose, including before the first report.
        self.applied_center
            .store(ticket.wrapping_add(1), Ordering::Release);
        if ticket != stats.last_anchor && ticket == self.requested_center.load(Ordering::Acquire) {
            stats.last_anchor = ticket;
            stats.anchors_confirmed += 1;
        }
        if stats.first.is_none() {
            stats.first = Some(now);
            stats.min_yaw = yaw;
            stats.max_yaw = yaw;
        }
        stats.last = Some(now);
        stats.samples += 1;
        stats.min_yaw = stats.min_yaw.min(yaw);
        stats.max_yaw = stats.max_yaw.max(yaw);
    }

    fn fresh_sample(&self, now: Instant) -> Option<Sample> {
        let ticket = self.requested_center.load(Ordering::Acquire);
        (*self.latest.lock().unwrap()).filter(|s| {
            s.ticket == ticket
                && self.applied_center.load(Ordering::Acquire) == ticket.wrapping_add(1)
                && now.saturating_duration_since(s.arrival) < Duration::from_millis(250)
        })
    }
}

pub(super) struct Motion {
    last_read: Cell<Option<Sample>>,
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}

impl Motion {
    pub(super) fn open(serial: &str, managed: bool) -> Result<Self> {
        let mut initial = if managed {
            None
        } else {
            Some(Lease::open(serial)?)
        };
        let serial = serial.to_owned();
        let shared = Arc::new(Shared {
            managed,
            ..Shared::default()
        });
        let state = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("dongle-head-motion".into())
            .spawn(move || {
                retry_reader(&state, || {
                    let mut lease = match initial.take() {
                        Some(lease) => lease,
                        None => Lease::open(&serial)?,
                    };
                    let result = lease.read_motion(&serial, &state);
                    // After an I/O error the owner is uncertain. Let the old lease expire.
                    lease.release_on_drop = result.is_ok();
                    result
                });
            })?;
        Ok(Self {
            last_read: Cell::new(None),
            shared,
            worker: Some(worker),
        })
    }

    fn sample(&self) -> Option<Sample> {
        // Never wait for HID on the audio thread. Only reuse a pose from this anchor.
        if let Ok(latest) = self.shared.latest.try_lock() {
            self.last_read.set(*latest);
        }
        let sample = self.last_read.get()?;
        let ticket = self.shared.requested_center.load(Ordering::Acquire);
        (sample.ticket == ticket
            && self.shared.applied_center.load(Ordering::Acquire) == ticket.wrapping_add(1))
        .then_some(sample)
    }

    pub(super) fn orientation(&self) -> Orientation {
        self.sample().map_or_else(Orientation::default, |s| {
            s.orientation.faded(s.arrival.elapsed())
        })
    }

    pub(super) fn reanchor_on_resume(&self) {
        self.shared.requested_center.fetch_add(1, Ordering::AcqRel);
    }

    pub(super) fn monitor_room(&self, room: Arc<super::LiveRoom>) -> Result<()> {
        let state = self.shared.clone();
        thread::Builder::new()
            .name("spatial-room-display".into())
            .spawn(move || {
                let mut fresh = Instant::now();
                let mut waiting = false;
                let mut previous = None;
                let mut previous_calibration = None;
                loop {
                    thread::sleep(Duration::from_millis(100));
                    if state.stop.load(Ordering::Acquire) || room.stop.load(Ordering::Acquire) {
                        break;
                    }
                    let mask = room.format.load(Ordering::Acquire);
                    let sample = state.fresh_sample(Instant::now());
                    let calibration = Sample::calibration_status(sample);
                    if previous_calibration != Some(calibration) {
                        println!("{calibration}");
                        previous_calibration = Some(calibration);
                    }
                    if mask == 0 {
                        fresh = Instant::now();
                        continue;
                    }
                    if sample.is_some() {
                        fresh = Instant::now();
                        if waiting {
                            println!("SPATIAL_MOTION_READY");
                            waiting = false;
                        }
                    } else if !waiting && fresh.elapsed() >= Duration::from_secs(2) {
                        println!("SPATIAL_MOTION_WAIT");
                        waiting = true;
                    }
                    let angles = sample.map(|s| (s.orientation.pose_degrees(), s.calibrated));
                    let (active, levels) = room.take_levels(mask);
                    if previous != Some((angles, mask, active, levels)) {
                        let db = levels
                            .iter()
                            .take(mask.count_ones() as usize)
                            .map(i8::to_string)
                            .collect::<Vec<_>>()
                            .join(",");
                        match angles {
                            Some(([y, p, r], true)) => {
                                println!("ROOM_POSE {y} {p} {r} {mask} {active} {db}")
                            }
                            Some(([y, _, _], false)) => {
                                println!("ROOM_POSE {y} none none {mask} {active} {db}")
                            }
                            None => println!("ROOM_POSE none none none {mask} {active} {db}"),
                        }
                        previous = Some((angles, mask, active, levels));
                    }
                }
            })?;
        Ok(())
    }

    pub(super) fn wait_ready(&self, timeout: Duration) -> Result<()> {
        let started = Instant::now();
        loop {
            let stats = self.stats();
            if self.sample().is_some()
                && stats.error.is_none()
                && stats
                    .last
                    .is_some_and(|last| last.elapsed() < Duration::from_millis(100))
            {
                return Ok(());
            }
            let remaining = timeout.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                if let Some(error) = stats.error {
                    return Err(error.into());
                }
                return Err(
                    "No fresh head pose. Wear your AirPods and face forward on the selected dongle"
                        .into(),
                );
            }
            thread::sleep(remaining.min(Duration::from_millis(10)));
        }
    }

    pub(super) fn stats(&self) -> MotionStats {
        self.shared.stats.lock().unwrap().clone()
    }
}

impl Drop for Motion {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

fn retry_reader(state: &Shared, mut read: impl FnMut() -> Result<()>) {
    while !state.stop.load(Ordering::Acquire) {
        let Err(error) = read() else { return };
        state.applied_center.store(0, Ordering::Release);
        *state.latest.lock().unwrap() = None;
        state.stats.lock().unwrap().error = Some(error.to_string());
        thread::park_timeout(Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(turn: f32, tick: u32) -> [u8; 51] {
        let mut report = [0; 51];
        report[..4].copy_from_slice(&[1, 1, 0, 2]);
        report[15..23].copy_from_slice(&(u64::from(tick) * 33_333_333).to_le_bytes());
        let z = ((turn.to_radians() / 2.0).sin() * 32768.0).round() as i16;
        report[31..33].copy_from_slice(&z.to_le_bytes());
        report[49..51].copy_from_slice(&(-4096i16).to_le_bytes());
        report
    }

    #[test]
    fn room_calibration_status_requires_a_fresh_pose_from_the_confirmed_role() {
        use super::super::pose::Calibration;
        let state = Shared::default();
        let now = Instant::now();
        let at = |tick| now + Duration::from_millis(30 * u64::from(tick));
        let status = |tick| Sample::calibration_status(state.fresh_sample(at(tick)));
        let mut pose = Pose::default();
        let rows = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]];
        assert_eq!(status(0), "CAL_STATUS_PENDING");
        for (side, tick, missing, ready) in [
            (1, 1, "CAL_STATUS_LEFT_MISSING", "CAL_STATUS_LEFT_READY"),
            (2, 4, "CAL_STATUS_RIGHT_MISSING", "CAL_STATUS_RIGHT_READY"),
        ] {
            let role = [side, 0, 0, 0];
            let basis = Calibration::from_rows(role, rows).unwrap();
            let mut sample = |tick, mount| {
                let mut r = report(30., tick);
                r[3] = side;
                state.record(&mut pose, 0, &r, mount, at(tick));
            };
            sample(tick, None);
            assert_eq!(status(tick), "CAL_STATUS_PENDING");
            sample(
                tick + 1,
                Some(mounts::BoundMount {
                    role,
                    profile: None,
                }),
            );
            assert_eq!(status(tick + 1), missing);
            sample(
                tick + 2,
                Some(mounts::BoundMount {
                    role,
                    profile: Some(&basis),
                }),
            );
            assert_eq!(status(tick + 2), ready);
        }
        assert_eq!(status(15), "CAL_STATUS_PENDING"); // pose has aged out
        state.applied_center.store(0, Ordering::Release);
        assert_eq!(status(6), "CAL_STATUS_PENDING"); // reader disconnected
    }

    #[test]
    fn reader_retries_after_io_loss_and_reanchors_on_a_fresh_game_mode_pose() {
        let state = Shared::default();
        let now = Instant::now();
        let mut attempt = 0;
        retry_reader(&state, || {
            attempt += 1;
            match attempt {
                1 => {
                    state.record(&mut Pose::default(), 0, &report(60.0, 1), None, now);
                    thread::current().unpark();
                    Err("USB read failed".into())
                }
                2 => {
                    assert_eq!(state.applied_center.load(Ordering::Acquire), 0);
                    assert!(state.latest.lock().unwrap().is_none());
                    state.requested_center.store(1, Ordering::Release);
                    thread::current().unpark();
                    Err("Motion lease is busy".into())
                }
                3 => {
                    let mut fresh = Pose::default();
                    state.record(&mut fresh, 1, &report(-20.0, 8), None, now + TTL);
                    let sample = state.latest.lock().unwrap().unwrap();
                    assert_eq!(sample.ticket, 1);
                    assert_eq!(sample.orientation.yaw_degrees(), 0.0);
                    assert!(state.stats.lock().unwrap().error.is_none());
                    Ok(())
                }
                _ => panic!("Reader reopened after shutdown"),
            }
        });
        assert_eq!(attempt, 3);
    }

    #[test]
    fn resume_masks_in_flight_samples_and_recenters_on_fresh_game_mode_reports() {
        let start = Instant::now();
        let motion = Motion {
            last_read: Cell::new(None),
            shared: Arc::default(),
            worker: None,
        };
        let s = &motion.shared;
        let mut pose = Pose::default();
        let frame_time = |tick: u32| start + Duration::from_nanos(u64::from(tick) * 33_333_333);
        let sample_yaw = |tick| {
            let sample = motion.sample().unwrap();
            sample
                .orientation
                .faded(frame_time(tick).duration_since(sample.arrival))
                .yaw_degrees()
        };
        s.record(&mut pose, 0, &report(65.0, 1), None, frame_time(1));
        s.record(&mut pose, 0, &report(20.0, 2), None, frame_time(2));
        assert!((sample_yaw(2) - 45.0).abs() < 0.1);
        s.record(&mut pose, 0, &report(20.0, 2), None, frame_time(3));
        assert!((sample_yaw(3) - 45.0).abs() < 0.1);
        motion.reanchor_on_resume();
        assert!(motion.sample().is_none());
        // A read already in progress can finish after the audio thread requests a center.
        s.record(&mut pose, 0, &report(19.0, 3), None, frame_time(3));
        assert!(motion.sample().is_none());
        pose.recenter();
        motion.reanchor_on_resume();
        s.record(&mut pose, 1, &report(10.0, 4), None, frame_time(4));
        assert!(motion.sample().is_none()); // superseded request cannot acknowledge the new one
        pose.recenter();
        s.record(&mut pose, 2, &report(10.0, 4), None, frame_time(5));
        assert!(motion.sample().is_none()); // pre-resume duplicate
        for tick in [6, 7] {
            s.record(&mut pose, 2, &report(-30.0, tick), None, frame_time(tick));
            assert_eq!(sample_yaw(tick), 0.0);
        }
        s.record(&mut pose, 2, &report(-45.0, 8), None, frame_time(8));
        assert!((sample_yaw(8) - 15.0).abs() < 0.1);
        let mut no_primary = report(-45.0, 9);
        no_primary[3] = 0;
        s.record(&mut pose, 2, &no_primary, None, frame_time(9));
        assert!(motion.sample().is_none());
        let mut other_bud = report(12.0, 10);
        other_bud[3] = 1;
        s.record(&mut pose, 2, &other_bud, None, frame_time(10));
        assert_eq!(sample_yaw(10), 0.0);
    }

    #[test]
    fn audio_start_requires_a_fresh_sample_from_a_working_reader() {
        let motion = Motion {
            last_read: Cell::new(None),
            shared: Arc::new(Shared::default()),
            worker: None,
        };
        assert!(motion.wait_ready(Duration::ZERO).is_err());
        motion.shared.stats.lock().unwrap().last = Some(Instant::now() - Duration::from_secs(1));
        assert!(motion.wait_ready(Duration::ZERO).is_err());
        motion.shared.stats.lock().unwrap().last = Some(Instant::now());
        *motion.shared.latest.lock().unwrap() = Some(Sample {
            ticket: 0,
            primary: None,
            calibrated: false,
            orientation: Orientation::default(),
            arrival: Instant::now(),
        });
        motion.shared.applied_center.store(1, Ordering::Release);
        assert!(motion.wait_ready(Duration::ZERO).is_ok());
        motion.shared.stats.lock().unwrap().error = Some("USB disconnected".into());
        assert_eq!(
            motion.wait_ready(Duration::ZERO).unwrap_err().to_string(),
            "USB disconnected"
        );
    }

    #[test]
    fn render_uses_last_snapshot_during_a_write_and_never_reuses_a_pre_resume_pose() {
        let now = Instant::now();
        let motion = Motion {
            shared: Arc::default(),
            worker: None,
            last_read: Cell::new(None),
        };
        let old = Sample {
            ticket: 0,
            primary: None,
            calibrated: false,
            orientation: Orientation::from_yaw(-45.0),
            arrival: now,
        };
        *motion.shared.latest.lock().unwrap() = Some(old);
        motion.shared.applied_center.store(1, Ordering::Release);
        assert!((motion.orientation().yaw_degrees() + 45.0).abs() < 0.01);
        let _write = motion.shared.latest.lock().unwrap();
        assert!((motion.orientation().yaw_degrees() + 45.0).abs() < 0.01);
        motion.reanchor_on_resume();
        assert!(motion.sample().is_none());
        assert_eq!(
            old.orientation
                .faded(Duration::from_millis(300))
                .yaw_degrees(),
            0.0
        );
    }
}
