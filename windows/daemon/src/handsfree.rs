//! Keep hands-free endpoints out of device pickers while the HQ route is available.

use crate::hfp_endpoints::{Endpoint, Endpoints};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::windows::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawHandle, process::CommandExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE_FLAG_INHERIT};
use windows_sys::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

const DISABLED: u32 = 2;
const MAX_RECORDS: usize = 32;
const MAX_JOURNAL_BYTES: u64 = 64 * 1024;
const MAX_ADDRESS: u64 = 0xffff_ffff_ffff;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Restore {
    address: u64,
    id: String,
    adapter: String,
    state: u32,
}

impl Restore {
    fn new(address: u64, endpoint: &Endpoint) -> Self {
        Self {
            address,
            id: endpoint.id.clone(),
            adapter: endpoint.adapter.clone(),
            state: endpoint.state,
        }
    }
}

fn validate(records: &[Restore]) -> io::Result<()> {
    if records.len() > MAX_RECORDS
        || records.iter().enumerate().any(|(index, record)| {
            record.address == 0
                || record.address > MAX_ADDRESS
                || !matches!(record.state, 1 | 4 | 8)
                || [&record.id, &record.adapter]
                    .iter()
                    .any(|value| value.is_empty() || value.len() > 1024 || value.contains('\0'))
                || records[..index].iter().any(|old| old.id == record.id)
        })
    {
        return Err(io::Error::other("Invalid hands-free restoration record"));
    }
    Ok(())
}

trait Devices {
    fn list(&self, address: u64) -> io::Result<Vec<Endpoint>>;
    fn state(&self, id: &str) -> io::Result<Option<u32>>;
    fn replacement_ready(&self, address: u64) -> io::Result<bool>;
    fn visible(&self, endpoint: &Endpoint, visible: bool) -> io::Result<()>;
}

impl Devices for Endpoints {
    fn list(&self, address: u64) -> io::Result<Vec<Endpoint>> {
        self.list(address).map_err(io::Error::other)
    }

    fn state(&self, id: &str) -> io::Result<Option<u32>> {
        self.state(id).map_err(io::Error::other)
    }

    fn replacement_ready(&self, address: u64) -> io::Result<bool> {
        self.replacement_ready(address).map_err(io::Error::other)
    }

    fn visible(&self, endpoint: &Endpoint, visible: bool) -> io::Result<()> {
        self.set_visible(endpoint, visible).map_err(|error| {
            if error.code().0 == 0x8000_000cu32 as i32 {
                io::Error::new(io::ErrorKind::AlreadyExists, error)
            } else {
                io::Error::other(error)
            }
        })
    }
}

trait Journal {
    fn save(&mut self, records: &[Restore]) -> io::Result<()>;
}

struct FileJournal(PathBuf);

impl FileJournal {
    fn read(&self) -> io::Result<Vec<Restore>> {
        let file = match File::open(&self.0) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        if file.metadata()?.len() > MAX_JOURNAL_BYTES {
            return Err(io::Error::other(
                "Hands-free restoration record is too large",
            ));
        }
        let records: Vec<Restore> = serde_json::from_reader(file.take(MAX_JOURNAL_BYTES + 1))?;
        validate(&records)?;
        Ok(records)
    }
}

impl Journal for FileJournal {
    fn save(&mut self, records: &[Restore]) -> io::Result<()> {
        validate(records)?;
        if records.is_empty() {
            return match std::fs::remove_file(&self.0) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                result => result,
            };
        }
        let bytes = serde_json::to_vec(records)?;
        if bytes.len() as u64 > MAX_JOURNAL_BYTES {
            return Err(io::Error::other(
                "Hands-free restoration record is too large",
            ));
        }
        let temporary = self.0.with_extension("json.tmp");
        let mut file = File::create(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        let wide = |path: &Path| {
            path.as_os_str()
                .encode_wide()
                .chain(Some(0))
                .collect::<Vec<_>>()
        };
        // Windows rename does not replace an existing file; keep the old journal on failure.
        if unsafe {
            MoveFileExW(
                wide(&temporary).as_ptr(),
                wide(&self.0).as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

#[derive(Default)]
struct Lease {
    records: Vec<Restore>,
    target: Option<u64>,
}

impl Lease {
    fn restore(&mut self, devices: &impl Devices, journal: &mut impl Journal) -> io::Result<()> {
        self.target = None;
        let remaining = self
            .records
            .iter()
            .filter(|saved| {
                let restored = devices.list(saved.address).and_then(|endpoints| {
                    let current = endpoints.iter().find(|endpoint| {
                        endpoint.id == saved.id && endpoint.adapter == saved.adapter
                    });
                    let Some(current) = current else {
                        // Windows retains endpoint IDs after removing their adapters.
                        return match devices.state(&saved.id)? {
                            None => Ok(()),
                            Some(state) if state & DISABLED == 0 && state & (1 | 4 | 8) != 0 => {
                                Ok(())
                            }
                            _ => Err(io::Error::other("Hands-free endpoint is not available")),
                        };
                    };
                    if current.state & DISABLED != 0 {
                        devices.visible(current, true)?;
                    }
                    Ok(())
                });
                restored.is_err()
            })
            .cloned()
            .collect::<Vec<_>>();
        journal.save(&remaining)?;
        self.records = remaining;
        if !self.records.is_empty() {
            return Err(io::Error::other("Hands-free restoration is pending"));
        }
        Ok(())
    }

    fn update(
        &mut self,
        target: Option<u64>,
        devices: &impl Devices,
        journal: &mut impl Journal,
    ) -> io::Result<()> {
        let target = match target {
            Some(address) => match devices.replacement_ready(address) {
                Ok(true) => Some(address),
                Ok(false) => None,
                Err(error) => {
                    self.restore(devices, journal)?;
                    return Err(error);
                }
            },
            None => None,
        };
        if self.target != target || (target.is_none() && !self.records.is_empty()) {
            self.restore(devices, journal)?;
            self.target = target;
        }
        let Some(address) = target else { return Ok(()) };
        for endpoint in devices.list(address)? {
            if endpoint.state & DISABLED != 0 {
                continue;
            }
            let already_owned = self.records.iter().any(|saved| {
                saved.address == address
                    && saved.id == endpoint.id
                    && saved.adapter == endpoint.adapter
            });
            if !already_owned {
                let mut records = self.records.clone();
                records.push(Restore::new(address, &endpoint));
                validate(&records)?;
                // Do not disable anything until recovery survives a process or power failure.
                journal.save(&records)?;
                self.records = records;
            }
            if let Err(error) = devices.visible(&endpoint, false) {
                if !already_owned && error.kind() == io::ErrorKind::AlreadyExists {
                    // A user disabled this endpoint between enumeration and the final check.
                    self.records.retain(|saved| saved.id != endpoint.id);
                    journal.save(&self.records)?;
                    continue;
                }
                return Err(error);
            }
        }
        Ok(())
    }
}

fn directory() -> io::Result<PathBuf> {
    Ok(PathBuf::from(
        std::env::var_os("LOCALAPPDATA")
            .ok_or_else(|| io::Error::other("Local app data is unavailable"))?,
    )
    .join("NTPods"))
}

fn ownership(directory: &Path) -> io::Result<File> {
    std::fs::create_dir_all(directory)?;
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(directory.join("handsfree.lock"))
        {
            Ok(file) => return Ok(file),
            Err(error) if error.raw_os_error() == Some(32) && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(100));
            }
            Err(error) => return Err(error),
        }
    }
}

fn journal_is_clean() -> io::Result<bool> {
    let directory = directory()?;
    let _owner = ownership(&directory)?;
    Ok(FileJournal(directory.join("handsfree-restore.json"))
        .read()?
        .is_empty())
}

fn restore_bounded(
    lease: &mut Lease,
    devices: &impl Devices,
    journal: &mut impl Journal,
) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match lease.restore(devices, journal) {
            Ok(()) => return Ok(()),
            Err(error) if Instant::now() >= deadline => return Err(error),
            Err(_) => thread::sleep(Duration::from_millis(500)),
        }
    }
}

fn read_targets(mut input: impl Read, mut receive: impl FnMut(u64)) -> io::Result<()> {
    let mut line = [0u8; 16];
    let mut length = 0;
    let mut byte = [0u8; 1];
    while input.read(&mut byte)? != 0 {
        if byte[0] == b'\n' {
            let text = std::str::from_utf8(&line[..length])
                .map_err(|_| io::Error::other("Invalid hands-free target"))?;
            let address = text
                .parse::<u64>()
                .ok()
                .filter(|value| *value <= MAX_ADDRESS);
            if text.is_empty()
                || !text.bytes().all(|value| value.is_ascii_digit())
                || address.is_none()
            {
                return Err(io::Error::other("Invalid hands-free target"));
            }
            receive(address.unwrap());
            length = 0;
        } else {
            if length == line.len() || !byte[0].is_ascii_digit() {
                return Err(io::Error::other("Invalid hands-free target"));
            }
            line[length] = byte[0];
            length += 1;
        }
    }
    if length != 0 {
        return Err(io::Error::other("Incomplete hands-free target"));
    }
    Ok(())
}

fn run_guard() -> io::Result<()> {
    let target = Arc::new(AtomicU64::new(0));
    let closed = Arc::new(AtomicBool::new(false));
    let input_target = target.clone();
    let input_closed = closed.clone();
    thread::spawn(move || {
        let _ = read_targets(io::stdin().lock(), |address| {
            input_target.store(address, Ordering::Release);
        });
        input_closed.store(true, Ordering::Release);
    });
    let directory = directory()?;
    let _owner = ownership(&directory)?;
    let mut journal = FileJournal(directory.join("handsfree-restore.json"));
    let mut lease = Lease {
        records: journal.read()?,
        target: None,
    };
    let devices = match Endpoints::new() {
        Ok(devices) => devices,
        Err(_) if lease.records.is_empty() => {
            crate::log("handsfree: device visibility is unavailable on this Windows version");
            while !closed.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(250));
            }
            return Ok(());
        }
        Err(error) => return Err(io::Error::other(error)),
    };
    restore_bounded(&mut lease, &devices, &mut journal)?;
    let mut retry_at = Instant::now();
    let mut requested = None;
    let mut reported_count = 0;
    let mut warned = false;
    while !closed.load(Ordering::Acquire) {
        let address = target.load(Ordering::Acquire);
        let changed = requested != Some(address);
        if changed {
            requested = Some(address);
            retry_at = Instant::now();
            warned = false;
        }
        if Instant::now() >= retry_at {
            match lease.update((address != 0).then_some(address), &devices, &mut journal) {
                Ok(()) => warned = false,
                Err(error) => {
                    if !warned {
                        crate::log(&format!("handsfree: visibility update failed: {error}"));
                        warned = true;
                    }
                    restore_bounded(&mut lease, &devices, &mut journal)?;
                    retry_at = Instant::now() + Duration::from_secs(5);
                }
            }
            if changed || reported_count != lease.records.len() {
                reported_count = lease.records.len();
                crate::log(&format!(
                    "handsfree: selected={} hidden endpoints={reported_count}",
                    address != 0
                ));
            }
        }
        thread::sleep(Duration::from_secs(1));
    }
    restore_bounded(&mut lease, &devices, &mut journal)
}

pub fn run_requested() -> Option<i32> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.len() != 1 || args[0] != "--handsfree-guard" {
        return None;
    }
    let result = run_guard();
    if let Err(error) = &result {
        crate::log(&format!("handsfree: guard stopped: {error}"));
    }
    Some(if result.is_ok() { 0 } else { 1 })
}

fn spawn_guard() -> io::Result<Child> {
    let mut child = Command::new(std::env::current_exe()?)
        .arg("--handsfree-guard")
        .creation_flags(0x0800_0000)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let input = child
        .stdin
        .as_ref()
        .ok_or_else(|| io::Error::other("Helper has no input"))?;
    // Future daemon children must not keep the guard alive after this process exits.
    if unsafe { SetHandleInformation(input.as_raw_handle().cast(), HANDLE_FLAG_INHERIT, 0) } == 0 {
        let error = io::Error::last_os_error();
        let _ = finish_child(&mut child);
        return Err(error);
    }
    Ok(child)
}

fn finish_child(child: &mut Child) -> Option<bool> {
    // Closing the only writer also restores endpoints when the daemon crashes.
    drop(child.stdin.take());
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status.success()),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(100)),
            _ => return None,
        }
    }
}

pub fn start(ctx: crate::Ctx) {
    thread::spawn(move || {
        let mut warned = false;
        loop {
            let result = spawn_guard();
            let mut child = match result {
                Ok(child) => child,
                Err(_) => {
                    if !warned {
                        crate::log("handsfree: visibility helper could not start");
                        warned = true;
                    }
                    if ctx.shutting_down.load(Ordering::Acquire) {
                        if !matches!(journal_is_clean(), Ok(true)) {
                            ctx.shutdown_cleanup_failed.store(true, Ordering::Release);
                        }
                        ctx.handsfree_done.store(true, Ordering::Release);
                        return;
                    }
                    thread::sleep(Duration::from_secs(3));
                    continue;
                }
            };
            let mut sent = None;
            loop {
                if ctx.shutting_down.load(Ordering::Acquire) {
                    let finished = finish_child(&mut child);
                    if finished != Some(true) {
                        ctx.shutdown_cleanup_failed.store(true, Ordering::Release);
                        crate::log("handsfree: endpoint restoration is still pending");
                    }
                    ctx.handsfree_done
                        .store(finished.is_some(), Ordering::Release);
                    return;
                }
                let target = crate::handsfree_target(&ctx).unwrap_or(0);
                if sent != Some(target) {
                    let result = child
                        .stdin
                        .as_mut()
                        .ok_or_else(|| io::Error::other("Helper closed"))
                        .and_then(|input| writeln!(input, "{target}"));
                    if result.is_err() {
                        break;
                    }
                    sent = Some(target);
                }
                if !matches!(child.try_wait(), Ok(None)) {
                    break;
                }
                thread::sleep(Duration::from_millis(500));
            }
            if finish_child(&mut child).is_none() {
                ctx.shutdown_cleanup_failed.store(true, Ordering::Release);
                crate::log("handsfree: visibility helper did not finish restoring endpoints");
                return;
            }
            if !warned {
                crate::log("handsfree: retrying endpoint visibility helper");
                warned = true;
            }
            thread::sleep(Duration::from_secs(3));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeDevices {
        endpoints: RefCell<Vec<(u64, Endpoint)>>,
        calls: RefCell<Vec<(String, bool)>>,
        fail_restore: bool,
        disable_race: bool,
        ready: bool,
        fail_ready: bool,
        fail_state: bool,
    }

    impl Devices for FakeDevices {
        fn replacement_ready(&self, _: u64) -> io::Result<bool> {
            if self.fail_ready {
                Err(io::Error::other("readiness failure"))
            } else {
                Ok(self.ready)
            }
        }

        fn state(&self, id: &str) -> io::Result<Option<u32>> {
            if self.fail_state {
                return Err(io::Error::other("state query failure"));
            }
            Ok(self
                .endpoints
                .borrow()
                .iter()
                .find(|(_, endpoint)| endpoint.id == id)
                .map(|(_, endpoint)| endpoint.state))
        }

        fn list(&self, address: u64) -> io::Result<Vec<Endpoint>> {
            Ok(self
                .endpoints
                .borrow()
                .iter()
                .filter(|(owner, _)| *owner == address)
                .map(|(_, endpoint)| endpoint.clone())
                .collect())
        }

        fn visible(&self, endpoint: &Endpoint, visible: bool) -> io::Result<()> {
            if !visible && self.disable_race {
                self.endpoints
                    .borrow_mut()
                    .iter_mut()
                    .find(|(_, current)| current.id == endpoint.id)
                    .unwrap()
                    .1
                    .state = DISABLED;
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "already disabled",
                ));
            }
            if visible && self.fail_restore {
                return Err(io::Error::other("restore failure"));
            }
            self.calls.borrow_mut().push((endpoint.id.clone(), visible));
            for (_, current) in self.endpoints.borrow_mut().iter_mut() {
                if current.id == endpoint.id {
                    current.state = if visible { 1 } else { DISABLED };
                }
            }
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeJournal {
        records: Vec<Restore>,
        fail: bool,
    }

    impl Journal for FakeJournal {
        fn save(&mut self, records: &[Restore]) -> io::Result<()> {
            if self.fail {
                return Err(io::Error::other("write failure"));
            }
            self.records = records.to_vec();
            Ok(())
        }
    }

    fn devices() -> FakeDevices {
        FakeDevices {
            endpoints: RefCell::new(vec![
                (
                    1,
                    Endpoint {
                        id: "mic".into(),
                        adapter: "one".into(),
                        state: 1,
                    },
                ),
                (
                    1,
                    Endpoint {
                        id: "speaker".into(),
                        adapter: "one".into(),
                        state: DISABLED,
                    },
                ),
                (
                    2,
                    Endpoint {
                        id: "other".into(),
                        adapter: "two".into(),
                        state: 1,
                    },
                ),
            ]),
            calls: RefCell::new(Vec::new()),
            fail_restore: false,
            disable_race: false,
            ready: true,
            fail_ready: false,
            fail_state: false,
        }
    }

    #[test]
    fn preserves_existing_disabled_and_other_pairs() {
        let devices = devices();
        let mut lease = Lease::default();
        let mut journal = FakeJournal::default();
        lease.update(Some(1), &devices, &mut journal).unwrap();
        lease.update(None, &devices, &mut journal).unwrap();
        assert_eq!(
            *devices.calls.borrow(),
            [("mic".into(), false), ("mic".into(), true)]
        );
        assert!(journal.records.is_empty());
    }

    #[test]
    fn unavailable_replacement_keeps_handsfree_available() {
        let mut devices = devices();
        devices.ready = false;
        let mut lease = Lease::default();
        let mut journal = FakeJournal::default();
        lease.update(Some(1), &devices, &mut journal).unwrap();
        assert!(journal.records.is_empty());
        assert!(devices.calls.borrow().is_empty());
        assert_eq!(lease.target, None);
    }

    #[test]
    fn losing_replacement_restores_owned_endpoint() {
        let mut devices = devices();
        let mut lease = Lease::default();
        let mut journal = FakeJournal::default();
        lease.update(Some(1), &devices, &mut journal).unwrap();
        devices.ready = false;
        lease.update(Some(1), &devices, &mut journal).unwrap();
        assert!(journal.records.is_empty());
        assert_eq!(
            *devices.calls.borrow(),
            [("mic".into(), false), ("mic".into(), true)]
        );
        devices.ready = true;
        lease.update(Some(1), &devices, &mut journal).unwrap();
        assert_eq!(journal.records.len(), 1);
    }

    #[test]
    fn replacement_query_failure_restores_before_reporting_error() {
        let mut devices = devices();
        let mut lease = Lease::default();
        let mut journal = FakeJournal::default();
        lease.update(Some(1), &devices, &mut journal).unwrap();
        devices.fail_ready = true;
        assert!(lease.update(Some(1), &devices, &mut journal).is_err());
        assert!(journal.records.is_empty());
        assert_eq!(
            *devices.calls.borrow(),
            [("mic".into(), false), ("mic".into(), true)]
        );
    }

    #[test]
    fn write_failure_never_hides_endpoint() {
        let devices = devices();
        let mut lease = Lease {
            target: Some(1),
            ..Default::default()
        };
        let mut journal = FakeJournal {
            fail: true,
            ..Default::default()
        };
        assert!(lease.update(Some(1), &devices, &mut journal).is_err());
        assert!(devices.calls.borrow().is_empty());
    }

    #[test]
    fn disable_race_drops_intent_without_enabling_user_disabled_endpoint() {
        let mut devices = devices();
        devices.disable_race = true;
        let mut lease = Lease::default();
        let mut journal = FakeJournal::default();
        lease.update(Some(1), &devices, &mut journal).unwrap();
        lease.restore(&devices, &mut journal).unwrap();
        assert!(devices.calls.borrow().is_empty());
        assert!(journal.records.is_empty());
        assert_eq!(devices.endpoints.borrow()[0].1.state, DISABLED);
    }

    #[test]
    fn interrupted_write_ahead_needs_no_visibility_change() {
        let devices = devices();
        let records = vec![Restore::new(1, &devices.endpoints.borrow()[0].1)];
        let mut lease = Lease {
            records: records.clone(),
            target: None,
        };
        let mut journal = FakeJournal {
            records,
            fail: false,
        };
        lease.restore(&devices, &mut journal).unwrap();
        assert!(journal.records.is_empty());
        assert!(devices.calls.borrow().is_empty());
    }

    #[test]
    fn failed_cleanup_write_retains_recovery_record() {
        let devices = devices();
        let mut lease = Lease::default();
        let mut journal = FakeJournal::default();
        lease.update(Some(1), &devices, &mut journal).unwrap();
        journal.fail = true;
        assert!(lease.restore(&devices, &mut journal).is_err());
        assert_eq!(journal.records.len(), 1);
        assert_eq!(lease.records.len(), 1);
        journal.fail = false;
        lease.restore(&devices, &mut journal).unwrap();
        assert!(journal.records.is_empty());
        assert_eq!(devices.calls.borrow().len(), 2);
    }

    #[test]
    fn restore_failure_keeps_journal_and_blocks_new_pair() {
        let mut devices = devices();
        let mut lease = Lease::default();
        let mut journal = FakeJournal::default();
        lease.update(Some(1), &devices, &mut journal).unwrap();
        devices.fail_restore = true;
        assert!(lease.update(Some(2), &devices, &mut journal).is_err());
        assert_eq!(journal.records.len(), 1);
        assert_eq!(journal.records[0].address, 1);
        assert_eq!(*devices.calls.borrow(), [("mic".into(), false)]);
        devices.fail_restore = false;
        lease.update(Some(2), &devices, &mut journal).unwrap();
        lease.restore(&devices, &mut journal).unwrap();
        assert!(journal.records.is_empty());
        assert_eq!(
            *devices.calls.borrow(),
            [
                ("mic".into(), false),
                ("mic".into(), true),
                ("other".into(), false),
                ("other".into(), true),
            ]
        );
    }

    #[test]
    fn reused_adapter_or_address_does_not_restore_another_endpoint() {
        let devices = devices();
        let mut lease = Lease::default();
        let mut journal = FakeJournal::default();
        lease.update(Some(1), &devices, &mut journal).unwrap();
        devices.endpoints.borrow_mut()[0].1.adapter = "replacement".into();
        assert!(lease.restore(&devices, &mut journal).is_err());
        assert_eq!(journal.records.len(), 1);
        assert_eq!(devices.calls.borrow().len(), 1);
    }

    #[test]
    fn removed_endpoint_clears_journal_but_moved_identity_does_not() {
        let devices = devices();
        let mut lease = Lease::default();
        let mut journal = FakeJournal::default();
        lease.update(Some(1), &devices, &mut journal).unwrap();
        devices.endpoints.borrow_mut()[0].0 = 2;
        assert!(lease.restore(&devices, &mut journal).is_err());
        assert_eq!(journal.records.len(), 1);
        devices.endpoints.borrow_mut().remove(0);
        lease.restore(&devices, &mut journal).unwrap();
        assert!(journal.records.is_empty());
        assert_eq!(devices.calls.borrow().len(), 1);
    }

    #[test]
    fn cached_visible_endpoint_without_adapter_clears_journal_without_writing() {
        for state in [1, 4, 8, 0x1000_0004] {
            let devices = devices();
            let mut lease = Lease::default();
            let mut journal = FakeJournal::default();
            lease.update(Some(1), &devices, &mut journal).unwrap();
            {
                let mut endpoints = devices.endpoints.borrow_mut();
                endpoints[0].0 = 0;
                endpoints[0].1.state = state;
            }
            lease.restore(&devices, &mut journal).unwrap();
            assert!(journal.records.is_empty());
            assert_eq!(*devices.calls.borrow(), [("mic".into(), false)]);
        }
    }

    #[test]
    fn unmatched_endpoint_with_unreadable_or_unknown_state_keeps_journal() {
        let mut devices = devices();
        let mut lease = Lease::default();
        let mut journal = FakeJournal::default();
        lease.update(Some(1), &devices, &mut journal).unwrap();
        devices.endpoints.borrow_mut()[0].0 = 0;
        devices.fail_state = true;
        assert!(lease.restore(&devices, &mut journal).is_err());
        assert_eq!(journal.records.len(), 1);
        devices.fail_state = false;
        devices.endpoints.borrow_mut()[0].1.state = 0;
        assert!(lease.restore(&devices, &mut journal).is_err());
        assert_eq!(journal.records.len(), 1);
        assert_eq!(*devices.calls.borrow(), [("mic".into(), false)]);
    }

    #[test]
    fn eof_restores_the_current_pair() {
        let devices = devices();
        let mut lease = Lease::default();
        let mut journal = FakeJournal::default();
        read_targets(&b"1\n2\n"[..], |target| {
            lease.update(Some(target), &devices, &mut journal).unwrap();
        })
        .unwrap();
        lease.restore(&devices, &mut journal).unwrap();
        assert!(journal.records.is_empty());
        assert!(devices
            .endpoints
            .borrow()
            .iter()
            .filter(|(_, endpoint)| endpoint.id != "speaker")
            .all(|(_, endpoint)| endpoint.state == 1));
    }

    #[test]
    fn input_and_journal_bounds_are_enforced() {
        for input in ["", "0\n", "281474976710655\n"] {
            assert!(read_targets(input.as_bytes(), |_| {}).is_ok());
        }
        for input in [
            "1",
            "\n",
            "-1\n",
            "1 2\n",
            "281474976710656\n",
            "00000000000000000\n",
        ] {
            assert!(read_targets(input.as_bytes(), |_| {}).is_err(), "{input:?}");
        }
        let record = Restore {
            address: 1,
            id: "mic".into(),
            adapter: "one".into(),
            state: 1,
        };
        assert!(validate(std::slice::from_ref(&record)).is_ok());
        assert!(validate(&vec![record.clone(); MAX_RECORDS + 1]).is_err());
        assert!(validate(&[record.clone(), record.clone()]).is_err());
        assert!(validate(&[Restore {
            state: DISABLED,
            ..record
        }])
        .is_err());
    }

    #[test]
    fn file_journal_replaces_clears_and_rejects_oversized_input() {
        let directory = std::env::temp_dir().join(format!(
            "ntpods-handsfree-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let owner = ownership(&directory).unwrap();
        let second = OpenOptions::new()
            .read(true)
            .write(true)
            .share_mode(0)
            .open(directory.join("handsfree.lock"));
        assert_eq!(second.unwrap_err().raw_os_error(), Some(32));
        let mut journal = FileJournal(directory.join("handsfree-restore.json"));
        let mut record = Restore {
            address: 1,
            id: "mic".into(),
            adapter: "one".into(),
            state: 1,
        };
        journal.save(std::slice::from_ref(&record)).unwrap();
        assert_eq!(journal.read().unwrap(), [record.clone()]);
        record.address = 2;
        journal.save(std::slice::from_ref(&record)).unwrap();
        assert_eq!(journal.read().unwrap(), [record]);
        std::fs::write(&journal.0, vec![b' '; MAX_JOURNAL_BYTES as usize + 1]).unwrap();
        assert!(journal.read().is_err());
        journal.save(&[]).unwrap();
        assert!(!journal.0.exists());
        drop(owner);
        std::fs::remove_file(directory.join("handsfree.lock")).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
