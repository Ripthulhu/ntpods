use std::ffi::c_void;
use std::io;
use std::ptr::NonNull;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU8, Ordering},
};
use windows_sys::Win32::System::{
    Console::{CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler},
    Threading::{AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW},
};

const IDLE: u8 = 0;
const RUNNING: u8 = 1;
const STOPPING: u8 = 2;
static CONSOLE: AtomicU8 = AtomicU8::new(IDLE);

fn handle_control(state: &AtomicU8, event: u32) -> i32 {
    if !matches!(event, CTRL_C_EVENT | CTRL_BREAK_EVENT) {
        return 0;
    }
    // The console callback runs on a Windows-owned thread; cleanup belongs to the audio thread.
    match state.compare_exchange(RUNNING, STOPPING, Ordering::Relaxed, Ordering::Relaxed) {
        Ok(_) | Err(STOPPING) => 1,
        _ => 0,
    }
}

unsafe extern "system" fn console_handler(event: u32) -> i32 {
    handle_control(&CONSOLE, event)
}

pub(super) fn stop_on_input(
    mut input: impl io::Read + Send + 'static,
) -> io::Result<Arc<AtomicBool>> {
    let stop = Arc::new(AtomicBool::new(false));
    let signal = stop.clone();
    // The parent closes the pipe to stop, including when it exits unexpectedly.
    std::thread::Builder::new()
        .name("spatial-parent".into())
        .spawn(move || {
            let _ = io::copy(&mut input, &mut io::sink());
            signal.store(true, Ordering::Release);
        })?;
    Ok(stop)
}

pub(super) struct ConsoleStop(());

impl ConsoleStop {
    pub(super) fn install() -> io::Result<Self> {
        CONSOLE
            .compare_exchange(IDLE, RUNNING, Ordering::Relaxed, Ordering::Relaxed)
            .map_err(|_| io::Error::other("Console handler already installed"))?;
        if unsafe { SetConsoleCtrlHandler(Some(console_handler), 1) } == 0 {
            let error = io::Error::last_os_error();
            CONSOLE.store(IDLE, Ordering::Relaxed);
            return Err(error);
        }
        Ok(Self(()))
    }

    pub(super) fn requested(&self) -> bool {
        CONSOLE.load(Ordering::Relaxed) == STOPPING
    }
}

impl Drop for ConsoleStop {
    fn drop(&mut self) {
        unsafe { SetConsoleCtrlHandler(Some(console_handler), 0) };
        CONSOLE.store(IDLE, Ordering::Relaxed);
    }
}

// NonNull is !Send, keeping MMCSS enrollment and restoration on the same thread.
pub(super) struct AudioScheduling(NonNull<c_void>);

impl AudioScheduling {
    pub(super) fn acquire() -> Option<Self> {
        let mut index = 0;
        NonNull::new(unsafe { AvSetMmThreadCharacteristicsW(windows_sys::w!("Audio"), &mut index) })
            .map(Self)
    }
}

impl Drop for AudioScheduling {
    fn drop(&mut self) {
        unsafe { AvRevertMmThreadCharacteristics(self.0.as_ptr()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Console::CTRL_CLOSE_EVENT;

    #[test]
    fn parent_pipe_eof_stops_managed_playback() {
        let stop = stop_on_input(io::empty()).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !stop.load(Ordering::Acquire) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        assert!(stop.load(Ordering::Acquire));
    }

    #[test]
    fn only_interrupt_and_break_request_a_graceful_stop() {
        let state = AtomicU8::new(IDLE);
        assert_eq!(handle_control(&state, CTRL_C_EVENT), 0);
        for event in [CTRL_C_EVENT, CTRL_BREAK_EVENT] {
            state.store(RUNNING, Ordering::Relaxed);
            assert_eq!(handle_control(&state, CTRL_CLOSE_EVENT), 0);
            assert_eq!(state.load(Ordering::Relaxed), RUNNING);
            assert_eq!(handle_control(&state, event), 1);
            assert_eq!(handle_control(&state, event), 1);
            assert_eq!(state.load(Ordering::Relaxed), STOPPING);
        }
    }
}
