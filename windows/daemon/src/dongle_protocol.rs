//! USB HID feature reports; see airpods-dongle/docs/USB-CONTROL.md.
use ntpods_ipc::{Battery, DongleState};

pub const SIZE: usize = 64;
pub const HEADER: [u8; 7] = [2, b'A', b'P', b'D', b'G', 1, 64];

#[derive(Clone)]
pub struct Report {
    pub state: DongleState,
    pub address: [u8; 6],
    pub sequence: u32,
    pub opcode: u8,
    pub result: u8,
}

pub fn parse(bytes: &[u8]) -> Option<Report> {
    if bytes.len() != SIZE
        || bytes[..7] != HEADER
        || bytes[7] & 0x80 != 0
        || bytes[18] > 4
        || bytes[22] & !7 != 0
        || bytes[23] > 3
        || bytes[40] > 6
        || bytes[41] > 4
        || bytes[44..].iter().any(|b| *b != 0)
        || bytes[19..22].iter().any(|b| *b > 100 && *b != 255)
    {
        return None;
    }
    let u32_at = |i| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
    let flags = bytes[7];
    let ages = [24, 28, 32].map(|i| (u32_at(i) != u32::MAX).then(|| u32_at(i)));
    let level = |i| (bytes[i] <= 100).then_some(bytes[i]);
    Some(Report {
        state: DongleState {
            present: true,
            generation: u32_at(8),
            selected: bytes[12..18].iter().any(|b| *b != 0),
            connected: flags & 1 != 0,
            controls_ready: flags & 2 != 0,
            streaming: flags & 4 != 0,
            phone_connected: flags & 8 != 0,
            microphone_active: flags & 16 != 0,
            phone_pairing: flags & 32 != 0,
            usb_playback: flags & 64 != 0,
            anc: bytes[18],
            battery: Battery {
                left: level(19),
                right: level(20),
                case: level(21),
                left_charging: bytes[22] & 1 != 0,
                right_charging: bytes[22] & 2 != 0,
                case_charging: bytes[22] & 4 != 0,
                ..Default::default()
            },
            battery_age_ms: ages,
            codec: bytes[23],
            capabilities: u16::from_le_bytes([bytes[42], bytes[43]]),
            ..Default::default()
        },
        address: bytes[12..18].try_into().unwrap(),
        sequence: u32_at(36),
        opcode: bytes[41],
        result: bytes[40],
    })
}

pub fn command(
    report: &Report,
    generation: u32,
    sequence: u32,
    opcode: u8,
    argument: u8,
) -> Result<[u8; SIZE], &'static str> {
    validate_command(&report.state, generation, opcode, argument)?;
    if sequence == 0 {
        return Err("unsupported");
    }
    let mut bytes = [0; SIZE];
    bytes[..7].copy_from_slice(&HEADER);
    bytes[7] = opcode;
    bytes[8..12].copy_from_slice(&generation.to_le_bytes());
    bytes[12..18].copy_from_slice(&report.address);
    bytes[18..22].copy_from_slice(&sequence.to_le_bytes());
    bytes[22] = argument;
    Ok(bytes)
}

pub fn validate_command(
    state: &DongleState,
    generation: u32,
    opcode: u8,
    argument: u8,
) -> Result<(), &'static str> {
    if generation != state.generation {
        return Err("stale");
    }
    if !(1..=4).contains(&opcode)
        || (opcode == 3 && !matches!(argument, 2 | 3))
        || (opcode != 3 && argument != 0)
    {
        return Err("unsupported");
    }
    let capability = match opcode {
        1 | 2 => 1,
        3 => 2,
        _ => 4,
    };
    if state.capabilities & capability == 0 {
        return Err("unsupported");
    }
    if opcode == 3 && !state.controls_ready {
        return Err("disconnected");
    }
    if opcode == 2 && (state.usb_playback || state.microphone_active || state.phone_connected) {
        return Err("busy");
    }
    Ok(())
}

pub fn receipt(report: &Report, sequence: u32, opcode: u8) -> Option<Result<(), &'static str>> {
    if report.sequence != sequence || report.opcode != opcode || report.result == 0 {
        return None;
    }
    Some(match report.result {
        1 => Ok(()),
        2 => Err("busy"),
        3 => Err("stale"),
        4 => Err("unsupported"),
        5 => Err("disconnected"),
        _ => Err("malformed"),
    })
}

/// A receipt is tied to one USB session, not to the Bluetooth generation it changes.
pub struct Pending {
    pub sequence: u32,
    pub opcode: u8,
    pub session: u64,
    pub since: std::time::Instant,
}
impl Pending {
    pub fn poll(
        &self,
        report: Option<&Report>,
        session: u64,
        now: std::time::Instant,
    ) -> Option<Result<(), &'static str>> {
        if session != self.session {
            return Some(Err("stale"));
        }
        let Some(report) = report else {
            return Some(Err("disconnected"));
        };
        if now.saturating_duration_since(self.since) >= std::time::Duration::from_secs(2) {
            return Some(Err("timeout"));
        }
        receipt(report, self.sequence, self.opcode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn status() -> [u8; SIZE] {
        let mut b = [0; SIZE];
        b[..7].copy_from_slice(&HEADER);
        b[7] = 3;
        b[8] = 7;
        b[12..18].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        b[42] = 7;
        b[19..22].fill(255);
        b[24..36].fill(255);
        b
    }
    #[test]
    fn rejects_incomplete_unknown_and_malformed_reports() {
        let good = status();
        for n in 0..SIZE {
            assert!(parse(&good[..n]).is_none());
        }
        for (offset, value) in [
            (0, 1),
            (5, 2),
            (6, 63),
            (18, 5),
            (19, 101),
            (22, 8),
            (23, 4),
            (40, 7),
            (44, 1),
        ] {
            let mut b = good;
            b[offset] = value;
            assert!(parse(&b).is_none());
        }
        let s = parse(&good).unwrap();
        assert_eq!(s.state.battery.left, None);
        assert_eq!(s.state.battery_age_ms, [None; 3]);
    }
    #[test]
    fn commands_pin_identity_and_reject_stale_generation() {
        let report = parse(&status()).unwrap();
        assert_eq!(command(&report, 6, 1, 3, 2), Err("stale"));
        assert_eq!(command(&report, 7, 1, 3, 4), Err("unsupported"));
        let b = command(&report, 7, 1, 3, 2).unwrap();
        assert_eq!(&b[12..18], &[1, 2, 3, 4, 5, 6]);
        assert!(b[23..].iter().all(|b| *b == 0));
    }
    #[test]
    fn receipt_survives_expected_pair_generation_change() {
        let mut b = status();
        b[8] = 8;
        b[36] = 42;
        b[40] = 1;
        b[41] = 2;
        let report = parse(&b).unwrap();
        assert_eq!(receipt(&report, 41, 2), None);
        assert_eq!(receipt(&report, 42, 3), None);
        assert_eq!(receipt(&report, 42, 2), Some(Ok(())));
    }
    #[test]
    fn pending_rejects_unplug_replacement_and_late_receipts() {
        use std::time::{Duration, Instant};
        let now = Instant::now();
        let pending = Pending {
            sequence: 42,
            opcode: 2,
            session: 9,
            since: now,
        };
        let mut b = status();
        b[36] = 42;
        b[40] = 1;
        b[41] = 2;
        let r = parse(&b).unwrap();
        assert_eq!(pending.poll(None, 9, now), Some(Err("disconnected")));
        assert_eq!(pending.poll(Some(&r), 10, now), Some(Err("stale")));
        assert_eq!(
            pending.poll(Some(&r), 9, now + Duration::from_secs(2)),
            Some(Err("timeout"))
        );
        assert_eq!(pending.poll(Some(&r), 9, now), Some(Ok(())));
    }
    #[test]
    fn destructive_pair_rejects_active_audio_and_unknown_controls() {
        for flag in [8, 16, 64] {
            let mut b = status();
            b[7] |= flag;
            assert_eq!(command(&parse(&b).unwrap(), 7, 1, 2, 0), Err("busy"));
        }
        let mut b = status();
        b[42] = 1;
        assert_eq!(command(&parse(&b).unwrap(), 7, 1, 3, 2), Err("unsupported"));
        b[42] = 7;
        b[7] = 1;
        assert_eq!(
            command(&parse(&b).unwrap(), 7, 1, 3, 2),
            Err("disconnected")
        );
    }
}
