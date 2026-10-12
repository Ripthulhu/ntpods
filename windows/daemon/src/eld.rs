//! AAC-ELD decoder — thin Rust wrapper over the FFmpeg C shim (`eld_shim.c`).
//! Converts AirPods AAC-ELD frames to 48 kHz mono i16 PCM. The source clock
//! assumption in the C shim still needs validation with an actual recording.

use std::ffi::c_void;
use std::os::raw::c_int;

extern "C" {
    fn eld_open(asc: *const u8, asc_len: c_int, sample_rate: c_int) -> *mut c_void;
    fn eld_decode(
        h: *mut c_void,
        au: *const u8,
        au_len: c_int,
        out: *mut i16,
        out_cap: c_int,
    ) -> c_int;
    fn eld_close(h: *mut c_void);
}

/// AudioSpecificConfig for the AirPods hi-res mic: AOT 39 (ER AAC ELD), 48 kHz,
/// mono. Taken from LibrePods PR #655.
const ASC: [u8; 4] = [0xF8, 0xE6, 0x30, 0x00];
const SAMPLE_RATE: c_int = 48_000;

pub struct Decoder {
    h: *mut c_void,
    out: Vec<i16>,
    last_packet_ms: Option<u64>,
}

// The handle is only ever used from the receive thread.
unsafe impl Send for Decoder {}

impl Decoder {
    pub fn new() -> Option<Decoder> {
        let h = unsafe { eld_open(ASC.as_ptr(), ASC.len() as c_int, SAMPLE_RATE) };
        if h.is_null() {
            None
        } else {
            Some(Decoder {
                h,
                out: vec![0i16; 8192],
                last_packet_ms: None,
            })
        }
    }

    /// A resumed uplink must not reuse AAC prediction or resampler history.
    /// Packet absence, rather than silent PCM, defines the gap.
    pub fn prepare_packet(&mut self, now_ms: u64) -> Result<bool, ()> {
        let interrupted = self
            .last_packet_ms
            .is_some_and(|last| now_ms.saturating_sub(last) >= 2_000);
        if interrupted {
            *self = Self::new().ok_or(())?;
        }
        self.last_packet_ms = Some(now_ms);
        Ok(interrupted)
    }

    /// Decode one access unit into i16 mono samples (48 kHz). May return empty
    /// (the decoder buffers a frame of latency before the first output).
    pub fn decode(&mut self, au: &[u8]) -> Result<&[i16], ()> {
        if au.is_empty() || au.len() > 255 {
            return Err(());
        }
        let n = unsafe {
            eld_decode(
                self.h,
                au.as_ptr(),
                au.len() as c_int,
                self.out.as_mut_ptr(),
                self.out.len() as c_int,
            )
        };
        if n < 0 || n as usize > self.out.len() {
            Err(())
        } else {
            Ok(&self.out[..n as usize])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resumed_packet_recreates_decoder_and_resampler_once() {
        let mut d = Decoder::new().unwrap();
        assert_eq!(d.prepare_packet(100), Ok(false));
        let old_handle = d.h;
        d.out.fill(123);
        assert_eq!(d.prepare_packet(2_100), Ok(true));
        assert_ne!(d.h, old_handle); // replacement opens before the old handle closes
        assert!(d.out.iter().all(|&sample| sample == 0));
        assert_eq!(d.prepare_packet(2_110), Ok(false));
        assert_eq!(d.prepare_packet(12_110), Ok(true));
    }

    #[test]
    fn continuous_packets_and_short_gaps_preserve_decoder_history() {
        let mut d = Decoder::new().unwrap();
        let handle = d.h;
        for now in [0, 10, 20, 1_000, 2_999, 3_009] {
            assert_eq!(d.prepare_packet(now), Ok(false));
            assert_eq!(d.h, handle);
        }
    }

    #[test]
    fn ffmpeg_loads_eld_and_rejects_invalid_buffer_arguments() {
        let mut d = Decoder::new().expect("Pinned FFmpeg must support the AirPods ELD config");
        assert!(d.decode(&[]).is_err());
        assert!(d.decode(&[0; 256]).is_err());
        let au = [0u8; 256];
        let mut guard = [123i16; 8];
        unsafe {
            assert_eq!(eld_decode(d.h, au.as_ptr(), 256, guard.as_mut_ptr(), 8), -1);
            assert_eq!(eld_decode(d.h, au.as_ptr(), 1, guard.as_mut_ptr(), 0), -1);
            assert_eq!(
                eld_decode(d.h, std::ptr::null(), 1, guard.as_mut_ptr(), 8),
                -1
            );
            assert_eq!(
                eld_decode(std::ptr::null_mut(), au.as_ptr(), 1, guard.as_mut_ptr(), 8),
                -1
            );
        }
        assert_eq!(guard, [123; 8]);
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe { eld_close(self.h) };
    }
}
