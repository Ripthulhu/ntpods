//! AACP capability reports use fixed widths per ID, without entry lengths.

pub const REQUEST: [u8; 7] = [4, 0, 4, 0, 1, 0, 8];

pub fn parse(data: &[u8]) -> Option<Vec<(u8, u32)>> {
    if data.len() < 7 || data[..6] != [4, 0, 4, 0, 2, 0] {
        return None;
    }
    let mut values = Vec::with_capacity(data[6] as usize);
    let mut offset = 7;
    for _ in 0..data[6] {
        let id = *data.get(offset)?;
        if values.iter().any(|(previous, _)| *previous == id) {
            return None;
        }
        offset += 1;
        let width = match id {
            0x03..=0x08 | 0x30 => 4,
            0x01
            | 0x02
            | 0x09..=0x0D
            | 0x0F..=0x19
            | 0x20..=0x26
            | 0x28
            | 0x29
            | 0x31..=0x37
            | 0x40
            | 0x50
            | 0x60
            | 0x80
            | 0x90
            | 0xA0
            | 0xB0
            | 0xC0
            | 0xD0
            | 0xE0
            | 0xF0 => 1,
            // An unknown width cannot be skipped without misreading later entries.
            _ => return None,
        };
        let value = data.get(offset..offset + width)?;
        let value = if width == 4 {
            u32::from_le_bytes(value.try_into().ok()?)
        } else {
            value[0].into()
        };
        values.push((id, value));
        offset += width;
    }
    (offset == data.len()).then_some(values)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Pro 3 / 9A348, captured Tahoe capability response; contains no identity data.
    const CAPTURED: &[u8] = &[
        0x04, 0x00, 0x04, 0x00, 0x02, 0x00, 0x29, 0x01, 0x0E, 0x02, 0x02, 0x04, 0x01, 0x00, 0x00,
        0x00, 0x10, 0x02, 0x40, 0x01, 0x03, 0x02, 0x00, 0x00, 0x00, 0x06, 0x04, 0x00, 0x00, 0x00,
        0x0A, 0x01, 0x0F, 0x01, 0x09, 0x01, 0x0B, 0x01, 0x07, 0x01, 0x00, 0x00, 0x00, 0x30, 0x01,
        0x00, 0x00, 0x00, 0x50, 0x02, 0xA0, 0x02, 0x90, 0x01, 0xB0, 0x01, 0x18, 0x03, 0xC0, 0x02,
        0xD0, 0x02, 0x60, 0x01, 0x11, 0x01, 0xF0, 0x01, 0xE0, 0x02, 0x12, 0x01, 0x13, 0x01, 0x14,
        0x01, 0x15, 0x01, 0x17, 0x01, 0x23, 0x01, 0x24, 0x01, 0x25, 0x01, 0x21, 0x01, 0x22, 0x01,
        0x19, 0x01, 0x28, 0x01, 0x26, 0x01, 0x29, 0x01, 0x31, 0x01, 0x36, 0x01, 0x37, 0x01,
    ];

    #[test]
    fn matches_captured_request_and_preserves_reported_values() {
        assert_eq!(REQUEST, [4, 0, 4, 0, 1, 0, 8]);
        let values = parse(CAPTURED).unwrap();
        assert_eq!(values.len(), 41);
        for expected in [(0x11, 1), (0xC0, 2), (0xD0, 2), (0x31, 1), (0x37, 1)] {
            assert!(values.contains(&expected));
        }
        assert_eq!(values[0], (1, 14));
        assert_eq!(values[2], (4, 1));
    }

    #[test]
    fn reads_all_seven_wide_ids_without_truncating_values() {
        for id in [3, 4, 5, 6, 7, 8, 0x30] {
            let packet = [4, 0, 4, 0, 2, 0, 2, id, 0x98, 0xBA, 0xDC, 0xFE, 0x11, 0xA5];
            assert_eq!(parse(&packet), Some(vec![(id, 0xFEDCBA98), (0x11, 0xA5)]));
            for length in 8..12 {
                assert!(parse(&packet[..length]).is_none());
            }
        }
    }

    #[test]
    fn rejects_ambiguous_or_incomplete_reports_without_replacing_previous_values() {
        let expected = parse(CAPTURED).unwrap();
        let mut previous = expected.clone();
        let mut malformed = Vec::new();
        for length in 0..CAPTURED.len() {
            malformed.push(CAPTURED[..length].to_vec());
        }
        for (index, value) in [(0, 0), (4, 1), (5, 1), (6, 40), (6, 42), (7, 0xFF)] {
            let mut packet = CAPTURED.to_vec();
            packet[index] = value;
            malformed.push(packet);
        }
        let mut trailing = CAPTURED.to_vec();
        trailing.push(0);
        malformed.push(trailing);
        malformed.push(vec![4, 0, 4, 0, 2, 0, 2, 0x11, 1, 0x11, 2]);
        for packet in malformed {
            if let Some(values) = parse(&packet) {
                previous = values;
            }
            assert_eq!(previous, expected, "packet={packet:02x?}");
        }
        assert_eq!(parse(&[4, 0, 4, 0, 2, 0, 0]), Some(vec![]));
        assert!(parse(&[4, 0, 4, 0, 2, 0, 0, 0]).is_none());
    }
}
