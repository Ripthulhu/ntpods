use super::protocol::{invalid, PCM_BYTES};
use std::cell::Cell;
use std::io;
use std::ptr;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, OPEN_EXISTING};
use windows_sys::Win32::System::IO::DeviceIoControl;

pub struct Render {
    handle: HANDLE,
    address: u64,
    ready: Cell<bool>,
}

#[derive(Debug)]
pub struct ClockStatus {
    pub generation: u32,
    pub phase: u32,
    pub virtual_hns: u64,
}

impl Render {
    pub fn open(address: u64) -> io::Result<Self> {
        if address == 0 || address > 0xffff_ffff_ffff {
            return Err(invalid("Invalid playback device address"));
        }
        let handle = unsafe {
            CreateFileW(
                crate::wide(&format!(r"\\.\NTPodsPlayback_{address:012x}")).as_ptr(),
                0xc000_0000,
                0,
                ptr::null(),
                OPEN_EXISTING,
                0,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let result = Self {
            handle,
            address,
            ready: Cell::new(false),
        };
        result.status()?;
        Ok(result)
    }

    fn ioctl(&self, code: u32, output: &mut [u8]) -> io::Result<usize> {
        let mut returned = 0;
        if unsafe {
            DeviceIoControl(
                self.handle,
                code,
                ptr::null(),
                0,
                output.as_mut_ptr().cast(),
                output.len() as u32,
                &mut returned,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if returned as usize > output.len() {
            return Err(invalid("Invalid render response size"));
        }
        Ok(returned as usize)
    }

    pub fn status(&self) -> io::Result<[u32; 4]> {
        let mut data = [0; 48];
        if self.ioctl(0x0022_6404, &mut data)? != data.len() {
            return Err(invalid("Truncated playback driver status"));
        }
        parse_status(&data, self.address, self.ready.get())
    }

    pub fn clock(&self) -> io::Result<ClockStatus> {
        let mut data = [0; 64];
        if self.ioctl(0x0022_6410, &mut data)? != data.len() {
            return Err(invalid("Truncated playback clock response"));
        }
        parse_clock(&data)
    }

    pub fn set_clock(&self, interval: u32, delta_ms: u32) -> io::Result<()> {
        let input = clock_request(self.clock()?.generation, interval, delta_ms)?;
        let mut returned = 0;
        if unsafe {
            DeviceIoControl(
                self.handle,
                0x0022_a40c,
                input.as_ptr().cast(),
                input.len() as u32,
                ptr::null_mut(),
                0,
                &mut returned,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if returned != 0 {
            return Err(invalid("Invalid playback clock response"));
        }
        Ok(())
    }

    pub fn cancel_clock(&self) -> io::Result<()> {
        self.set_clock(10, 0)
    }

    pub fn set_ready(&self, ready: bool) -> io::Result<()> {
        let input = [1u32.to_le_bytes(), u32::from(ready).to_le_bytes()].concat();
        let mut returned = 0;
        if unsafe {
            DeviceIoControl(
                self.handle,
                0x0022_a408,
                input.as_ptr().cast(),
                input.len() as u32,
                ptr::null_mut(),
                0,
                &mut returned,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if returned != 0 {
            return Err(invalid("Invalid provider response"));
        }
        self.ready.set(ready);
        Ok(())
    }

    pub fn read(&self, maximum: usize) -> io::Result<Vec<u8>> {
        if maximum == 0 || maximum > PCM_BYTES || maximum % 4 != 0 {
            return Err(invalid("Invalid PCM read size"));
        }
        let mut pcm = vec![0; maximum];
        let bytes = self.ioctl(0x0022_6400, &mut pcm)?;
        if bytes % 4 != 0 {
            return Err(invalid("Partial stereo PCM frame"));
        }
        pcm.truncate(bytes);
        Ok(pcm)
    }
}

fn clock_request(generation: u32, interval: u32, delta_ms: u32) -> io::Result<Vec<u8>> {
    if !((interval == 10 && delta_ms == 0)
        || ([9, 11].contains(&interval) && (1..=500).contains(&delta_ms)))
    {
        return Err(invalid("Invalid playback clock adjustment"));
    }
    Ok([1, generation, interval, delta_ms]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect())
}

fn parse_clock(data: &[u8; 64]) -> io::Result<ClockStatus> {
    let value = |offset| u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
    let interval = value(8);
    let phase = value(12);
    let requested = value(32);
    let delta_ms = value(36);
    if value(0) != 1
        || ![9, 10, 11].contains(&interval)
        || phase > 5
        || ![9, 10, 11].contains(&requested)
        || delta_ms > 500
        || (phase != 2 && interval != 10)
    {
        return Err(invalid("Unsupported playback clock response"));
    }
    Ok(ClockStatus {
        generation: value(4),
        phase,
        virtual_hns: u64::from_le_bytes(data[24..32].try_into().unwrap()),
    })
}

impl Drop for Render {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

fn parse_status(data: &[u8; 48], address: u64, expected_ready: bool) -> io::Result<[u32; 4]> {
    let values: Vec<_> = data
        .chunks_exact(4)
        .map(|v| u32::from_le_bytes(v.try_into().unwrap()))
        .collect();
    if values[..4] != [3, 48_000, 2, 16]
        || values[10] > 1
        || values[11] > 1
        || u64::from_le_bytes(data[32..40].try_into().unwrap()) != address
    {
        return Err(invalid("Unsupported playback driver format"));
    }
    if values[11] == 0 {
        return Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "AirPods playback is offline",
        ));
    }
    if expected_ready && values[10] == 0 {
        return Err(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "AirPods playback driver restarted",
        ));
    }
    Ok([values[4], values[5], values[6], values[7]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_requests_reject_unbounded_or_ambiguous_adjustments() {
        for (interval, delta) in [(9, 1), (11, 500), (10, 0)] {
            let request = clock_request(u32::MAX, interval, delta).unwrap();
            assert_eq!(request.len(), 16);
            assert_eq!(&request[4..8], &u32::MAX.to_le_bytes());
        }
        for (interval, delta) in [(9, 0), (11, 501), (10, 1), (0, 1), (12, 50)] {
            assert!(clock_request(0, interval, delta).is_err());
        }
    }

    #[test]
    fn clock_response_preserves_generation_and_rejects_unknown_states() {
        let mut data = [0; 64];
        data[..4].copy_from_slice(&1u32.to_le_bytes());
        data[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        data[8..12].copy_from_slice(&10u32.to_le_bytes());
        data[24..32].copy_from_slice(&123_456_789u64.to_le_bytes());
        data[32..36].copy_from_slice(&11u32.to_le_bytes());
        for phase in 0u32..=5 {
            data[12..16].copy_from_slice(&phase.to_le_bytes());
            let snapshot = parse_clock(&data).unwrap();
            assert_eq!(snapshot.generation, u32::MAX);
            assert_eq!(snapshot.phase, phase);
            assert_eq!(snapshot.virtual_hns, 123_456_789);
        }
        data[12..16].copy_from_slice(&6u32.to_le_bytes());
        assert!(parse_clock(&data).is_err());
        data[12..16].copy_from_slice(&3u32.to_le_bytes());
        data[8..12].copy_from_slice(&11u32.to_le_bytes());
        assert!(parse_clock(&data).is_err());
    }

    #[test]
    fn power_loss_requires_negotiation_again() {
        let values = [3u32, 48000, 2, 16, 1, 0, 0, 2, 42, 0, 0, 1];
        let bytes: Vec<_> = values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let data = bytes.as_slice().try_into().unwrap();
        assert!(parse_status(data, 42, false).is_ok());
        assert_eq!(
            parse_status(data, 42, true).unwrap_err().kind(),
            io::ErrorKind::ConnectionReset
        );
    }

    #[test]
    fn status_requires_generation_aware_driver_and_preserves_wrap() {
        let mut values = [3u32, 48_000, 2, 16, 1, 480, 0, 0, 42, 0, 0, 1];
        let decode = |values: [u32; 12]| {
            let bytes: Vec<_> = values
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect();
            parse_status(bytes.as_slice().try_into().unwrap(), 42, false)
        };
        for generation in [0, 1, u32::MAX] {
            values[7] = generation;
            assert_eq!(decode(values).unwrap(), [1, 480, 0, generation]);
        }
        values[0] = 1;
        assert!(decode(values).is_err());
        values[0] = 3;
        values[2] = 1;
        assert!(decode(values).is_err());
        values[2] = 2;
        values[8] = 43;
        assert!(decode(values).is_err());
        values[8] = 42;
        values[11] = 0;
        assert_eq!(
            decode(values).unwrap_err().kind(),
            io::ErrorKind::NotConnected
        );
        values[11] = 1;
        values[10] = 2;
        assert!(decode(values).is_err());
    }
}
