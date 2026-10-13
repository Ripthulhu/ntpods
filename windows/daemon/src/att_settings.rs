//! Discovery shared by the AirPods hearing and Custom Transparency settings.

// ATT UUIDs are little-endian, unlike their printed form.
pub(super) const SERVICE: [u8; 16] = [
    0x4e, 0xe0, 0xef, 0x92, 0x34, 0x56, 0x33, 0x99, 0xa6, 0x45, 0xb7, 0xb7, 0x2b, 0x08, 0x98, 0x77,
];
pub(super) fn no_attribute(response: &[u8], request: &[u8]) -> bool {
    let [opcode @ (0x06 | 0x08), start_lo, start_hi, end_lo, end_hi, ..] = request else {
        return false;
    };
    let [0x01, failed, handle_lo, handle_hi, 0x0a] = response else {
        return false;
    };
    let start = u16::from_le_bytes([*start_lo, *start_hi]);
    let end = u16::from_le_bytes([*end_lo, *end_hi]);
    let handle = u16::from_le_bytes([*handle_lo, *handle_hi]);
    // Pro 3 can report the last searched handle rather than the requested start.
    failed == opcode && start != 0 && (start..=end).contains(&handle)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Characteristic {
    pub handle: u16,
    pub write_opcode: Option<u8>,
}

pub(super) fn discover(
    exchange: &mut impl FnMut(&[u8]) -> Result<Vec<u8>, String>,
    characteristic: &[u8; 16],
    name: &str,
) -> Result<Option<Characteristic>, String> {
    let mut service_request = vec![0x06, 0x01, 0x00, 0xff, 0xff, 0x00, 0x28];
    service_request.extend_from_slice(&SERVICE);
    let service = exchange(&service_request)?;
    if no_attribute(&service, &service_request) {
        return Ok(None);
    }
    let [0x07, start_lo, start_hi, end_lo, end_hi] = service.as_slice() else {
        return Err(format!("{name} service discovery failed."));
    };
    let mut start = u16::from_le_bytes([*start_lo, *start_hi]);
    let end = u16::from_le_bytes([*end_lo, *end_hi]);
    if start == 0 || start >= end {
        return Err(format!("{name} service range is invalid."));
    }
    let mut found = None;
    for _ in 0..16 {
        let mut request = vec![0x08];
        request.extend_from_slice(&start.to_le_bytes());
        request.extend_from_slice(&end.to_le_bytes());
        request.extend_from_slice(&[0x03, 0x28]);
        let response = exchange(&request)?;
        if no_attribute(&response, &request) {
            return Ok(found);
        }
        let [0x09, each @ (7 | 21), entries @ ..] = response.as_slice() else {
            return Err(format!("{name} characteristic discovery failed."));
        };
        if entries.is_empty() || entries.len() % *each as usize != 0 {
            return Err(format!("{name} characteristic list is incomplete."));
        }
        let mut previous = start - 1;
        for entry in entries.chunks_exact(*each as usize) {
            let declaration = u16::from_le_bytes([entry[0], entry[1]]);
            let handle = u16::from_le_bytes([entry[3], entry[4]]);
            if declaration <= previous || declaration > end || handle <= declaration || handle > end
            {
                return Err(format!("{name} characteristic range is invalid."));
            }

            if entry[5..] == *characteristic {
                if found.is_some() || entry[2] & 0x02 == 0 {
                    return Err(format!("{name} does not expose a unique readable setting."));
                }
                found = Some(Characteristic {
                    handle,
                    write_opcode: if entry[2] & 0x08 != 0 {
                        Some(0x12)
                    } else if entry[2] & 0x04 != 0 {
                        Some(0x52)
                    } else {
                        None
                    },
                });
            }
            previous = declaration;
        }
        if previous == end {
            return Ok(found);
        }
        start = previous + 1;
    }
    Err(format!("{name} discovery exceeded the service limit."))
}
