use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub fn packet(now: SystemTime) -> Result<[u8; 41], &'static str> {
    let elapsed = now
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "Host clock precedes the Unix epoch")?;
    let seconds = u32::try_from(elapsed.as_secs())
        .map_err(|_| "Host clock exceeds the AACP timestamp range")?;
    // Keep both representations in UTC; Pro 3 diagnostics can apply the text's offset.
    let formatted = crate::utc_timestamp(Duration::from_secs(u64::from(seconds)));
    let text = format!("{}+00:00", &formatted[..19]);
    let mut packet = [0; 41];
    packet[..6].copy_from_slice(&[4, 0, 4, 0, 0x1b, 0]);
    packet[6..10].copy_from_slice(&seconds.to_le_bytes());
    packet[10..14].copy_from_slice(&elapsed.subsec_micros().to_le_bytes());
    packet[14..16].copy_from_slice(&25u16.to_le_bytes());
    packet[16..].copy_from_slice(text.as_bytes());
    Ok(packet)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_share_one_instant_with_utc_text() {
        let elapsed = Duration::new(1_791_839_997, 933_171_999);
        let packet = packet(UNIX_EPOCH + elapsed).unwrap();
        assert_eq!(&packet[..6], &[4, 0, 4, 0, 0x1b, 0]);
        assert_eq!(&packet[6..10], &1_791_839_997u32.to_le_bytes());
        assert_eq!(&packet[10..14], &933_171u32.to_le_bytes());
        assert_eq!(&packet[14..16], &25u16.to_le_bytes());
        assert_eq!(&packet[16..], b"2026-10-12T21:19:57+00:00");
    }

    #[test]
    fn epoch_and_leap_day_format_without_fractional_text() {
        for (seconds, text) in [
            (0, b"1970-01-01T00:00:00+00:00"),
            (951_782_400, b"2000-02-29T00:00:00+00:00"),
        ] {
            let packet = packet(UNIX_EPOCH + Duration::from_secs(seconds)).unwrap();
            assert_eq!(&packet[10..14], &[0; 4]);
            assert_eq!(&packet[16..], text);
        }
    }

    #[test]
    fn unsigned_seconds_work_past_2038_through_the_wire_limit() {
        for (seconds, text) in [
            (i32::MAX as u64 + 1, b"2038-01-19T03:14:08+00:00"),
            (u32::MAX as u64, b"2106-02-07T06:28:15+00:00"),
        ] {
            let packet = packet(UNIX_EPOCH + Duration::new(seconds, 999_999_999)).unwrap();
            assert_eq!(&packet[6..10], &(seconds as u32).to_le_bytes());
            assert_eq!(&packet[10..14], &999_999u32.to_le_bytes());
            assert_eq!(&packet[16..], text);
        }
    }

    #[test]
    fn out_of_range_clocks_are_rejected_instead_of_wrapping() {
        assert!(packet(UNIX_EPOCH - Duration::from_micros(1)).is_err());
        assert!(packet(UNIX_EPOCH + Duration::from_secs(u32::MAX as u64 + 1)).is_err());
    }
}
