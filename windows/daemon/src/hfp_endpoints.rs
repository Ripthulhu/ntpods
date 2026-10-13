//! Endpoint visibility for the selected AirPods' Windows hands-free profile.

use std::ffi::c_void;
use std::ptr;

use windows::core::{
    Error, IUnknown, IUnknown_Vtbl, Interface, Result, BSTR, GUID, HRESULT, PCWSTR,
};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_InstanceId;
use windows::Win32::Foundation::{ERROR_NOT_FOUND, E_CHANGED_STATE, E_FAIL, E_INVALIDARG};
use windows::Win32::Media::Audio::{
    eAll, eCapture, eRender, IDeviceTopology, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
    DEVICE_STATE, DEVICE_STATEMASK_ALL, DEVICE_STATE_ACTIVE, DEVICE_STATE_DISABLED,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED, STGM_READ,
};
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_DevNode_Registry_PropertyW, CM_Get_Device_IDW, CM_Get_Parent, CM_Locate_DevNodeW,
    CM_DRP_HARDWAREID, CM_DRP_SERVICE, CM_LOCATE_DEVNODE_PHANTOM, CR_SUCCESS,
};

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
        if !String::from_utf16_lossy(&service[..end]).eq_ignore_ascii_case("BthHFAud") {
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
