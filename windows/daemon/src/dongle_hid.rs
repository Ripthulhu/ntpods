use super::dongle_protocol::{self, Report, SIZE};
use std::{io, mem, ptr};
use windows_sys::Win32::{
    Devices::{DeviceAndDriverInstallation::*, HumanInterfaceDevice::*},
    Foundation::*,
    Storage::FileSystem::*,
    System::{Threading::*, IO::*},
};

struct Handle(HANDLE);
// Each request has its own OVERLAPPED and buffer; CloseHandle waits for the last owner.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

pub struct Device {
    handle: Handle,
    pub path: Vec<u16>,
}

impl Device {
    pub(super) fn read_feature(&self, id: u8) -> io::Result<[u8; SIZE]> {
        let mut bytes = [0; SIZE];
        bytes[0] = id;
        self.feature(false, &mut bytes)?;
        Ok(bytes)
    }
    fn feature(&self, write: bool, bytes: &mut [u8; SIZE]) -> io::Result<()> {
        unsafe {
            let event = Handle(CreateEventW(ptr::null(), 1, 0, ptr::null()));
            if event.0.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mut overlapped: OVERLAPPED = mem::zeroed();
            overlapped.hEvent = event.0;
            let mut count = 0;
            // HID_IN_CTL_CODE(100), HID_OUT_CTL_CODE(100), from the Windows SDK.
            let code = if write { 0x000b0191 } else { 0x000b0192 };
            let ok = DeviceIoControl(
                self.handle.0,
                code,
                if write {
                    bytes.as_ptr().cast()
                } else {
                    ptr::null()
                },
                if write { SIZE as u32 } else { 0 },
                if write {
                    ptr::null_mut()
                } else {
                    bytes.as_mut_ptr().cast()
                },
                if write { 0 } else { SIZE as u32 },
                &mut count,
                &mut overlapped,
            );
            if ok == 0 && GetLastError() != ERROR_IO_PENDING {
                return Err(io::Error::last_os_error());
            }
            if ok == 0 && WaitForSingleObject(event.0, 400) != WAIT_OBJECT_0 {
                CancelIoEx(self.handle.0, &overlapped);
                // Windows still owns the buffer until cancellation has completed.
                GetOverlappedResult(self.handle.0, &overlapped, &mut count, 1);
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "USB control timed out",
                ));
            }
            if GetOverlappedResult(self.handle.0, &overlapped, &mut count, 0) == 0 {
                return Err(io::Error::last_os_error());
            }
            if !write && count != SIZE as u32 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Incomplete USB control report",
                ));
            }
            Ok(())
        }
    }
    fn read_raw(&self) -> io::Result<[u8; SIZE]> {
        let mut bytes = [0; SIZE];
        bytes[0] = 2;
        self.feature(false, &mut bytes)?;
        Ok(bytes)
    }
    pub fn read(&self) -> io::Result<Report> {
        let bytes = self.read_raw()?;
        dongle_protocol::parse(&bytes).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "Unsupported USB control report")
        })
    }
    pub fn write(&self, mut bytes: [u8; SIZE]) -> io::Result<()> {
        self.feature(true, &mut bytes)
    }
}

pub enum Discovery {
    None,
    Multiple,
    One(Device),
}

pub fn discover(previous_path: Option<&[u16]>) -> io::Result<Discovery> {
    unsafe {
        let mut guid = mem::zeroed();
        HidD_GetHidGuid(&mut guid);
        let info = SetupDiGetClassDevsW(
            &guid,
            ptr::null(),
            ptr::null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        );
        if info == INVALID_HANDLE_VALUE as isize {
            return Err(io::Error::last_os_error());
        }
        struct Info(isize);
        impl Drop for Info {
            fn drop(&mut self) {
                unsafe {
                    SetupDiDestroyDeviceInfoList(self.0);
                }
            }
        }
        let _info = Info(info);
        let mut found = None;
        let mut uncertain = false;
        let mut complete = false;
        for index in 0..256 {
            let mut interface: SP_DEVICE_INTERFACE_DATA = mem::zeroed();
            interface.cbSize = mem::size_of_val(&interface) as u32;
            if SetupDiEnumDeviceInterfaces(info, ptr::null(), &guid, index, &mut interface) == 0 {
                if GetLastError() == ERROR_NO_MORE_ITEMS {
                    complete = true;
                    break;
                }
                return Err(io::Error::last_os_error());
            }
            let mut required = 0;
            SetupDiGetDeviceInterfaceDetailW(
                info,
                &interface,
                ptr::null_mut(),
                0,
                &mut required,
                ptr::null_mut(),
            );
            if GetLastError() != ERROR_INSUFFICIENT_BUFFER || !(8..=65536).contains(&required) {
                uncertain = true;
                continue;
            }
            let mut storage = vec![0usize; (required as usize).div_ceil(mem::size_of::<usize>())];
            let detail = storage
                .as_mut_ptr()
                .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
            (*detail).cbSize = if cfg!(target_pointer_width = "64") {
                8
            } else {
                6
            };
            if SetupDiGetDeviceInterfaceDetailW(
                info,
                &interface,
                detail,
                required,
                ptr::null_mut(),
                ptr::null_mut(),
            ) == 0
            {
                uncertain = true;
                continue;
            }
            let path = std::slice::from_raw_parts(
                (*detail).DevicePath.as_ptr(),
                (required as usize - 4) / 2,
            );
            let Some(end) = path.iter().position(|c| *c == 0) else {
                uncertain = true;
                continue;
            };
            let path = path[..end].to_vec();
            let was_present = previous_path == Some(path.as_slice());
            let handle = CreateFileW(
                (*detail).DevicePath.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                ptr::null_mut(),
            );
            if handle == INVALID_HANDLE_VALUE {
                uncertain |= was_present;
                continue;
            }
            let handle = Handle(handle);
            let mut attributes: HIDD_ATTRIBUTES = mem::zeroed();
            attributes.Size = mem::size_of_val(&attributes) as u32;
            if HidD_GetAttributes(handle.0, &mut attributes) == 0 {
                uncertain |= was_present;
                continue;
            }
            if attributes.VendorID != 0xcafe {
                continue;
            }
            let mut preparsed = 0isize;
            if HidD_GetPreparsedData(handle.0, &mut preparsed) == 0 {
                uncertain |= was_present;
                continue;
            }
            let mut caps: HIDP_CAPS = mem::zeroed();
            let status = HidP_GetCaps(preparsed, &mut caps);
            HidD_FreePreparsedData(preparsed);
            if status != HIDP_STATUS_SUCCESS
                || caps.UsagePage != 0xff31
                || caps.Usage != 1
                || caps.FeatureReportByteLength != 64
            {
                uncertain |= was_present;
                continue;
            }
            let next = Device { handle, path };
            match next.read_raw() {
                Ok(identity) if identity[1..5] == *b"APDG" => {}
                Err(_) if previous_path == Some(next.path.as_slice()) => {}
                Err(_) => {
                    uncertain = true;
                    continue;
                }
                _ => continue,
            }
            // Unknown APDG versions stay visible, but cannot receive commands.
            if found.is_some() {
                return Ok(Discovery::Multiple);
            }
            found = Some(next);
        }
        if uncertain || !complete {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "USB identity unavailable",
            ));
        }
        Ok(found.map_or(Discovery::None, Discovery::One))
    }
}
