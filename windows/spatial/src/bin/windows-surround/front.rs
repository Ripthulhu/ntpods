use super::{BLOCK, Device, Motion, OUT_FRAME_BYTES, RATE, Result, Surround, stereo_output};

// The only active speaker is front center (channel 2 in the 7.1 bed).
struct Cue {
    seed: u32,
    low: f32,
    frame: usize,
}

impl Cue {
    fn fill(&mut self, input: &mut [f32], total: usize) {
        for frame in input.chunks_exact_mut(8) {
            self.seed ^= self.seed << 13;
            self.seed ^= self.seed >> 17;
            self.seed ^= self.seed << 5;
            let noise = self.seed as i32 as f32 / i32::MAX as f32;
            self.low += 0.45 * (noise - self.low);
            let phase = self.frame % RATE as usize;
            let burst = phase.min(36_000usize.saturating_sub(phase)).min(1_200) as f32 / 1_200.0;
            let fade = total.saturating_sub(self.frame).min(1_200) as f32 / 1_200.0;
            frame[2] = self.low * 0.08 * burst * fade;
            self.frame += 1;
        }
    }
}

pub(super) fn listen(sink: &Device, seconds: u64, motion: &Motion) -> Result<()> {
    let mut surround = Surround::preferred(8, 0x63f)?;
    let mut cue = Cue {
        seed: 0xc0ffee,
        low: 0.0,
        frame: 0,
    };
    let mut input = [0.0; BLOCK * 8];
    let total = seconds as usize * RATE as usize;
    let mut energy = [[0.0f64; 2]; 3];
    let mut counts = [0usize; 3];
    play_blocks(sink, total, |block, remaining| {
        cue.fill(&mut input, total);
        let orientation = motion.orientation();
        let yaw = orientation.yaw_degrees();
        let bin = if yaw < -15.0 {
            0
        } else if yaw > 15.0 {
            2
        } else {
            1
        };
        surround.render(&input, orientation, block)?;
        for pair in &block[..remaining.min(BLOCK)] {
            for (ch, sample) in pair.iter().enumerate() {
                energy[bin][ch] += f64::from(*sample).powi(2);
            }
            counts[bin] += 1;
        }
        Ok(())
    })?;
    for ((name, samples), [left, right]) in ["left", "center", "right"]
        .into_iter()
        .zip(counts)
        .zip(energy)
    {
        if left > 0.0 && right > 0.0 {
            println!(
                "Head {name}: {samples} output frames, left/right energy {:+.2} dB",
                10.0 * (left / right).log10()
            );
        }
    }
    Ok(())
}

pub(super) fn play_blocks(
    sink: &Device,
    total: usize,
    mut fill: impl FnMut(&mut [[f32; 2]; BLOCK], usize) -> Result<()>,
) -> Result<()> {
    let output = stereo_output(sink)?;
    let render = output.get_audiorenderclient()?;
    let event = output.set_get_eventhandle()?;
    let size = output.get_buffer_size()? as usize;
    let mut bytes = vec![0; size * OUT_FRAME_BYTES];
    let mut block = [[0.0; 2]; BLOCK];
    let mut index = BLOCK;
    let mut written = 0;
    render.write_to_device(&bytes, None)?;
    output.start_stream()?;
    while written < total {
        event.wait_for_event(250)?;
        let frames = (output.get_available_space_in_frames()? as usize).min(total - written);
        if frames == 0 {
            continue;
        }
        let packet = &mut bytes[..frames * OUT_FRAME_BYTES];
        for pair in packet.chunks_exact_mut(OUT_FRAME_BYTES) {
            if index == BLOCK {
                fill(&mut block, total - written)?;
                index = 0;
            }
            for (channel, out) in pair.chunks_exact_mut(4).enumerate() {
                out.copy_from_slice(&block[index][channel].clamp(-1.0, 1.0).to_le_bytes());
            }
            index += 1;
            written += 1;
        }
        render.write_to_device(packet, None)?;
    }
    std::thread::sleep(std::time::Duration::from_secs_f64(
        size as f64 / f64::from(RATE),
    ));
    output.stop_stream()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cue_is_quiet_and_uses_only_front_center() {
        let mut cue = Cue {
            seed: 1,
            low: 0.0,
            frame: 0,
        };
        let mut buf = vec![0.0; 8 * RATE as usize];
        cue.fill(&mut buf, RATE as usize);
        let mut useful = 0;
        for (i, frame) in buf.chunks_exact(8).enumerate() {
            assert!(frame.iter().enumerate().all(|(ch, &v)| if ch == 2 {
                v.abs() <= 0.08
            } else {
                v == 0.0
            }));
            if i >= 36_000 {
                assert_eq!(frame[2], 0.0);
            }
            useful += usize::from(frame[2].abs() > 0.005);
        }
        assert!(useful > 20_000);
    }
}
