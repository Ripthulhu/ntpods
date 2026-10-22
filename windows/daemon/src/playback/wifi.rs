use std::ffi::c_void;
use std::io;
use std::mem::{offset_of, size_of};
use std::ptr;
use windows_sys::core::GUID;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::NetworkManagement::WiFi::{
    wlan_interface_state_ad_hoc_network_formed, wlan_interface_state_associating,
    wlan_interface_state_authenticating, wlan_interface_state_connected,
    wlan_interface_state_disconnected, wlan_interface_state_disconnecting,
    wlan_interface_state_discovering, wlan_interface_state_not_ready,
    wlan_intf_opcode_interface_state, WlanCloseHandle, WlanEnumInterfaces, WlanFreeMemory,
    WlanOpenHandle, WlanQueryInterface, WLAN_INTERFACE_INFO, WLAN_INTERFACE_INFO_LIST,
    WLAN_INTF_OPCODE, WLAN_RATE_SET,
};

// SDK 10.0.26100 wlanapi.h; windows-sys 0.59 predates this location-free query.
const REALTIME_CONNECTION_QUALITY: WLAN_INTF_OPCODE = 19;
const MAX_INTERFACES: usize = 64;
const MAX_LINKS: usize = 16;

#[repr(C)]
#[derive(Clone, Copy)]
struct QualityHeader {
    phy_type: i32,
    quality: u32,
    rx_rate: u32,
    tx_rate: u32,
    mlo: i32,
    links: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct QualityLink {
    id: u8,
    frequency_mhz: u32,
    bandwidth: u32,
    rssi: i32,
    rates: WLAN_RATE_SET,
}

struct Client(HANDLE);

impl Drop for Client {
    fn drop(&mut self) {
        unsafe { WlanCloseHandle(self.0, ptr::null()) };
    }
}

struct Allocation(*mut c_void);

impl Drop for Allocation {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { WlanFreeMemory(self.0) };
        }
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn checked_count(count: u32, maximum: usize) -> io::Result<usize> {
    if count as usize > maximum {
        return Err(invalid("Wi-Fi response exceeds its item bound"));
    }
    Ok(count as usize)
}

fn quality_24ghz(data: &[u8]) -> io::Result<bool> {
    if data.len() < size_of::<QualityHeader>() {
        return Err(invalid("Truncated Wi-Fi connection quality"));
    }
    let header = unsafe { ptr::read_unaligned(data.as_ptr().cast::<QualityHeader>()) };
    let count = checked_count(header.links, MAX_LINKS)?;
    if count == 0
        || !matches!(header.phy_type, 2 | 4..=11)
        || !matches!(header.mlo, 0 | 1)
        || (header.mlo == 0 && count != 1)
        || header.quality > 100
        || data.len() != size_of::<QualityHeader>() + count * size_of::<QualityLink>()
    {
        return Err(invalid("Invalid Wi-Fi connection quality layout"));
    }
    any_24ghz(
        data[size_of::<QualityHeader>()..]
            .chunks_exact(size_of::<QualityLink>())
            .map(|bytes| {
                let link = unsafe { ptr::read_unaligned(bytes.as_ptr().cast::<QualityLink>()) };
                frequency_24ghz(link.frequency_mhz)
            }),
    )
}

fn frequency_24ghz(value: u32) -> io::Result<bool> {
    // Microsoft names the field MHz but describes kHz; both ranges are unambiguous.
    let mhz = if value >= 1_000_000 && value % 1000 == 0 {
        value / 1000
    } else {
        value
    };
    match mhz {
        2400..=2500 => Ok(true),
        4900..=5900 | 5925..=7125 => Ok(false),
        _ => Err(invalid("Unknown Wi-Fi connection frequency")),
    }
}

fn any_24ghz(results: impl IntoIterator<Item = io::Result<bool>>) -> io::Result<bool> {
    let mut error = None;
    for result in results {
        match result {
            Ok(true) => return Ok(true),
            Ok(false) => (),
            Err(failure) if error.is_none() => error = Some(failure),
            Err(_) => (),
        }
    }
    match error {
        Some(error) => Err(error),
        None => Ok(false),
    }
}

#[allow(non_upper_case_globals)]
fn connected(state: i32) -> io::Result<bool> {
    match state {
        wlan_interface_state_connected | wlan_interface_state_ad_hoc_network_formed => Ok(true),
        wlan_interface_state_not_ready | wlan_interface_state_disconnected => Ok(false),
        wlan_interface_state_disconnecting
        | wlan_interface_state_associating
        | wlan_interface_state_discovering
        | wlan_interface_state_authenticating => Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "Wi-Fi connection has no stable band",
        )),
        _ => Err(invalid("Unknown Wi-Fi interface state")),
    }
}

impl Client {
    fn query(&self, interface: &GUID, opcode: WLAN_INTF_OPCODE) -> io::Result<(Allocation, usize)> {
        let mut size = 0;
        let mut allocation = Allocation(ptr::null_mut());
        let result = unsafe {
            WlanQueryInterface(
                self.0,
                interface,
                opcode,
                ptr::null(),
                &mut size,
                &mut allocation.0,
                ptr::null_mut(),
            )
        };
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result as i32));
        }
        if allocation.0.is_null()
            || size == 0
            || size as usize > size_of::<QualityHeader>() + MAX_LINKS * size_of::<QualityLink>()
        {
            return Err(invalid("Invalid Wi-Fi query allocation"));
        }
        Ok((allocation, size as usize))
    }

    fn connected_24ghz(&self, interface: &GUID) -> io::Result<bool> {
        let (state, size) = self.query(interface, wlan_intf_opcode_interface_state)?;
        if size != size_of::<i32>() {
            return Err(invalid("Invalid Wi-Fi interface state size"));
        }
        let state = unsafe { ptr::read_unaligned(state.0.cast::<i32>()) };
        if !connected(state)? {
            return Ok(false);
        }
        let (quality, size) = self.query(interface, REALTIME_CONNECTION_QUALITY)?;
        // This query describes connected links, never cached or nearby access points.
        quality_24ghz(unsafe { std::slice::from_raw_parts(quality.0.cast::<u8>(), size) })
    }
}

pub(super) fn connected_24ghz() -> io::Result<bool> {
    let mut version = 0;
    let mut handle = ptr::null_mut();
    let result = unsafe { WlanOpenHandle(2, ptr::null(), &mut version, &mut handle) };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    if handle.is_null() {
        return Err(invalid("Missing Wi-Fi client handle"));
    }
    let client = Client(handle);
    if version != 2 {
        return Err(invalid("Unsupported Wi-Fi client version"));
    }
    let mut interfaces = ptr::null_mut();
    let result = unsafe { WlanEnumInterfaces(client.0, ptr::null(), &mut interfaces) };
    let allocation = Allocation(interfaces.cast());
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    if allocation.0.is_null() {
        return Err(invalid("Missing Wi-Fi interface list"));
    }
    // WlanEnumInterfaces owns the allocation and guarantees count entries after its header.
    let count = unsafe { ptr::read_unaligned(allocation.0.cast::<u32>()) };
    let count = checked_count(count, MAX_INTERFACES)?;
    let entries = unsafe {
        allocation
            .0
            .cast::<u8>()
            .add(offset_of!(WLAN_INTERFACE_INFO_LIST, InterfaceInfo))
            .cast::<WLAN_INTERFACE_INFO>()
    };
    any_24ghz((0..count).map(|index| {
        let interface = unsafe { ptr::read_unaligned(entries.add(index)) };
        client.connected_24ghz(&interface.InterfaceGuid)
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "reads the local PC's current Wi-Fi band"]
    fn reads_current_host_wifi_band() {
        println!(
            "connected_2_4_ghz={}",
            connected_24ghz().expect("Current Wi-Fi band is unavailable")
        );
    }

    fn quality(frequencies: &[u32]) -> Vec<u8> {
        let mut bytes =
            vec![0; size_of::<QualityHeader>() + frequencies.len() * size_of::<QualityLink>()];
        bytes[..4].copy_from_slice(&7i32.to_le_bytes());
        bytes[4..8].copy_from_slice(&50u32.to_le_bytes());
        bytes[16..20].copy_from_slice(&i32::from(frequencies.len() > 1).to_le_bytes());
        bytes[20..24].copy_from_slice(&(frequencies.len() as u32).to_le_bytes());
        for (index, frequency) in frequencies.iter().enumerate() {
            let offset = size_of::<QualityHeader>()
                + index * size_of::<QualityLink>()
                + offset_of!(QualityLink, frequency_mhz);
            bytes[offset..offset + 4].copy_from_slice(&frequency.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn connection_quality_layout_matches_the_pinned_windows_sdk() {
        assert_eq!(size_of::<QualityHeader>(), 24);
        assert_eq!(size_of::<QualityLink>(), 272);
        assert_eq!(offset_of!(QualityLink, frequency_mhz), 4);
        assert_eq!(offset_of!(WLAN_INTERFACE_INFO_LIST, InterfaceInfo), 8);
    }

    #[test]
    fn only_the_connected_link_frequency_decides_the_band() {
        for frequency in [2412, 2437, 2484] {
            assert!(quality_24ghz(&quality(&[frequency])).unwrap());
            assert!(quality_24ghz(&quality(&[frequency * 1000])).unwrap());
        }
        for frequency in [5180, 5500, 5955, 6115, 7115] {
            assert!(!quality_24ghz(&quality(&[frequency])).unwrap());
            assert!(!quality_24ghz(&quality(&[frequency * 1000])).unwrap());
        }
        assert!(quality_24ghz(&quality(&[5180, 2437])).unwrap());
        assert!(quality_24ghz(&quality(&[0, 2437])).unwrap());
        assert!(quality_24ghz(&quality(&[2437, 0])).unwrap());
        assert!(quality_24ghz(&quality(&[5180, 0])).is_err());
        assert!(!quality_24ghz(&quality(&[5180, 5955])).unwrap());
        assert!(quality_24ghz(&quality(&[0])).is_err());
        for unknown in [0, 1, 6, 2_412_001, 2_412_000_000] {
            assert!(frequency_24ghz(unknown).is_err());
        }
    }

    #[test]
    fn truncated_oversized_and_malformed_reports_are_unknown() {
        let valid = quality(&[2412]);
        for size in 0..valid.len() {
            assert!(quality_24ghz(&valid[..size]).is_err());
        }
        let mut extra = valid.clone();
        extra.push(0);
        assert!(quality_24ghz(&extra).is_err());
        for count in [0, 17, u32::MAX] {
            let mut invalid = valid.clone();
            invalid[20..24].copy_from_slice(&count.to_le_bytes());
            assert!(quality_24ghz(&invalid).is_err());
        }
        let mut non_mlo = quality(&[2412, 5180]);
        non_mlo[16..20].fill(0);
        assert!(quality_24ghz(&non_mlo).is_err());
        for phy in [0i32, 1, 3, 12] {
            let mut invalid = valid.clone();
            invalid[..4].copy_from_slice(&phy.to_le_bytes());
            assert!(quality_24ghz(&invalid).is_err());
        }
        for flag in [-1i32, 2] {
            let mut invalid = valid.clone();
            invalid[16..20].copy_from_slice(&flag.to_le_bytes());
            assert!(quality_24ghz(&invalid).is_err());
        }
        assert_eq!(checked_count(0, MAX_INTERFACES).unwrap(), 0);
        assert_eq!(checked_count(64, MAX_INTERFACES).unwrap(), 64);
        assert!(checked_count(65, MAX_INTERFACES).is_err());
        assert!(checked_count(u32::MAX, MAX_INTERFACES).is_err());
    }

    #[test]
    fn multiple_interfaces_require_proof_or_preserve_unknown_errors() {
        assert!(!any_24ghz([]).unwrap());
        assert!(!any_24ghz([Ok(false), Ok(false)]).unwrap());
        assert!(any_24ghz([Err(invalid("unknown")), Ok(true)]).unwrap());
        assert!(any_24ghz([Ok(false), Err(invalid("unknown"))]).is_err());
        assert!(any_24ghz([Ok(true), Err(invalid("unknown"))]).unwrap());
    }

    #[test]
    fn transitions_do_not_claim_that_wifi_left_the_24ghz_band() {
        for associated in [
            wlan_interface_state_connected,
            wlan_interface_state_ad_hoc_network_formed,
        ] {
            assert!(connected(associated).unwrap());
        }
        for inactive in [
            wlan_interface_state_not_ready,
            wlan_interface_state_disconnected,
        ] {
            assert!(!connected(inactive).unwrap());
        }
        for unknown in [-1, 3, 5, 6, 7, 8] {
            assert!(connected(unknown).is_err());
        }
    }
}
