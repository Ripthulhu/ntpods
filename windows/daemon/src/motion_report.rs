//! Raw motion fields shared by SensorDataWX and RemoteHID. Calibration is model-specific.

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Report {
    pub timestamp_ns: Option<u64>,
    pub attitude_q15: [i16; 3],
    pub gyro_raw: [i16; 3],
    pub user_acceleration_raw: [i16; 3],
    pub gravity_raw: [i16; 3],
    pub counter: Option<u32>,
}

impl From<Report> for ntpods_ipc::MotionReport {
    fn from(r: Report) -> Self {
        Self {
            timestamp_ns: r.timestamp_ns,
            attitude_q15: r.attitude_q15,
            gyro_raw: r.gyro_raw,
            user_acceleration_raw: r.user_acceleration_raw,
            gravity_raw: r.gravity_raw,
            counter: r.counter,
        }
    }
}

impl Report {
    pub fn parse(data: &[u8]) -> Option<Self> {
        if !matches!(data.len(), 56 | 58) || data[0] != 1 {
            return None;
        }
        let timestamp = u64::from_le_bytes(data[1..9].try_into().ok()?);
        if timestamp > i64::MAX as u64 {
            return None;
        }
        let vector = |at| {
            std::array::from_fn(|i| i16::from_le_bytes([data[at + 2 * i], data[at + 2 * i + 1]]))
        };
        Some(Self {
            timestamp_ns: (timestamp != 0).then_some(timestamp),
            attitude_q15: vector(20),
            gyro_raw: vector(26),
            user_acceleration_raw: vector(38),
            gravity_raw: vector(44),
            counter: (data.len() == 58)
                .then(|| u32::from_le_bytes(data[54..58].try_into().unwrap())),
        })
    }

    /// Measured on ordinary AirPods 3; callers must validate that model and its descriptor.
    pub fn airpods3_quaternion(&self) -> Option<[f64; 4]> {
        let v = self.attitude_q15.map(|x| f64::from(x) / 32768.0);
        let norm: f64 = v.iter().map(|x| x * x).sum();
        if norm > 1.01 {
            return None;
        }
        let mut q = [(1.0 - norm).max(0.0).sqrt(), v[0], v[1], v[2]];
        let length = q.iter().map(|x| x * x).sum::<f64>().sqrt();
        for x in &mut q {
            *x /= length;
        }
        Some(q)
    }

    /// Gravity-referenced rates require the user's measured lateral axis; they are not absolute pose.
    pub fn airpods3_rates(&self, lateral: [f64; 3]) -> Option<[f64; 3]> {
        let dot = |a: [f64; 3], b: [f64; 3]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f64>();
        let normalize = |a: [f64; 3]| -> Option<[f64; 3]> {
            let n = dot(a, a).sqrt();
            (n.is_finite() && n > 0.001).then(|| a.map(|x| x / n))
        };
        let up = normalize(self.gravity_raw.map(f64::from))?;
        let projection = dot(lateral, up);
        let side = normalize(std::array::from_fn(|i| lateral[i] - projection * up[i]))?;
        let forward = [
            up[1] * side[2] - up[2] * side[1],
            up[2] * side[0] - up[0] * side[2],
            up[0] * side[1] - up[1] * side[0],
        ];
        let gyro = self.gyro_raw.map(|x| f64::from(x) * 0.122);
        Some([dot(gyro, up), dot(gyro, side), dot(gyro, forward)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_fields_and_identity_quaternion_use_shared_fixture() {
        let f: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/sensor-wire-v1.json")).unwrap();
        let text = f["motion"]["report"].as_str().unwrap();
        let b: Vec<_> = text
            .as_bytes()
            .chunks_exact(2)
            .map(|x| u8::from_str_radix(std::str::from_utf8(x).unwrap(), 16).unwrap())
            .collect();
        let m = Report::parse(&b).unwrap();
        assert_eq!(m.timestamp_ns, Some(1234567890));
        assert_eq!(m.gyro_raw, [-10, 20, -30]);
        assert_eq!(m.user_acceleration_raw, [100, -200, 300]);
        assert_eq!(m.gravity_raw, [0, 0, 1000]);
        assert_eq!(m.counter, Some(77));
        assert_eq!(m.airpods3_quaternion(), Some([1., 0., 0., 0.]));
        assert_eq!(m.airpods3_rates([0., 1., 0.]), Some([-3.66, 2.44, 1.22]));
        assert!(m.airpods3_rates([0., 0., 1.]).is_none());
        assert!(m.airpods3_rates([f64::NAN, 0., 0.]).is_none());
        assert_eq!(Report::parse(&b[..56]).unwrap().counter, None);
        for n in 0..56 {
            assert!(Report::parse(&b[..n]).is_none());
        }
    }
}
