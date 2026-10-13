//! Custom Transparency ATT settings, separate from hearing assistance.
//! Layout: LibrePods android/rewrite 34076c85, Transparency.kt; Pro 3 macOS captures.

use crate::{driver::Driver, hearing};
use ntpods_ipc::TransparencySettings;
use std::sync::atomic::{AtomicU64, Ordering};

// ATT UUIDs are little-endian, unlike their printed form.
const SERVICE: [u8; 16] = [
    0x4e, 0xe0, 0xef, 0x92, 0x34, 0x56, 0x33, 0x99, 0xa6, 0x45, 0xb7, 0xb7, 0x2b, 0x08, 0x98, 0x77,
];
const CHARACTERISTIC: [u8; 16] = [
    0x90, 0x53, 0x4d, 0x74, 0xaf, 0x9e, 0x03, 0x94, 0xdb, 0x45, 0xab, 0xf7, 0xc1, 0x1c, 0x62, 0xd5,
];
const VALUE_LEN: usize = 104;
static GENERATION: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
pub struct Token {
    generation: u64,
    session: u64,
}

impl Token {
    pub fn is_current(self) -> bool {
        self.generation == GENERATION.load(Ordering::SeqCst) && self.session == hearing::session()
    }
}

pub fn token() -> Token {
    Token {
        generation: GENERATION.fetch_add(1, Ordering::SeqCst).wrapping_add(1),
        session: hearing::session(),
    }
}

fn read_f32(value: &[u8], offset: usize) -> f32 {
    f32::from_le_bytes(value[offset..offset + 4].try_into().unwrap())
}

fn parse(value: &[u8]) -> Result<TransparencySettings, String> {
    // Reject unfamiliar layouts instead of overwriting a newer firmware's settings.
    if value.len() != VALUE_LEN
        || !value
            .chunks_exact(4)
            .all(|v| f32::from_le_bytes(v.try_into().unwrap()).is_finite())
        || ![0, 44, 92]
            .into_iter()
            .all(|off| matches!(read_f32(value, off), 0.0 | 1.0))
    {
        return Err("The AirPods returned an unsupported Custom Transparency format.".into());
    }
    let result = TransparencySettings {
        enabled: read_f32(value, 0) == 1.0,
        left_amplification: read_f32(value, 36),
        left_tone: read_f32(value, 40),
        left_conversation_boost: read_f32(value, 44) == 1.0,
        left_ambient_noise_reduction: read_f32(value, 48),
        right_amplification: read_f32(value, 84),
        right_tone: read_f32(value, 88),
        right_conversation_boost: read_f32(value, 92) == 1.0,
        right_ambient_noise_reduction: read_f32(value, 96),
    };
    if result.is_valid() {
        Ok(result)
    } else {
        Err("The AirPods returned unsupported Custom Transparency values.".into())
    }
}

fn patch(value: &mut [u8], requested: &TransparencySettings) {
    // Leave both eight-band EQ curves and own-voice settings byte-for-byte intact.
    for (offset, v) in [
        (0, if requested.enabled { 1.0 } else { 0.0 }),
        (36, requested.left_amplification),
        (40, requested.left_tone),
        (
            44,
            if requested.left_conversation_boost {
                1.0
            } else {
                0.0
            },
        ),
        (48, requested.left_ambient_noise_reduction),
        (84, requested.right_amplification),
        (88, requested.right_tone),
        (
            92,
            if requested.right_conversation_boost {
                1.0
            } else {
                0.0
            },
        ),
        (96, requested.right_ambient_noise_reduction),
    ] {
        value[offset..offset + 4].copy_from_slice(&v.to_le_bytes());
    }
}

fn no_attribute(response: &[u8], request: &[u8]) -> bool {
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

fn discover(
    exchange: &mut impl FnMut(&[u8]) -> Result<Vec<u8>, String>,
) -> Result<Option<(u16, u8)>, String> {
    let mut service_request = vec![0x06, 0x01, 0x00, 0xff, 0xff, 0x00, 0x28];
    service_request.extend_from_slice(&SERVICE);
    let service = exchange(&service_request)?;
    if no_attribute(&service, &service_request) {
        return Ok(None);
    }
    let [0x07, start_lo, start_hi, end_lo, end_hi] = service.as_slice() else {
        return Err("Custom Transparency service discovery failed.".into());
    };
    let mut start = u16::from_le_bytes([*start_lo, *start_hi]);
    let end = u16::from_le_bytes([*end_lo, *end_hi]);
    if start == 0 || start >= end {
        return Err("Custom Transparency service range is invalid.".into());
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
            return Err("Custom Transparency characteristic discovery failed.".into());
        };
        if entries.is_empty() || entries.len() % *each as usize != 0 {
            return Err("Custom Transparency characteristic list is incomplete.".into());
        }
        let mut previous = start - 1;
        for entry in entries.chunks_exact(*each as usize) {
            let declaration = u16::from_le_bytes([entry[0], entry[1]]);
            let handle = u16::from_le_bytes([entry[3], entry[4]]);
            if declaration <= previous || declaration > end || handle <= declaration || handle > end
            {
                return Err("Custom Transparency characteristic range is invalid.".into());
            }

            if entry[5..] == CHARACTERISTIC {
                if found.is_some() || entry[2] & 0x02 == 0 || entry[2] & 0x0c == 0 {
                    return Err(
                        "Custom Transparency does not expose a unique writable setting.".into(),
                    );
                }
                found = Some((handle, if entry[2] & 0x08 != 0 { 0x12 } else { 0x52 }));
            }
            previous = declaration;
        }
        if previous == end {
            return Ok(found);
        }
        start = previous + 1;
    }
    Err("Custom Transparency discovery exceeded the service limit.".into())
}

fn transaction(
    requested: Option<&TransparencySettings>,
    mut exchange: impl FnMut(&[u8]) -> Result<Vec<u8>, String>,
) -> Result<Option<TransparencySettings>, String> {
    if requested.is_some_and(|value| !value.is_valid()) {
        return Err("Custom Transparency values are out of range.".into());
    }
    let Some((handle, write_opcode)) = discover(&mut exchange)? else {
        return Ok(None);
    };
    let read = [0x0a, handle as u8, (handle >> 8) as u8];
    let response = exchange(&read)?;
    let Some(value) = response.strip_prefix(&[0x0b]) else {
        return Err("Could not read Custom Transparency.".into());
    };
    let current = parse(value)?;
    let Some(requested) = requested.filter(|requested| **requested != current) else {
        return Ok(Some(current));
    };
    let mut value = value.to_vec();
    patch(&mut value, requested);
    let mut write = vec![write_opcode, handle as u8, (handle >> 8) as u8];
    write.extend_from_slice(&value);
    let response = exchange(&write)?;
    if write_opcode == 0x12 && response != [0x13] {
        return Err("AirPods did not acknowledge Custom Transparency.".into());
    }
    let response = exchange(&read)?;
    let Some(value) = response.strip_prefix(&[0x0b]) else {
        return Err("Could not confirm Custom Transparency.".into());
    };
    // The caller compares this confirmed state with the request before reporting success.
    parse(value).map(Some)
}

pub fn settings(
    drv: &Driver,
    token: Token,
    requested: Option<&TransparencySettings>,
    hearing_supported: bool,
) -> Result<Option<TransparencySettings>, String> {
    if requested.is_some_and(|value| !value.is_valid()) {
        return Err("Custom Transparency values are out of range.".into());
    }
    match hearing::att_transaction(
        drv,
        || token.is_current(),
        |exchange| {
            if requested.is_some_and(|s| s.enabled)
                && !hearing::custom_transparency_allowed(hearing_supported)
            {
                return Err("Turn off hearing assistance before enabling Custom Transparency. Reconnect if it is already off.".into());
            }
            transaction(requested, exchange)
        },
    )? {
        Some(Some(settings)) => Ok(Some(settings)),
        Some(None) => Err("Custom Transparency is not available on these AirPods.".into()),
        None => Ok(None),
    }
}

pub(super) fn read_enabled(
    exchange: impl FnMut(&[u8]) -> Result<Vec<u8>, String>,
) -> Result<bool, String> {
    transaction(None, exchange).map(|settings| settings.is_some_and(|s| s.enabled))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value() -> Vec<u8> {
        let mut value = vec![0; VALUE_LEN];
        value[100..104].copy_from_slice(&0.5f32.to_le_bytes());
        value
    }

    fn declaration(handle: u16) -> Vec<u8> {
        let mut response = vec![0x09, 21];
        response.extend_from_slice(&(handle - 1).to_le_bytes());
        response.push(0x0a);
        response.extend_from_slice(&handle.to_le_bytes());
        response.extend_from_slice(&CHARACTERISTIC);
        response
    }

    fn discovery() -> Vec<(Vec<u8>, Vec<u8>)> {
        let mut service = vec![0x06, 1, 0, 0xff, 0xff, 0, 0x28];
        service.extend_from_slice(&SERVICE);
        vec![
            (service, vec![0x07, 0x30, 0, 0x40, 0]),
            (vec![0x08, 0x30, 0, 0x40, 0, 3, 0x28], declaration(0x35)),
            (
                vec![0x08, 0x35, 0, 0x40, 0, 3, 0x28],
                vec![1, 8, 0x35, 0, 0x0a],
            ),
        ]
    }

    #[test]
    fn captures_decode_and_unexposed_values_survive_edits() {
        let mut bytes = value();
        assert!(!parse(&bytes).unwrap().enabled);
        for offset in (4..36).chain(52..84).step_by(4) {
            bytes[offset..offset + 4].copy_from_slice(&12.5f32.to_le_bytes());
        }
        let original = bytes.clone();
        let mut requested = parse(&bytes).unwrap();
        requested.enabled = true;
        requested.left_amplification = -1.0;
        requested.right_amplification = 1.0;
        requested.left_tone = 0.25;
        requested.right_tone = -0.75;
        requested.left_conversation_boost = true;
        requested.right_ambient_noise_reduction = 1.0;
        patch(&mut bytes, &requested);
        assert_eq!(parse(&bytes).unwrap(), requested);
        assert_eq!(&bytes[4..36], &original[4..36]);
        assert_eq!(&bytes[52..84], &original[52..84]);
        assert_eq!(&bytes[100..104], &original[100..104]);
    }

    #[test]
    fn malformed_or_hearing_aid_layouts_are_not_written() {
        for size in [0, 4, 99, 100, 103, 105, 108] {
            assert!(parse(&vec![0; size]).is_err());
        }
        for (offset, bad) in [(0, 0.5), (36, 1.01), (48, -0.1), (92, 2.0), (100, f32::NAN)] {
            let mut bytes = value();
            bytes[offset..offset + 4].copy_from_slice(&bad.to_le_bytes());
            assert!(parse(&bytes).is_err());
        }
        let mut bytes = value();
        bytes[2] = 0x64; // Hearing assistance uses a mode byte, not a float boolean.
        assert!(parse(&bytes).is_err());
        for bad in [f32::NAN, f32::INFINITY, -1.01, 1.01] {
            let mut requested = parse(&value()).unwrap();
            requested.left_tone = bad;
            assert!(
                transaction(Some(&requested), |_| panic!("invalid input reached ATT")).is_err()
            );
        }
    }

    #[test]
    fn discovery_uses_uuids_and_never_the_captured_handle() {
        let mut exchanges = discovery().into_iter();
        assert_eq!(
            discover(&mut |request| {
                let (expected, response) = exchanges.next().unwrap();
                assert_eq!(request, expected);
                Ok(response)
            })
            .unwrap(),
            Some((0x35, 0x12))
        );
        assert!(exchanges.next().is_none());
        assert_eq!(discover(&mut |_| Ok(vec![1, 6, 1, 0, 0x0a])).unwrap(), None);
    }

    #[test]
    fn attribute_not_found_must_match_the_current_discovery_range() {
        let request = [0x08, 0x2a, 0x00, 0x2b, 0x00, 0x03, 0x28];
        for handle in [0x2a, 0x2b] {
            assert!(no_attribute(&[1, 8, handle, 0, 0x0a], &request));
        }
        for response in [
            vec![1, 8, 0x29, 0, 0x0a],
            vec![1, 8, 0x2c, 0, 0x0a],
            vec![1, 8, 0, 0, 0x0a],
            vec![1, 6, 0x2a, 0, 0x0a],
            vec![2, 8, 0x2a, 0, 0x0a],
            vec![1, 8, 0x2a, 0, 0x05],
            vec![1, 8, 0x2a, 0],
            vec![1, 8, 0x2a, 0, 0x0a, 0],
        ] {
            assert!(!no_attribute(&response, &request));
        }
        for request in [
            vec![],
            vec![8, 0x2a, 0, 0x2b],
            vec![0x0a, 0x2a, 0, 0x2b, 0],
            vec![8, 0, 0, 0x2b, 0],
            vec![8, 0x2b, 0, 0x2a, 0],
        ] {
            assert!(!no_attribute(&[1, 8, 0x2a, 0, 0x0a], &request));
        }
    }

    #[test]
    fn captured_tail_error_preserves_the_discovered_setting_and_allows_readback() {
        let mut service = vec![0x06, 1, 0, 0xff, 0xff, 0, 0x28];
        service.extend_from_slice(&SERVICE);
        let mut characteristics = declaration(0x18);
        characteristics[4] = 0x16;
        let mut later = declaration(0x2a);
        later[7] ^= 1;
        characteristics.extend_from_slice(&later[2..]);
        let expected = parse(&value()).unwrap();
        let mut response = vec![0x0b];
        response.extend_from_slice(&value());
        let mut exchanges = vec![
            (service, vec![7, 0x12, 0, 0x2b, 0]),
            (vec![8, 0x12, 0, 0x2b, 0, 3, 0x28], characteristics),
            (
                vec![8, 0x2a, 0, 0x2b, 0, 3, 0x28],
                vec![1, 8, 0x2b, 0, 0x0a],
            ),
            (vec![0x0a, 0x18, 0], response),
        ]
        .into_iter();
        assert_eq!(
            transaction(None, |request| {
                let (expected, response) = exchanges.next().unwrap();
                assert_eq!(request, expected);
                Ok(response)
            })
            .unwrap(),
            Some(expected)
        );
        assert!(exchanges.next().is_none());
    }

    #[test]
    fn discovery_rejects_bad_lengths_handles_permissions_and_uuid() {
        for mut malformed in [declaration(0x35), vec![9, 21], vec![9, 8, 1, 2, 3]] {
            if malformed.len() > 3 {
                malformed.pop();
            }
            let mut calls = 0;
            assert!(discover(&mut |_| {
                calls += 1;
                Ok(if calls == 1 {
                    vec![7, 0x30, 0, 0x40, 0]
                } else {
                    malformed.clone()
                })
            })
            .is_err());
        }
        for (declaration_handle, value_handle, permissions) in [
            (0x2f, 0x35, 0x0a),
            (0x34, 0x41, 0x0a),
            (0x34, 0x34, 0x0a),
            (0x34, 0x35, 0x02),
        ] {
            let mut response = declaration(value_handle);
            response[2..4].copy_from_slice(&u16::to_le_bytes(declaration_handle));
            response[4] = permissions;
            let mut calls = 0;
            assert!(discover(&mut |_| {
                calls += 1;
                Ok(if calls == 1 {
                    vec![7, 0x30, 0, 0x40, 0]
                } else {
                    response.clone()
                })
            })
            .is_err());
        }
        let mut exchanges = discovery();
        exchanges[1].1[7] ^= 1;
        let mut exchanges = exchanges.into_iter();
        assert_eq!(
            discover(&mut |_| Ok(exchanges.next().unwrap().1)).unwrap(),
            None
        );
    }

    #[test]
    fn discovery_is_bounded_and_rejects_repeated_characteristics() {
        let mut calls = 0;
        assert!(discover(&mut |request| {
            calls += 1;
            if calls == 1 {
                return Ok(vec![7, 1, 0, 0xff, 0xff]);
            }
            let start = u16::from_le_bytes([request[1], request[2]]);
            let mut response = declaration(start + 1);
            response[7] ^= 1;
            Ok(response)
        })
        .is_err());
        assert_eq!(calls, 17);
        let mut exchanges = discovery();
        exchanges[2].1 = declaration(0x38);
        let mut exchanges = exchanges.into_iter();
        assert!(discover(&mut |_| Ok(exchanges.next().unwrap().1)).is_err());
    }

    #[test]
    fn transaction_reads_before_writing_and_requires_readback() {
        let current = value();
        let mut requested = parse(&current).unwrap();
        requested.enabled = true;
        requested.right_tone = 0.75;
        let mut changed = current.clone();
        patch(&mut changed, &requested);
        let mut write = vec![0x12, 0x35, 0];
        write.extend_from_slice(&changed);
        for readback in [&current, &changed] {
            let mut exchanges = discovery();
            exchanges.extend([
                (vec![0x0a, 0x35, 0], [vec![0x0b], current.clone()].concat()),
                (write.clone(), vec![0x13]),
                (vec![0x0a, 0x35, 0], [vec![0x0b], readback.clone()].concat()),
            ]);
            let mut exchanges = exchanges.into_iter();
            assert_eq!(
                transaction(Some(&requested), |request| {
                    let (expected, response) = exchanges.next().expect("extra ATT write");
                    assert_eq!(request, expected);
                    Ok(response)
                })
                .unwrap(),
                Some(parse(readback).unwrap())
            );
            assert!(exchanges.next().is_none());
        }
    }

    #[test]
    fn captured_write_without_response_property_uses_a_command_then_readback() {
        let mut requested = parse(&value()).unwrap();
        requested.enabled = true;
        let mut changed = value();
        patch(&mut changed, &requested);
        let mut write = vec![0x52, 0x18, 0];
        write.extend_from_slice(&changed);
        let mut exchanges = discovery();
        exchanges[0].1 = vec![0x07, 0x12, 0, 0x2b, 0];
        exchanges[1] = (vec![0x08, 0x12, 0, 0x2b, 0, 3, 0x28], declaration(0x18));
        // Pairing capture frame5106: declaration0017, properties16, value0018.
        exchanges[1].1[4] = 0x16;
        exchanges[2] = (
            vec![0x08, 0x18, 0, 0x2b, 0, 3, 0x28],
            vec![1, 8, 0x18, 0, 0x0a],
        );
        exchanges.extend([
            (vec![0x0a, 0x18, 0], [vec![0x0b], value()].concat()),
            (write, vec![]),
            (vec![0x0a, 0x18, 0], [vec![0x0b], changed].concat()),
        ]);
        let mut exchanges = exchanges.into_iter();
        assert_eq!(
            transaction(Some(&requested), |request| {
                let (expected, response) = exchanges.next().expect("unexpected ACK wait");
                assert_eq!(request, expected);
                Ok(response)
            })
            .unwrap(),
            Some(requested)
        );
        assert!(exchanges.next().is_none());
    }

    #[test]
    fn reads_unchanged_settings_and_failed_reads_never_write() {
        let requested = parse(&value()).unwrap();
        for input in [None, Some(&requested)] {
            let mut exchanges = discovery();
            exchanges.push((vec![0x0a, 0x35, 0], [vec![0x0b], value()].concat()));
            let mut exchanges = exchanges.into_iter();
            assert_eq!(
                transaction(input, |request| {
                    let (expected, response) = exchanges.next().expect("unexpected write");
                    assert_eq!(request, expected);
                    Ok(response)
                })
                .unwrap(),
                Some(requested.clone())
            );
            assert!(exchanges.next().is_none());
        }
        for response in [vec![], vec![0x0b, 0], vec![1, 0x0a, 0x35, 0, 3]] {
            let mut exchanges = discovery().into_iter();
            let mut reads = 0;
            assert!(transaction(Some(&requested), |request| {
                if let Some((_, response)) = exchanges.next() {
                    return Ok(response);
                }
                reads += 1;
                assert_eq!(request, [0x0a, 0x35, 0]);
                Ok(response.clone())
            })
            .is_err());
            assert_eq!(reads, 1);
        }
    }

    #[test]
    fn write_ack_and_readback_must_both_be_valid() {
        let mut requested = parse(&value()).unwrap();
        requested.enabled = true;
        for reply in [vec![], vec![0x13, 0], vec![1, 0x12, 0x35, 0, 3], vec![0x13]] {
            let mut exchanges = discovery().into_iter();
            let mut step = 0;
            assert!(transaction(Some(&requested), |request| {
                if let Some((_, response)) = exchanges.next() {
                    return Ok(response);
                }
                step += 1;
                match step {
                    1 => Ok([vec![0x0b], value()].concat()),
                    2 => {
                        assert_eq!(request[0], 0x12);
                        Ok(reply.clone())
                    }
                    3 => {
                        assert_eq!(request, [0x0a, 0x35, 0]);
                        Ok(vec![0x0b, 0])
                    }
                    _ => panic!("unconfirmed write retried"),
                }
            })
            .is_err());
            assert_eq!(step, if reply == [0x13] { 3 } else { 2 });
        }
    }

    #[test]
    fn hearing_enable_guard_reads_actual_custom_transparency() {
        for enabled in [false, true] {
            let mut bytes = value();
            bytes[..4].copy_from_slice(&(if enabled { 1.0f32 } else { 0.0 }).to_le_bytes());
            let mut exchanges = discovery().into_iter();
            assert_eq!(
                read_enabled(|request| {
                    if let Some((_, response)) = exchanges.next() {
                        return Ok(response);
                    }
                    assert_eq!(request, [0x0a, 0x35, 0]);
                    Ok([vec![0x0b], bytes.clone()].concat())
                })
                .unwrap(),
                enabled
            );
        }
        assert!(!read_enabled(|_| Ok(vec![1, 6, 1, 0, 0x0a])).unwrap());
        assert!(read_enabled(|_| Err("timeout".into())).is_err());
    }
}
