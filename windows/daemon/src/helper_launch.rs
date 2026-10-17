//! Run the two installed task helpers without ever allocating a console.
use std::ffi::{OsStr, OsString};
use std::io;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use windows_sys::Win32::System::Registry::{
    RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY,
};

fn helper_script(args: &[OsString]) -> Option<&'static str> {
    if args.len() != 2 || args[0] != "--run-helper" {
        return None;
    }
    match args[1].to_str()? {
        "rename-mic" => Some("rename-mic.ps1"),
        "fix-driver" => Some("fix-driver.ps1"),
        _ => None,
    }
}

pub(crate) fn machine_directory(key: &str, value: &str) -> io::Result<PathBuf> {
    let mut buffer = [0u16; 32768];
    let mut bytes = std::mem::size_of_val(&buffer) as u32;
    let result = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            crate::wide(key).as_ptr(),
            crate::wide(value).as_ptr(),
            RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    let text = String::from_utf16(&buffer[..bytes as usize / 2])
        .map_err(|_| io::Error::other("Invalid system directory"))?;
    Ok(PathBuf::from(text.trim_end_matches('\0')))
}

fn installed_script(exe: &Path, install: &Path, script: &str) -> io::Result<PathBuf> {
    let root = install.canonicalize()?;
    if exe.canonicalize()?.parent() != Some(root.as_path()) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Helper must run from the installed app",
        ));
    }
    let path = root.join(script);
    if path.canonicalize()? != path {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Helper must remain in the installed app",
        ));
    }
    Ok(path)
}

fn hidden_command(powershell: &Path, script: &Path) -> Command {
    let mut command = Command::new(powershell);
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(script)
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW applies before PowerShell starts.
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn run(script: &str) -> io::Result<i32> {
    // Read machine paths, not the elevated task's inherited environment or PATH.
    let install = machine_directory(
        r"SOFTWARE\Microsoft\Windows\CurrentVersion",
        "ProgramFilesDir",
    )?
    .join("NTPods");
    let powershell = machine_directory(
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        "SystemRoot",
    )?
    .join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    let path = installed_script(&std::env::current_exe()?, &install, script)?;
    Ok(hidden_command(&powershell, &path)
        .status()?
        .code()
        .unwrap_or(1))
}

pub fn run_requested() -> Option<i32> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().map(OsString::as_os_str) != Some(OsStr::new("--run-helper")) {
        return None;
    }
    Some(helper_script(&args).map_or(2, |script| run(script).unwrap_or(1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_helpers_accept_only_two_fixed_commands() {
        let parse =
            |args: &[&str]| helper_script(&args.iter().map(OsString::from).collect::<Vec<_>>());
        assert_eq!(
            parse(&["--run-helper", "rename-mic"]),
            Some("rename-mic.ps1")
        );
        assert_eq!(
            parse(&["--run-helper", "fix-driver"]),
            Some("fix-driver.ps1")
        );
        for args in [
            vec![],
            vec!["--run-helper"],
            vec!["--run-helper", "other.ps1"],
            vec!["--run-helper", "..\\rename-mic"],
            vec!["--run-helper", "rename-mic", "-Name", "other"],
        ] {
            assert_eq!(parse(&args), None);
        }
    }

    #[test]
    fn hidden_helper_has_no_console_and_preserves_exit_status() {
        let directory =
            std::env::temp_dir().join(format!("ntpods-helper-test-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let script = directory.join("probe.ps1");
        let exe = directory.join("ntpodsd.exe");
        std::fs::write(&exe, b"fixture").unwrap();
        std::fs::write(&script, r#"$ErrorActionPreference = 'Stop'
Add-Type -Name ConsoleProbe -Namespace NTPodsTest -MemberDefinition '[DllImport("kernel32.dll")] public static extern IntPtr GetConsoleWindow();'
if ([NTPodsTest.ConsoleProbe]::GetConsoleWindow() -ne [IntPtr]::Zero) { exit 1 }
exit 3010"#).unwrap();
        let powershell = machine_directory(
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "SystemRoot",
        )
        .unwrap()
        .join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
        let path = installed_script(&exe, &directory, "probe.ps1").unwrap();
        assert!(
            installed_script(&std::env::current_exe().unwrap(), &directory, "probe.ps1").is_err()
        );
        let result = hidden_command(&powershell, &path).status();
        std::fs::remove_file(script).unwrap();
        std::fs::remove_file(exe).unwrap();
        std::fs::remove_dir(directory).unwrap();
        assert_eq!(result.unwrap().code(), Some(3010));
    }
}
