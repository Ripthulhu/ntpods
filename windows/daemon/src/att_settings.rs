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
    Ok(discover_many(exchange, &[*characteristic], name)?[0])
}

pub(super) fn discover_many(
    exchange: &mut impl FnMut(&[u8]) -> Result<Vec<u8>, String>,
    characteristics: &[[u8; 16]],
    name: &str,
) -> Result<Vec<Option<Characteristic>>, String> {
    discover_with_properties(exchange, characteristics, name, 0x02)
}

pub(super) fn discover_write_request(
    exchange: &mut impl FnMut(&[u8]) -> Result<Vec<u8>, String>,
    characteristic: &[u8; 16],
    name: &str,
) -> Result<Option<Characteristic>, String> {
    Ok(discover_with_properties(exchange, &[*characteristic], name, 0x08)?[0])
}

fn discover_with_properties(
    exchange: &mut impl FnMut(&[u8]) -> Result<Vec<u8>, String>,
    characteristics: &[[u8; 16]],
    name: &str,
    required: u8,
) -> Result<Vec<Option<Characteristic>>, String> {
    if characteristics.is_empty()
        || characteristics.len() > 8
        || characteristics
            .iter()
            .enumerate()
            .any(|(index, uuid)| characteristics[..index].contains(uuid))
    {
        return Err(format!("{name} requested an invalid characteristic set."));
    }
    let mut found = vec![None; characteristics.len()];
    let mut service_request = vec![0x06, 0x01, 0x00, 0xff, 0xff, 0x00, 0x28];
    service_request.extend_from_slice(&SERVICE);
    let service = exchange(&service_request)?;
    if no_attribute(&service, &service_request) {
        return Ok(found);
    }
    let [0x07, start_lo, start_hi, end_lo, end_hi] = service.as_slice() else {
        return Err(format!("{name} service discovery failed."));
    };
    let mut start = u16::from_le_bytes([*start_lo, *start_hi]);
    let end = u16::from_le_bytes([*end_lo, *end_hi]);
    if start == 0 || start >= end {
        return Err(format!("{name} service range is invalid."));
    }
    let mut previous_value = 0;
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
            if declaration <= previous
                || declaration <= previous_value
                || declaration > end
                || handle <= declaration
                || handle > end
            {
                return Err(format!("{name} characteristic range is invalid."));
            }

            if let Some(index) = characteristics.iter().position(|uuid| entry[5..] == *uuid) {
                if found[index].is_some() || entry[2] & required != required {
                    let access = if required == 0x02 {
                        "readable"
                    } else {
                        "write-request"
                    };
                    return Err(format!("{name} does not expose a unique {access} setting."));
                }
                found[index] = Some(Characteristic {
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
            previous_value = handle;
        }
        if previous == end {
            return Ok(found);
        }
        start = previous + 1;
    }
    Err(format!("{name} discovery exceeded the service limit."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    fn script(properties: u8) -> VecDeque<Vec<u8>> {
        let mut entry = vec![0x09, 21, 0x13, 0, properties, 0x14, 0];
        entry.extend_from_slice(&[0x77; 16]);
        VecDeque::from([
            vec![0x07, 0x12, 0, 0x18, 0],
            entry,
            vec![1, 8, 0x18, 0, 0x0a],
        ])
    }

    #[test]
    fn explicit_write_discovery_accepts_only_write_request_properties() {
        for properties in [0x08, 0x0a, 0x0c, 0x1e] {
            let mut script = script(properties);
            let found = discover_write_request(
                &mut |_| Ok(script.pop_front().unwrap()),
                &[0x77; 16],
                "Trigger",
            )
            .unwrap()
            .unwrap();
            assert_eq!(
                found,
                Characteristic {
                    handle: 0x14,
                    write_opcode: Some(0x12)
                }
            );
            assert!(script.is_empty());
        }
        for properties in [0, 0x02, 0x04, 0x06, 0x10] {
            let mut script = script(properties);
            assert!(discover_write_request(
                &mut |_| Ok(script.pop_front().unwrap()),
                &[0x77; 16],
                "Trigger"
            )
            .is_err());
        }
        let mut script = script(0x08);
        assert!(discover(
            &mut |_| Ok(script.pop_front().unwrap()),
            &[0x77; 16],
            "Read"
        )
        .is_err());
    }

    #[test]
    fn invalid_requested_uuid_sets_fail_without_sending() {
        for characteristics in [vec![], vec![[0; 16]; 2], vec![[0; 16]; 9]] {
            assert!(discover_many(
                &mut |_| panic!("invalid set reached ATT"),
                &characteristics,
                "Metadata"
            )
            .is_err());
        }
    }

    #[test]
    fn characteristics_cannot_overlap_or_alias_value_handles_across_pages() {
        let first = [0x77; 16];
        let second = [0x88; 16];
        let mut page = vec![0x09, 21, 0x13, 0, 2, 0x20, 0];
        page.extend_from_slice(&first);
        let mut overlap = vec![0x09, 21, 0x15, 0, 2, 0x20, 0];
        overlap.extend_from_slice(&second);
        let mut replies = VecDeque::from([vec![0x07, 0x12, 0, 0x30, 0], page, overlap]);
        assert!(discover_many(
            &mut |_| Ok(replies.pop_front().unwrap()),
            &[first, second],
            "Metadata"
        )
        .is_err());
        assert!(replies.is_empty());
    }
}
