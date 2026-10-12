//! Core Audio volume, mute and Conversation Awareness for the current output.

use windows::core::{AgileReference, Error, Interface, BSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_InstanceId;
use windows::Win32::Foundation::{ERROR_NOT_FOUND, E_FAIL};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    eConsole, eRender, AudioSessionStateActive, IAudioSessionControl2, IAudioSessionManager2,
    IDeviceTopology, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
};

/// Join the process MTA for the calling thread. Idempotent-safe (a second call
/// returns S_FALSE). Every daemon thread that touches volume must call this.
pub fn init() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

fn output_device() -> windows::core::Result<IMMDevice> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        #[cfg(feature = "experimental-playback")]
        let address = crate::playback::active_address();
        #[cfg(not(feature = "experimental-playback"))]
        let address = None;
        resolve_output(
            address.is_some(),
            || enumerator.GetDefaultAudioEndpoint(eRender, eConsole),
            || {
                let devices = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
                let mut candidates = Vec::new();
                for index in 0..devices.GetCount()? {
                    let device = devices.Item(index)?;
                    if adapter_instance(&enumerator, &device)
                        .is_ok_and(|id| crate::driver::native_endpoint_owner(&id, address.unwrap()))
                    {
                        candidates.push(device);
                    }
                }
                Ok(candidates)
            },
        )
    }
}

fn adapter_instance(
    enumerator: &IMMDeviceEnumerator,
    endpoint: &IMMDevice,
) -> windows::core::Result<String> {
    unsafe {
        // Endpoint property stores can omit InstanceId; the connected adapter supplies it.
        let topology: IDeviceTopology = endpoint.Activate(CLSCTX_ALL, None)?;
        let adapter_id = topology.GetConnector(0)?.GetDeviceIdConnectedTo()?;
        let adapter = enumerator.GetDevice(adapter_id);
        CoTaskMemFree(Some(adapter_id.0.cast()));
        let properties = adapter?.OpenPropertyStore(STGM_READ)?;
        let instance = properties.GetValue(&PKEY_Device_InstanceId)?;
        Ok(BSTR::try_from(&instance)?.to_string())
    }
}

fn resolve_output<T>(
    experimental: bool,
    default: impl FnOnce() -> windows::core::Result<T>,
    candidates: impl FnOnce() -> windows::core::Result<Vec<T>>,
) -> windows::core::Result<T> {
    if !experimental {
        return default();
    }
    let mut matches = candidates()?;
    match matches.len() {
        0 => Err(Error::new(
            windows::core::HRESULT::from_win32(ERROR_NOT_FOUND.0),
            "AirPods playback endpoint is unavailable",
        )),
        1 => Ok(matches.pop().unwrap()),
        _ => Err(Error::new(
            E_FAIL,
            "Multiple AirPods playback endpoints are active",
        )),
    }
}

fn endpoint() -> windows::core::Result<IAudioEndpointVolume> {
    unsafe { output_device()?.Activate(CLSCTX_ALL, None) }
}

/// Current master volume of the selected output (0..=100), or None if it fails.
pub fn get() -> Option<u8> {
    let vol = endpoint().ok()?;
    let level = unsafe { vol.GetMasterVolumeLevelScalar().ok()? };
    Some((level * 100.0).round().clamp(0.0, 100.0) as u8)
}

pub fn set(percent: u8) {
    if let Ok(vol) = endpoint() {
        let level = percent.min(100) as f32 / 100.0;
        unsafe {
            let _ = vol.SetMasterVolumeLevelScalar(level, std::ptr::null());
        }
    }
}

pub fn step(delta: i32) {
    if let Ok(vol) = endpoint() {
        if let Ok(current) = unsafe { vol.GetMasterVolumeLevelScalar() } {
            let percent = (current * 100.0).round() as i32;
            let level = percent.saturating_add(delta).clamp(0, 100) as f32 / 100.0;
            unsafe {
                let _ = vol.SetMasterVolumeLevelScalar(level, std::ptr::null());
            }
        }
    }
}

pub fn is_muted() -> bool {
    endpoint()
        .and_then(|v| unsafe { v.GetMute() })
        .map(|b| b.as_bool())
        .unwrap_or(false)
}

pub fn toggle_mute() {
    if let Ok(vol) = endpoint() {
        let new = !unsafe { vol.GetMute() }
            .map(|b| b.as_bool())
            .unwrap_or(false);
        unsafe {
            let _ = vol.SetMute(new, std::ptr::null());
        }
    }
}

/// Includes apps without an SMTC session, but excludes notification sounds.
pub fn any_render_active() -> bool {
    unsafe {
        let Ok(device) = output_device() else {
            return false;
        };
        let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
            return false;
        };
        let Ok(sessions) = manager.GetSessionEnumerator() else {
            return false;
        };
        let count = sessions.GetCount().unwrap_or(0);
        for i in 0..count {
            let Ok(session) = sessions.GetSession(i) else {
                continue;
            };
            if session.GetState() != Ok(AudioSessionStateActive) {
                continue;
            }
            // Skip the system-sounds session (tier 3).
            if let Ok(s2) = session.cast::<IAudioSessionControl2>() {
                if !is_application_session(s2.IsSystemSoundsSession()) {
                    continue;
                }
            }
            return true;
        }
        false
    }
}

fn is_application_session(result: windows::core::HRESULT) -> bool {
    result == windows::Win32::Foundation::S_FALSE
}

/// Conversational Awareness volume ducking. The AirPods only *signal* the state
/// (status byte of the 0x4B event) — the host lowers/restores the media volume.
/// Mirrors the app's MediaController::handle_conversational_awareness.
pub struct ConvDuck {
    // CA can be disabled from an IPC thread, so retain a marshalled reference.
    active: Option<DuckedEndpoint<AgileReference<IAudioEndpointVolume>>>,
    limit: u8,
}

impl Default for ConvDuck {
    fn default() -> Self {
        Self::new(25)
    }
}

impl ConvDuck {
    pub fn new(limit: u8) -> Self {
        Self {
            active: None,
            limit: limit.min(100),
        }
    }

    pub fn set_limit(&mut self, limit: u8) {
        self.limit = limit.min(100);
        if let Some(duck) = &mut self.active {
            duck.apply_limit(self.limit);
        }
    }

    /// 1/3 cap playback at the chosen volume; 2 reduces it further to 60% of
    /// that cap. Ending statuses restore only volume that we still own.
    pub fn on_status(&mut self, status: u8) {
        update_duck(&mut self.active, status, self.limit, || {
            AgileReference::new(&endpoint().ok()?).ok()
        });
    }

    /// Restore the pre-duck volume immediately (e.g. the user turned CA off while
    /// it was mid-duck — no end event will come, so we'd be stuck low otherwise).
    pub fn restore(&mut self) {
        restore_duck(&mut self.active);
    }
}

trait DuckVolume {
    fn level(&self) -> Option<f32>;
    fn set_level(&self, level: f32) -> bool;
}

impl DuckVolume for AgileReference<IAudioEndpointVolume> {
    fn level(&self) -> Option<f32> {
        unsafe { self.resolve().ok()?.GetMasterVolumeLevelScalar().ok() }
            .filter(|level| level.is_finite() && (0.0..=1.0).contains(level))
    }

    fn set_level(&self, level: f32) -> bool {
        self.resolve().is_ok_and(|endpoint| unsafe {
            endpoint
                .SetMasterVolumeLevelScalar(level, std::ptr::null())
                .is_ok()
        })
    }
}

struct DuckedEndpoint<E> {
    endpoint: E,
    original: f32,
    expected: f32,
    overridden: bool,
    status: u8,
}

impl<E: DuckVolume> DuckedEndpoint<E> {
    fn apply_limit(&mut self, limit: u8) {
        let cap = f32::from(limit.min(100)) / 100.0;
        let cap = if self.status == 2 { cap * 0.6 } else { cap };
        self.set_if_owned(self.original.min(cap));
    }

    fn set_if_owned(&mut self, target: f32) {
        if self.overridden {
            return;
        }
        let Some(current) = self.endpoint.level() else {
            return;
        };
        if (current - self.expected).abs() > 0.0001 {
            // A user or another app changed this endpoint; leave that choice alone.
            self.overridden = true;
            return;
        }
        if (current - target).abs() > 0.0001 && self.endpoint.set_level(target) {
            self.expected = self.endpoint.level().unwrap_or(target);
        }
    }
}

fn update_duck<E: DuckVolume>(
    active: &mut Option<DuckedEndpoint<E>>,
    status: u8,
    limit: u8,
    find_endpoint: impl FnOnce() -> Option<E>,
) {
    match status {
        4 | 6 | 7 | 8 | 9 => {
            restore_duck(active);
            return;
        }
        1 | 2 | 3 => {}
        _ => return,
    }
    if active.is_none() && status == 1 {
        let Some(endpoint) = find_endpoint() else {
            return;
        };
        let Some(original) = endpoint.level() else {
            return;
        };
        *active = Some(DuckedEndpoint {
            endpoint,
            original,
            expected: original,
            overridden: false,
            status,
        });
    }
    if let Some(duck) = active {
        duck.status = status;
        duck.apply_limit(limit);
    }
}

fn restore_duck<E: DuckVolume>(active: &mut Option<DuckedEndpoint<E>>) {
    if let Some(mut duck) = active.take() {
        duck.set_if_owned(duck.original);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    #[test]
    fn output_resolution_never_falls_back_while_experimental_playback_is_owned() {
        assert_eq!(
            resolve_output(
                false,
                || Ok(7),
                || panic!("normal playback must not enumerate")
            )
            .unwrap(),
            7
        );
        assert_eq!(
            resolve_output(true, || panic!("must not use default"), || Ok(vec![9])).unwrap(),
            9
        );
        for matches in [vec![], vec![8, 9]] {
            assert!(
                resolve_output(true, || panic!("must not use default"), || Ok(matches)).is_err()
            );
        }
        assert!(resolve_output::<u8>(
            true,
            || panic!("must not use default"),
            || Err(Error::new(E_FAIL, "enumeration failed"))
        )
        .is_err());
    }

    #[test]
    fn ordinary_playback_is_not_system_sounds() {
        use windows::Win32::Foundation::{E_FAIL, S_FALSE, S_OK};
        assert!(is_application_session(S_FALSE));
        assert!(!is_application_session(S_OK));
        assert!(!is_application_session(E_FAIL));
    }

    #[derive(Clone)]
    struct Endpoint {
        level: Rc<Cell<f32>>,
        readable: Rc<Cell<bool>>,
        writable: Rc<Cell<bool>>,
        writes: Rc<RefCell<Vec<f32>>>,
    }

    impl Endpoint {
        fn new(level: f32) -> Self {
            Self {
                level: Rc::new(Cell::new(level)),
                readable: Rc::new(Cell::new(true)),
                writable: Rc::new(Cell::new(true)),
                writes: Rc::new(RefCell::new(Vec::new())),
            }
        }
    }

    impl DuckVolume for Endpoint {
        fn level(&self) -> Option<f32> {
            self.readable.get().then(|| self.level.get())
        }

        fn set_level(&self, level: f32) -> bool {
            if !self.writable.get() {
                return false;
            }
            self.level.set(level);
            self.writes.borrow_mut().push(level);
            true
        }
    }

    #[test]
    fn playback_toggle_keeps_duck_restoration_on_the_endpoint_it_changed() {
        let default = Endpoint::new(0.7);
        let native = Endpoint::new(0.8);
        let owned = Cell::new(false);
        let resolve = || {
            resolve_output(
                owned.get(),
                || Ok(default.clone()),
                || Ok(vec![native.clone()]),
            )
            .ok()
        };
        let mut active = None;
        update_duck(&mut active, 1, 25, resolve);
        owned.set(true);
        update_duck(&mut active, 4, 25, resolve);
        assert_eq!(default.level.get(), 0.7);
        assert!(native.writes.borrow().is_empty());

        update_duck(&mut active, 1, 25, resolve);
        owned.set(false);
        update_duck(&mut active, 4, 25, resolve);
        assert_eq!(native.level.get(), 0.8);
        assert_eq!(*default.writes.borrow(), [0.25, 0.7]);
        assert_eq!(*native.writes.borrow(), [0.25, 0.8]);
    }

    #[test]
    fn endpoint_switch_restores_only_the_original_output_without_rounding() {
        let buds = Endpoint::new(0.4732);
        let speakers = Endpoint::new(0.8);
        let mut active = None;
        update_duck(&mut active, 1, 25, || Some(buds.clone()));
        assert_eq!(buds.level.get(), 0.25);
        for status in [2, 3, 4] {
            update_duck(&mut active, status, 25, || Some(speakers.clone()));
        }
        assert_eq!(*buds.writes.borrow(), [0.25, 0.15, 0.25, 0.4732]);
        assert!(speakers.writes.borrow().is_empty());
        assert!(active.is_none());
    }

    #[test]
    fn failed_initial_read_never_owns_or_mutes_the_endpoint() {
        let endpoint = Endpoint::new(0.6);
        endpoint.readable.set(false);
        let mut active = None;
        update_duck(&mut active, 1, 25, || Some(endpoint.clone()));
        assert!(active.is_none());
        endpoint.readable.set(true);
        for status in [2, 3, 4] {
            update_duck(&mut active, status, 25, || Some(endpoint.clone()));
        }
        assert_eq!(endpoint.level.get(), 0.6);
        assert!(endpoint.writes.borrow().is_empty());
    }

    #[test]
    fn manual_volume_change_cancels_the_rest_of_that_conversation() {
        let endpoint = Endpoint::new(0.6);
        let mut active = None;
        update_duck(&mut active, 1, 25, || Some(endpoint.clone()));
        endpoint.level.set(0.42);
        for status in [2, 1, 3, 7] {
            update_duck(&mut active, status, 25, || Some(endpoint.clone()));
        }
        assert_eq!(endpoint.level.get(), 0.42);
        assert_eq!(*endpoint.writes.borrow(), [0.25]);
        update_duck(&mut active, 1, 25, || Some(endpoint.clone()));
        restore_duck(&mut active);
        assert_eq!(endpoint.level.get(), 0.42);
    }

    #[test]
    fn cleanup_restores_once_and_failed_writes_never_change_the_saved_level() {
        let endpoint = Endpoint::new(0.6);
        let mut active = None;
        endpoint.writable.set(false);
        update_duck(&mut active, 1, 25, || Some(endpoint.clone()));
        endpoint.writable.set(true);
        update_duck(&mut active, 2, 25, || Some(endpoint.clone()));
        restore_duck(&mut active);
        restore_duck(&mut active);
        assert_eq!(*endpoint.writes.borrow(), [0.15, 0.6]);
    }

    #[test]
    fn quiet_playback_is_never_raised_by_a_conversation() {
        let endpoint = Endpoint::new(0.1);
        let mut active = None;
        for status in [1, 2, 3, 6] {
            update_duck(&mut active, status, 25, || Some(endpoint.clone()));
        }
        assert!(endpoint.writes.borrow().is_empty());
    }

    #[test]
    fn every_end_status_restores_and_releases_the_saved_endpoint() {
        for status in [4, 6, 7, 8, 9] {
            let endpoint = Endpoint::new(0.6);
            let mut active = None;
            update_duck(&mut active, 1, 25, || Some(endpoint.clone()));
            update_duck(&mut active, status, 25, || {
                panic!("end must use the saved endpoint")
            });
            assert_eq!(endpoint.level.get(), 0.6, "status {status}");
            assert!(active.is_none());
        }
    }

    #[test]
    fn changing_limit_updates_the_current_stage_without_losing_original_volume() {
        let endpoint = Endpoint::new(0.8);
        let mut active = None;
        update_duck(&mut active, 1, 50, || Some(endpoint.clone()));
        assert_eq!(endpoint.level.get(), 0.5);
        update_duck(&mut active, 2, 50, || None);
        assert_eq!(endpoint.level.get(), 0.3);
        active.as_mut().unwrap().apply_limit(20);
        assert!((endpoint.level.get() - 0.12).abs() < 0.0001);
        update_duck(&mut active, 3, 20, || None);
        assert_eq!(endpoint.level.get(), 0.2);
        restore_duck(&mut active);
        assert_eq!(endpoint.level.get(), 0.8);
    }

    #[test]
    fn configurable_limit_clamps_and_does_not_override_manual_volume() {
        assert_eq!(ConvDuck::default().limit, 25);
        assert_eq!(ConvDuck::new(255).limit, 100);
        let endpoint = Endpoint::new(0.8);
        let mut active = None;
        update_duck(&mut active, 1, 0, || Some(endpoint.clone()));
        assert_eq!(endpoint.level.get(), 0.0);
        active.as_mut().unwrap().apply_limit(255);
        assert_eq!(endpoint.level.get(), 0.8);
        endpoint.level.set(0.4);
        active.as_mut().unwrap().apply_limit(10);
        restore_duck(&mut active);
        assert_eq!(endpoint.level.get(), 0.4);
    }
    #[test]
    #[ignore = "requires NTPODS_VOLUME_TEST_ENDPOINT and NTPODS_VOLUME_TEST_ADDRESS; read-only"]
    fn native_adapter_instance_resolves_the_selected_endpoint() {
        init();
        let id = std::env::var("NTPODS_VOLUME_TEST_ENDPOINT").unwrap();
        let address =
            u64::from_str_radix(&std::env::var("NTPODS_VOLUME_TEST_ADDRESS").unwrap(), 16).unwrap();
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
            let device = enumerator
                .GetDevice(windows::core::PCWSTR(crate::wide(&id).as_ptr()))
                .unwrap();
            let instance = adapter_instance(&enumerator, &device).unwrap();
            assert!(crate::driver::native_endpoint_owner(&instance, address));
            assert!(!crate::driver::native_endpoint_owner(
                &instance,
                address ^ 1
            ));
            let volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).unwrap();
            let scalar = volume.GetMasterVolumeLevelScalar().unwrap();
            assert!(scalar.is_finite() && (0.0..=1.0).contains(&scalar));
            assert!(volume.GetMute().is_ok());
        }
    }
}
