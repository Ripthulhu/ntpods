//! Reads cached AirPods self-check results without starting a diagnostic test.

use crate::{att_settings, driver::Driver, hearing, hearing_diagnostics};
use ntpods_ipc::{HardwareDiagnosticEar, HardwareDiagnostics};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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

fn product_id(model: &str) -> Option<u32> {
    match model {
        "A2931" | "A2699" | "A2698" => Some(0x2014),
        "A3047" | "A3048" | "A3049" => Some(0x2024),
        "A3063" | "A3064" | "A3065" => Some(0x2027),
        _ => None,
    }
}

fn ear(value: &hearing_diagnostics::Ear) -> HardwareDiagnosticEar {
    let raw = value.result.raw;
    HardwareDiagnosticEar {
        measured_at_unix_seconds: value.last_measurement_unix_seconds,
        first_use_unix_seconds: (value.first_use_unix_seconds != 0)
            .then_some(value.first_use_unix_seconds),
        general: (raw & 7) as u8,
        measurement: ((raw >> 3) & 7) as u8,
        component_failures: value
            .result
            .component_flags
            .as_ref()
            .map(|_| (raw >> 7) as u8),
        unknown_bits: value.result.uninterpreted_bits | (raw & 0x8000),
    }
}

fn transaction(
    model: &str,
    mut exchange: impl FnMut(&[u8]) -> Result<Vec<u8>, String>,
) -> Result<Option<HardwareDiagnostics>, String> {
    let product =
        product_id(model).ok_or("Hardware diagnostics are not available on these AirPods.")?;
    let Some(characteristic) = att_settings::discover(
        &mut exchange,
        &hearing_diagnostics::UUID,
        "Hardware diagnostics",
    )?
    else {
        return Ok(None);
    };
    let handle = characteristic.handle;
    let response = exchange(&[0x0a, handle as u8, (handle >> 8) as u8])?;
    let Some(value) = response.strip_prefix(&[0x0b]) else {
        return Err(hearing::att_error(
            "Could not read hardware diagnostics.",
            &response,
        ));
    };
    let record = hearing_diagnostics::parse(value, Some(product))?;
    let read_at_unix_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "The Windows clock is unavailable.")?
        .as_secs();
    Ok(Some(HardwareDiagnostics {
        read_at_unix_seconds,
        left: ear(&record.left),
        right: ear(&record.right),
    }))
}

pub fn read(
    drv: &Driver,
    token: Token,
    model: &str,
) -> Result<Option<HardwareDiagnostics>, String> {
    match hearing::att_transaction(
        drv,
        || token.is_current(),
        |exchange| transaction(model, exchange),
    )? {
        Some(Some(record)) => Ok(Some(record)),
        Some(None) => Err("Hardware diagnostics are not available on these AirPods.".into()),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value() -> Vec<u8> {
        let mut value = vec![0; 71];
        value[0] = 8;
        value[1..9].copy_from_slice(&1_700_000_001u64.to_le_bytes());
        value[9..17].copy_from_slice(&1_700_000_002u64.to_le_bytes());
        value
    }

    fn discovery() -> Vec<(Vec<u8>, Vec<u8>)> {
        let mut service = vec![0x06, 1, 0, 0xff, 0xff, 0, 0x28];
        service.extend_from_slice(&att_settings::SERVICE);
        let mut declaration = vec![0x09, 21, 0x34, 0, 0x02, 0x35, 0];
        declaration.extend_from_slice(&hearing_diagnostics::UUID);
        vec![
            (service, vec![0x07, 0x30, 0, 0x40, 0]),
            (vec![0x08, 0x30, 0, 0x40, 0, 3, 0x28], declaration),
            (
                vec![0x08, 0x35, 0, 0x40, 0, 3, 0x28],
                vec![1, 8, 0x35, 0, 0x0a],
            ),
        ]
    }

    #[test]
    fn only_known_pro_models_supply_a_component_product_id() {
        for (models, expected) in [
            (["A2931", "A2699", "A2698"], 0x2014),
            (["A3047", "A3048", "A3049"], 0x2024),
            (["A3063", "A3064", "A3065"], 0x2027),
        ] {
            for model in models {
                assert_eq!(product_id(model), Some(expected));
            }
        }
        for model in ["", "A2084", "A3050", "AirPods Pro", "A3063 ", "unknown"] {
            assert!(transaction(model, |_| panic!("unsupported model reached ATT")).is_err());
        }
    }

    #[test]
    fn projection_keeps_status_and_completion_separate_and_retains_unknown_flags() {
        let mut value = value();
        value[17..21].copy_from_slice(&0x1234_8181u32.to_le_bytes());
        value[21..25].copy_from_slice(&0x4018u32.to_le_bytes());
        value[33..37].copy_from_slice(&1_600_000_001u32.to_le_bytes());
        let record = hearing_diagnostics::parse(&value, Some(0x2027)).unwrap();
        let left = ear(&record.left);
        assert_eq!(left.measured_at_unix_seconds, 1_700_000_001);
        assert_eq!(left.first_use_unix_seconds, Some(1_600_000_001));
        assert_eq!((left.general, left.measurement), (1, 0));
        assert_eq!(left.component_failures, Some(3));
        assert_eq!(left.unknown_bits, 0x1234_8000);
        let right = ear(&record.right);
        assert_eq!(right.first_use_unix_seconds, None);
        assert_eq!((right.general, right.measurement), (0, 3));
        assert_eq!(right.component_failures, None);
        value[1..9].fill(0);
        let empty = hearing_diagnostics::parse(&value, Some(0x2027)).unwrap();
        assert_eq!(ear(&empty.left).component_failures, None);
    }

    #[test]
    fn reads_only_the_discovered_handle_without_writes_or_subscriptions() {
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let mut exchanges = discovery();
        exchanges.push((vec![0x0a, 0x35, 0], [vec![0x0b], value()].concat()));
        let mut exchanges = exchanges.into_iter();
        let record = transaction("A3063", |request| {
            assert!(matches!(request[0], 0x06 | 0x08 | 0x0a));
            let (expected, response) = exchanges.next().expect("extra ATT operation");
            assert_eq!(request, expected);
            Ok(response)
        })
        .unwrap()
        .unwrap();
        assert!(exchanges.next().is_none());
        let after = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!((before..=after).contains(&record.read_at_unix_seconds));
        assert_eq!(record.left.component_failures, Some(0));
        assert_eq!(record.right.measured_at_unix_seconds, 1_700_000_002);
    }

    #[test]
    fn absent_service_stops_discovery_without_reading() {
        let mut requests = 0;
        let result = transaction("A3063", |request| {
            requests += 1;
            assert_eq!(request[0], 0x06);
            Ok(vec![1, 6, 1, 0, 0x0a])
        });
        assert_eq!(result.unwrap(), None);
        assert_eq!(requests, 1);
    }

    #[test]
    fn malformed_or_failed_reads_are_not_retried_or_published() {
        for response in [
            vec![],
            vec![0x0b, 8],
            vec![0x0b; 73],
            vec![1, 0x0a, 0x35, 0, 3],
        ] {
            let mut exchanges = discovery().into_iter();
            let mut reads = 0;
            assert!(transaction("A3063", |request| {
                if let Some((expected, response)) = exchanges.next() {
                    assert_eq!(request, expected);
                    return Ok(response);
                }
                reads += 1;
                assert_eq!(request, [0x0a, 0x35, 0]);
                Ok(response.clone())
            })
            .is_err());
            assert_eq!(reads, 1);
        }
        assert!(transaction("A3063", |_| Err("connection changed".into())).is_err());
    }
}
