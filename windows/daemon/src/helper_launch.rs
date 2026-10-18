//! Run the two installed task helpers without ever allocating a console.
use std::ffi::{OsStr, OsString};
use std::io::{self, Read};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use windows_sys::Win32::Security::{
    GetTokenInformation, ImpersonateLoggedOnUser, RevertToSelf, TokenElevationType,
    TokenElevationTypeLimited, TokenLinkedToken, TOKEN_LINKED_TOKEN, TOKEN_QUERY,
};
use windows_sys::Win32::System::Registry::{
    RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RRF_SUBKEY_WOW6464KEY,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

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

fn linked_user_token() -> io::Result<OwnedHandle> {
    unsafe {
        let mut process = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut process) == 0 {
            return Err(io::Error::last_os_error());
        }
        let process = OwnedHandle::from_raw_handle(process);
        let mut linked = TOKEN_LINKED_TOKEN {
            LinkedToken: std::ptr::null_mut(),
        };
        let mut bytes = 0;
        if GetTokenInformation(
            process.as_raw_handle(),
            TokenLinkedToken,
            (&mut linked as *mut TOKEN_LINKED_TOKEN).cast(),
            std::mem::size_of_val(&linked) as u32,
            &mut bytes,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(OwnedHandle::from_raw_handle(linked.LinkedToken))
    }
}

struct UserRead;

impl UserRead {
    fn begin(token: &OwnedHandle) -> io::Result<Self> {
        unsafe {
            let mut elevation = 0;
            let mut bytes = 0;
            if GetTokenInformation(
                token.as_raw_handle(),
                TokenElevationType,
                (&mut elevation as *mut i32).cast(),
                std::mem::size_of_val(&elevation) as u32,
                &mut bytes,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            if elevation != TokenElevationTypeLimited {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "A limited user token is required",
                ));
            }
            if ImpersonateLoggedOnUser(token.as_raw_handle()) == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self)
        }
    }
}

impl Drop for UserRead {
    fn drop(&mut self) {
        // Never launch the privileged writer with an uncertain thread identity.
        if unsafe { RevertToSelf() } == 0 {
            std::process::abort();
        }
    }
}

fn read_name(reader: impl Read) -> io::Result<String> {
    let mut bytes = Vec::new();
    reader.take(1025).read_to_end(&mut bytes)?;
    if bytes.len() > 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Microphone name file is too large",
        ));
    }
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid microphone name encoding",
        )
    })?;
    let name = text.trim();
    if name.is_empty()
        || name.encode_utf16().count() > 128
        || name.chars().any(|c| c <= '\u{1f}' || c == '\u{7f}')
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid microphone name",
        ));
    }
    Ok(name.to_owned())
}

fn user_name(path: &Path, token: &OwnedHandle) -> io::Result<String> {
    let _user = UserRead::begin(token)?;
    // Open under the limited token so links cannot grant administrator-only reads.
    read_name(std::fs::File::open(path)?)
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
    let mut command = hidden_command(&powershell, &path);
    if script == "rename-mic.ps1" {
        let request =
            PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "Local profile unavailable")
            })?)
            .join(r"NTPods\micname.txt");
        let name = user_name(&request, &linked_user_token()?)?;
        command.arg("-Name").arg(name);
    }
    Ok(command.status()?.code().unwrap_or(1))
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
    fn microphone_name_is_bounded_and_validated_before_elevation() {
        for name in [
            "AirPods Pro",
            "Dylan's AirPods",
            "Écouteurs 🎧",
            "-NoProfile",
        ] {
            assert_eq!(read_name(name.as_bytes()).unwrap(), name);
        }
        assert_eq!(
            read_name("🎧".repeat(64).as_bytes()).unwrap(),
            "🎧".repeat(64)
        );
        for value in [
            vec![0xff],
            vec![b'x'; 1025],
            vec![b'x'; 129],
            b"a\0b".to_vec(),
            b"a\nb".to_vec(),
            b"a\x7fb".to_vec(),
            b"   ".to_vec(),
            "🎧".repeat(65).into_bytes(),
        ] {
            assert_eq!(
                read_name(value.as_slice()).unwrap_err().kind(),
                io::ErrorKind::InvalidData
            );
        }
        let mut unlimited = io::repeat(b'x');
        let mut bounded = (&mut unlimited).take(2048);
        assert!(read_name(&mut bounded).is_err());
        assert_eq!(bounded.limit(), 1023);
    }

    #[test]
    fn microphone_name_reaches_powershell_as_literal_data() {
        let directory =
            std::env::temp_dir().join(format!("ntpods-name-args-{}", std::process::id()));
        std::fs::create_dir(&directory).unwrap();
        let script = directory.join("name.ps1");
        std::fs::write(
            &script,
            r#"param([Parameter(Mandatory=$true)][string]$Name)
[Console]::OutputEncoding = New-Object Text.UTF8Encoding($false)
[Console]::Out.Write($Name)"#,
        )
        .unwrap();
        let powershell = machine_directory(
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "SystemRoot",
        )
        .unwrap()
        .join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
        for name in [
            "Dylan's \"AirPods\" 🎧",
            "$(throw 'bad'); ` -NoProfile \\",
            "-ExecutionPolicy",
        ] {
            let result = hidden_command(&powershell, &script)
                .args(["-Name", name])
                .stdout(Stdio::piped())
                .output()
                .unwrap();
            assert!(result.status.success());
            assert_eq!(String::from_utf8(result.stdout).unwrap(), name);
        }
        assert!(!hidden_command(&powershell, &script)
            .status()
            .unwrap()
            .success());
        std::fs::remove_file(script).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

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

    #[test]
    fn rename_request_links_cannot_read_admin_only_files_and_restore_identity() {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Foundation::{LocalFree, ERROR_NO_TOKEN};
        use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
        use windows_sys::Win32::Security::{
            SetFileSecurityW, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
            TOKEN_DUPLICATE,
        };
        use windows_sys::Win32::System::Threading::{GetCurrentThread, OpenThreadToken};

        fn set_acl(path: &Path, sddl: &str) -> io::Result<()> {
            let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let sddl: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
            unsafe {
                let mut descriptor = std::ptr::null_mut();
                if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    1,
                    &mut descriptor,
                    std::ptr::null_mut(),
                ) == 0
                {
                    return Err(io::Error::last_os_error());
                }
                let changed = SetFileSecurityW(
                    path.as_ptr(),
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    descriptor,
                );
                let error = io::Error::last_os_error();
                LocalFree(descriptor);
                if changed == 0 {
                    Err(error)
                } else {
                    Ok(())
                }
            }
        }

        fn no_impersonation() {
            unsafe {
                let mut token = std::ptr::null_mut();
                let opened = OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token);
                if opened != 0 {
                    drop(OwnedHandle::from_raw_handle(token));
                    panic!("Name read left the test thread impersonating");
                }
                assert_eq!(
                    io::Error::last_os_error().raw_os_error(),
                    Some(ERROR_NO_TOKEN as i32)
                );
            }
        }

        let process = unsafe {
            let mut token = std::ptr::null_mut();
            assert_ne!(
                OpenProcessToken(
                    GetCurrentProcess(),
                    TOKEN_QUERY | TOKEN_DUPLICATE,
                    &mut token
                ),
                0
            );
            OwnedHandle::from_raw_handle(token)
        };
        no_impersonation();
        let token = if let Ok(guard) = UserRead::begin(&process) {
            drop(guard);
            process
        } else if let Ok(linked) = linked_user_token() {
            match UserRead::begin(&linked) {
                Ok(guard) => {
                    drop(guard);
                    linked
                }
                Err(_) => {
                    let error = user_name(
                        Path::new(r"Z:\ntpods-missing-fixture\micname.txt"),
                        &process,
                    )
                    .unwrap_err();
                    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
                    no_impersonation();
                    eprintln!("No usable limited UAC token: verified fail-closed behavior; native junction/ACL check unavailable.");
                    return;
                }
            }
        } else {
            let error = user_name(
                Path::new(r"Z:\ntpods-missing-fixture\micname.txt"),
                &process,
            )
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
            no_impersonation();
            eprintln!("No linked UAC token: verified fail-closed behavior; native junction/ACL check unavailable.");
            return;
        };
        no_impersonation();

        struct Fixture(PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let protected = self.0.join(r"target\micname.txt");
                // Restore owner access before removing only this fixture's files and junction.
                let _ = set_acl(&protected, "D:P(A;;FA;;;OW)");
                let _ = std::fs::remove_file(protected);
                let _ = std::fs::remove_file(self.0.join("ordinary.txt"));
                let _ = std::fs::remove_file(self.0.join("junction.ps1"));
                let _ = std::fs::remove_dir(self.0.join("redirect"));
                let _ = std::fs::remove_dir(self.0.join("target"));
                let _ = std::fs::remove_dir(&self.0);
            }
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let fixture = Fixture(std::env::temp_dir().join(format!(
            "ntpods-name-permission-{}-{stamp}",
            std::process::id()
        )));
        std::fs::create_dir(&fixture.0).unwrap();
        let target = fixture.0.join("target");
        std::fs::create_dir(&target).unwrap();
        let protected = target.join("micname.txt");
        std::fs::write(&protected, b"Synthetic administrator-only name").unwrap();
        set_acl(&protected, "D:P(A;;FA;;;BA)").unwrap();
        let ordinary = fixture.0.join("ordinary.txt");
        std::fs::write(&ordinary, b"AirPods Pro").unwrap();
        let script = fixture.0.join("junction.ps1");
        std::fs::write(&script, "param([string]$Link, [string]$Target)\n$ErrorActionPreference = 'Stop'\nNew-Item -ItemType Junction -Path $Link -Target $Target | Out-Null\n").unwrap();
        let powershell = machine_directory(
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "SystemRoot",
        )
        .unwrap()
        .join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
        assert!(hidden_command(&powershell, &script)
            .arg("-Link")
            .arg(fixture.0.join("redirect"))
            .arg("-Target")
            .arg(&target)
            .status()
            .unwrap()
            .success());

        assert_eq!(user_name(&ordinary, &token).unwrap(), "AirPods Pro");
        no_impersonation();
        let error = user_name(&fixture.0.join(r"redirect\micname.txt"), &token).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        no_impersonation();
        std::fs::write(&ordinary, [0xff]).unwrap();
        assert_eq!(
            user_name(&ordinary, &token).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        no_impersonation();
    }
}
