//! Standard A2DP AAC-LC: in-band LATM configuration and RFC 6416 fragmentation.
use super::protocol::invalid;
use std::io;

pub const FRAME_SAMPLES: usize = 1024;
pub const BITRATE: u32 = 256_000;
pub const MAX_AU: usize = 1536;
pub const RTP_TICKS_PER_FRAME: u32 = FRAME_SAMPLES as u32 * 90_000 / 48_000;
pub const AAC_CAPABILITIES: [u8; 6] = [0xc0, 0, 0x84, 0x07, 0xd0, 0];
const ASC: [u8; 2] = [0x11, 0x90]; // AAC-LC, 48 kHz, stereo, 1024 samples.

pub fn select_codec(capabilities: &[u8]) -> io::Result<Option<[u8; 6]>> {
    select_codec_at(
        capabilities,
        ntpods_ipc::PlaybackBitrate {
            best_effort: false,
            kbps: 256,
        },
    )
    .map(|selected| selected.map(|(codec, _)| codec))
}

pub fn select_codec_at(
    mut capabilities: &[u8],
    preference: ntpods_ipc::PlaybackBitrate,
) -> io::Result<Option<([u8; 6], u32)>> {
    let mut transport = false;
    let mut codec = None;
    let mut seen = 0u16;
    while !capabilities.is_empty() {
        if capabilities.len() < 2 || capabilities.len() < 2 + capabilities[1] as usize {
            return Err(invalid("Truncated AVDTP capability"));
        }
        let category = capabilities[0];
        let value = &capabilities[2..2 + capabilities[1] as usize];
        capabilities = &capabilities[2 + capabilities[1] as usize..];
        if category == 0 || (category <= 8 && seen & (1 << category) != 0) {
            return Err(invalid("Invalid or duplicate AVDTP service category"));
        }
        if category <= 8 {
            seen |= 1 << category;
        }
        match category {
            1 => {
                if !value.is_empty() {
                    return Err(invalid("Invalid media transport capability"));
                }
                transport = true;
            }
            7 => {
                if value.len() < 2 || (value[1] == 2 && value.len() != 8) {
                    return Err(invalid("Invalid AAC codec capability"));
                }
                if value[0] != 0 || value[1] != 2 {
                    continue;
                }
                let data = &value[2..];
                let maximum = u32::from_be_bytes([0, data[3] & 0x7f, data[4], data[5]]);
                let object = if data[0] & 0x80 != 0 {
                    0x80
                } else {
                    data[0] & 0x40
                };
                if object != 0 && data[2] & 0x84 == 0x84 {
                    let Ok(bitrate) = super::codec::Mode::Aac.bitrate(preference, maximum) else {
                        continue;
                    };
                    // The encoder disables MPEG-4-only tools, so either LC object type works.
                    let rate = bitrate.to_be_bytes();
                    codec = Some(([object, 0, 0x84, rate[1], rate[2], rate[3]], bitrate));
                }
            }
            _ => {}
        }
    }
    Ok(if transport { codec } else { None })
}

pub fn configuration(seid: u8, codec: [u8; 6]) -> Vec<u8> {
    let mut bytes = vec![seid << 2, 4, 1, 0, 7, 8, 0, 2];
    bytes.extend_from_slice(&codec);
    bytes
}

struct Bits {
    bytes: Vec<u8>,
    count: usize,
}

impl Bits {
    fn put(&mut self, value: u32, count: usize) {
        for shift in (0..count).rev() {
            if self.count % 8 == 0 {
                self.bytes.push(0);
            }
            self.bytes[self.count / 8] |= (((value >> shift) & 1) as u8) << (7 - self.count % 8);
            self.count += 1;
        }
    }
}

pub struct Packetizer {
    sequence: u16,
}

impl Packetizer {
    pub fn new(asc: &[u8]) -> io::Result<Self> {
        // FFmpeg also appends a sync extension explicitly declaring SBR absent.
        if asc != ASC && asc != [0x11, 0x90, 0x56, 0xe5, 0] {
            return Err(invalid(
                "Expected 48 kHz stereo AAC-LC with 1024-sample frames",
            ));
        }
        Ok(Self { sequence: 0 })
    }

    pub fn packets(
        &mut self,
        au: &[u8],
        timestamp90k: u32,
        mtu: usize,
    ) -> io::Result<Vec<Vec<u8>>> {
        if au.is_empty() || au.len() > MAX_AU || !(48..=8192).contains(&mtu) {
            return Err(invalid("Invalid AAC frame or media MTU"));
        }
        let mut mux = Bits {
            bytes: Vec::with_capacity(au.len() + 16),
            count: 0,
        };
        mux.put(0, 1); // useSameStreamMux: repeat configuration after packet loss or suspend.
        mux.put(0, 1); // audioMuxVersion.
        mux.put(1, 1); // allStreamsSameTimeFraming.
        mux.put(0, 13); // numSubFrames, numProgram, numLayer.
        mux.put(u16::from_be_bytes(ASC) as u32, 16);
        mux.put(0, 3); // frameLengthType.
        mux.put(0xff, 8); // latmBufferFullness.
        mux.put(0, 2); // otherDataPresent, crcCheckPresent.
        let mut remaining = au.len();
        while remaining >= 255 {
            mux.put(255, 8);
            remaining -= 255;
        }
        mux.put(remaining as u32, 8);
        for &byte in au {
            mux.put(byte as u32, 8);
        }

        let chunks = mux.bytes.chunks(mtu - 12);
        let count = chunks.len();
        let mut packets = Vec::with_capacity(count);
        for (index, chunk) in chunks.enumerate() {
            let mut packet = Vec::with_capacity(12 + chunk.len());
            packet.extend_from_slice(&[0x80, 0x60 | if index + 1 == count { 0x80 } else { 0 }]);
            packet.extend_from_slice(&self.sequence.to_be_bytes());
            packet.extend_from_slice(&timestamp90k.to_be_bytes());
            packet.extend_from_slice(&1u32.to_be_bytes());
            packet.extend_from_slice(chunk);
            packets.push(packet);
            self.sequence = self.sequence.wrapping_add(1);
        }
        Ok(packets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODEC: [u8; 6] = [0x80, 0, 0x84, 0x03, 0xe8, 0];

    #[test]
    fn negotiates_lc_48k_stereo_with_a_sufficient_or_unspecified_bitrate() {
        assert_eq!(
            select_codec(&configuration(2, AAC_CAPABILITIES)[2..]).unwrap(),
            Some(CODEC)
        );
        let mut caps = configuration(2, CODEC)[2..].to_vec();
        assert_eq!(select_codec(&caps).unwrap(), Some(CODEC));
        caps[6] = 0xc0;
        caps[7] = 0xff;
        caps[8] = 0xff;
        caps[9] = 0x80;
        caps[10] = 0;
        assert_eq!(select_codec(&caps).unwrap(), Some(CODEC));
        caps[6] = 0x40;
        assert_eq!(select_codec(&caps).unwrap().unwrap()[0], 0x40);
        for (offset, value) in [(6, 0x20), (8, 0x74), (8, 0x88), (9, 1)] {
            let mut bad = caps.clone();
            bad[offset] = value;
            assert_eq!(select_codec(&bad).unwrap(), None);
        }
    }

    #[test]
    fn rejects_incomplete_duplicate_and_non_audio_capabilities() {
        let caps = configuration(2, CODEC)[2..].to_vec();
        for size in 1..caps.len() {
            assert!(!matches!(select_codec(&caps[..size]), Ok(Some(_))));
        }
        assert_eq!(select_codec(&caps[2..]).unwrap(), None);
        assert!(select_codec(&[caps.clone(), vec![1, 0]].concat()).is_err());
        assert!(select_codec(&[caps.clone(), caps[2..].to_vec()].concat()).is_err());
        let mut video = caps.clone();
        video[4] = 0x10;
        assert_eq!(select_codec(&video).unwrap(), None);
        let mut extra = caps;
        extra[3] += 1;
        extra.push(0);
        assert!(select_codec(&extra).is_err());
    }

    #[test]
    fn validates_the_entire_audio_specific_configuration() {
        assert!(Packetizer::new(&ASC).is_ok());
        assert!(Packetizer::new(&[0x11, 0x90, 0x56, 0xe5, 0]).is_ok());
        for asc in [
            &[][..],
            &[0x11][..],
            &[0x12, 0x10][..],
            &[0x11, 0x94][..],
            &[0x11, 0x90, 0][..],
            &[0x11, 0x90, 0x56, 0xe5, 0x80][..],
        ] {
            assert!(Packetizer::new(asc).is_err());
        }
    }

    #[test]
    fn matches_fdk_latm_version_zero_fixture() {
        // FDK cac0447: MPEG-2 LC, 48 kHz stereo, LATM MCP1, header period 1.
        let au = [0x21, 0x11, 0x45, 0, 0x14, 0x50, 1, 0x47];
        let expected = [
            0x20, 0, 0x11, 0x90, 0x1f, 0xe0, 0x41, 8, 0x8a, 0x28, 0, 0xa2, 0x80, 0x0a, 0x38,
        ];
        let packets = Packetizer::new(&ASC).unwrap().packets(&au, 0, 672).unwrap();
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0][1], 0xe0);
        assert_eq!(&packets[0][12..], expected);
    }

    #[test]
    fn preserves_payload_across_length_escape_boundaries() {
        for length in [1, 254, 255, 256, 510, 674, MAX_AU] {
            let au: Vec<_> = (0..length).map(|index| index as u8).collect();
            let packet = Packetizer::new(&ASC)
                .unwrap()
                .packets(&au, 0, 8192)
                .unwrap()
                .remove(0);
            let data = &packet[12..];
            let byte = |bit: usize| {
                (0..8).fold(0u8, |value, n| {
                    (value << 1) | ((data[(bit + n) / 8] >> (7 - (bit + n) % 8)) & 1)
                })
            };
            let mut bit = 45;
            let mut decoded_length = 0;
            loop {
                let value = byte(bit);
                bit += 8;
                decoded_length += value as usize;
                if value != 255 {
                    break;
                }
            }
            assert_eq!(decoded_length, length);
            assert_eq!(
                (0..length).map(|i| byte(bit + i * 8)).collect::<Vec<_>>(),
                au
            );
            assert_eq!(data.last().unwrap() & 7, 0);
        }
    }

    #[test]
    fn fragments_one_element_with_last_marker_same_timestamp_and_wrapping_sequence() {
        let au = vec![0xa5; 682];
        let mut whole = Packetizer::new(&ASC).unwrap();
        let expected = whole.packets(&au, 1920, 8192).unwrap().remove(0);
        let mut fragmented = Packetizer::new(&ASC).unwrap();
        fragmented.sequence = u16::MAX;
        let packets = fragmented.packets(&au, 1920, 335).unwrap();
        assert_eq!(packets.len(), 3);
        for (index, packet) in packets.iter().enumerate() {
            assert!(packet.len() <= 335);
            assert_eq!(packet[0], 0x80);
            assert_eq!(packet[1], if index == 2 { 0xe0 } else { 0x60 });
            assert_eq!(
                &packet[2..4],
                &u16::MAX.wrapping_add(index as u16).to_be_bytes()
            );
            assert_eq!(&packet[4..8], &1920u32.to_be_bytes());
            assert_eq!(&packet[8..12], &[0, 0, 0, 1]);
        }
        let payload: Vec<_> = packets
            .iter()
            .flat_map(|packet| packet[12..].iter().copied())
            .collect();
        assert_eq!(payload, expected[12..]);
        let next = fragmented
            .packets(&au, 1920 + RTP_TICKS_PER_FRAME, 8192)
            .unwrap();
        assert_eq!(&next[0][2..4], &[0, 2]);
        assert_eq!(&next[0][4..8], &3840u32.to_be_bytes());
        assert_eq!(&next[0][12..], &expected[12..]);
    }

    #[test]
    fn invalid_input_does_not_advance_the_sequence() {
        let mut packetizer = Packetizer::new(&ASC).unwrap();
        for (au, mtu) in [
            (vec![], 672),
            (vec![0; MAX_AU + 1], 672),
            (vec![0], 12),
            (vec![0], 47),
            (vec![0], 8193),
        ] {
            assert!(packetizer.packets(&au, 0, mtu).is_err());
            assert_eq!(packetizer.sequence, 0);
        }
    }
}
