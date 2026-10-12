use super::{aac::AacEncoder, aac_protocol, encoder::Encoder, protocol};
use std::io;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Aac,
    Eld,
}

impl Mode {
    pub fn requested(args: impl Iterator<Item = String>) -> io::Result<Option<Self>> {
        let mut mode = None;
        for arg in args.filter(|arg| arg.starts_with("--experimental-playback")) {
            let selected = match arg.as_str() {
                "--experimental-playback" | "--experimental-playback=aac" => Self::Aac,
                "--experimental-playback=eld" => Self::Eld,
                _ => {
                    return Err(protocol::invalid(
                        "Use --experimental-playback=aac or --experimental-playback=eld",
                    ))
                }
            };
            if mode.replace(selected).is_some() {
                return Err(protocol::invalid("Choose one playback codec"));
            }
        }
        Ok(mode)
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
    Aac(AacEncoder),
    Eld(Encoder),
}
impl AudioEncoder {
    pub fn new(mode: Mode) -> io::Result<Self> {
        match mode {
            Mode::Aac => AacEncoder::new().map(Self::Aac),
            Mode::Eld => Encoder::start().map(Self::Eld),
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
    fn parse(args: &[&str]) -> io::Result<Option<Mode>> {
        Mode::requested(args.iter().map(|s| s.to_string()))
    }
    #[test]
    fn normal_aac_is_the_explicit_playback_default() {
        assert_eq!(parse(&[]).unwrap(), None);
        assert_eq!(
            parse(&["--experimental-playback"]).unwrap(),
            Some(Mode::Aac)
        );
        assert_eq!(
            parse(&["--experimental-playback=eld"]).unwrap(),
            Some(Mode::Eld)
        );
        assert!(parse(&["--experimental-playback=lossless"]).is_err());
        assert!(parse(&["--experimental-playback=aac", "--experimental-playback=eld"]).is_err());
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
