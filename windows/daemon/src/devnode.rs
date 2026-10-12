//! Recover an AAP devnode in Code 38 (`CM_PROB_DRIVER_FAILED_PRIOR_UNLOAD`).
//! Release the exclusive device handle before restarting the devnode so the
//! previous driver instance can unload. Restarting requires administrator access;
//! the unelevated daemon triggers the installer-created "NTPods Fix Driver" task.
//! Its `fix-driver.ps1` rechecks the devnode and leaves healthy devices alone.

use std::os::windows::process::CommandExt;
use std::process::Command;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const TASK_NAME: &str = "NTPods Fix Driver";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Limit recovery attempts because each devnode restart interrupts audio.
const COOLDOWN: Duration = Duration::from_secs(120);

static LAST_FIRED: Mutex<Option<Instant>> = Mutex::new(None);

/// Trigger recovery without blocking. False means the cooldown prevented a request.
pub fn request_recovery() -> bool {
    {
        let mut last = LAST_FIRED.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|t| t.elapsed() < COOLDOWN) {
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
