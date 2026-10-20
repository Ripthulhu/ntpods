//! A separate process restores the selected pair's HFP service when its owner exits.

use crate::hfp_profile;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle, process::CommandExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, TryLockError, Weak};
use std::thread;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE_FLAG_INHERIT};

const MAX_ADDRESS: u64 = 0xffff_ffff_ffff;
const READY_TIMEOUT: Duration = Duration::from_secs(8);
static PROFILE_GATE: Mutex<Weak<AtomicBool>> = Mutex::new(Weak::new());

pub struct Guard {
    child: Option<Child>,
    alive: Arc<AtomicBool>,
}

impl Guard {
    pub fn start(address: u64) -> io::Result<Self> {
        Self::start_if(address, || Ok(()))
    }

    pub fn start_if(address: u64, mut current: impl FnMut() -> io::Result<()>) -> io::Result<Self> {
        if address > MAX_ADDRESS {
            return Err(io::Error::other("Invalid hands-free profile target"));
        }
        // On an error the operation gate must drop before Guard restores the helper.
        let alive = Arc::new(AtomicBool::new(false));
        let mut guard = Self {
            child: None,
            alive: alive.clone(),
        };
        let mut gate = PROFILE_GATE
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if helper_active(&gate) {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Hands-free restoration is still active",
            ));
        }
        current()?;
        let child = Command::new(std::env::current_exe()?)
            .args(["--hfp-profile-guard", &address.to_string()])
            .creation_flags(0x0800_0000)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        alive.store(true, Ordering::Release);
        *gate = Arc::downgrade(&alive);
        guard.child = Some(child);
        let child = guard.child.as_mut().unwrap();
        let output = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("Hands-free helper has no output"))?;
        let (send, receive) = mpsc::sync_channel(1);
        thread::spawn(move || observe_helper(output, address, &alive, send));
        let input = child
            .stdin
            .as_ref()
            .ok_or_else(|| io::Error::other("Hands-free helper has no input"))?;
        // Later daemon children must not inherit this helper's lifetime lease.
        if unsafe { SetHandleInformation(input.as_raw_handle().cast(), HANDLE_FLAG_INHERIT, 0) }
            == 0
        {
            return Err(io::Error::last_os_error());
        }
        receive.recv_timeout(READY_TIMEOUT).map_err(|_| {
            io::Error::new(io::ErrorKind::TimedOut, "Hands-free preparation timed out")
        })??;
        if child.try_wait()?.is_some() || !guard.alive() {
            return Err(io::Error::other(
                "Hands-free helper exited after preparation",
            ));
        }
        current()?;
        Ok(guard)
    }

    pub fn alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }

    pub fn finish(&mut self) -> io::Result<()> {
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
        let _gate = PROFILE_GATE
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        drop(child.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(25);
        loop {
            match child.try_wait()? {
                Some(status) => {
                    self.alive.store(false, Ordering::Release);
                    return if status.success() {
                        Ok(())
                    } else {
                        Err(io::Error::other("Hands-free profile restoration failed"))
                    };
                }
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(100)),
                // Keep the independent helper alive to finish a slow Windows service call.
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "Hands-free profile restoration is pending",
                    ))
                }
            }
        }
    }
}

fn helper_active(active: &Weak<AtomicBool>) -> bool {
    active
        .upgrade()
        .is_some_and(|alive| alive.load(Ordering::Acquire))
}

pub fn after_release<T>(operation: impl FnOnce() -> T) -> io::Result<T> {
    after_release_with(&PROFILE_GATE, Duration::from_secs(30), operation)
}

fn after_release_with<T>(
    gate: &Mutex<Weak<AtomicBool>>,
    timeout: Duration,
    operation: impl FnOnce() -> T,
) -> io::Result<T> {
    let deadline = Instant::now() + timeout;
    loop {
        let active = match gate.try_lock() {
            Ok(active) => Some(active),
            Err(TryLockError::Poisoned(error)) => Some(error.into_inner()),
            Err(TryLockError::WouldBlock) => None,
        };
        if let Some(active) = active {
            if !helper_active(&active) {
                return Ok(operation());
            }
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Hands-free profile restoration is pending",
            ));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Err(error) = self.finish() {
            crate::log(&format!("handsfree profile: {error}"));
        }
    }
}

fn read_ready(mut input: impl Read, address: u64) -> io::Result<()> {
    let expected = format!("ready {address}\n");
    let mut line = Vec::new();
    let mut byte = [0];
    while input.read(&mut byte)? != 0 {
        line.push(byte[0]);
        if byte[0] == b'\n' {
            return if line == expected.as_bytes() {
                Ok(())
            } else {
                Err(io::Error::other(
                    "Invalid hands-free preparation acknowledgement",
                ))
            };
        }
        if line.len() >= 32 {
            return Err(io::Error::other(
                "Oversized hands-free preparation acknowledgement",
            ));
        }
    }
    Err(io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "Hands-free helper closed before preparation",
    ))
}

fn observe_helper(
    mut output: impl Read,
    address: u64,
    alive: &AtomicBool,
    send: mpsc::SyncSender<io::Result<()>>,
) {
    let result = read_ready(&mut output, address);
    let _ = send.send(result);
    // A failed acknowledgement can precede a slow restoration. Keep the gate
    // closed until the helper actually closes its output, including on errors.
    let mut buffer = [0; 64];
    while let Ok(count) = output.read(&mut buffer) {
        if count == 0 {
            break;
        }
    }
    alive.store(false, Ordering::Release);
}

fn target(args: &[std::ffi::OsString]) -> io::Result<u64> {
    let text = args.get(1).and_then(|value| value.to_str());
    if args.len() != 2
        || text.is_none_or(|value| value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err(io::Error::other("Invalid hands-free helper arguments"));
    }
    text.unwrap()
        .parse::<u64>()
        .ok()
        .filter(|address| *address <= MAX_ADDRESS)
        .ok_or_else(|| io::Error::other("Invalid hands-free profile target"))
}

fn directory() -> io::Result<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(|path| PathBuf::from(path).join("NTPods"))
        .ok_or_else(|| io::Error::other("Local app data is unavailable"))
}

pub fn recover() -> io::Result<()> {
    if hfp_profile::pending(&directory()?)? {
        Guard::start(0)?.finish()?;
    }
    Ok(())
}

fn ownership(directory: &Path, closed: &AtomicBool) -> io::Result<File> {
    std::fs::create_dir_all(directory)?;
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        if closed.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Hands-free owner exited",
            ));
        }
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(directory.join("hfp-profile.lock"))
        {
            Ok(file) => return Ok(file),
            Err(error) if error.raw_os_error() == Some(32) && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(100))
            }
            Err(error) => return Err(error),
        }
    }
}

fn run(address: u64) -> io::Result<()> {
    let closed = Arc::new(AtomicBool::new(false));
    let input_closed = closed.clone();
    let (send, receive) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let mut byte = [0];
        let result = match io::stdin().read(&mut byte) {
            Ok(0) => Ok(()),
            Ok(_) => Err(io::Error::other("Unexpected hands-free lease input")),
            Err(error) => Err(error),
        };
        input_closed.store(true, Ordering::Release);
        let _ = send.send(result);
    });
    let directory = directory()?;
    let _owner = ownership(&directory, &closed)?;
    let mut lease = hfp_profile::Lease::open(&directory)?;
    let result = (|| {
        lease.update(None)?;
        if address != 0 {
            lease.update(Some(address))?;
            if !lease.ready(address) {
                return Err(io::Error::other("Hands-free profile did not become ready"));
            }
        }
        if closed.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Hands-free owner exited",
            ));
        }
        writeln!(io::stdout(), "ready {address}")?;
        io::stdout().flush()?;
        receive
            .recv()
            .map_err(|_| io::Error::other("Hands-free lease monitor stopped"))?
    })();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match lease.update(None) {
            Ok(()) => return result,
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_) => thread::sleep(Duration::from_millis(250)),
        }
    }
}

pub fn run_requested() -> Option<i32> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.first().is_none_or(|arg| arg != "--hfp-profile-guard") {
        return None;
    }
    let result = target(&args).and_then(run);
    if let Err(error) = &result {
        crate::log(&format!("handsfree profile helper: {error}"));
    }
    Some(if result.is_ok() { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_bounded_decimal_targets_and_exact_arguments() {
        for address in [0, 1, MAX_ADDRESS] {
            assert_eq!(
                target(&["--hfp-profile-guard".into(), address.to_string().into()]).unwrap(),
                address
            );
        }
        for value in [
            "",
            "-1",
            "+1",
            "0x1",
            "1 ",
            "281474976710656",
            "18446744073709551616",
        ] {
            assert!(target(&["--hfp-profile-guard".into(), value.into()]).is_err());
        }
        assert!(target(&["--hfp-profile-guard".into()]).is_err());
        assert!(target(&["--hfp-profile-guard".into(), "1".into(), "extra".into()]).is_err());
    }

    #[test]
    fn acknowledgement_requires_matching_target_and_a_complete_bounded_line() {
        assert!(read_ready(&b"ready 0\n"[..], 0).is_ok());
        assert!(read_ready(&b"ready 1\n"[..], 1).is_ok());
        for value in [
            &b""[..],
            &b"ready 1"[..],
            &b"ready 2\n"[..],
            &b"noise\nready 1\n"[..],
            &b"ready 1\r\n"[..],
            &[b'x'; 64][..],
        ] {
            assert!(read_ready(value, 1).is_err());
        }
    }

    #[test]
    fn helper_exit_revokes_readiness_after_acknowledgement() {
        struct Output {
            ready: io::Cursor<&'static [u8]>,
            exit: mpsc::Receiver<()>,
        }
        impl Read for Output {
            fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
                let count = self.ready.read(bytes)?;
                if count == 0 {
                    let _ = self.exit.recv();
                }
                Ok(count)
            }
        }
        for (prefix, valid) in [(&b"ready 1\n"[..], true), (&b"invalid\n"[..], false)] {
            let (exit, finished) = mpsc::sync_channel(1);
            let (send, receive) = mpsc::sync_channel(1);
            let alive = Arc::new(AtomicBool::new(true));
            let child_alive = alive.clone();
            let observer = thread::spawn(move || {
                observe_helper(
                    Output {
                        ready: io::Cursor::new(prefix),
                        exit: finished,
                    },
                    1,
                    &child_alive,
                    send,
                );
            });
            assert_eq!(
                receive
                    .recv_timeout(Duration::from_secs(1))
                    .unwrap()
                    .is_ok(),
                valid
            );
            assert!(alive.load(Ordering::Acquire));
            drop(exit);
            observer.join().unwrap();
            assert!(!alive.load(Ordering::Acquire));
        }

        let alive = AtomicBool::new(true);
        let (send, receive) = mpsc::sync_channel(1);
        observe_helper(&b""[..], 1, &alive, send);
        assert!(receive.recv().unwrap().is_err());
        assert!(!alive.load(Ordering::Acquire));
    }

    #[test]
    fn final_operation_waits_for_restoration_and_excludes_a_new_lease() {
        let alive = Arc::new(AtomicBool::new(true));
        let gate = Arc::new(Mutex::new(Arc::downgrade(&alive)));
        let restored = Arc::new(AtomicBool::new(false));
        let worker_gate = gate.clone();
        let worker_restored = restored.clone();
        let worker = thread::spawn(move || {
            thread::sleep(Duration::from_millis(20));
            let _gate = worker_gate.lock().unwrap();
            worker_restored.store(true, Ordering::Release);
            alive.store(false, Ordering::Release);
        });
        after_release_with(&gate, Duration::from_secs(1), || {
            assert!(restored.load(Ordering::Acquire));
            assert!(matches!(gate.try_lock(), Err(TryLockError::WouldBlock)));
        })
        .unwrap();
        worker.join().unwrap();
    }

    #[test]
    fn incomplete_or_blocked_restoration_prevents_the_final_operation() {
        let alive = Arc::new(AtomicBool::new(true));
        let gate = Mutex::new(Arc::downgrade(&alive));
        let operation = || panic!("must not disconnect while a late restoration can enable HFP");
        assert!(after_release_with(&gate, Duration::from_millis(1), operation).is_err());
        let held = gate.lock().unwrap();
        assert!(after_release_with(&gate, Duration::from_millis(1), operation).is_err());
        drop(held);
        alive.store(false, Ordering::Release);
        assert_eq!(
            after_release_with(&gate, Duration::from_millis(1), || 1).unwrap(),
            1
        );
    }
}
