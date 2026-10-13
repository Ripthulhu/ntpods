//! Name the virtual microphone after the connected device.
//! Endpoint names are stored in HKLM, so the unelevated daemon writes the desired
//! name to a file and triggers the installer-created "NTPods Rename Mic" task.
//! `rename-mic.ps1` skips the write and audio-service restart when the name matches.

use std::os::windows::process::CommandExt;
use std::process::Command;

const TASK_NAME: &str = "NTPods Rename Mic";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Queue an endpoint rename without blocking or surfacing task errors.
/// If the task is missing, run `rename-mic.ps1 "<name>"` as administrator.
pub fn apply(dev_name: &str) {
    let name = dev_name.trim().to_string();
    if name.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        let la = match std::env::var("LOCALAPPDATA") {
            Ok(la) => la,
            Err(_) => return,
        };
        let dir = format!("{la}\\NTPods");
        let _ = std::fs::create_dir_all(&dir);
        if std::fs::write(format!("{dir}\\micname.txt"), &name).is_err() {
            return;
        }
        let _ = Command::new("schtasks")
            .args(["/run", "/tn", TASK_NAME])
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    });
}
