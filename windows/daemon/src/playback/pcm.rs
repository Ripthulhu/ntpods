use std::io;
use std::time::{Duration, Instant};

const SAMPLE_RATE: u64 = 48_000;
const SAMPLE_BYTES: usize = 4;
pub const MAX_READ_BYTES: usize = 1_920;
const MAX_FRAME_BYTES: usize = 1_024 * SAMPLE_BYTES;

fn frame_bytes(samples: usize) -> io::Result<usize> {
    match samples {
        480 | 1_024 => Ok(samples * SAMPLE_BYTES),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Unsupported PCM frame size",
        )),
    }
}

pub struct FrameAssembler {
    bytes: [u8; MAX_FRAME_BYTES],
    used: usize,
    frame_bytes: usize,
}

impl FrameAssembler {
    pub fn new(samples_per_frame: usize) -> io::Result<Self> {
        Ok(Self {
            bytes: [0; MAX_FRAME_BYTES],
            used: 0,
            frame_bytes: frame_bytes(samples_per_frame)?,
        })
    }

    pub fn read_limit(&self) -> usize {
        (self.frame_bytes - self.used).min(MAX_READ_BYTES)
    }

    pub fn read_available(
        &mut self,
        mut read: impl FnMut(usize) -> io::Result<Vec<u8>>,
        mut current: impl FnMut() -> io::Result<bool>,
    ) -> io::Result<Option<Vec<u8>>> {
        // Drain one due frame without sleeping between already available pieces.
        for _ in 0..self.frame_bytes.div_ceil(MAX_READ_BYTES) {
            let data = read(self.read_limit())?;
            if !current()? {
                self.reset();
                return Ok(None);
            }
            if data.is_empty() {
                break;
            }
            if let Some(frame) = self.push(&data)? {
                return Ok(Some(frame));
            }
        }
        Ok(None)
    }

    pub fn push(&mut self, bytes: &[u8]) -> io::Result<Option<Vec<u8>>> {
        if bytes.len() % SAMPLE_BYTES != 0 || bytes.len() > self.frame_bytes - self.used {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid stereo PCM block",
            ));
        }
        let end = self.used + bytes.len();
        self.bytes[self.used..end].copy_from_slice(bytes);
        self.used = end;
        if self.used != self.frame_bytes {
            return Ok(None);
        }
        let frame = self.bytes[..self.frame_bytes].to_vec();
        self.used = 0;
        Ok(Some(frame))
    }

    pub fn reset(&mut self) {
        self.bytes.fill(0);
        self.used = 0;
    }
}

pub struct FrameClock {
    origin: Instant,
    samples_per_frame: u64,
    samples_sent: u64,
}

impl FrameClock {
    pub fn new(samples_per_frame: usize, origin: Instant) -> io::Result<Self> {
        frame_bytes(samples_per_frame)?;
        Ok(Self {
            origin,
            samples_per_frame: samples_per_frame as u64,
            samples_sent: 0,
        })
    }

    pub fn deadline(&self) -> Instant {
        // AAC-LC has a fractional 21.333... ms period; rounding each frame would drift.
        self.origin
            + Duration::new(
                self.samples_sent / SAMPLE_RATE,
                ((self.samples_sent % SAMPLE_RATE) * 1_000_000_000 / SAMPLE_RATE) as u32,
            )
    }

    pub fn advance(&mut self) {
        self.samples_sent += self.samples_per_frame;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_reads_keep_sample_order_without_padding() {
        for samples in [480, 1_024] {
            let mut pcm = FrameAssembler::new(samples).unwrap();
            let expected: Vec<u8> = (0..samples * SAMPLE_BYTES * 20)
                .map(|n| (n % 251) as u8)
                .collect();
            let mut encoded = Vec::new();
            let mut consumed = 0;
            let mut tick = 0;
            while consumed < expected.len() {
                assert!(pcm.push(&[]).unwrap().is_none());
                let length = pcm
                    .read_limit()
                    .min((tick % 113 + 1) * SAMPLE_BYTES)
                    .min(expected.len() - consumed);
                assert!(length <= MAX_READ_BYTES);
                if let Some(frame) = pcm.push(&expected[consumed..consumed + length]).unwrap() {
                    assert_eq!(frame.len(), samples * SAMPLE_BYTES);
                    encoded.extend(frame);
                }
                consumed += length;
                tick += 1;
            }
            assert_eq!(encoded, expected);
            for _ in 0..10_000 {
                assert!(pcm.push(&[]).unwrap().is_none());
            }
        }
    }

    #[test]
    fn aac_reads_fit_the_render_ioctl() {
        let mut pcm = FrameAssembler::new(1_024).unwrap();
        assert_eq!(pcm.read_limit(), 1_920);
        assert!(pcm.push(&vec![1; 1_920]).unwrap().is_none());
        assert_eq!(pcm.read_limit(), 1_920);
        assert!(pcm.push(&vec![2; 1_920]).unwrap().is_none());
        assert_eq!(pcm.read_limit(), 256);
        let frame = pcm.push(&[3; 256]).unwrap().unwrap();
        assert_eq!(frame.len(), 4_096);
        assert_eq!(&frame[..1_920], &[1; 1_920]);
        assert_eq!(&frame[1_920..3_840], &[2; 1_920]);
        assert_eq!(&frame[3_840..], &[3; 256]);
        assert_eq!(pcm.read_limit(), 1_920);
    }

    #[test]
    fn available_frame_is_drained_in_one_bounded_pass() {
        for samples in [480, 1_024] {
            let mut pcm = FrameAssembler::new(samples).unwrap();
            let mut reads = Vec::new();
            let mut checks = 0;
            let frame = pcm
                .read_available(
                    |limit| {
                        reads.push(limit);
                        Ok(vec![7; limit])
                    },
                    || {
                        checks += 1;
                        Ok(true)
                    },
                )
                .unwrap()
                .unwrap();
            assert_eq!(frame, vec![7; samples * SAMPLE_BYTES]);
            assert_eq!(
                reads,
                if samples == 480 {
                    vec![1920]
                } else {
                    vec![1920, 1920, 256]
                }
            );
            assert_eq!(checks, reads.len());
            let mut empty_reads = 0;
            assert!(pcm
                .read_available(
                    |_| {
                        empty_reads += 1;
                        Ok(Vec::new())
                    },
                    || Ok(true),
                )
                .unwrap()
                .is_none());
            assert_eq!(empty_reads, 1);
        }
    }

    #[test]
    fn short_reads_yield_and_keep_partial_frame() {
        let mut pcm = FrameAssembler::new(1_024).unwrap();
        let mut reads = 0;
        assert!(pcm
            .read_available(
                |_| {
                    reads += 1;
                    Ok(vec![5; 4])
                },
                || Ok(true),
            )
            .unwrap()
            .is_none());
        assert_eq!(reads, 3);
        let frame = pcm
            .read_available(|limit| Ok(vec![9; limit]), || Ok(true))
            .unwrap()
            .unwrap();
        assert_eq!(&frame[..12], &[5; 12]);
        assert!(frame[12..].iter().all(|byte| *byte == 9));
    }

    #[test]
    fn producer_change_discards_partial_frame_after_each_read() {
        for changed_at in 1..=3 {
            let mut pcm = FrameAssembler::new(1_024).unwrap();
            let mut checks = 0;
            assert!(pcm
                .read_available(
                    |limit| Ok(vec![3; limit]),
                    || {
                        checks += 1;
                        Ok(checks != changed_at)
                    },
                )
                .unwrap()
                .is_none());
            assert_eq!(checks, changed_at);
            let frame = pcm
                .read_available(|limit| Ok(vec![8; limit]), || Ok(true))
                .unwrap()
                .unwrap();
            assert_eq!(frame, vec![8; 4096]);
        }
    }

    #[test]
    fn read_and_generation_errors_stop_the_frame() {
        let mut pcm = FrameAssembler::new(1_024).unwrap();
        assert!(pcm
            .read_available(
                |_| Err(io::Error::other("read failed")),
                || panic!("failed read must not check generation"),
            )
            .is_err());
        assert!(pcm
            .read_available(
                |limit| Ok(vec![1; limit]),
                || Err(io::Error::other("status failed")),
            )
            .is_err());
        assert!(pcm.read_available(|_| Ok(vec![1; 3]), || Ok(true)).is_err());
    }

    #[test]
    fn malformed_reads_leave_partial_audio_intact_and_reset_discards_it() {
        for samples in [480, 1_024] {
            let mut pcm = FrameAssembler::new(samples).unwrap();
            assert!(pcm.push(&[7; 4]).unwrap().is_none());
            assert!(pcm.push(&[1, 2]).is_err());
            assert!(pcm.push(&vec![1; samples * SAMPLE_BYTES]).is_err());
            let frame = pcm
                .push(&vec![9; samples * SAMPLE_BYTES - 4])
                .unwrap()
                .unwrap();
            assert_eq!(&frame[..4], &[7; 4]);
            assert!(frame[4..].iter().all(|byte| *byte == 9));
            assert!(pcm.push(&[7; 4]).unwrap().is_none());
            pcm.reset();
            let frame = pcm.push(&vec![8; samples * SAMPLE_BYTES]).unwrap().unwrap();
            assert!(frame.iter().all(|byte| *byte == 8));
        }
    }

    #[test]
    fn deadlines_follow_sample_count_for_an_hour_without_rounding_drift() {
        for samples in [480, 1_024] {
            let origin = Instant::now();
            let mut clock = FrameClock::new(samples, origin).unwrap();
            let frames = SAMPLE_RATE * 3_600 / samples as u64;
            for frame in 0..=frames {
                let exact_ns =
                    frame as u128 * samples as u128 * 1_000_000_000 / SAMPLE_RATE as u128;
                assert_eq!(clock.deadline().duration_since(origin).as_nanos(), exact_ns);
                clock.advance();
            }
        }
    }

    #[test]
    fn aac_fraction_reaches_exact_64_milliseconds_every_three_frames() {
        let origin = Instant::now();
        let mut clock = FrameClock::new(1_024, origin).unwrap();
        assert_eq!(clock.deadline(), origin);
        for expected in [21_333_333, 42_666_666, 64_000_000] {
            clock.advance();
            assert_eq!(clock.deadline().duration_since(origin).as_nanos(), expected);
        }
    }

    #[test]
    fn unsupported_codec_frame_sizes_are_rejected() {
        for samples in [0, 1, 479, 481, 1_023, 1_025, usize::MAX] {
            assert!(FrameAssembler::new(samples).is_err());
            assert!(FrameClock::new(samples, Instant::now()).is_err());
        }
    }
}
