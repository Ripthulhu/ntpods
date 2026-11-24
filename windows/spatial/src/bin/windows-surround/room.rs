use super::{BLOCK, RATE, Result, Surround};
use std::{fs::File, io::Read, path::Path};

const MAX_SAMPLES: usize = 30 * RATE as usize;
const MAX_BYTES: usize = 2 * MAX_SAMPLES + 4096;
// WAVE 7.1 order: BL, BR, SL, SR are 4, 5, 6, 7. Each delay is in 48 kHz frames.
// The unequal wall distances avoid a symmetric copy of the center source in both ears.
const REFLECTIONS: [(usize, usize, f32); 4] = [
    (6, 408, 0.18),
    (7, 648, 0.16),
    (4, 1128, 0.10),
    (5, 1464, 0.08),
];
const MAX_STEREO_PEAK: f32 = 0.08;

fn pcm(bytes: &[u8]) -> Result<&[u8]> {
    if !(12..=MAX_BYTES).contains(&bytes.len())
        || &bytes[..4] != b"RIFF"
        || &bytes[8..12] != b"WAVE"
        || u32::from_le_bytes(bytes[4..8].try_into()?) as usize != bytes.len() - 8
    {
        return Err("Invalid or oversized RIFF/WAVE speech file".into());
    }
    let mut rest = &bytes[12..];
    let mut format = false;
    let mut data = None;
    while !rest.is_empty() {
        let header = rest.get(..8).ok_or("Truncated WAV chunk header")?;
        let n = u32::from_le_bytes(header[4..8].try_into()?) as usize;
        let body = &rest[8..];
        let chunk = body.get(..n).ok_or("Truncated WAV chunk")?;
        match &header[..4] {
            b"fmt " => {
                const FORMAT: [u8; 16] = [1, 0, 1, 0, 0x80, 0xbb, 0, 0, 0, 0x77, 1, 0, 2, 0, 16, 0];
                if format
                    || !chunk.starts_with(&FORMAT)
                    || !(chunk.len() == 16 || chunk[16..] == [0, 0])
                {
                    return Err("Speech must be mono 48 kHz PCM16 WAV".into());
                }
                format = true;
            }
            b"data" => {
                if data.replace(chunk).is_some() {
                    return Err("Multiple WAV data chunks".into());
                }
            }
            _ => {}
        }
        rest = body.get(n + (n & 1)..).ok_or("Missing WAV pad byte")?;
    }
    let samples = data.ok_or("Missing WAV data")?;
    if !format || samples.len() % 2 != 0 || !(4..=MAX_SAMPLES * 2).contains(&samples.len()) {
        return Err("Expected one PCM format and up to 30 seconds of whole samples".into());
    }
    Ok(samples)
}

pub(super) struct RoomSpeech {
    direct: Vec<f32>,
    damped: Vec<f32>,
    gain: [f32; 2],
}

fn read_wav(path: impl AsRef<Path>) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn voice(bytes: &[u8]) -> Result<Vec<f32>> {
    let bytes = pcm(bytes)?;
    let last = bytes.len() / 2 - 1;
    let mut direct: Vec<_> = bytes
        .chunks_exact(2)
        .enumerate()
        .map(|(i, sample)| {
            let fade = i.min(last - i).min(240) as f32 / 240.0;
            f32::from(i16::from_le_bytes([sample[0], sample[1]])) / 32768.0 * fade
        })
        .collect();
    let peak = direct.iter().copied().map(f32::abs).fold(0.0, f32::max);
    if peak == 0.0 {
        return Err("Speech WAV is silent".into());
    }
    for sample in &mut direct {
        *sample *= 0.12 / peak;
    }
    Ok(direct)
}

pub(super) fn load_voice(path: impl AsRef<Path>) -> Result<Vec<f32>> {
    voice(&read_wav(path)?)
}

impl RoomSpeech {
    pub(super) fn load(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_wav(&read_wav(path)?)
    }

    fn from_wav(bytes: &[u8]) -> Result<Self> {
        let direct = voice(bytes)?;
        // First-order wall absorption on the reflected sound only. Leave the voice itself dry.
        let mut filtered = 0.0;
        let damped = direct
            .iter()
            .copied()
            .chain([0.0; 32])
            .map(|x| {
                filtered += 0.46 * (x - filtered);
                filtered
            })
            .collect();
        let mut speech = Self {
            direct,
            damped,
            gain: [1.0, 1.0],
        };
        let mut renderer = Surround::new(8, 0x63f)?;
        let (dry_energy, dry_peak) = speech.measure(&mut renderer, false)?;
        let (room_energy, room_peak) = speech.measure(&mut renderer, true)?;
        if !(dry_energy > 0.0
            && room_energy > 0.0
            && dry_energy.is_finite()
            && room_energy.is_finite())
        {
            return Err("Could not match dry and room speech levels".into());
        }
        let wet_gain = (dry_energy / room_energy).sqrt() as f32;
        let common = (MAX_STEREO_PEAK / dry_peak.max(room_peak * wet_gain)).min(1.0);
        speech.gain = [common, wet_gain * common];
        Ok(speech)
    }

    pub(super) fn period_frames(&self) -> usize {
        // Complete every reflection and leave a quiet gap before repeating the phrase.
        self.direct.len() + RATE as usize / 2
    }

    pub(super) fn fill(&self, bed: &mut [f32], frame: usize, room: bool) {
        assert_eq!(bed.len() % 8, 0);
        let period = self.period_frames();
        let gain = self.gain[usize::from(room)];
        let mut position = frame % period;
        for speaker in bed.chunks_exact_mut(8) {
            speaker.fill(0.0);
            speaker[2] = self.direct.get(position).copied().unwrap_or(0.0) * gain;
            if room {
                for (ch, delay, level) in REFLECTIONS {
                    if let Some(sample) =
                        position.checked_sub(delay).and_then(|i| self.damped.get(i))
                    {
                        speaker[ch] = sample * level * gain;
                    }
                }
            }
            position += 1;
            if position == period {
                position = 0;
            }
        }
    }

    fn measure(&self, renderer: &mut Surround, room: bool) -> Result<(f64, f32)> {
        renderer.reset();
        let mut bed = [0.0; BLOCK * 8];
        let mut stereo = [[0.0; 2]; BLOCK];
        let mut energy = 0.0;
        let mut peak: f32 = 0.0;
        let end = self.period_frames();
        for frame in (0..end).step_by(BLOCK) {
            self.fill(&mut bed, frame, room);
            renderer.render(&bed, super::Orientation::default(), &mut stereo)?;
            for &sample in stereo[..BLOCK.min(end - frame)].iter().flatten() {
                energy += f64::from(sample).powi(2);
                peak = peak.max(sample.abs());
            }
        }
        Ok((energy, peak))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn wav() -> Vec<u8> {
        let mut bytes = b"RIFF\0\0\0\0WAVEJUNK\x01\0\0\0x\0fmt \x12\0\0\0\x01\0\x01\0\x80\xbb\0\0\0\x77\x01\0\x02\0\x10\0\0\0data".to_vec();
        let count: u32 = 9600;
        bytes.extend((count * 2).to_le_bytes());
        for i in 0..count {
            let value = ((i as f32 * 0.079).sin() * 12_000.0) as i16;
            bytes.extend(value.to_le_bytes());
        }
        let length = bytes.len() as u32 - 8;
        bytes[4..8].copy_from_slice(&length.to_le_bytes());
        bytes
    }

    #[test]
    fn wav_requires_matching_sizes_whole_samples_and_pcm_mono() {
        let original = wav();
        assert_eq!(pcm(&original).unwrap().len(), 19200);
        assert!(pcm(&original[..6]).is_err());
        assert!(pcm(&original[..original.len() - 1]).is_err());
        let mut wav = original.clone();
        wav[32] = 2; // channel count
        assert!(pcm(&wav).is_err());
        let mut wav = original;
        wav[52..56].copy_from_slice(&19199u32.to_le_bytes());
        assert!(pcm(&wav).is_err());
    }

    #[test]
    fn fixed_room_has_delayed_sides_rears_matched_stereo_energy_and_headroom() -> Result<()> {
        let speech = RoomSpeech::from_wav(&wav())?;
        let mut dry = vec![0.0; speech.period_frames() * 8];
        let mut room = dry.clone();
        speech.fill(&mut dry, 0, false);
        speech.fill(&mut room, 0, true);
        for f in &dry[..8] {
            assert_eq!(*f, 0.0);
        }
        for f in dry.chunks_exact(8) {
            assert!(f.iter().enumerate().all(|(ch, &v)| ch == 2 || v == 0.0));
        }
        for (ch, delay, _) in REFLECTIONS {
            assert!(room[..delay * 8].chunks_exact(8).all(|f| f[ch] == 0.0));
            assert!(room[(delay + 1) * 8 + ch] != 0.0);
        }
        assert!(room[room.len() - BLOCK * 8..].iter().all(|x| *x == 0.0));
        let mut renderer = Surround::new(8, 0x63f)?;
        let (dry_energy, dry_peak) = speech.measure(&mut renderer, false)?;
        let (room_energy, room_peak) = speech.measure(&mut renderer, true)?;
        assert!((10.0 * (dry_energy / room_energy).log10()).abs() < 0.05);
        assert!(dry_peak.max(room_peak) <= MAX_STEREO_PEAK + 1e-5);
        Ok(())
    }
}
