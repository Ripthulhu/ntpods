use super::{Q, counter_follows};

#[derive(Clone, Copy, Debug)]
pub(crate) struct CalibrationSample {
    pub(super) attitude: Q,
    pub(super) gravity: [f64; 3],
    pub(super) timestamp: u64,
    pub(super) counter: Option<u32>,
    role: [u8; 4],
    generation: u32,
}

impl CalibrationSample {
    pub(crate) fn source(report: &[u8]) -> Option<([u8; 4], u32)> {
        if report.len() != 51 || report[..2] != [1, 1] || report[2] & !1 != 0 {
            return None;
        }
        Some((
            report[3..7].try_into().ok()?,
            u32::from_le_bytes(report[7..11].try_into().ok()?),
        ))
    }

    pub(crate) fn from_report(report: &[u8]) -> Option<Self> {
        let (role, generation) = Self::source(report)?;
        Self::decode(report, role, generation)
    }

    pub(super) fn decode(report: &[u8], role: [u8; 4], generation: u32) -> Option<Self> {
        if !matches!(role[0], 1 | 2) {
            return None;
        }
        let timestamp = u64::from_le_bytes(report[15..23].try_into().ok()?);
        if timestamp == 0 || timestamp > i64::MAX as u64 {
            return None;
        }
        let vector = |offset| {
            std::array::from_fn(|i| {
                let j = offset + 2 * i;
                f64::from(i16::from_le_bytes([report[j], report[j + 1]]))
            })
        };
        Some(Self {
            role,
            generation,
            timestamp,
            counter: (report[2] & 1 != 0)
                .then(|| u32::from_le_bytes(report[23..27].try_into().unwrap())),
            attitude: Q::decode(vector(27).map(|n| n / 32768.0))?,
            gravity: vector(45),
        })
    }
}

type Vec3 = [f64; 3];

fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn unit(v: Vec3) -> Option<Vec3> {
    let length = dot(v, v).sqrt();
    (length > 1e-10).then(|| v.map(|n| n / length))
}
fn sum(v: &mut Vec3, b: Vec3) {
    for i in 0..3 {
        v[i] += b[i];
    }
}

// Use the same signed nose-up axis and settled-gravity criteria as LibrePods.
fn settled_hold(samples: &[CalibrationSample]) -> Option<Q> {
    let start = samples.last()?.timestamp.saturating_sub(500_000_000);
    let tail = &samples[samples.partition_point(|s| s.timestamp < start)..];
    if tail.len() < 5 {
        return None;
    }
    let first = tail[0].attitude;
    let mut total = [0.0; 4];
    let mut gravity = [0.0; 3];
    for s in tail {
        let q = s.attitude;
        let sign = if first.dot(q) < 0.0 { -1.0 } else { 1.0 };
        for (result, component) in total.iter_mut().zip([q.w, q.x, q.y, q.z]) {
            *result += sign * component;
        }
        sum(&mut gravity, unit(s.gravity)?);
    }
    let norm = total.iter().map(|n| n * n).sum::<f64>().sqrt();
    if norm < 1e-10 {
        return None;
    }
    let [w, x, y, z] = total.map(|n| n / norm);
    let mean = Q { w, x, y, z };
    let down = unit(gravity)?;
    let threshold = 5.0_f64.to_radians().cos();
    if !mean.gravity_agrees(down)
        || tail.iter().any(|s| {
            mean.distance(s.attitude) > 2.0_f64.to_radians()
                || unit(s.gravity).is_none_or(|g| dot(g, down) < threshold)
        })
    {
        return None;
    }
    Some(mean)
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Calibration {
    pub role: [u8; 4],
    rows: [Vec3; 3],
}

impl Calibration {
    pub(crate) fn rows(&self) -> [Vec3; 3] {
        self.rows
    }

    pub(crate) fn from_rows(role: [u8; 4], rows: [Vec3; 3]) -> Option<Self> {
        let [x, y, z] = rows;
        if !matches!(role[0], 1 | 2)
            || rows.iter().any(|r| {
                r.iter().any(|n| !n.is_finite()) || (dot(*r, *r).sqrt() - 1.0).abs() > 1e-5
            })
            || [dot(x, y), dot(x, z), dot(y, z)]
                .iter()
                .any(|p| p.abs() > 1e-5)
            || (dot(cross(x, y), z) - 1.0).abs() > 1e-5
        {
            return None;
        }
        Some(Self { role, rows })
    }

    pub(crate) fn from_holds(
        neutral: &[CalibrationSample],
        up: &[CalibrationSample],
        returned: &[CalibrationSample],
    ) -> Result<Self, &'static str> {
        let first = neutral
            .iter()
            .position(|s| {
                let q = s.attitude;
                q.x != 0.0 || q.y != 0.0 || q.z != 0.0
            })
            .ok_or("No valid starting pose")?;
        let neutral = &neutral[first..];
        if neutral.len() < 5 || up.len() < 3 || returned.len() < 5 {
            return Err("Too few motion reports");
        }
        let mut previous = None::<&CalibrationSample>;
        for s in neutral.iter().chain(up).chain(returned) {
            if s.role != neutral[0].role || s.generation != neutral[0].generation {
                return Err("The primary bud or its motion stream changed during calibration");
            }
            if let Some(last) = previous
                && (s.timestamp <= last.timestamp
                    || last.counter.is_some_and(|old| {
                        s.counter.is_none_or(|new| {
                            !counter_follows(old, new, s.timestamp - last.timestamp)
                        })
                    }))
            {
                return Err("The motion clock or counter restarted during calibration");
            }
            previous = Some(s);
        }
        let base =
            settled_hold(neutral).ok_or("Hold your head still facing forward before looking up")?;
        let finish =
            settled_hold(returned).ok_or("Hold your head still after returning to center")?;
        if base.distance(finish) > 5.0_f64.to_radians() {
            return Err("Return to the same forward direction after looking up");
        }
        let z = base.inverse().rotate([0.0, 0.0, 1.0]);
        let mut axes = Vec::with_capacity(up.len());
        let mut axis_sum = [0.0; 3];
        for sample in up {
            let delta = base.inverse().mul(sample.attitude);
            if delta.distance(Q::IDENTITY) < 8.0_f64.to_radians() {
                continue;
            }
            let sign = if delta.w < 0.0 { -1.0 } else { 1.0 };
            let axis = unit([delta.x * sign, delta.y * sign, delta.z * sign])
                .ok_or("Invalid look-up rotation")?;
            let vertical = dot(axis, z);
            if vertical.abs() > 10.0_f64.to_radians().sin() {
                return Err("Look straight up without turning left or right");
            }
            let horizontal = unit(std::array::from_fn(|i| axis[i] - z[i] * vertical))
                .ok_or("Invalid look-up axis")?;
            sum(&mut axis_sum, horizontal);
            axes.push(horizontal);
        }
        if axes.len() < 3 {
            return Err("Look up farther and hold the position");
        }
        let x = unit(axis_sum).ok_or("Inconsistent look-up direction")?;
        if axes
            .iter()
            .any(|axis| dot(*axis, x) < 5.0_f64.to_radians().cos())
        {
            return Err("Look straight up without tilting to one side");
        }
        Self::from_rows(neutral[0].role, [x, cross(z, x), z]).ok_or("Invalid calibration axes")
    }
}
