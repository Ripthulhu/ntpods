//! Shared NTPodsAAP driver handle for the daemon's receive and command threads.

use std::ffi::c_void;
use std::io;
use std::ptr;
use std::sync::{Arc, Mutex, Weak};

use windows_sys::core::GUID;
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_Device_IDW, CM_Get_Parent, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces,
    SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW, CR_SUCCESS, DIGCF_DEVICEINTERFACE,
    DIGCF_PRESENT, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W, SP_DEVINFO_DATA,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_INSUFFICIENT_BUFFER, ERROR_IO_PENDING, ERROR_NO_MORE_ITEMS,
    HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Threading::CreateEventW;
use windows_sys::Win32::System::IO::{
    CancelIoEx, DeviceIoControl, GetOverlappedResult, GetOverlappedResultEx, OVERLAPPED,
};

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;

const GUID_DEVINTERFACE_NTPODSAAP: GUID = GUID {
    data1: 0xC0FF_EE00,
    data2: 0x1337,
    data3: 0x4A5B,
    data4: [0x9E, 0x6F, 0xA1, 0xB2, 0xC3, 0xD4, 0xE5, 0xF6],
};

const IOCTL_LP_CONNECT: u32 = 0x8000_2000;
const IOCTL_LP_DISCONNECT: u32 = 0x8000_2004;
const IOCTL_LP_SEND: u32 = 0x8000_2008;
const IOCTL_LP_RECEIVE: u32 = 0x8000_200C;
const IOCTL_LP_GET_STATUS: u32 = 0x8000_2010;
const IOCTL_LP_ATT_SEND: u32 = 0x8000_2014;
const IOCTL_LP_ATT_RECEIVE: u32 = 0x8000_2018;

struct WinHandle(HANDLE);
unsafe impl Send for WinHandle {}
unsafe impl Sync for WinHandle {}
impl Drop for WinHandle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

#[derive(Clone)]
pub struct Driver {
    handle: Arc<WinHandle>,
}

#[derive(Default)]
pub struct Shared(Mutex<Vec<(u64, Weak<WinHandle>)>>);

impl Shared {
    pub fn open(&self, address: u64) -> io::Result<Driver> {
        let mut handles = self.0.lock().unwrap();
        if crate::devnode::recovering() {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "The AirPods driver is recovering",
            ));
        }
        handles.retain(|(_, handle)| handle.strong_count() != 0);
        if let Some(handle) = handles
            .iter()
            .find_map(|(peer, handle)| (*peer == address).then(|| handle.upgrade()).flatten())
        {
            return Ok(Driver { handle });
        }
        let driver = Driver::open(address)?;
        crate::devnode::opened(address);
        // The exclusive AAP handle outlives individual control-channel reconnects.
        handles.push((address, Arc::downgrade(&driver.handle)));
        Ok(driver)
    }

    pub fn request_recovery(
        &self,
        address: u64,
        finished: impl FnOnce(crate::devnode::RecoveryResult) + Send + 'static,
    ) -> bool {
        // Serialize recovery with both control and playback opens; an owner must close first.
        let handles = self.0.lock().unwrap();
        if handles
            .iter()
            .any(|(peer, handle)| *peer == address && handle.strong_count() != 0)
        {
            return false;
        }
        crate::devnode::request_recovery(address, finished)
    }
}

pub fn native_endpoint_owner(instance: &str, address: u64) -> bool {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        CM_Get_DevNode_Registry_PropertyW, CM_Locate_DevNodeW, CM_DRP_SERVICE,
    };
    if address == 0 || instance.is_empty() || instance.contains('\0') {
        return false;
    }
    unsafe {
        let mut node = 0;
        if CM_Locate_DevNodeW(&mut node, crate::wide(instance).as_ptr(), 0) != CR_SUCCESS {
            return false;
        }
        for _ in 0..8 {
            let mut service = [0u16; 128];
            let mut bytes = std::mem::size_of_val(&service) as u32;
            if CM_Get_DevNode_Registry_PropertyW(
                node,
                CM_DRP_SERVICE,
                ptr::null_mut(),
                service.as_mut_ptr().cast(),
                &mut bytes,
                0,
            ) == CR_SUCCESS
            {
                let end = service
                    .iter()
                    .position(|value| *value == 0)
                    .unwrap_or(service.len());
                if String::from_utf16_lossy(&service[..end]).eq_ignore_ascii_case("NTPodsAudio") {
                    return owning_bluetooth_address(node) == Some(address);
                }
            }
            let mut parent = 0;
            if CM_Get_Parent(&mut parent, node, 0) != CR_SUCCESS || parent == node {
                break;
            }
            node = parent;
        }
    }
    false
}

#[cfg(feature = "experimental-playback")]
pub fn native_audio_bound(address: u64) -> bool {
    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
        SetupDiEnumDeviceInfo, SetupDiGetDeviceRegistryPropertyW, DIGCF_ALLCLASSES, SPDRP_SERVICE,
    };
    unsafe {
        let devices = SetupDiGetClassDevsW(
            ptr::null(),
            crate::wide("BTHENUM").as_ptr(),
            ptr::null_mut(),
            DIGCF_ALLCLASSES | DIGCF_PRESENT,
        );
        if devices == INVALID_HANDLE_VALUE as isize {
            return false;
        }
        let mut found = false;
        for index in 0..u32::MAX {
            let mut device: SP_DEVINFO_DATA = std::mem::zeroed();
            device.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as u32;
            if SetupDiEnumDeviceInfo(devices, index, &mut device) == 0 {
                break;
            }
            let mut service = [0u16; 128];
            let mut bytes = 0;
            if SetupDiGetDeviceRegistryPropertyW(
                devices,
                &device,
                SPDRP_SERVICE,
                ptr::null_mut(),
                service.as_mut_ptr().cast(),
                std::mem::size_of_val(&service) as u32,
                &mut bytes,
            ) == 0
            {
                continue;
            }
            let end = service
                .iter()
                .position(|value| *value == 0)
                .unwrap_or(service.len());
            if String::from_utf16_lossy(&service[..end]).eq_ignore_ascii_case("NTPodsAudio")
                && owning_bluetooth_address(device.DevInst) == Some(address)
            {
                found = true;
                break;
            }
        }
        SetupDiDestroyDeviceInfoList(devices);
        found
    }
}

fn ioctl(handle: HANDLE, code: u32, input: &[u8], output: &mut [u8]) -> io::Result<u32> {
    ioctl_with_timeout(handle, code, input, output, 20_000)
}

fn ioctl_with_timeout(
    handle: HANDLE,
    code: u32,
    input: &[u8],
    output: &mut [u8],
    timeout_ms: u32,
) -> io::Result<u32> {
    unsafe {
        let mut returned: u32 = 0;
        let event = WinHandle(CreateEventW(ptr::null(), 1, 0, ptr::null()));
        if event.0.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut overlapped: OVERLAPPED = std::mem::zeroed();
        overlapped.hEvent = event.0;
        let in_ptr = if input.is_empty() {
            ptr::null()
        } else {
            input.as_ptr() as *const c_void
        };
        let out_ptr = if output.is_empty() {
            ptr::null_mut()
        } else {
            output.as_mut_ptr() as *mut c_void
        };
        if DeviceIoControl(
            handle,
            code,
            in_ptr,
            input.len() as u32,
            out_ptr,
            output.len() as u32,
            &mut returned,
            &mut overlapped,
        ) == 0
        {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
                return Err(error);
            }
            // Includes queueing, lazy ATT open and channel cleanup. The driver has
            // shorter per-BRB deadlines; a stuck call must still drain on cancel.
            wait_for_completion(handle, &overlapped, &mut returned, timeout_ms)?;
        }
        if returned as usize > output.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Driver returned an oversized reply",
            ));
        }
        Ok(returned)
    }
}

// The caller owns the event, buffers and OVERLAPPED until this function returns.
unsafe fn wait_for_completion(
    handle: HANDLE,
    overlapped: &OVERLAPPED,
    returned: &mut u32,
    timeout_ms: u32,
) -> io::Result<()> {
    if GetOverlappedResultEx(handle, overlapped, returned, timeout_ms, 0) == 0 {
        let error = io::Error::last_os_error();
        CancelIoEx(handle, overlapped);
        // Cancellation is a request: drain it before freeing any referenced memory.
        GetOverlappedResult(handle, overlapped, returned, 1);
        return Err(error);
    }
    Ok(())
}

fn require_reply(actual: u32, minimum: u32) -> io::Result<()> {
    if actual < minimum {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Truncated driver reply",
        ))
    } else {
        Ok(())
    }
}

#[cfg(feature = "experimental-playback")]
fn playback_error(error: io::Error) -> io::Error {
    use windows_sys::Win32::Foundation::{ERROR_CONNECTION_ABORTED, ERROR_DEVICE_NOT_CONNECTED};

    // Rust leaves these driver statuses unclassified; ownership loss must not retry.
    match error.raw_os_error().map(|code| code as u32) {
        Some(ERROR_DEVICE_NOT_CONNECTED) => io::Error::new(io::ErrorKind::NotConnected, error),
        Some(ERROR_CONNECTION_ABORTED) => io::Error::new(io::ErrorKind::ConnectionAborted, error),
        _ => error,
    }
}

impl Driver {
    #[cfg(feature = "experimental-playback")]
    pub(crate) fn playback_ioctl(
        &self,
        function: u32,
        input: &[u8],
        output: &mut [u8],
    ) -> io::Result<usize> {
        if !(0x810..=0x815).contains(&function) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid playback operation",
            ));
        }
        let timeout = match function {
            0x810 | 0x811 => 20_000,
            0x812 => 3_000,
            0x813 if input.len() == 12 => {
                u32::from_le_bytes(input[8..12].try_into().unwrap()).min(10_000) + 1_000
            }
            0x815 if input.len() == 20 && input[4..8] != [0; 4] => 20_000,
            _ => 2_000,
        };
        Ok(ioctl_with_timeout(
            self.handle.0,
            0x8000_c000 | (function << 2),
            input,
            output,
            timeout,
        )
        .map_err(playback_error)? as usize)
    }

    pub fn open(mac: u64) -> io::Result<Driver> {
        let handle = open_driver(mac)?;
        Ok(Driver {
            handle: Arc::new(WinHandle(handle)),
        })
    }

    pub fn connect(&self, addr: u64, psm: u16) -> io::Result<bool> {
        let mut input = [0u8; 10];
        input[0..8].copy_from_slice(&addr.to_le_bytes());
        input[8..10].copy_from_slice(&psm.to_le_bytes());
        let mut out = [0u8; 8];
        require_reply(ioctl(self.handle.0, IOCTL_LP_CONNECT, &input, &mut out)?, 8)?;
        Ok(u32::from_le_bytes([out[0], out[1], out[2], out[3]]) != 0)
    }

    pub fn send(&self, data: &[u8]) -> io::Result<()> {
        ioctl(self.handle.0, IOCTL_LP_SEND, data, &mut [])?;
        Ok(())
    }

    pub fn send_sensor(&self, data: &[u8]) -> io::Result<()> {
        // Optional sensors must leave time for microphone cleanup during shutdown.
        ioctl_with_timeout(self.handle.0, IOCTL_LP_SEND, data, &mut [], 2_000)?;
        Ok(())
    }

    #[cfg(feature = "experimental-playback")]
    pub(crate) fn send_playback_control(&self, data: &[u8]) -> io::Result<()> {
        // Leave time to restore a trial before the microphone's transport-reset deadline.
        ioctl_with_timeout(self.handle.0, IOCTL_LP_SEND, data, &mut [], 1_000)?;
        Ok(())
    }

    pub fn recv(&self, timeout_ms: u32, buf: &mut [u8]) -> io::Result<usize> {
        let to = timeout_ms.to_le_bytes();
        Ok(ioctl(self.handle.0, IOCTL_LP_RECEIVE, &to, buf)? as usize)
    }

    /// Send a raw ATT PDU over the ATT (PSM 0x001F) hearing-aid channel.
    pub fn att_send(&self, data: &[u8]) -> io::Result<()> {
        ioctl(self.handle.0, IOCTL_LP_ATT_SEND, data, &mut [])?;
        Ok(())
    }

    /// Receive a raw ATT PDU from the ATT channel (blocking up to timeout_ms).
    pub fn att_recv(&self, timeout_ms: u32, buf: &mut [u8]) -> io::Result<usize> {
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms as u64);
        loop {
            // Retain short waits for compatibility with the older serial driver.
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let to = (remaining.as_millis().min(100) as u32).to_le_bytes();
            let n = ioctl(self.handle.0, IOCTL_LP_ATT_RECEIVE, &to, buf)? as usize;
            if n > 0 || std::time::Instant::now() >= deadline {
                return Ok(n);
            }
        }
    }

    /// Driver connection state (2 = connected), read without L2CAP traffic.
    pub fn status(&self) -> io::Result<u32> {
        let mut out = [0u8; 32];
        require_reply(
            ioctl(self.handle.0, IOCTL_LP_GET_STATUS, &[], &mut out)?,
            32,
        )?;
        Ok(u32::from_le_bytes([out[0], out[1], out[2], out[3]]))
    }

    /// Legacy diagnostic layout: the removed ATT server's first three fields
    /// are inactive. The last two describe the outbound ATT client open result
    /// and channel state, respectively.
    pub fn att_diag(&self) -> io::Result<(i32, u32, u32, i32, u32)> {
        let mut out = [0u8; 32];
        require_reply(
            ioctl(self.handle.0, IOCTL_LP_GET_STATUS, &[], &mut out)?,
            32,
        )?;
        Ok((
            i32::from_le_bytes([out[28], out[29], out[30], out[31]]), // register status
            u32::from_le_bytes([out[12], out[13], out[14], out[15]]), // registered 0/1
            u32::from_le_bytes([out[16], out[17], out[18], out[19]]), // indications
            i32::from_le_bytes([out[20], out[21], out[22], out[23]]), // accept status
            u32::from_le_bytes([out[24], out[25], out[26], out[27]]), // channel open
        ))
    }

    pub fn disconnect(&self) -> io::Result<()> {
        ioctl(self.handle.0, IOCTL_LP_DISCONNECT, &[], &mut [])?;
        Ok(())
    }
}

pub(crate) fn device_instance_address(instance: &str) -> Option<u64> {
    let id = instance.to_ascii_lowercase();
    let mut parts = id.splitn(3, '\\');
    if parts.next()? != "bthenum" {
        return None;
    }
    let device = parts.next()?;
    let address = if let Some(address) = device.strip_prefix("dev_") {
        address
    } else if matches!(
        device.split('_').next()?,
        "{74ec2172-0bad-4d01-8f77-997b2be0722a}" | "{0000110b-0000-1000-8000-00805f9b34fb}"
    ) {
        let (address, channel) = parts.next()?.rsplit('&').next()?.split_once("_c")?;
        if channel.len() != 8 || !channel.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        address
    } else {
        return None;
    };
    if address.len() != 12 || !address.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u64::from_str_radix(address, 16)
        .ok()
        .filter(|address| *address != 0)
}

unsafe fn owning_bluetooth_address(mut node: u32) -> Option<u64> {
    // Service nodes may sit directly under the radio, without a DEV_<MAC> ancestor.
    for _ in 0..8 {
        let mut id = [0u16; 512];
        if CM_Get_Device_IDW(node, id.as_mut_ptr(), id.len() as u32, 0) == CR_SUCCESS {
            let end = id.iter().position(|c| *c == 0)?;
            if let Some(address) = device_instance_address(&String::from_utf16_lossy(&id[..end])) {
                return Some(address);
            }
        }
        let mut parent = 0;
        if CM_Get_Parent(&mut parent, node, 0) != CR_SUCCESS || parent == node {
            break;
        }
        node = parent;
    }
    None
}

fn open_driver(mac: u64) -> io::Result<HANDLE> {
    if mac == 0 || mac > 0xffff_ffff_ffff {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid Bluetooth address",
        ));
    }
    unsafe {
        let devinfo = SetupDiGetClassDevsW(
            &GUID_DEVINTERFACE_NTPODSAAP,
            ptr::null(),
            ptr::null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        );
        if devinfo == INVALID_HANDLE_VALUE as isize {
            return Err(io::Error::last_os_error());
        }

        let result = (|| {
            for index in 0..u32::MAX {
                let mut ifdata: SP_DEVICE_INTERFACE_DATA = std::mem::zeroed();
                ifdata.cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
                if SetupDiEnumDeviceInterfaces(
                    devinfo,
                    ptr::null(),
                    &GUID_DEVINTERFACE_NTPODSAAP,
                    index,
                    &mut ifdata,
                ) == 0
                {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() == Some(ERROR_NO_MORE_ITEMS as i32) {
                        break;
                    }
                    return Err(error);
                }
                let mut required: u32 = 0;
                let first = SetupDiGetDeviceInterfaceDetailW(
                    devinfo,
                    &ifdata,
                    ptr::null_mut(),
                    0,
                    &mut required,
                    ptr::null_mut(),
                );
                if first != 0
                    || GetLastError() != ERROR_INSUFFICIENT_BUFFER
                    || (required as usize)
                        < std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>()
                    || required > 64 * 1024
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Invalid device interface size",
                    ));
                }
                let mut buf =
                    vec![0usize; (required as usize).div_ceil(std::mem::size_of::<usize>())];
                let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
                (*detail).cbSize = if cfg!(target_pointer_width = "64") {
                    8
                } else {
                    6
                };
                let mut device: SP_DEVINFO_DATA = std::mem::zeroed();
                device.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as u32;
                if SetupDiGetDeviceInterfaceDetailW(
                    devinfo,
                    &ifdata,
                    detail,
                    required,
                    ptr::null_mut(),
                    &mut device,
                ) == 0
                {
                    return Err(io::Error::last_os_error());
                }
                if owning_bluetooth_address(device.DevInst) != Some(mac) {
                    continue;
                }
                let handle = CreateFileW(
                    (*detail).DevicePath.as_ptr(),
                    GENERIC_READ | GENERIC_WRITE,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    ptr::null(),
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED,
                    ptr::null_mut(),
                );
                return if handle == INVALID_HANDLE_VALUE {
                    Err(io::Error::last_os_error())
                } else {
                    Ok(handle)
                };
            }
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                "NTPodsAAP driver not found for the selected AirPods",
            ))
        })();
        SetupDiDestroyDeviceInfoList(devinfo);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use windows_sys::Win32::Foundation::{
        ERROR_IO_INCOMPLETE, ERROR_OPERATION_ABORTED, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile, PIPE_ACCESS_DUPLEX};
    use windows_sys::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS,
    };

    #[cfg(feature = "experimental-playback")]
    #[test]
    fn playback_disconnect_is_classified_without_losing_the_windows_error() {
        use windows_sys::Win32::Foundation::{
            ERROR_CONNECTION_ABORTED, ERROR_DEVICE_NOT_CONNECTED,
        };

        for (code, expected) in [
            (ERROR_DEVICE_NOT_CONNECTED, io::ErrorKind::NotConnected),
            (ERROR_CONNECTION_ABORTED, io::ErrorKind::ConnectionAborted),
        ] {
            let original = io::Error::from_raw_os_error(code as i32);
            let message = original.to_string();
            let error = playback_error(original);
            assert_eq!(error.kind(), expected);
            assert_eq!(error.to_string(), message);
            assert_eq!(
                error
                    .get_ref()
                    .unwrap()
                    .downcast_ref::<io::Error>()
                    .unwrap()
                    .raw_os_error(),
                Some(code as i32)
            );
        }
    }

    #[cfg(feature = "experimental-playback")]
    #[test]
    fn playback_preserves_rejection_cancellation_and_timeout_errors() {
        use windows_sys::Win32::Foundation::{
            ERROR_REQ_NOT_ACCEP, ERROR_SEM_TIMEOUT, ERROR_TIMEOUT,
        };

        for code in [
            ERROR_REQ_NOT_ACCEP,
            ERROR_OPERATION_ABORTED,
            WAIT_TIMEOUT,
            ERROR_SEM_TIMEOUT,
            ERROR_TIMEOUT,
        ] {
            let original = io::Error::from_raw_os_error(code as i32);
            let kind = original.kind();
            let message = original.to_string();
            let error = playback_error(original);
            assert_eq!(error.raw_os_error(), Some(code as i32));
            assert_eq!(error.kind(), kind);
            assert_eq!(error.to_string(), message);
        }
    }

    #[test]
    fn shared_handle_survives_control_session_replacement() {
        let shared = Shared::default();
        let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        assert!(!event.is_null());
        let original = Driver {
            handle: Arc::new(WinHandle(event)),
        };
        let weak = Arc::downgrade(&original.handle);
        shared.0.lock().unwrap().push((42, weak.clone()));
        assert!(!shared.request_recovery(42, |_| panic!("Recovery ran with a live handle")));
        let playback = shared.open(42).unwrap();
        drop(original);
        let replacement = shared.open(42).unwrap();
        assert!(Arc::ptr_eq(&playback.handle, &replacement.handle));
        drop(replacement);
        assert!(weak.upgrade().is_some());
        drop(playback);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn driver_instance_addresses_match_only_the_selected_bluetooth_peer() {
        let first = r"BTHENUM\DEV_112233445566\7&1&2&BLUETOOTHDEVICE_112233445566";
        let second = r"BTHENUM\{74ec2172-0bad-4d01-8f77-997b2be0722a}_VID&0001004c_PID&2027\7&1&2&AABBCCDDEEFF_C00000000";
        let selected = 0xaabbccddeeff;
        assert_eq!(device_instance_address(first), Some(0x112233445566));
        assert_eq!(device_instance_address(second), Some(selected));
        assert_eq!(
            [first, second]
                .into_iter()
                .position(|id| device_instance_address(id) == Some(selected)),
            Some(1)
        );
        assert_eq!(
            [first]
                .into_iter()
                .position(|id| device_instance_address(id) == Some(selected)),
            None
        );
        assert_eq!(
            device_instance_address(&second.to_lowercase()),
            Some(selected)
        );
        for invalid in [
            r"USB\DEV_AABBCCDDEEFF\instance",
            r"BTHENUM\DEV_000000000000\instance",
            r"BTHENUM\DEV_AABBCCDDEEF\instance",
            r"BTHENUM\DEV_AABBCCDDEEFG\instance",
            r"BTHENUM\DEV_AABBCCDDEEFF0\instance",
            r"BTHENUM\{00000000-0000-0000-0000-000000000000}\7&1&2&AABBCCDDEEFF_C00000000",
            r"BTHENUM\{74ec2172-0bad-4d01-8f77-997b2be0722a}\7&1&2&AABBCCDDEEFF_C0",
        ] {
            assert_eq!(device_instance_address(invalid), None, "{invalid}");
        }
    }

    #[test]
    fn audio_sink_instance_contains_its_peer_address_without_a_dev_ancestor() {
        let sink = r"BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&0001004c_PID&2027\7&1&2&AABBCCDDEEFF_C00000000";
        assert_eq!(device_instance_address(sink), Some(0xaabbccddeeff));
        assert_eq!(
            device_instance_address(&sink.to_lowercase()),
            Some(0xaabbccddeeff)
        );
        for invalid in [
            r"BTHENUM\{0000110a-0000-1000-8000-00805f9b34fb}\7&1&2&AABBCCDDEEFF_C00000000",
            r"BTHENUM\{0000110b-0000-1000-8000-00805f9b34fc}\7&1&2&AABBCCDDEEFF_C00000000",
            r"BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}junk\7&1&2&AABBCCDDEEFF_C00000000",
            r"BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}\7&1&2&000000000000_C00000000",
            r"BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}\7&1&2&AABBCCDDEEFG_C00000000",
            r"BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}\7&1&2&AABBCCDDEEFF_C0",
            r"BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}\7&1&2&AABBCCDDEEFF_C0000000G",
            r"BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}\7&1&2&AABBCCDDEEFF_C000000000",
        ] {
            assert_eq!(device_instance_address(invalid), None, "{invalid}");
        }
    }

    // Local test pipes exercise real Windows overlapped completion without AirPods.
    unsafe fn pipe_pair() -> (WinHandle, WinHandle) {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let name: Vec<u16> = format!(
            r"\\.\pipe\NTPods-transport-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )
        .encode_utf16()
        .chain(Some(0))
        .collect();
        let server = WinHandle(CreateNamedPipeW(
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
        let connect_event = WinHandle(CreateEventW(ptr::null(), 1, 0, ptr::null()));
        assert!(!connect_event.0.is_null());
        let mut connection: OVERLAPPED = std::mem::zeroed();
        connection.hEvent = connect_event.0;
        assert_eq!(ConnectNamedPipe(server.0, &mut connection), 0);
        assert_eq!(GetLastError(), ERROR_IO_PENDING);
        let client = WinHandle(CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            ptr::null(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        ));
        assert_ne!(client.0, INVALID_HANDLE_VALUE);
        wait_for_completion(server.0, &connection, &mut 0, 1000).unwrap();
        (server, client)
    }

    #[test]
    fn overlapping_reads_cancel_only_the_timed_out_request_and_drain_it() {
        unsafe {
            let (server, client) = pipe_pair();
            let first_event = WinHandle(CreateEventW(ptr::null(), 1, 0, ptr::null()));
            let second_event = WinHandle(CreateEventW(ptr::null(), 1, 0, ptr::null()));
            assert!(!first_event.0.is_null() && !second_event.0.is_null());
            let mut first: OVERLAPPED = std::mem::zeroed();
            let mut second: OVERLAPPED = std::mem::zeroed();
            first.hEvent = first_event.0;
            second.hEvent = second_event.0;
            let mut first_data = [0u8; 8];
            let mut second_data = [0u8; 8];
            assert_eq!(
                ReadFile(
                    server.0,
                    first_data.as_mut_ptr(),
                    8,
                    ptr::null_mut(),
                    &mut first
                ),
                0
            );
            assert_eq!(GetLastError(), ERROR_IO_PENDING);
            assert_eq!(
                ReadFile(
                    server.0,
                    second_data.as_mut_ptr(),
                    8,
                    ptr::null_mut(),
                    &mut second
                ),
                0
            );
            assert_eq!(GetLastError(), ERROR_IO_PENDING);
            let mut count = 0;
            assert_eq!(
                wait_for_completion(server.0, &first, &mut count, 10)
                    .unwrap_err()
                    .raw_os_error(),
                Some(WAIT_TIMEOUT as i32)
            );
            assert_eq!(GetOverlappedResult(server.0, &first, &mut count, 0), 0);
            assert_eq!(GetLastError(), ERROR_OPERATION_ABORTED);
            assert_eq!(GetOverlappedResult(server.0, &second, &mut count, 0), 0);
            assert_eq!(GetLastError(), ERROR_IO_INCOMPLETE);
            assert_ne!(
                WriteFile(client.0, b"audio".as_ptr(), 5, &mut count, ptr::null_mut()),
                0
            );
            wait_for_completion(server.0, &second, &mut count, 1000).unwrap();
            assert_eq!(count, 5);
            assert_eq!(&second_data[..5], b"audio");
            assert_eq!(first_data, [0u8; 8]);
        }
    }

    #[test]
    fn overlapped_completion_reports_closed_peer_without_waiting_for_deadline() {
        unsafe {
            let (server, client) = pipe_pair();
            let event = WinHandle(CreateEventW(ptr::null(), 1, 0, ptr::null()));
            assert!(!event.0.is_null());
            let mut operation: OVERLAPPED = std::mem::zeroed();
            operation.hEvent = event.0;
            let mut data = [0u8; 8];
            assert_eq!(
                ReadFile(
                    server.0,
                    data.as_mut_ptr(),
                    8,
                    ptr::null_mut(),
                    &mut operation
                ),
                0
            );
            assert_eq!(GetLastError(), ERROR_IO_PENDING);
            drop(client);
            let error = wait_for_completion(server.0, &operation, &mut 0, 1000).unwrap_err();
            assert_ne!(error.raw_os_error(), Some(WAIT_TIMEOUT as i32));
        }
    }
}
