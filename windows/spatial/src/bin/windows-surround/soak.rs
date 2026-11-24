use super::{CaptureTimeline, PacketAction, Playout, RATE, RenderBed, Result, Surround};

const PERIOD: usize = 480;
const RUN: usize = 10_000; // 100 s at each rate.
const PAUSE: usize = 200;

fn bed_packet(gain: f32, solo: usize) -> Vec<u8> {
    let mut pcm = Vec::with_capacity(PERIOD * 8 * 4);
    for frame in 0..PERIOD {
        // Upper-midrange markers make the HRTF's ear-to-ear level cue measurable.
        for (ch, harmonic) in [37, 41, 7, 1, 11, 13, 17, 19].into_iter().enumerate() {
            let phase = (frame * harmonic) as f32 * std::f32::consts::TAU / PERIOD as f32;
            let sample = gain * if ch == solo { 1.0 } else { 0.04 } * phase.sin();
            pcm.extend_from_slice(&sample.to_le_bytes());
        }
    }
    pcm
}

#[test]
#[ignore = "processes 200 seconds of real 7.1 binaural DSP; run in release mode"]
fn streaming_7_1_across_independent_clocks() -> Result<()> {
    let wall_start = std::time::Instant::now();
    let mut bed = RenderBed::new(Surround::new(8, 0x63f)?, 0x63f);
    let mut play = Playout::new(2 * PERIOD + super::BLOCK, 4096);
    let mut timeline = CaptureTimeline::default();
    let mut out = [[0.0; 2]; PERIOD];
    let left = bed_packet(0.06, 0);
    let right = bed_packet(0.06, 1);
    let quiet = bed_packet(0.000_002, 0);
    let silence = [0; 32 * PERIOD];
    let (mut captured, mut padded, mut resets, mut mask) = (0usize, 0usize, 0, 0);
    let mut packets = [0u64; 2];
    let mut correction = [0.0; 2];
    let mut ear_energy = [[0.0f64; 2]; 2];
    let (mut peak, mut max_depth) = (0.0f32, 0usize);
    let mut last_packet_tick = None;
    let mut drained = 0;
    // The source clock runs fast before the 2 s pause and slow afterwards.
    for tick in 0..(RUN * 2 + PAUSE + 100) {
        let segment = usize::from(tick >= RUN);
        let start = if segment == 0 { 0 } else { RUN + PAUSE };
        let local = tick.saturating_sub(start);
        let active = (start..start + RUN).contains(&tick);
        let factor = [1.000_2, 0.999_8][segment];
        let orientation = super::Orientation::from_yaw(12.0 * (tick as f32 * 0.004).sin());
        if active {
            let due = (((local + 1) as f64) * factor).floor() as u64;
            while packets[segment] < due {
                let n = packets[segment];
                let index = (n + if segment == 0 { 0 } else { packets[0] }) * PERIOD as u64;
                let source_qpc =
                    10_000_000 + start as u64 * 100_000 + (n as f64 * 100_000.0 / factor) as u64;
                let short = segment == 0 && matches!(n, 2 | 5);
                let frames = if short { 96 } else { PERIOD };
                let action =
                    timeline.observe(index, source_qpc, frames as u32, short || n == 0, false);
                let pad = match action {
                    PacketAction::Pad(count) => count as usize,
                    PacketAction::Keep => 0,
                    PacketAction::Reset => {
                        assert_eq!((segment, n, bed.filled, play.depth()), (1, 0, 0, 0));
                        bed.renderer.reset();
                        play.reset();
                        resets += 1;
                        0
                    }
                    PacketAction::Trim(n) => panic!("Unexpected overlap of {n} frames"),
                };
                let pcm: &[u8] = match (segment, local) {
                    (0, 2000..2500) => &quiet,
                    (0, 3000..3500) | (1, 0..100) => &silence,
                    (0, _) => &left,
                    _ => &right,
                };
                if tick % 100 == 0 {
                    let sink = (
                        (tick + 1) as u64 * PERIOD as u64,
                        10_000_000 + (tick + 1) as u64 * 100_000,
                    );
                    play.observe_clocks((index, source_qpc), sink, u64::from(RATE));
                }
                mask |= bed.push(&pcm[..frames * 32], pad, &|| orientation, &mut play)?;
                padded += pad;
                captured += frames;
                packets[segment] += 1;
                last_packet_tick = Some(tick);
            }
        }
        if last_packet_tick.is_some_and(|t| tick > t) && play.depth() < PERIOD + 24 {
            bed.finish(orientation, &mut play)?;
            last_packet_tick = None;
            drained += 1;
        }
        max_depth = max_depth.max(play.depth());
        play.pull(&mut out);
        let mut energy = [0.0; 2];
        for pair in &out {
            for (ch, &value) in pair.iter().enumerate() {
                assert!(value.is_finite());
                peak = peak.max(value.abs());
                energy[ch] += f64::from(value).powi(2);
            }
        }
        let rms = ((energy[0] + energy[1]) / (2 * PERIOD) as f64).sqrt();
        if active
            && (100..RUN - 1).contains(&local)
            && !(segment == 0 && (1990..3510).contains(&local))
            && !(segment == 1 && local < 120)
        {
            assert!(rms > 0.001, "unexpected silent block at {tick}");
            for (total, e) in ear_energy[segment].iter_mut().zip(energy) {
                *total += e;
            }
        }
        if segment == 0 && (2010..2490).contains(&local) {
            assert!((1e-8..1e-5).contains(&rms), "quiet PCM at {tick}: {rms}");
        }
        if (RUN + 20..RUN + PAUSE + 90).contains(&tick)
            || (3010..3490).contains(&tick)
            || tick > RUN * 2 + PAUSE + 20
        {
            assert_eq!(rms, 0.0, "stale audio at {tick}");
        }
        if active && local == RUN - 10 {
            correction[segment] = play.correction_ppm();
        }
    }
    assert_eq!(mask, 0x63f);
    assert_eq!(padded, 768);
    assert_eq!(
        captured + padded,
        (packets[0] + packets[1]) as usize * PERIOD
    );
    assert_eq!((resets, drained, bed.filled, play.depth()), (1, 2, 0, 0));
    assert_eq!((play.stats.underruns, play.stats.discarded), (0, 0));
    assert!(peak < 1.0 && max_depth < 4096);
    let balance = ear_energy.map(|[l, r]| 10.0 * (l / r).log10());
    assert!(
        balance[0] > 3.0 && balance[1] < -3.0,
        "left/right levels {balance:?} dB"
    );
    assert!((correction[0] - 200.0).abs() < 50.0 && (correction[1] + 200.0).abs() < 50.0);
    println!(
        "203 s simulated, {} captured, {padded} gap-filled, {drained} drains, {resets} timeline reset; 0 underruns/discards; peak {peak:.4}, max queue {max_depth}, correction {correction:?} ppm, left/right levels {balance:?} dB; startup silence {}, total fill {}; {:.2} s wall time",
        captured,
        play.stats.startup_silence,
        play.stats.silence,
        wall_start.elapsed().as_secs_f64()
    );
    Ok(())
}
