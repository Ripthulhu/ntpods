//! Cached AHPS diagnostics; see docs/WINDOWS-AIRPODS-CONTROL-PROTOCOL.md.

use serde::Serialize;

pub const UUID: [u8; 16] = 0xa4120002_95c5_4d6f_9098_0f0b41457e0au128.to_le_bytes();

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Record {
    pub version: u8,
    pub left: Ear,
    pub right: Ear,
    pub component_diagnostics_supported: Option<bool>,
    pub anc_scores_supported: Option<bool>,
    pub anc_scores_raw: [u8; 8],
    pub opaque_tail: [u8; 30],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Ear {
    // Firmware clock alignment is not established; preserve these seconds exactly.
    pub last_measurement_unix_seconds: u64,
    pub first_use_unix_seconds: u32,
    pub result: ResultWord,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ResultWord {
    pub raw: u32,
    pub general: GeneralStatus,
    pub measurement: MeasurementStatus,
    pub component_flags: Option<ComponentFlags>,
    pub anc_loss_selector_raw: bool,
    pub uninterpreted_bits: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GeneralStatus {
    Pass,
    Fail,
    NoDataOutOfFactoryLessThanAYear,
    NoUtpConnection,
    NoDataOutOfFactoryMoreThanAYear,
    Unknown(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MeasurementStatus {
    SuccessHarmonic,
    SuccessNonHarmonic,
    LidOpenAbort,
    MotionAbort,
    TempAbort,
    NoiseAbort,
    SystemAbort,
    NoStatus,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ComponentFlags {
    // True means the saved device result has this failure flag, not a current diagnosis.
    pub total_harmonic_distortion: bool,
    pub frequency_accuracy: bool,
    pub speaker: bool,
    pub rear_vent: bool,
    pub front_vent: bool,
    pub bottom_mic: bool,
    pub top_mic: bool,
    pub inner_mic: bool,
}

pub fn parse(value: &[u8], product_id: Option<u32>) -> Result<Record, String> {
    if value.len() != 71 || value[0] != 8 {
        return Err(
            "Unsupported hardware diagnostic record; expected version 8 and 71 bytes".into(),
        );
    }
    let components = product_id.map(|id| matches!(id, 0x2014 | 0x2024 | 0x2027));
    let ear = |timestamp_offset, result_offset, first_use_offset| {
        let timestamp = u64::from_le_bytes(
            value[timestamp_offset..timestamp_offset + 8]
                .try_into()
                .unwrap(),
        );
        let raw = u32::from_le_bytes(value[result_offset..result_offset + 4].try_into().unwrap());
        Ear {
            last_measurement_unix_seconds: timestamp,
            first_use_unix_seconds: u32::from_le_bytes(
                value[first_use_offset..first_use_offset + 4]
                    .try_into()
                    .unwrap(),
            ),
            result: decode_word(raw, components == Some(true) && timestamp != 0),
        }
    };
    Ok(Record {
        version: 8,
        left: ear(1, 17, 33),
        right: ear(9, 21, 37),
        component_diagnostics_supported: components,
        anc_scores_supported: product_id.map(|id| id == 0x201b),
        anc_scores_raw: value[25..33].try_into().unwrap(),
        opaque_tail: value[41..71].try_into().unwrap(),
    })
}

fn decode_word(raw: u32, components_supported: bool) -> ResultWord {
    let general = match raw & 7 {
        0 => GeneralStatus::Pass,
        1 => GeneralStatus::Fail,
        2 => GeneralStatus::NoDataOutOfFactoryLessThanAYear,
        3 => GeneralStatus::NoUtpConnection,
        4 => GeneralStatus::NoDataOutOfFactoryMoreThanAYear,
        other => GeneralStatus::Unknown(other as u8),
    };
    let measurement = match (raw >> 3) & 7 {
        0 => MeasurementStatus::SuccessHarmonic,
        1 => MeasurementStatus::SuccessNonHarmonic,
        2 => MeasurementStatus::LidOpenAbort,
        3 => MeasurementStatus::MotionAbort,
        4 => MeasurementStatus::TempAbort,
        5 => MeasurementStatus::NoiseAbort,
        6 => MeasurementStatus::SystemAbort,
        _ => MeasurementStatus::NoStatus,
    };
    let component_flags = (components_supported && ((raw >> 3) & 7) < 2).then(|| ComponentFlags {
        total_harmonic_distortion: raw & (1 << 7) != 0,
        frequency_accuracy: raw & (1 << 8) != 0,
        speaker: raw & (1 << 9) != 0,
        rear_vent: raw & (1 << 10) != 0,
        front_vent: raw & (1 << 11) != 0,
        bottom_mic: raw & (1 << 12) != 0,
        top_mic: raw & (1 << 13) != 0,
        inner_mic: raw & (1 << 14) != 0,
    });
    ResultWord {
        raw,
        general,
        measurement,
        component_flags,
        anc_loss_selector_raw: raw & (1 << 15) != 0,
        uninterpreted_bits: raw & !0xffbf,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> [u8; 71] {
        let mut value = [0; 71];
        value[0] = 8;
        value[1..9].copy_from_slice(&1_700_000_001u64.to_le_bytes());
        value[9..17].copy_from_slice(&1_700_000_002u64.to_le_bytes());
        value
    }

    #[test]
    fn rejects_unverified_versions_and_lengths() {
        for length in [0, 1, 40, 70, 72, 100] {
            assert!(parse(&vec![8; length], Some(0x2027)).is_err());
        }
        for version in [0, 1, 4, 7, 9, 255] {
            let mut value = fixture();
            value[0] = version;
            assert!(parse(&value, Some(0x2027)).is_err());
        }
    }

    #[test]
    fn preserves_full_width_dates_and_opaque_bytes() {
        let mut value = fixture();
        value[1..9].copy_from_slice(&u64::MAX.to_le_bytes());
        value[33..37].copy_from_slice(&u32::MAX.to_le_bytes());
        value[37..41].copy_from_slice(&1_600_000_002u32.to_le_bytes());
        value[25..33].copy_from_slice(&[0, 1, 2, 3, 4, 5, 6, 255]);
        value[41..71].fill(0xa5);
        let record = parse(&value, Some(0x2027)).unwrap();
        assert_eq!(record.left.last_measurement_unix_seconds, u64::MAX);
        assert_eq!(record.right.last_measurement_unix_seconds, 1_700_000_002);
        assert_eq!(record.left.first_use_unix_seconds, u32::MAX);
        assert_eq!(record.right.first_use_unix_seconds, 1_600_000_002);
        assert_eq!(record.anc_scores_raw, [0, 1, 2, 3, 4, 5, 6, 255]);
        assert_eq!(record.opaque_tail, [0xa5; 30]);
    }

    #[test]
    fn keeps_completion_separate_from_general_status_and_component_flags() {
        let mut value = fixture();
        value[17..21].copy_from_slice(&0x81u32.to_le_bytes());
        value[21..25].copy_from_slice(&(0x4000u32 | (3 << 3)).to_le_bytes());
        let record = parse(&value, Some(0x2027)).unwrap();
        assert_eq!(record.left.result.general, GeneralStatus::Fail);
        assert_eq!(
            record.left.result.measurement,
            MeasurementStatus::SuccessHarmonic
        );
        assert!(
            record
                .left
                .result
                .component_flags
                .unwrap()
                .total_harmonic_distortion
        );
        assert_eq!(
            record.right.result.measurement,
            MeasurementStatus::MotionAbort
        );
        assert!(record.right.result.component_flags.is_none());
        assert_eq!(record.right.result.raw, 0x4018);
    }

    #[test]
    fn component_flags_require_supported_product_dated_completed_measurement() {
        for product_id in [None, Some(0x201b), Some(0x200e), Some(0xffffffff)] {
            assert!(parse(&fixture(), product_id)
                .unwrap()
                .left
                .result
                .component_flags
                .is_none());
        }
        for product_id in [0x2014, 0x2024, 0x2027] {
            for measurement in 0u32..8 {
                let mut value = fixture();
                value[17..21].copy_from_slice(&(measurement << 3u32).to_le_bytes());
                let record = parse(&value, Some(product_id)).unwrap();
                assert_eq!(
                    record.left.result.component_flags.is_some(),
                    measurement < 2
                );
            }
        }
        let mut empty = [0; 71];
        empty[0] = 8;
        let record = parse(&empty, Some(0x2027)).unwrap();
        assert_eq!(record.left.last_measurement_unix_seconds, 0);
        assert!(record.left.result.component_flags.is_none());
        assert_eq!(record.component_diagnostics_supported, Some(true));
        assert_eq!(record.anc_scores_supported, Some(false));
        assert_eq!(
            parse(&fixture(), Some(0x201b))
                .unwrap()
                .anc_scores_supported,
            Some(true)
        );
        assert_eq!(parse(&fixture(), None).unwrap().anc_scores_supported, None);
    }

    #[test]
    fn retains_unknown_statuses_and_bits() {
        for general in 5..8 {
            assert_eq!(
                decode_word(general, false).general,
                GeneralStatus::Unknown(general as u8)
            );
        }
        let word = decode_word(0xffff8040, true);
        assert_eq!(word.raw, 0xffff8040);
        assert_eq!(word.uninterpreted_bits, 0xffff0040);
        assert!(word.anc_loss_selector_raw);
        assert_eq!(
            UUID,
            [
                0x0a, 0x7e, 0x45, 0x41, 0x0b, 0x0f, 0x98, 0x90, 0x6f, 0x4d, 0xc5, 0x95, 2, 0, 0x12,
                0xa4
            ]
        );
    }
}
