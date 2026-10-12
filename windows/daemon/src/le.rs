//! Passive Apple advertisements. The caller verifies the paired device's IRK
//! before using battery data or considering an automatic connection.
//!
//! Scanning is PASSIVE (listen only, no scan requests) and only runs while
//! `should_scan` permits it. Connected scanning is opt-in because radio
//! coexistence varies; microphone capture and uplink always inhibit it.

use windows::Devices::Bluetooth::Advertisement::{
    BluetoothLEAdvertisementReceivedEventArgs, BluetoothLEAdvertisementWatcher,
    BluetoothLEAdvertisementWatcherStatus, BluetoothLEScanningMode,
};
use windows::Devices::Bluetooth::BluetoothAddressType;
use windows::Foundation::TypedEventHandler;
use windows::Storage::Streams::DataReader;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

const APPLE_COMPANY_ID: u16 = 0x004C;
/// Watch for Apple advertisements, calling `on_nearby` on each — but
/// only scan while `should_scan()` is true. Blocks (keeps COM + the watcher
/// alive), so run it on its own thread.
pub fn watch_nearby(
    on_nearby: impl Fn(u64, &[u8]) + Send + Sync + 'static,
    should_scan: impl Fn() -> bool + Send + 'static,
) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let watcher = match setup(on_nearby) {
        Ok(w) => w,
        Err(_) => return,
    };
    loop {
        match scan_action(should_scan(), watcher.Status().ok()) {
            Some(true) => {
                let _ = watcher.Start();
            }
            Some(false) => {
                let _ = watcher.Stop();
            }
            None => {}
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

fn scan_action(
    wanted: bool,
    status: Option<BluetoothLEAdvertisementWatcherStatus>,
) -> Option<bool> {
    use BluetoothLEAdvertisementWatcherStatus as Status;
    match (wanted, status) {
        (true, Some(Status::Started | Status::Stopping)) => None,
        (true, _) => Some(true),
        (false, Some(Status::Started)) => Some(false),
        _ => None,
    }
}

fn setup(
    on_nearby: impl Fn(u64, &[u8]) + Send + Sync + 'static,
) -> windows::core::Result<BluetoothLEAdvertisementWatcher> {
    let watcher = BluetoothLEAdvertisementWatcher::new()?;
    watcher.SetScanningMode(BluetoothLEScanningMode::Passive)?;
    let handler = TypedEventHandler::new(
        move |_s: &Option<BluetoothLEAdvertisementWatcher>,
              args: &Option<BluetoothLEAdvertisementReceivedEventArgs>| {
            if let Some(args) = args.as_ref() {
                read_apple_data(args, &on_nearby);
            }
            Ok(())
        },
    );
    watcher.Received(&handler)?;
    Ok(watcher)
}

fn read_apple_data(
    args: &BluetoothLEAdvertisementReceivedEventArgs,
    on_nearby: &impl Fn(u64, &[u8]),
) {
    if args.BluetoothAddressType().ok() != Some(BluetoothAddressType::Random) {
        return;
    }
    let Ok(address) = args.BluetoothAddress() else {
        return;
    };
    let Ok(adv) = args.Advertisement() else {
        return;
    };
    let Ok(mfg) = adv.ManufacturerData() else {
        return;
    };
    let size = mfg.Size().unwrap_or(0);
    for i in 0..size {
        let Ok(md) = mfg.GetAt(i) else { continue };
        if md.CompanyId().unwrap_or(0) != APPLE_COMPANY_ID {
            continue;
        }
        if let Ok(buf) = md.Data() {
            let len = buf.Length().unwrap_or(0) as usize;
            if !(2..=254).contains(&len) {
                continue;
            }
            if let Ok(reader) = DataReader::FromBuffer(&buf) {
                let mut bytes = vec![0u8; len];
                if reader.ReadBytes(&mut bytes).is_ok() {
                    on_nearby(address, &bytes);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aborted_scans_retry_after_radio_loss_without_restarting_an_active_scan() {
        use BluetoothLEAdvertisementWatcherStatus as Status;
        assert_eq!(scan_action(true, Some(Status::Aborted)), Some(true));
        assert_eq!(scan_action(true, Some(Status::Stopped)), Some(true));
        assert_eq!(scan_action(true, Some(Status::Created)), Some(true));
        assert_eq!(scan_action(true, Some(Status::Started)), None);
        assert_eq!(scan_action(true, Some(Status::Stopping)), None);
        assert_eq!(scan_action(false, Some(Status::Started)), Some(false));
        assert_eq!(scan_action(false, Some(Status::Aborted)), None);
    }
}
