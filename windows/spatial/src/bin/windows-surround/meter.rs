use super::{Io, OUT_FRAME_BYTES, RATE, Result};
use std::time::{Duration, Instant};
use wasapi::{AudioCaptureClient, AudioClient, Device, SampleType, StreamMode};

const DWELL: Duration = Duration::from_millis(300);

#[derive(Default)]
struct Energy {
    frames: usize,
    channels: [f64; 2],
}

impl Energy {
    fn add(&mut self, bytes: &[u8]) {
        let frames = bytes.len() / OUT_FRAME_BYTES;
        let mut sum = [0.0; 2];
        for frame in bytes.chunks_exact(OUT_FRAME_BYTES) {
            for (energy, word) in sum.iter_mut().zip(frame.chunks_exact(4)) {
                let sample = f32::from_le_bytes(word.try_into().unwrap());
                if !sample.is_finite() {
                    return;
                }
                *energy += f64::from(sample).powi(2);
            }
        }
        // Ignore quiet gaps below -80 dBFS RMS in both channels.
        if frames > 0 && sum[0].max(sum[1]) >= 1e-8 * frames as f64 {
            self.frames += frames;
            self.channels[0] += sum[0];
            self.channels[1] += sum[1];
        }
    }
}

pub(super) struct Meter {
    capture: AudioCaptureClient,
    input: AudioClient,
    scratch: Vec<u8>,
    bin: Option<usize>,
    since: Instant,
    last_poll: Instant,
    energy: [Energy; 3],
}

impl Meter {
    pub(super) fn open(sink: &Device) -> Result<Self> {
        if sink.get_direction() != Io::Render {
            return Err("The loopback meter requires a playback endpoint".into());
        }
        let mut input = sink.get_iaudioclient()?;
        let format = input.get_mixformat()?;
        if format.get_samplespersec() != RATE
            || format.get_nchannels() != 2
            || format.get_dwchannelmask() != 3
            || format.get_subformat()? != SampleType::Float
            || format.get_bitspersample() != 32
            || format.get_blockalign() as usize != OUT_FRAME_BYTES
        {
            return Err("The loopback meter requires 48 kHz float32 left/right stereo".into());
        }
        input.initialize_client(
            &format,
            Io::Capture,
            StreamMode::PollingShared {
                autoconvert: false,
                buffer_duration_hns: input.get_device_period()?.0,
            },
            None,
        )?;
        let capture = input.get_audiocaptureclient()?;
        let scratch = vec![0; input.get_buffer_size()? as usize * OUT_FRAME_BYTES];
        input.start_stream()?;
        let now = Instant::now();
        Ok(Self {
            capture,
            input,
            scratch,
            bin: None,
            since: now,
            last_poll: now,
            energy: Default::default(),
        })
    }

    pub(super) fn poll(&mut self, yaw: f32) -> Result<()> {
        let bin = yaw.is_finite().then_some({
            if yaw < -15.0 {
                0
            } else if yaw > 15.0 {
                2
            } else {
                1
            }
        });
        let now = Instant::now();
        if self.bin != bin || now - self.last_poll > Duration::from_millis(100) {
            self.since = now;
        }
        self.bin = bin;
        self.last_poll = now;
        // The render worker must never wait for capture to catch up.
        for _ in 0..16 {
            if self.capture.get_next_packet_size()?.unwrap_or(0) == 0 {
                break;
            }
            let (frames, info) = self.capture.read_from_device(&mut self.scratch)?;
            if frames == 0 {
                break;
            }
            let packet = &mut self.scratch[..frames as usize * OUT_FRAME_BYTES];
            if info.flags.data_discontinuity {
                self.since = now;
            }
            if let Some(index) = bin
                && !info.flags.silent
                && now - self.since >= DWELL
            {
                self.energy[index].add(packet);
            }
            packet.fill(0);
        }
        Ok(())
    }

    pub(super) fn finish(self) -> Result<()> {
        self.input.stop_stream()?;
        for (name, energy) in ["left", "center", "right"].into_iter().zip(self.energy) {
            if energy.frames == 0 {
                println!("Dongle loopback, head {name}: no stable audible frames");
            } else {
                let [left, right] = energy.channels;
                println!(
                    "Dongle loopback, head {name}: {} audible frames, left/right {:+.2} dB",
                    energy.frames,
                    10.0 * (left / right).log10()
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn energy_preserves_stereo_and_skips_silent_or_invalid_packets() {
        let mut energy = Energy::default();
        for pair in [[0.0, 0.0], [1e-6, -1e-6], [f32::NAN, 1.0], [0.25, -0.5]] {
            energy.add(
                &pair
                    .into_iter()
                    .flat_map(f32::to_le_bytes)
                    .collect::<Vec<_>>(),
            );
        }
        assert_eq!(energy.frames, 1);
        assert_eq!(energy.channels, [0.0625, 0.25]);
    }
}
