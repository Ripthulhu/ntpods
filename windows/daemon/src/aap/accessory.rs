//! Read-only service-4 accessory reports. Unknown fields never grant feature or audio permission.

#[derive(Debug, PartialEq, Eq)]
pub enum Report<'a> {
    BudRole {
        primary: u8,
        flags: [u8; 3],
    },
    CaseInfo(&'a [u8]),
    AdaptiveVolumeDrc {
        mode: u8,
        version: u8,
    },
    AdaptiveVolumeTargets {
        media: u8,
        telephony: u8,
        voice_command: u8,
        ramp_up_ms: u16,
        ramp_down_ms: u16,
    },
    AdaptiveVolume(&'a [u8]),
    Unknown55([u8; 4]),
}

/// Case metadata only; these fields do not identify or authorize a BLE case.
#[derive(Debug, PartialEq, Eq)]
pub struct CaseInfoView<'a> {
    pub message_version: u8,
    pub vendor_id: u16,
    pub product_id: u32,
    pub vendor_id_source: u16,
    pub case_color: u8,
    pub case_version: u64,
    pub reserved: u32,
    pub name: &'a str,
}

impl<'a> CaseInfoView<'a> {
    pub fn parse(body: &'a [u8]) -> Option<Self> {
        // shortcut: decode only captured v3/25-byte bodies; validate other forms before extending.
        if body.len() != 25 || body[0] != 3 {
            return None;
        }
        let extension = 14usize.checked_add(body[12] as usize)?;
        if extension.checked_add(10)? != body.len() {
            return None;
        }
        let name = body.get(13..extension - 1)?;
        if body[extension - 1] != 0 || name.contains(&0) {
            return None;
        }
        // The low version word precedes the name; the remaining six bytes follow its NUL.
        let case_version = u64::from_le_bytes([
            body[10],
            body[11],
            body[extension],
            body[extension + 1],
            body[extension + 2],
            body[extension + 3],
            body[extension + 4],
            body[extension + 5],
        ]);
        Some(Self {
            message_version: body[0],
            vendor_id: u16::from_le_bytes(body[1..3].try_into().ok()?),
            product_id: u32::from_le_bytes(body[3..7].try_into().ok()?),
            vendor_id_source: u16::from_le_bytes(body[7..9].try_into().ok()?),
            case_color: body[9],
            case_version,
            reserved: u32::from_le_bytes(body[extension + 6..extension + 10].try_into().ok()?),
            name: std::str::from_utf8(name).ok()?,
        })
    }
}

pub fn parse(data: &[u8]) -> Option<Report<'_>> {
    if data.len() < 6 || data[..4] != super::HEADER || data[5] != 0 {
        return None;
    }
    let body = &data[6..];
    match data[4] {
        0x08 if body.len() == 4 => Some(Report::BudRole {
            primary: body[0],
            flags: body[1..].try_into().ok()?,
        }),
        // Only the 25-byte CaseInfo form is established by our Pro 3 captures.
        0x23 if body.len() == 25 => Some(Report::CaseInfo(body)),
        0x4c if body.len() >= 2 => {
            let length = u16::from_le_bytes(body[..2].try_into().ok()?) as usize;
            if length > 256 || body.len() != length + 2 {
                return None;
            }
            Some(match &body[2..] {
                [4, mode, version] => Report::AdaptiveVolumeDrc {
                    mode: *mode,
                    version: *version,
                },
                [1, media, telephony, voice_command, up_lo, up_hi, down_lo, down_hi] => {
                    Report::AdaptiveVolumeTargets {
                        media: *media,
                        telephony: *telephony,
                        voice_command: *voice_command,
                        ramp_up_ms: u16::from_le_bytes([*up_lo, *up_hi]),
                        ramp_down_ms: u16::from_le_bytes([*down_lo, *down_hi]),
                    }
                }
                payload => Report::AdaptiveVolume(payload),
            })
        }
        0x55 if body.len() == 4 => Some(Report::Unknown55(body.try_into().ok()?)),
        _ => None,
    }
}

impl Report<'_> {
    /// Routine logs omit case names, opaque identity and unassigned payload bytes.
    pub fn summary(&self) -> String {
        match self {
            Self::BudRole { primary, .. } => format!("bud role primary={primary}"),
            Self::CaseInfo(body) => match CaseInfoView::parse(body) {
                Some(info) => format!(
                    "case info v={} vendor={:04x} product={:08x} source={} color={} version={} reserved={:08x} name_bytes={}",
                    info.message_version, info.vendor_id, info.product_id, info.vendor_id_source,
                    info.case_color, info.case_version, info.reserved, info.name.len()
                ),
                None => format!("case info bytes={}", body.len()),
            },
            Self::AdaptiveVolumeDrc { mode, version } => {
                format!("adaptive volume DRC mode={mode} version={version}")
            }
            Self::AdaptiveVolumeTargets {
                media,
                telephony,
                voice_command,
                ramp_up_ms,
                ramp_down_ms,
            } => format!(
                "adaptive volume targets media={media} telephony={telephony} voice_command={voice_command} ramp_up_ms={ramp_up_ms} ramp_down_ms={ramp_down_ms}"
            ),
            Self::AdaptiveVolume(body) => format!("adaptive volume bytes={}", body.len()),
            Self::Unknown55(_) => "opcode 0x55 bytes=4".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(op: u8, body: &[u8]) -> Vec<u8> {
        [vec![4, 0, 4, 0, op, 0], body.to_vec()].concat()
    }

    #[test]
    fn role_preserves_unassigned_side_and_flags_without_changing_policy() {
        assert_eq!(
            parse(&packet(8, &[0xfe, 0x80, 3, 4])),
            Some(Report::BudRole {
                primary: 0xfe,
                flags: [0x80, 3, 4]
            })
        );
        assert!(parse(&packet(8, &[1, 0, 1])).is_none());
        assert!(parse(&packet(8, &[1, 0, 1, 0, 0])).is_none());
    }

    #[test]
    fn opaque_case_and_setup_reports_preserve_bytes_but_do_not_log_them() {
        let case = packet(0x23, &[0xab; 25]);
        let report = parse(&case).unwrap();
        assert_eq!(report, Report::CaseInfo(&[0xab; 25]));
        assert_eq!(report.summary(), "case info bytes=25");
        assert!(parse(&case[..case.len() - 1]).is_none());
        assert_eq!(
            parse(&packet(0x55, &[1, 1, 0, 0x19])),
            Some(Report::Unknown55([1, 1, 0, 0x19]))
        );
    }

    fn case_v3() -> [u8; 25] {
        // Captured layout and non-unique metadata, with a replacement name.
        [
            3, 0x4c, 0, 2, 0x32, 0x0c, 0, 1, 0, 0, 0x0c, 0x5a, 1, b'X', 0, 0, 0x15, 0, 0, 0, 0, 0,
            0, 0, 0,
        ]
    }

    #[test]
    fn case_v3_decodes_native_widths_and_keeps_name_out_of_summary() {
        let body = case_v3();
        assert_eq!(
            CaseInfoView::parse(&body),
            Some(CaseInfoView {
                message_version: 3,
                vendor_id: 0x004c,
                product_id: 0x000c3202,
                vendor_id_source: 1,
                case_color: 0,
                case_version: 0x15005a0c,
                reserved: 0,
                name: "X",
            })
        );
        let bytes = packet(0x23, &body);
        let report = parse(&bytes).unwrap();
        assert_eq!(report, Report::CaseInfo(&body));
        let summary = report.summary();
        assert_eq!(summary, "case info v=3 vendor=004c product=000c3202 source=1 color=0 version=352344588 reserved=00000000 name_bytes=1");
        assert!(!summary.contains('X'));
    }

    #[test]
    fn case_v3_preserves_full_width_unassigned_metadata() {
        let mut body = case_v3();
        body[1..10].fill(0xff);
        body[10..12].copy_from_slice(&[0x11, 0x22]);
        body[15..21].copy_from_slice(&[0x33, 0x44, 0x55, 0x66, 0x77, 0x88]);
        body[21..25].copy_from_slice(&[0x99, 0xaa, 0xbb, 0xcc]);
        let info = CaseInfoView::parse(&body).unwrap();
        assert_eq!(info.vendor_id, 0xffff);
        assert_eq!(info.product_id, 0xffffffff);
        assert_eq!(info.vendor_id_source, 0xffff);
        assert_eq!(info.case_color, 0xff);
        assert_eq!(info.case_version, 0x8877665544332211);
        assert_eq!(info.reserved, 0xccbbaa99);
    }

    #[test]
    fn case_typed_decoder_rejects_bad_lengths_names_and_utf8_without_losing_raw_report() {
        let body = case_v3();
        for length in 0..body.len() {
            assert!(CaseInfoView::parse(&body[..length]).is_none());
        }
        for length in [26, 256, 4096] {
            let oversized = vec![0; length];
            assert!(CaseInfoView::parse(&oversized).is_none());
            assert!(parse(&packet(0x23, &oversized)).is_none());
        }
        for (offset, value) in [(12, 0), (12, 2), (12, 255), (13, 0), (13, 0xff), (14, 1)] {
            let mut invalid = body;
            invalid[offset] = value;
            assert!(
                CaseInfoView::parse(&invalid).is_none(),
                "offset {offset}, value {value}"
            );
            let bytes = packet(0x23, &invalid);
            let report = parse(&bytes).unwrap();
            assert_eq!(report, Report::CaseInfo(&invalid));
            assert_eq!(report.summary(), "case info bytes=25");
        }
    }

    #[test]
    fn case_unknown_versions_stay_opaque() {
        for version in [0, 1, 2, 4, 0xff] {
            let mut body = case_v3();
            body[0] = version;
            assert!(CaseInfoView::parse(&body).is_none());
            let bytes = packet(0x23, &body);
            let report = parse(&bytes).unwrap();
            assert_eq!(report, Report::CaseInfo(&body));
            assert_eq!(report.summary(), "case info bytes=25");
        }
    }

    #[test]
    fn captured_drc_reports_keep_mode_and_version_as_raw_values() {
        for (mode, version) in [(2, 0), (0x3a, 1), (0xff, 0xfe)] {
            let bytes = packet(0x4c, &[3, 0, 4, mode, version]);
            let report = parse(&bytes).unwrap();
            assert_eq!(report, Report::AdaptiveVolumeDrc { mode, version });
            assert_eq!(
                report.summary(),
                format!("adaptive volume DRC mode={mode} version={version}")
            );
        }
    }

    #[test]
    fn native_targets_layout_preserves_values_and_little_endian_ramps() {
        assert_eq!(
            parse(&packet(
                0x4c,
                &[8, 0, 1, 0x80, 2, 0xff, 0x34, 0x12, 0xcd, 0xab]
            )),
            Some(Report::AdaptiveVolumeTargets {
                media: 0x80,
                telephony: 2,
                voice_command: 0xff,
                ramp_up_ms: 0x1234,
                ramp_down_ms: 0xabcd,
            })
        );
    }

    #[test]
    fn adaptive_volume_rejects_bad_envelopes_and_retains_unknown_forms() {
        for payload in [&[4, 2][..], &[4, 2, 0, 0], &[1, 2], &[0xfe, 0xaa], &[]] {
            let mut body = (payload.len() as u16).to_le_bytes().to_vec();
            body.extend_from_slice(payload);
            let bytes = packet(0x4c, &body);
            assert_eq!(parse(&bytes), Some(Report::AdaptiveVolume(payload)));
        }
        for body in [&[4, 0, 4, 0x3a, 1][..], &[3, 0, 4, 2], &[0]] {
            assert!(parse(&packet(0x4c, body)).is_none());
        }
        let mut oversized = vec![1, 1];
        oversized.resize(259, 0);
        assert!(parse(&packet(0x4c, &oversized)).is_none());
    }

    #[test]
    fn microphone_sensor_and_other_services_are_not_accessory_inventory() {
        for op in [0x17, 0x31, 0x53, 0x58] {
            assert!(parse(&packet(op, &[0; 25])).is_none());
        }
        let mut bytes = packet(2, &[0]);
        bytes[2] = 0;
        assert!(parse(&bytes).is_none());
        bytes[2] = 4;
        bytes[5] = 1;
        assert!(parse(&bytes).is_none());
    }
}
