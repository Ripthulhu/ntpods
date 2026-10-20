//! Restore the paired device's HFP service after a native-audio lease ends.

use std::fs::File;
use std::io::{self, Read};
use std::mem::{size_of, zeroed};
use std::path::{Path, PathBuf};
use windows_sys::core::GUID;
use windows_sys::Win32::Devices::Bluetooth::{
    BluetoothEnumerateInstalledServices, BluetoothFindFirstRadio, BluetoothFindNextRadio,
    BluetoothFindRadioClose, BluetoothGetDeviceInfo, BluetoothGetRadioInfo,
    BluetoothSetServiceState, BLUETOOTH_DEVICE_INFO, BLUETOOTH_FIND_RADIO_PARAMS,
    BLUETOOTH_RADIO_INFO,
};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};

const MAX_ADDRESS: u64 = 0xffff_ffff_ffff;
const MAX_JOURNAL_BYTES: u64 = 1024;
const JOURNAL: &str = "hfp-profile-restore.json";
const HANDSFREE: GUID = GUID {
    data1: 0x111e,
    data2: 0,
    data3: 0x1000,
    data4: [0x80, 0, 0, 0x80, 0x5f, 0x9b, 0x34, 0xfb],
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Restore {
    address: u64,
    radio_address: u64,
    was_enabled: bool,
}

fn valid_address(address: u64) -> bool {
    address != 0 && address <= MAX_ADDRESS
}

fn validate(record: Restore) -> io::Result<()> {
    if !valid_address(record.address)
        || !valid_address(record.radio_address)
        || record.address == record.radio_address
        || !record.was_enabled
    {
        return Err(io::Error::other(
            "Invalid hands-free service restoration record",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct State {
    radio_address: u64,
    enabled: bool,
}

trait Services {
    fn selected(&self, address: u64) -> io::Result<State>;
    // None means the owning radio is present but the pair has been forgotten.
    fn on_radio(&self, address: u64, radio_address: u64) -> io::Result<Option<State>>;
    fn set(&self, address: u64, radio_address: u64, enabled: bool) -> io::Result<()>;
}

trait Journal {
    fn save(&mut self, record: Option<Restore>) -> io::Result<()>;
}

struct FileJournal(PathBuf);

impl FileJournal {
    fn read(&self) -> io::Result<Option<Restore>> {
        let file = match File::open(&self.0) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        if file.metadata()?.len() > MAX_JOURNAL_BYTES {
            return Err(io::Error::other(
                "Hands-free service restoration record is too large",
            ));
        }
        let record = serde_json::from_reader(file.take(MAX_JOURNAL_BYTES + 1))?;
        validate(record)?;
        Ok(Some(record))
    }
}

impl Journal for FileJournal {
    fn save(&mut self, record: Option<Restore>) -> io::Result<()> {
        if let Some(record) = record {
            validate(record)?;
            crate::atomic_file::write(&self.0, &serde_json::to_vec(&record)?)
        } else {
            match std::fs::remove_file(&self.0) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                result => result,
            }
        }
    }
}

pub fn pending(directory: &Path) -> io::Result<bool> {
    FileJournal(directory.join(JOURNAL))
        .read()
        .map(|record| record.is_some())
}

pub struct Lease {
    path: PathBuf,
    restore: Option<Restore>,
    ready: Option<u64>,
}

impl Lease {
    /// The guard holds its profile lock until restoration and journal removal finish.
    pub fn open(directory: &Path) -> io::Result<Self> {
        let path = directory.join(JOURNAL);
        Ok(Self {
            restore: FileJournal(path.clone()).read()?,
            path,
            ready: None,
        })
    }

    pub fn update(&mut self, address: Option<u64>) -> io::Result<()> {
        self.update_with(
            address,
            &WindowsServices,
            &mut FileJournal(self.path.clone()),
        )
    }

    pub fn ready(&self, address: u64) -> bool {
        self.ready == Some(address)
    }

    fn update_with(
        &mut self,
        address: Option<u64>,
        services: &impl Services,
        journal: &mut impl Journal,
    ) -> io::Result<()> {
        self.ready = None;
        if address.is_some_and(|address| !valid_address(address)) {
            return Err(io::Error::other("Invalid hands-free service target"));
        }
        if self
            .restore
            .is_some_and(|record| Some(record.address) != address)
        {
            self.restore_with(services, journal)?;
        }
        let Some(address) = address else {
            return Ok(());
        };
        let state = services.selected(address)?;
        if !valid_address(state.radio_address) || state.radio_address == address {
            return Err(io::Error::other("Invalid owning Bluetooth radio"));
        }
        if let Some(record) = self.restore {
            if record.radio_address != state.radio_address {
                return Err(io::Error::other(
                    "The hands-free service's owning radio changed",
                ));
            }
        } else if state.enabled {
            let record = Restore {
                address,
                radio_address: state.radio_address,
                was_enabled: true,
            };
            // A failed or interrupted service call may still have changed Windows.
            journal.save(Some(record))?;
            self.restore = Some(record);
        }
        if state.enabled {
            services.set(address, state.radio_address, false)?;
        }
        let after = services.selected(address)?;
        if after.radio_address != state.radio_address || after.enabled {
            return Err(io::Error::other(
                "Windows did not confirm hands-free service isolation",
            ));
        }
        self.ready = Some(address);
        Ok(())
    }

    fn restore_with(
        &mut self,
        services: &impl Services,
        journal: &mut impl Journal,
    ) -> io::Result<()> {
        let Some(record) = self.restore else {
            return Ok(());
        };
        if let Some(state) = services.on_radio(record.address, record.radio_address)? {
            if state.radio_address != record.radio_address {
                return Err(io::Error::other("The hands-free restoration radio changed"));
            }
            if !state.enabled {
                services.set(record.address, record.radio_address, true)?;
                if services.on_radio(record.address, record.radio_address)?
                    != Some(State {
                        radio_address: record.radio_address,
                        enabled: true,
                    })
                {
                    return Err(io::Error::other(
                        "Windows did not confirm hands-free service restoration",
                    ));
                }
            }
        }
        journal.save(None)?;
        self.restore = None;
        Ok(())
    }
}

struct Radio {
    handle: HANDLE,
    address: u64,
    device: BLUETOOTH_DEVICE_INFO,
}

impl Drop for Radio {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.handle) };
    }
}

impl Radio {
    fn find(address: u64, expected: Option<u64>) -> io::Result<Option<Self>> {
        if !valid_address(address) || expected.is_some_and(|radio| !valid_address(radio)) {
            return Err(io::Error::other("Invalid paired Bluetooth identity"));
        }
        unsafe {
            let mut params: BLUETOOTH_FIND_RADIO_PARAMS = zeroed();
            params.dwSize = size_of::<BLUETOOTH_FIND_RADIO_PARAMS>() as u32;
            let mut handle = std::ptr::null_mut();
            let search = BluetoothFindFirstRadio(&params, &mut handle);
            if search.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mut owner = None;
            let mut matches = 0;
            let mut radio_present = false;
            let mut failure = None;
            loop {
                let mut info: BLUETOOTH_RADIO_INFO = zeroed();
                info.dwSize = size_of::<BLUETOOTH_RADIO_INFO>() as u32;
                let rc = BluetoothGetRadioInfo(handle, &mut info);
                if rc != 0 {
                    failure = Some(io::Error::from_raw_os_error(rc as i32));
                } else {
                    let local = info.address.Anonymous.ullLong;
                    if expected.is_none_or(|expected| expected == local) {
                        radio_present = true;
                        let mut device: BLUETOOTH_DEVICE_INFO = zeroed();
                        device.dwSize = size_of::<BLUETOOTH_DEVICE_INFO>() as u32;
                        device.Address.Anonymous.ullLong = address;
                        let rc = BluetoothGetDeviceInfo(handle, &mut device);
                        if rc == 0 && device.Address.Anonymous.ullLong != address {
                            failure =
                                Some(io::Error::other("The paired Bluetooth identity changed"));
                        } else if rc == 0 && device.fAuthenticated != 0 && device.fRemembered != 0 {
                            matches += 1;
                            if owner.is_none() {
                                owner = Some(Self {
                                    handle,
                                    address: local,
                                    device,
                                });
                                handle = std::ptr::null_mut();
                            }
                        } else if rc != 0 && rc != 1168 {
                            failure = Some(io::Error::from_raw_os_error(rc as i32));
                        }
                    }
                }
                if !handle.is_null() {
                    CloseHandle(handle);
                }
                handle = std::ptr::null_mut();
                if BluetoothFindNextRadio(search, &mut handle) == 0 {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() != Some(259) {
                        failure = Some(error);
                    }
                    break;
                }
            }
            BluetoothFindRadioClose(search);
            if let Some(error) = failure {
                return Err(error);
            }
            if matches > 1 {
                return Err(io::Error::other(
                    "The pair has multiple owning Bluetooth radios",
                ));
            }
            if !radio_present || (expected.is_none() && owner.is_none()) {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "The paired Bluetooth radio is unavailable",
                ));
            }
            Ok(owner)
        }
    }

    fn state(&self) -> io::Result<State> {
        unsafe {
            let mut count = 0;
            let rc = BluetoothEnumerateInstalledServices(
                self.handle,
                &self.device,
                &mut count,
                std::ptr::null_mut(),
            );
            if rc != 0 && rc != 234 {
                return Err(io::Error::from_raw_os_error(rc as i32));
            }
            if count > 128 {
                return Err(io::Error::other("Invalid Bluetooth service count"));
            }
            let mut services: Vec<GUID> = (0..count).map(|_| zeroed()).collect();
            if count != 0 {
                let rc = BluetoothEnumerateInstalledServices(
                    self.handle,
                    &self.device,
                    &mut count,
                    services.as_mut_ptr(),
                );
                if rc != 0 {
                    return Err(io::Error::from_raw_os_error(rc as i32));
                }
                if count as usize > services.len() {
                    return Err(io::Error::other("Bluetooth service list changed"));
                }
            }
            let enabled = services[..count as usize].iter().any(|service| {
                service.data1 == HANDSFREE.data1
                    && service.data2 == HANDSFREE.data2
                    && service.data3 == HANDSFREE.data3
                    && service.data4 == HANDSFREE.data4
            });
            Ok(State {
                radio_address: self.address,
                enabled,
            })
        }
    }
}

struct WindowsServices;

impl Services for WindowsServices {
    fn selected(&self, address: u64) -> io::Result<State> {
        Radio::find(address, None)?
            .ok_or_else(|| io::Error::other("The paired radio disappeared"))?
            .state()
    }

    fn on_radio(&self, address: u64, radio_address: u64) -> io::Result<Option<State>> {
        Radio::find(address, Some(radio_address))?
            .map(|radio| radio.state())
            .transpose()
    }

    fn set(&self, address: u64, radio_address: u64, enabled: bool) -> io::Result<()> {
        let radio = Radio::find(address, None)?
            .ok_or_else(|| io::Error::other("The paired radio disappeared"))?;
        if radio.address != radio_address {
            return Err(io::Error::other(
                "The paired radio changed before the service request",
            ));
        }
        if radio.state()?.enabled == enabled {
            return Ok(());
        }
        let rc = unsafe {
            BluetoothSetServiceState(radio.handle, &radio.device, &HANDSFREE, u32::from(enabled))
        };
        if rc != 0 {
            return Err(io::Error::from_raw_os_error(rc as i32));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    struct FakeServices {
        state: Cell<State>,
        forgotten: Cell<bool>,
        unavailable: Cell<bool>,
        writes: RefCell<Vec<bool>>,
        fail_set: Cell<bool>,
        ignore_set: Cell<bool>,
    }
    impl FakeServices {
        fn new(enabled: bool) -> Self {
            Self {
                state: Cell::new(State {
                    radio_address: 2,
                    enabled,
                }),
                forgotten: Cell::new(false),
                unavailable: Cell::new(false),
                writes: RefCell::new(vec![]),
                fail_set: Cell::new(false),
                ignore_set: Cell::new(false),
            }
        }
    }
    impl Services for FakeServices {
        fn selected(&self, _: u64) -> io::Result<State> {
            if self.unavailable.get() {
                return Err(io::Error::other("unavailable"));
            }
            Ok(self.state.get())
        }
        fn on_radio(&self, address: u64, _: u64) -> io::Result<Option<State>> {
            if self.forgotten.get() {
                Ok(None)
            } else {
                self.selected(address).map(Some)
            }
        }
        fn set(&self, _: u64, _: u64, enabled: bool) -> io::Result<()> {
            self.writes.borrow_mut().push(enabled);
            if !self.ignore_set.get() {
                self.state.set(State {
                    enabled,
                    ..self.state.get()
                });
            }
            if self.fail_set.get() {
                Err(io::Error::other("partial service change"))
            } else {
                Ok(())
            }
        }
    }
    #[derive(Default)]
    struct FakeJournal {
        saved: Option<Restore>,
        fail: bool,
    }
    impl Journal for FakeJournal {
        fn save(&mut self, record: Option<Restore>) -> io::Result<()> {
            if self.fail {
                return Err(io::Error::other("journal failure"));
            }
            self.saved = record;
            Ok(())
        }
    }
    fn lease() -> Lease {
        Lease {
            path: PathBuf::new(),
            restore: None,
            ready: None,
        }
    }

    #[test]
    fn disconnect_restores_only_an_originally_enabled_service() {
        for enabled in [false, true] {
            let services = FakeServices::new(enabled);
            let mut journal = FakeJournal::default();
            let mut lease = lease();
            lease.update_with(Some(1), &services, &mut journal).unwrap();
            assert!(lease.ready(1));
            assert_eq!(journal.saved.is_some(), enabled);
            lease.update_with(None, &services, &mut journal).unwrap();
            assert!(!lease.ready(1));
            assert_eq!(services.state.get().enabled, enabled);
            assert_eq!(
                *services.writes.borrow(),
                if enabled { vec![false, true] } else { vec![] }
            );
            assert!(journal.saved.is_none());
        }
    }

    #[test]
    fn durable_save_precedes_mutation_and_partial_disable_keeps_recovery() {
        let services = FakeServices::new(true);
        let mut journal = FakeJournal {
            fail: true,
            ..Default::default()
        };
        let mut lease = lease();
        assert!(lease.update_with(Some(1), &services, &mut journal).is_err());
        assert!(services.writes.borrow().is_empty());
        journal.fail = false;
        services.fail_set.set(true);
        assert!(lease.update_with(Some(1), &services, &mut journal).is_err());
        assert!(!lease.ready(1) && journal.saved.is_some());
        services.fail_set.set(false);
        lease.update_with(None, &services, &mut journal).unwrap();
        assert!(journal.saved.is_none() && services.state.get().enabled);
    }

    #[test]
    fn failed_readback_never_signals_ready_or_discards_restoration() {
        let services = FakeServices::new(true);
        let mut journal = FakeJournal::default();
        let mut lease = lease();
        services.ignore_set.set(true);
        assert!(lease.update_with(Some(1), &services, &mut journal).is_err());
        assert!(!lease.ready(1) && journal.saved.is_some());
        // It was not actually disabled, so cleanup must not submit an enable.
        lease.update_with(None, &services, &mut journal).unwrap();
        assert_eq!(*services.writes.borrow(), vec![false]);
    }

    #[test]
    fn crashed_guard_recovery_retries_failed_restore_and_journal_removal() {
        let services = FakeServices::new(true);
        let mut journal = FakeJournal::default();
        let mut original = lease();
        original
            .update_with(Some(1), &services, &mut journal)
            .unwrap();
        let mut recovery = lease();
        recovery.restore = journal.saved;
        services.ignore_set.set(true);
        assert!(recovery.update_with(None, &services, &mut journal).is_err());
        assert!(journal.saved.is_some());
        services.ignore_set.set(false);
        journal.fail = true;
        assert!(recovery.update_with(None, &services, &mut journal).is_err());
        assert!(journal.saved.is_some() && services.state.get().enabled);
        journal.fail = false;
        recovery.update_with(None, &services, &mut journal).unwrap();
        assert!(journal.saved.is_none());
        assert_eq!(*services.writes.borrow(), vec![false, true, true]);
    }

    #[test]
    fn changed_or_unavailable_adapter_cannot_restore_a_different_pairing() {
        let services = FakeServices::new(true);
        let mut journal = FakeJournal::default();
        let mut lease = lease();
        lease.update_with(Some(1), &services, &mut journal).unwrap();
        services.state.set(State {
            radio_address: 3,
            enabled: false,
        });
        assert!(lease.update_with(Some(1), &services, &mut journal).is_err());
        assert!(lease.update_with(None, &services, &mut journal).is_err());
        services.unavailable.set(true);
        assert!(lease.update_with(None, &services, &mut journal).is_err());
        assert_eq!(*services.writes.borrow(), vec![false]);
        assert!(journal.saved.is_some());
        services.unavailable.set(false);
        services.forgotten.set(true);
        lease.update_with(None, &services, &mut journal).unwrap();
        assert!(journal.saved.is_none());
    }

    #[test]
    fn journal_rejects_malformed_records_and_oversized_input() {
        for (index, text) in [
            r#"{"address":0,"radio_address":2,"was_enabled":true}"#,
            r#"{"address":1,"radio_address":1,"was_enabled":true}"#,
            r#"{"address":1,"radio_address":2,"was_enabled":false}"#,
            r#"{"address":1,"radio_address":2,"was_enabled":true,"extra":1}"#,
            r#"{"address":1,"radio_address":2,"was_enabled":true"#,
            &" ".repeat(1025),
        ]
        .iter()
        .enumerate()
        {
            let path = std::env::temp_dir()
                .join(format!("ntpods-hfp-journal-{}-{index}", std::process::id()));
            std::fs::write(&path, text).unwrap();
            assert!(FileJournal(path.clone()).read().is_err());
            std::fs::remove_file(path).unwrap();
        }
    }
}
