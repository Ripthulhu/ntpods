use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut, read_unaligned};
use std::time::{Duration, Instant};
use windows_sys::core::GUID;
use windows_sys::Win32::Devices::Bluetooth::*;
use windows_sys::Win32::Foundation::{ERROR_NO_MORE_ITEMS, HANDLE};
use windows_sys::Win32::Networking::WinSock::*;
use windows_sys::Win32::System::Com::BLOB;

const PNP_SERVICE: GUID = GUID::from_u128(0x00001200_0000_1000_8000_00805f9b34fb);
const MAX_RESULT_BYTES: usize = 64 * 1024;
const MAX_RECORDS: usize = 8;

struct Winsock;
impl Drop for Winsock {
    fn drop(&mut self) {
        unsafe { WSACleanup() };
    }
}

struct Lookup(HANDLE);
impl Drop for Lookup {
    fn drop(&mut self) {
        unsafe { WSALookupServiceEnd(self.0) };
    }
}

pub fn gaming_supported(address: u64) -> Option<bool> {
    if address == 0 || address >> 48 != 0 {
        return None;
    }
    unsafe {
        let mut data = zeroed();
        if WSAStartup(0x0202, &mut data) != 0 {
            return None;
        }
        let _winsock = Winsock;
        let socket_address = SOCKADDR_BTH {
            addressFamily: AF_BTH,
            btAddr: address,
            serviceClassId: zeroed(),
            port: 0,
        };
        let mut context = [0u16; 64];
        let mut context_length = context.len() as u32;
        if WSAAddressToStringW(
            (&socket_address as *const SOCKADDR_BTH).cast(),
            size_of::<SOCKADDR_BTH>() as u32,
            null(),
            context.as_mut_ptr(),
            &mut context_length,
        ) != 0
        {
            return None;
        }
        let mut service = PNP_SERVICE;
        let mut query: WSAQUERYSETW = zeroed();
        query.dwSize = size_of::<WSAQUERYSETW>() as u32;
        query.dwNameSpace = NS_BTH;
        query.lpServiceClassId = &mut service;
        query.lpszContext = context.as_mut_ptr();
        let mut handle = null_mut();
        let deadline = Instant::now() + Duration::from_millis(250);
        // No FLUSHCACHE: missing cached records must not trigger over-air SDP at startup.
        if WSALookupServiceBeginW(&query, LUP_RETURN_BLOB, &mut handle) != 0 {
            return None;
        }
        let lookup = Lookup(handle);
        let mut storage = vec![0usize; MAX_RESULT_BYTES / size_of::<usize>()];
        let mut support = None;
        for index in 0..=MAX_RECORDS {
            if Instant::now() >= deadline {
                return None;
            }
            storage.fill(0);
            let result = storage.as_mut_ptr().cast::<WSAQUERYSETW>();
            (*result).dwSize = size_of::<WSAQUERYSETW>() as u32;
            let mut length = MAX_RESULT_BYTES as u32;
            if WSALookupServiceNextW(lookup.0, LUP_RETURN_BLOB, &mut length, result) != 0 {
                return match WSAGetLastError() {
                    WSA_E_NO_MORE | WSAENOMORE | WSASERVICE_NOT_FOUND => support,
                    _ => None,
                };
            }
            if index == MAX_RECORDS || length as usize > MAX_RESULT_BYTES {
                return None;
            }
            let bytes = std::slice::from_raw_parts(storage.as_ptr().cast(), length as usize);
            let value = result_gaming_supported(bytes)?;
            if support.is_some_and(|previous| previous != value) {
                return None;
            }
            support = Some(value);
        }
        None
    }
}

fn contained(bytes: &[u8], pointer: *const u8, length: usize) -> Option<&[u8]> {
    let start = (pointer as usize).checked_sub(bytes.as_ptr() as usize)?;
    bytes.get(start..start.checked_add(length)?)
}

fn result_gaming_supported(bytes: &[u8]) -> Option<bool> {
    let header = bytes.get(..size_of::<WSAQUERYSETW>())?;
    let query: WSAQUERYSETW = unsafe { read_unaligned(header.as_ptr().cast()) };
    if query.dwSize as usize != size_of::<WSAQUERYSETW>() || query.dwNameSpace != NS_BTH {
        return None;
    }
    let blob = contained(bytes, query.lpBlob.cast(), size_of::<BLOB>())?;
    let blob: BLOB = unsafe { read_unaligned(blob.as_ptr().cast()) };
    // PnP has no connectable address; the lookup's exact address context scopes this record.
    record_gaming_supported(contained(bytes, blob.pBlobData, blob.cbSize as usize)?)
}

fn attribute(record: &[u8], id: u16) -> Option<SDP_ELEMENT_DATA> {
    let mut data = unsafe { zeroed() };
    (unsafe { BluetoothSdpGetAttributeValue(record.as_ptr(), record.len() as u32, id, &mut data) }
        == 0)
        .then_some(data)
}

fn record_gaming_supported(record: &[u8]) -> Option<bool> {
    if record.is_empty() || record.len() > MAX_RESULT_BYTES {
        return None;
    }
    unsafe {
        let mut top: SDP_ELEMENT_DATA = zeroed();
        if BluetoothSdpGetElementData(record.as_ptr(), record.len() as u32, &mut top) != 0
            || top.r#type != SDP_TYPE_SEQUENCE
            || top.data.sequence.length as usize != record.len()
        {
            return None;
        }
        let mut previous = None::<u32>;
        if BluetoothSdpEnumAttributes(
            record.as_ptr(),
            record.len() as u32,
            Some(ordered_attribute),
            (&mut previous as *mut Option<u32>).cast(),
        ) == 0
            || previous == Some(u32::MAX)
        {
            return None;
        }
        let classes = attribute(record, SDP_ATTRIB_CLASS_ID_LIST as u16)?;
        if classes.r#type != SDP_TYPE_SEQUENCE {
            return None;
        }
        let sequence = classes.data.sequence;
        let sequence = contained(record, sequence.value, sequence.length as usize)?;
        let mut cursor = 0;
        let mut found_pnp = false;
        for _ in 0..16 {
            let mut item: SDP_ELEMENT_DATA = zeroed();
            match BluetoothSdpGetContainerElementData(
                sequence.as_ptr(),
                sequence.len() as u32,
                &mut cursor,
                &mut item,
            ) {
                ERROR_NO_MORE_ITEMS => {
                    if !found_pnp {
                        return None;
                    }
                    let features = attribute(record, 0xa001)?;
                    if features.r#type != SDP_TYPE_UINT || features.specificType != SDP_ST_UINT64 {
                        return None;
                    }
                    // SDP's canonical integer uses bit 20; macOS rotates the 32-bit halves.
                    return Some(features.data.uint64 & (1 << 20) != 0);
                }
                0 if item.r#type == SDP_TYPE_UUID => {
                    found_pnp |= match item.specificType {
                        SDP_ST_UUID16 => item.data.uuid16 == 0x1200,
                        SDP_ST_UUID32 => item.data.uuid32 == 0x1200,
                        SDP_ST_UUID128 => {
                            let uuid = item.data.uuid128;
                            uuid.data1 == PNP_SERVICE.data1
                                && uuid.data2 == PNP_SERVICE.data2
                                && uuid.data3 == PNP_SERVICE.data3
                                && uuid.data4 == PNP_SERVICE.data4
                        }
                        _ => return None,
                    };
                }
                _ => return None,
            }
        }
        None
    }
}

unsafe extern "system" fn ordered_attribute(
    id: u32,
    _: *const u8,
    _: u32,
    context: *const core::ffi::c_void,
) -> i32 {
    let previous = &mut *context.cast_mut().cast::<Option<u32>>();
    if id > u16::MAX as u32 || previous.is_some_and(|old| id <= old) {
        *previous = Some(u32::MAX);
        return 0;
    }
    *previous = Some(id);
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(features: u64) -> Vec<u8> {
        let mut result = vec![
            0x35, 20, 0x09, 0x00, 0x01, 0x35, 3, 0x19, 0x12, 0x00, 0x09, 0xa0, 0x01, 0x0b,
        ];
        result.extend_from_slice(&features.to_be_bytes());
        result
    }

    #[test]
    fn reads_captured_sdp_word_without_macos_half_rotation() {
        assert_eq!(record_gaming_supported(&record(0x087c7c1b)), Some(true));
        assert_eq!(record_gaming_supported(&record(0)), Some(false));
        assert_eq!(record_gaming_supported(&record(1 << 20)), Some(true));
        assert_eq!(record_gaming_supported(&record(1 << 49)), Some(false));
        assert_eq!(record_gaming_supported(&record(1 << 52)), Some(false));
    }

    #[test]
    fn missing_wrong_service_or_wrong_attribute_is_unknown() {
        let original = record(1 << 20);
        for (index, value) in [(8, 0x11), (12, 0x00), (13, 0x13)] {
            let mut bad = original.clone();
            bad[index] = value;
            assert_eq!(record_gaming_supported(&bad), None);
        }
        assert_eq!(record_gaming_supported(&[0x35, 0]), None);
    }

    #[test]
    fn truncated_trailing_and_duplicate_attributes_are_unknown() {
        let original = record(1 << 20);
        for length in 0..original.len() {
            assert_eq!(record_gaming_supported(&original[..length]), None);
        }
        let mut trailing = original.clone();
        trailing.push(0);
        assert_eq!(record_gaming_supported(&trailing), None);
        let mut duplicate = original.clone();
        duplicate.extend_from_slice(&original[10..]);
        duplicate[1] = (duplicate.len() - 2) as u8;
        assert_eq!(record_gaming_supported(&duplicate), None);
    }

    #[test]
    fn result_pointers_cannot_escape_returned_storage() {
        let bytes = [0u8; 32];
        assert!(contained(&bytes, null(), 1).is_none());
        assert!(contained(&bytes, bytes.as_ptr(), usize::MAX).is_none());
        assert!(contained(&bytes, bytes.as_ptr().wrapping_add(31), 2).is_none());
        assert_eq!(
            contained(&bytes, bytes.as_ptr().wrapping_add(31), 1),
            Some(&bytes[31..])
        );
        assert_eq!(result_gaming_supported(&bytes), None);
    }

    #[test]
    fn validates_namespace_and_sdp_blob_from_lookup_result() {
        let record = record(1 << 20);
        let blob_offset = size_of::<WSAQUERYSETW>();
        let record_offset = blob_offset + size_of::<BLOB>();
        let mut storage = vec![0usize; (record_offset + record.len()).div_ceil(size_of::<usize>())];
        let bytes = unsafe {
            std::slice::from_raw_parts_mut(
                storage.as_mut_ptr().cast::<u8>(),
                storage.len() * size_of::<usize>(),
            )
        };
        let query_pointer = bytes.as_mut_ptr().cast::<WSAQUERYSETW>();
        let blob_pointer = bytes.as_mut_ptr().wrapping_add(blob_offset).cast::<BLOB>();
        unsafe {
            (*query_pointer).dwSize = size_of::<WSAQUERYSETW>() as u32;
            (*query_pointer).dwNameSpace = NS_BTH;
            (*query_pointer).lpBlob = blob_pointer;
            (*blob_pointer).cbSize = record.len() as u32;
            (*blob_pointer).pBlobData = bytes.as_mut_ptr().add(record_offset);
        }
        bytes[record_offset..record_offset + record.len()].copy_from_slice(&record);
        assert_eq!(result_gaming_supported(bytes), Some(true));
        unsafe { (*query_pointer).dwNameSpace = 0 };
        assert_eq!(result_gaming_supported(bytes), None);
        unsafe {
            (*query_pointer).dwNameSpace = NS_BTH;
            (*blob_pointer).pBlobData = record.as_ptr().cast_mut();
        }
        assert_eq!(result_gaming_supported(bytes), None);
        unsafe {
            (*blob_pointer).pBlobData = bytes.as_mut_ptr().add(record_offset);
            (*blob_pointer).cbSize = u32::MAX;
        }
        assert_eq!(result_gaming_supported(bytes), None);
        unsafe { (*query_pointer).lpBlob = null_mut() };
        assert_eq!(result_gaming_supported(bytes), None);
    }

    #[test]
    fn invalid_device_addresses_do_not_start_lookup() {
        assert_eq!(gaming_supported(0), None);
        assert_eq!(gaming_supported(1 << 48), None);
    }
}
