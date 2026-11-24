use super::{BLOCK, Orientation, Result, Surround, pose};
use std::time::{Duration, Instant};

// A level bud with a nonzero starting heading, followed by a nose-up or
// right-ear-down turn. Encode exactly the three signed Q15 fields the HID sends.
fn reported_tilt(pitch: bool) -> Orientation {
    let role = [1, 0, 1, 0];
    let profile =
        pose::Calibration::from_rows(role, [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]]).unwrap();
    let mut pose = pose::Pose::default();
    let start = Instant::now();
    let (sz, cz) = (15f64.to_radians()).sin_cos();
    for (i, tilt) in [0f64, 25.].into_iter().enumerate() {
        let (s, c) = (tilt.to_radians() * 0.5).sin_cos();
        let axes = if pitch {
            [cz * s, sz * s, sz * c]
        } else {
            [-sz * s, cz * s, sz * c]
        };
        let mut report = [0; 51];
        report[..2].copy_from_slice(&[1, 1]);
        report[3..7].copy_from_slice(&role);
        report[15..23].copy_from_slice(&((i as u64 + 1) * 33_000_000).to_le_bytes());
        for (bytes, v) in report[27..33].chunks_exact_mut(2).zip(axes) {
            bytes.copy_from_slice(&((v * 32768.).round() as i16).to_le_bytes());
        }
        report[49..51].copy_from_slice(&(-1024i16).to_le_bytes());
        assert!(pose.update(&report, start + Duration::from_millis(33 * i as u64)));
    }
    pose.orientation(Some(&profile))
}

#[test]
fn calibrated_pitch_and_roll_match_signed_elevation_in_binaural_pcm() -> Result<()> {
    let mut tracked = Surround::new(8, 0x63f)?;
    let mut reference = Surround::new(8, 0x63f)?;
    let (s, c) = (25f32.to_radians()).sin_cos();
    for (pitch, channel, expected) in [
        (true, 2, [0., -s, -c]),  // Screen voice below a raised chin.
        (false, 7, [c, s, 0.]),   // Right speaker above the lowered right ear.
        (false, 6, [-c, -s, 0.]), // Left speaker below the raised left ear.
    ] {
        let mut bed = [0f32; 8 * BLOCK];
        bed[channel] = 0.2;
        let mut actual = [[0.; 2]; BLOCK];
        tracked.reset();
        tracked.render(&bed, reported_tilt(pitch), &mut actual)?;
        let error_at = |reference: &mut Surround, direction| -> Result<f32> {
            reference.reset();
            reference
                .speakers
                .iter_mut()
                .find(|speaker| speaker.0 == channel)
                .unwrap()
                .1 = direction;
            let mut fixed = [[0.; 2]; BLOCK];
            reference.render(&bed, Orientation::default(), &mut fixed)?;
            let energy: f32 = actual.iter().flatten().map(|v| v * v).sum();
            assert!(energy > 1e-8);
            let error: f32 = actual
                .iter()
                .flatten()
                .zip(fixed.iter().flatten())
                .map(|(a, b)| (a - b).powi(2))
                .sum();
            Ok((error / energy).sqrt())
        };
        let [x, y, z] = expected;
        assert!(error_at(&mut reference, expected)? < 0.002);
        assert!(error_at(&mut reference, [x, -y, z])? > 0.01);
    }
    Ok(())
}
