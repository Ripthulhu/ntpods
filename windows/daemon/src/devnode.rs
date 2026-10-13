//! Recover an AAP devnode in Code 38 (`CM_PROB_DRIVER_FAILED_PRIOR_UNLOAD`).
//! Release the exclusive device handle before restarting the devnode so the
//! previous driver instance can unload. Restarting requires administrator access;
//! the unelevated daemon triggers the installer-created "NTPods Fix Driver" task.
//! Its `fix-driver.ps1` rechecks the devnode and leaves healthy devices alone.

use std::io::{self, Write};
use std::os::windows::{ffi::OsStrExt, process::CommandExt};
use std::path::Path;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use windows_sys::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

const TASK_NAME: &str = "NTPods Fix Driver";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Limit recovery attempts because each devnode restart interrupts audio.
const COOLDOWN: Duration = Duration::from_secs(120);

static LAST_FIRED: Mutex<Option<Instant>> = Mutex::new(None);

fn write_request(directory: &Path, address: u64, now: SystemTime) -> io::Result<()> {
    if address == 0 || address > 0xffff_ffff_ffff {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid recovery address",
        ));
    }
    let time = now.duration_since(UNIX_EPOCH).map_err(io::Error::other)?;
    let temporary = directory.join(format!(
        "driver-recovery-{}-{}.tmp",
        std::process::id(),
        time.as_nanos()
    ));
    let destination = directory.join("driver-recovery.txt");
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        write!(file, "{address:012x} {}", time.as_secs())?;
        file.sync_all()?;
        drop(file);
        let wide = |path: &Path| {
            path.as_os_str()
                .encode_wide()
                .chain(Some(0))
                .collect::<Vec<_>>()
        };
        if unsafe {
            MoveFileExW(
                wide(&temporary).as_ptr(),
                wide(&destination).as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

/// Publish the selected pair before triggering the fixed elevated task.
pub fn request_recovery(address: u64) -> bool {
    {
        let mut last = LAST_FIRED.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|t| t.elapsed() < COOLDOWN) {
            return false;
        }
        let Some(base) = std::env::var_os("LOCALAPPDATA") else {
            return false;
        };
        if write_request(&Path::new(&base).join("NTPods"), address, SystemTime::now()).is_err() {
            return false;
        }
        *last = Some(Instant::now());
    }
    std::thread::spawn(|| {
        // Missing tasks fail quietly.
        let _ = Command::new("schtasks")
            .args(["/run", "/tn", TASK_NAME])
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

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
