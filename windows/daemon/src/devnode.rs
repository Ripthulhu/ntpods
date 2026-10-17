//! Recover an AAP devnode in Code 38 (`CM_PROB_DRIVER_FAILED_PRIOR_UNLOAD`).
//! Release the exclusive device handle before restarting the devnode so the
//! previous driver instance can unload. Restarting requires administrator access;
//! the unelevated daemon triggers the installer-created "NTPods Fix Driver" task.
//! Its `fix-driver.ps1` rechecks the devnode and leaves healthy devices alone.

use std::io;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Limit recovery attempts because each devnode restart interrupts audio.
const COOLDOWN: Duration = Duration::from_secs(120);

static LAST_FIRED: Mutex<Option<Instant>> = Mutex::new(None);
static RECOVERING: AtomicBool = AtomicBool::new(false);
static RESTART_REQUIRED: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryResult {
    Ready,
    RestartRequired,
    Failed,
}

impl RecoveryResult {
    fn from_exit_code(code: Option<i32>) -> Self {
        match code {
            Some(0) => Self::Ready,
            Some(3010) => Self::RestartRequired,
            _ => Self::Failed,
        }
    }
}

pub fn recovering() -> bool {
    RECOVERING.load(Ordering::Acquire)
}

pub fn restart_required(address: u64) -> bool {
    address != 0 && RESTART_REQUIRED.load(Ordering::Acquire) == address
}

pub fn opened(address: u64) {
    let _ = RESTART_REQUIRED.compare_exchange(address, 0, Ordering::AcqRel, Ordering::Acquire);
}

fn write_request(directory: &Path, address: u64, now: SystemTime) -> io::Result<()> {
    if address == 0 || address > 0xffff_ffff_ffff {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid recovery address",
        ));
    }
    let time = now.duration_since(UNIX_EPOCH).map_err(io::Error::other)?;
    crate::atomic_file::write(
        &directory.join("driver-recovery.txt"),
        format!("{address:012x} {}", time.as_secs()).as_bytes(),
    )
}

fn run_recovery_task() -> io::Result<RecoveryResult> {
    let powershell = crate::helper_launch::machine_directory(
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "SystemRoot",
    )?
    .join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    let mut child = Command::new(powershell)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            include_str!("driver_recovery_task.ps1"),
        ])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(130);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(RecoveryResult::from_exit_code(status.code()));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(RecoveryResult::Failed);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Publish the selected pair before triggering the fixed elevated task.
pub fn request_recovery(
    address: u64,
    finished: impl FnOnce(RecoveryResult) + Send + 'static,
) -> bool {
    {
        let mut last = LAST_FIRED.lock().unwrap_or_else(|e| e.into_inner());
        if recovering() || restart_required(address) || last.is_some_and(|t| t.elapsed() < COOLDOWN)
        {
            return false;
        }
        let Some(base) = std::env::var_os("LOCALAPPDATA") else {
            return false;
        };
        if write_request(&Path::new(&base).join("NTPods"), address, SystemTime::now()).is_err() {
            return false;
        }
        *last = Some(Instant::now());
        RECOVERING.store(true, Ordering::Release);
    }
    std::thread::spawn(move || {
        let result = run_recovery_task().unwrap_or(RecoveryResult::Failed);
        if result == RecoveryResult::RestartRequired {
            RESTART_REQUIRED.store(address, Ordering::Release);
        }
        *LAST_FIRED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
        RECOVERING.store(false, Ordering::Release);
        finished(result);
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_requires_a_completed_verified_task_result() {
        assert_eq!(
            RecoveryResult::from_exit_code(Some(0)),
            RecoveryResult::Ready
        );
        assert_eq!(
            RecoveryResult::from_exit_code(Some(3010)),
            RecoveryResult::RestartRequired,
        );
        for code in [None, Some(1), Some(267009), Some(-1)] {
            assert_eq!(RecoveryResult::from_exit_code(code), RecoveryResult::Failed);
        }
    }

    #[test]
    fn recovery_request_replaces_only_the_fixed_file_and_rejects_invalid_addresses() {
        let directory =
            std::env::temp_dir().join(format!("ntpods-recovery-test-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let time = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        let path = directory.join("driver-recovery.txt");
        write_request(&directory, 1, time).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "000000000001 1800000000"
        );
        write_request(&directory, 0xabcdef012345, time).unwrap();
        let expected = "abcdef012345 1800000000";
        assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
        for address in [0, 0x1_0000_0000_0000, u64::MAX] {
            assert!(write_request(&directory, address, time).is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), expected);
        }
        assert!(write_request(&directory, 1, UNIX_EPOCH - Duration::from_secs(1)).is_err());
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
