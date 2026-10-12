//! Writer for the NTPodsMic virtual microphone: pushes decoded PCM into the
//! driver's ring buffer over the control device `\\.\NTPodsMic`.

use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::IO::DeviceIoControl;

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
// CTL_CODE(FILE_DEVICE_UNKNOWN, 0x800, METHOD_BUFFERED, FILE_WRITE_DATA)
const IOCTL_NTPODS_MIC_WRITE_PCM: u32 = 0x0022_A000;
// CTL_CODE(FILE_DEVICE_UNKNOWN, 0x801, METHOD_BUFFERED, FILE_READ_DATA)
const IOCTL_NTPODS_MIC_STATUS: u32 = 0x0022_6004;
const IOCTL_NTPODS_MIC_FLUSH: u32 = 0x0022_A008;
const MAX_PCM_SAMPLES: usize = 4096;

/// Which mic driver is loaded, for the log. Both expose the same `\\.\NTPodsMic`,
/// so the handle can't tell; the running kernel service can: ACX is the
/// `NTPodsMicACX` service (drivers/mic), PortCls is `NTPodsMicPC` (drivers/mic-portcls).
pub fn driver_kind() -> &'static str {
    use windows_sys::Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatus, SC_MANAGER_CONNECT,
        SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_STATUS,
    };
    let running = |name: &str| -> bool {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let scm = OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_CONNECT);
            if scm.is_null() {
                return false;
            }
            let svc = OpenServiceW(scm, wide.as_ptr(), SERVICE_QUERY_STATUS);
            let mut st: SERVICE_STATUS = std::mem::zeroed();
            let ok = !svc.is_null()
                && QueryServiceStatus(svc, &mut st) != 0
                && st.dwCurrentState == SERVICE_RUNNING;
            if !svc.is_null() {
                CloseServiceHandle(svc);
            }
            CloseServiceHandle(scm);
            ok
        }
    };
    match (running("NTPodsMicPC"), running("NTPodsMicACX")) {
        (true, false) => "PortCls (NTPodsMicPC)",
        (false, true) => "ACX (NTPodsMicACX)",
        (true, true) => "both PortCls and ACX are running",
        (false, false) => "none running",
    }
}

pub struct MicPipe {
    handle: HANDLE,
}

// One tray owns the single exclusive handle and uses it from the receive thread
// (writes) and the status-poll thread (reads); both DeviceIoControl paths are
// independent, so sharing the handle is safe.
unsafe impl Send for MicPipe {}
unsafe impl Sync for MicPipe {}

impl MicPipe {
    /// Open the virtual-mic control device. None if the NTPodsMic driver
    /// isn't installed (or another writer holds the exclusive handle).
    pub fn open() -> Option<MicPipe> {
        let path: Vec<u16> = r"\\.\NTPodsMic"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null(),
                OPEN_EXISTING,
                0,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            None
        } else {
            let pipe = MicPipe { handle };
            // Old drivers cannot guarantee immediate silence on disable.
            pipe.flush().then_some(pipe)
        }
    }

    /// Number of running capture streams in the Windows audio engine. `None` = the DeviceIoControl failed (handle dead / device gone), so the
    /// caller can reopen.
    pub fn status(&self) -> Option<u32> {
        let mut out = [0u8; 8];
        let mut returned = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                self.handle,
                IOCTL_NTPODS_MIC_STATUS,
                ptr::null(),
                0,
                out.as_mut_ptr() as *mut c_void,
                8,
                &mut returned,
                ptr::null_mut(),
            )
        };
        if ok != 0 && returned == 8 {
            Some(u32::from_le_bytes(out[4..8].try_into().unwrap()))
        } else {
            None
        }
    }

    /// Drop all queued PCM without removing the Windows capture endpoint.
    pub fn flush(&self) -> bool {
        let mut returned = 0;
        unsafe {
            DeviceIoControl(
                self.handle,
                IOCTL_NTPODS_MIC_FLUSH,
                ptr::null(),
                0,
                ptr::null_mut(),
                0,
                &mut returned,
                ptr::null_mut(),
            ) != 0
        }
    }

    /// Push mono 16-bit PCM samples into the mic ring. Returns false if the write
    /// failed (handle dead) so the caller can reopen.
    pub fn write(&self, samples: &[i16]) -> bool {
        if samples.is_empty() {
            return true;
        }
        if samples.len() > MAX_PCM_SAMPLES {
            return samples
                .chunks(MAX_PCM_SAMPLES)
                .all(|chunk| self.write(chunk));
        }
        let bytes = std::mem::size_of_val(samples);
        let mut returned = 0u32;
        let ok = unsafe {
            DeviceIoControl(
                self.handle,
                IOCTL_NTPODS_MIC_WRITE_PCM,
                samples.as_ptr() as *const c_void,
                bytes as u32,
                ptr::null_mut(),
                0,
                &mut returned,
                ptr::null_mut(),
            )
        };
        ok != 0
    }
}

/// Self-healing holder for the virtual-mic pipe. The device may not be enumerated yet
/// when the daemon starts at boot, and the handle can later break (mic driver
/// reinstalled / device re-plugged) — either used to leave the mic dead until a daemon
/// restart. All mic access goes through this cell, which (re)opens the pipe on demand
/// so audio + mic recover on their own. Cheap Mutex; the write path is uncontended.
pub struct MicPipeCell {
    inner: Mutex<Option<MicPipe>>,
    enabled: AtomicBool,
    feature_enabled: AtomicBool,
    capture_active: AtomicBool,
    permission_revision: AtomicU64,
}

impl MicPipeCell {
    /// Try to open once up front; a failure here is fine — it reopens on first use.
    pub fn new() -> MicPipeCell {
        MicPipeCell {
            inner: Mutex::new(MicPipe::open()),
            enabled: AtomicBool::new(false),
            feature_enabled: AtomicBool::new(true),
            capture_active: AtomicBool::new(false),
            permission_revision: AtomicU64::new(0),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<MicPipe>> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Whether a pipe is currently open (reopening if it was closed).
    pub fn is_open(&self) -> bool {
        let mut g = self.lock();
        if g.is_none() {
            *g = MicPipe::open();
        }
        g.is_some()
    }

    pub fn set_feature_enabled(&self, enabled: bool) {
        let mut g = self.lock();
        if self.feature_enabled.swap(enabled, Ordering::AcqRel) && !enabled {
            self.permission_revision.fetch_add(1, Ordering::AcqRel);
        }
        if !enabled {
            self.enabled.store(false, Ordering::Release);
            if g.as_ref().is_some_and(|p| !p.flush()) {
                *g = None;
            }
        }
    }

    pub fn set_capture_active(&self, active: bool) {
        let mut g = self.lock();
        if self.capture_active.swap(active, Ordering::AcqRel) != active {
            // Each client session starts empty. Uplink lifetime is a separate policy.
            if g.as_ref().is_some_and(|p| !p.flush()) {
                self.enabled.store(false, Ordering::Release);
                *g = None;
            }
        }
    }

    pub fn revoke_stream(&self) {
        let mut g = self.lock();
        self.permission_revision.fetch_add(1, Ordering::AcqRel);
        self.enabled.store(false, Ordering::Release);
        if g.as_ref().is_some_and(|p| !p.flush()) {
            *g = None;
        }
    }

    pub fn permission_revision(&self) -> u64 {
        self.permission_revision.load(Ordering::Acquire)
    }

    /// Clear interrupted audio without granting or changing capture permission.
    pub fn flush(&self) -> Result<(), ()> {
        let mut g = self.lock();
        if g.as_ref().is_some_and(|p| p.flush()) {
            Ok(())
        } else {
            self.enabled.store(false, Ordering::Release);
            *g = None;
            Err(())
        }
    }

    /// Serialize permission changes with writes, so a late decoder cannot refill
    /// the ring after mute/disable has flushed it.
    pub fn set_enabled(&self, enabled: bool) -> Result<bool, ()> {
        let mut g = self.lock();
        self.enabled.store(false, Ordering::Release);
        if g.is_none() {
            *g = MicPipe::open();
        }
        let ok = g.as_ref().is_some_and(|p| p.flush());
        if !ok {
            *g = None;
        }
        let permitted = self.feature_enabled.load(Ordering::Acquire);
        self.enabled
            .store(enabled && ok && permitted, Ordering::Release);
        if ok {
            Ok(!enabled || permitted)
        } else {
            Err(())
        }
    }

    /// Push only while permitted; a failed endpoint silences this capture until
    /// the lifecycle explicitly starts it again.
    pub fn write(&self, samples: &[i16]) -> Result<bool, ()> {
        let mut g = self.lock();
        if !self.enabled.load(Ordering::Acquire) || !self.capture_active.load(Ordering::Acquire) {
            return Ok(false);
        }
        let ok = g.as_ref().is_some_and(|p| p.write(samples));
        if !ok {
            self.enabled.store(false, Ordering::Release);
            *g = None;
        }
        if ok {
            Ok(true)
        } else {
            Err(())
        }
    }

    /// Active capture streams; unavailable or old drivers fail closed.
    pub fn status(&self) -> Option<u32> {
        let mut g = self.lock();
        if g.is_none() {
            *g = MicPipe::open();
        }
        match g.as_ref().and_then(|p| p.status()) {
            Some(v) => Some(v),
            None => {
                *g = None;
                None
            }
        }
    }
}

impl Drop for MicPipe {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disconnected_test_pipe() -> MicPipeCell {
        MicPipeCell {
            inner: Mutex::new(None),
            enabled: AtomicBool::new(true),
            feature_enabled: AtomicBool::new(true),
            capture_active: AtomicBool::new(false),
            permission_revision: AtomicU64::new(0),
        }
    }

    #[test]
    fn retained_stream_discards_pcm_without_an_app_and_keeps_its_health() {
        let pipe = disconnected_test_pipe();
        let mut mic = crate::mic_lifecycle::Lifecycle::default();
        mic.tick(0, true, true);
        mic.start_attempted();
        for now in [1, 4_000, 8_000, 12_000] {
            // No handle exists: touching the bridge here would fail the test.
            assert_eq!(pipe.write(&[1, 2, 3]), Ok(false));
            mic.pcm_received(now);
            assert!(!mic.capture_timed_out(now + 1));
            assert!(pipe.enabled.load(Ordering::Acquire));
        }
        pipe.set_capture_active(true);
        pipe.set_capture_active(false);
        assert!(pipe.enabled.load(Ordering::Acquire));
        assert_eq!(pipe.permission_revision(), 0);
        pipe.set_feature_enabled(false);
        assert!(!pipe.enabled.load(Ordering::Acquire));
        assert_eq!(pipe.permission_revision(), 1);
        assert_eq!(pipe.write(&[1, 2, 3]), Ok(false));
    }

    #[test]
    fn manual_stop_revokes_a_stream_even_if_capture_remains_open() {
        let pipe = disconnected_test_pipe();
        pipe.set_capture_active(true);
        pipe.revoke_stream();
        assert!(pipe.capture_active.load(Ordering::Acquire));
        assert!(!pipe.enabled.load(Ordering::Acquire));
        assert_eq!(pipe.permission_revision(), 1);
        assert_eq!(pipe.write(&[1, 2, 3]), Ok(false));
    }
}
