use std::io::{self, Write};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use windows_sys::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

static NEXT_FILE: AtomicU64 = AtomicU64::new(0);
static REPLACE_GATE: Mutex<()> = Mutex::new(());

pub fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_with(path, |file| file.write_all(bytes))
}

fn write_with(
    path: &Path,
    write: impl FnOnce(&mut std::fs::File) -> io::Result<()>,
) -> io::Result<()> {
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("Missing settings directory"))?;
    std::fs::create_dir_all(directory)?;
    let (temporary, mut file) = loop {
        let temporary = directory.join(format!(
            ".ntpods-{}-{}.tmp",
            std::process::id(),
            NEXT_FILE.fetch_add(1, Ordering::Relaxed)
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => break (temporary, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    };
    let result = (|| {
        write(&mut file)?;
        file.sync_all()
    })();
    drop(file);
    let result = result.and_then(|()| {
        let wide = |path: &Path| {
            path.as_os_str()
                .encode_wide()
                .chain(Some(0))
                .collect::<Vec<_>>()
        };
        // Concurrent Windows replacements can overlap transient destination handles.
        let _replace = REPLACE_GATE.lock().unwrap();
        if unsafe {
            MoveFileExW(
                wide(&temporary).as_ptr(),
                wide(path).as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    });
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;

    #[test]
    fn failed_write_or_replace_preserves_previous_settings_and_cleans_temporary() {
        let directory = std::env::temp_dir().join(format!("ntpods-atomic-{}", std::process::id()));
        let path = directory.join("settings.json");
        write(&path, b"old").unwrap();
        assert!(write_with(&path, |file| {
            file.write_all(b"partial")?;
            Err(io::Error::other("interrupted write"))
        })
        .is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
        let locked = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&path)
            .unwrap();
        assert!(write(&path, b"new").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"old");
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        drop(locked);
        write(&path, b"new").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn concurrent_replacements_leave_one_complete_file() {
        let directory =
            std::env::temp_dir().join(format!("ntpods-atomic-concurrent-{}", std::process::id()));
        let path = directory.join("settings");
        let ready = std::sync::Barrier::new(8);
        std::thread::scope(|scope| {
            for byte in 0..8 {
                let path = &path;
                let ready = &ready;
                scope.spawn(move || {
                    ready.wait();
                    for _ in 0..16 {
                        write(path, &[byte; 8192]).unwrap();
                    }
                });
            }
        });
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.len(), 8192);
        assert!(bytes.iter().all(|byte| *byte == bytes[0]));
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
