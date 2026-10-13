use super::protocol::*;
use super::{aac_protocol, codec::Mode};
use crate::driver::Driver;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::ERROR_REQ_NOT_ACCEP;

pub struct Session {
    driver: Driver,
    address: u64,
    flush_ms: u16,
    pub mode: Mode,
    pub eld_aus: usize,
    opened: Instant,
    media_packets: u64,
    last_request: u8,
    label: u8,
    seid: u8,
    state: StreamState,
    peer_ended: bool,
    source_caps: Vec<u8>,
    selected_config: Vec<u8>,
    cancel: Arc<AtomicBool>,
    closing: bool,
    interrupted: Box<dyn Fn() -> bool>,
    pub mtu: usize,
    pub bitrate: u32,
    pub variable: bool,
    pub delay_reporting: bool,
    // AVDTP reports use 100-microsecond units, not measured end-to-end latency.
    pub delay_report: Option<u16>,
    pub delay_serial: u64,
}

pub fn driver_ready(driver: &Driver, address: u64) -> io::Result<()> {
    let mut data = [0; 28];
    if driver.playback_ioctl(0x814, &[], &mut data)? != data.len()
        || u32::from_le_bytes(data[..4].try_into().unwrap()) != 1
        || u64::from_le_bytes(data[20..].try_into().unwrap()) != address
    {
        return Err(invalid("NTPods Bluetooth driver is unavailable"));
    }
    if data[4..12] != [0; 8] {
        return Err(invalid("A playback session is already open"));
    }
    Ok(())
}

impl Session {
    pub fn open(
        driver: Driver,
        address: u64,
        mode: Mode,
        eld_aus: usize,
        bitrates: [ntpods_ipc::PlaybackBitrate; 2],
        cancel: Arc<AtomicBool>,
        interrupted: impl Fn() -> bool + 'static,
        on_mode: impl FnOnce(Mode),
    ) -> io::Result<Self> {
        validate_eld_aus(eld_aus)?;
        let mut session = Self {
            driver,
            address,
            flush_ms: 200,
            mode,
            eld_aus,
            opened: Instant::now(),
            media_packets: 0,
            last_request: 0,
            label: 0,
            seid: 0,
            state: StreamState::Idle,
            peer_ended: false,
            source_caps: mode.source_configuration()[2..].to_vec(),
            selected_config: Vec::new(),
            cancel,
            closing: false,
            interrupted: Box::new(interrupted),
            mtu: 0,
            bitrate: 0,
            variable: false,
            delay_reporting: false,
            delay_report: None,
            delay_serial: 0,
        };
        open_signaling_with_retry(
            || session.open_channel(address, 0),
            |delay| {
                crate::log(&format!(
                    "playback signaling: initial channel rejected, retrying in {} ms",
                    delay.as_millis()
                ));
                let deadline = Instant::now() + delay;
                while Instant::now() < deadline {
                    session.check_cancelled()?;
                    thread::sleep(
                        Duration::from_millis(50)
                            .min(deadline.saturating_duration_since(Instant::now())),
                    );
                }
                session.check_cancelled()?;
                if !crate::bt::is_connected(address) {
                    return Err(io::Error::new(
                        io::ErrorKind::NotConnected,
                        "AirPods disconnected before playback could start",
                    ));
                }
                Ok(())
            },
        )?;
        let mut endpoints = sinks(&session.request(DISCOVER, &[])?)?.into_iter();
        let deadline = Instant::now() + Duration::from_secs(15);
        let selected = select_configuration(
            mode,
            bitrates,
            || loop {
                session.check_cancelled()?;
                let Some(seid) = endpoints.next() else {
                    return Ok(None);
                };
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "Playback capability discovery timed out",
                    ));
                }
                let caps =
                    match discover_capabilities(|signal| session.request(signal, &[seid << 2])) {
                        Ok(caps) => caps,
                        Err(error) if request_not_supported(&error, GET_CAPABILITIES) => continue,
                        Err(error) => return Err(error),
                    };
                return Ok(Some((seid, caps)));
            },
            on_mode,
        )?;
        session.check_cancelled()?;
        session.mode = selected.mode;
        session.source_caps = selected.mode.source_configuration()[2..].to_vec();
        session.seid = selected.seid;
        session.selected_config = selected.config;
        session.bitrate = selected.bitrate;
        let mode = session.mode;
        let preference = bitrates[usize::from(mode == Mode::Eld)];
        session.variable = preference.variable;
        crate::log(&format!(
            "playback signaling: {} selected endpoint {}, {} bps",
            mode.name(),
            session.seid,
            session.bitrate
        ));
        session.configure()?;
        session.delay_reporting = selected.delay_reporting;
        session.request(OPEN, &[session.seid << 2])?;
        session.state = StreamState::Open;
        session.mtu = session.open_channel(address, 1)?;
        crate::log(&format!(
            "playback signaling: {} media channel opened, MTU {}",
            mode.name(),
            session.mtu
        ));
        if session.mtu < 48 {
            return Err(invalid("Media MTU is too small for the selected codec"));
        }
        if mode == Mode::Eld {
            let maximum = eld_bitrate_limit(session.mtu, session.eld_aus)?;
            if session.bitrate > maximum {
                if !preference.best_effort {
                    return Err(invalid("Media MTU is too small for this bitrate"));
                }
                session.bitrate = mode.bitrate(preference, maximum)?;
                let (request, config) =
                    eld_reconfiguration(session.seid, session.bitrate, session.delay_reporting);
                session.request(RECONFIGURE, &request)?;
                session.selected_config = config;
            }
        }
        Ok(session)
    }

    fn open_channel(&self, address: u64, channel: u32) -> io::Result<usize> {
        self.check_cancelled()?;
        let input = [
            1u32.to_le_bytes().as_slice(),
            channel.to_le_bytes().as_slice(),
            address.to_le_bytes().as_slice(),
        ]
        .concat();
        let mut output = [0; 16];
        if self.driver.playback_ioctl(0x810, &input, &mut output)? != output.len()
            || output[..4] != 1u32.to_le_bytes()
            || output[4..8] != channel.to_le_bytes()
        {
            return Err(invalid("Invalid playback channel response"));
        }
        Ok(u32::from_le_bytes(output[8..12].try_into().unwrap()) as usize)
    }

    fn open_media(&self, flush_ms: u16) -> io::Result<usize> {
        self.check_cancelled()?;
        let input = media_open_request(self.address, flush_ms)?;
        let mut output = [0; 16];
        if self.driver.playback_ioctl(0x810, &input, &mut output)? != output.len()
            || output[..4] != 2u32.to_le_bytes()
            || output[4..8] != 1u32.to_le_bytes()
        {
            return Err(invalid(
                "The Bluetooth driver does not support buffer trials",
            ));
        }
        Ok(u32::from_le_bytes(output[8..12].try_into().unwrap()) as usize)
    }

    pub fn check_flush_support(&self) -> io::Result<()> {
        let mut data = [0; 28];
        if self
            .driver
            .playback_ioctl(0x814, &2u32.to_le_bytes(), &mut data)?
            != data.len()
            || data[..4] != 2u32.to_le_bytes()
            || data[4..8] != 1u32.to_le_bytes()
            || data[8..12] != 1u32.to_le_bytes()
            || u64::from_le_bytes(data[20..].try_into().unwrap()) != self.address
        {
            return Err(invalid(
                "The Bluetooth driver does not support buffer trials on this stream",
            ));
        }
        Ok(())
    }

    pub fn reopen_media(&mut self, flush_ms: u16) -> io::Result<()> {
        media_open_request(self.address, flush_ms)?;
        self.suspend()?;
        self.request(CLOSE, &[self.seid << 2])?;
        self.state = StreamState::Idle;
        let seid = self.seid;
        self.seid = 0;
        let close = [2u32.to_le_bytes(), 1u32.to_le_bytes()].concat();
        if self.driver.playback_ioctl(0x811, &close, &mut [])? != 0 {
            return Err(invalid("Invalid media close response"));
        }
        self.delay_report = None;
        self.seid = seid;
        self.configure()?;
        self.request(OPEN, &[self.seid << 2])?;
        self.state = StreamState::Open;
        self.mtu = self.open_media(flush_ms)?;
        self.flush_ms = flush_ms;
        if self.mtu < 48
            || (self.mode == Mode::Eld && self.bitrate > eld_bitrate_limit(self.mtu, self.eld_aus)?)
        {
            return Err(invalid(
                "Reopened media MTU is too small for the selected bitrate",
            ));
        }
        crate::log(&format!(
            "playback media: reopened with {flush_ms} ms flush timeout"
        ));
        Ok(())
    }

    fn configure(&mut self) -> io::Result<()> {
        let config = self.selected_config.clone();
        if let Err(error) = self.request(SET_CONFIGURATION, &config) {
            // An explicit rejection gives us no ownership of the peer's endpoint.
            if error.kind() == io::ErrorKind::Unsupported {
                self.seid = 0;
            }
            return Err(error);
        }
        self.state = StreamState::Configured;
        Ok(())
    }

    pub fn restore_media_timeout(&mut self) -> io::Result<()> {
        if self.flush_ms != 200 {
            self.reopen_media(200)?;
        }
        Ok(())
    }

    fn send(&self, channel: u32, packet: &[u8]) -> io::Result<()> {
        let mut input = Vec::with_capacity(8 + packet.len());
        input.extend_from_slice(&1u32.to_le_bytes());
        input.extend_from_slice(&channel.to_le_bytes());
        input.extend_from_slice(packet);
        self.driver.playback_ioctl(0x812, &input, &mut [])?;
        Ok(())
    }

    fn receive(&self, wait_ms: u32) -> io::Result<Vec<u8>> {
        let input = [
            1u32.to_le_bytes(),
            0u32.to_le_bytes(),
            wait_ms.to_le_bytes(),
        ]
        .concat();
        let mut output = vec![0; 8192];
        let size = self.driver.playback_ioctl(0x813, &input, &mut output)?;
        output.truncate(size);
        Ok(output)
    }

    fn request(&mut self, signal: u8, payload: &[u8]) -> io::Result<Vec<u8>> {
        self.last_request = signal;
        let started = Instant::now();
        let result = self.request_inner(signal, payload);
        crate::log(&format!(
            "playback signaling: {} {} {}, state {:?}, {} ms",
            self.mode.name(),
            signal_name(signal),
            if result.is_ok() { "accepted" } else { "failed" },
            self.state,
            started.elapsed().as_millis()
        ));
        result
    }

    fn request_inner(&mut self, signal: u8, payload: &[u8]) -> io::Result<Vec<u8>> {
        self.check_cancelled()?;
        self.label = (self.label + 1) & 15;
        self.send(0, &command(self.label, signal, payload))?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            self.check_cancelled()?;
            let reply = self.receive(100)?;
            if reply.is_empty() {
                continue;
            }
            if self.peer_command(&reply)? {
                continue;
            }
            if let Some(payload) = response(&reply, self.label, signal)? {
                return Ok(payload.to_vec());
            }
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "AirPods did not acknowledge the playback request",
        ))
    }

    fn check_cancelled(&self) -> io::Result<()> {
        if !self.closing && (self.cancel.load(Ordering::Acquire) || (self.interrupted)()) {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Playback selection changed",
            ))
        } else {
            Ok(())
        }
    }

    fn peer_command(&mut self, packet: &[u8]) -> io::Result<bool> {
        let caps = if packet.get(1) == Some(&GET_CONFIGURATION) {
            self.selected_config.get(2..).unwrap_or_default()
        } else {
            &self.source_caps
        };
        let Some((reply, action)) = peer_response(packet, self.state, caps, self.delay_reporting)?
        else {
            return Ok(false);
        };
        crate::log(&format!(
            "playback signaling: {} peer {} ({:#04x}), endpoint {:?}, state {:?}, action {:?}, last request {}, {} media packets, {} ms since open",
            self.mode.name(), signal_name(packet[1]), packet[1],
            packet.get(2).map(|seid| seid >> 2), self.state, action,
            signal_name(self.last_request), self.media_packets, self.opened.elapsed().as_millis()
        ));
        self.send(0, &reply)?;
        match action {
            PeerAction::None => {}
            PeerAction::Start => {
                // A peer START does not establish that Windows is producing PCM.
                self.state = StreamState::Streaming;
            }
            PeerAction::Suspend => self.state = StreamState::Open,
            PeerAction::Close => {
                self.state = StreamState::Idle;
                self.peer_ended = true;
                self.delay_reporting = false;
                self.delay_report = None;
            }
            PeerAction::DelayReport(delay) => {
                if self.delay_report != Some(delay) {
                    crate::log(&format!(
                        "playback signaling: {} earbud-reported delay {}.{} ms",
                        self.mode.name(),
                        delay / 10,
                        delay % 10
                    ));
                }
                self.delay_report = Some(delay);
                self.delay_serial = self.delay_serial.wrapping_add(1);
            }
        }
        if matches!(action, PeerAction::Suspend | PeerAction::Close) {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "AirPods ended playback",
            ));
        }
        Ok(true)
    }

    pub fn poll(&mut self) -> io::Result<()> {
        let data = self.receive(1)?;
        if !data.is_empty() {
            self.peer_command(&data)?;
        }
        Ok(())
    }

    pub fn media(&mut self, packet: &[u8]) -> io::Result<()> {
        self.send(1, packet)?;
        self.media_packets += 1;
        if self.mode == Mode::Eld && self.media_packets <= 3 {
            crate::log(&format!(
                "playback media: AAC-ELD packet {}, {} bytes, RTP {:02x?}",
                self.media_packets,
                packet.len(),
                packet.get(..12).unwrap_or_default()
            ));
        }
        Ok(())
    }
    pub fn suspend(&mut self) -> io::Result<()> {
        if self.state == StreamState::Streaming {
            self.request(SUSPEND, &[self.seid << 2])?;
            self.state = StreamState::Open;
        }
        Ok(())
    }
    pub fn start(&mut self) -> io::Result<()> {
        start_if_needed(self.state, || {
            self.request(START, &[self.seid << 2]).map(|_| ())
        })?;
        self.state = StreamState::Streaming;
        Ok(())
    }
}

fn media_open_request(address: u64, flush_ms: u16) -> io::Result<Vec<u8>> {
    if address == 0
        || address > 0xffff_ffff_ffff
        || ![60, 80, 100, 120, 140, 150, 200, 300].contains(&flush_ms)
    {
        return Err(invalid("Invalid media flush timeout or device address"));
    }
    Ok([
        2u32.to_le_bytes().as_slice(),
        1u32.to_le_bytes().as_slice(),
        address.to_le_bytes().as_slice(),
        u32::from(flush_ms).to_le_bytes().as_slice(),
    ]
    .concat())
}

fn signal_name(signal: u8) -> &'static str {
    match signal {
        DISCOVER => "DISCOVER",
        GET_CAPABILITIES => "GET_CAPABILITIES",
        SET_CONFIGURATION => "SET_CONFIGURATION",
        GET_CONFIGURATION => "GET_CONFIGURATION",
        RECONFIGURE => "RECONFIGURE",
        OPEN => "OPEN",
        START => "START",
        CLOSE => "CLOSE",
        SUSPEND => "SUSPEND",
        ABORT => "ABORT",
        GET_ALL_CAPABILITIES => "GET_ALL_CAPABILITIES",
        DELAY_REPORT => "DELAY_REPORT",
        _ => "UNKNOWN",
    }
}

#[derive(Debug)]
struct Configuration {
    mode: Mode,
    seid: u8,
    config: Vec<u8>,
    bitrate: u32,
    delay_reporting: bool,
}

fn discover_capabilities(
    mut request: impl FnMut(u8) -> io::Result<Vec<u8>>,
) -> io::Result<Vec<u8>> {
    match request(GET_ALL_CAPABILITIES) {
        Err(error) if request_not_supported(&error, GET_ALL_CAPABILITIES) => {
            request(GET_CAPABILITIES)
        }
        result => result,
    }
}

fn with_delay_reporting(mut config: Vec<u8>, delay_reporting: bool) -> Vec<u8> {
    if delay_reporting {
        config.extend_from_slice(&[DELAY_REPORTING, 0]);
    }
    config
}

fn eld_reconfiguration(seid: u8, bitrate: u32, delay_reporting: bool) -> (Vec<u8>, Vec<u8>) {
    let config = configuration_at(seid, bitrate);
    // RECONFIGURE changes the codec; the negotiated reporting capability stays in GET_CONFIGURATION.
    let mut request = vec![seid << 2];
    request.extend_from_slice(&config[4..]);
    (request, with_delay_reporting(config, delay_reporting))
}

fn select_configuration(
    requested: Mode,
    bitrates: [ntpods_ipc::PlaybackBitrate; 2],
    mut next: impl FnMut() -> io::Result<Option<(u8, Vec<u8>)>>,
    on_mode: impl FnOnce(Mode),
) -> io::Result<Configuration> {
    let mut fallback = Vec::new();
    let mut eld_offered = false;
    let mut aac_offered = false;
    while let Some((seid, caps)) = next()? {
        let delay_reporting = supports_delay_reporting(&caps)?;
        let selected = match requested {
            Mode::Aac => {
                aac_offered |=
                    aac_protocol::select_codec_at(&caps, ntpods_ipc::PlaybackBitrate::default())?
                        .is_some();
                aac_protocol::select_codec_at(&caps, bitrates[0])?
                    .map(|(codec, bitrate)| (aac_protocol::configuration(seid, codec), bitrate))
            }
            Mode::Eld => {
                eld_offered |=
                    select_apple_eld(&caps, ntpods_ipc::PlaybackBitrate::default())?.is_some();
                let selected = select_apple_eld(&caps, bitrates[1])?
                    .map(|bitrate| (configuration_at(seid, bitrate), bitrate));
                if selected.is_none() {
                    fallback.push((seid, caps));
                }
                selected
            }
        };
        if let Some((config, bitrate)) = selected {
            on_mode(requested);
            return Ok(Configuration {
                mode: requested,
                seid,
                config: with_delay_reporting(config, delay_reporting),
                bitrate,
                delay_reporting,
            });
        }
    }
    if requested == Mode::Eld {
        if eld_offered {
            on_mode(Mode::Eld);
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "The AirPods cannot use this AAC-ELD bitrate.",
            ));
        }
        // Fall back only after all endpoints were checked, before configuring any of them.
        for (seid, caps) in fallback {
            aac_offered |=
                aac_protocol::select_codec_at(&caps, ntpods_ipc::PlaybackBitrate::default())?
                    .is_some();
            if let Some((codec, bitrate)) = aac_protocol::select_codec_at(&caps, bitrates[0])? {
                let delay_reporting = supports_delay_reporting(&caps)?;
                on_mode(Mode::Aac);
                return Ok(Configuration {
                    mode: Mode::Aac,
                    seid,
                    config: with_delay_reporting(
                        aac_protocol::configuration(seid, codec),
                        delay_reporting,
                    ),
                    bitrate,
                    delay_reporting,
                });
            }
        }
    }
    if aac_offered {
        on_mode(Mode::Aac);
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "The AirPods cannot use this AAC bitrate.",
        ));
    }
    Err(invalid(&format!(
        "AirPods did not offer {} stereo",
        requested.name()
    )))
}

fn eld_bitrate_limit(mtu: usize, aus: usize) -> io::Result<u32> {
    validate_eld_aus(aus)?;
    // CBR may use one padding byte per access unit.
    let au_budget = mtu.saturating_sub(12 + 4 * aus) / aus;
    Ok(au_budget.saturating_sub(1).min(320) as u32 * 800)
}

fn open_signaling_with_retry(
    mut open: impl FnMut() -> io::Result<usize>,
    mut wait: impl FnMut(Duration) -> io::Result<()>,
) -> io::Result<usize> {
    // Retry before any signaling or endpoint ownership; later failures need explicit recovery.
    let mut retries = 0;
    loop {
        match open() {
            Err(error)
                if error.raw_os_error() == Some(ERROR_REQ_NOT_ACCEP as i32) && retries < 2 =>
            {
                retries += 1;
                wait(Duration::from_secs(retries))?;
            }
            result => return result,
        }
    }
}

fn start_if_needed(state: StreamState, request: impl FnOnce() -> io::Result<()>) -> io::Result<()> {
    match state {
        StreamState::Streaming => Ok(()),
        StreamState::Open => request(),
        _ => Err(invalid("Playback stream is not open")),
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.closing = true;
        let _ = self.suspend();
        if self.seid != 0 && !self.peer_ended {
            let signal = if matches!(self.state, StreamState::Open | StreamState::Streaming) {
                CLOSE
            } else {
                ABORT
            };
            let _ = self.request(signal, &[self.seid << 2]);
        }
        if let Err(error) = self.driver.playback_ioctl(0x811, &[], &mut []) {
            crate::log(&format!("playback: channel cleanup failed: {error}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ntpods_ipc::PlaybackBitrate;

    #[test]
    fn media_options_preserve_the_versioned_packed_driver_layout() {
        let address = 0x1234_5678_9abc;
        let request = media_open_request(address, 60).unwrap();
        assert_eq!(request.len(), 20);
        assert_eq!(&request[..8], &[2, 0, 0, 0, 1, 0, 0, 0]);
        assert_eq!(&request[8..16], &address.to_le_bytes());
        assert_eq!(&request[16..], &[60, 0, 0, 0]);
        for timeout in [0, 30, 59, 61, 500, u16::MAX] {
            assert!(media_open_request(address, timeout).is_err());
        }
        for address in [0, 0x1_0000_0000_0000, u64::MAX] {
            assert!(media_open_request(address, 200).is_err());
        }
    }

    fn select_configuration(
        requested: Mode,
        bitrates: [PlaybackBitrate; 2],
        next: impl FnMut() -> io::Result<Option<(u8, Vec<u8>)>>,
    ) -> io::Result<Configuration> {
        super::select_configuration(requested, bitrates, next, |_| {})
    }

    fn aac_caps() -> Vec<u8> {
        aac_protocol::configuration(1, aac_protocol::AAC_CAPABILITIES)[2..].to_vec()
    }

    #[test]
    fn capability_discovery_falls_back_only_for_explicit_unsupported_replies() {
        for packet in [
            vec![0x31, GET_ALL_CAPABILITIES],
            vec![0x33, GET_ALL_CAPABILITIES, 0x19],
        ] {
            let mut signals = Vec::new();
            let caps = discover_capabilities(|signal| {
                signals.push(signal);
                if signal == GET_ALL_CAPABILITIES {
                    response(&packet, 3, signal).map(|_| unreachable!())
                } else {
                    Ok(aac_caps())
                }
            })
            .unwrap();
            assert_eq!(caps, aac_caps());
            assert_eq!(signals, [GET_ALL_CAPABILITIES, GET_CAPABILITIES]);
        }
        for error in [
            response(&[0x33, GET_ALL_CAPABILITIES, 0x31], 3, GET_ALL_CAPABILITIES).unwrap_err(),
            response(&[0x33, GET_ALL_CAPABILITIES, 0x12], 3, GET_ALL_CAPABILITIES).unwrap_err(),
            response(&[0x31, GET_ALL_CAPABILITIES, 0], 3, GET_ALL_CAPABILITIES).unwrap_err(),
            response(&[0x33, GET_CAPABILITIES, 0x19], 3, GET_CAPABILITIES).unwrap_err(),
            io::Error::from(io::ErrorKind::TimedOut),
            io::Error::from(io::ErrorKind::NotConnected),
            io::Error::from(io::ErrorKind::Interrupted),
            io::Error::from(io::ErrorKind::Unsupported),
        ] {
            let mut error = Some(error);
            assert!(discover_capabilities(|signal| {
                assert_eq!(signal, GET_ALL_CAPABILITIES);
                Err(error.take().expect("must not fall back"))
            })
            .is_err());
        }
        let mut calls = 0;
        assert_eq!(
            discover_capabilities(|signal| {
                calls += 1;
                assert_eq!(signal, GET_ALL_CAPABILITIES);
                Ok(aac_caps())
            })
            .unwrap(),
            aac_caps()
        );
        assert_eq!(calls, 1);
    }

    #[test]
    fn delay_reporting_follows_the_selected_endpoint_and_survives_reconfiguration() {
        for (requested, offered, actual) in [
            (Mode::Aac, Mode::Aac, Mode::Aac),
            (Mode::Eld, Mode::Eld, Mode::Eld),
            (Mode::Eld, Mode::Aac, Mode::Aac),
        ] {
            for reporting in [false, true] {
                let caps =
                    with_delay_reporting(offered.source_configuration()[2..].to_vec(), reporting);
                let mut endpoints = vec![(3, caps)].into_iter();
                let selected =
                    select_configuration(requested, [PlaybackBitrate::default(); 2], || {
                        Ok(endpoints.next())
                    })
                    .unwrap();
                assert_eq!(selected.mode, actual);
                assert_eq!(selected.delay_reporting, reporting);
                assert_eq!(
                    supports_delay_reporting(&selected.config[2..]).unwrap(),
                    reporting
                );
                let (request, config) = eld_reconfiguration(3, 192_000, reporting);
                assert_eq!(request[0], 3 << 2);
                assert_eq!(request[1], 7);
                assert!(!supports_delay_reporting(&request[1..]).unwrap());
                assert_eq!(supports_delay_reporting(&config[2..]).unwrap(), reporting);
                assert_eq!(
                    select_apple_eld(&config[2..], PlaybackBitrate::default()).unwrap(),
                    Some(192_000)
                );
            }
        }
        // A capability on the fallback endpoint must not enable it on a different selected endpoint.
        let mut endpoints = vec![
            (1, with_delay_reporting(aac_caps(), true)),
            (2, configuration(2)[2..].to_vec()),
        ]
        .into_iter();
        let selected = select_configuration(Mode::Eld, [PlaybackBitrate::default(); 2], || {
            Ok(endpoints.next())
        })
        .unwrap();
        assert_eq!(selected.seid, 2);
        assert!(!selected.delay_reporting);
    }

    #[test]
    fn delay_report_during_open_wait_does_not_complete_the_open_request() {
        let report = command(3, DELAY_REPORT, &[4, 0x05, 0xdd]);
        let (reply, action) = peer_response(&report, StreamState::Configured, &[], true)
            .unwrap()
            .unwrap();
        assert_eq!(reply, [0x32, DELAY_REPORT]);
        assert_eq!(action, PeerAction::DelayReport(1501));
        assert_eq!(response(&report, 3, OPEN).unwrap(), None);
        assert!(
            peer_response(&[0x32, OPEN], StreamState::Configured, &[], true)
                .unwrap()
                .is_none()
        );
        assert_eq!(response(&[0x32, OPEN], 3, OPEN).unwrap(), Some(&[][..]));
    }

    #[test]
    fn variable_profile_negotiates_its_peak_and_keeps_aac_fallback_constant() {
        let preferences = [
            PlaybackBitrate::default(),
            PlaybackBitrate {
                variable: true,
                ..PlaybackBitrate::default()
            },
        ];
        let mut caps = vec![(1, aac_caps()), (2, configuration(2)[2..].to_vec())].into_iter();
        let selected = select_configuration(Mode::Eld, preferences, || Ok(caps.next())).unwrap();
        assert_eq!(selected.mode, Mode::Eld);
        assert_eq!(selected.bitrate, 256_000);
        assert_eq!(selected.config, configuration_at(2, 256_000));

        let mut caps = vec![
            (1, aac_caps()),
            (2, configuration_at(2, 192_000)[2..].to_vec()),
        ]
        .into_iter();
        assert_eq!(
            select_configuration(Mode::Eld, preferences, || Ok(caps.next()))
                .unwrap_err()
                .kind(),
            io::ErrorKind::Unsupported
        );
        let mut caps = vec![(1, aac_caps())].into_iter();
        let fallback = select_configuration(Mode::Eld, preferences, || Ok(caps.next())).unwrap();
        assert_eq!(fallback.mode, Mode::Aac);
        assert!(!preferences[0].variable);
        assert_eq!(fallback.bitrate, 512_000);

        for (aus, sufficient_mtu) in [(1, 337), (2, 662)] {
            for mtu in [48, sufficient_mtu - 1] {
                assert!(Mode::Eld
                    .bitrate(preferences[1], eld_bitrate_limit(mtu, aus).unwrap())
                    .is_err());
            }
            assert_eq!(
                Mode::Eld
                    .bitrate(
                        preferences[1],
                        eld_bitrate_limit(sufficient_mtu, aus).unwrap()
                    )
                    .unwrap(),
                256_000
            );
        }
    }

    #[test]
    fn eld_takes_precedence_and_fallback_uses_the_aac_preference() {
        let requested = Mode::Eld;
        let preferences = [
            PlaybackBitrate {
                variable: false,
                best_effort: false,
                kbps: 129,
            },
            PlaybackBitrate::default(),
        ];
        let mut caps = vec![(3, aac_caps())].into_iter();
        let selected = select_configuration(requested, preferences, || Ok(caps.next())).unwrap();
        assert_eq!(selected.mode, Mode::Aac);
        assert_eq!(selected.seid, 3);
        assert_eq!(selected.bitrate, 129000);
        assert_eq!(selected.config[0], 3 << 2);
        assert_eq!(&selected.config[6..8], &[0, 2]);

        let mut caps = vec![(1, aac_caps()), (2, configuration(2)[2..].to_vec())].into_iter();
        let selected = select_configuration(requested, preferences, || Ok(caps.next())).unwrap();
        assert_eq!(selected.mode, Mode::Eld);
        assert_eq!(selected.seid, 2);
        assert_eq!(selected.bitrate, 256000);
    }

    #[test]
    fn unsupported_eld_profile_can_fall_back_but_a_manual_bitrate_cannot() {
        let mut unsupported = configuration(2);
        unsupported[17] = 4; // Apple codec without the implemented 48 kHz profile.
        for eld in [
            unsupported[2..].to_vec(),
            configuration_at(2, 96000)[2..].to_vec(),
        ] {
            let mut caps = vec![(1, aac_caps()), (2, eld)].into_iter();
            let selected = select_configuration(Mode::Eld, [PlaybackBitrate::default(); 2], || {
                Ok(caps.next())
            })
            .unwrap();
            assert_eq!(selected.mode, Mode::Aac);
            assert_eq!(selected.bitrate, 512000);
        }
        let mut caps = vec![
            (1, aac_caps()),
            (2, configuration_at(2, 192000)[2..].to_vec()),
        ]
        .into_iter();
        let preferences = [
            PlaybackBitrate::default(),
            PlaybackBitrate {
                variable: false,
                best_effort: false,
                kbps: 256,
            },
        ];
        let error = select_configuration(Mode::Eld, preferences, || Ok(caps.next())).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert!(error.to_string().contains("bitrate"));
    }

    #[test]
    fn fallback_does_not_hide_bad_capabilities_or_discovery_errors() {
        let mut malformed_aac = aac_caps();
        malformed_aac[3] = 7;
        malformed_aac.pop();
        for invalid_caps in [vec![7, 16, 0, 0xff], malformed_aac] {
            let mut caps = vec![(1, invalid_caps)].into_iter();
            assert_eq!(
                select_configuration(Mode::Eld, [PlaybackBitrate::default(); 2], || Ok(
                    caps.next()
                ))
                .unwrap_err()
                .kind(),
                io::ErrorKind::InvalidData
            );
        }
        for kind in [
            io::ErrorKind::Interrupted,
            io::ErrorKind::NotConnected,
            io::ErrorKind::TimedOut,
        ] {
            let mut candidate = Some((1, aac_caps()));
            let error = super::select_configuration(
                Mode::Eld,
                [PlaybackBitrate::default(); 2],
                || {
                    candidate
                        .take()
                        .map_or_else(|| Err(io::Error::from(kind)), |caps| Ok(Some(caps)))
                },
                |_| panic!("failed discovery must not select a codec"),
            )
            .unwrap_err();
            assert_eq!(error.kind(), kind);
        }
    }

    #[test]
    fn aac_never_falls_forward_to_eld_and_fallback_obeys_aac_bitrate_limits() {
        let mut caps = vec![(2, configuration(2)[2..].to_vec())].into_iter();
        assert!(
            select_configuration(Mode::Aac, [PlaybackBitrate::default(); 2], || Ok(
                caps.next()
            ))
            .is_err()
        );
        let mut aac = aac_caps();
        let end = aac.len();
        aac[end - 3..].copy_from_slice(&[0x01, 0xf4, 0]); // 128 kbps ceiling.
        let mut preferences = [
            PlaybackBitrate {
                variable: false,
                best_effort: false,
                kbps: 129,
            },
            PlaybackBitrate::default(),
        ];
        for requested in [Mode::Aac, Mode::Eld] {
            let mut attempted = requested;
            let mut caps = vec![(1, aac.clone())].into_iter();
            let error = super::select_configuration(
                requested,
                preferences,
                || Ok(caps.next()),
                |mode| attempted = mode,
            )
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::Unsupported);
            assert_eq!(
                error.to_string(),
                "The AirPods cannot use this AAC bitrate."
            );
            assert_eq!(attempted, Mode::Aac);
        }
        preferences[0].kbps = 128;
        let mut attempted = Mode::Eld;
        let mut caps = vec![(1, aac)].into_iter();
        let selected = super::select_configuration(
            Mode::Eld,
            preferences,
            || Ok(caps.next()),
            |mode| attempted = mode,
        )
        .unwrap();
        assert_eq!(attempted, Mode::Aac);
        assert_eq!(selected.mode, attempted);
        assert_eq!(selected.bitrate, 128000);
    }

    #[test]
    fn initial_signaling_rejection_retries_then_succeeds() {
        assert_eq!(
            open_signaling_with_retry(|| Ok(1023), |_| panic!("successful open must not wait"))
                .unwrap(),
            1023
        );
        let mut attempts = 0;
        let mut waits = Vec::new();
        let mtu = open_signaling_with_retry(
            || {
                attempts += 1;
                if attempts < 3 {
                    Err(io::Error::from_raw_os_error(ERROR_REQ_NOT_ACCEP as i32))
                } else {
                    Ok(1023)
                }
            },
            |delay| {
                waits.push(delay);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(mtu, 1023);
        assert_eq!(attempts, 3);
        assert_eq!(waits, [Duration::from_secs(1), Duration::from_secs(2)]);
    }

    #[test]
    fn initial_signaling_retry_is_bounded_and_specific() {
        for (code, expected_attempts) in [(71, 3), (5, 1), (995, 1), (1167, 1), (1460, 1)] {
            let mut attempts = 0;
            let error = open_signaling_with_retry(
                || {
                    attempts += 1;
                    Err(io::Error::from_raw_os_error(code))
                },
                |_| Ok(()),
            )
            .unwrap_err();
            assert_eq!(error.raw_os_error(), Some(code));
            assert_eq!(attempts, expected_attempts);
        }
        assert!(open_signaling_with_retry(
            || Err(invalid("Invalid playback channel response")),
            |_| panic!("protocol errors must not retry")
        )
        .is_err());
    }

    #[test]
    fn cancellation_or_disconnect_during_wait_stops_signaling_retry() {
        for kind in [io::ErrorKind::Interrupted, io::ErrorKind::NotConnected] {
            let mut attempts = 0;
            let error = open_signaling_with_retry(
                || {
                    attempts += 1;
                    Err(io::Error::from_raw_os_error(ERROR_REQ_NOT_ACCEP as i32))
                },
                |_| Err(io::Error::from(kind)),
            )
            .unwrap_err();
            assert_eq!(error.kind(), kind);
            assert_eq!(attempts, 1);
        }
    }

    #[test]
    fn eld_mtu_budget_includes_cbr_padding() {
        for (aus, maximum_mtu, minimum_mtu) in [(1, 337, 177), (2, 662, 342)] {
            assert_eq!(eld_bitrate_limit(1023, aus).unwrap(), 256000);
            assert_eq!(eld_bitrate_limit(maximum_mtu, aus).unwrap(), 256000);
            assert_eq!(eld_bitrate_limit(maximum_mtu - 1, aus).unwrap(), 255200);
            assert_eq!(eld_bitrate_limit(minimum_mtu, aus).unwrap(), 128000);
            assert_eq!(eld_bitrate_limit(minimum_mtu - 1, aus).unwrap(), 127200);
            assert_eq!(eld_bitrate_limit(0, aus).unwrap(), 0);
            assert_eq!(eld_bitrate_limit(usize::MAX, aus).unwrap(), 256000);
        }
        for aus in [0, 3, usize::MAX] {
            assert!(eld_bitrate_limit(1023, aus).is_err());
        }
    }

    #[test]
    fn accepted_peer_start_does_not_send_a_second_start() {
        let (_, action) = peer_response(&command(3, START, &[4]), StreamState::Open, &[], false)
            .unwrap()
            .unwrap();
        assert_eq!(action, PeerAction::Start);
        start_if_needed(StreamState::Streaming, || {
            panic!("peer already started the stream")
        })
        .unwrap();
        for state in [StreamState::Idle, StreamState::Configured] {
            assert!(start_if_needed(state, || panic!("stream is not open")).is_err());
        }
        let mut requests = 0;
        start_if_needed(StreamState::Open, || {
            requests += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(requests, 1);
        assert!(start_if_needed(StreamState::Open, || Err(invalid("START rejected"))).is_err());
    }
}
