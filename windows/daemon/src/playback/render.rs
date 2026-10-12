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
