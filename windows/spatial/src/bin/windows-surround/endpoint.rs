use std::error::Error;
use wasapi::Device;
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_Device_IDW, CM_Get_Parent, CM_Locate_DevNodeW, CR_SUCCESS, MAX_DEVICE_ID_LEN,
};

fn usb_serial(instance_id: &str) -> Option<&str> {
    let mut parts = instance_id.split('\\');
    let bus = parts.next()?;
    let device = parts.next()?;
    let serial = parts.next()?;
    (bus.eq_ignore_ascii_case("USB")
        && device.eq_ignore_ascii_case("VID_CAFE&PID_4015")
        && parts.next().is_none()
        && !serial.is_empty()
        && !serial.contains('&'))
    .then_some(serial)
}

pub fn dongle_serial(sink: &Device) -> Result<String, Box<dyn Error>> {
    // The endpoint's PnP ancestry identifies the right unit even with two identical dongles.
    let endpoint: Vec<u16> = format!("SWD\\MMDEVAPI\\{}\0", sink.get_id()?)
        .encode_utf16()
        .collect();
    let mut node = 0;
    if unsafe { CM_Locate_DevNodeW(&mut node, endpoint.as_ptr(), 0) } != CR_SUCCESS {
        return Err("The selected Windows output is no longer present".into());
    }
    let mut id = [0u16; MAX_DEVICE_ID_LEN as usize];
    loop {
        let mut parent = 0;
        if unsafe { CM_Get_Parent(&mut parent, node, 0) } != CR_SUCCESS || node == parent {
            break;
        }
        node = parent;
        if unsafe { CM_Get_Device_IDW(node, id.as_mut_ptr(), MAX_DEVICE_ID_LEN, 0) } != CR_SUCCESS {
            return Err("Could not read the selected Windows output's USB parent".into());
        }
        let end = id
            .iter()
            .position(|&unit| unit == 0)
            .ok_or("Invalid PnP device ID")?;
        let instance_id = String::from_utf16(&id[..end])?;
        if let Some(serial) = usb_serial(&instance_id) {
            return Ok(serial.to_owned());
        }
    }
    Err("The selected output does not belong to an AirPods USB dongle with a serial number".into())
}

#[cfg(test)]
mod tests {
    use super::usb_serial;

    #[test]
    fn binds_only_to_a_serialized_physical_dongle() {
        assert_eq!(usb_serial(r"USB\vid_cafe&pid_4015\A012"), Some("A012"));
        for invalid in [
            r"USB\VID_CAFE&PID_4015&MI_00\other",
            r"USB\VID_CAFE&PID_4015\7&12345678&0&9",
            r"USB\VID_CAFE&PID_4015\",
            r"USB\VID_CAFE&PID_4016\A012",
            r"HID\VID_CAFE&PID_4015\A012",
            r"USB\VID_CAFE&PID_4015\A012\A013",
        ] {
            assert_eq!(usb_serial(invalid), None);
        }
    }
}
