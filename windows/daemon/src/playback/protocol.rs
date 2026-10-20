use std::io;

pub const DISCOVER: u8 = 1;
pub const GET_CAPABILITIES: u8 = 2;
pub const SET_CONFIGURATION: u8 = 3;
pub const GET_CONFIGURATION: u8 = 4;
pub const RECONFIGURE: u8 = 5;
pub const OPEN: u8 = 6;
pub const START: u8 = 7;
pub const CLOSE: u8 = 8;
pub const SUSPEND: u8 = 9;
pub const ABORT: u8 = 10;
pub const GET_ALL_CAPABILITIES: u8 = 12;
pub const DELAY_REPORT: u8 = 13;
pub const DELAY_REPORTING: u8 = 8;
pub const APPLE_ELD: [u8; 14] = [0x4c, 0, 0, 0, 1, 0x80, 0, 0x80, 0, 0x84, 0, 0x83, 0xe8, 0];
pub const FRAME_SAMPLES: usize = 480;
pub const MAX_AU: usize = 321;

pub fn validate_eld_aus(count: usize) -> io::Result<()> {
    if !matches!(count, 1 | 2) {
        return Err(invalid("AAC-ELD packets must contain one or two frames"));
    }
    Ok(())
}

pub fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub fn command(label: u8, signal: u8, payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![(label & 15) << 4, signal];
    bytes.extend_from_slice(payload);
    bytes
}

#[derive(Debug)]
struct Rejection {
    signal: u8,
    code: Option<u8>,
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.code {
            Some(code) => write!(
                f,
                "AirPods rejected AVDTP request {:#04x} ({code:#04x})",
                self.signal
            ),
            None => f.write_str("AirPods do not support the AVDTP request"),
        }
    }
}

impl std::error::Error for Rejection {}

pub fn request_not_supported(error: &io::Error, signal: u8) -> bool {
    error
        .get_ref()
        .and_then(|error| error.downcast_ref::<Rejection>())
        .is_some_and(|rejection| {
            rejection.signal == signal && matches!(rejection.code, None | Some(0x19))
        })
}

pub fn response(packet: &[u8], label: u8, signal: u8) -> io::Result<Option<&[u8]>> {
    if packet.len() < 2 || packet[0] & 0x0c != 0 || packet[1] & 0xc0 != 0 {
        return Err(invalid("Fragmented or truncated AVDTP response"));
    }
    if packet[0] >> 4 != label || packet[1] & 0x3f != signal {
        return Ok(None);
    }
    match packet[0] & 3 {
        2 => {
            if matches!(
                signal,
                SET_CONFIGURATION | RECONFIGURE | OPEN | START | CLOSE | SUSPEND | ABORT
            ) && packet.len() != 2
            {
                return Err(invalid("Unexpected AVDTP acceptance payload"));
            }
            Ok(Some(&packet[2..]))
        }
        1 if packet.len() == 2 => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            Rejection { signal, code: None },
        )),
        3 => {
            let expected = if matches!(signal, SET_CONFIGURATION | RECONFIGURE | START | SUSPEND) {
                4
            } else {
                3
            };
            if packet.len() != expected {
                return Err(invalid("Invalid AVDTP rejection payload"));
            }
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                Rejection {
                    signal,
                    code: Some(packet[expected - 1]),
                },
            ))
        }
        1 => Err(invalid("Invalid AVDTP general rejection")),
        _ => Ok(None),
    }
}

pub fn supports_delay_reporting(mut payload: &[u8]) -> io::Result<bool> {
    let mut supported = false;
    while !payload.is_empty() {
        if payload.len() < 2 || payload.len() < 2 + payload[1] as usize {
            return Err(invalid("Truncated AVDTP capability"));
        }
        if payload[0] == DELAY_REPORTING {
            if supported || payload[1] != 0 {
                return Err(invalid("Invalid delay reporting capability"));
            }
            supported = true;
        }
        payload = &payload[2 + payload[1] as usize..];
    }
    Ok(supported)
}

pub fn sinks(payload: &[u8]) -> io::Result<Vec<u8>> {
    if payload.is_empty() || payload.len() % 2 != 0 || payload.len() > 124 {
        return Err(invalid("Invalid AVDTP endpoint list"));
    }
    let mut result = Vec::new();
    for entry in payload.chunks_exact(2) {
        let seid = entry[0] >> 2;
        if seid == 0 || seid == 63 || entry[0] & 1 != 0 || entry[1] & 7 != 0 {
            return Err(invalid("Invalid AVDTP endpoint"));
        }
        if entry[0] & 2 == 0 && entry[1] == 8 && !result.contains(&seid) {
            result.push(seid);
        }
    }
    Ok(result)
}

pub fn supports_apple_eld(payload: &[u8]) -> io::Result<bool> {
    Ok(select_apple_eld(
        payload,
        ntpods_ipc::PlaybackBitrate {
            variable: false,
            best_effort: false,
            kbps: 256,
        },
    )?
    .is_some())
}

pub fn select_apple_eld(
    mut payload: &[u8],
    preference: ntpods_ipc::PlaybackBitrate,
) -> io::Result<Option<u32>> {
    supports_delay_reporting(payload)?;
    let mut media_transport = false;
    let mut codec = None;
    let mut seen = 0u16;
    while !payload.is_empty() {
        if payload.len() < 2 || payload.len() < 2 + payload[1] as usize {
            return Err(invalid("Truncated AVDTP capability"));
        }
        let category = payload[0];
        let value = &payload[2..2 + payload[1] as usize];
        payload = &payload[2 + payload[1] as usize..];
        if category == 0 || (category <= 8 && seen & (1 << category) != 0) {
            return Err(invalid("Invalid or duplicate AVDTP service category"));
        }
        if category <= 8 {
            seen |= 1 << category;
        }
        if category == 1 {
            if !value.is_empty() {
                return Err(invalid("Invalid media transport capability"));
            }
            media_transport = true;
        }
        if category == 7 && (value.len() < 2 || (value[1] == 0xff && value.len() < 8)) {
            return Err(invalid("Truncated AVDTP media codec"));
        }
        if category == 7 && value.len() >= 8 && value[..8] == [0, 0xff, 0x4c, 0, 0, 0, 1, 0x80] {
            if value.len() != 16 {
                return Err(invalid("Invalid Apple codec capability"));
            }
            let data = &value[2..];
            let bitrate = u32::from_be_bytes([0, data[11] & 0x7f, data[12], data[13]]);
            if data[7] & 0x80 != 0 && data[9] & 0x84 == 0x84 && data[11] & 0x80 != 0 {
                codec = super::codec::Mode::Eld.bitrate(preference, bitrate).ok();
            }
        }
    }
    Ok(if media_transport { codec } else { None })
}

pub fn configuration(seid: u8) -> Vec<u8> {
    configuration_at(seid, 256_000)
}

pub fn configuration_at(seid: u8, bitrate: u32) -> Vec<u8> {
    let mut data = vec![seid << 2, 1 << 2, 1, 0, 7, 16, 0, 0xff];
    data.extend_from_slice(&APPLE_ELD);
    let end = data.len();
    let bytes = bitrate.to_be_bytes();
    data[end - 3] = 0x80 | bytes[1];
    data[end - 2] = bytes[2];
    data[end - 1] = bytes[3];
    data
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamState {
    Idle,
    Configured,
    Open,
    Streaming,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeerAction {
    None,
    Start,
    Suspend,
    Close,
    DelayReport(u16),
}

pub fn peer_response(
    packet: &[u8],
    state: StreamState,
    capabilities: &[u8],
    delay_reporting: bool,
) -> io::Result<Option<(Vec<u8>, PeerAction)>> {
    if packet.len() < 2 || packet[0] & 0x0c != 0 || packet[1] & 0xc0 != 0 {
        return Err(invalid("Invalid or fragmented AVDTP packet"));
    }
    if packet[0] & 3 != 0 {
        return Ok(None);
    }
    let signal = packet[1];
    let header = packet[0] & 0xf0;
    if !(DISCOVER..=DELAY_REPORT).contains(&signal) {
        return Ok(Some((vec![header | 1, signal], PeerAction::None)));
    }
    let reject = |error, seid| {
        let mut response = vec![header | 3, signal];
        if matches!(signal, SET_CONFIGURATION | RECONFIGURE) {
            response.push(0);
        }
        if matches!(signal, START | SUSPEND) {
            response.push(seid);
        }
        response.push(error);
        Ok(Some((response, PeerAction::None)))
    };
    let valid_length = match signal {
        DISCOVER => packet.len() == 2,
        SET_CONFIGURATION => packet.len() >= 4,
        START | SUSPEND | RECONFIGURE | 11 => packet.len() >= 3,
        DELAY_REPORT => packet.len() == 5,
        _ => packet.len() == 3,
    };
    if !valid_length {
        return reject(0x11, packet.get(2).copied().unwrap_or(0));
    }
    if signal != DISCOVER {
        let seids = if matches!(signal, START | SUSPEND) {
            &packet[2..]
        } else {
            &packet[2..3]
        };
        if let Some(&seid) = seids.iter().find(|&&seid| seid != 4) {
            return reject(0x12, seid);
        }
        if seids.len() > 1 {
            return reject(0x12, seids[1]);
        }
    }
    let mut reply = vec![header | 2, signal];
    let action = match signal {
        DISCOVER => {
            reply.extend_from_slice(&[4 | if state == StreamState::Idle { 0 } else { 2 }, 0]);
            PeerAction::None
        }
        GET_CAPABILITIES | GET_ALL_CAPABILITIES => {
            supports_delay_reporting(capabilities)?;
            let mut remaining = capabilities;
            while !remaining.is_empty() {
                let length = 2 + remaining[1] as usize;
                if remaining[0] != DELAY_REPORTING {
                    reply.extend_from_slice(&remaining[..length]);
                }
                remaining = &remaining[length..];
            }
            if signal == GET_ALL_CAPABILITIES {
                reply.extend_from_slice(&[DELAY_REPORTING, 0]);
            }
            PeerAction::None
        }
        GET_CONFIGURATION if state != StreamState::Idle => {
            reply.extend_from_slice(capabilities);
            PeerAction::None
        }
        START if state == StreamState::Open => PeerAction::Start,
        SUSPEND if state == StreamState::Streaming => PeerAction::Suspend,
        CLOSE if matches!(state, StreamState::Open | StreamState::Streaming) => PeerAction::Close,
        ABORT => PeerAction::Close,
        DELAY_REPORT if state == StreamState::Idle => return reject(0x31, 4),
        DELAY_REPORT if delay_reporting => {
            PeerAction::DelayReport(u16::from_be_bytes([packet[3], packet[4]]))
        }
        GET_CONFIGURATION | START | SUSPEND | CLOSE => return reject(0x31, 4),
        _ => return reject(0x19, 4),
    };
    Ok(Some((reply, action)))
}

pub struct Packetizer {
    sequence: u16,
    au_sequence: u16,
    ssrc: u32,
}

impl Default for Packetizer {
    fn default() -> Self {
        Self::new(0)
    }
}

impl Packetizer {
    pub fn new(ssrc: u32) -> Self {
        Self {
            sequence: 0,
            au_sequence: 1,
            ssrc,
        }
    }

    pub fn reset_audio(&mut self) {
        self.au_sequence = 1;
    }

    pub fn packet(&mut self, units: &[Vec<u8>], timestamp: u32, mtu: usize) -> io::Result<Vec<u8>> {
        validate_eld_aus(units.len())?;
        if units.iter().any(|au| au.is_empty() || au.len() > MAX_AU) {
            return Err(invalid("Expected bounded AAC-ELD frames"));
        }
        let size = 12 + units.iter().map(|au| 4 + au.len()).sum::<usize>();
        if size > mtu {
            return Err(invalid("AAC-ELD packet exceeds the negotiated media MTU"));
        }
        let mut packet = Vec::with_capacity(size);
        packet.extend_from_slice(&[0x80, 0x60]);
        packet.extend_from_slice(&self.sequence.to_be_bytes());
        packet.extend_from_slice(&timestamp.to_be_bytes());
        packet.extend_from_slice(&self.ssrc.to_be_bytes());
        for au in units {
            packet.extend_from_slice(&[
                0xb0 | ((self.au_sequence >> 8) as u8 & 15),
                self.au_sequence as u8,
                0x10 | ((au.len() >> 8) as u8 & 15),
                au.len() as u8,
            ]);
            packet.extend_from_slice(au);
            self.au_sequence = (self.au_sequence + 1) & 0x0fff;
        }
        self.sequence = self.sequence.wrapping_add(1);
        Ok(packet)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer_response(
        packet: &[u8],
        state: StreamState,
    ) -> io::Result<Option<(Vec<u8>, PeerAction)>> {
        super::peer_response(packet, state, &configuration(1)[2..], false)
    }

    #[test]
    fn eld_bitrate_is_negotiated_and_reconfigure_responses_are_bounded() {
        let caps = configuration_at(1, 192000);
        assert_eq!(
            select_apple_eld(&caps[2..], ntpods_ipc::PlaybackBitrate::default()).unwrap(),
            Some(192000)
        );
        assert_eq!(
            select_apple_eld(
                &caps[2..],
                ntpods_ipc::PlaybackBitrate {
                    variable: false,
                    best_effort: false,
                    kbps: 256
                }
            )
            .unwrap(),
            None
        );
        assert_eq!(
            select_apple_eld(
                &caps[2..],
                ntpods_ipc::PlaybackBitrate {
                    variable: false,
                    best_effort: false,
                    kbps: 128
                }
            )
            .unwrap(),
            Some(128000)
        );
        assert!(response(&[0x32, RECONFIGURE, 0], 3, RECONFIGURE).is_err());
        assert_eq!(
            response(&[0x32, RECONFIGURE], 3, RECONFIGURE).unwrap(),
            Some(&[][..])
        );
    }

    #[test]
    fn peer_reads_selected_codec_capabilities() {
        let caps = [1, 0, 7, 8, 0, 2, 0x80, 0, 0x84, 3, 0xe8, 0];
        for signal in [GET_CAPABILITIES, GET_ALL_CAPABILITIES, GET_CONFIGURATION] {
            let (reply, action) =
                super::peer_response(&[0x20, signal, 4], StreamState::Open, &caps, false)
                    .unwrap()
                    .unwrap();
            let mut expected = caps.to_vec();
            if signal == GET_ALL_CAPABILITIES {
                expected.extend_from_slice(&[DELAY_REPORTING, 0]);
            }
            assert_eq!(&reply[2..], &expected);
            assert_eq!(action, PeerAction::None);
        }
    }

    #[test]
    fn delay_reports_require_a_negotiated_stream_and_preserve_tenths_of_ms() {
        for state in [
            StreamState::Configured,
            StreamState::Open,
            StreamState::Streaming,
        ] {
            for delay in [0u16, 1, 1500, u16::MAX] {
                let [high, low] = delay.to_be_bytes();
                assert_eq!(
                    super::peer_response(&[0xa0, DELAY_REPORT, 4, high, low], state, &[], true)
                        .unwrap(),
                    Some((vec![0xa2, DELAY_REPORT], PeerAction::DelayReport(delay)))
                );
            }
        }
        for (packet, state, negotiated, error) in [
            (
                vec![0xa0, DELAY_REPORT, 4, 5, 0xdc],
                StreamState::Idle,
                true,
                0x31,
            ),
            (
                vec![0xa0, DELAY_REPORT, 4, 5, 0xdc],
                StreamState::Open,
                false,
                0x19,
            ),
            (
                vec![0xa0, DELAY_REPORT, 8, 5, 0xdc],
                StreamState::Open,
                true,
                0x12,
            ),
            (
                vec![0xa0, DELAY_REPORT, 5, 5, 0xdc],
                StreamState::Open,
                true,
                0x12,
            ),
            (
                vec![0xa0, DELAY_REPORT, 4, 5],
                StreamState::Open,
                true,
                0x11,
            ),
            (
                vec![0xa0, DELAY_REPORT, 4, 5, 0xdc, 0],
                StreamState::Open,
                true,
                0x11,
            ),
        ] {
            assert_eq!(
                super::peer_response(&packet, state, &[], negotiated).unwrap(),
                Some((vec![0xa3, DELAY_REPORT, error], PeerAction::None))
            );
        }
        assert!(super::peer_response(
            &[0xa4, DELAY_REPORT, 4, 5, 0xdc],
            StreamState::Open,
            &[],
            true
        )
        .is_err());
        assert_eq!(
            super::peer_response(&[0xa2, DELAY_REPORT], StreamState::Open, &[], true).unwrap(),
            None
        );
    }

    #[test]
    fn delay_capability_is_validated_for_both_codecs_and_hidden_from_legacy_discovery() {
        for mut caps in [
            configuration(1)[2..].to_vec(),
            super::super::aac_protocol::configuration(
                1,
                super::super::aac_protocol::AAC_CAPABILITIES,
            )[2..]
                .to_vec(),
        ] {
            assert!(!supports_delay_reporting(&caps).unwrap());
            let legacy = caps.clone();
            caps.extend_from_slice(&[DELAY_REPORTING, 0]);
            assert!(supports_delay_reporting(&caps).unwrap());
            for signal in [GET_CAPABILITIES, GET_ALL_CAPABILITIES, GET_CONFIGURATION] {
                let (reply, _) =
                    super::peer_response(&[0x40, signal, 4], StreamState::Configured, &caps, true)
                        .unwrap()
                        .unwrap();
                assert_eq!(
                    &reply[2..],
                    if signal == GET_CAPABILITIES {
                        &legacy
                    } else {
                        &caps
                    }
                );
            }
            for suffix in [
                &[DELAY_REPORTING][..],
                &[DELAY_REPORTING, 1][..],
                &[DELAY_REPORTING, 1, 0][..],
                &[DELAY_REPORTING, 0, DELAY_REPORTING, 0][..],
            ] {
                let mut bad = legacy.clone();
                bad.extend_from_slice(suffix);
                assert!(supports_delay_reporting(&bad).is_err());
                assert!(select_apple_eld(&bad, ntpods_ipc::PlaybackBitrate::default()).is_err());
                assert!(super::super::aac_protocol::select_codec(&bad).is_err());
            }
        }
    }

    #[test]
    fn selects_only_available_audio_sinks() {
        assert_eq!(sinks(&[4, 8, 10, 8, 12, 0, 16, 24]).unwrap(), vec![1]);
        for malformed in [&[][..], &[4][..], &[0, 8][..], &[252, 8][..]] {
            assert!(sinks(malformed).is_err());
        }
    }

    #[test]
    fn requires_supported_vendor_format_and_complete_capabilities() {
        let data = configuration(2);
        assert!(supports_apple_eld(&data[2..]).unwrap());
        for length in 1..data.len() - 2 {
            let partial = &data[2..2 + length];
            assert!(!supports_apple_eld(partial).unwrap_or(false));
        }
        for index in [7, 8, 12, 15, 17, 19] {
            let mut bad = data.clone();
            bad[index] = 0;
            assert!(!supports_apple_eld(&bad[2..]).unwrap_or(false));
        }
    }

    #[test]
    fn correlates_responses_and_rejects_fragmentation() {
        assert_eq!(response(&[0x32, START], 3, START).unwrap(), Some(&[][..]));
        assert_eq!(response(&[0x22, START], 3, START).unwrap(), None);
        assert!(response(&[0x33, START, 0x31], 3, START).is_err());
        assert!(response(&[0x36, START], 3, START).is_err());
        assert!(response(&[0x32, START, 0], 3, START).is_err());
        assert!(response(&[0x32, START | 0x80], 3, START).is_err());
        assert_eq!(
            response(&[0x33, GET_CAPABILITIES, 0x19], 3, GET_CAPABILITIES)
                .unwrap_err()
                .kind(),
            io::ErrorKind::Unsupported
        );
        assert_eq!(
            response(&[0x33, GET_CAPABILITIES], 3, GET_CAPABILITIES)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn rejects_duplicate_and_truncated_codec_capabilities() {
        let mut caps = configuration(1)[2..].to_vec();
        let second_codec = caps[2..].to_vec();
        caps[13] = 0;
        caps.extend_from_slice(&second_codec);
        assert!(supports_apple_eld(&caps).is_err());
        for bytes in [&[0, 0][..], &[7, 1, 0][..], &[7, 3, 0, 0xff, 0x4c][..]] {
            assert!(supports_apple_eld(bytes).is_err());
        }
    }

    #[test]
    fn peer_commands_reject_bad_lengths_and_endpoints_without_stopping() {
        for (request, expected) in [
            (
                vec![0x50, GET_CAPABILITIES, 4, 0],
                vec![0x53, GET_CAPABILITIES, 0x11],
            ),
            (
                vec![0x50, GET_CAPABILITIES, 8],
                vec![0x53, GET_CAPABILITIES, 0x12],
            ),
            (vec![0x50, SUSPEND, 4, 8], vec![0x53, SUSPEND, 8, 0x12]),
            (vec![0x50, START], vec![0x53, START, 0, 0x11]),
            (vec![0x50, 14], vec![0x51, 14]),
            (vec![0x50, RECONFIGURE, 4], vec![0x53, RECONFIGURE, 0, 0x19]),
        ] {
            assert_eq!(
                peer_response(&request, StreamState::Streaming).unwrap(),
                Some((expected, PeerAction::None))
            );
        }
    }

    #[test]
    fn peer_stream_actions_follow_configuration_state() {
        assert_eq!(
            peer_response(&[0x20, START, 4], StreamState::Open).unwrap(),
            Some((vec![0x22, START], PeerAction::Start))
        );
        assert_eq!(
            peer_response(&[0x20, SUSPEND, 4], StreamState::Streaming).unwrap(),
            Some((vec![0x22, SUSPEND], PeerAction::Suspend))
        );
        assert_eq!(
            peer_response(&[0x20, CLOSE, 4], StreamState::Configured).unwrap(),
            Some((vec![0x23, CLOSE, 0x31], PeerAction::None))
        );
        assert_eq!(
            peer_response(&[0x20, ABORT, 4], StreamState::Idle).unwrap(),
            Some((vec![0x22, ABORT], PeerAction::Close))
        );
        assert_eq!(
            peer_response(&[0x20, GET_CONFIGURATION, 4], StreamState::Idle).unwrap(),
            Some((vec![0x23, GET_CONFIGURATION, 0x31], PeerAction::None))
        );
        let (caps, _) = peer_response(&[0x20, GET_ALL_CAPABILITIES, 4], StreamState::Open)
            .unwrap()
            .unwrap();
        assert!(supports_apple_eld(&caps[2..]).unwrap());
        assert!(peer_response(&[0x24, DISCOVER, 2], StreamState::Idle).is_err());
    }

    #[test]
    fn packet_framing_wrap_and_mtu_are_checked_before_state_changes() {
        for count in [1, 2] {
            let units = vec![vec![0xaa; MAX_AU]; count];
            let mut writer = Packetizer {
                sequence: u16::MAX,
                au_sequence: 4095,
                ssrc: 0x11223344,
            };
            assert!(writer.packet(&[], 1234, 1023).is_err());
            assert!(writer.packet(&vec![vec![0; 1]; 3], 1234, 1023).is_err());
            assert!(writer.packet(&[vec![]], 1234, 1023).is_err());
            assert!(writer.packet(&[vec![0; MAX_AU + 1]], 1234, 1023).is_err());
            let mtu = if count == 1 { 337 } else { 662 };
            assert!(writer.packet(&units, 1234, mtu - 1).is_err());
            let packet = writer.packet(&units, 1234, mtu).unwrap();
            let mut expected = vec![
                0x80, 0x60, 0xff, 0xff, 0, 0, 4, 0xd2, 0x11, 0x22, 0x33, 0x44, 0xbf, 0xff, 0x11,
                0x41,
            ];
            expected.extend_from_slice(&[0xaa; MAX_AU]);
            if count == 2 {
                expected.extend_from_slice(&[0xb0, 0, 0x11, 0x41]);
                expected.extend_from_slice(&[0xaa; MAX_AU]);
            }
            assert_eq!(packet, expected);
            assert_eq!(writer.sequence, 0);
            assert_eq!(writer.au_sequence, count as u16 - 1);
            writer.reset_audio();
            let reset = writer.packet(&units, 0, mtu).unwrap();
            assert_eq!(&reset[2..8], &[0; 6]);
            assert_eq!(&reset[12..14], &[0xb0, 1]);
        }
        assert!(validate_eld_aus(usize::MAX).is_err());
    }
}
