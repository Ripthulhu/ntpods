use super::protocol::{invalid, PCM_BYTES};
use std::io;
use std::ptr;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, OPEN_EXISTING};
use windows_sys::Win32::System::IO::DeviceIoControl;

pub struct Render(HANDLE);

impl Render {
    pub fn open() -> io::Result<Self> {
        let handle = unsafe {
            CreateFileW(
                crate::wide(r"\\.\NTPodsPlayback").as_ptr(),
                0x8000_0000,
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
        let result = Self(handle);
        result.status()?;
        Ok(result)
    }

    fn ioctl(&self, code: u32, output: &mut [u8]) -> io::Result<usize> {
        let mut returned = 0;
        if unsafe {
            DeviceIoControl(
                self.0,
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
        let mut data = [0; 32];
        if self.ioctl(0x0022_6404, &mut data)? != data.len() {
            return Err(invalid("Truncated playback driver status"));
        }
        parse_status(&data)
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
            CloseHandle(self.0);
        }
    }
}

fn parse_status(data: &[u8; 32]) -> io::Result<[u32; 4]> {
    let values: Vec<_> = data
        .chunks_exact(4)
        .map(|v| u32::from_le_bytes(v.try_into().unwrap()))
        .collect();
    if values[..4] != [2, 48_000, 2, 16] {
        return Err(invalid("Unsupported playback driver format"));
    }
    Ok([values[4], values[5], values[6], values[7]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_requires_generation_aware_driver_and_preserves_wrap() {
        let mut values = [2u32, 48_000, 2, 16, 1, 480, 0, 0];
        let decode = |values: [u32; 8]| {
            let bytes: Vec<_> = values
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect();
            parse_status(bytes.as_slice().try_into().unwrap())
        };
        for generation in [0, 1, u32::MAX] {
            values[7] = generation;
            assert_eq!(decode(values).unwrap(), [1, 480, 0, generation]);
        }
        values[0] = 1;
        assert!(decode(values).is_err());
        values[0] = 2;
        values[2] = 1;
        assert!(decode(values).is_err());
    }
}
