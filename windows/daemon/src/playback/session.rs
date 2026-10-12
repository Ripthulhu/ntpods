use super::protocol::*;
use super::{aac_protocol, codec::Mode};
use crate::driver::Driver;
use std::io;
use std::time::{Duration, Instant};

pub struct Session {
    driver: Driver,
    label: u8,
    seid: u8,
    state: StreamState,
    peer_ended: bool,
    source_caps: Vec<u8>,
    selected_config: Vec<u8>,
    pub mtu: usize,
}

pub fn driver_ready(driver: &Driver, address: u64) -> io::Result<()> {
    let mut data = [0; 28];
    if driver.playback_ioctl(0x814, &[], &mut data)? != data.len()
        || u32::from_le_bytes(data[..4].try_into().unwrap()) != 1
        || u64::from_le_bytes(data[20..].try_into().unwrap()) != address
    {
        return Err(invalid("Experimental Bluetooth driver is unavailable"));
    }
    if data[4..12] != [0; 8] {
        return Err(invalid("A playback session is already open"));
    }
    Ok(())
}

impl Session {
    pub fn open(driver: Driver, address: u64, mode: Mode) -> io::Result<Self> {
        let mut session = Self {
            driver,
            label: 0,
            seid: 0,
            state: StreamState::Idle,
            peer_ended: false,
            source_caps: mode.source_configuration()[2..].to_vec(),
            selected_config: Vec::new(),
            mtu: 0,
        };
        session.open_channel(address, 0)?;
        let endpoints = sinks(&session.request(DISCOVER, &[])?)?;
        let deadline = Instant::now() + Duration::from_secs(15);
        for seid in endpoints {
            if Instant::now() >= deadline {
                break;
            }
            let caps = match session.request(GET_CAPABILITIES, &[seid << 2]) {
                Ok(caps) => caps,
                Err(error) if error.kind() == io::ErrorKind::Unsupported => continue,
                Err(error) => return Err(error),
            };
            let config = match mode {
                Mode::Aac => aac_protocol::select_codec(&caps)?
                    .map(|codec| aac_protocol::configuration(seid, codec)),
                Mode::Eld => supports_apple_eld(&caps)?.then(|| configuration(seid)),
            };
            if let Some(config) = config {
                session.seid = seid;
                session.selected_config = config;
                break;
            }
        }
        if session.seid == 0 {
            return Err(invalid(&format!(
                "AirPods did not offer {} stereo",
                mode.name()
            )));
        }
        let config = session.selected_config.clone();
        if let Err(error) = session.request(SET_CONFIGURATION, &config) {
            // An explicit rejection gives us no ownership of the peer's endpoint.
            if error.kind() == io::ErrorKind::Unsupported {
                session.seid = 0;
            }
            return Err(error);
        }
        session.state = StreamState::Configured;
        session.request(OPEN, &[session.seid << 2])?;
        session.state = StreamState::Open;
        session.mtu = session.open_channel(address, 1)?;
        if session.mtu < 48 || (mode == Mode::Eld && session.mtu < 984) {
            return Err(invalid("Media MTU is too small for the selected codec"));
        }
        session.start()?;
        Ok(session)
    }

    fn open_channel(&self, address: u64, channel: u32) -> io::Result<usize> {
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
        self.label = (self.label + 1) & 15;
        self.send(0, &command(self.label, signal, payload))?;
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
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

    fn peer_command(&mut self, packet: &[u8]) -> io::Result<bool> {
        let caps = if packet.get(1) == Some(&GET_CONFIGURATION) {
            self.selected_config.get(2..).unwrap_or_default()
        } else {
            &self.source_caps
        };
        let Some((reply, action)) = peer_response(packet, self.state, caps)? else {
            return Ok(false);
        };
        self.send(0, &reply)?;
        match action {
            PeerAction::None => {}
            PeerAction::Start => self.state = StreamState::Streaming,
            PeerAction::Suspend => self.state = StreamState::Open,
            PeerAction::Close => {
                self.state = StreamState::Idle;
                self.peer_ended = true;
            }
        }
        if matches!(action, PeerAction::Suspend | PeerAction::Close) {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "AirPods ended experimental playback",
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

    pub fn media(&self, packet: &[u8]) -> io::Result<()> {
        self.send(1, packet)
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

fn start_if_needed(state: StreamState, request: impl FnOnce() -> io::Result<()>) -> io::Result<()> {
    match state {
        StreamState::Streaming => Ok(()),
        StreamState::Open => request(),
        _ => Err(invalid("Playback stream is not open")),
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.suspend();
        if self.seid != 0 && !self.peer_ended {
            let signal = if matches!(self.state, StreamState::Open | StreamState::Streaming) {
                CLOSE
            } else {
                ABORT
            };
            let _ = self.request(signal, &[self.seid << 2]);
        }
        let _ = self.driver.playback_ioctl(0x811, &[], &mut []);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_peer_start_does_not_send_a_second_start() {
        let (_, action) = peer_response(&command(3, START, &[4]), StreamState::Open, &[])
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
