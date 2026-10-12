use super::{aac_protocol, encoder::Encoder, protocol};
use std::io;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Aac,
    Eld,
}

impl Mode {
    pub fn bitrate(
        self,
        preference: ntpods_ipc::PlaybackBitrate,
        peer_limit: u32,
    ) -> io::Result<u32> {
        let (minimum, maximum) = match self {
            Self::Aac => (96_000, 512_000),
            Self::Eld => (128_000, 256_000),
        };
        let ceiling = if peer_limit == 0 {
            maximum
        } else {
            peer_limit.min(maximum)
        } / 1000
            * 1000;
        let chosen = if preference.best_effort {
            ceiling
        } else {
            preference
                .kbps
                .checked_mul(1000)
                .ok_or_else(|| protocol::invalid("Invalid bitrate"))?
        };
        if chosen < minimum || chosen > ceiling {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "The AirPods cannot use this bitrate.",
            ));
        }
        Ok(chosen)
    }
    pub fn from_selection(mode: ntpods_ipc::PlaybackMode) -> Option<Self> {
        match mode {
            ntpods_ipc::PlaybackMode::Windows => None,
            ntpods_ipc::PlaybackMode::Aac => Some(Self::Aac),
            ntpods_ipc::PlaybackMode::AacEld => Some(Self::Eld),
        }
    }
    pub fn frame_samples(self) -> usize {
        match self {
            Self::Aac => aac_protocol::FRAME_SAMPLES,
            Self::Eld => protocol::FRAME_SAMPLES,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Aac => "AAC",
            Self::Eld => "AAC-ELD",
        }
    }
    pub fn source_configuration(self) -> Vec<u8> {
        match self {
            Self::Aac => aac_protocol::configuration(1, aac_protocol::AAC_CAPABILITIES),
            Self::Eld => protocol::configuration(1),
        }
    }
}

pub enum AudioEncoder {
    Aac(Encoder),
    Eld(Encoder),
}
impl AudioEncoder {
    pub fn new(mode: Mode, bitrate: u32) -> io::Result<Self> {
        match mode {
            Mode::Aac => Encoder::start(mode, bitrate).map(Self::Aac),
            Mode::Eld => Encoder::start(mode, bitrate).map(Self::Eld),
        }
    }
    pub fn write(&mut self, pcm: Vec<u8>) -> io::Result<()> {
        match self {
            Self::Aac(encoder) => encoder.write(pcm),
            Self::Eld(encoder) => encoder.write(pcm),
        }
    }
    pub fn read(&mut self) -> io::Result<Option<Vec<u8>>> {
        match self {
            Self::Aac(encoder) => encoder.read(),
            Self::Eld(encoder) => encoder.read(),
        }
    }
}

pub enum MediaPackets {
    Aac {
        writer: aac_protocol::Packetizer,
        timestamp: u32,
    },
    Eld {
        writer: protocol::Packetizer,
        pending: Vec<Vec<u8>>,
    },
}
impl MediaPackets {
    pub fn new(encoder: &AudioEncoder) -> io::Result<Self> {
        match encoder {
            AudioEncoder::Aac(encoder) => Ok(Self::Aac {
                writer: aac_protocol::Packetizer::new(encoder.config())?,
                timestamp: 0,
            }),
            AudioEncoder::Eld(_) => Ok(Self::Eld {
                writer: protocol::Packetizer::default(),
                pending: Vec::with_capacity(3),
            }),
        }
    }
    pub fn clear(&mut self) {
        if let Self::Eld { pending, .. } = self {
            pending.clear();
        }
    }
    pub fn push(&mut self, unit: Vec<u8>, elapsed_ms: u32, mtu: usize) -> io::Result<Vec<Vec<u8>>> {
        match self {
            Self::Aac { writer, timestamp } => {
                let packets = writer.packets(&unit, *timestamp, mtu)?;
                *timestamp = timestamp.wrapping_add(aac_protocol::RTP_TICKS_PER_FRAME);
                Ok(packets)
            }
            Self::Eld { writer, pending } => {
                pending.push(unit);
                if pending.len() < 3 {
                    return Ok(Vec::new());
                }
                let packet = writer.packet(pending, elapsed_ms, mtu)?;
                pending.clear();
                Ok(vec![packet])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_default_leaves_codec_ownership_to_windows() {
        use ntpods_ipc::PlaybackMode;
        assert_eq!(Mode::from_selection(PlaybackMode::default()), None);
        assert_eq!(Mode::from_selection(PlaybackMode::Aac), Some(Mode::Aac));
        assert_eq!(Mode::from_selection(PlaybackMode::AacEld), Some(Mode::Eld));
    }
    #[test]
    fn aac_fragments_share_a_timestamp_and_advance_once_per_frame() {
        let mut packets = MediaPackets::Aac {
            writer: aac_protocol::Packetizer::new(&[0x11, 0x90]).unwrap(),
            timestamp: u32::MAX - 10,
        };
        let first = packets.push(vec![0; 256], 0, 48).unwrap();
        assert!(first.len() > 1);
        for packet in &first {
            assert_eq!(&packet[4..8], &(u32::MAX - 10).to_be_bytes());
        }
        let next = packets.push(vec![0; 256], 0, 48).unwrap();
        assert_eq!(
            &next[0][4..8],
            &((u32::MAX - 10).wrapping_add(1920)).to_be_bytes()
        );
    }
}

#[cfg(test)]
mod bitrate_tests {
    use super::*;
    use ntpods_ipc::PlaybackBitrate;
    #[test]
    fn bitrate_respects_peer_limits_without_clamping_manual_choices() {
        let best = PlaybackBitrate::default();
        assert_eq!(Mode::Aac.bitrate(best, 0).unwrap(), 512000);
        assert_eq!(Mode::Eld.bitrate(best, 0).unwrap(), 256000);
        assert_eq!(Mode::Aac.bitrate(best, 255900).unwrap(), 255000);
        assert_eq!(Mode::Eld.bitrate(best, 192400).unwrap(), 192000);
        assert!(Mode::Eld.bitrate(best, 127999).is_err());
        assert_eq!(
            Mode::Aac
                .bitrate(
                    PlaybackBitrate {
                        best_effort: false,
                        kbps: 320
                    },
                    512000
                )
                .unwrap(),
            320000
        );
        assert!(Mode::Aac
            .bitrate(
                PlaybackBitrate {
                    best_effort: false,
                    kbps: 321
                },
                320000
            )
            .is_err());
        assert!(Mode::Eld
            .bitrate(
                PlaybackBitrate {
                    best_effort: false,
                    kbps: u32::MAX
                },
                0
            )
            .is_err());
    }
}
