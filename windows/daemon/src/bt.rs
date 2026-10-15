//! Locate the paired AirPods and return their 48-bit Bluetooth address.

use std::mem::{size_of, zeroed};
use std::sync::{Mutex, MutexGuard, TryLockError};
use std::time::Instant;

use windows_sys::core::GUID;
use windows_sys::Win32::Devices::Bluetooth::{
    BluetoothFindDeviceClose, BluetoothFindFirstDevice, BluetoothFindFirstRadio,
    BluetoothFindNextDevice, BluetoothFindNextRadio, BluetoothFindRadioClose,
    BluetoothGetDeviceInfo, BluetoothGetRadioInfo, BluetoothSetServiceState, BLUETOOTH_DEVICE_INFO,
    BLUETOOTH_DEVICE_SEARCH_PARAMS, BLUETOOTH_FIND_RADIO_PARAMS, BLUETOOTH_RADIO_INFO,
};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};

// Classic-audio service GUIDs — A2DP AudioSink + Handsfree.
const AUDIO_SINK: GUID = GUID {
    data1: 0x0000_110b,
    data2: 0,
    data3: 0x1000,
    data4: [0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb],
};
const HANDSFREE: GUID = GUID {
    data1: 0x0000_111e,
    data2: 0,
    data3: 0x1000,
    data4: [0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb],
};

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PlaybackRestore {
    address: u64,
    sink: bool,
    handsfree: bool,
}

fn playback_journal() -> std::io::Result<std::path::PathBuf> {
    Ok(std::path::PathBuf::from(
        std::env::var_os("LOCALAPPDATA")
            .ok_or_else(|| std::io::Error::other("Local app data is unavailable"))?,
    )
    .join("NTPods")
    .join("playback-restore.json"))
}

fn validate_playback_record(record: &PlaybackRestore) -> std::io::Result<()> {
    if record.address == 0 || record.address > 0xffff_ffff_ffff || !record.sink {
        return Err(std::io::Error::other("Invalid playback restoration record"));
    }
    Ok(())
}

pub fn playback_restore_pending() -> std::io::Result<bool> {
    playback_journal()?.try_exists()
}

fn restore_playback(record: &PlaybackRestore, intent: u64) -> std::io::Result<()> {
    validate_playback_record(record)?;
    if !AUDIO_INTENT.allows_recovery(intent) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "Playback restoration was superseded",
        ));
    }
    let (sink, handsfree) = set_audio_services(record.address, true, !record.handsfree, intent);
    if sink != 0 || (record.handsfree && handsfree != 0) {
        return Err(std::io::Error::other(format!(
            "Playback restoration failed ({sink}/{handsfree})"
        )));
    }
    Ok(())
}

fn finish_playback_restore(
    intent: &AudioIntent,
    token: u64,
    restore: impl FnOnce(u64) -> std::io::Result<()>,
    remove_journal: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<()> {
    let result = if intent.allows_recovery(token) {
        restore(token)
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "Playback restoration was superseded",
        ))
    };
    // Keep journal removal atomic with respect to a newer Connect or Disconnect.
    let state = intent.0.lock().unwrap_or_else(|error| error.into_inner());
    if state.token & 1 == 0 {
        return remove_journal();
    }
    if state.token != token {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "Playback restoration was superseded",
        ));
    }
    result?;
    remove_journal()
}

/// A crashed experimental build may have left the normal audio service disabled.
pub fn restore_experimental_audio() -> std::io::Result<()> {
    use std::io::Read;
    let path = playback_journal()?;
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if file.metadata()?.len() > 1024 {
        return Err(std::io::Error::other("Invalid playback restoration record"));
    }
    let record: PlaybackRestore = serde_json::from_reader(file.take(1025))?;
    let _gate = AUDIO_GATE
        .enter(&AUDIO_INTENT, true)
        .ok_or_else(|| std::io::Error::other("Audio is shutting down"))?;
    finish_playback_restore(
        &AUDIO_INTENT,
        AUDIO_INTENT.snapshot(),
        |token| restore_playback(&record, token),
        || std::fs::remove_file(path),
    )
}

#[cfg(feature = "experimental-playback")]
pub fn playback_intent() -> u64 {
    AUDIO_INTENT.snapshot()
}

unsafe fn set_service(radio: HANDLE, mac: u64, service: &GUID, flags: u32) -> u32 {
    #[cfg(feature = "experimental-playback")]
    if service.data1 == AUDIO_SINK.data1
        && service.data2 == AUDIO_SINK.data2
        && service.data3 == AUDIO_SINK.data3
        && service.data4 == AUDIO_SINK.data4
        && crate::playback::owns_sink(mac)
    {
        // Removing this service would remove the active physical audio endpoint.
        return 0;
    }
    let mut device: BLUETOOTH_DEVICE_INFO = zeroed();
    device.dwSize = size_of::<BLUETOOTH_DEVICE_INFO>() as u32;
    device.Address.Anonymous.ullLong = mac;
    // Query the same owning radio immediately before each service operation;
    // a global enumeration record can be stale or belong to another adapter.
    let result = BluetoothGetDeviceInfo(radio, &mut device);
    if result != 0 {
        return result;
    }
    if device.Address.Anonymous.ullLong != mac
        || device.fAuthenticated == 0
        || device.fRemembered == 0
    {
        return 1168;
    }
    let started = Instant::now();
    let result = BluetoothSetServiceState(radio, &device, service, flags);
    crate::log(&format!(
        "audio service: uuid={:#06x} enabled={} result={} elapsed_ms={}",
        service.data1,
        flags,
        result,
        started.elapsed().as_millis()
    ));
    result
}

// Call only while AUDIO_GATE is held; a later intent cancels remaining calls.
fn set_audio_services(mac: u64, connect: bool, sink_only: bool, intent: u64) -> (u32, u32) {
    if mac == 0 || mac > 0xffff_ffff_ffff {
        return (87, 87);
    }
    unsafe {
        let flags = u32::from(connect);
        let mut params: BLUETOOTH_FIND_RADIO_PARAMS = zeroed();
        params.dwSize = size_of::<BLUETOOTH_FIND_RADIO_PARAMS>() as u32;
        let mut radio: HANDLE = std::ptr::null_mut();
        let found = BluetoothFindFirstRadio(&params, &mut radio);
        if found.is_null() {
            return (1168, 1168);
        }
        let (mut sink, mut handsfree) = (u32::MAX, u32::MAX);
        loop {
            if !AUDIO_INTENT.is_current(intent) {
                CloseHandle(radio);
                break;
            }
            sink = sink.min(set_service(radio, mac, &AUDIO_SINK, flags));
            if !sink_only && AUDIO_INTENT.is_current(intent) {
                handsfree = handsfree.min(set_service(radio, mac, &HANDSFREE, flags));
            }
            CloseHandle(radio);
            radio = std::ptr::null_mut();
            if BluetoothFindNextRadio(found, &mut radio) == 0 {
                break;
            }
        }
        BluetoothFindRadioClose(found);
        if !AUDIO_INTENT.is_current(intent) {
            (1223, 1223)
        } else {
            (sink, handsfree)
        }
    }
}
/// Windows device name and Class of Device; peers known only to the AirPods may be absent.
pub fn paired_device(addr: u64) -> Option<(String, u32)> {
    unsafe { device_info(addr).map(|d| (utf16_name(&d.szName), d.ulClassofDevice)) }
}

/// Map a Bluetooth Class of Device to the icon kind the UI understands.
/// Major device class is bits 8..12; the minor class refines Computer and Wearable.
pub fn kind_from_cod(cod: u32) -> &'static str {
    let major = (cod >> 8) & 0x1F;
    let minor = (cod >> 2) & 0x3F;
    match major {
        0x01 => match minor {
            0x03 | 0x04 => "laptop", // laptop / handheld PC
            _ => "pc",
        },
        0x02 => "phone",
        0x04 => "audio",
        0x07 => match minor {
            0x02 => "watch",
            _ => "unknown",
        },
        _ => "unknown",
    }
}

/// All local radio addresses, used to identify this PC in AAP 0x2E host reports.
/// Include every adapter because the AirPods may be paired through any of them.
pub fn local_radio_addresses() -> Vec<u64> {
    unsafe {
        let mut out = Vec::new();
        let mut rparams: BLUETOOTH_FIND_RADIO_PARAMS = zeroed();
        rparams.dwSize = size_of::<BLUETOOTH_FIND_RADIO_PARAMS>() as u32;
        let mut hradio: HANDLE = std::ptr::null_mut();
        let hfind = BluetoothFindFirstRadio(&rparams, &mut hradio);
        if hfind.is_null() {
            return out;
        }
        loop {
            let mut info: BLUETOOTH_RADIO_INFO = zeroed();
            info.dwSize = size_of::<BLUETOOTH_RADIO_INFO>() as u32;
            if BluetoothGetRadioInfo(hradio, &mut info) == 0 {
                out.push(info.address.Anonymous.ullLong);
            }
            CloseHandle(hradio);
            hradio = std::ptr::null_mut();
            if BluetoothFindNextRadio(hfind, &mut hradio) == 0 {
                break;
            }
        }
        BluetoothFindRadioClose(hfind);
        out
    }
}

// Low bit is the requested audio state; the remaining bits are a generation.
// Start permissive so a link already connected by Windows can recover.
struct AudioRequestState {
    token: u64,
    accepting: bool,
}
struct AudioIntent(Mutex<AudioRequestState>);
impl AudioIntent {
    const fn new() -> Self {
        Self(Mutex::new(AudioRequestState {
            token: 1,
            accepting: true,
        }))
    }
    fn reserve(&self, connect: bool) -> Option<u64> {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if !state.accepting {
            return None;
        }
        state.token = (state.token.wrapping_add(2) & !1) | u64::from(connect);
        Some(state.token)
    }
    fn snapshot(&self) -> u64 {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).token
    }
    fn accepts_requests(&self) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).accepting
    }
    fn stop_requests(&self) {
        // Keep the current intent valid until an active reset has re-enabled audio.
        self.0.lock().unwrap_or_else(|e| e.into_inner()).accepting = false;
    }
    fn is_current(&self, token: u64) -> bool {
        self.snapshot() == token
    }
    fn allows_recovery(&self, token: u64) -> bool {
        token & 1 != 0 && self.is_current(token)
    }
}

struct AudioGate(Mutex<bool>);
impl AudioGate {
    const fn new() -> Self {
        Self(Mutex::new(false))
    }
    fn enter(&self, intent: &AudioIntent, cleanup: bool) -> Option<MutexGuard<'_, bool>> {
        let closed = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if *closed || (!cleanup && !intent.accepts_requests()) {
            None
        } else {
            Some(closed)
        }
    }
    fn finish_shutdown(&self) -> bool {
        let mut closed = match self.0.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::Poisoned(error)) => error.into_inner(),
            Err(TryLockError::WouldBlock) => return false,
        };
        *closed = true;
        true
    }
}

static AUDIO_GATE: AudioGate = AudioGate::new();
static AUDIO_INTENT: AudioIntent = AudioIntent::new();

/// Reserve in the command thread, before spawning work, so scheduling cannot
/// turn an old Connect into a newer request than the user's Disconnect.
pub fn begin_audio_request(connect: bool) -> Option<u64> {
    AUDIO_INTENT.reserve(connect)
}

pub fn begin_audio_shutdown() {
    AUDIO_INTENT.stop_requests();
}

/// Call after receiver cleanup. Queued and late workers cannot enter once closed.
pub fn finish_audio_shutdown() -> bool {
    AUDIO_GATE.finish_shutdown()
}

pub fn audio_request_is_current(intent: u64) -> bool {
    AUDIO_INTENT.is_current(intent)
}

pub fn request_audio(mac: u64, intent: u64) -> Option<(u32, u32)> {
    request_audio_inner(mac, intent, false)
}

pub fn request_audio_for_cleanup(mac: u64, intent: u64) -> Option<(u32, u32)> {
    request_audio_inner(mac, intent, true)
}

fn request_audio_inner(mac: u64, intent: u64, cleanup: bool) -> Option<(u32, u32)> {
    let _gate = AUDIO_GATE.enter(&AUDIO_INTENT, cleanup)?;
    if !AUDIO_INTENT.is_current(intent) {
        return None;
    }
    Some(set_audio_services(mac, intent & 1 != 0, false, intent))
}

/// One serialized disable/enable transaction. Recovery observes user intent;
/// it never creates a new intent that could override a newer Disconnect.
/// Win32 service calls have no timeout API, so this bounds attempts, not wall time.
pub fn reset_audio_services(mac: u64, sink_only: bool) -> (u32, u32) {
    let intent = AUDIO_INTENT.snapshot();
    reset_audio_services_inner(mac, sink_only, intent, false)
}

pub fn reset_audio_services_for_cleanup(mac: u64, sink_only: bool) -> (u32, u32) {
    reset_audio_services_inner(mac, sink_only, AUDIO_INTENT.snapshot(), true)
}

/// A deferred Connect must recover only its own generation, not whichever
/// request happens to be newest when its worker starts.
pub fn reset_audio_services_for_request(mac: u64, sink_only: bool, intent: u64) -> (u32, u32) {
    #[cfg(feature = "experimental-playback")]
    {
        if !AUDIO_INTENT.allows_recovery(intent) {
            return (1223, 1223);
        }
        if let Some(result) = crate::playback::reset_if_active(mac, intent, true) {
            return result;
        }
    }
    reset_audio_services_inner(mac, sink_only, intent, true)
}

/// A manual rebuild keeps the device and intent reserved by its command thread.
pub fn reset_audio_services_for_user_request(mac: u64, sink_only: bool, intent: u64) -> (u32, u32) {
    reset_audio_services_inner(mac, sink_only, intent, false)
}

fn reset_audio_services_inner(mac: u64, sink_only: bool, intent: u64, cleanup: bool) -> (u32, u32) {
    let waiting = Instant::now();
    #[cfg(feature = "experimental-playback")]
    let _gate = loop {
        if !AUDIO_INTENT.allows_recovery(intent) || (!cleanup && !AUDIO_INTENT.accepts_requests()) {
            return (1223, 1223);
        }
        if let Some(result) = crate::playback::reset_if_active(mac, intent, false) {
            return result;
        }
        match AUDIO_GATE.0.try_lock() {
            Ok(guard) if !*guard => break guard,
            Ok(_) => return (1223, 1223),
            Err(TryLockError::Poisoned(_)) => return (1223, 1223),
            Err(TryLockError::WouldBlock) => {
                std::thread::sleep(std::time::Duration::from_millis(10))
            }
        }
    };
    #[cfg(not(feature = "experimental-playback"))]
    let Some(_gate) = AUDIO_GATE.enter(&AUDIO_INTENT, cleanup) else {
        return (1223, 1223);
    };
    crate::log(&format!(
        "audio recovery: gate_wait_ms={}",
        waiting.elapsed().as_millis()
    ));
    if !AUDIO_INTENT.allows_recovery(intent) {
        return (1223, 1223);
    }
    // HFP succeeding cannot confirm stereo restoration: AudioSink is required.
    let (disabled, _) = set_audio_services(mac, false, sink_only, intent);
    std::thread::sleep(std::time::Duration::from_millis(1000));
    if !AUDIO_INTENT.allows_recovery(intent) {
        return (disabled, 1223);
    }
    let (enabled, _) = set_audio_services(mac, true, sink_only, intent);
    if enabled == 0 {
        std::thread::sleep(std::time::Duration::from_millis(1500));
    }
    (disabled, enabled)
}

#[cfg(test)]
mod audio_intent_tests {
    use super::{AudioGate, AudioIntent};
    use std::sync::{mpsc, Arc};

    #[test]
    fn playback_restore_requires_a_bounded_address_and_saved_audio_service() {
        let valid = super::PlaybackRestore {
            address: 1,
            sink: true,
            handsfree: false,
        };
        assert!(super::validate_playback_record(&valid).is_ok());
        for address in [0, 0x1_0000_0000_0000] {
            assert!(
                super::validate_playback_record(&super::PlaybackRestore { address, ..valid })
                    .is_err()
            );
        }
        assert!(super::validate_playback_record(&super::PlaybackRestore {
            sink: false,
            ..valid
        })
        .is_err());
        assert!(serde_json::from_str::<super::PlaybackRestore>(
            r#"{"address":1,"sink":true,"handsfree":false,"other":true}"#
        )
        .is_err());
    }

    #[test]
    fn windows_connected_audio_can_recover_before_any_ui_request() {
        let intent = AudioIntent::new();
        assert!(intent.allows_recovery(intent.snapshot()));
    }

    #[test]
    fn disconnect_between_restore_capture_and_service_call_never_enables_audio() {
        use std::cell::Cell;
        let intent = AudioIntent::new();
        let captured = intent.snapshot();
        intent.reserve(false).unwrap();
        let removed = Cell::new(false);
        super::finish_playback_restore(
            &intent,
            captured,
            |_| panic!("Disconnect must not authorize an enable"),
            || {
                removed.set(true);
                Ok(())
            },
        )
        .unwrap();
        assert!(removed.get());
    }

    #[test]
    fn disconnect_during_service_restore_removes_the_stale_journal() {
        use std::cell::Cell;
        let intent = AudioIntent::new();
        let captured = intent.snapshot();
        let removed = Cell::new(false);
        super::finish_playback_restore(
            &intent,
            captured,
            |token| {
                assert_eq!(token, captured);
                intent.reserve(false).unwrap();
                assert!(!intent.allows_recovery(token));
                Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "cancelled",
                ))
            },
            || {
                removed.set(true);
                Ok(())
            },
        )
        .unwrap();
        assert!(removed.get());
    }

    #[test]
    fn newer_connect_keeps_the_restore_journal_even_if_old_service_calls_succeed() {
        let intent = AudioIntent::new();
        let captured = intent.snapshot();
        intent.reserve(true).unwrap();
        assert!(super::finish_playback_restore(
            &intent,
            captured,
            |_| panic!("stale restore must not run"),
            || panic!("newer Connect still needs the journal")
        )
        .is_err());
        for succeeded in [false, true] {
            let captured = intent.snapshot();
            let error = super::finish_playback_restore(
                &intent,
                captured,
                |token| {
                    assert_eq!(token, captured);
                    intent.reserve(true).unwrap();
                    if succeeded {
                        Ok(())
                    } else {
                        Err(std::io::Error::other("service failed"))
                    }
                },
                || panic!("newer Connect still needs the journal"),
            )
            .unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        }
    }

    #[test]
    fn restore_failure_keeps_the_journal_and_success_reports_removal_errors() {
        let intent = AudioIntent::new();
        let token = intent.snapshot();
        let error = super::finish_playback_restore(
            &intent,
            token,
            |_| Err(std::io::Error::other("service failed")),
            || panic!("failed restoration must retain the journal"),
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Other);
        let error = super::finish_playback_restore(
            &intent,
            token,
            |passed| {
                assert_eq!(passed, token);
                Ok(())
            },
            || {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "cannot remove",
                ))
            },
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        super::finish_playback_restore(&intent, token, |_| Ok(()), || Ok(())).unwrap();
    }

    #[test]
    fn changing_pairs_cancels_a_queued_rebuild_without_a_disconnect() {
        let intent = AudioIntent::new();
        let rebuild = intent.reserve(true).unwrap();
        let other_pair = intent.reserve(true).unwrap();
        assert!(!intent.allows_recovery(rebuild));
        assert!(intent.allows_recovery(other_pair));
    }

    #[test]
    fn disconnect_cancels_both_queued_connect_and_recovery_enable() {
        let intent = AudioIntent::new();
        let connect = intent.reserve(true).unwrap();
        let recovery = intent.snapshot();
        let disconnect = intent.reserve(false).unwrap();
        assert!(!intent.is_current(connect));
        assert!(!intent.allows_recovery(recovery));
        assert!(intent.is_current(disconnect));
        // Even recovery scheduled late, after Disconnect, must not reenable.
        assert!(!intent.allows_recovery(intent.snapshot()));
        let reconnect = intent.reserve(true).unwrap();
        assert!(intent.allows_recovery(reconnect));
        assert!(!intent.is_current(disconnect));
        assert!(!intent.allows_recovery(recovery));
    }

    #[test]
    fn shutdown_waits_for_active_restore_and_rejects_queued_or_late_audio_work() {
        let gate = Arc::new(AudioGate::new());
        let intent = Arc::new(AudioIntent::new());
        let token = intent.reserve(true).unwrap();
        let (disabled_tx, disabled_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let active = {
            let gate = gate.clone();
            let intent = intent.clone();
            std::thread::spawn(move || {
                let _guard = gate.enter(&intent, false).unwrap();
                disabled_tx.send(()).unwrap();
                resume_rx.recv().unwrap();
                // Shutdown must not invalidate the enable half of an active reset.
                assert!(intent.allows_recovery(token));
            })
        };
        disabled_rx.recv().unwrap();
        let (queued_tx, queued_rx) = mpsc::channel();
        let queued = {
            let gate = gate.clone();
            let intent = intent.clone();
            std::thread::spawn(move || {
                queued_tx.send(()).unwrap();
                assert!(gate.enter(&intent, false).is_none());
            })
        };
        queued_rx.recv().unwrap();
        intent.stop_requests();
        assert!(!gate.finish_shutdown());
        assert_eq!(intent.reserve(false), None);
        assert_eq!(intent.reserve(true), None);
        assert_eq!(intent.snapshot(), token);
        resume_tx.send(()).unwrap();
        active.join().unwrap();
        queued.join().unwrap();
        assert!(gate.finish_shutdown());
        assert!(gate.enter(&intent, false).is_none());
        assert!(gate.enter(&intent, true).is_none());
    }

    #[test]
    fn shutdown_permits_receiver_cleanup_until_the_audio_gate_is_closed() {
        let gate = AudioGate::new();
        let intent = AudioIntent::new();
        intent.stop_requests();
        assert!(gate.enter(&intent, false).is_none());
        let cleanup = gate.enter(&intent, true).unwrap();
        assert!(!gate.finish_shutdown());
        drop(cleanup);
        assert!(gate.finish_shutdown());
        assert!(gate.enter(&intent, true).is_none());
    }
}
/// Check the current Classic Bluetooth link, not just pairing history.
/// Search flags are OR'd: including `fReturnRemembered` would also return disconnected pairs.
pub fn is_connected(mac: u64) -> bool {
    unsafe {
        let mut params: BLUETOOTH_DEVICE_SEARCH_PARAMS = zeroed();
        params.dwSize = size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32;
        params.fReturnConnected = 1;
        let mut info: BLUETOOTH_DEVICE_INFO = zeroed();
        info.dwSize = size_of::<BLUETOOTH_DEVICE_INFO>() as u32;
        let h = BluetoothFindFirstDevice(&params, &mut info);
        if h.is_null() {
            return false;
        }
        let mut connected = false;
        loop {
            if info.Address.Anonymous.ullLong == mac && info.fConnected != 0 {
                connected = true;
                break;
            }
            info.dwSize = size_of::<BLUETOOTH_DEVICE_INFO>() as u32;
            if BluetoothFindNextDevice(h, &mut info) == 0 {
                break;
            }
        }
        BluetoothFindDeviceClose(h);
        connected
    }
}

/// Look up a paired device's `BLUETOOTH_DEVICE_INFO` by its 48-bit address.
unsafe fn device_info(mac: u64) -> Option<BLUETOOTH_DEVICE_INFO> {
    let mut params: BLUETOOTH_DEVICE_SEARCH_PARAMS = zeroed();
    params.dwSize = size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32;
    params.fReturnAuthenticated = 1;
    params.fReturnRemembered = 1;
    params.fReturnConnected = 1;
    let mut info: BLUETOOTH_DEVICE_INFO = zeroed();
    info.dwSize = size_of::<BLUETOOTH_DEVICE_INFO>() as u32;
    let h = BluetoothFindFirstDevice(&params, &mut info);
    if h.is_null() {
        return None;
    }
    let mut found = None;
    loop {
        if info.Address.Anonymous.ullLong == mac {
            found = Some(info);
            break;
        }
        info.dwSize = size_of::<BLUETOOTH_DEVICE_INFO>() as u32;
        if BluetoothFindNextDevice(h, &mut info) == 0 {
            break;
        }
    }
    BluetoothFindDeviceClose(h);
    found
}

fn utf16_name(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Case-insensitive discovery hints; a matching name alone does not establish AAP support.
const AAP_NAME_HINTS: &[&str] = &["airpod", "beats"];

/// Addresses from Windows AAP service devnodes, including renamed devices.
/// The 12 hex digits before `_C00000000` in a BTHENUM instance identify the peer.
fn aap_service_addresses() -> Vec<u64> {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegOpenKeyExW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ,
    };
    const BTHENUM: &str = r"SYSTEM\CurrentControlSet\Enum\BTHENUM";
    const AAP_PREFIX: &str = "{74ec2172-0bad-4d01-8f77-997b2be0722a}";

    unsafe fn subkeys(key: HKEY) -> Vec<String> {
        let mut out = Vec::new();
        let mut buf = [0u16; 256];
        let mut i = 0;
        loop {
            let mut len = buf.len() as u32;
            let r = RegEnumKeyExW(
                key,
                i,
                buf.as_mut_ptr(),
                &mut len,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            if r != 0 {
                break;
            }
            out.push(String::from_utf16_lossy(&buf[..len as usize]));
            i += 1;
        }
        out
    }
    unsafe fn open(parent: HKEY, path: &str) -> Option<HKEY> {
        let w: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut k: HKEY = std::ptr::null_mut();
        (RegOpenKeyExW(parent, w.as_ptr(), 0, KEY_READ, &mut k) == 0).then_some(k)
    }

    let mut addrs = Vec::new();
    unsafe {
        let Some(root) = open(HKEY_LOCAL_MACHINE, BTHENUM) else {
            return addrs;
        };
        for dev in subkeys(root) {
            if !dev.to_ascii_lowercase().starts_with(AAP_PREFIX) {
                continue;
            }
            if let Some(k) = open(root, &dev) {
                for inst in subkeys(k) {
                    let upper = inst.to_ascii_uppercase();
                    if let Some(pos) = upper.rfind("_C00000000") {
                        if pos >= 12 {
                            if let Ok(a) = u64::from_str_radix(&upper[pos - 12..pos], 16) {
                                if a != 0 && !addrs.contains(&a) {
                                    addrs.push(a);
                                }
                            }
                        }
                    }
                }
                RegCloseKey(k);
            }
        }
        RegCloseKey(root);
    }
    addrs
}

struct AirPodsCandidate {
    address: u64,
    name: String,
    authenticated: bool,
    remembered: bool,
    connected: bool,
    has_service: bool,
}

pub fn parse_pairing_address(value: &str) -> Option<u64> {
    let mut hex = String::with_capacity(12);
    match value.len() {
        12 if value.bytes().all(|b| b.is_ascii_hexdigit()) => hex.push_str(value),
        17 => {
            for (i, b) in value.bytes().enumerate() {
                if i % 3 == 2 {
                    if b != b':' {
                        return None;
                    }
                } else if b.is_ascii_hexdigit() {
                    hex.push(b as char);
                } else {
                    return None;
                }
            }
        }
        _ => return None,
    }
    let address = u64::from_str_radix(&hex, 16).ok()?;
    (address != 0 && address != 0xffff_ffff_ffff).then_some(address)
}

fn paired_aap_candidate(device: AirPodsCandidate) -> Option<String> {
    if !device.has_service {
        return None;
    }
    choose_airpods([device], 0).map(|(_, name)| name)
}

/// A successful pairing callback does not establish AAP support by itself.
pub fn paired_aap_name(address: u64) -> Option<String> {
    let info = unsafe { device_info(address)? };
    paired_aap_candidate(AirPodsCandidate {
        address,
        name: utf16_name(&info.szName),
        authenticated: info.fAuthenticated != 0,
        remembered: info.fRemembered != 0,
        connected: info.fConnected != 0,
        has_service: aap_service_addresses().contains(&address),
    })
}

fn choose_airpods(
    candidates: impl IntoIterator<Item = AirPodsCandidate>,
    current: u64,
) -> Option<(u64, String)> {
    let device = candidates
        .into_iter()
        .filter(|d| {
            d.authenticated
                && d.remembered
                && d.address != 0
                && d.address <= 0xffff_ffff_ffff
                && (d.has_service
                    || AAP_NAME_HINTS
                        .iter()
                        .any(|hint| d.name.to_lowercase().contains(hint)))
        })
        .max_by_key(|d| {
            (
                d.connected,
                d.address == current,
                d.has_service,
                std::cmp::Reverse(d.address),
            )
        })?;
    let name = device
        .name
        .trim_end_matches("- Find My")
        .trim_end_matches(" -")
        .trim();
    Some((
        device.address,
        if name.is_empty() { "AirPods" } else { name }.into(),
    ))
}

fn last_airpods(settings: impl std::io::Read) -> Option<u64> {
    use std::io::Read;
    #[derive(serde::Deserialize)]
    struct Saved {
        #[serde(rename = "LastDeviceId")]
        device: String,
    }
    let mut data = Vec::new();
    settings.take(16_385).read_to_end(&mut data).ok()?;
    if data.len() > 16_384 {
        return None;
    }
    let saved: Saved = serde_json::from_slice(&data).ok()?;
    if saved.device.len() != 12 || !saved.device.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let address = u64::from_str_radix(&saved.device, 16).ok()?;
    (address != 0).then_some(address)
}

pub fn find_airpods() -> Option<(u64, String)> {
    // The UI already saves the last pair for artwork; reuse it when both pairs sleep.
    let preferred = std::env::var_os("LOCALAPPDATA")
        .map(|root| std::path::PathBuf::from(root).join("NTPods/winui-settings.json"))
        .and_then(|path| std::fs::File::open(path).ok())
        .and_then(last_airpods)
        .unwrap_or(0);
    find_airpods_for(preferred)
}

/// Keep the current connected pair; otherwise prefer another connected pair.
/// AAP service identity also finds renamed AirPods, without trusting unpaired names.
pub fn find_airpods_for(current: u64) -> Option<(u64, String)> {
    let aap = aap_service_addresses();
    let mut candidates = Vec::new();
    unsafe {
        let mut params: BLUETOOTH_DEVICE_SEARCH_PARAMS = zeroed();
        params.dwSize = size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32;
        params.fReturnAuthenticated = 1;
        params.fReturnRemembered = 1;
        params.fReturnConnected = 1;

        let mut info: BLUETOOTH_DEVICE_INFO = zeroed();
        info.dwSize = size_of::<BLUETOOTH_DEVICE_INFO>() as u32;

        let h = BluetoothFindFirstDevice(&params, &mut info);
        if h.is_null() {
            return None;
        }
        loop {
            let addr = info.Address.Anonymous.ullLong;
            candidates.push(AirPodsCandidate {
                address: addr,
                name: utf16_name(&info.szName),
                authenticated: info.fAuthenticated != 0,
                remembered: info.fRemembered != 0,
                connected: info.fConnected != 0,
                has_service: aap.contains(&addr),
            });
            info.dwSize = size_of::<BLUETOOTH_DEVICE_INFO>() as u32;
            if BluetoothFindNextDevice(h, &mut info) == 0 {
                break;
            }
        }
        BluetoothFindDeviceClose(h);
    }
    choose_airpods(candidates, current)
}

#[cfg(test)]
mod discovery_tests {
    use super::{
        choose_airpods, last_airpods, paired_aap_candidate, parse_pairing_address, AirPodsCandidate,
    };

    fn device(address: u64, connected: bool) -> AirPodsCandidate {
        AirPodsCandidate {
            address,
            name: "AirPods".into(),
            authenticated: true,
            remembered: true,
            connected,
            has_service: true,
        }
    }

    #[test]
    fn pairing_handoff_requires_exact_classic_address_and_paired_aap_identity() {
        assert_eq!(
            parse_pairing_address("12:34:56:78:9a:bc"),
            Some(0x123456789abc)
        );
        assert_eq!(parse_pairing_address("123456789ABC"), Some(0x123456789abc));
        for bad in [
            "",
            "000000000000",
            "FFFFFFFFFFFF",
            "123456789abcd",
            "12-34-56-78-9a-bc",
            "12:34:56:78:9g:bc",
            "12:34:56:78:9a:bc ",
        ] {
            assert_eq!(parse_pairing_address(bad), None, "{bad}");
        }
        let mut renamed = device(1, false);
        renamed.name = "My earbuds".into();
        assert_eq!(paired_aap_candidate(renamed).as_deref(), Some("My earbuds"));
        for field in 0..3 {
            let mut untrusted = device(1, false);
            match field {
                0 => untrusted.authenticated = false,
                1 => untrusted.remembered = false,
                _ => untrusted.has_service = false,
            }
            assert!(paired_aap_candidate(untrusted).is_none());
        }
    }

    #[test]
    fn startup_uses_saved_pair_only_when_no_other_pair_is_connected() {
        let saved = last_airpods(
            br#"{"ThemeIndex":2,"LastModel":"A3064","LastDeviceId":"000000000002"}"#.as_slice(),
        )
        .unwrap();
        assert_eq!(
            choose_airpods([device(1, false), device(2, false)], saved)
                .unwrap()
                .0,
            2
        );
        assert_eq!(
            choose_airpods([device(1, true), device(2, false)], saved)
                .unwrap()
                .0,
            1
        );
        assert_eq!(choose_airpods([device(1, false)], saved).unwrap().0, 1);
        let mut unpaired = device(saved, false);
        unpaired.remembered = false;
        assert_eq!(
            choose_airpods([device(1, false), unpaired], saved)
                .unwrap()
                .0,
            1
        );
    }

    #[test]
    fn startup_preference_rejects_invalid_or_oversized_settings() {
        for settings in [
            "",
            "{}",
            r#"{"LastDeviceId":null}"#,
            r#"{"LastDeviceId":2}"#,
            r#"{"LastDeviceId":"000000000002""#,
        ] {
            assert!(last_airpods(settings.as_bytes()).is_none());
        }
        for id in [
            "",
            "000000000000",
            "2",
            "1000000000000",
            "00000000000g",
            "+00000000002",
            " 00000000002",
            "00:00:00:00:00:02",
        ] {
            let settings = serde_json::json!({ "LastDeviceId": id }).to_string();
            assert!(last_airpods(settings.as_bytes()).is_none());
        }
        assert_eq!(
            last_airpods(br#"{"LastDeviceId":"aAbBcCdDeEfF"}"#.as_slice()),
            Some(0xaabbccddeeff)
        );
        let mut oversized = br#"{"LastDeviceId":"000000000002"}"#.to_vec();
        oversized.resize(16_385, b' ');
        assert!(last_airpods(oversized.as_slice()).is_none());
        assert!(last_airpods(std::io::repeat(b' ')).is_none());
    }

    #[test]
    fn connected_pair_wins_and_stays_selected_until_disconnected() {
        assert_eq!(
            choose_airpods([device(1, false), device(2, true)], 1)
                .unwrap()
                .0,
            2
        );
        assert_eq!(
            choose_airpods([device(1, true), device(2, true)], 2)
                .unwrap()
                .0,
            2
        );
        assert_eq!(
            choose_airpods([device(2, false), device(1, false)], 2)
                .unwrap()
                .0,
            2
        );
        for order in [[1, 2], [2, 1]] {
            assert_eq!(
                choose_airpods(order.map(|a| device(a, true)), 0).unwrap().0,
                1
            );
        }
    }

    #[test]
    fn service_identity_supports_renames_but_unpaired_devices_are_never_selected() {
        let mut renamed = device(1, true);
        renamed.name = "My headphones - Find My".into();
        assert_eq!(
            choose_airpods([renamed], 0),
            Some((1, "My headphones".into()))
        );
        let mut named = device(2, true);
        named.has_service = false;
        assert!(choose_airpods([named], 0).is_some());
        for (authenticated, remembered) in [(false, true), (true, false), (false, false)] {
            let mut unpaired = device(1, true);
            unpaired.authenticated = authenticated;
            unpaired.remembered = remembered;
            assert!(choose_airpods([unpaired], 1).is_none());
        }
        let mut other = device(1, true);
        other.name = "Other headset".into();
        other.has_service = false;
        assert!(choose_airpods([other], 0).is_none());
        for invalid in [0, 0x1_0000_0000_0000] {
            assert!(choose_airpods([device(invalid, true)], invalid).is_none());
        }
    }

    #[test]
    fn service_evidence_breaks_ties_but_never_overrides_a_connected_pair() {
        let mut named = device(1, false);
        named.has_service = false;
        assert_eq!(choose_airpods([named, device(2, false)], 0).unwrap().0, 2);
        let mut named = device(1, true);
        named.has_service = false;
        assert_eq!(choose_airpods([named, device(2, false)], 0).unwrap().0, 1);
    }
}
