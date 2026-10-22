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

/// Initialize COM MTA on each daemon thread before it accesses volume.
pub fn init() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

pub fn output_address(mode: ntpods_ipc::PlaybackMode, selected: u64) -> Option<u64> {
    // A missing selected address must fail, not select unrelated default speakers.
    (mode != ntpods_ipc::PlaybackMode::Windows).then_some(selected)
}

#[cfg(feature = "experimental-playback")]
pub(crate) fn native_output_device(address: u64) -> windows::core::Result<IMMDevice> {
    find_output(Some(address), crate::driver::native_endpoint_owner)
}

pub(crate) fn output_device_for(address: Option<u64>) -> windows::core::Result<IMMDevice> {
    find_output(address, |instance, selected| {
        selected_output_owner(instance, selected, || {
            crate::driver::native_endpoint_owner(instance, selected)
        })
    })
}

fn dongle_adapter(instance: &str) -> bool {
    let mut parts = instance.split('\\');
    parts.next().is_some_and(|part| part.eq_ignore_ascii_case("USB"))
        && parts.next().is_some_and(|part| {
            let fields: Vec<_> = part.split('&').collect();
            fields.iter().any(|field| field.eq_ignore_ascii_case("VID_CAFE"))
                && fields.iter().any(|field| field.eq_ignore_ascii_case("PID_4015"))
                && fields.iter().any(|field| field.eq_ignore_ascii_case("MI_00"))
        })
}

/// A single dongle render endpoint must also be the current default output.
pub(crate) fn default_dongle_output() -> windows::core::Result<IMMDevice> {
    unsafe {
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let default = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let default_id = endpoint_id(&default).ok_or_else(|| Error::from(E_FAIL))?;
        let devices = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        let mut matching = Vec::new();
        for index in 0..devices.GetCount()? {
            let device = devices.Item(index)?;
            if adapter_instance(&enumerator, &device).is_ok_and(|instance| dongle_adapter(&instance)) {
                matching.push(device);
            }
        }
        if matching.len() == 1 && endpoint_id(&matching[0]).as_deref() == Some(&default_id) {
            Ok(matching.pop().unwrap())
        } else {
            Err(Error::from(E_FAIL))
        }
    }
}

pub(crate) fn player_owns_dongle_output(player: &str) -> bool {
    default_dongle_output().is_ok_and(|selected| player_owns_device(selected, player))
}

fn find_output(
    address: Option<u64>,
    matches: impl Fn(&str, u64) -> bool,
) -> windows::core::Result<IMMDevice> {
    if address.is_some_and(|address| address == 0 || address > 0xffff_ffff_ffff) {
        return Err(Error::new(
            windows::Win32::Foundation::E_INVALIDARG,
            "Invalid Bluetooth address",
        ));
    }
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        resolve_output(
            address.is_some(),
            || enumerator.GetDefaultAudioEndpoint(eRender, eConsole),
            || {
                let devices = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
                let mut candidates = Vec::new();
                for index in 0..devices.GetCount()? {
                    let device = devices.Item(index)?;
                    if adapter_instance(&enumerator, &device)
                        .is_ok_and(|id| matches(&id, address.unwrap()))
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

fn selected_adapter(instance: &str, address: u64) -> bool {
    const AUDIO_SINK: &str = r"BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}";
    instance
        .get(..AUDIO_SINK.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(AUDIO_SINK))
        && crate::driver::device_instance_address(instance) == Some(address)
}

fn selected_output_owner(instance: &str, address: u64, native: impl FnOnce() -> bool) -> bool {
    selected_adapter(instance, address) || native()
}

/// An active render endpoint whose connected Bluetooth adapter belongs to this pair.
pub fn selected_output_ready(address: u64) -> bool {
    if address == 0 || address > 0xffff_ffff_ffff {
        return false;
    }
    unsafe {
        let Ok(enumerator): windows::core::Result<IMMDeviceEnumerator> =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
        else {
            return false;
        };
        let Ok(devices) = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) else {
            return false;
        };
        for index in 0..devices.GetCount().unwrap_or(0) {
            let Ok(device) = devices.Item(index) else {
                continue;
            };
            if adapter_instance(&enumerator, &device)
                .ok()
                .is_some_and(|instance| selected_adapter(&instance, address))
            {
                return true;
            }
        }
        false
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

fn endpoint(address: Option<u64>) -> windows::core::Result<IAudioEndpointVolume> {
    unsafe { output_device_for(address)?.Activate(CLSCTX_ALL, None) }
}

fn endpoint_id(device: &IMMDevice) -> Option<String> {
    unsafe {
        let id = device.GetId().ok()?;
        let result = id.to_string().ok();
        CoTaskMemFree(Some(id.0.cast()));
        result
    }
}

fn change_volume(
    address: Option<u64>,
    change: impl FnOnce(&IAudioEndpointVolume) -> windows::core::Result<()>,
) -> Option<String> {
    let device = output_device_for(address).ok()?;
    let id = endpoint_id(&device)?;
    let volume = unsafe { device.Activate(CLSCTX_ALL, None).ok()? };
    change(&volume).ok()?;
    Some(id)
}

/// Current master volume of the selected output (0..=100), or None if it fails.
pub fn get(address: Option<u64>) -> Option<u8> {
    let vol = endpoint(address).ok()?;
    let level = unsafe { vol.GetMasterVolumeLevelScalar().ok()? };
    Some((level * 100.0).round().clamp(0.0, 100.0) as u8)
}

pub fn set(address: Option<u64>, percent: u8) -> Option<String> {
    change_volume(address, |vol| unsafe {
        let level = percent.min(100) as f32 / 100.0;
        vol.SetMasterVolumeLevelScalar(level, std::ptr::null())
    })
}

pub fn step(address: Option<u64>, delta: i32) -> Option<String> {
    change_volume(address, |vol| unsafe {
        let current = vol.GetMasterVolumeLevelScalar()?;
        let percent = (current * 100.0).round() as i32;
        let level = percent.saturating_add(delta).clamp(0, 100) as f32 / 100.0;
        vol.SetMasterVolumeLevelScalar(level, std::ptr::null())
    })
}

pub fn is_muted(address: Option<u64>) -> Option<bool> {
    endpoint(address)
        .and_then(|v| unsafe { v.GetMute() })
        .map(|b| b.as_bool())
        .ok()
}

pub fn update_readback(
    state: &mut ntpods_ipc::Snapshot,
    level: Option<u8>,
    muted: Option<bool>,
) -> bool {
    let level = level.unwrap_or(state.volume);
    let muted = muted.unwrap_or(state.muted);
    let changed = state.volume != level || state.muted != muted;
    state.volume = level;
    state.muted = muted;
    changed
}

pub fn toggle_mute(address: Option<u64>) -> Option<String> {
    change_volume(address, |vol| unsafe {
        let new = !vol.GetMute()?.as_bool();
        vol.SetMute(new, std::ptr::null())
    })
}

/// Includes apps without an SMTC session, but excludes notification sounds.
pub fn any_render_active(address: Option<u64>) -> bool {
    render_activity(address).unwrap_or(false)
}

pub fn render_activity(address: Option<u64>) -> windows::core::Result<bool> {
    unsafe {
        let device = output_device_for(address)?;
        let manager = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None)?;
        let sessions = manager.GetSessionEnumerator()?;
        let count = sessions.GetCount()?;
        active_sessions((0..count).map(|i| {
            let session = sessions.GetSession(i)?;
            if session.GetState()? != AudioSessionStateActive {
                return Ok(false);
            }
            // Skip the system-sounds session (tier 3).
            if let Ok(s2) = session.cast::<IAudioSessionControl2>() {
                if !is_application_session(s2.IsSystemSoundsSession()) {
                    return Ok(false);
                }
            }
            Ok(true)
        }))
    }
}

fn active_sessions(
    sessions: impl IntoIterator<Item = windows::core::Result<bool>>,
) -> windows::core::Result<bool> {
    let mut unavailable = None;
    for active in sessions {
        match active {
            Ok(true) => return Ok(true),
            Ok(false) => {}
            Err(error) => unavailable = Some(error),
        }
    }
    unavailable.map_or(Ok(false), Err)
}

fn is_application_session(result: windows::core::HRESULT) -> bool {
    result == windows::Win32::Foundation::S_FALSE
}

/// Bind an SMTC player to this output before an automatic transfer pauses it.
/// Ambiguous players or the same application rendering elsewhere are refused.
pub(crate) fn player_owns_output(address: u64, player: &str) -> bool {
    output_device_for(Some(address)).is_ok_and(|selected| player_owns_device(selected, player))
}

fn player_owns_device(selected: IMMDevice, player: &str) -> bool {
    (|| -> windows::core::Result<bool> {
        let selected_id = endpoint_id(&selected).ok_or_else(|| Error::from(E_FAIL))?;
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let devices = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
            let mut proof = PlayerOutputProof::default();
            for index in 0..devices.GetCount()? {
                let device = devices.Item(index)?;
                let on_selected = endpoint_id(&device).as_deref() == Some(&selected_id);
                let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
                let sessions = manager.GetSessionEnumerator()?;
                for index in 0..sessions.GetCount()? {
                    let session = sessions
                        .GetSession(index)?
                        .cast::<IAudioSessionControl2>()?;
                    if session.GetState()? != AudioSessionStateActive
                        || !is_application_session(session.IsSystemSoundsSession())
                    {
                        continue;
                    }
                    let identity = process_player_identity(session.GetProcessId()?);
                    let matches = identity
                        .as_ref()
                        .map(|(app, image)| player_identity_matches(player, app.as_deref(), image));
                    proof.observe(on_selected, matches);
                }
            }
            Ok(proof.permits_pause())
        }
    })()
    .unwrap_or(false)
}

#[derive(Default)]
struct PlayerOutputProof {
    selected_matches: usize,
    ambiguous: bool,
}

impl PlayerOutputProof {
    fn observe(&mut self, selected: bool, matches: Option<bool>) {
        if selected {
            if matches == Some(true) {
                self.selected_matches += 1;
            } else {
                self.ambiguous = true;
            }
        } else if matches != Some(false) {
            self.ambiguous = true;
        }
    }

    fn permits_pause(&self) -> bool {
        self.selected_matches == 1 && !self.ambiguous
    }
}

fn process_player_identity(pid: u32) -> Option<(Option<String>, String)> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::Storage::Packaging::Appx::GetApplicationUserModelId;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut image = [0u16; 32768];
        let mut length = image.len() as u32;
        let image_ok = QueryFullProcessImageNameW(process, 0, image.as_mut_ptr(), &mut length);
        let mut app = [0u16; 512];
        let mut app_length = app.len() as u32;
        let app_ok = GetApplicationUserModelId(process, &mut app_length, app.as_mut_ptr());
        CloseHandle(process);
        if image_ok == 0 || length as usize > image.len() {
            return None;
        }
        let app = (app_ok == 0 && app_length > 1 && app_length as usize <= app.len())
            .then(|| String::from_utf16_lossy(&app[..app_length as usize - 1]));
        Some((app, String::from_utf16_lossy(&image[..length as usize])))
    }
}

fn player_identity_matches(player: &str, app: Option<&str>, image: &str) -> bool {
    if player.is_empty() {
        return false;
    }
    if let Some(app) = app {
        return player == app;
    }
    // A packaged ID cannot be inferred from a desktop executable name.
    if player.contains('!') || !player.to_ascii_lowercase().ends_with(".exe") {
        return false;
    }
    if player.contains(['\\', '/']) {
        player.eq_ignore_ascii_case(image)
    } else {
        image
            .rsplit(['\\', '/'])
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case(player))
    }
}

/// Host volume ducking driven by the AirPods' 0x4B Conversation Awareness events.
pub struct ConvDuck {
    // CA can be disabled from an IPC thread, so retain a marshalled reference.
    active: Option<DuckedEndpoint<DuckEndpoint>>,
    policy: ntpods_ipc::ConversationPolicy,
    paused: Option<ConversationPause>,
    speaking_output: Option<(u64, String)>,
}

struct ConversationPause {
    ticket: crate::media::SelectedPause,
    route: (u64, String),
    speech: SpeechPause,
    permitted: Option<Box<dyn FnOnce() -> bool + Send>>,
    resume_sent: bool,
}

struct SpeechPause {
    active: std::sync::Arc<std::sync::atomic::AtomicBool>,
    resume_after: Option<std::time::Instant>,
}

impl SpeechPause {
    fn new() -> Self {
        Self {
            active: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            resume_after: None,
        }
    }

    fn speaking(&mut self, speaking: bool, now: std::time::Instant) {
        let previous = self
            .active
            .swap(speaking, std::sync::atomic::Ordering::AcqRel);
        if speaking {
            self.resume_after = None;
        } else if previous {
            self.resume_after = Some(now + std::time::Duration::from_millis(250));
        }
    }

    fn may_resume(&self, now: std::time::Instant) -> bool {
        self.resume_after.is_some_and(|at| now >= at)
    }
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
            policy: ntpods_ipc::ConversationPolicy::from_legacy(limit),
            paused: None,
            speaking_output: None,
        }
    }

    pub fn set_limit(&mut self, limit: u8) {
        self.set_policy(ntpods_ipc::ConversationPolicy::from_legacy(limit));
    }

    pub fn set_policy(&mut self, policy: ntpods_ipc::ConversationPolicy) {
        if !policy.is_valid() {
            return;
        }
        if self.policy.pause_media && !policy.pause_media {
            self.finish_pause();
        }
        self.policy = policy;
        if let Some(duck) = &mut self.active {
            duck.apply_policy(&self.policy);
        }
    }

    /// Ending statuses restore only volume and playback still owned by this episode.
    pub fn on_status(
        &mut self,
        status: u8,
        address: u64,
        output: Option<u64>,
        permitted: impl FnOnce() -> bool + Send + 'static,
    ) {
        if matches!(status, 1 | 2 | 3) {
            let current = DuckEndpoint::current(address, output);
            let route = current
                .as_ref()
                .map(|endpoint| (address, endpoint.id.clone()));
            if self.speaking_output.is_some() && self.speaking_output != route {
                self.paused = None;
                self.speaking_output = None;
            }
            if self
                .paused
                .as_ref()
                .is_some_and(|paused| Some(&paused.route) != route.as_ref())
            {
                self.paused = None;
            }
            if let Some(paused) = self.paused.as_mut() {
                self.speaking_output = route.clone();
                if self.policy.pause_media {
                    paused.speech.speaking(true, std::time::Instant::now());
                }
            } else if status == 1 && self.speaking_output.is_none() {
                self.speaking_output = route.clone();
                if self.policy.pause_media {
                    if let Some(route) = route {
                        self.paused = Some(ConversationPause {
                            ticket: crate::media::SelectedPause::start(address),
                            route,
                            speech: SpeechPause::new(),
                            permitted: Some(Box::new(permitted)),
                            resume_sent: false,
                        });
                    }
                }
            }
            update_duck_for_route_policy(&mut self.active, status, &self.policy, current);
        } else {
            update_duck_policy(&mut self.active, status, &self.policy, || None);
            if matches!(status, 4 | 6 | 7 | 8 | 9) {
                self.speaking_output = None;
                self.finish_pause();
            }
        }
        self.poll();
    }

    /// Disabling Conversation Awareness may omit the end event; restore any owned duck.
    pub fn restore(&mut self) {
        restore_duck(&mut self.active);
        self.speaking_output = None;
        self.finish_pause();
    }

    fn finish_pause(&mut self) {
        if let Some(paused) = self.paused.as_mut() {
            if !paused.resume_sent {
                paused.speech.speaking(false, std::time::Instant::now());
            }
        }
    }

    /// Called by the existing host poll so media work never blocks Bluetooth reception.
    pub fn poll(&mut self) {
        let Some(paused) = self.paused.as_mut() else {
            return;
        };
        while let Some(receipt) = paused.ticket.poll() {
            if matches!(
                receipt,
                crate::media::PauseReceipt::Failed | crate::media::PauseReceipt::Resumed(_)
            ) {
                self.paused = None;
                return;
            }
        }
        if !paused.resume_sent && paused.speech.may_resume(std::time::Instant::now()) {
            let speech = paused.speech.active.clone();
            let permitted = paused.permitted.take();
            paused.resume_sent = true;
            if !paused.ticket.resume(move || {
                !speech.load(std::sync::atomic::Ordering::Acquire)
                    && permitted.is_some_and(|permitted| permitted())
            }) {
                self.paused = None;
            }
        }
    }

    /// A successful choice takes ownership only on the output that was ducked.
    pub fn manual_override(&mut self, changed_endpoint: Option<&str>) {
        override_duck(&mut self.active, changed_endpoint);
    }
}

trait DuckVolume {
    fn id(&self) -> &str;
    fn level(&self) -> Option<f32>;
    fn set_level(&self, level: f32) -> bool;
    fn muted(&self) -> Option<bool>;
    fn remote_step(&self) -> Result<Option<u8>, ()> {
        Ok(None)
    }
}

struct DuckEndpoint {
    id: String,
    endpoint: AgileReference<IAudioEndpointVolume>,
    remote_volume: bool,
}

impl DuckEndpoint {
    fn current(address: u64, output: Option<u64>) -> Option<Self> {
        if address == 0 {
            return None;
        }
        unsafe {
            let device = output_device_for(output).ok()?;
            let id = endpoint_id(&device)?;
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            let instance = adapter_instance(&enumerator, &device).ok()?;
            let native = crate::driver::native_endpoint_owner(&instance, address);
            let owned = output.is_none_or(|output| output == address)
                && (native || crate::driver::device_instance_address(&instance) == Some(address));
            if !owned {
                return None;
            }
            let volume: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None).ok()?;
            let remote_volume = native
                && volume.QueryHardwareSupport().unwrap_or(0)
                    & windows::Win32::Media::Audio::ENDPOINT_HARDWARE_SUPPORT_VOLUME
                    != 0;
            Some(Self {
                id,
                endpoint: AgileReference::new(&volume).ok()?,
                remote_volume,
            })
        }
    }
}

impl DuckVolume for DuckEndpoint {
    fn id(&self) -> &str {
        &self.id
    }

    fn level(&self) -> Option<f32> {
        unsafe {
            self.endpoint
                .resolve()
                .ok()?
                .GetMasterVolumeLevelScalar()
                .ok()
        }
        .filter(|level| level.is_finite() && (0.0..=1.0).contains(level))
    }

    fn set_level(&self, level: f32) -> bool {
        self.endpoint.resolve().is_ok_and(|endpoint| unsafe {
            endpoint
                .SetMasterVolumeLevelScalar(level, std::ptr::null())
                .is_ok()
        })
    }

    fn muted(&self) -> Option<bool> {
        unsafe {
            self.endpoint
                .resolve()
                .ok()?
                .GetMute()
                .ok()
                .map(|mute| mute.as_bool())
        }
    }

    fn remote_step(&self) -> Result<Option<u8>, ()> {
        if !self.remote_volume {
            return Ok(None);
        }
        let db = unsafe {
            self.endpoint
                .resolve()
                .map_err(|_| ())?
                .GetMasterVolumeLevel()
                .map_err(|_| ())?
        };
        avrcp_volume_step(db).map(Some).ok_or(())
    }
}

fn avrcp_volume_step(db: f32) -> Option<u8> {
    if !db.is_finite() {
        return None;
    }
    // Windows' AVRCP bridge uses this taper; compare wire steps, not rounded echoes.
    let floor = 10.0_f64.powf(-63.5 / 35.0);
    let wiper = (10.0_f64.powf(f64::from(db.clamp(-63.5, 0.0)) / 35.0) - floor) / (1.0 - floor);
    Some((wiper * 127.0).round().clamp(0.0, 127.0) as u8)
}

struct DuckedEndpoint<E> {
    endpoint: E,
    original: f32,
    expected: f32,
    expected_step: Option<u8>,
    expected_muted: bool,
    overridden: bool,
    status: u8,
}

impl<E: DuckVolume> DuckedEndpoint<E> {
    fn apply_policy(&mut self, policy: &ntpods_ipc::ConversationPolicy) {
        if let Some(target) = policy.target(self.original, self.status) {
            self.set_if_owned(target);
        }
    }

    #[cfg(test)]
    fn apply_limit(&mut self, limit: u8) {
        self.apply_policy(&ntpods_ipc::ConversationPolicy::from_legacy(limit));
    }

    fn set_if_owned(&mut self, target: f32) {
        if self.overridden {
            return;
        }
        let Some(current) = self.endpoint.level() else {
            return;
        };
        let owned = match (self.expected_step, self.endpoint.remote_step()) {
            (Some(expected), Ok(Some(current))) => current == expected,
            (None, Ok(None)) => (current - self.expected).abs() <= 0.0001,
            _ => false,
        };
        if !owned || self.endpoint.muted() != Some(self.expected_muted) {
            // A user or another app changed this endpoint; leave that choice alone.
            self.overridden = true;
            return;
        }
        if (current - target).abs() > 0.0001 && self.endpoint.set_level(target) {
            self.expected = self.endpoint.level().unwrap_or(target);
            match self.endpoint.remote_step() {
                Ok(step) => self.expected_step = step,
                Err(()) => self.overridden = true,
            }
            match self.endpoint.muted() {
                Some(muted) => self.expected_muted = muted,
                None => self.overridden = true,
            }
        }
    }
}

fn update_duck_policy<E: DuckVolume>(
    active: &mut Option<DuckedEndpoint<E>>,
    status: u8,
    policy: &ntpods_ipc::ConversationPolicy,
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
        let Ok(expected_step) = endpoint.remote_step() else {
            return;
        };
        let Some(expected_muted) = endpoint.muted() else {
            return;
        };
        if expected_muted {
            return;
        }
        *active = Some(DuckedEndpoint {
            endpoint,
            original,
            expected: original,
            expected_step,
            expected_muted,
            overridden: false,
            status,
        });
    }
    if let Some(duck) = active {
        duck.status = status;
        duck.apply_policy(policy);
    }
}

fn update_duck_for_route_policy<E: DuckVolume>(
    active: &mut Option<DuckedEndpoint<E>>,
    status: u8,
    policy: &ntpods_ipc::ConversationPolicy,
    current: Option<E>,
) {
    if active
        .as_ref()
        .is_some_and(|duck| current.as_ref().map(DuckVolume::id) != Some(duck.endpoint.id()))
    {
        restore_duck(active);
    }
    update_duck_policy(active, status, policy, || current);
}

#[cfg(test)]
fn update_duck<E: DuckVolume>(
    active: &mut Option<DuckedEndpoint<E>>,
    status: u8,
    limit: u8,
    find: impl FnOnce() -> Option<E>,
) {
    update_duck_policy(
        active,
        status,
        &ntpods_ipc::ConversationPolicy::from_legacy(limit),
        find,
    );
}

#[cfg(test)]
fn update_duck_for_route<E: DuckVolume>(
    active: &mut Option<DuckedEndpoint<E>>,
    status: u8,
    limit: u8,
    current: Option<E>,
) {
    update_duck_for_route_policy(
        active,
        status,
        &ntpods_ipc::ConversationPolicy::from_legacy(limit),
        current,
    );
}

fn restore_duck<E: DuckVolume>(active: &mut Option<DuckedEndpoint<E>>) {
    if let Some(mut duck) = active.take() {
        duck.set_if_owned(duck.original);
    }
}

fn override_duck<E: DuckVolume>(
    active: &mut Option<DuckedEndpoint<E>>,
    changed_endpoint: Option<&str>,
) {
    if let Some(duck) = active {
        if changed_endpoint == Some(duck.endpoint.id()) {
            // Retain the conversation until its end event, including repeated starts.
            duck.overridden = true;
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn dongle_endpoint_requires_the_exact_audio_interface_identity() {
        assert!(super::dongle_adapter(r"USB\VID_CAFE&PID_4015&MI_00\instance"));
        for instance in [r"USB\VID_CAFE&PID_4015&MI_02\x", r"BTHENUM\VID_CAFE&PID_4015&MI_00\x",
            r"USB\VID_CAFE&PID_40150&MI_00\x", r"USB\VID_CAFE&PID_4015\x"] {
            assert!(!super::dongle_adapter(instance));
        }
    }
    #[test]
    fn conversation_resume_waits_for_a_stable_end_without_extending_on_repeated_end_reports() {
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let mut speech = super::SpeechPause::new();
        speech.speaking(false, now);
        assert!(!speech.may_resume(now + Duration::from_millis(249)));
        speech.speaking(false, now + Duration::from_millis(200));
        assert!(speech.may_resume(now + Duration::from_millis(250)));
    }

    #[test]
    fn new_speech_revokes_a_pending_conversation_resume() {
        use std::sync::atomic::Ordering;
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let mut speech = super::SpeechPause::new();
        let guard = speech.active.clone();
        speech.speaking(false, now);
        speech.speaking(true, now + Duration::from_millis(100));
        assert!(guard.load(Ordering::Acquire));
        assert!(!speech.may_resume(now + Duration::from_secs(1)));
        speech.speaking(false, now + Duration::from_secs(2));
        assert!(speech.may_resume(now + Duration::from_millis(2250)));
    }

    #[test]
    fn automatic_pause_requires_one_unambiguous_player_on_the_selected_output() {
        for (sessions, expected) in [
            (vec![], false),
            (vec![(true, Some(true))], true),
            (vec![(true, Some(true)), (false, Some(false))], true),
            (vec![(true, Some(true)), (true, Some(true))], false),
            (vec![(true, Some(true)), (true, Some(false))], false),
            (vec![(true, Some(true)), (false, Some(true))], false),
            (vec![(true, Some(true)), (false, None)], false),
            (vec![(true, None)], false),
        ] {
            let mut proof = super::PlayerOutputProof::default();
            for (selected, matches) in sessions {
                proof.observe(selected, matches);
            }
            assert_eq!(proof.permits_pause(), expected);
        }
    }

    #[test]
    fn player_identity_requires_exact_packaged_id_or_desktop_executable() {
        assert!(super::player_identity_matches(
            "Music.Package!App",
            Some("Music.Package!App"),
            r"C:\Apps\player.exe"
        ));
        assert!(!super::player_identity_matches(
            "Music.Package!Other",
            Some("Music.Package!App"),
            r"C:\Apps\player.exe"
        ));
        assert!(!super::player_identity_matches(
            "player.exe",
            Some("Music.Package!App"),
            r"C:\Apps\player.exe"
        ));
        assert!(super::player_identity_matches(
            "Spotify.exe",
            None,
            r"C:\Apps\Spotify.EXE"
        ));
        assert!(!super::player_identity_matches(
            "Spotify",
            None,
            r"C:\Apps\Spotify.exe"
        ));
        assert!(!super::player_identity_matches(
            "Music.Package!App",
            None,
            r"C:\Apps\App.exe"
        ));
        assert!(!super::player_identity_matches(
            r"C:\Other\Spotify.exe",
            None,
            r"C:\Apps\Spotify.exe"
        ));
        assert!(!super::player_identity_matches(
            "",
            None,
            r"C:\Apps\player.exe"
        ));
    }

    #[test]
    fn activity_errors_cannot_hide_known_playback_or_prove_idle() {
        use windows::Win32::Foundation::E_FAIL;
        assert_eq!(
            super::active_sessions([Err(E_FAIL.into()), Ok(true)]).unwrap(),
            true
        );
        assert!(super::active_sessions([Ok(false), Err(E_FAIL.into())]).is_err());
        assert!(!super::active_sessions([Ok(false), Ok(false)]).unwrap());
    }

    use super::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    #[test]
    fn selected_render_adapter_rejects_other_pairs_and_non_audio_services() {
        let address = 0xaabbccddeeff;
        let sink = r"BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004c_PID&2027\7&1&2&AABBCCDDEEFF_C00000000";
        let other = r"BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004c_PID&2027\7&1&2&112233445566_C00000000";
        let handsfree = r"BTHENUM\{0000111e-0000-1000-8000-00805f9b34fb}_VID&0001004c_PID&2027\7&1&2&AABBCCDDEEFF_C00000000";
        let native = r"BTHENUM\{74ec2172-0bad-4d01-8f77-997b2be0722a}_VID&0001004c_PID&2027\7&1&2&AABBCCDDEEFF_C00000000";
        assert!(selected_adapter(sink, address));
        assert!(!selected_adapter(other, address));
        assert!(!selected_adapter(handsfree, address));
        assert!(!selected_adapter(native, address));
    }

    #[test]
    #[cfg(feature = "experimental-playback")]
    fn native_output_rejects_invalid_addresses_before_using_com() {
        for address in [0, 0x1_0000_0000_0000, u64::MAX] {
            assert_eq!(
                native_output_device(address).unwrap_err().code(),
                windows::Win32::Foundation::E_INVALIDARG
            );
        }
    }

    #[test]
    fn configured_native_volume_never_uses_default_during_recovery() {
        use ntpods_ipc::PlaybackMode;
        let selected = 0xaabbccddeeff;
        for mode in [PlaybackMode::Aac, PlaybackMode::AacEld] {
            let target = output_address(mode, selected);
            assert_eq!(target, Some(selected));
            let error = resolve_output::<u8>(
                target.is_some(),
                || panic!("recovering AirPods must not change default speakers"),
                || Ok(vec![]),
            )
            .unwrap_err();
            assert_eq!(
                error.code(),
                windows::core::HRESULT::from_win32(ERROR_NOT_FOUND.0)
            );
            for missing in [0, 0x1_0000_0000_0000] {
                assert_eq!(output_address(mode, missing), Some(missing));
                assert_eq!(
                    output_device_for(output_address(mode, missing))
                        .unwrap_err()
                        .code(),
                    windows::Win32::Foundation::E_INVALIDARG
                );
            }
        }
        for address in [0, selected] {
            let target = output_address(PlaybackMode::Windows, address);
            assert_eq!(target, None);
            assert_eq!(
                resolve_output(
                    target.is_some(),
                    || Ok(7),
                    || panic!("legacy output uses default")
                )
                .unwrap(),
                7
            );
        }
    }

    #[test]
    fn native_preference_still_controls_the_selected_windows_audio_sink() {
        let address = 0xaabbccddeeff;
        let sink = r"BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004c_PID&2027\7&1&2&AABBCCDDEEFF_C00000000";
        let other = sink.replace("AABBCCDDEEFF", "112233445566");
        let speakers = r"HDAUDIO\FUNC_01&VEN_10EC\4&123";
        for mode in [
            ntpods_ipc::PlaybackMode::Aac,
            ntpods_ipc::PlaybackMode::AacEld,
        ] {
            let target = output_address(mode, address);
            let candidates: Vec<_> = [speakers, other.as_str(), sink]
                .into_iter()
                .filter(|instance| selected_output_owner(instance, target.unwrap(), || false))
                .collect();
            assert_eq!(
                resolve_output(
                    target.is_some(),
                    || panic!("must not use default speakers"),
                    || Ok(candidates)
                )
                .unwrap(),
                sink
            );
        }
        assert!(selected_output_owner(
            "native child adapter",
            address,
            || true
        ));
        assert!(!selected_output_owner(
            "native child adapter",
            address,
            || false
        ));
    }

    #[test]
    fn unavailable_readbacks_preserve_volume_and_mute() {
        let mut state = ntpods_ipc::Snapshot {
            volume: 53,
            muted: true,
            ..Default::default()
        };
        assert!(!update_readback(&mut state, None, None));
        assert_eq!((state.volume, state.muted), (53, true));
        assert!(update_readback(&mut state, Some(0), None));
        assert_eq!((state.volume, state.muted), (0, true));
        assert!(update_readback(&mut state, None, Some(false)));
        assert_eq!((state.volume, state.muted), (0, false));
        assert!(!update_readback(&mut state, Some(0), Some(false)));
    }

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
        id: &'static str,
        level: Rc<Cell<f32>>,
        readable: Rc<Cell<bool>>,
        writable: Rc<Cell<bool>>,
        remote_step: Rc<Cell<Result<Option<u8>, ()>>>,
        muted: Rc<Cell<bool>>,
        writes: Rc<RefCell<Vec<f32>>>,
    }

    impl Endpoint {
        fn new(level: f32) -> Self {
            Self {
                id: "buds",
                level: Rc::new(Cell::new(level)),
                readable: Rc::new(Cell::new(true)),
                writable: Rc::new(Cell::new(true)),
                remote_step: Rc::new(Cell::new(Ok(None))),
                muted: Rc::new(Cell::new(false)),
                writes: Rc::new(RefCell::new(Vec::new())),
            }
        }
    }

    impl DuckVolume for Endpoint {
        fn id(&self) -> &str {
            self.id
        }

        fn level(&self) -> Option<f32> {
            self.readable.get().then(|| self.level.get())
        }

        fn set_level(&self, level: f32) -> bool {
            if !self.writable.get() {
                return false;
            }
            self.level.set(level);
            if self.remote_step.get().is_ok_and(|step| step.is_some()) {
                self.remote_step
                    .set(Ok(Some((level * 127.0).round() as u8)));
                self.muted.set(level == 0.0);
            }
            self.writes.borrow_mut().push(level);
            true
        }

        fn remote_step(&self) -> Result<Option<u8>, ()> {
            self.remote_step.get()
        }

        fn muted(&self) -> Option<bool> {
            self.readable.get().then(|| self.muted.get())
        }
    }

    #[test]
    fn avrcp_ownership_round_trips_every_wire_step_after_fixed_point_truncation() {
        let floor = 10.0_f64.powf(-63.5 / 35.0);
        for step in 0..=127 {
            let db = (35.0 * (floor + (1.0 - floor) * f64::from(step) / 127.0).log10()) as f32;
            let fixed = (db * 65536.0) as i32;
            assert_eq!(avrcp_volume_step(fixed as f32 / 65536.0), Some(step));
        }
        assert_eq!(avrcp_volume_step(-100.0), Some(0));
        assert_eq!(avrcp_volume_step(10.0), Some(127));
        for db in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(avrcp_volume_step(db), None);
        }
    }

    #[test]
    fn rounded_remote_echo_preserves_duck_but_adjacent_remote_step_cancels_it() {
        for manual_change in [false, true] {
            let endpoint = Endpoint::new(0.6);
            endpoint.remote_step.set(Ok(Some(76)));
            let mut active = None;
            update_duck(&mut active, 1, 25, || Some(endpoint.clone()));
            // The fake uses a linear scalar; production derives the wire step from dB.
            let step = if manual_change { 33 } else { 32 };
            endpoint.remote_step.set(Ok(Some(step)));
            endpoint.level.set(f32::from(step) / 127.0);
            restore_duck(&mut active);
            assert_eq!(
                endpoint.writes.borrow().len(),
                if manual_change { 1 } else { 2 }
            );
            if !manual_change {
                assert_eq!(endpoint.level.get(), 0.6);
            }
        }
    }

    #[test]
    fn remote_read_failure_cannot_acquire_or_restore_duck_ownership() {
        let endpoint = Endpoint::new(0.6);
        endpoint.remote_step.set(Err(()));
        let mut active = None;
        update_duck(&mut active, 1, 25, || Some(endpoint.clone()));
        assert!(active.is_none());
        assert!(endpoint.writes.borrow().is_empty());
        endpoint.remote_step.set(Ok(Some(76)));
        update_duck(&mut active, 1, 25, || Some(endpoint.clone()));
        endpoint.remote_step.set(Err(()));
        update_duck(&mut active, 2, 25, || None);
        endpoint.remote_step.set(Ok(Some(32)));
        restore_duck(&mut active);
        assert_eq!(*endpoint.writes.borrow(), [0.25]);
    }

    #[test]
    fn user_mute_cancels_duck_restoration_but_our_zero_limit_can_restore() {
        for user_mute in [false, true] {
            let endpoint = Endpoint::new(0.6);
            endpoint.remote_step.set(Ok(Some(76)));
            let mut active = None;
            let limit = if user_mute { 25 } else { 0 };
            update_duck(&mut active, 1, limit, || Some(endpoint.clone()));
            endpoint.muted.set(true);
            restore_duck(&mut active);
            assert_eq!(endpoint.muted.get(), user_mute);
            assert_eq!(endpoint.level.get(), if user_mute { 0.25 } else { 0.6 });
        }
        let endpoint = Endpoint::new(0.6);
        endpoint.muted.set(true);
        let mut active = None;
        update_duck(&mut active, 1, 25, || Some(endpoint.clone()));
        assert!(active.is_none());
        assert!(endpoint.writes.borrow().is_empty());
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
    fn conversation_duck_only_uses_the_selected_airpods_route() {
        let selected = 0xaabbccddeeff;
        let buds = Endpoint::new(0.6);
        let mut speakers = Endpoint::new(0.8);
        speakers.id = "speakers";
        let buds_instance = r"BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004c_PID&2027\7&1&2&AABBCCDDEEFF_C00000000";
        let speakers_instance = r"HDAUDIO\FUNC_01&VEN_10EC\4&123";
        let selected_endpoint = |endpoint, instance| {
            (crate::driver::device_instance_address(instance) == Some(selected)).then_some(endpoint)
        };
        let mut active = None;
        update_duck_for_route(
            &mut active,
            1,
            25,
            selected_endpoint(speakers.clone(), speakers_instance),
        );
        assert!(active.is_none());
        assert!(speakers.writes.borrow().is_empty());

        update_duck_for_route(
            &mut active,
            1,
            25,
            selected_endpoint(buds.clone(), buds_instance),
        );
        assert_eq!(*buds.writes.borrow(), [0.25]);

        update_duck_for_route(
            &mut active,
            2,
            25,
            selected_endpoint(speakers.clone(), speakers_instance),
        );
        assert!(active.is_none());
        assert_eq!(*buds.writes.borrow(), [0.25, 0.6]);
        assert!(speakers.writes.borrow().is_empty());
    }

    #[test]
    fn switching_between_paired_outputs_restores_the_old_duck_before_starting_the_new_one() {
        let first = Endpoint::new(0.6);
        let mut second = Endpoint::new(0.8);
        second.id = "other buds";
        let mut active = None;
        update_duck_for_route(&mut active, 1, 25, Some(first.clone()));
        update_duck_for_route(&mut active, 1, 25, Some(second.clone()));
        assert_eq!(*first.writes.borrow(), [0.25, 0.6]);
        assert_eq!(*second.writes.borrow(), [0.25]);
        restore_duck(&mut active);
        assert_eq!(*second.writes.borrow(), [0.25, 0.8]);
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
    fn explicit_same_level_volume_cancels_restore_and_later_ducking() {
        let endpoint = Endpoint::new(0.6);
        let mut active = None;
        update_duck(&mut active, 1, 25, || Some(endpoint.clone()));
        override_duck(
            &mut active,
            endpoint.set_level(0.25).then_some(endpoint.id()),
        );
        for status in [2, 1, 3] {
            update_duck(&mut active, status, 25, || Some(endpoint.clone()));
        }
        active.as_mut().unwrap().apply_limit(10);
        update_duck(&mut active, 4, 25, || None);
        assert_eq!(endpoint.level.get(), 0.25);
        assert_eq!(*endpoint.writes.borrow(), [0.25, 0.25]);
        assert!(active.is_none());
    }

    #[test]
    fn next_conversation_restores_the_manually_chosen_baseline() {
        let endpoint = Endpoint::new(0.6);
        let mut active = None;
        update_duck(&mut active, 1, 25, || Some(endpoint.clone()));
        override_duck(
            &mut active,
            endpoint.set_level(0.25).then_some(endpoint.id()),
        );
        restore_duck(&mut active);
        for status in [1, 2, 4] {
            update_duck(&mut active, status, 25, || Some(endpoint.clone()));
        }
        assert_eq!(endpoint.level.get(), 0.25);
        assert_eq!(*endpoint.writes.borrow(), [0.25, 0.25, 0.15, 0.25]);
        assert!(active.is_none());
    }

    #[test]
    fn manual_command_on_another_output_keeps_the_original_restore() {
        let buds = Endpoint::new(0.6);
        let mut speakers = Endpoint::new(0.8);
        speakers.id = "speakers";
        let mut active = None;
        update_duck(&mut active, 1, 25, || Some(buds.clone()));
        override_duck(
            &mut active,
            speakers.set_level(0.7).then_some(speakers.id()),
        );
        for status in [2, 4] {
            update_duck(&mut active, status, 25, || Some(speakers.clone()));
        }
        assert_eq!(*buds.writes.borrow(), [0.25, 0.15, 0.6]);
        assert_eq!(*speakers.writes.borrow(), [0.7]);
        assert!(active.is_none());
    }

    #[test]
    fn failed_manual_write_keeps_ducking_and_restoration() {
        let endpoint = Endpoint::new(0.6);
        let mut active = None;
        update_duck(&mut active, 1, 25, || Some(endpoint.clone()));
        endpoint.writable.set(false);
        override_duck(
            &mut active,
            endpoint.set_level(0.7).then_some(endpoint.id()),
        );
        endpoint.writable.set(true);
        for status in [2, 4] {
            update_duck(&mut active, status, 25, || Some(endpoint.clone()));
        }
        assert_eq!(*endpoint.writes.borrow(), [0.25, 0.15, 0.6]);
        assert!(active.is_none());
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
        assert_eq!(ConvDuck::default().policy.first, "25");
        assert_eq!(ConvDuck::new(255).policy.first, "100");
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
