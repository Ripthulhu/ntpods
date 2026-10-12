use std::ffi::c_void;
use std::io;
use std::ptr::NonNull;

pub const PCM_BYTES: usize = 1024 * 4;
pub const MAX_AU: usize = 674;

extern "C" {
    fn ntpods_aac_open() -> *mut c_void;
    fn ntpods_aac_close(encoder: *mut c_void);
    fn ntpods_aac_config(encoder: *mut c_void, output: *mut u8, capacity: i32) -> i32;
    fn ntpods_aac_write(encoder: *mut c_void, pcm: *const u8, length: i32) -> i32;
    fn ntpods_aac_read(encoder: *mut c_void, output: *mut u8, capacity: i32) -> i32;
}

pub struct AacEncoder {
    handle: NonNull<c_void>,
    config: Vec<u8>,
}

impl AacEncoder {
    pub fn new() -> io::Result<Self> {
        let handle = NonNull::new(unsafe { ntpods_aac_open() })
            .ok_or_else(|| io::Error::other("Could not start the AAC encoder"))?;
        let mut encoder = Self {
            handle,
            config: vec![0; 64],
        };
        let size = unsafe {
            ntpods_aac_config(
                encoder.handle.as_ptr(),
                encoder.config.as_mut_ptr(),
                encoder.config.len() as i32,
            )
        };
        if !(2..=64).contains(&size) || encoder.config[..2] != [0x11, 0x90] {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "AAC encoder selected an unsupported format",
            ));
        }
        encoder.config.truncate(size as usize);
        Ok(encoder)
    }

    pub fn config(&self) -> &[u8] {
        &self.config
    }

    pub fn write(&mut self, pcm: Vec<u8>) -> io::Result<()> {
        if pcm.len() != PCM_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Expected 1024 stereo PCM frames",
            ));
        }
        match unsafe { ntpods_aac_write(self.handle.as_ptr(), pcm.as_ptr(), pcm.len() as i32) } {
            0 => Ok(()),
            1 => Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Read pending AAC audio before writing more PCM",
            )),
            _ => Err(io::Error::other("AAC encoding failed")),
        }
    }

    pub fn read(&mut self) -> io::Result<Option<Vec<u8>>> {
        let mut output = vec![0; MAX_AU];
        let size = unsafe {
            ntpods_aac_read(
                self.handle.as_ptr(),
                output.as_mut_ptr(),
                output.len() as i32,
            )
        };
        match size {
            0 => Ok(None),
            1..=674 => {
                output.truncate(size as usize);
                Ok(Some(output))
            }
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "AAC encoder returned invalid audio",
            )),
        }
    }
}

impl Drop for AacEncoder {
    fn drop(&mut self) {
        unsafe { ntpods_aac_close(self.handle.as_ptr()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_bounded_raw_units_and_rejects_incomplete_pcm() {
        let mut encoder = AacEncoder::new().unwrap();
        assert_eq!(encoder.config(), [0x11, 0x90, 0x56, 0xe5, 0]);
        for size in [0, PCM_BYTES - 1, PCM_BYTES + 1] {
            assert_eq!(
                encoder.write(vec![0; size]).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
        assert!(encoder.read().unwrap().is_none());
        let mut packets = 0;
        for _ in 0..20 {
            encoder.write(vec![0; PCM_BYTES]).unwrap();
            while let Some(unit) = encoder.read().unwrap() {
                assert!(!unit.is_empty() && unit.len() <= MAX_AU);
                packets += 1;
            }
        }
        assert!(packets >= 18, "AAC delay grew beyond two frames");
    }
}
