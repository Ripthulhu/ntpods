//! Paired-device BLE battery reports. Keys come only from the existing AAP link.
use ntpods_ipc::{Battery, BatteryFreshness, BatteryObservation, Snapshot};
use std::{
    io::{self, Read},
    path::{Path, PathBuf},
    ptr,
};
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::*;

pub const REQUEST_KEYS: [u8; 8] = [4, 0, 4, 0, 0x30, 0, 5, 0];
const KEYS_HEADER: [u8; 6] = [4, 0, 4, 0, 0x31, 0];
// Closed-case reports on Pro 3 can arrive roughly a minute apart.
pub const FRESH_MS: u64 = 90_000;
const VISIT_ABSENCE_MS: u64 = 45_000;

#[derive(Clone, Copy, Default)]
pub struct Cache(Option<(u64, Battery, BatteryFreshness)>);

impl Cache {
    pub fn observe(&mut self, battery: Battery, at_ms: u64) {
        if self.0.is_none_or(|(previous, _, _)| at_ms >= previous) {
            let (at, levels, freshness) = self.0.get_or_insert_with(Default::default);
            merge_report(
                levels,
                freshness,
                battery,
                BatteryFreshness {
                    left: observation(battery.left, at_ms),
                    right: observation(battery.right, at_ms),
                    case: observation(battery.case, at_ms),
                    headphone: observation(battery.headphone, at_ms),
                },
            );
            *at = at_ms;
        }
    }

    pub fn restore(&self, state: &mut Snapshot, now_ms: u64) -> bool {
        let Some((at, battery, observations)) = self.0 else {
            return false;
        };
        if (state.connected && !state.link_stale)
            || now_ms < at
            || now_ms - at >= FRESH_MS
            || state.battery_timestamp_ms.is_some_and(|current| {
                current > at || (current == at && state.battery_source == "ble")
            })
        {
            return false;
        }
        merge_report(
            &mut state.battery,
            state.battery_freshness.get_or_insert_with(Default::default),
            battery,
            observations,
        );
        state.battery_source = "ble".into();
        state.battery_timestamp_ms = Some(at);
        refresh_live(state, now_ms);
        true
    }
}

fn merge_report(
    battery: &mut Battery,
    freshness: &mut BatteryFreshness,
    report: Battery,
    observations: BatteryFreshness,
) {
    for (value, charging, observation, dest, dest_charging, age) in [
        (
            report.left,
            report.left_charging,
            observations.left,
            &mut battery.left,
            &mut battery.left_charging,
            &mut freshness.left,
        ),
        (
            report.right,
            report.right_charging,
            observations.right,
            &mut battery.right,
            &mut battery.right_charging,
            &mut freshness.right,
        ),
        (
            report.case,
            report.case_charging,
            observations.case,
            &mut battery.case,
            &mut battery.case_charging,
            &mut freshness.case,
        ),
        (
            report.headphone,
            report.headphone_charging,
            observations.headphone,
            &mut battery.headphone,
            &mut battery.headphone_charging,
            &mut freshness.headphone,
        ),
    ] {
        let newer = value.is_some() && observation.timestamp_ms >= age.timestamp_ms;
        if newer {
            *dest = value;
            age.timestamp_ms = observation.timestamp_ms;
        }
        // A newer complete BLE report may mark a part absent while its cached
        // percentage still belongs to an earlier report.
        age.present = observation.present;
        age.live = newer && observation.live;
        *dest_charging = age.live && charging;
    }
}

pub fn invalidate(state: &mut Snapshot) {
    state.battery_live = false;
    let freshness = state.battery_freshness.get_or_insert_with(Default::default);
    for (age, charging) in [
        (&mut freshness.left, &mut state.battery.left_charging),
        (&mut freshness.right, &mut state.battery.right_charging),
        (&mut freshness.case, &mut state.battery.case_charging),
        (
            &mut freshness.headphone,
            &mut state.battery.headphone_charging,
        ),
    ] {
        age.live = false;
        age.present = None;
        *charging = false;
    }
}

fn observation(value: Option<u8>, at_ms: u64) -> BatteryObservation {
    BatteryObservation {
        timestamp_ms: value.map(|_| at_ms),
        live: value.is_some(),
        present: Some(value.is_some()),
    }
}

pub fn scan_allowed(opt_in: bool, connected: bool, capture: bool, uplink: bool) -> bool {
    !capture && !uplink && (opt_in || !connected)
}

// Deliberately no Debug/Serialize: these keys never belong in logs or IPC.
#[derive(Clone, PartialEq, Eq)]
pub struct Keys {
    pub mac: u64,
    irk: [u8; 16],
    encryption: [u8; 16],
}

#[derive(Debug, PartialEq, Eq)]
pub struct Report {
    pub battery: Battery,
    pub both_in_case: bool,
    pub lid_closed: Option<bool>,
    headset: bool,
}

impl Report {
    pub fn may_connect(&self) -> bool {
        // shortcut: Max reports update battery only; verify wear/Smart Case flags before reconnecting.
        !self.headset && !self.both_in_case && self.lid_closed != Some(true)
    }
}

#[derive(Default)]
pub struct Visit {
    acted: bool,
    last_seen_ms: Option<u64>,
}

impl Visit {
    pub fn observe(&mut self, report: &Report, now_ms: u64) -> bool {
        if self
            .last_seen_ms
            .is_none_or(|last| now_ms.saturating_sub(last) > VISIT_ABSENCE_MS)
        {
            self.acted = false;
        }
        self.last_seen_ms = Some(now_ms);
        // A closed case with one bud still outside is the same visit.
        if report.both_in_case && report.lid_closed == Some(true) {
            self.acted = false;
        }
        report.may_connect() && !self.acted
    }

    pub fn mark_acted(&mut self) {
        self.acted = true;
    }

    pub fn observe_connected(&mut self, report: &Report, now_ms: u64) {
        self.observe(report, now_ms);
        if !(report.both_in_case && report.lid_closed == Some(true)) {
            self.mark_acted();
        }
    }
}

pub fn is_key_response(data: &[u8]) -> bool {
    data.starts_with(&KEYS_HEADER)
}

pub fn parse_keys(mac: u64, data: &[u8]) -> Option<Keys> {
    if mac == 0
        || mac > 0xffff_ffff_ffff
        || !is_key_response(data)
        || data.len() != 47
        || data[6] != 2
    {
        return None;
    }
    let mut irk = None;
    let mut encryption = None;
    for record in data[7..].chunks_exact(20) {
        if record[1..4] != [0, 16, 0] {
            return None;
        }
        let value: [u8; 16] = record[4..].try_into().ok()?;
        if value == [0; 16] {
            return None;
        }
        match record[0] {
            1 if irk.is_none() => irk = Some(value),
            4 if encryption.is_none() => encryption = Some(value),
            _ => return None,
        }
    }
    Some(Keys {
        mac,
        irk: irk?,
        encryption: encryption?,
    })
}

fn aes_block(key: &[u8; 16], input: &[u8; 16], decrypt: bool) -> Option<[u8; 16]> {
    unsafe {
        let mut algorithm = ptr::null_mut();
        if BCryptOpenAlgorithmProvider(&mut algorithm, BCRYPT_AES_ALGORITHM, ptr::null(), 0) < 0 {
            return None;
        }
        let mode: Vec<u16> = "ChainingModeECB\0".encode_utf16().collect();
        let mut handle = ptr::null_mut();
        let result = (|| {
            if BCryptSetProperty(
                algorithm,
                BCRYPT_CHAINING_MODE,
                mode.as_ptr().cast(),
                (mode.len() * 2) as u32,
                0,
            ) < 0
            {
                return None;
            }
            if BCryptGenerateSymmetricKey(
                algorithm,
                &mut handle,
                ptr::null_mut(),
                0,
                key.as_ptr(),
                16,
                0,
            ) < 0
            {
                return None;
            }
            let mut output = [0; 16];
            let mut written = 0;
            let operation = if decrypt {
                BCryptDecrypt
            } else {
                BCryptEncrypt
            };
            let status = operation(
                handle,
                input.as_ptr(),
                16,
                ptr::null(),
                ptr::null_mut(),
                0,
                output.as_mut_ptr(),
                16,
                &mut written,
                0,
            );
            (status >= 0 && written == 16).then_some(output)
        })();
        if !handle.is_null() {
            BCryptDestroyKey(handle);
        }
        BCryptCloseAlgorithmProvider(algorithm, 0);
        result
    }
}

impl Keys {
    fn matches_address(&self, address: u64) -> bool {
        if address > 0xffff_ffff_ffff || (address >> 46) != 1 {
            return false;
        }
        let address = address.to_be_bytes();
        let mut key = self.irk;
        key.reverse();
        let mut input = [0; 16];
        input[13..].copy_from_slice(&address[2..5]);
        aes_block(&key, &input, false).is_some_and(|output| output[13..] == address[5..])
    }

    pub fn decode(&self, mac: u64, address: u64, manufacturer: &[u8]) -> Option<Report> {
        let data = proximity_record(manufacturer)?;
        if data[2] != 1 || mac != self.mac || !self.matches_address(address) {
            return None;
        }
        let model = u16::from_be_bytes([data[3], data[4]]);
        let headset = matches!(model, 0x0a20 | 0x1f20);
        if !headset
            && !matches!(
                model,
                0x0220
                    | 0x0f20
                    | 0x1320
                    | 0x1920
                    | 0x1b20
                    | 0x0e20
                    | 0x1420
                    | 0x2420
                    | 0x2720
                    | 0x3020
                    | 0x3620
            )
        {
            return None;
        }
        let encrypted = data[11..].try_into().ok()?;
        let plain = aes_block(&self.encryption, &encrypted, true)?;
        let primary_left = data[5] & 0x20 != 0;
        if headset {
            // LibrePods 34076c85 linux/battery.hpp: Max uses the first available slot.
            let slots = [
                component(plain[if primary_left { 1 } else { 2 }])?,
                component(plain[if primary_left { 2 } else { 1 }])?,
                component(plain[3])?,
            ];
            let (headphone, headphone_charging) = slots
                .into_iter()
                .find(|(level, _)| level.is_some())
                .unwrap_or((None, false));
            return Some(Report {
                battery: Battery {
                    headphone,
                    headphone_charging,
                    ..Default::default()
                },
                both_in_case: false,
                lid_closed: None,
                headset: true,
            });
        }
        let (left, left_charging) = component(plain[if primary_left { 1 } else { 2 }])?;
        let (right, right_charging) = component(plain[if primary_left { 2 } else { 1 }])?;
        let (case, case_charging) = component(plain[3])?;
        let both_in_case = data[5] & 4 != 0;
        let lid_closed = (data[5] & 0x44 != 0).then_some(data[8] & 8 != 0);
        Some(Report {
            battery: Battery {
                left,
                right,
                case,
                left_charging,
                right_charging,
                case_charging,
                ..Default::default()
            },
            both_in_case,
            lid_closed,
            headset: false,
        })
    }

    pub fn load(mac: u64) -> Option<Self> {
        Self::load_at(&key_directory()?, mac)
    }

    fn load_at(directory: &Path, mac: u64) -> Option<Self> {
        if mac == 0 || mac > 0xffff_ffff_ffff {
            return None;
        }
        let path = key_path(directory, mac);
        if path.try_exists().ok()? {
            return Self::load_file(&path, mac);
        }
        let keys = Self::load_file(&directory.join("ble-battery-keys.dat"), mac)?;
        // Retain the legacy file so switching pairs or a failed migration loses no keys.
        let _ = keys.save_at(directory);
        Some(keys)
    }

    fn load_file(path: &Path, mac: u64) -> Option<Self> {
        let mut data = Vec::new();
        std::fs::File::open(path)
            .ok()?
            .take(4097)
            .read_to_end(&mut data)
            .ok()?;
        let mut plain = dpapi(&data, false).ok()?;
        let result = (|| {
            if plain.len() != 44
                || &plain[..4] != b"NBK1"
                || u64::from_le_bytes(plain[4..12].try_into().ok()?) != mac
            {
                return None;
            }
            let keys = Self {
                mac,
                irk: plain[12..28].try_into().ok()?,
                encryption: plain[28..44].try_into().ok()?,
            };
            (mac != 0
                && mac <= 0xffff_ffff_ffff
                && keys.irk != [0; 16]
                && keys.encryption != [0; 16])
                .then_some(keys)
        })();
        wipe(&mut plain);
        result
    }

    pub fn save(&self) -> io::Result<()> {
        let directory = key_directory()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "local profile unavailable"))?;
        self.save_at(&directory)
    }

    fn save_at(&self, directory: &Path) -> io::Result<()> {
        let mut plain = Vec::with_capacity(44);
        plain.extend_from_slice(b"NBK1");
        plain.extend_from_slice(&self.mac.to_le_bytes());
        plain.extend_from_slice(&self.irk);
        plain.extend_from_slice(&self.encryption);
        let protected = dpapi(&plain, true);
        wipe(&mut plain);
        let protected = protected?;
        std::fs::create_dir_all(directory)?;
        let path = key_path(directory, self.mac);
        let temporary = path.with_extension("tmp");
        std::fs::write(&temporary, protected)?;
        let result = std::fs::rename(&temporary, path);
        if result.is_err() {
            let _ = std::fs::remove_file(temporary);
        }
        result
    }
}

fn key_directory() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("NTPods"))
}

fn key_path(directory: &Path, mac: u64) -> PathBuf {
    directory.join(format!("ble-battery-keys-{mac:012x}.dat"))
}

fn wipe(bytes: &mut [u8]) {
    for byte in bytes {
        unsafe {
            ptr::write_volatile(byte, 0);
        }
    }
}

fn dpapi(data: &[u8], protect: bool) -> io::Result<Vec<u8>> {
    if data.is_empty() || data.len() > 4096 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid protected-key length",
        ));
    }
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: ptr::null_mut(),
        };
        let ok = if protect {
            CryptProtectData(
                &input,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        let bytes = std::slice::from_raw_parts_mut(output.pbData, output.cbData as usize);
        let result = if bytes.len() <= 4096 {
            Ok(bytes.to_vec())
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid unprotected-key length",
            ))
        };
        wipe(bytes);
        LocalFree(output.pbData.cast());
        result
    }
}

fn component(raw: u8) -> Option<(Option<u8>, bool)> {
    match raw & 0x7f {
        127 => Some((None, false)),
        level @ 0..=100 => Some((Some(level), raw & 0x80 != 0)),
        _ => None,
    }
}

fn proximity_record(mut data: &[u8]) -> Option<&[u8]> {
    if data.len() > 254 {
        return None;
    }
    let mut found = None;
    while !data.is_empty() {
        if data.len() < 2 {
            return None;
        }
        let length = data[1] as usize + 2;
        if length > data.len() {
            return None;
        }
        if data[0] == 7 {
            if length != 27 || found.is_some() {
                return None;
            }
            found = Some(&data[..length]);
        }
        data = &data[length..];
    }
    found
}

pub fn refresh_live(state: &mut Snapshot, now: u64) -> bool {
    let source_live = match state.battery_source.as_str() {
        "aap" => state.connected && !state.link_stale && state.battery_timestamp_ms.is_some(),
        "ble" => state
            .battery_timestamp_ms
            .is_some_and(|at| now >= at && now - at < FRESH_MS),
        _ => false,
    };
    let mut changed = false;
    let live = if let Some(freshness) = &mut state.battery_freshness {
        let mut any_live = false;
        for (value, age) in [
            (state.battery.left, &mut freshness.left),
            (state.battery.right, &mut freshness.right),
            (state.battery.case, &mut freshness.case),
            (state.battery.headphone, &mut freshness.headphone),
        ] {
            let live = age.live
                && source_live
                && value.is_some()
                && age
                    .timestamp_ms
                    .is_some_and(|at| now >= at && now - at < FRESH_MS);
            changed |= age.live != live;
            age.live = live;
            any_live |= live;
        }
        any_live
    } else {
        source_live
    };
    changed |= state.battery_live != live;
    state.battery_live = live;
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> Keys {
        Keys {
            mac: 0x112233445566,
            irk: [
                0x79, 0x04, 0x65, 0x1e, 0xe2, 0xcc, 0xd9, 0x26, 0xf2, 0x6e, 0x20, 0xee, 0x3e, 0xcc,
                0xde, 0x79,
            ],
            encryption: [0x18; 16],
        }
    }

    #[test]
    fn cached_ble_restores_only_a_newer_real_sample_after_disconnect() {
        let mut cache = Cache::default();
        let battery = Battery {
            left: Some(77),
            right: None,
            right_charging: false,
            ..Default::default()
        };
        cache.observe(battery, 2_000);
        let mut state = Snapshot {
            connected: true,
            battery_source: "aap".into(),
            battery_timestamp_ms: Some(1_000),
            battery: Battery {
                left: Some(76),
                right: Some(76),
                right_charging: true,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(!cache.restore(&mut state, 3_000));
        state.connected = false;
        assert!(cache.restore(&mut state, 3_000));
        assert_eq!(state.battery_timestamp_ms, Some(2_000));
        assert_eq!(state.battery.left, battery.left);
        assert_eq!(state.battery.right, Some(76));
        assert!(!state.battery.right_charging);
        assert!(!state.battery_freshness.unwrap().right.live);
        assert!(!cache.restore(&mut state, 4_000));
        assert_eq!(state.battery_timestamp_ms, Some(2_000));
        assert!(refresh_live(&mut state, 2_000 + FRESH_MS));
        assert!(!state.battery_live);
        assert!(!cache.restore(&mut state, 2_000 + FRESH_MS));
        state.battery_source = "aap".into();
        state.battery_timestamp_ms = Some(5_000);
        assert!(!cache.restore(&mut state, 6_000));
        assert_eq!(state.battery_source, "aap");
        cache.observe(Battery::default(), 1_000);
        state.battery_timestamp_ms = None;
        assert!(cache.restore(&mut state, 6_000));
        assert_eq!(state.battery_timestamp_ms, Some(2_000));
    }

    #[test]
    fn cached_ble_updates_stale_aap_without_changing_connection_state() {
        let mut cache = Cache::default();
        cache.observe(
            Battery {
                left: Some(77),
                ..Default::default()
            },
            2_000,
        );
        let mut state = Snapshot {
            connected: true,
            link_stale: true,
            battery_source: "aap".into(),
            battery_timestamp_ms: Some(1_000),
            ..Default::default()
        };
        assert!(cache.restore(&mut state, 3_000));
        assert!(state.connected && state.link_stale);
        assert_eq!(state.battery_timestamp_ms, Some(2_000));
        assert_eq!(state.battery.left, Some(77));
        state.link_stale = false;
        cache.observe(Battery::default(), 4_000);
        assert!(!cache.restore(&mut state, 5_000));
        assert_eq!(state.battery_timestamp_ms, Some(2_000));
    }

    #[test]
    fn connected_advertisements_do_not_create_a_new_visit_on_handoff() {
        let outside = Report {
            battery: Battery::default(),
            both_in_case: false,
            lid_closed: None,
            headset: false,
        };
        let closed = Report {
            battery: Battery::default(),
            both_in_case: true,
            lid_closed: Some(true),
            headset: false,
        };
        let mut visit = Visit::default();
        for now in (0..90_000).step_by(10_000) {
            visit.observe_connected(&outside, now);
        }
        assert!(!visit.observe(&outside, 90_000));
        visit.observe_connected(&closed, 91_000);
        assert!(visit.observe(&outside, 92_000));
    }

    #[test]
    fn scan_opt_in_never_overrides_active_capture_or_uplink() {
        assert!(scan_allowed(false, false, false, false));
        assert!(!scan_allowed(false, true, false, false));
        assert!(scan_allowed(true, true, false, false));
        for opt_in in [false, true] {
            for connected in [false, true] {
                assert!(!scan_allowed(opt_in, connected, true, false));
                assert!(!scan_allowed(opt_in, connected, false, true));
            }
        }
    }
    fn advertisement(status: u8, levels: [u8; 3]) -> Vec<u8> {
        let mut data = vec![7, 25, 1, 0x27, 0x20, status, 0x99, 0x8f, 8, 0, 5];
        let mut plain = [0; 16];
        plain[1..4].copy_from_slice(&levels);
        data.extend(aes_block(&keys().encryption, &plain, false).unwrap());
        data
    }

    #[test]
    fn aes_matches_nist_vector_and_rpa_matches_independent_capod_fixture() {
        let key = [
            0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf,
            0x4f, 0x3c,
        ];
        let plain = [
            0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96, 0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93,
            0x17, 0x2a,
        ];
        let cipher = [
            0x3a, 0xd7, 0x7b, 0xb4, 0x0d, 0x7a, 0x36, 0x60, 0xa8, 0x9e, 0xca, 0xf3, 0x24, 0x66,
            0xef, 0x97,
        ];
        assert_eq!(aes_block(&key, &plain, false), Some(cipher));
        assert_eq!(aes_block(&key, &cipher, true), Some(plain));
        let mut k = keys();
        assert!(k.matches_address(0x5a162b91d1cd));
        assert!(!k.matches_address(0x5a162b91d1cc));
        k.irk[15] = 0xaa;
        assert!(!k.matches_address(0x5a162b91d1cd));
    }

    #[test]
    fn encrypted_report_requires_owned_address_and_preserves_exact_levels_and_unknowns() {
        let k = keys();
        let data = advertisement(0x64, [0x80 | 73, 0x80 | 62, 0x80 | 51]);
        let report = k.decode(k.mac, 0x5a162b91d1cd, &data).unwrap();
        assert_eq!(
            (
                report.battery.left,
                report.battery.right,
                report.battery.case
            ),
            (Some(73), Some(62), Some(51))
        );
        assert!(
            report.battery.left_charging
                && report.battery.right_charging
                && report.battery.case_charging
        );
        assert!(report.both_in_case);
        assert_eq!(report.lid_closed, Some(true));
        assert!(!report.may_connect());
        assert!(k.decode(k.mac + 1, 0x5a162b91d1cd, &data).is_none());
        assert!(k.decode(k.mac, 0x5a162b91d1cc, &data).is_none());
        for model in [0x0000u16, 0xffff] {
            let mut wrong_format = data.clone();
            wrong_format[3..5].copy_from_slice(&model.to_be_bytes());
            assert!(k.decode(k.mac, 0x5a162b91d1cd, &wrong_format).is_none());
        }
        for (index, value) in [(1, 24), (1, 26), (2, 0)] {
            let mut malformed = data.clone();
            malformed[index] = value;
            assert!(k.decode(k.mac, 0x5a162b91d1cd, &malformed).is_none());
        }
        let mut compound = vec![0x10, 1, 0x42];
        compound.extend(&data);
        compound.extend([0x15, 0]);
        assert_eq!(k.decode(k.mac, 0x5a162b91d1cd, &compound), Some(report));
        let report = k
            .decode(k.mac, 0x5a162b91d1cd, &advertisement(0x10, [100, 0, 0xff]))
            .unwrap();
        assert_eq!(
            (
                report.battery.left,
                report.battery.right,
                report.battery.case
            ),
            (Some(0), Some(100), None)
        );
        assert!(!report.battery.case_charging);
        assert_eq!(report.lid_closed, None);
        assert!(report.may_connect());
        assert!(k
            .decode(k.mac, 0x5a162b91d1cd, &advertisement(0x64, [101, 42, 42]))
            .is_none());
        for n in 0..data.len() {
            assert!(k.decode(k.mac, 0x5a162b91d1cd, &data[..n]).is_none());
        }
        let mut duplicate = data.clone();
        duplicate.extend(&data);
        assert!(k.decode(k.mac, 0x5a162b91d1cd, &duplicate).is_none());
    }

    #[test]
    fn airpods_five_encrypted_reports_preserve_primary_side_charging_and_absence() {
        // Product IDs follow AirPodsDesktop 71fbb5e9; ciphertext uses synthetic
        // keys and independently generated AES-ECB blocks, never accessory keys.
        let fixtures = [
            (
                [
                    7, 25, 1, 0x30, 0x20, 0x64, 0x99, 0x8f, 8, 0, 5, 0xfd, 0x32, 0xcb, 0xbc, 0x8f,
                    0xae, 0xe4, 0x09, 0x84, 0x88, 0xbb, 0x5b, 0xf4, 0xb8, 0x53, 0x91,
                ],
                Battery {
                    left: Some(65),
                    right: Some(31),
                    case: Some(88),
                    left_charging: true,
                    case_charging: true,
                    ..Default::default()
                },
            ),
            (
                [
                    7, 25, 1, 0x36, 0x20, 0x44, 0x99, 0x8f, 8, 0, 5, 0x73, 0x70, 0xa8, 0x17, 0x48,
                    0x79, 0x8f, 0x5f, 0xae, 0xba, 0xfa, 0x46, 0x6f, 0x0d, 0xfc, 0xf7,
                ],
                Battery {
                    left: Some(39),
                    right: None,
                    case: Some(62),
                    left_charging: true,
                    ..Default::default()
                },
            ),
        ];
        let k = keys();
        for (data, expected) in fixtures {
            let report = k.decode(k.mac, 0x5a162b91d1cd, &data).unwrap();
            assert_eq!(report.battery, expected);
            assert!(!report.headset && report.both_in_case);
            assert_eq!(report.lid_closed, Some(true));
            assert!(!report.may_connect());
            assert!(k.decode(k.mac + 1, 0x5a162b91d1cd, &data).is_none());
            assert!(k.decode(k.mac, 0x5a162b91d1cc, &data).is_none());
            for length in 0..data.len() {
                assert!(k.decode(k.mac, 0x5a162b91d1cd, &data[..length]).is_none());
            }
            for unverified_model in [0x2d20u16, 0x1d20] {
                let mut unverified = data;
                unverified[3..5].copy_from_slice(&unverified_model.to_be_bytes());
                assert!(k.decode(k.mac, 0x5a162b91d1cd, &unverified).is_none());
            }
        }
    }

    #[test]
    fn closed_and_unknown_cased_reports_do_not_request_an_audio_connection() {
        for (both_in_case, lid_closed, expected) in [
            (true, Some(true), false),
            (true, None, false),
            (true, Some(false), false),
            (false, Some(true), false),
            (false, None, true),
        ] {
            assert_eq!(
                Report {
                    battery: Battery::default(),
                    both_in_case,
                    lid_closed,
                    headset: false,
                }
                .may_connect(),
                expected
            );
        }
    }

    #[test]
    fn max_reports_use_one_headset_battery_without_earbud_reconnect_hints() {
        let k = keys();
        for model in [0x0a20u16, 0x1f20] {
            for status in [0, 0x20, 0x64] {
                for (levels, expected, charging) in [
                    ([0xff, 0x80 | 73, 0xff], Some(73), true),
                    ([0xff, 0xff, 22], Some(22), false),
                    ([0, 0xff, 0xff], Some(0), false),
                    ([0xff; 3], None, false),
                    (
                        [25, 55, 80],
                        Some(if status & 0x20 != 0 { 25 } else { 55 }),
                        false,
                    ),
                ] {
                    let mut data = advertisement(status, levels);
                    data[3..5].copy_from_slice(&model.to_be_bytes());
                    let report = k.decode(k.mac, 0x5a162b91d1cd, &data).unwrap();
                    assert_eq!(
                        report.battery,
                        Battery {
                            headphone: expected,
                            headphone_charging: charging,
                            ..Default::default()
                        }
                    );
                    assert!(report.headset);
                    assert!(!report.may_connect());
                    assert_eq!(report.lid_closed, None);
                    assert!(k.decode(k.mac + 1, 0x5a162b91d1cd, &data).is_none());
                }
            }
            let mut invalid = advertisement(0x20, [101, 0xff, 0xff]);
            invalid[3..5].copy_from_slice(&model.to_be_bytes());
            assert!(k.decode(k.mac, 0x5a162b91d1cd, &invalid).is_none());
        }
    }

    #[test]
    fn alternating_sibling_reports_do_not_rearm_an_already_attempted_visit() {
        let outside = Report {
            battery: Battery::default(),
            both_in_case: false,
            lid_closed: None,
            headset: false,
        };
        let single_closed = Report {
            battery: Battery::default(),
            both_in_case: false,
            lid_closed: Some(true),
            headset: false,
        };
        let both_closed = Report {
            battery: Battery::default(),
            both_in_case: true,
            lid_closed: Some(true),
            headset: false,
        };
        let mut visit = Visit::default();
        assert!(visit.observe(&outside, 0));
        visit.mark_acted();
        // Includes the daemon's failed-connection window and more than 45s of
        // continued advertisements: neither sibling means the device went away.
        for now in (1_000..=90_000).step_by(1_000) {
            assert!(!visit.observe(&single_closed, now));
            assert!(!visit.observe(&outside, now + 1));
        }
        assert!(!visit.observe(&both_closed, 91_000));
        assert!(visit.observe(&outside, 92_000));
        visit.mark_acted();
        assert!(!visit.observe(&outside, 93_000));
        assert!(visit.observe(&outside, 93_000 + VISIT_ABSENCE_MS + 1));
    }

    #[test]
    fn key_responses_reject_truncation_duplicates_reserved_bits_and_wrong_lengths() {
        let mut packet = KEYS_HEADER.to_vec();
        packet.push(2);
        packet.extend([1, 0, 16, 0]);
        packet.extend(keys().irk);
        packet.extend([4, 0, 16, 0]);
        packet.extend(keys().encryption);
        assert!(parse_keys(keys().mac, &packet) == Some(keys()));
        for n in 0..packet.len() {
            assert!(parse_keys(keys().mac, &packet[..n]).is_none());
        }
        for (at, value) in [(6, 3), (8, 1), (9, 15), (10, 1), (27, 1)] {
            let mut bad = packet.clone();
            bad[at] = value;
            assert!(parse_keys(keys().mac, &bad).is_none());
        }
        assert!(parse_keys(0, &packet).is_none());
    }

    #[test]
    fn dpapi_roundtrip_and_tampering_are_checked() {
        let plain = b"synthetic battery keys for test";
        let mut encrypted = dpapi(plain, true).unwrap();
        assert_ne!(encrypted, plain);
        assert_eq!(dpapi(&encrypted, false).unwrap(), plain);
        let n = encrypted.len();
        encrypted[n - 1] ^= 1;
        assert!(dpapi(&encrypted, false).is_err());
    }

    #[test]
    fn keys_are_kept_per_pair_and_legacy_migration_preserves_the_original() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "ntpods-battery-key-test-{}-{nonce}",
            std::process::id()
        ));
        let mut first = keys();
        let mut second = keys();
        second.mac += 1;
        second.encryption = [0x42; 16];
        let first_path = key_path(&directory, first.mac);
        let second_path = key_path(&directory, second.mac);
        let legacy_path = directory.join("ble-battery-keys.dat");
        first.save_at(&directory).unwrap();
        std::fs::rename(&first_path, &legacy_path).unwrap();
        let legacy = std::fs::read(&legacy_path).unwrap();

        second.save_at(&directory).unwrap();
        assert!(Keys::load_at(&directory, second.mac) == Some(second.clone()));
        assert_eq!(std::fs::read(&legacy_path).unwrap(), legacy);

        // A blocked destination must leave the legacy pair usable and intact.
        let blocked_temporary = first_path.with_extension("tmp");
        std::fs::create_dir(&blocked_temporary).unwrap();
        assert!(Keys::load_at(&directory, first.mac) == Some(first.clone()));
        assert!(!first_path.exists());
        std::fs::remove_dir(&blocked_temporary).unwrap();
        assert!(Keys::load_at(&directory, first.mac) == Some(first.clone()));
        assert!(first_path.is_file());
        assert_eq!(std::fs::read(&legacy_path).unwrap(), legacy);

        first.encryption = [0x53; 16];
        first.save_at(&directory).unwrap();
        assert!(Keys::load_at(&directory, first.mac) == Some(first.clone()));
        assert!(Keys::load_at(&directory, second.mac) == Some(second.clone()));
        assert!(Keys::load_at(&directory, second.mac + 1).is_none());
        assert!(Keys::load_at(&directory, 0).is_none());
        assert!(Keys::load_at(&directory, 0x1_0000_0000_0000).is_none());

        std::fs::copy(&second_path, &first_path).unwrap();
        assert!(Keys::load_at(&directory, first.mac).is_none());
        std::fs::write(&second_path, vec![0; 4097]).unwrap();
        assert!(Keys::load_at(&directory, second.mac).is_none());
        for path in [first_path, second_path, legacy_path] {
            std::fs::remove_file(path).unwrap();
        }
        std::fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn sleeping_case_expires_without_claiming_audio_connected_or_changing_sample_time() {
        let mut state = Snapshot {
            battery_source: "ble".into(),
            battery_timestamp_ms: Some(1000),
            battery_freshness: Some(BatteryFreshness {
                left: observation(Some(73), 1000),
                ..Default::default()
            }),
            battery: Battery {
                left: Some(73),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(refresh_live(&mut state, 1000));
        assert!(state.battery_live && !state.connected);
        assert!(!refresh_live(&mut state, 1000 + 60_000));
        assert!(state.battery_live);
        assert!(!refresh_live(&mut state, 1000 + FRESH_MS - 1));
        assert!(refresh_live(&mut state, 1000 + FRESH_MS));
        assert!(!state.battery_live);
        assert_eq!(state.battery_timestamp_ms, Some(1000));
        assert_eq!(state.battery.left, Some(73));
        state.battery_source = "aap".into();
        state.connected = true;
        assert!(!refresh_live(&mut state, 900_000));
        assert!(
            !state.battery_live,
            "A connected link must not rejuvenate an old report"
        );
        crate::aap::parse_battery(&[4, 0, 4, 0, 4, 0, 1, 4, 1, 73, 1, 1])
            .unwrap()
            .apply_to_state(&mut state, 900_000);
        assert!(refresh_live(&mut state, 900_000));
        state.link_stale = true;
        assert!(refresh_live(&mut state, 900_001));
        assert!(!state.battery_live);
    }

    #[test]
    fn buds_only_reports_do_not_refresh_an_old_case_or_its_charging_flag() {
        let mut state = Snapshot {
            connected: true,
            ..Default::default()
        };
        crate::aap::parse_battery(&[4, 0, 4, 0, 4, 0, 1, 8, 1, 90, 1, 1])
            .unwrap()
            .apply_to_state(&mut state, 1000);
        refresh_live(&mut state, 1000);
        assert!(state.battery_freshness.unwrap().case.live);
        crate::aap::parse_battery(&[4, 0, 4, 0, 4, 0, 2, 4, 1, 70, 2, 1, 2, 1, 72, 2, 1])
            .unwrap()
            .apply_to_state(&mut state, 90_000);
        refresh_live(&mut state, 90_000);
        assert!(
            refresh_live(&mut state, 1000 + FRESH_MS),
            "Case expiry must publish even while buds stay live"
        );
        let ages = state.battery_freshness.unwrap();
        assert!(state.battery_live && ages.left.live && ages.right.live);
        assert!(!ages.case.live);
        assert_eq!(ages.case.timestamp_ms, Some(1000));
        assert_eq!(
            state.battery.case,
            Some(90),
            "Retain the last known percentage"
        );
        assert!(
            state.battery.case_charging,
            "Preserve the report; the UI gates charging on its own freshness"
        );
        assert!(!refresh_live(&mut state, 1000 + FRESH_MS + 1));
        crate::aap::parse_battery(&[4, 0, 4, 0, 4, 0, 1, 8, 1, 255, 4, 1])
            .unwrap()
            .apply_to_state(&mut state, 92_000);
        refresh_live(&mut state, 92_000);
        assert_eq!(state.battery.case, Some(90));
        assert!(!state.battery.case_charging);
        assert_eq!(
            state.battery_freshness.unwrap().case,
            BatteryObservation {
                timestamp_ms: Some(1000),
                live: false,
                present: Some(false),
            }
        );
    }

    #[test]
    fn explicit_absence_keeps_last_known_levels_and_cannot_revive_them() {
        for part in [1, 2, 4, 8] {
            for (raw, status) in [(255, 1), (55, 4)] {
                let mut state = Snapshot {
                    connected: true,
                    ..Default::default()
                };
                crate::aap::parse_battery(&[4, 0, 4, 0, 4, 0, 1, part, 1, 80, 1, 1])
                    .unwrap()
                    .apply_to_state(&mut state, 1000);
                refresh_live(&mut state, 1000);
                let previous_battery = state.battery;
                let mut previous_ages = state.battery_freshness.unwrap();
                for (bit, age) in [
                    (4, &mut previous_ages.left),
                    (2, &mut previous_ages.right),
                    (8, &mut previous_ages.case),
                    (1, &mut previous_ages.headphone),
                ] {
                    age.live = false;
                    if bit == part {
                        age.present = Some(false);
                    }
                }
                crate::aap::parse_battery(&[4, 0, 4, 0, 4, 0, 1, part, 1, raw, status, 1])
                    .unwrap()
                    .apply_to_state(&mut state, 2000);
                refresh_live(&mut state, 2000);
                assert_eq!(state.battery.left, previous_battery.left);
                assert_eq!(state.battery.right, previous_battery.right);
                assert_eq!(state.battery.case, previous_battery.case);
                assert_eq!(state.battery.headphone, previous_battery.headphone);
                assert!(
                    !state.battery.left_charging
                        && !state.battery.right_charging
                        && !state.battery.case_charging
                        && !state.battery.headphone_charging
                );
                assert_eq!(state.battery_freshness.unwrap(), previous_ages);
                assert!(!state.battery_live);
                assert!(!refresh_live(&mut state, 2001));
                crate::aap::parse_battery(&[4, 0, 4, 0, 4, 0, 1, part, 1, 81, 1, 1])
                    .unwrap()
                    .apply_to_state(&mut state, 3000);
                assert!(refresh_live(&mut state, 3000));
                assert!(state.battery_live);
            }
        }
        let mut unseen = Snapshot {
            connected: true,
            ..Default::default()
        };
        crate::aap::parse_battery(&[4, 0, 4, 0, 4, 0, 1, 8, 1, 255, 4, 1])
            .unwrap()
            .apply_to_state(&mut unseen, 2000);
        refresh_live(&mut unseen, 2000);
        assert!(unseen.battery.case.is_none());
        assert_eq!(
            unseen.battery_freshness.unwrap().case,
            BatteryObservation {
                present: Some(false),
                ..Default::default()
            }
        );
    }

    #[test]
    fn component_presence_survives_age_and_omission_without_reusing_cached_levels() {
        let mut state = Snapshot {
            connected: true,
            ..Default::default()
        };
        for (at, packet) in [
            (1000, &[4, 0, 4, 0, 4, 0, 2, 4, 1, 80, 2, 1, 2, 1, 81, 2, 1][..]),
            (2000, &[4, 0, 4, 0, 4, 0, 1, 2, 1, 255, 4, 1][..]),
            (3000, &[4, 0, 4, 0, 4, 0, 1, 4, 1, 79, 2, 1][..]),
        ] {
            crate::aap::parse_battery(packet).unwrap().apply_to_state(&mut state, at);
            refresh_live(&mut state, at);
        }
        let ages = state.battery_freshness.unwrap();
        assert_eq!(ages.left.present, Some(true));
        assert_eq!(ages.right.present, Some(false));
        assert_eq!(state.battery.right, Some(81));
        assert_eq!(ages.right.timestamp_ms, Some(1000));
        assert!(!ages.right.live);

        refresh_live(&mut state, 3000 + FRESH_MS);
        let expired = state.battery_freshness.unwrap();
        assert!(!expired.left.live && !expired.right.live);
        assert_eq!(expired.left.present, Some(true));
        assert_eq!(expired.right.present, Some(false));

        invalidate(&mut state);
        assert_eq!(state.battery_freshness.unwrap().left.present, None);
        assert_eq!(state.battery_freshness.unwrap().right.present, None);
        crate::aap::parse_battery(&[4, 0, 4, 0, 4, 0, 1, 2, 1, 82, 5, 1])
            .unwrap()
            .apply_to_state(&mut state, 200_000);
        assert_eq!(state.battery_freshness.unwrap().right.present, Some(true));
        assert!(state.battery.right_charging);
    }

    #[test]
    fn reconnect_keeps_levels_stale_until_each_component_reports_again() {
        let mut state = Snapshot {
            connected: true,
            ..Default::default()
        };
        crate::aap::parse_battery(&[4, 0, 4, 0, 4, 0, 2, 4, 1, 70, 1, 1, 8, 1, 90, 1, 1])
            .unwrap()
            .apply_to_state(&mut state, 1000);
        refresh_live(&mut state, 1000);
        invalidate(&mut state);
        assert_eq!(state.battery.left, Some(70));
        assert_eq!(state.battery.case, Some(90));
        assert_eq!(state.battery_timestamp_ms, Some(1000));
        assert!(!state.battery.left_charging && !state.battery.case_charging);
        assert!(!refresh_live(&mut state, 2000));
        crate::aap::parse_battery(&[4, 0, 4, 0, 4, 0, 1, 4, 1, 69, 2, 1])
            .unwrap()
            .apply_to_state(&mut state, 3000);
        refresh_live(&mut state, 3000);
        let ages = state.battery_freshness.unwrap();
        assert!(ages.left.live && !ages.case.live);
        assert_eq!(ages.case.timestamp_ms, Some(1000));
        assert_eq!(state.battery.case, Some(90));
    }

    #[test]
    fn ble_absence_retains_cached_case_without_replacing_newer_aap_history() {
        let mut cache = Cache::default();
        cache.observe(
            Battery {
                case: Some(80),
                case_charging: true,
                ..Default::default()
            },
            1000,
        );
        cache.observe(
            Battery {
                left: Some(50),
                ..Default::default()
            },
            3000,
        );
        let mut state = Snapshot::default();
        assert!(cache.restore(&mut state, 3000));
        assert_eq!(state.battery.case, Some(80));
        assert!(!state.battery.case_charging);
        assert_eq!(
            state.battery_freshness.unwrap().case,
            BatteryObservation {
                timestamp_ms: Some(1000),
                live: false,
                present: Some(false),
            }
        );
        assert!(state.battery_freshness.unwrap().left.live);
        assert!(!refresh_live(&mut state, 3001));

        crate::aap::parse_battery(&[4, 0, 4, 0, 4, 0, 1, 8, 1, 85, 1, 1])
            .unwrap()
            .apply_to_state(&mut state, 4000);
        cache.observe(
            Battery {
                left: Some(49),
                ..Default::default()
            },
            5000,
        );
        assert!(cache.restore(&mut state, 5000));
        assert_eq!(state.battery.case, Some(85));
        assert_eq!(
            state.battery_freshness.unwrap().case,
            BatteryObservation {
                timestamp_ms: Some(4000),
                live: false,
                present: Some(false),
            }
        );
        assert!(!state.battery.case_charging);
        cache.observe(
            Battery {
                case: Some(86),
                case_charging: true,
                ..Default::default()
            },
            6000,
        );
        assert!(cache.restore(&mut state, 6000));
        assert_eq!(state.battery.case, Some(86));
        assert!(state.battery.case_charging && state.battery_freshness.unwrap().case.live);
        assert_eq!(state.battery.left, Some(49));
        assert_eq!(
            state.battery_freshness.unwrap().left,
            BatteryObservation {
                timestamp_ms: Some(5000),
                live: false,
                present: Some(false),
            }
        );
    }

    #[test]
    fn restoring_ble_keeps_each_present_parts_original_report_time() {
        let mut cache = Cache::default();
        cache.observe(
            Battery {
                left: Some(80),
                case: Some(50),
                case_charging: true,
                ..Default::default()
            },
            1000,
        );
        let mut state = Snapshot::default();
        assert!(cache.restore(&mut state, 60_000));
        let ages = state.battery_freshness.unwrap();
        assert_eq!(ages.left.timestamp_ms, Some(1000));
        assert_eq!(ages.case.timestamp_ms, Some(1000));
        assert!(ages.case.live && !ages.right.live && ages.right.timestamp_ms.is_none());
        assert!(refresh_live(&mut state, 1000 + FRESH_MS));
        assert!(!state.battery_freshness.unwrap().case.live);
        assert_eq!(
            state.battery_freshness.unwrap().case.timestamp_ms,
            Some(1000)
        );
    }
}
