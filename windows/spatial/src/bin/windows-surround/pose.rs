use std::time::{Duration, Instant};

const STALE: Duration = Duration::from_millis(250);

pub(crate) fn fade_yaw(degrees: f32, age: Duration) -> f32 {
    degrees * ((0.25 - age.as_secs_f32()) / 0.15).clamp(0.0, 1.0)
}

// Active world-from-bud rotation, in LibrePods' canonical positive-w Q15 encoding.
#[derive(Clone, Copy, Debug)]
struct Q {
    w: f64,
    x: f64,
    y: f64,
    z: f64,
}

impl Q {
    const IDENTITY: Self = Self {
        w: 1.0,
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    fn inverse(self) -> Self {
        Self {
            w: self.w,
            x: -self.x,
            y: -self.y,
            z: -self.z,
        }
    }

    fn mul(self, b: Self) -> Self {
        Self {
            w: self.w * b.w - self.x * b.x - self.y * b.y - self.z * b.z,
            x: self.w * b.x + self.x * b.w + self.y * b.z - self.z * b.y,
            y: self.w * b.y - self.x * b.z + self.y * b.w + self.z * b.x,
            z: self.w * b.z + self.x * b.y - self.y * b.x + self.z * b.w,
        }
    }

    fn rotate(self, [x, y, z]: [f64; 3]) -> [f64; 3] {
        let v = self.mul(Q { w: 0.0, x, y, z }).mul(self.inverse());
        [v.x, v.y, v.z]
    }

    fn distance(self, other: Self) -> f64 {
        2.0 * self.dot(other).abs().min(1.0).acos()
    }

    fn decode([x, y, z]: [f64; 3]) -> Option<Self> {
        let norm = x * x + y * y + z * z;
        if norm > 1.01 {
            return None;
        }
        let length = norm.max(1.0).sqrt();
        Some(Self {
            w: (1.0 - norm).max(0.0).sqrt(),
            x: x / length,
            y: y / length,
            z: z / length,
        })
    }

    fn dot(self, b: Self) -> f64 {
        self.w * b.w + self.x * b.x + self.y * b.y + self.z * b.z
    }

    fn gravity_agrees(self, gravity: [f64; 3]) -> bool {
        // Rotate raw gravity into world coordinates. At rest it should point down.
        let Self { w, x, y, z } = self;
        let [gx, gy, gz] = gravity;
        let world_z = 2.0 * (x * z - w * y) * gx
            + 2.0 * (y * z + w * x) * gy
            + (1.0 - 2.0 * (x * x + y * y)) * gz;
        world_z < -5.0_f64.to_radians().cos() * gx.hypot(gy).hypot(gz)
    }

    fn yaw_from(self, base: Self) -> Option<f32> {
        // current * inverse(base) cancels each bud's fixed mounting rotation.
        // Extract its twist about world up. Positive world-Z turns toward the left.
        let w = self.dot(base);
        let z = self.z * base.w - self.x * base.y + self.y * base.x - self.w * base.z;
        if w * w + z * z < 1e-6 {
            return None;
        } // Upside down: yaw is undefined.
        let right = (-2.0 * z.atan2(w)).to_degrees();
        Some(((right + 180.0).rem_euclid(360.0) - 180.0) as f32)
    }
}

// The stored rotation maps fixed room directions into the listener's Steam Audio axes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Orientation(Q);

impl Default for Orientation {
    fn default() -> Self {
        Self(Q::IDENTITY)
    }
}

impl Orientation {
    pub(crate) fn from_yaw(right_degrees: f32) -> Self {
        let (y, w) = (0.5 * f64::from(right_degrees).to_radians()).sin_cos();
        Self(Q {
            w,
            x: 0.0,
            y,
            z: 0.0,
        })
    }

    pub(crate) fn listener_direction(self, room: [f32; 3]) -> [f32; 3] {
        self.0.rotate(room.map(f64::from)).map(|n| n as f32)
    }

    pub(crate) fn yaw_degrees(self) -> f32 {
        let [x, _, z] = self.0.inverse().rotate([0.0, 0.0, -1.0]);
        x.atan2(-z).to_degrees() as f32
    }

    pub(crate) fn pose_degrees(self) -> [i16; 3] {
        let head = self.0.inverse();
        let [x, y, z] = head.rotate([0., 0., -1.]);
        let right_y = head.rotate([1., 0., 0.])[1];
        let up_y = head.rotate([0., 1., 0.])[1];
        [x.atan2(-z), y.atan2(x.hypot(z)), (-right_y).atan2(up_y)]
            .map(|angle| angle.to_degrees().round() as i16)
    }

    pub(crate) fn faded(self, age: Duration) -> Self {
        let fraction = f64::from(fade_yaw(1.0, age));
        if fraction >= 1.0 {
            return self;
        }
        if fraction <= 0.0 {
            return Self::default();
        }
        let q = self.0;
        let length = q.x.hypot(q.y).hypot(q.z);
        if length < 1e-10 {
            return Self::default();
        }
        // Use the shorter arc even when the raw quaternion crosses the sign branch.
        let (s, w) = (fraction * length.atan2(q.w.abs())).sin_cos();
        let scale = s * if q.w < 0.0 { -1.0 } else { 1.0 } / length;
        Self(Q {
            w,
            x: q.x * scale,
            y: q.y * scale,
            z: q.z * scale,
        })
    }
}

#[path = "calibration.rs"]
mod calibration;
pub(crate) use calibration::{Calibration, CalibrationSample};

struct Reference {
    generation: u32,
    role: [u8; 4],
    center: Q,
    previous: Q,
    timestamp: u64,
    counter: Option<u32>,
    arrival: Instant,
    degrees: f32,
}

#[derive(Default)]
pub(crate) struct Pose {
    reference: Option<Reference>,
    needs_center: bool,
}

fn counter_follows(old: u32, new: u32, dt: u64) -> bool {
    if (1..=0x7fff_ffff).contains(&new.wrapping_sub(old)) {
        return true;
    }
    // Some Pro 3 streams wrap at 16 bits. Only recognize that wrap if the ~5 ms tick agrees.
    let steps = (new as u16).wrapping_sub(old as u16);
    old <= 0xffff && new < old && dt.abs_diff(u64::from(steps) * 5_000_000) <= 5_000_000
}

impl Pose {
    pub(crate) fn role(&self) -> Option<[u8; 4]> {
        self.reference
            .as_ref()
            .filter(|_| !self.needs_center)
            .map(|r| r.role)
    }

    pub(crate) fn orientation(&self, profile: Option<&Calibration>) -> Orientation {
        let Some(r) = self.reference.as_ref().filter(|_| !self.needs_center) else {
            return Orientation::default();
        };
        let Some(profile) = profile.filter(|p| p.role == r.role) else {
            return Orientation::from_yaw(r.degrees);
        };
        // C * (Rnow^-1 * Rcenter) * C^T rotates only the quaternion vector part by C.
        let delta = r.previous.inverse().mul(r.center);
        let [right, front, up] = profile
            .rows()
            .map(|axis| axis[0] * delta.x + axis[1] * delta.y + axis[2] * delta.z);
        Orientation(Q {
            w: delta.w,
            x: right,
            y: up,
            z: -front,
        })
    }

    pub(crate) fn recenter(&mut self) {
        // Keep the sensor timestamp so a duplicate cannot become the new forward direction.
        self.needs_center = true;
    }

    #[cfg(test)]
    pub(crate) fn yaw(&self, now: Instant) -> f32 {
        if self.needs_center {
            return 0.0;
        }
        self.reference.as_ref().map_or(0.0, |r| {
            fade_yaw(r.degrees, now.saturating_duration_since(r.arrival))
        })
    }

    pub(crate) fn update(&mut self, report: &[u8], now: Instant) -> bool {
        let Some((role, generation)) = CalibrationSample::source(report) else {
            return false;
        };
        if let Some(r) = &self.reference
            && (r.role != role
                || r.generation != generation
                || now.saturating_duration_since(r.arrival) >= STALE)
        {
            self.recenter();
        }
        let Some(CalibrationSample {
            timestamp,
            counter,
            attitude: q,
            gravity,
            ..
        }) = CalibrationSample::decode(report, role, generation)
        else {
            return false;
        };

        if let Some(r) = &mut self.reference {
            if r.generation == generation && r.role == role && timestamp <= r.timestamp {
                return false;
            }
            if !self.needs_center {
                let dt = timestamp - r.timestamp;
                let counter_ok = r
                    .counter
                    .is_none_or(|old| counter.is_some_and(|new| counter_follows(old, new, dt)));
                if dt < 250_000_000 && counter_ok {
                    let angle = q.distance(r.previous);
                    if angle <= 32.0 * dt as f64 / 1e9 {
                        let degrees = q.yaw_from(r.center).unwrap_or(r.degrees);
                        r.previous = q;
                        r.timestamp = timestamp;
                        r.counter = counter;
                        r.arrival = now;
                        r.degrees = degrees;
                        return true;
                    }
                }
            }
            self.recenter();
        }
        // At startup the buds can send zero-filled attitude; a true identity is valid later.
        if (q.x == 0.0 && q.y == 0.0 && q.z == 0.0) || !q.gravity_agrees(gravity) {
            return false;
        }
        self.reference = Some(Reference {
            generation,
            role,
            center: q,
            previous: q,
            timestamp,
            counter,
            arrival: now,
            degrees: 0.0,
        });
        self.needs_center = false;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn z(degrees: f64) -> Q {
        let (s, c) = (0.5 * degrees.to_radians()).sin_cos();
        Q {
            w: c,
            x: 0.0,
            y: 0.0,
            z: s,
        }
    }
    fn mount_x(q: Q, degrees: f64) -> Q {
        let (s, c) = (0.5 * degrees.to_radians()).sin_cos();
        Q {
            w: q.w * c - q.x * s,
            x: q.x * c + q.w * s,
            y: q.y * c + q.z * s,
            z: q.z * c - q.y * s,
        }
    }
    fn packet(q: Q, time: u64) -> [u8; 51] {
        let mut b = [0u8; 51];
        b[..4].copy_from_slice(&[1, 1, 1, 2]);
        b[7] = 12;
        b[15..23].copy_from_slice(&time.to_le_bytes());
        b[23..27].copy_from_slice(&((time / 5_000_000) as u32).to_le_bytes());
        let Q { w, x, y, z } = q;
        let sign = if w < 0.0 { -1.0 } else { 1.0 };
        for (start, values, scale) in [
            (27, [x, y, z], sign * 32768.0),
            (
                45,
                [
                    2.0 * (x * z - w * y),
                    2.0 * (y * z + w * x),
                    1.0 - 2.0 * (x * x + y * y),
                ],
                -4096.0,
            ),
        ] {
            for (i, value) in values.iter().enumerate() {
                b[start + 2 * i..start + 2 * i + 2]
                    .copy_from_slice(&((value * scale).round() as i16).to_le_bytes());
            }
        }
        b
    }

    fn fixture(
        mount: Q,
    ) -> (
        Q,
        Vec<CalibrationSample>,
        Vec<CalibrationSample>,
        Vec<CalibrationSample>,
    ) {
        let heading = z(178.0);
        let phase = |start: u64, angle| {
            (0..18)
                .map(|i| {
                    let q = heading.mul(mount_x(Q::IDENTITY, angle)).mul(mount);
                    CalibrationSample::from_report(&packet(
                        q,
                        1_000_000_000 + (start + i) * 33_000_000,
                    ))
                    .unwrap()
                })
                .collect()
        };
        (heading, phase(0, 0.0), phase(18, 30.0), phase(36, 0.0))
    }

    fn approx(actual: [f32; 3], expected: [f32; 3]) {
        assert!(
            actual
                .into_iter()
                .zip(expected)
                .all(|(a, b)| (a - b).abs() < 0.003),
            "direction {actual:?} vs {expected:?}"
        );
    }

    #[test]
    fn displayed_pose_uses_right_yaw_nose_up_and_right_ear_down() {
        assert_eq!(Orientation::from_yaw(35.).pose_degrees(), [35, 0, 0]);
        let (s, c) = (-15.0_f64.to_radians()).sin_cos();
        assert_eq!(
            Orientation(Q {
                w: c,
                x: s,
                y: 0.,
                z: 0.
            })
            .pose_degrees(),
            [0, 30, 0]
        );
        assert_eq!(
            Orientation(Q {
                w: c,
                x: 0.,
                y: 0.,
                z: -s
            })
            .pose_degrees(),
            [0, 0, 30]
        );
    }

    #[test]
    fn calibrated_room_stays_fixed_across_real_mount_angles_and_all_three_axes() {
        let front = [0.0, 0.0, -1.0];
        let right = [1.0, 0.0, 0.0];
        let (s, c) = 30.0_f32.to_radians().sin_cos();
        let (rs, rc) = (15.0_f64.to_radians()).sin_cos();
        let rolls_right = Q {
            w: rc,
            x: 0.0,
            y: rs,
            z: 0.0,
        };
        for mount in [mount_x(z(40.0), -103.0), mount_x(z(-63.0), 60.0)] {
            let (heading, neutral, up, returned) = fixture(mount);
            let cal = Calibration::from_holds(&neutral, &up, &returned).unwrap();
            let reloaded = Calibration::from_rows(cal.role, cal.rows()).unwrap();
            for (tilt, source, expected) in [
                (z(-30.0), front, [-s, 0.0, -c]),
                (mount_x(Q::IDENTITY, 30.0), front, [0.0, -s, -c]),
                (rolls_right, right, [c, s, 0.0]),
                (rolls_right, front, front),
                (
                    z(-30.0).mul(mount_x(Q::IDENTITY, 30.0)),
                    front,
                    [-s, -s * c, -c * c],
                ),
            ] {
                let mut pose = Pose::default();
                let now = Instant::now();
                assert!(pose.update(&packet(heading.mul(mount), 1_000_000_000), now));
                let turned = packet(heading.mul(tilt).mul(mount), 1_050_000_000);
                assert!(pose.update(&turned, now + Duration::from_millis(50)));
                approx(
                    pose.orientation(Some(&reloaded)).listener_direction(source),
                    expected,
                );
                approx(
                    pose.orientation(Some(&reloaded))
                        .faded(STALE)
                        .listener_direction(source),
                    source,
                );
                let mut other = reloaded;
                other.role[1] ^= 1;
                approx(
                    pose.orientation(Some(&other)).listener_direction(right),
                    pose.orientation(None).listener_direction(right),
                );
                pose.recenter();
                assert!(pose.role().is_none());
                approx(
                    pose.orientation(Some(&reloaded)).listener_direction(source),
                    source,
                );
                assert!(!pose.update(&turned, now + Duration::from_millis(51)));
                let fresh = packet(heading.mul(tilt).mul(mount), 1_080_000_000);
                assert!(pose.update(&fresh, now + Duration::from_millis(80)));
                approx(
                    pose.orientation(Some(&reloaded)).listener_direction(source),
                    source,
                );
            }
        }
    }

    #[test]
    fn calibration_rejects_role_swaps_bad_gravity_and_a_mislabeled_turn() {
        let (heading, neutral, up, returned) = fixture(z(25.0));
        let mut corrupt = up.clone();
        let mut wrong_role = packet(heading, 1_600_000_000);
        wrong_role[5] ^= 1;
        corrupt[0] = CalibrationSample::from_report(&wrong_role).unwrap();
        assert!(Calibration::from_holds(&neutral, &corrupt, &returned).is_err());
        assert!(Calibration::from_holds(&neutral, &up, &neutral).is_err());
        let mut bad_hold = returned.clone();
        bad_hold.last_mut().unwrap().gravity = [0.0; 3];
        assert!(Calibration::from_holds(&neutral, &up, &bad_hold).is_err());
        let mut wrong_axis = up;
        for s in &mut wrong_axis {
            s.attitude = heading.mul(z(30.0)).mul(z(25.0));
        }
        assert!(Calibration::from_holds(&neutral, &wrong_axis, &returned).is_err());
        let mut malformed = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]];
        assert!(Calibration::from_rows([2, 0, 0, 0], malformed).is_none());
        malformed[2][2] = f64::NAN;
        assert!(Calibration::from_rows([2, 0, 0, 0], malformed).is_none());
    }

    #[test]
    fn stale_orientation_fades_on_the_short_arc_for_all_axes() {
        let q = Orientation::from_yaw(270.0);
        assert!((q.faded(Duration::from_millis(175)).yaw_degrees() + 45.0).abs() < 1e-4);
        let tilted = Orientation(mount_x(Q::IDENTITY, 60.0));
        approx(
            tilted
                .faded(Duration::from_millis(175))
                .listener_direction([0.0, 0.0, -1.0]),
            [0.0, 0.5, -(0.75_f32.sqrt())],
        );
    }

    #[test]
    fn right_yaw_independent_of_bud_mount_pitch_and_quaternion_sign() {
        for mount in [-103.0, 60.0] {
            let mut pose = Pose::default();
            let now = Instant::now();
            let dt = Duration::from_millis(30);
            for (i, (turn, pitch)) in [(176.0, 0.0), (188.0, 0.0), (163.0, 15.0)]
                .into_iter()
                .enumerate()
            {
                let q = mount_x(z(turn), mount + pitch);
                let arrival = now + dt * i as u32;
                assert!(pose.update(&packet(q, 1_000_000_000 + 30_000_000 * i as u64), arrival));
                assert!((pose.yaw(arrival) + (turn - 176.0) as f32).abs() < 0.15);
            }
        }
    }

    #[test]
    fn freshness_and_source_changes_require_a_new_neutral() {
        let mut pose = Pose::default();
        let now = Instant::now();
        let next = now + Duration::from_millis(30);
        let base = packet(z(25.0), 1_000_000_000);
        let turned = packet(z(5.0), 1_030_000_000);
        assert!(pose.update(&base, now));
        assert!(pose.update(&turned, next));
        assert!(!pose.update(&turned, next + Duration::from_millis(70)));
        assert!((pose.yaw(next + Duration::from_millis(175)) - 10.0).abs() < 0.01);
        assert_eq!(pose.yaw(next + STALE), 0.0);
        assert!(!pose.update(&turned, next + STALE)); // Old sensor sample cannot establish a new center.
        assert_eq!(pose.yaw(next + STALE), 0.0);
        let fresh = next + STALE + Duration::from_millis(30);
        assert!(pose.update(&packet(z(6.0), 1_310_000_000), fresh));
        assert_eq!(pose.yaw(fresh), 0.0);
        for selector in [3, 7] {
            let mut pose = Pose::default();
            pose.update(&base, now);
            pose.update(&turned, next);
            let mut replacement = turned;
            replacement[selector] = 1;
            assert!(pose.update(&replacement, next));
            assert_eq!(pose.yaw(next), 0.0);
        }
    }

    #[test]
    fn gravity_is_only_used_at_center_and_counters_allow_valid_wraps() {
        let mut pose = Pose::default();
        let now = Instant::now();
        let mut center = packet(z(2.0), 1_000_000_000);
        let mut next = packet(z(0.0), 1_010_000_000);
        center[23..27].copy_from_slice(&65534u32.to_le_bytes());
        next[23..27].copy_from_slice(&0u32.to_le_bytes());
        next[45..].fill(0);
        assert!(pose.update(&center, now));
        assert!(pose.update(&next, now + Duration::from_millis(10)));
        assert!((pose.yaw(now + Duration::from_millis(10)) - 2.0).abs() < 0.01);
        pose.recenter();
        assert!(!pose.update(&next, now));
        center[45..].fill(0);
        assert!(!pose.update(&center, now));
        assert!(counter_follows(u32::MAX - 1, 0, 10_000_000));
        assert!(!counter_follows(30000, 0, 10_000_000));
    }

    #[test]
    fn rejects_malformed_or_out_of_order_reports_without_refreshing() {
        let mut pose = Pose::default();
        let now = Instant::now();
        let center = packet(z(-2.0), 1_000_000_000);
        assert!(pose.update(&center, now));
        assert!(!pose.update(&center[..50], now));
        for i in [0, 1, 3] {
            let mut bad = center;
            bad[i] = 0;
            assert!(!pose.update(&bad, now));
        }
        let mut invalid_norm = packet(z(5.0), 1_010_000_000);
        invalid_norm[27..33].copy_from_slice(&[0xff, 0x7f, 0xff, 0x7f, 0xff, 0x7f]);
        assert!(!pose.update(&invalid_norm, now));
        assert!(!pose.update(&packet(z(-2.0), 999_000_000), now));
    }
}
