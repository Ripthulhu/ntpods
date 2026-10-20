use super::{aac_protocol, encoder::Encoder, pcm::SampleFormat, protocol};
use std::{fmt, io, time::Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EldRtpClock {
    Monotonic,
    Samples48000,
    Samples1000,
}

impl fmt::Display for EldRtpClock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Monotonic => "monotonic milliseconds",
            Self::Samples48000 => "48000 Hz sample counter",
            Self::Samples1000 => "1000 Hz sample counter",
        })
    }
}

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
        if preference.variable
            && (self != Self::Eld || !preference.best_effort || preference.kbps != 256)
        {
            return Err(protocol::invalid("Invalid variable bitrate profile"));
        }
        let chosen = if preference.variable {
            256_000
        } else if preference.best_effort {
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
    pub fn selection(self) -> ntpods_ipc::PlaybackMode {
        match self {
            Self::Aac => ntpods_ipc::PlaybackMode::Aac,
            Self::Eld => ntpods_ipc::PlaybackMode::AacEld,
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
    pub fn new(mode: Mode, bitrate: u32, variable: bool) -> io::Result<Self> {
        match mode {
            Mode::Aac => Encoder::start(mode, bitrate, variable).map(Self::Aac),
            Mode::Eld => Encoder::start(mode, bitrate, variable).map(Self::Eld),
        }
    }
    pub fn new_with_format(
        mode: Mode,
        bitrate: u32,
        variable: bool,
        format: SampleFormat,
    ) -> io::Result<Self> {
        match mode {
            Mode::Aac => Encoder::start_with_format(mode, bitrate, variable, format).map(Self::Aac),
            Mode::Eld => Encoder::start_with_format(mode, bitrate, variable, format).map(Self::Eld),
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
        timestamp: u32,
        aus: usize,
        clock: EldRtpClock,
        epoch: Option<Instant>,
    },
}
impl MediaPackets {
    pub fn new(
        encoder: &AudioEncoder,
        eld_aus: usize,
        eld_rtp_clock: EldRtpClock,
        eld_rtp_ssrc: u32,
    ) -> io::Result<Self> {
        protocol::validate_eld_aus(eld_aus)?;
        match encoder {
            AudioEncoder::Aac(encoder) => Ok(Self::Aac {
                writer: aac_protocol::Packetizer::new(encoder.config())?,
                timestamp: 0,
            }),
            AudioEncoder::Eld(_) => Ok(Self::Eld {
                writer: protocol::Packetizer::new(eld_rtp_ssrc),
                pending: Vec::with_capacity(eld_aus),
                timestamp: 0,
                aus: eld_aus,
                clock: eld_rtp_clock,
                epoch: None,
            }),
        }
    }
    pub fn clear(&mut self) {
        if let Self::Eld {
            writer,
            pending,
            timestamp,
            epoch,
            ..
        } = self
        {
            writer.reset_audio();
            pending.clear();
            *timestamp = 0;
            *epoch = None;
        }
    }
    pub fn start(&mut self, at: Instant) {
        self.clear();
        if let Self::Eld { epoch, .. } = self {
            *epoch = Some(at);
        }
    }

    pub fn stamp(&self, packet: &mut [u8], now: Instant) -> io::Result<()> {
        if let Self::Eld {
            clock: EldRtpClock::Monotonic,
            epoch,
            ..
        } = self
        {
            if packet.len() < 12 || packet[..2] != [0x80, 0x60] {
                return Err(protocol::invalid("Invalid AAC-ELD RTP header"));
            }
            let elapsed = epoch
                .and_then(|epoch| now.checked_duration_since(epoch))
                .ok_or_else(|| protocol::invalid("AAC-ELD RTP clock has not started"))?;
            // RTP timestamps wrap at 2^32 milliseconds.
            packet[4..8].copy_from_slice(&(elapsed.as_millis() as u32).to_be_bytes());
        }
        Ok(())
    }
    pub fn push(&mut self, unit: Vec<u8>, mtu: usize) -> io::Result<Vec<Vec<u8>>> {
        match self {
            Self::Aac { writer, timestamp } => {
                let packets = writer.packets(&unit, *timestamp, mtu)?;
                *timestamp = timestamp.wrapping_add(aac_protocol::RTP_TICKS_PER_FRAME);
                Ok(packets)
            }
            Self::Eld {
                writer,
                pending,
                timestamp,
                aus,
                clock,
                ..
            } => {
                pending.push(unit);
                if pending.len() < *aus {
                    return Ok(Vec::new());
                }
                let packet = writer.packet(pending, *timestamp, mtu)?;
                let ticks_per_au = match clock {
                    EldRtpClock::Monotonic => 0,
                    EldRtpClock::Samples48000 => 480,
                    EldRtpClock::Samples1000 => 10,
                };
                *timestamp = timestamp.wrapping_add(pending.len() as u32 * ticks_per_au);
                pending.clear();
                Ok(vec![packet])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn eld_packets(aus: usize, clock: EldRtpClock, ssrc: u32) -> MediaPackets {
        MediaPackets::Eld {
            writer: protocol::Packetizer::new(ssrc),
            pending: Vec::new(),
            timestamp: 0,
            aus,
            clock,
            epoch: None,
        }
    }

    fn eld_batch(packets: &mut MediaPackets, aus: usize) -> Vec<u8> {
        for _ in 1..aus {
            assert!(packets.push(vec![1; 10], 1023).unwrap().is_empty());
        }
        packets.push(vec![2; 20], 1023).unwrap().remove(0)
    }

    #[test]
    fn windows_default_leaves_codec_ownership_to_windows() {
        use ntpods_ipc::PlaybackMode;
        assert_eq!(Mode::from_selection(PlaybackMode::default()), None);
        assert_eq!(Mode::from_selection(PlaybackMode::Aac), Some(Mode::Aac));
        assert_eq!(Mode::from_selection(PlaybackMode::AacEld), Some(Mode::Eld));
        assert_eq!(Mode::Aac.selection(), PlaybackMode::Aac);
        assert_eq!(Mode::Eld.selection(), PlaybackMode::AacEld);
    }
    #[test]
    fn aac_fragments_share_a_timestamp_and_advance_once_per_frame() {
        let mut packets = MediaPackets::Aac {
            writer: aac_protocol::Packetizer::new(&[0x11, 0x90]).unwrap(),
            timestamp: u32::MAX - 10,
        };
        let first = packets.push(vec![0; 256], 48).unwrap();
        assert!(first.len() > 1);
        for packet in &first {
            assert_eq!(&packet[4..8], &(u32::MAX - 10).to_be_bytes());
        }
        packets.start(Instant::now());
        packets.clear();
        let next = packets.push(vec![0; 256], 48).unwrap();
        assert_eq!(
            &next[0][4..8],
            &((u32::MAX - 10).wrapping_add(1920)).to_be_bytes()
        );
        for mut packet in next {
            let original = packet.clone();
            packets.stamp(&mut packet, Instant::now()).unwrap();
            assert_eq!(packet, original);
        }
    }

    #[test]
    fn eld_frame_counts_follow_sample_time_and_reset_with_the_encoder() {
        for aus in [1, 2] {
            for (clock, ticks_per_au) in [
                (EldRtpClock::Samples48000, 480),
                (EldRtpClock::Samples1000, 10),
            ] {
                let mut packets = eld_packets(aus, clock, 0x11223344);
                let batch = |packets: &mut MediaPackets| eld_batch(packets, aus);
                let first = batch(&mut packets);
                assert_eq!(&first[4..8], &0u32.to_be_bytes());
                assert_eq!(&first[12..14], &[0xb0, 1]);
                let second = batch(&mut packets);
                assert_eq!(&second[4..8], &(ticks_per_au * aus as u32).to_be_bytes());
                assert_eq!(&second[2..4], &1u16.to_be_bytes());
                assert_eq!(&second[12..14], &[0xb0, 1 + aus as u8]);
                let mut stamped = second.clone();
                packets.stamp(&mut stamped, Instant::now()).unwrap();
                assert_eq!(stamped, second);
                if aus == 2 {
                    assert!(packets.push(vec![4; 40], 1023).unwrap().is_empty());
                }
                packets.clear();
                let restarted = batch(&mut packets);
                assert_eq!(&restarted[2..4], &2u16.to_be_bytes());
                assert_eq!(&restarted[4..], &first[4..]);
                if let MediaPackets::Eld { timestamp, .. } = &mut packets {
                    *timestamp = u32::MAX - (ticks_per_au - 1);
                }
                let before_wrap = batch(&mut packets);
                assert_eq!(
                    &before_wrap[4..8],
                    &(u32::MAX - (ticks_per_au - 1)).to_be_bytes()
                );
                let wrapped = batch(&mut packets);
                assert_eq!(
                    &wrapped[4..8],
                    &(ticks_per_au * (aus as u32 - 1)).to_be_bytes()
                );
            }
        }
    }

    #[test]
    fn eld_clock_comparison_changes_only_timestamp_bytes() {
        for aus in [1, 2] {
            let mut streams = [EldRtpClock::Samples48000, EldRtpClock::Samples1000]
                .map(|clock| eld_packets(aus, clock, 0x11223344));
            for frame in 0..6 {
                let sample_clock = streams[0].push(vec![frame; 31], 1023).unwrap();
                let millisecond_clock = streams[1].push(vec![frame; 31], 1023).unwrap();
                assert_eq!(sample_clock.len(), millisecond_clock.len());
                for (sample, millis) in sample_clock.iter().zip(&millisecond_clock) {
                    assert_eq!(&sample[..4], &millis[..4]);
                    assert_eq!(&sample[8..], &millis[8..]);
                    let first_frame = (frame as u32 + 1) - aus as u32;
                    assert_eq!(&sample[4..8], &(first_frame * 480).to_be_bytes());
                    assert_eq!(&millis[4..8], &(first_frame * 10).to_be_bytes());
                }
            }
        }
    }

    #[test]
    fn monotonic_stamp_uses_current_submission_time_for_either_group_size() {
        let epoch = Instant::now();
        for aus in [1, 2] {
            let mut packets = eld_packets(aus, EldRtpClock::Monotonic, 0);
            packets.start(epoch);
            for (index, millis) in [274, 293, 334, 354, 394].into_iter().enumerate() {
                let mut packet = eld_batch(&mut packets, aus);
                let before = packet.clone();
                assert_eq!(&packet[4..8], &[0; 4]);
                packets
                    .stamp(&mut packet, epoch + Duration::from_millis(millis))
                    .unwrap();
                assert_eq!(&packet[4..8], &(millis as u32).to_be_bytes());
                assert_eq!(&packet[..4], &before[..4]);
                assert_eq!(&packet[8..], &before[8..]);
                assert_eq!(&packet[8..12], &[0; 4]);
                assert_eq!(&packet[2..4], &(index as u16).to_be_bytes());
                assert_eq!(&packet[12..14], &[0xb0, 1 + (index * aus) as u8]);
            }
        }
    }

    #[test]
    fn new_stream_discards_partial_groups_and_excludes_the_pause_from_its_epoch() {
        let epoch = Instant::now();
        let mut packets = eld_packets(2, EldRtpClock::Monotonic, 0);
        packets.start(epoch);
        let mut first = eld_batch(&mut packets, 2);
        packets
            .stamp(&mut first, epoch + Duration::from_millis(7))
            .unwrap();
        assert!(packets.push(vec![9; 40], 1023).unwrap().is_empty());
        packets.clear();
        assert!(packets
            .stamp(&mut first, epoch + Duration::from_secs(60))
            .is_err());
        let resumed = epoch + Duration::from_secs(120);
        packets.start(resumed);
        let mut next = eld_batch(&mut packets, 2);
        packets
            .stamp(&mut next, resumed + Duration::from_millis(7))
            .unwrap();
        assert_eq!(&next[2..4], &1u16.to_be_bytes());
        assert_eq!(&next[4..], &first[4..]);
    }

    #[test]
    fn monotonic_timestamp_wraps_and_rejects_missing_epoch_or_invalid_header() {
        let epoch = Instant::now();
        let mut packets = eld_packets(1, EldRtpClock::Monotonic, 0);
        let mut packet = eld_batch(&mut packets, 1);
        let original = packet.clone();
        assert!(packets.stamp(&mut packet, epoch).is_err());
        assert_eq!(packet, original);
        packets.start(epoch + Duration::from_millis(1));
        assert!(packets.stamp(&mut packet, epoch).is_err());
        assert_eq!(packet, original);
        packets.start(epoch);
        for elapsed in [u32::MAX as u64, u32::MAX as u64 + 1, u32::MAX as u64 + 275] {
            packets
                .stamp(&mut packet, epoch + Duration::from_millis(elapsed))
                .unwrap();
            assert_eq!(&packet[4..8], &(elapsed as u32).to_be_bytes());
        }
        for mut invalid in [vec![], vec![0x80; 11], vec![0; 12]] {
            let original = invalid.clone();
            assert!(packets.stamp(&mut invalid, epoch).is_err());
            assert_eq!(invalid, original);
        }
    }

    #[test]
    fn source_identifier_comparison_changes_only_ssrc() {
        for clock in [
            EldRtpClock::Monotonic,
            EldRtpClock::Samples48000,
            EldRtpClock::Samples1000,
        ] {
            let mut zero = eld_packets(1, clock, 0);
            let mut legacy = eld_packets(1, clock, 0x11223344);
            for _ in 0..2 {
                let zero = eld_batch(&mut zero, 1);
                let legacy = eld_batch(&mut legacy, 1);
                assert_eq!(&zero[..8], &legacy[..8]);
                assert_eq!(&zero[12..], &legacy[12..]);
                assert_eq!(&zero[8..12], &[0; 4]);
                assert_eq!(&legacy[8..12], &0x11223344u32.to_be_bytes());
            }
        }
    }
}

#[cfg(test)]
mod bitrate_tests {
    use super::*;
    use ntpods_ipc::PlaybackBitrate;
    #[test]
    fn variable_eld_requires_its_full_peak_budget() {
        let variable = PlaybackBitrate {
            variable: true,
            ..PlaybackBitrate::default()
        };
        assert_eq!(Mode::Eld.bitrate(variable, 0).unwrap(), 256_000);
        assert_eq!(Mode::Eld.bitrate(variable, 256_000).unwrap(), 256_000);
        assert!(Mode::Eld.bitrate(variable, 255_999).is_err());
        assert!(Mode::Aac.bitrate(variable, 512_000).is_err());
        for invalid in [
            PlaybackBitrate {
                best_effort: false,
                ..variable
            },
            PlaybackBitrate {
                kbps: 192,
                ..variable
            },
        ] {
            assert!(Mode::Eld.bitrate(invalid, 0).is_err());
        }
    }

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
                        variable: false,
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
                    variable: false,
                    best_effort: false,
                    kbps: 321
                },
                320000
            )
            .is_err());
        assert!(Mode::Eld
            .bitrate(
                PlaybackBitrate {
                    variable: false,
                    best_effort: false,
                    kbps: u32::MAX
                },
                0
            )
            .is_err());
    }
}
