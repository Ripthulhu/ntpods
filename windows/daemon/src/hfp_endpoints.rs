//! Selected AirPods hands-free control and legacy endpoint visibility recovery.

use std::ffi::c_void;
use std::io;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use windows::core::{
    Error, IUnknown, IUnknown_Vtbl, Interface, Result, BSTR, GUID, HRESULT, PCWSTR,
};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_InstanceId;
use windows::Win32::Foundation::{ERROR_NOT_FOUND, E_CHANGED_STATE, E_FAIL, E_INVALIDARG};
use windows::Win32::Media::Audio::{
    eAll, eCapture, eRender, AudioSessionStateActive, IAudioSessionManager2, IDeviceTopology,
    IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
    DEVICE_STATE, DEVICE_STATEMASK_ALL, DEVICE_STATE_ACTIVE, DEVICE_STATE_DISABLED,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED, STGM_READ,
};
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_DevNode_Registry_PropertyW, CM_Get_DevNode_Status, CM_Get_Device_IDW,
    CM_Get_Device_ID_ListW, CM_Get_Device_ID_List_SizeW, CM_Get_Device_Interface_ListW,
    CM_Get_Device_Interface_List_SizeW, CM_Get_Parent, CM_Locate_DevNodeW, CM_DRP_HARDWAREID,
    CM_DRP_SERVICE, CM_GETIDLIST_FILTER_PRESENT, CM_GETIDLIST_FILTER_SERVICE,
    CM_GET_DEVICE_INTERFACE_LIST_PRESENT, CM_LOCATE_DEVNODE_PHANTOM, CR_SUCCESS, DN_STARTED,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_IO_PENDING, ERROR_NOT_SUPPORTED, GENERIC_READ, GENERIC_WRITE, HANDLE,
    INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows_sys::Win32::System::IO::{
    CancelIoEx, DeviceIoControl, GetOverlappedResult, OVERLAPPED,
};

const CONTROL_BUDGET: Duration = Duration::from_secs(2);
const KS_PROPERTY: u32 = 0x002f_0003;
const PROPERTY_GET: u32 = 1;
const PROPERTY_SUPPORT: u32 = 0x200;
const BT_AUDIO: GUID = GUID::from_u128(0x7fa06c40_b8f6_4c7e_8556_e8c33a12e54d);
const PIN: GUID = GUID::from_u128(0x8c134960_51ad_11cf_878a_94f801c10000);
const JACK: GUID = GUID::from_u128(0x4509f757_2d46_4637_8e62_ce7db944f57b);
const AUDIO_CATEGORY: windows_sys::core::GUID =
    windows_sys::core::GUID::from_u128(0x6994ad04_93ef_11d0_a3cc_00a0c9223196);
static CONTROL_BUSY: AtomicBool = AtomicBool::new(false);

#[derive(Debug, PartialEq, Eq)]
pub struct ControlStatus {
    pub request_acknowledged: bool,
    pub jack_ready: bool,
}

/// Connect the selected pair's HFP control link without opening an audio pin.
/// The caller must check its current playback session and audio intent in `current`.
pub fn connect_control(
    address: u64,
    mut current: impl FnMut() -> io::Result<()>,
) -> io::Result<ControlStatus> {
    current()?;
    if address == 0 || address > 0xffff_ffff_ffff {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid Bluetooth address",
        ));
    }
    let deadline = Instant::now() + CONTROL_BUDGET;
    let permit = ControlPermit::acquire()?;
    let candidate = control_candidate(address, &mut current)?;
    current()?;
    verify_control_candidate(&candidate, address)?;
    let handle = unsafe {
        CreateFileW(
            crate::wide(&candidate.path).as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let file = Arc::new(ControlFile {
        handle: ControlHandle(handle),
        _permit: permit,
    });
    let mut property = |request: &[u8], size| file.property(request, size, deadline, &mut current);
    let support = property(&ks_property(BT_AUDIO, 0, PROPERTY_SUPPORT, None), 4)?;
    if dword(&support)? & PROPERTY_GET == 0 {
        return Err(io::Error::from_raw_os_error(ERROR_NOT_SUPPORTED as i32));
    }
    let pins = dword(&property(&ks_property(PIN, 1, PROPERTY_GET, None), 4)?)?;
    if pins == 0 || pins > 16 {
        return Err(invalid_control_data("Invalid hands-free pin count"));
    }
    if jacks_ready(pins, &mut property)? {
        current()?;
        return Ok(ControlStatus {
            request_acknowledged: false,
            jack_ready: true,
        });
    }
    // The documented GET requests only a control connection, never SCO audio.
    current()?;
    verify_control_candidate(&candidate, address)?;
    file.property(
        &ks_property(BT_AUDIO, 0, PROPERTY_GET, None),
        0,
        deadline,
        &mut current,
    )?;
    loop {
        current()?;
        if Instant::now() >= deadline {
            return Ok(ControlStatus {
                request_acknowledged: true,
                jack_ready: false,
            });
        }
        let ready = match jacks_ready(pins, &mut |request, size| {
            file.property(request, size, deadline, &mut current)
        }) {
            Ok(ready) => ready,
            Err(error) if error.kind() == io::ErrorKind::TimedOut => {
                current()?;
                return Ok(ControlStatus {
                    request_acknowledged: true,
                    jack_ready: false,
                });
            }
            Err(error) => return Err(error),
        };
        if ready {
            current()?;
            return Ok(ControlStatus {
                request_acknowledged: true,
                jack_ready: true,
            });
        }
        std::thread::sleep(
            Duration::from_millis(20).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}

struct ControlCandidate {
    path: String,
    instance: String,
}

fn control_candidate(
    address: u64,
    current: &mut impl FnMut() -> io::Result<()>,
) -> io::Result<ControlCandidate> {
    let service = crate::wide("BthHFAud");
    let flags = CM_GETIDLIST_FILTER_PRESENT | CM_GETIDLIST_FILTER_SERVICE;
    let mut size = 0;
    if unsafe { CM_Get_Device_ID_List_SizeW(&mut size, service.as_ptr(), flags) } != CR_SUCCESS {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Hands-free devices unavailable",
        ));
    }
    let mut devices = bounded_control_list(size)?;
    if unsafe { CM_Get_Device_ID_ListW(service.as_ptr(), devices.as_mut_ptr(), size, flags) }
        != CR_SUCCESS
    {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Hands-free devices changed",
        ));
    }
    let mut candidates = Vec::new();
    for instance in control_list(&devices)? {
        current()?;
        if handsfree_owner(&instance) != Some(address) {
            continue;
        }
        let mut candidate = ControlCandidate {
            path: String::new(),
            instance,
        };
        verify_control_candidate(&candidate, address)?;
        let id = crate::wide(&candidate.instance);
        size = 0;
        if unsafe {
            CM_Get_Device_Interface_List_SizeW(
                &mut size,
                &AUDIO_CATEGORY,
                id.as_ptr(),
                CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
            )
        } != CR_SUCCESS
        {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Hands-free interface unavailable",
            ));
        }
        let mut interfaces = bounded_control_list(size)?;
        if unsafe {
            CM_Get_Device_Interface_ListW(
                &AUDIO_CATEGORY,
                id.as_ptr(),
                interfaces.as_mut_ptr(),
                size,
                CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
            )
        } != CR_SUCCESS
        {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "Hands-free interfaces changed",
            ));
        }
        for path in control_list(&interfaces)? {
            if !valid_id(&path) {
                return Err(invalid_control_data("Invalid hands-free interface"));
            }
            candidate.path = path;
            candidates.push(ControlCandidate {
                path: candidate.path.clone(),
                instance: candidate.instance.clone(),
            });
        }
    }
    only_control_candidate(candidates)
}

fn only_control_candidate(mut candidates: Vec<ControlCandidate>) -> io::Result<ControlCandidate> {
    if candidates.len() != 1 {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "Expected one selected hands-free interface",
        ));
    }
    Ok(candidates.remove(0))
}

fn verify_control_candidate(candidate: &ControlCandidate, address: u64) -> io::Result<()> {
    let mut node = 0;
    let (mut status, mut problem) = (0, 0);
    if handsfree_owner(&candidate.instance) != Some(address)
        || unsafe { CM_Locate_DevNodeW(&mut node, crate::wide(&candidate.instance).as_ptr(), 0) }
            != CR_SUCCESS
        || unsafe { CM_Get_DevNode_Status(&mut status, &mut problem, node, 0) } != CR_SUCCESS
        || problem != 0
        || status & DN_STARTED == 0
    {
        return Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "Selected hands-free driver is not ready",
        ));
    }
    Ok(())
}

fn bounded_control_list(size: u32) -> io::Result<Vec<u16>> {
    if !(1..=65536).contains(&size) {
        return Err(invalid_control_data("Invalid device list size"));
    }
    Ok(vec![0; size as usize])
}

fn control_list(mut buffer: &[u16]) -> io::Result<Vec<String>> {
    let mut result = Vec::new();
    loop {
        let end = buffer
            .iter()
            .position(|v| *v == 0)
            .ok_or_else(|| invalid_control_data("Unterminated device list"))?;
        if end == 0 {
            return Ok(result);
        }
        result.push(
            String::from_utf16(&buffer[..end])
                .map_err(|_| invalid_control_data("Invalid device string"))?,
        );
        buffer = &buffer[end + 1..];
    }
}

fn invalid_control_data(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn dword(bytes: &[u8]) -> io::Result<u32> {
    bytes
        .try_into()
        .map(u32::from_le_bytes)
        .map_err(|_| invalid_control_data("Invalid hands-free property length"))
}

fn ks_property(set: GUID, id: u32, flags: u32, pin: Option<u32>) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(if pin.is_some() { 32 } else { 24 });
    bytes.extend_from_slice(&set.data1.to_le_bytes());
    bytes.extend_from_slice(&set.data2.to_le_bytes());
    bytes.extend_from_slice(&set.data3.to_le_bytes());
    bytes.extend_from_slice(&set.data4);
    bytes.extend_from_slice(&id.to_le_bytes());
    bytes.extend_from_slice(&flags.to_le_bytes());
    if let Some(pin) = pin {
        bytes.extend_from_slice(&pin.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
    }
    bytes
}

fn jacks_ready(
    pins: u32,
    property: &mut impl FnMut(&[u8], usize) -> io::Result<Vec<u8>>,
) -> io::Result<bool> {
    let mut seen = false;
    let mut ready = true;
    for pin in 0..pins {
        match property(&ks_property(JACK, 1, PROPERTY_GET, Some(pin)), 4096) {
            Ok(bytes) => {
                let connected = jack_descriptions(&bytes)?;
                seen |= !connected.is_empty();
                ready &= connected.iter().all(|v| *v);
            }
            Err(error) if matches!(error.raw_os_error(), Some(1 | 50 | 1168)) => (),
            Err(error) => return Err(error),
        }
    }
    Ok(seen && ready)
}

fn jack_descriptions(bytes: &[u8]) -> io::Result<Vec<bool>> {
    if bytes.len() < 8 {
        return Err(invalid_control_data("Truncated hands-free jack header"));
    }
    let size = dword(&bytes[..4])? as usize;
    let count = dword(&bytes[4..8])? as usize;
    if size < 8 || size > bytes.len() || count > 16 || 8 + count * 28 > size {
        return Err(invalid_control_data("Invalid hands-free jack descriptions"));
    }
    (0..count)
        .map(|n| dword(&bytes[8 + n * 28 + 24..8 + n * 28 + 28]).map(|v| v != 0))
        .collect()
}

struct ControlPermit;
impl ControlPermit {
    fn acquire() -> io::Result<Self> {
        CONTROL_BUSY
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Hands-free control cleanup is pending",
                )
            })
    }
}
impl Drop for ControlPermit {
    fn drop(&mut self) {
        CONTROL_BUSY.store(false, Ordering::Release);
    }
}

struct ControlHandle(HANDLE);
impl Drop for ControlHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
struct ControlFile {
    handle: ControlHandle,
    _permit: ControlPermit,
}

struct ControlIo {
    overlapped: OVERLAPPED,
    input: Vec<u8>,
    output: Vec<u8>,
    _event: ControlHandle,
    file: Arc<ControlFile>,
    pending: bool,
}

impl ControlFile {
    fn property(
        self: &Arc<Self>,
        input: &[u8],
        size: usize,
        deadline: Instant,
        current: &mut impl FnMut() -> io::Result<()>,
    ) -> io::Result<Vec<u8>> {
        current()?;
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        let event = ControlHandle(unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) });
        if event.0.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut operation = Box::new(ControlIo {
            overlapped: unsafe { std::mem::zeroed() },
            input: input.to_vec(),
            output: vec![0; size],
            _event: event,
            file: Arc::clone(self),
            pending: false,
        });
        operation.overlapped.hEvent = operation._event.0;
        current()?;
        let mut returned = 0;
        if unsafe {
            DeviceIoControl(
                self.handle.0,
                KS_PROPERTY,
                operation.input.as_ptr().cast(),
                input.len() as u32,
                if size == 0 {
                    ptr::null_mut()
                } else {
                    operation.output.as_mut_ptr().cast()
                },
                size as u32,
                &mut returned,
                &mut operation.overlapped,
            )
        } == 0
        {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
                return Err(error);
            }
            operation.pending = true;
            (operation, returned) = operation.complete(deadline, current)?;
        }
        current()?;
        if returned as usize > size {
            return Err(invalid_control_data("Oversized hands-free property"));
        }
        operation.output.truncate(returned as usize);
        Ok(std::mem::take(&mut operation.output))
    }
}

impl ControlIo {
    fn complete(
        mut self: Box<Self>,
        deadline: Instant,
        current: &mut impl FnMut() -> io::Result<()>,
    ) -> io::Result<(Box<Self>, u32)> {
        loop {
            let check = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(&mut *current))
            {
                Ok(result) => result,
                Err(panic) => {
                    self.cancel();
                    std::panic::resume_unwind(panic);
                }
            }
            .and_then(|()| {
                if Instant::now() < deadline {
                    Ok(())
                } else {
                    Err(io::ErrorKind::TimedOut.into())
                }
            });
            if let Err(error) = check {
                self.cancel();
                return Err(error);
            }
            let remaining = deadline
                .saturating_duration_since(Instant::now())
                .as_millis()
                .clamp(1, 20) as u32;
            match unsafe { WaitForSingleObject(self._event.0, remaining) } {
                WAIT_OBJECT_0 => break,
                WAIT_TIMEOUT => (),
                _ => {
                    let error = io::Error::last_os_error();
                    self.cancel();
                    return Err(error);
                }
            }
        }
        let mut returned = 0;
        let result =
            unsafe { GetOverlappedResult(self.file.handle.0, &self.overlapped, &mut returned, 0) };
        // The event is signaled only after completion, including failed requests.
        self.pending = false;
        if result == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((self, returned))
    }

    fn cancel(mut self: Box<Self>) {
        unsafe {
            CancelIoEx(self.file.handle.0, &self.overlapped);
        }
        if unsafe { WaitForSingleObject(self._event.0, 100) } == WAIT_OBJECT_0 {
            self.pending = false;
            return;
        }
        // A faulty driver may ignore cancellation. Retain kernel-owned buffers and
        // the global permit until completion; never accumulate more pending calls.
        let pending = Box::into_raw(self) as usize;
        let _ = std::thread::Builder::new()
            .name("hfp-control-drain".into())
            .spawn(move || unsafe {
                let mut operation = Box::from_raw(pending as *mut ControlIo);
                let mut returned = 0;
                GetOverlappedResult(
                    operation.file.handle.0,
                    &operation.overlapped,
                    &mut returned,
                    1,
                );
                operation.pending = false;
            });
        // If the drain thread cannot start, keeping this one allocation until exit
        // is safer than freeing memory that Windows can still write to.
    }
}

impl Drop for ControlIo {
    fn drop(&mut self) {
        if self.pending {
            // Panic unwinding must also keep kernel-owned memory alive until drained.
            unsafe {
                CancelIoEx(self.file.handle.0, &self.overlapped);
                GetOverlappedResult(self.file.handle.0, &self.overlapped, &mut 0, 1);
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub id: String,
    pub adapter: String,
    pub state: u32,
}

pub struct Endpoints {
    enumerator: IMMDeviceEnumerator,
    policy: PolicyConfig,
    // COM interfaces must be released before leaving this thread's apartment.
    _apartment: Apartment,
}

struct Apartment;

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

impl Endpoints {
    pub fn new() -> Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
            let apartment = Apartment;
            Ok(Self {
                enumerator: CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?,
                policy: CoCreateInstance(&POLICY_CONFIG_CLIENT, None, CLSCTX_ALL)?,
                _apartment: apartment,
            })
        }
    }

    pub fn list(&self, address: u64) -> Result<Vec<Endpoint>> {
        if address == 0 || address > 0xffff_ffff_ffff {
            return Err(Error::new(E_INVALIDARG, "Invalid Bluetooth address"));
        }
        unsafe {
            let devices = self
                .enumerator
                .EnumAudioEndpoints(eAll, DEVICE_STATE(DEVICE_STATEMASK_ALL))?;
            let mut endpoints = Vec::new();
            for index in 0..devices.GetCount()? {
                // Unplugging during enumeration must not select a different endpoint.
                let Ok(device) = devices.Item(index) else {
                    continue;
                };
                let Ok(adapter) = self.adapter_instance(&device) else {
                    continue;
                };
                if handsfree_owner(&adapter) != Some(address) {
                    continue;
                }
                let (Ok(id), Ok(state)) = (endpoint_id(&device), device.GetState()) else {
                    continue;
                };
                endpoints.push(Endpoint {
                    id,
                    adapter,
                    state: state.0,
                });
            }
            Ok(endpoints)
        }
    }

    /// A live selected-pair HFP session may be a call; never interrupt it for a handoff.
    pub fn busy(&self, address: u64) -> Result<bool> {
        if address == 0 || address > 0xffff_ffff_ffff {
            return Err(Error::new(E_INVALIDARG, "Invalid Bluetooth address"));
        }
        unsafe {
            let devices = self.enumerator.EnumAudioEndpoints(eAll, DEVICE_STATE_ACTIVE)?;
            for index in 0..devices.GetCount()? {
                let device = devices.Item(index)?;
                let adapter = self.adapter_instance(&device)?;
                let owner = handsfree_owner(&adapter);
                if owner.is_none() && adapter.to_ascii_lowercase().starts_with("bthhfenum\\bthhfpaudio\\") {
                    return Err(Error::new(E_CHANGED_STATE, "Could not identify an active hands-free endpoint"));
                }
                if owner != Some(address) {
                    continue;
                }
                let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
                let sessions = manager.GetSessionEnumerator()?;
                for index in 0..sessions.GetCount()? {
                    if sessions.GetSession(index)?.GetState()? == AudioSessionStateActive {
                        return Ok(true);
                    }
                }
            }
            Ok(false)
        }
    }

    pub fn state(&self, id: &str) -> Result<Option<u32>> {
        if !valid_id(id) {
            return Err(Error::new(E_INVALIDARG, "Invalid audio endpoint ID"));
        }
        unsafe {
            match self.enumerator.GetDevice(PCWSTR(crate::wide(id).as_ptr())) {
                Ok(device) => device.GetState().map(|state| Some(state.0)),
                Err(error) if error.code() == HRESULT::from_win32(ERROR_NOT_FOUND.0) => Ok(None),
                Err(error) => Err(error),
            }
        }
    }

    pub fn replacement_ready(&self, address: u64) -> Result<bool> {
        if address == 0 || address > 0xffff_ffff_ffff {
            return Err(Error::new(E_INVALIDARG, "Invalid Bluetooth address"));
        }
        unsafe {
            for flow in [eRender, eCapture] {
                let devices = self
                    .enumerator
                    .EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)?;
                let mut found = false;
                for index in 0..devices.GetCount()? {
                    let Ok(device) = devices.Item(index) else {
                        continue;
                    };
                    if device.GetState()? != DEVICE_STATE_ACTIVE {
                        continue;
                    }
                    let Ok(adapter) = self.adapter_instance(&device) else {
                        continue;
                    };
                    found = if flow == eRender {
                        crate::driver::native_endpoint_owner(&adapter, address)
                    } else {
                        hq_microphone(&adapter)
                    };
                    if found {
                        break;
                    }
                }
                if !found {
                    return Ok(false);
                }
            }
            Ok(true)
        }
    }

    pub fn set_visible(&self, endpoint: &Endpoint, visible: bool) -> Result<()> {
        if !valid_id(&endpoint.id) || !valid_id(&endpoint.adapter) {
            return Err(Error::new(E_INVALIDARG, "Invalid hands-free endpoint"));
        }
        unsafe {
            let id = crate::wide(&endpoint.id);
            let device = self.enumerator.GetDevice(PCWSTR(id.as_ptr()))?;
            let adapter = self.adapter_instance(&device)?;
            if !adapter.eq_ignore_ascii_case(&endpoint.adapter)
                || handsfree_owner(&adapter).is_none()
            {
                return Err(Error::new(
                    E_CHANGED_STATE,
                    "The hands-free endpoint's adapter changed",
                ));
            }
            if !visibility_change(endpoint.state, device.GetState()?.0, visible)? {
                return Ok(());
            }
            (self.policy.vtable().set_endpoint_visibility)(
                self.policy.as_raw(),
                PCWSTR(id.as_ptr()),
                i32::from(visible),
            )
            .ok()
            .and_then(|()| {
                if (device.GetState()?.0 & DEVICE_STATE_DISABLED.0 == 0) == visible {
                    Ok(())
                } else {
                    Err(Error::new(
                        E_FAIL,
                        "Windows did not confirm hands-free visibility",
                    ))
                }
            })
            .map_err(|error| {
                // E_CHANGED_STATE is reserved for the checks before our write.
                if error.code() == E_CHANGED_STATE {
                    Error::new(E_FAIL, "Windows could not change hands-free visibility")
                } else {
                    error
                }
            })
        }
    }

    fn adapter_instance(&self, endpoint: &IMMDevice) -> Result<String> {
        unsafe {
            // The endpoint store can omit InstanceId; its connected adapter has it.
            let topology: IDeviceTopology = endpoint.Activate(CLSCTX_ALL, None)?;
            let adapter_id = topology.GetConnector(0)?.GetDeviceIdConnectedTo()?;
            let adapter = self.enumerator.GetDevice(adapter_id);
            CoTaskMemFree(Some(adapter_id.0.cast()));
            let properties = adapter?.OpenPropertyStore(STGM_READ)?;
            let instance = properties.GetValue(&PKEY_Device_InstanceId)?;
            Ok(BSTR::try_from(&instance)?.to_string())
        }
    }
}

fn endpoint_id(endpoint: &IMMDevice) -> Result<String> {
    unsafe {
        let id = endpoint.GetId()?;
        let result = id
            .to_string()
            .map_err(|_| Error::new(E_FAIL, "Invalid audio endpoint ID"));
        CoTaskMemFree(Some(id.0.cast()));
        result
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 1024 && !id.chars().any(char::is_control)
}

fn visibility_change(previous: u32, current: u32, visible: bool) -> Result<bool> {
    let disabled = current & DEVICE_STATE_DISABLED.0 != 0;
    if !visible && (disabled || previous & DEVICE_STATE_DISABLED.0 != 0) {
        return Err(Error::new(
            E_CHANGED_STATE,
            "The hands-free endpoint was already disabled",
        ));
    }
    Ok(!visible || disabled)
}

fn handsfree_owner(adapter: &str) -> Option<u64> {
    if !valid_id(adapter)
        || !adapter
            .to_ascii_lowercase()
            .starts_with("bthhfenum\\bthhfpaudio\\")
    {
        return None;
    }
    unsafe {
        let mut node = 0;
        if CM_Locate_DevNodeW(
            &mut node,
            crate::wide(adapter).as_ptr(),
            CM_LOCATE_DEVNODE_PHANTOM,
        ) != CR_SUCCESS
        {
            return None;
        }
        let mut service = [0u16; 128];
        let mut bytes = std::mem::size_of_val(&service) as u32;
        if CM_Get_DevNode_Registry_PropertyW(
            node,
            CM_DRP_SERVICE,
            ptr::null_mut(),
            service.as_mut_ptr().cast(),
            &mut bytes,
            0,
        ) != CR_SUCCESS
        {
            return None;
        }
        let end = service.iter().position(|value| *value == 0)?;
        if !handsfree_service(&String::from_utf16_lossy(&service[..end])) {
            return None;
        }
        let mut parent = 0;
        if CM_Get_Parent(&mut parent, node, 0) != CR_SUCCESS || parent == node {
            return None;
        }
        let mut id = [0u16; 512];
        if CM_Get_Device_IDW(parent, id.as_mut_ptr(), id.len() as u32, 0) != CR_SUCCESS {
            return None;
        }
        let end = id.iter().position(|value| *value == 0)?;
        handsfree_address(&String::from_utf16_lossy(&id[..end]))
    }
}

fn handsfree_service(service: &str) -> bool {
    service.eq_ignore_ascii_case("BthHFAud")
}

fn hq_microphone(adapter: &str) -> bool {
    if !valid_id(adapter) || !adapter.to_ascii_lowercase().starts_with("root\\") {
        return false;
    }
    unsafe {
        let mut node = 0;
        if CM_Locate_DevNodeW(&mut node, crate::wide(adapter).as_ptr(), 0) != CR_SUCCESS {
            return false;
        }
        let property = |key| -> Option<Vec<String>> {
            let mut buffer = [0u16; 512];
            let mut bytes = std::mem::size_of_val(&buffer) as u32;
            if CM_Get_DevNode_Registry_PropertyW(
                node,
                key,
                ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut bytes,
                0,
            ) != CR_SUCCESS
                || bytes as usize > std::mem::size_of_val(&buffer)
                || bytes % 2 != 0
            {
                return None;
            }
            Some(
                buffer[..bytes as usize / 2]
                    .split(|value| *value == 0)
                    .take_while(|value| !value.is_empty())
                    .map(String::from_utf16_lossy)
                    .collect(),
            )
        };
        let Some(service) = property(CM_DRP_SERVICE) else {
            return false;
        };
        let Some(hardware_ids) = property(CM_DRP_HARDWAREID) else {
            return false;
        };
        hq_microphone_identity(&service, &hardware_ids)
    }
}

fn hq_microphone_identity(service: &[String], hardware_ids: &[String]) -> bool {
    service.len() == 1
        && service[0].eq_ignore_ascii_case("NTPodsMicACX")
        && hardware_ids
            .iter()
            .any(|id| id.eq_ignore_ascii_case("ROOT\\NTPodsMicACX"))
}

fn handsfree_address(instance: &str) -> Option<u64> {
    if !valid_id(instance) {
        return None;
    }
    let instance = instance.to_ascii_lowercase();
    let rest = instance
        .strip_prefix("bthenum\\{0000111e-0000-1000-8000-00805f9b34fb}_vid&0001004c_pid&")?;
    let (model, suffix) = rest.split_once('\\')?;
    if !matches!(
        model,
        "2002"
            | "200a"
            | "200e"
            | "200f"
            | "2013"
            | "2014"
            | "2019"
            | "201b"
            | "201f"
            | "2024"
            | "2027"
    ) || !suffix
        .bytes()
        .all(|b| b.is_ascii_hexdigit() || b == b'&' || b == b'_')
    {
        return None;
    }
    let (prefix, tail) = suffix.rsplit_once('&')?;
    let (address, channel) = tail.split_once("_c")?;
    if prefix.is_empty()
        || address.len() != 12
        || !address.bytes().all(|b| b.is_ascii_hexdigit())
        || channel.len() != 8
        || !channel.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    u64::from_str_radix(address, 16)
        .ok()
        .filter(|address| *address != 0)
}

const POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

#[repr(transparent)]
#[derive(Clone)]
struct PolicyConfig(IUnknown);

unsafe impl Interface for PolicyConfig {
    type Vtable = PolicyConfigVtable;
    const IID: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);
}

// Private Windows interface: if activation fails, callers leave endpoints visible.
#[repr(C)]
struct PolicyConfigVtable {
    unknown: IUnknown_Vtbl,
    // GetMixFormat through SetDefaultEndpoint precede SetEndpointVisibility.
    unused: [usize; 11],
    set_endpoint_visibility: unsafe extern "system" fn(*mut c_void, PCWSTR, i32) -> HRESULT,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::Storage::FileSystem::{ReadFile, PIPE_ACCESS_DUPLEX};
    use windows_sys::Win32::System::Pipes::{CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS};

    static CONTROL_TEST: Mutex<()> = Mutex::new(());

    fn jack_packet(connected: &[bool]) -> Vec<u8> {
        let mut bytes = vec![0; 8 + connected.len() * 28];
        let size = bytes.len() as u32;
        bytes[..4].copy_from_slice(&size.to_le_bytes());
        bytes[4..8].copy_from_slice(&(connected.len() as u32).to_le_bytes());
        for (n, value) in connected.iter().enumerate() {
            bytes[32 + n * 28..36 + n * 28].copy_from_slice(&u32::from(*value).to_le_bytes());
        }
        bytes
    }

    #[test]
    fn control_property_uses_documented_get_without_sco_or_sets() {
        let request = ks_property(BT_AUDIO, 0, PROPERTY_GET, None);
        assert_eq!(request.len(), 24);
        assert_eq!(
            &request[..16],
            &[
                0x40, 0x6c, 0xa0, 0x7f, 0xf6, 0xb8, 0x7e, 0x4c, 0x85, 0x56, 0xe8, 0xc3, 0x3a, 0x12,
                0xe5, 0x4d
            ]
        );
        assert_eq!(dword(&request[16..20]).unwrap(), 0);
        assert_eq!(dword(&request[20..24]).unwrap(), PROPERTY_GET);
        let jack = ks_property(JACK, 1, PROPERTY_GET, Some(2));
        assert_eq!(jack.len(), 32);
        assert_eq!(dword(&jack[24..28]).unwrap(), 2);
        assert_eq!(dword(&jack[28..]).unwrap(), 0);
    }

    #[test]
    fn jack_readiness_requires_actual_nonempty_reports_from_every_supported_pin() {
        for (left, right, expected) in [
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (true, true, true),
        ] {
            assert_eq!(
                jacks_ready(4, &mut |request, size| {
                    assert_eq!(size, 4096);
                    match dword(&request[24..28])? {
                        1 => Ok(jack_packet(&[left])),
                        2 => Ok(jack_packet(&[right])),
                        _ => Err(io::Error::from_raw_os_error(1168)),
                    }
                })
                .unwrap(),
                expected
            );
        }
        assert!(!jacks_ready(4, &mut |_, _| Err(io::Error::from_raw_os_error(1168))).unwrap());
        assert!(!jacks_ready(1, &mut |_, _| Ok(jack_packet(&[]))).unwrap());
        assert_eq!(
            jacks_ready(1, &mut |_, _| Err(io::ErrorKind::PermissionDenied.into()))
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
    }

    #[test]
    fn malformed_jacks_never_imply_connected() {
        assert_eq!(
            jack_descriptions(&jack_packet(&[true, false])).unwrap(),
            [true, false]
        );
        let valid = jack_packet(&[true]);
        for truncated in 0..valid.len() {
            assert!(jack_descriptions(&valid[..truncated]).is_err());
        }
        for (offset, value) in [(0, 7u32), (0, 37), (4, 17), (4, u32::MAX)] {
            let mut invalid = valid.clone();
            invalid[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(jack_descriptions(&invalid).is_err());
        }
        assert!(dword(&[0; 3]).is_err());
        assert!(dword(&[0; 5]).is_err());
    }

    #[test]
    fn control_enumeration_rejects_malformed_or_ambiguous_targets() {
        assert!(bounded_control_list(0).is_err());
        assert!(bounded_control_list(65537).is_err());
        assert!(control_list(&[b'x' as u16]).is_err());
        assert!(control_list(&[b'x' as u16, 0]).is_err());
        assert!(control_list(&[0xd800, 0, 0]).is_err());
        assert_eq!(
            control_list(&[b'x' as u16, 0, b'y' as u16, 0, 0]).unwrap(),
            ["x", "y"]
        );
        let candidate = || ControlCandidate {
            path: "selected".into(),
            instance: instance("2027", "001122334455"),
        };
        assert!(only_control_candidate(vec![]).is_err());
        assert!(only_control_candidate(vec![candidate(), candidate()]).is_err());
        assert_eq!(
            only_control_candidate(vec![candidate()]).unwrap().path,
            "selected"
        );
        assert!(handsfree_service("bthhfaud"));
        for wrong in ["BthHFAudExtra", "NTPodsAudio", "BthHFEnum", ""] {
            assert!(!handsfree_service(wrong));
        }
    }

    #[test]
    fn stale_intent_stops_before_enumeration_and_before_ioctl() {
        assert_eq!(
            connect_control(0x0011_2233_4455, || Err(
                io::ErrorKind::ConnectionReset.into()
            ))
            .unwrap_err()
            .kind(),
            io::ErrorKind::ConnectionReset
        );
        let _serial = CONTROL_TEST.lock().unwrap();
        let file = Arc::new(ControlFile {
            handle: ControlHandle(INVALID_HANDLE_VALUE),
            _permit: ControlPermit::acquire().unwrap(),
        });
        let mut checks = 0;
        let error = file
            .property(&[], 0, Instant::now() + CONTROL_BUDGET, &mut || {
                checks += 1;
                if checks == 2 {
                    Err(io::ErrorKind::Interrupted.into())
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(checks, 2);
        assert!(ControlPermit::acquire().is_err());
        drop(file);
        assert!(ControlPermit::acquire().is_ok());
    }

    // Local pipes exercise the same OVERLAPPED ownership without touching Bluetooth.
    fn pending_control_read() -> (Arc<ControlFile>, ControlHandle, Box<ControlIo>) {
        unsafe {
            let name = crate::wide(&format!(
                r"\\.\pipe\ntpods-hfp-cancel-{}",
                std::process::id()
            ));
            let server = ControlHandle(CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
                PIPE_REJECT_REMOTE_CLIENTS,
                1,
                64,
                64,
                0,
                ptr::null(),
            ));
            assert_ne!(server.0, INVALID_HANDLE_VALUE);
            let client = ControlHandle(CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                ptr::null(),
                OPEN_EXISTING,
                0,
                ptr::null_mut(),
            ));
            assert_ne!(client.0, INVALID_HANDLE_VALUE);
            let file = Arc::new(ControlFile {
                handle: server,
                _permit: ControlPermit::acquire().unwrap(),
            });
            let event = ControlHandle(CreateEventW(ptr::null(), 1, 0, ptr::null()));
            assert!(!event.0.is_null());
            let mut operation = Box::new(ControlIo {
                overlapped: std::mem::zeroed(),
                input: vec![],
                output: vec![0; 8],
                _event: event,
                file: Arc::clone(&file),
                pending: false,
            });
            operation.overlapped.hEvent = operation._event.0;
            assert_eq!(
                ReadFile(
                    file.handle.0,
                    operation.output.as_mut_ptr(),
                    8,
                    ptr::null_mut(),
                    &mut operation.overlapped
                ),
                0
            );
            assert_eq!(GetLastError(), ERROR_IO_PENDING);
            operation.pending = true;
            (file, client, operation)
        }
    }

    #[test]
    fn cancelled_and_timed_out_io_drains_and_releases_its_permit() {
        let _serial = CONTROL_TEST.lock().unwrap();
        for cancelled in [true, false] {
            let (file, client, operation) = pending_control_read();
            let mut checks = 0;
            let started = Instant::now();
            let result = operation.complete(started + Duration::from_millis(30), &mut || {
                checks += 1;
                if cancelled && checks >= 2 {
                    Err(io::ErrorKind::Interrupted.into())
                } else {
                    Ok(())
                }
            });
            let error = match result {
                Err(error) => error,
                Ok(_) => panic!("Pending read unexpectedly completed"),
            };
            assert_eq!(
                error.kind(),
                if cancelled {
                    io::ErrorKind::Interrupted
                } else {
                    io::ErrorKind::TimedOut
                }
            );
            assert!(started.elapsed() < Duration::from_millis(500));
            // Completion already released its event and extra file reference.
            assert_eq!(Arc::strong_count(&file), 1);
            assert!(ControlPermit::acquire().is_err());
            drop((file, client));
            assert!(ControlPermit::acquire().is_ok());
        }
    }

    #[test]
    fn panicking_cancellation_guard_drains_kernel_owned_memory() {
        let _serial = CONTROL_TEST.lock().unwrap();
        let (file, client, operation) = pending_control_read();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = operation.complete(Instant::now() + CONTROL_BUDGET, &mut || {
                panic!("Cancellation guard panicked")
            });
        }));
        assert!(result.is_err());
        assert_eq!(Arc::strong_count(&file), 1);
        drop((file, client));
        assert!(ControlPermit::acquire().is_ok());
    }

    fn instance(model: &str, address: &str) -> String {
        format!(
            "BTHENUM\\{{0000111e-0000-1000-8000-00805f9b34fb}}_VID&0001004C_PID&{model}\\1&123&0&{address}_C00000000"
        )
    }

    #[test]
    fn matches_supported_handsfree_models_and_exact_pair() {
        for model in [
            "2002", "200A", "200E", "200F", "2013", "2014", "2019", "201B", "201F", "2024", "2027",
        ] {
            let id = instance(model, "001122334455");
            assert_eq!(handsfree_address(&id), Some(0x0011_2233_4455));
            assert_eq!(
                handsfree_address(&id.to_ascii_lowercase()),
                Some(0x0011_2233_4455)
            );
        }
        assert_ne!(
            handsfree_address(&instance("2027", "001122334455")),
            handsfree_address(&instance("2027", "001122334456"))
        );
    }

    #[test]
    fn rejects_other_profiles_manufacturers_and_unknown_models() {
        let id = instance("2027", "001122334455");
        for invalid in [
            id.replace("0000111e", "0000110b"),
            id.replace("0000111e", "74ec2172"),
            id.replace("0001004C", "0002054C"),
            instance("FFFF", "001122334455"),
            instance("2027extra", "001122334455"),
            "ROOT\\NTPodsMicACX\\0000".into(),
            "BTHHFENUM\\BTHHFPAUDIO\\1&123&0&97".into(),
        ] {
            assert_eq!(handsfree_address(&invalid), None, "{invalid}");
        }
    }

    #[test]
    fn rejects_malformed_instance_addresses() {
        let id = instance("200E", "001122334455");
        for invalid in [
            instance("200E", "000000000000"),
            instance("200E", "00112233445"),
            instance("200E", "0011223344556"),
            instance("200E", "00112233445g"),
            id.replace("_C00000000", "_C0000000"),
            id.replace("_C00000000", "_C0000000G"),
            id.replace("1&123&0&", ""),
            format!("{id}\\extra"),
            format!("{id}\0"),
            format!("{id} "),
        ] {
            assert_eq!(handsfree_address(&invalid), None, "{invalid:?}");
        }
    }

    #[test]
    fn visibility_uses_the_policy_config_abi_slot() {
        assert_eq!(
            std::mem::offset_of!(PolicyConfigVtable, set_endpoint_visibility),
            14 * std::mem::size_of::<usize>()
        );
    }

    #[test]
    fn replacement_microphone_requires_both_acx_service_and_exact_hardware_id() {
        let service = ["NTPodsMicACX".into()];
        let hardware = ["ROOT\\NTPodsMicACX".into()];
        assert!(hq_microphone_identity(&service, &hardware));
        assert!(hq_microphone_identity(
            &["ntpodsmicacx".into()],
            &["unrelated".into(), "root\\ntpodsmicacx".into()]
        ));
        for other_service in [vec![], vec!["NTPodsMic".into()], vec!["BthHFAud".into()]] {
            assert!(!hq_microphone_identity(&other_service, &hardware));
        }
        for other_hardware in [
            vec![],
            vec!["ROOT\\NTPodsMicACXExtra".into()],
            vec!["ROOT\\NTPodsMic".into()],
            vec!["BTHHFENUM\\BTHHFPAUDIO".into()],
        ] {
            assert!(!hq_microphone_identity(&service, &other_hardware));
        }
    }

    #[test]
    fn preserves_manual_disables_and_allows_repeated_restoration() {
        for state in [1, 4, 8] {
            assert!(visibility_change(state, state, false).unwrap());
            assert!(!visibility_change(state, state, true).unwrap());
            assert!(visibility_change(state, 2, true).unwrap());
            assert_eq!(
                visibility_change(state, 2, false).unwrap_err().code(),
                E_CHANGED_STATE
            );
            assert_eq!(
                visibility_change(2, state, false).unwrap_err().code(),
                E_CHANGED_STATE
            );
        }
    }
}
