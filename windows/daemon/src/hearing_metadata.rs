//! Readable hearing metadata; raw dosimetry values do not establish sound pressure.

use crate::{att_settings, driver::Driver, hearing};
use ntpods_ipc::HearingMetadata;
use std::time::{SystemTime, UNIX_EPOCH};

const MODE: [u8; 16] = 0xa4120001_95c5_4d6f_9098_0f0b41457e0au128.to_le_bytes();
const SENSITIVITY: [u8; 16] = 0x82f6bc0a_1bca_4873_afc9_ec5890e3469au128.to_le_bytes();
const VOLUME_CURVE: [u8; 16] = 0xd5f96afa_2f2c_41bb_a7e6_f54abe6235b4u128.to_le_bytes();
const CHARACTERISTICS: [[u8; 16]; 3] = [MODE, SENSITIVITY, VOLUME_CURVE];

fn floats<const N: usize>(value: &[u8]) -> Result<[f32; N], String> {
    if value.len() != 4 * N {
        return Err("AirPods returned an unsupported dosimetry value length.".into());
    }
    let values = std::array::from_fn(|index| {
        f32::from_le_bytes(value[index * 4..index * 4 + 4].try_into().unwrap())
    });
    if values.iter().any(|value| !value.is_finite()) {
        return Err("AirPods returned a non-finite dosimetry value.".into());
    }
    Ok(values)
}

fn transaction(
    current: impl Fn() -> bool,
    mut exchange: impl FnMut(&[u8]) -> Result<Vec<u8>, String>,
) -> Result<HearingMetadata, String> {
    let check = || {
        current()
            .then_some(())
            .ok_or_else(|| "AirPods metadata request is no longer current.".to_string())
    };
    check()?;
    let found = att_settings::discover_many(
        &mut |request| {
            check()?;
            exchange(request)
        },
        &CHARACTERISTICS,
        "Hearing metadata",
    )?;
    let mut result = HearingMetadata::default();
    for (index, characteristic) in found.into_iter().enumerate() {
        let Some(characteristic) = characteristic else {
            continue;
        };
        check()?;
        let handle = characteristic.handle.to_le_bytes();
        let response = exchange(&[0x0a, handle[0], handle[1]])?;
        check()?;
        let Some(value) = response.strip_prefix(&[0x0b]) else {
            return Err(hearing::att_error(
                "Could not read hearing metadata.",
                &response,
            ));
        };
        match index {
            0 => match value {
                [mode] => result.test_mode_raw = Some(*mode),
                _ => return Err("AirPods returned an unsupported hearing-test mode length.".into()),
            },
            1 => result.dosimetry_sensitivity_raw = Some(floats::<1>(value)?[0]),
            2 => result.dosimetry_volume_curve_raw = Some(floats::<17>(value)?),
            _ => unreachable!(),
        }
    }
    check()?;
    result.read_at_unix_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "The Windows clock is unavailable.")?
        .as_secs();
    Ok(result)
}

pub fn read(drv: &Driver, current: impl Fn() -> bool) -> Result<Option<HearingMetadata>, String> {
    let session = hearing::session();
    let current = || current() && hearing::session() == session;
    hearing::att_transaction(drv, &current, |exchange| transaction(&current, exchange))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, collections::VecDeque};

    fn discovery(entries: &[(u16, u8, u16, [u8; 16])]) -> VecDeque<(Vec<u8>, Vec<u8>)> {
        let mut service = vec![0x06, 1, 0, 0xff, 0xff, 0, 0x28];
        service.extend_from_slice(&att_settings::SERVICE);
        let mut list = vec![0x09, 21];
        for (declaration, properties, handle, uuid) in entries {
            list.extend_from_slice(&declaration.to_le_bytes());
            list.push(*properties);
            list.extend_from_slice(&handle.to_le_bytes());
            list.extend_from_slice(uuid);
        }
        let next = entries.last().unwrap().0 + 1;
        VecDeque::from([
            (service, vec![0x07, 0x12, 0, 0x40, 0]),
            (vec![0x08, 0x12, 0, 0x40, 0, 3, 0x28], list),
            (
                vec![0x08, next as u8, (next >> 8) as u8, 0x40, 0, 3, 0x28],
                vec![1, 8, 0x40, 0, 0x0a],
            ),
        ])
    }

    fn all() -> VecDeque<(Vec<u8>, Vec<u8>)> {
        discovery(&[
            (0x13, 2, 0x14, SENSITIVITY),
            (0x15, 2, 0x16, VOLUME_CURVE),
            (0x20, 0x16, 0x21, MODE),
        ])
    }

    fn response(values: &[f32]) -> Vec<u8> {
        [
            vec![0x0b],
            values
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect(),
        ]
        .concat()
    }

    #[test]
    fn reads_all_fields_in_one_discovery_and_keeps_unknown_mode_and_raw_floats() {
        let curve = std::array::from_fn::<_, 17, _>(|index| index as f32 - 8.0);
        let mut script = all();
        script.extend([
            (vec![0x0a, 0x21, 0], vec![0x0b, 0xfe]),
            (vec![0x0a, 0x14, 0], response(&[-0.0])),
            (vec![0x0a, 0x16, 0], response(&curve)),
        ]);
        let metadata = transaction(
            || true,
            |request| {
                assert!(matches!(request[0], 0x06 | 0x08 | 0x0a));
                let (expected, response) = script.pop_front().expect("extra ATT operation");
                assert_eq!(request, expected);
                Ok(response)
            },
        )
        .unwrap();
        assert!(script.is_empty());
        assert_eq!(metadata.test_mode_raw, Some(0xfe));
        assert_eq!(
            metadata.dosimetry_sensitivity_raw.unwrap().to_bits(),
            (-0.0f32).to_bits()
        );
        assert_eq!(metadata.dosimetry_volume_curve_raw, Some(curve));
        assert!(metadata.read_at_unix_seconds > 0);
    }

    #[test]
    fn changed_handles_are_rediscovered_and_absent_uuids_do_not_get_read() {
        let mut script = discovery(&[(0x30, 2, 0x31, MODE)]);
        script.push_back((vec![0x0a, 0x31, 0], vec![0x0b, 0]));
        let metadata = transaction(
            || true,
            |request| {
                let (expected, response) = script.pop_front().unwrap();
                assert_eq!(request, expected);
                Ok(response)
            },
        )
        .unwrap();
        assert!(script.is_empty());
        assert_eq!(metadata.test_mode_raw, Some(0));
        assert!(metadata.dosimetry_sensitivity_raw.is_none());
        assert!(metadata.dosimetry_volume_curve_raw.is_none());
    }

    #[test]
    fn missing_service_and_unknown_characteristics_produce_no_attribute_reads() {
        let mut count = 0;
        let metadata = transaction(
            || true,
            |request| {
                count += 1;
                assert_eq!(request[0], 0x06);
                Ok(vec![1, 6, 1, 0, 0x0a])
            },
        )
        .unwrap();
        assert_eq!(count, 1);
        assert!(metadata.test_mode_raw.is_none());
        let mut script = discovery(&[(0x30, 4, 0x31, [0x55; 16])]);
        transaction(
            || true,
            |request| {
                let (expected, response) = script
                    .pop_front()
                    .expect("read an unrelated characteristic");
                assert_eq!(request, expected);
                Ok(response)
            },
        )
        .unwrap();
        assert!(script.is_empty());
    }

    #[test]
    fn changed_unreadable_or_duplicate_declarations_fail_before_reads() {
        for entries in [
            vec![(0x13, 4, 0x14, SENSITIVITY)],
            vec![(0x13, 2, 0x14, MODE), (0x15, 2, 0x16, MODE)],
            vec![(0x13, 2, 0x13, MODE)],
        ] {
            let mut script = discovery(&entries);
            assert!(transaction(
                || true,
                |request| {
                    assert_ne!(request[0], 0x0a);
                    let (expected, response) = script.pop_front().unwrap();
                    assert_eq!(request, expected);
                    Ok(response)
                }
            )
            .is_err());
        }
    }

    #[test]
    fn malformed_truncated_and_nonfinite_values_cannot_publish_a_bundle() {
        for (uuid, bad) in [
            (MODE, vec![0x0b]),
            (MODE, vec![0x0b, 0, 1]),
            (SENSITIVITY, vec![0x0b, 1, 2, 3]),
            (SENSITIVITY, response(&[f32::NAN])),
            (SENSITIVITY, response(&[f32::INFINITY])),
            (VOLUME_CURVE, response(&[0.0; 16])),
            (VOLUME_CURVE, response(&[0.0; 18])),
            (VOLUME_CURVE, response(&[f32::NEG_INFINITY; 17])),
            (MODE, vec![1, 0x0a, 0x14, 0, 5]),
        ] {
            let mut script = discovery(&[(0x13, 2, 0x14, uuid)]);
            script.push_back((vec![0x0a, 0x14, 0], bad));
            assert!(transaction(
                || true,
                |request| {
                    let (expected, response) =
                        script.pop_front().expect("retry after malformed value");
                    assert_eq!(request, expected);
                    Ok(response)
                }
            )
            .is_err());
            assert!(script.is_empty());
        }
    }

    #[test]
    fn interrupted_or_stale_transactions_stop_without_partial_results() {
        assert!(transaction(|| false, |_| panic!("stale request sent ATT")).is_err());
        for cancel_after in 1..=6 {
            let active = Cell::new(true);
            let mut script = all();
            script.extend([
                (vec![0x0a, 0x21, 0], vec![0x0b, 0]),
                (vec![0x0a, 0x14, 0], response(&[98.0])),
                (vec![0x0a, 0x16, 0], response(&[0.0; 17])),
            ]);
            let calls = Cell::new(0);
            assert!(transaction(
                || active.get(),
                |request| {
                    let (expected, response) = script.pop_front().unwrap();
                    assert_eq!(request, expected);
                    calls.set(calls.get() + 1);
                    if calls.get() == cancel_after {
                        active.set(false);
                    }
                    Ok(response)
                }
            )
            .is_err());
            assert_eq!(calls.get(), cancel_after);
        }
        let mut script = all();
        let mut failed = false;
        assert!(transaction(
            || true,
            |request| {
                assert!(!failed, "exchanged again after transport failure");
                if let Some((expected, response)) = script.pop_front() {
                    assert_eq!(request, expected);
                    Ok(response)
                } else {
                    failed = true;
                    Err("read timed out".into())
                }
            }
        )
        .is_err());
        assert!(failed);
    }
}
