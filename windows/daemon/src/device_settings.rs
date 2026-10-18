//! Device settings use the LibrePods android/rewrite protocol at 34076c85.
//! Keep requested values separate until the AirPods report them back.

use ntpods_ipc::{Equalizer, Snapshot};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const CONFIRM_TIMEOUT: Duration = Duration::from_secs(10);
const CHOICES_MAX_BYTES: u64 = 1024;

pub fn explicit_choice_id(id: u8) -> bool {
    matches!(id, 0x01 | 0x0A | 0x16 | 0x1A | 0x23 | 0x31 | 0x40)
}

fn choices_path(base: &Path, mac: u64) -> io::Result<PathBuf> {
    if mac == 0 || mac > 0xffff_ffff_ffff {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid Bluetooth address",
        ));
    }
    Ok(base
        .join("NTPods")
        .join(format!("device-choices-{mac:012x}.json")))
}

fn load_control_choices_at(base: &Path, mac: u64) -> BTreeMap<u8, u32> {
    let Some(bytes) = choices_path(base, mac).ok().and_then(|path| {
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .ok()?
            .take(CHOICES_MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        (bytes.len() as u64 <= CHOICES_MAX_BYTES).then_some(bytes)
    }) else {
        return BTreeMap::new();
    };
    let Ok(raw) = serde_json::from_slice::<BTreeMap<String, u32>>(&bytes) else {
        return BTreeMap::new();
    };
    if raw.len() > 7 {
        return BTreeMap::new();
    }
    raw.into_iter()
        .map(|(key, value)| {
            let id = key.parse::<u8>().ok()?;
            (key == id.to_string() && explicit_choice_id(id) && valid_control(id, value))
                .then_some((id, value))
        })
        .collect::<Option<BTreeMap<_, _>>>()
        .unwrap_or_default()
}

pub fn load_control_choices(mac: u64) -> BTreeMap<u8, u32> {
    std::env::var_os("LOCALAPPDATA")
        .map(|base| load_control_choices_at(Path::new(&base), mac))
        .unwrap_or_default()
}

fn save_control_choice_at(base: &Path, mac: u64, id: u8, value: u32) -> io::Result<()> {
    let path = choices_path(base, mac)?;
    if !explicit_choice_id(id) || !valid_control(id, value) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid device choice",
        ));
    }
    let mut choices = load_control_choices_at(base, mac);
    choices.insert(id, value);
    crate::atomic_file::write(
        &path,
        &serde_json::to_vec(&choices).map_err(io::Error::other)?,
    )
}

pub fn save_control_choice(mac: u64, id: u8, value: u32) -> io::Result<()> {
    let base = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "LOCALAPPDATA is unavailable"))?;
    save_control_choice_at(Path::new(&base), mac, id, value)
}

pub fn apply_control_choices(
    state: &mut Snapshot,
    pending: &PendingSettings,
    saved: BTreeMap<u8, u32>,
) -> bool {
    let mut changed = false;
    for (id, value) in saved {
        if !pending.in_flight.contains(&id)
            && !state.device_controls_requested.contains_key(&id)
            && !state.device_controls_pending.contains_key(&id)
            && crate::device_models::unreported_choice(state, id, value)
        {
            state.device_controls_requested.insert(id, value);
            changed = true;
        }
    }
    changed
}

pub fn supports_silent_allow_off(model: &str) -> bool {
    matches!(model, "A3063" | "A3064" | "A3065")
}

fn allow_off_choice_path(base: &Path, mac: u64) -> PathBuf {
    base.join("NTPods").join(format!("allow-off-{mac:012x}"))
}

fn load_allow_off_choice_at(base: &Path, mac: u64) -> Option<bool> {
    match std::fs::read_to_string(allow_off_choice_path(base, mac))
        .ok()?
        .trim()
    {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

pub fn load_allow_off_choice(mac: u64) -> Option<bool> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    load_allow_off_choice_at(Path::new(&base), mac)
}

pub fn save_allow_off_choice(mac: u64, enabled: bool) -> std::io::Result<()> {
    let base = std::env::var_os("LOCALAPPDATA").ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "LOCALAPPDATA is unavailable")
    })?;
    let path = allow_off_choice_path(Path::new(&base), mac);
    let result = crate::atomic_file::write(&path, if enabled { b"true" } else { b"false" });
    if result.is_err() {
        crate::log("Could not save the Allow Off preference");
    }
    result
}

pub fn is_toggle(id: u8) -> bool {
    matches!(
        id,
        0x0A | 0x1B | 0x20 | 0x25 | 0x26 | 0x28 | 0x31 | 0x34 | 0x35 | 0x36 | 0x3B
    )
}

pub fn valid_control(id: u8, value: u32) -> bool {
    if is_toggle(id) {
        return matches!(value, 1 | 2);
    }
    match id {
        0x01 | 0x17 | 0x18 => value <= 2,
        0x0D => (1..=4).contains(&value),
        0x16 => {
            value <= 0xFFFF && matches!(value & 0xFF, 1 | 5) && matches!((value >> 8) & 0xFF, 1 | 5)
        }
        0x1A => value <= 0x0F && value.count_ones() >= 2,
        // LibrePods docs/control_commands.md: reversed = 1, default = 2.
        0x1C => matches!(value, 1 | 2),
        // LibrePods sends the tone level followed by the fixed 0x50 byte.
        0x1F => value >> 8 == 0x50 && value & 0xFF <= 100,
        0x23 => value <= 2,
        0x24 => matches!(value, 0x0200 | 0x0300),
        0x2E | 0x40 => value <= 100,
        _ => false,
    }
}

pub fn valid_equalizer(eq: Equalizer) -> bool {
    matches!(eq.mode, 1 | 2) && eq.low <= 100 && eq.mid <= 100 && eq.high <= 100
}

#[derive(Default)]
pub struct PendingSettings {
    controls: BTreeMap<u8, Instant>,
    eq: Option<Instant>,
    unconfirmed_anc: Option<u32>,
    session: u64,
    reports: BTreeMap<u8, u64>,
    in_flight: BTreeSet<u8>,
}

#[derive(Clone, Copy)]
pub struct WriteToken {
    session: u64,
    report: u64,
    unreported_choice: bool,
}

impl PendingSettings {
    pub fn reset(&mut self, state: &mut Snapshot) {
        self.session = self.session.wrapping_add(1);
        self.reports.clear();
        self.in_flight.clear();
        self.controls.clear();
        self.eq = None;
        self.unconfirmed_anc = None;
        state.device_controls.clear();
        state.device_controls_requested.clear();
        state.device_controls_pending.clear();
        state.adaptive_strength_requested = None;
        state.allow_off_requested = None;
        state.device_settings_error = None;
        state.eq = None;
        state.eq_pending = None;
        state.loud_sound_reduction = None;
        state.loud_sound_reduction_pending = false;
        state.loud_sound_reduction_error = None;
        state.hearing_aid_settings = None;
        state.hearing_aid_pending = false;
        state.hearing_aid_error = None;
        state.hardware_diagnostics = None;
        state.hardware_diagnostics_pending = false;
        state.hardware_diagnostics_error = None;
        state.transparency = None;
        state.transparency_pending = false;
        state.transparency_error = None;
        state.conversational_awareness_pending = None;
        state.conversational_awareness = false;
        state.adaptive_volume = false;
        state.allow_off = false;
        state.anc = 0;
        state.anc_unconfirmed = false;
        // Identity metadata survives a same-pair reconnect; pair selection clears it.
    }

    fn token(&self, id: u8) -> WriteToken {
        WriteToken {
            session: self.session,
            report: *self.reports.get(&id).unwrap_or(&0),
            unreported_choice: false,
        }
    }

    pub fn session(&self) -> u64 {
        self.session
    }

    pub fn reported_during(&self, id: u8, token: WriteToken) -> bool {
        token.session == self.session && token.report != self.token(id).report
    }

    pub fn can_remember_choice(
        &self,
        state: &Snapshot,
        id: u8,
        value: u32,
        token: WriteToken,
    ) -> bool {
        token.session == self.session
            && (!self.reported_during(id, token) || state.device_controls.get(&id) == Some(&value))
    }

    pub fn begin_control(
        &mut self,
        state: &mut Snapshot,
        id: u8,
        value: u32,
        unreported_choice: bool,
    ) -> WriteToken {
        let mut token = self.token(id);
        token.unreported_choice = unreported_choice;
        self.in_flight.insert(id);
        if unreported_choice
            || id == 0x2E
            || (id == 0x34 && supports_silent_allow_off(&state.model))
        {
            self.control_attempted(state, id, value);
        } else {
            // Register before sending: the receive thread may observe the reply first.
            self.control_sent(state, id, value, Instant::now());
        }
        token
    }

    pub fn finish_control(
        &mut self,
        state: &mut Snapshot,
        id: u8,
        value: u32,
        token: WriteToken,
        sent: bool,
    ) -> bool {
        if token.session != self.session {
            return false;
        }
        self.in_flight.remove(&id);
        if !sent {
            self.controls.remove(&id);
            state.device_controls_pending.remove(&id);
            if id == 0x28 {
                state.conversational_awareness_pending = None;
            }
            state.device_settings_error =
                Some("Could not send the setting. Check the AirPods connection.".into());
        } else if token.unreported_choice {
            if token.report == self.token(id).report {
                state.device_controls_requested.insert(id, value);
                state.device_settings_error = None;
            }
        } else if (id == 0x2E || (id == 0x34 && supports_silent_allow_off(&state.model)))
            && token.report == self.token(id).report
        {
            self.control_sent(state, id, value, Instant::now());
        } else if let Some(sent_at) = self.controls.get_mut(&id) {
            *sent_at = Instant::now();
        }
        true
    }

    pub fn begin_eq(&mut self, state: &mut Snapshot, eq: Equalizer) -> WriteToken {
        self.eq_sent(state, eq, Instant::now());
        self.in_flight.insert(0xFF);
        self.token(0xFF)
    }

    pub fn finish_eq(&mut self, state: &mut Snapshot, token: WriteToken, sent: bool) {
        if token.session != self.session {
            return;
        }
        self.in_flight.remove(&0xFF);
        if !sent {
            self.eq = None;
            state.eq_pending = None;
            state.device_settings_error =
                Some("Could not send EQ. Check the AirPods connection.".into());
        } else if self.eq.is_some() {
            self.eq = Some(Instant::now());
        }
    }

    pub fn control_sent(&mut self, state: &mut Snapshot, id: u8, value: u32, now: Instant) {
        self.control_attempted(state, id, value);
        state.device_settings_error = None;
        if id == 0x2E {
            // Pro 3 saves strength silently and may round it; keep the last report separate.
            state.adaptive_strength_requested = Some(value);
            self.controls.remove(&id);
            state.device_controls_pending.remove(&id);
            return;
        }
        if id == 0x34 && supports_silent_allow_off(&state.model) {
            // Pro 3 applies Allow Off silently; a successful send is not a device report.
            state.allow_off_requested = Some(value == 1);
            self.controls.remove(&id);
            state.device_controls_pending.remove(&id);
            return;
        }
        self.controls.insert(id, now);
        state.device_controls_pending.insert(id, value);
        if id == 0x28 {
            state.conversational_awareness_pending = Some(value == 1);
        }
    }

    pub fn control_attempted(&mut self, state: &mut Snapshot, id: u8, value: u32) {
        if id == 0x0D {
            // A timeout or failed send does not prove that the mode stayed unchanged.
            self.unconfirmed_anc = Some(value);
            state.anc_unconfirmed = true;
        }
    }

    pub fn anc_to_reassert(&self, state: &Snapshot) -> Option<u8> {
        if self.unconfirmed_anc.is_some() {
            return None;
        }
        state
            .device_controls
            .get(&0x0D)
            .filter(|value| valid_control(0x0D, **value))
            .map(|value| *value as u8)
    }

    pub fn eq_sent(&mut self, state: &mut Snapshot, eq: Equalizer, now: Instant) {
        self.eq = Some(now);
        state.eq_pending = Some(eq);
        state.device_settings_error = None;
    }

    pub fn confirm_control(&mut self, state: &mut Snapshot, id: u8, value: u32) -> bool {
        if !valid_control(id, value) {
            return false;
        }
        let count = self.reports.entry(id).or_default();
        *count = count.wrapping_add(1);
        let strength_reported = id == 0x2E && state.adaptive_strength_requested.take().is_some();
        let allow_off_reported = id == 0x34 && state.allow_off_requested.take().is_some();
        let changed = state.device_controls.insert(id, value) != Some(value);
        let requested_cleared = state.device_controls_requested.remove(&id).is_some();
        let matched = state.device_controls_pending.get(&id) == Some(&value);
        if matched {
            state.device_controls_pending.remove(&id);
            self.controls.remove(&id);
            if id == 0x28 {
                state.conversational_awareness_pending = None;
            }
        }
        let anc_confirmed = id == 0x0D && self.unconfirmed_anc == Some(value);
        if anc_confirmed {
            self.unconfirmed_anc = None;
            state.anc_unconfirmed = false;
        }
        match id {
            0x0D => state.anc = value as u8,
            0x26 => state.adaptive_volume = value == 1,
            0x28 => state.conversational_awareness = value == 1,
            0x34 => state.allow_off = value == 1,
            _ => (),
        }
        changed
            || matched
            || requested_cleared
            || anc_confirmed
            || strength_reported
            || allow_off_reported
    }

    pub fn confirm_eq(&mut self, state: &mut Snapshot, eq: Equalizer) -> bool {
        if !valid_equalizer(eq) {
            return false;
        }
        let changed = state.eq != Some(eq);
        let matched = state.eq_pending == Some(eq);
        state.eq = Some(eq);
        if matched {
            state.eq_pending = None;
            self.eq = None;
        }
        changed || matched
    }

    pub fn expire(&mut self, state: &mut Snapshot, now: Instant) -> bool {
        let mut expired = false;
        self.controls.retain(|id, sent| {
            if self.in_flight.contains(id) {
                return true;
            }
            if now.saturating_duration_since(*sent) < CONFIRM_TIMEOUT {
                return true;
            }
            state.device_controls_pending.remove(id);
            if *id == 0x28 {
                state.conversational_awareness_pending = None;
            }
            expired = true;
            false
        });
        if !self.in_flight.contains(&0xFF)
            && self
                .eq
                .is_some_and(|sent| now.saturating_duration_since(sent) >= CONFIRM_TIMEOUT)
        {
            self.eq = None;
            state.eq_pending = None;
            expired = true;
        }
        if expired {
            state.device_settings_error =
                Some("AirPods did not confirm the last request within 10 seconds.".into());
        }
        expired
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pro_three() -> Snapshot {
        Snapshot {
            model: "A3063".into(),
            firmware: "9A348".into(),
            ..Default::default()
        }
    }

    #[test]
    fn unreported_choice_is_not_a_report_or_a_timed_request() {
        let mut pending = PendingSettings::default();
        let mut state = pro_three();
        assert!(crate::device_models::unreported_choice(&state, 0x23, 2));
        let token = pending.begin_control(&mut state, 0x23, 2, true);
        assert!(state.device_controls_pending.is_empty());
        assert!(pending.finish_control(&mut state, 0x23, 2, token, true));
        assert_eq!(state.device_controls_requested.get(&0x23), Some(&2));
        assert!(!state.device_controls.contains_key(&0x23));
        assert!(!pending.expire(&mut state, Instant::now() + CONFIRM_TIMEOUT));
        assert!(state.device_settings_error.is_none());
        assert!(pending.confirm_control(&mut state, 0x23, 0));
        assert_eq!(state.device_controls.get(&0x23), Some(&0));
        assert!(!state.device_controls_requested.contains_key(&0x23));
    }

    #[test]
    fn failed_unreported_send_preserves_the_last_choice_and_reset_clears_it() {
        let mut pending = PendingSettings::default();
        let mut state = pro_three();
        let first = pending.begin_control(&mut state, 0x23, 1, true);
        pending.finish_control(&mut state, 0x23, 1, first, true);
        let second = pending.begin_control(&mut state, 0x23, 2, true);
        pending.finish_control(&mut state, 0x23, 2, second, false);
        assert_eq!(state.device_controls_requested.get(&0x23), Some(&1));
        assert!(state.device_settings_error.is_some());
        pending.reset(&mut state);
        assert!(state.device_controls_requested.is_empty());
        assert!(!pending.finish_control(&mut state, 0x23, 2, second, true));
    }

    #[test]
    fn only_a_fresh_opposing_report_vetoes_remembering_a_successful_choice() {
        let mut pending = PendingSettings::default();
        let mut state = pro_three();
        pending.confirm_control(&mut state, 0x23, 0);

        let token = pending.begin_control(&mut state, 0x23, 2, false);
        pending.finish_control(&mut state, 0x23, 2, token, true);
        assert!(pending.can_remember_choice(&state, 0x23, 2, token));

        let token = pending.begin_control(&mut state, 0x23, 2, false);
        pending.confirm_control(&mut state, 0x23, 2);
        pending.finish_control(&mut state, 0x23, 2, token, true);
        assert!(pending.reported_during(0x23, token));
        assert!(pending.can_remember_choice(&state, 0x23, 2, token));
        assert!(state.device_controls_requested.is_empty());

        let token = pending.begin_control(&mut state, 0x23, 1, false);
        pending.confirm_control(&mut state, 0x23, 2);
        pending.finish_control(&mut state, 0x23, 1, token, true);
        assert!(!pending.can_remember_choice(&state, 0x23, 1, token));
    }

    #[test]
    fn cached_choices_are_per_pair_bounded_and_validated() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base =
            std::env::temp_dir().join(format!("ntpods-choices-{}-{unique}", std::process::id()));
        let first = 0x112233445566;
        let second = 0x665544332211;
        assert!(choices_path(&base, 0).is_err());
        assert!(choices_path(&base, 0x1_0000_0000_0000).is_err());
        assert!(save_control_choice_at(&base, first, 0x25, 1).is_err());
        assert!(save_control_choice_at(&base, first, 0x23, 3).is_err());
        assert!(load_control_choices_at(&base, first).is_empty());
        save_control_choice_at(&base, first, 0x23, 1).unwrap();
        let mut pending = PendingSettings::default();
        let mut state = pro_three();
        pending.confirm_control(&mut state, 0x23, 0);
        let token = pending.begin_control(&mut state, 0x23, 2, false);
        assert!(pending.finish_control(&mut state, 0x23, 2, token, true));
        assert!(pending.can_remember_choice(&state, 0x23, 2, token));
        save_control_choice_at(&base, first, 0x23, 2).unwrap();
        pending.reset(&mut state);
        assert!(apply_control_choices(
            &mut state,
            &pending,
            load_control_choices_at(&base, first)
        ));
        assert_eq!(state.device_controls_requested.get(&0x23), Some(&2));
        assert!(!state.device_controls.contains_key(&0x23));
        assert_eq!(load_control_choices_at(&base, first).get(&0x23), Some(&2));
        assert!(load_control_choices_at(&base, second).is_empty());
        let path = choices_path(&base, first).unwrap();
        for invalid in [
            "not json".to_string(),
            "{\"35\":3}".to_string(),
            "{\"37\":1}".to_string(),
            "{\"035\":2}".to_string(),
            "x".repeat(CHOICES_MAX_BYTES as usize + 1),
        ] {
            std::fs::write(&path, invalid).unwrap();
            assert!(load_control_choices_at(&base, first).is_empty());
        }
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn cache_restore_waits_for_idle_and_never_replaces_fresh_state() {
        let mut pending = PendingSettings::default();
        let mut state = pro_three();
        let saved = [(0x23, 2)].into();
        assert!(apply_control_choices(&mut state, &pending, saved));
        assert_eq!(state.device_controls_requested.get(&0x23), Some(&2));

        state.device_controls_requested.insert(0x23, 1);
        assert!(!apply_control_choices(
            &mut state,
            &pending,
            [(0x23, 2)].into()
        ));
        assert_eq!(state.device_controls_requested.get(&0x23), Some(&1));

        state.device_controls_requested.clear();
        let token = pending.begin_control(&mut state, 0x23, 1, true);
        assert!(!apply_control_choices(
            &mut state,
            &pending,
            [(0x23, 2)].into()
        ));
        pending.finish_control(&mut state, 0x23, 1, token, true);
        assert_eq!(state.device_controls_requested.get(&0x23), Some(&1));

        pending.confirm_control(&mut state, 0x23, 0);
        assert!(!apply_control_choices(
            &mut state,
            &pending,
            [(0x23, 2)].into()
        ));
        assert!(!state.device_controls_requested.contains_key(&0x23));

        pending.reset(&mut state);
        state.model = "future".into();
        assert!(!apply_control_choices(
            &mut state,
            &pending,
            [(0x23, 2)].into()
        ));
    }

    #[test]
    fn confirmation_timeout_starts_after_the_write_finishes() {
        let mut pending = PendingSettings::default();
        let mut state = Snapshot::default();
        let start = Instant::now();
        let token = pending.begin_control(&mut state, 0x28, 1, false);
        assert!(!pending.expire(&mut state, start + Duration::from_secs(30)));
        pending.finish_control(&mut state, 0x28, 1, token, true);
        assert!(!pending.expire(&mut state, Instant::now()));
        assert!(pending.expire(&mut state, Instant::now() + CONFIRM_TIMEOUT));
        let eq = Equalizer {
            mode: 2,
            low: 50,
            mid: 50,
            high: 50,
        };
        let token = pending.begin_eq(&mut state, eq);
        assert!(!pending.expire(&mut state, Instant::now() + CONFIRM_TIMEOUT));
        pending.finish_eq(&mut state, token, true);
        assert!(!pending.expire(&mut state, Instant::now()));
    }

    #[test]
    fn replies_during_a_send_are_not_replaced_by_pending_state() {
        let mut pending = PendingSettings::default();
        let mut state = Snapshot::default();
        let token = pending.begin_control(&mut state, 0x0D, 4, false);
        pending.confirm_control(&mut state, 0x0D, 4);
        pending.finish_control(&mut state, 0x0D, 4, token, true);
        assert_eq!(state.anc, 4);
        assert!(!state.anc_unconfirmed);
        assert!(state.device_controls_pending.is_empty());
        let eq = Equalizer {
            mode: 2,
            low: 20,
            mid: 50,
            high: 80,
        };
        let token = pending.begin_eq(&mut state, eq);
        pending.confirm_eq(&mut state, eq);
        pending.finish_eq(&mut state, token, true);
        assert_eq!(state.eq, Some(eq));
        assert!(state.eq_pending.is_none());
        let token = pending.begin_control(&mut state, 0x2E, 51, false);
        pending.confirm_control(&mut state, 0x2E, 50);
        pending.finish_control(&mut state, 0x2E, 51, token, true);
        assert!(state.adaptive_strength_requested.is_none());
        assert_eq!(state.device_controls[&0x2E], 50);
    }

    #[test]
    fn failed_and_old_session_writes_do_not_recreate_requests() {
        let mut pending = PendingSettings::default();
        let mut state = Snapshot::default();
        let token = pending.begin_control(&mut state, 0x28, 1, false);
        pending.finish_control(&mut state, 0x28, 1, token, false);
        assert!(state.device_controls_pending.is_empty());
        assert!(state.conversational_awareness_pending.is_none());
        assert!(state.device_settings_error.is_some());
        let token = pending.begin_control(&mut state, 0x2E, 23, false);
        state.loud_sound_reduction = Some(true);
        state.loud_sound_reduction_pending = true;
        state.loud_sound_reduction_error = Some("old connection".into());
        pending.reset(&mut state);
        assert!(state.loud_sound_reduction.is_none());
        assert!(!state.loud_sound_reduction_pending);
        assert!(state.loud_sound_reduction_error.is_none());
        assert!(!pending.finish_control(&mut state, 0x2E, 23, token, true));
        assert!(state.adaptive_strength_requested.is_none());
        let token = pending.begin_control(&mut state, 0x2E, 35, false);
        pending.finish_control(&mut state, 0x2E, 35, token, false);
        assert!(state.adaptive_strength_requested.is_none());
    }

    #[test]
    fn allow_off_cache_is_device_scoped_and_rejects_missing_or_malformed_choices() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("ntpods-allow-off-{unique}"));
        let first = 0x112233445566;
        let second = 0x665544332211;
        let path = allow_off_choice_path(&base, first);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        assert_eq!(load_allow_off_choice_at(&base, first), None);
        for (text, expected) in [
            ("true", Some(true)),
            ("false", Some(false)),
            ("null", None),
            ("", None),
            ("1", None),
            ("True", None),
            ("{\"allow_off\":true}", None),
        ] {
            std::fs::write(&path, text).unwrap();
            assert_eq!(load_allow_off_choice_at(&base, first), expected);
            assert_eq!(load_allow_off_choice_at(&base, second), None);
        }
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(base.join("NTPods")).unwrap();
        std::fs::remove_dir(base).unwrap();
    }

    #[test]
    fn pro3_allow_off_writes_stay_requested_without_a_confirmation_timeout() {
        for model in ["A3063", "A3064", "A3065"] {
            let mut pending = PendingSettings::default();
            let mut state = Snapshot {
                model: model.into(),
                ..Default::default()
            };
            let now = Instant::now();
            pending.control_sent(&mut state, 0x34, 1, now);
            assert_eq!(state.allow_off_requested, Some(true));
            assert!(!state.allow_off && !state.device_controls.contains_key(&0x34));
            assert!(state.device_controls_pending.is_empty());
            assert!(!pending.expire(&mut state, now + CONFIRM_TIMEOUT));
            assert!(state.device_settings_error.is_none());

            assert!(!pending.confirm_control(&mut state, 0x34, 0));
            assert_eq!(state.allow_off_requested, Some(true));
            assert!(pending.confirm_control(&mut state, 0x34, 1));
            assert!(state.allow_off && state.allow_off_requested.is_none());

            pending.control_sent(&mut state, 0x34, 2, now);
            assert_eq!(state.allow_off_requested, Some(false));
            assert_eq!(state.device_controls.get(&0x34), Some(&1));
            assert!(state.allow_off);
            // Even an unchanged, opposing report must clear the request and publish state.
            assert!(pending.confirm_control(&mut state, 0x34, 1));
            assert!(state.allow_off && state.allow_off_requested.is_none());
            assert!(!pending.confirm_control(&mut state, 0x34, 1));
        }
    }

    #[test]
    fn allow_off_send_failures_preserve_requests_and_reset_forgets_them() {
        let mut pending = PendingSettings::default();
        let mut state = Snapshot {
            model: "A3063".into(),
            ..Default::default()
        };
        pending.control_attempted(&mut state, 0x34, 1);
        assert!(state.allow_off_requested.is_none());
        pending.control_sent(&mut state, 0x34, 1, Instant::now());
        pending.control_attempted(&mut state, 0x34, 2);
        pending.confirm_control(&mut state, 0x28, 1);
        assert_eq!(state.allow_off_requested, Some(true));
        assert!(!state.allow_off && !state.device_controls.contains_key(&0x34));
        pending.reset(&mut state);
        assert!(state.allow_off_requested.is_none());
        assert!(!state.allow_off && state.device_controls.is_empty());
    }

    #[test]
    fn other_models_still_require_allow_off_confirmation() {
        for model in ["", "A3047", "A30630"] {
            let mut pending = PendingSettings::default();
            let mut state = Snapshot {
                model: model.into(),
                ..Default::default()
            };
            let now = Instant::now();
            pending.control_sent(&mut state, 0x34, 1, now);
            assert!(state.allow_off_requested.is_none());
            assert_eq!(state.device_controls_pending.get(&0x34), Some(&1));
            assert!(pending.expire(&mut state, now + CONFIRM_TIMEOUT));
            assert!(!state.allow_off && state.device_controls.is_empty());
        }
    }

    #[test]
    fn silent_strength_requests_do_not_replace_reports_or_time_out() {
        let mut pending = PendingSettings::default();
        let mut state = Snapshot::default();
        let now = Instant::now();
        pending.confirm_control(&mut state, 0x2E, 50);
        pending.control_sent(&mut state, 0x2E, 0, now);
        assert_eq!(state.adaptive_strength_requested, Some(0));
        assert_eq!(state.device_controls.get(&0x2E), Some(&50));
        assert!(state.device_controls_pending.is_empty());
        assert!(!pending.expire(&mut state, now + CONFIRM_TIMEOUT));
        assert!(state.device_settings_error.is_none());

        // Returning to the last reported value must still replace a newer request.
        pending.control_sent(&mut state, 0x2E, 50, now + CONFIRM_TIMEOUT);
        assert_eq!(state.adaptive_strength_requested, Some(50));
        assert!(pending.confirm_control(&mut state, 0x2E, 50));
        assert!(state.adaptive_strength_requested.is_none());

        pending.control_sent(&mut state, 0x2E, 25, now);
        assert!(!pending.confirm_control(&mut state, 0x2E, 101));
        assert_eq!(state.adaptive_strength_requested, Some(25));
        assert!(pending.confirm_control(&mut state, 0x2E, 50));
        assert!(state.adaptive_strength_requested.is_none());
        assert_eq!(state.device_controls.get(&0x2E), Some(&50));
    }

    #[test]
    fn strength_send_failures_and_other_reports_preserve_the_last_request() {
        let mut pending = PendingSettings::default();
        let mut state = Snapshot::default();
        let now = Instant::now();
        pending.control_attempted(&mut state, 0x2E, 100);
        assert!(state.adaptive_strength_requested.is_none());
        pending.control_sent(&mut state, 0x2E, 0, now);
        pending.control_attempted(&mut state, 0x2E, 50);
        pending.confirm_control(&mut state, 0x0D, 4);
        assert_eq!(state.adaptive_strength_requested, Some(0));

        pending.control_sent(&mut state, 0x28, 1, now);
        assert!(pending.expire(&mut state, now + CONFIRM_TIMEOUT));
        assert!(state.device_settings_error.is_some());
        assert_eq!(state.adaptive_strength_requested, Some(0));
        state.model = "A3063".into();
        state.firmware = "old session firmware".into();
        state.serial = "old session serial".into();
        pending.reset(&mut state);
        assert!(state.adaptive_strength_requested.is_none());
        assert!(state.device_controls.is_empty());
        assert_eq!(state.model, "A3063");
        assert_eq!(state.firmware, "old session firmware");
        assert_eq!(state.serial, "old session serial");
    }

    #[test]
    fn captured_default_swipe_duration_is_retained_and_all_choices_confirm() {
        let report = [4, 0, 4, 0, 9, 0, 0x23, 0, 0, 0, 0];
        let (id, value) = crate::aap::parse_device_control(&report).unwrap();
        let mut pending = PendingSettings::default();
        let mut state = Snapshot::default();
        assert!(pending.confirm_control(&mut state, id, value));
        assert_eq!(state.device_controls.get(&0x23), Some(&0));
        for value in 0..=2 {
            assert!(valid_control(id, value));
            pending.control_sent(&mut state, id, value, Instant::now());
            pending.confirm_control(&mut state, id, value);
            assert_eq!(state.device_controls.get(&id), Some(&value));
            assert!(state.device_controls_pending.is_empty());
        }
        assert!(!pending.confirm_control(&mut state, id, 3));
        assert_eq!(state.device_controls.get(&id), Some(&2));
    }

    #[test]
    fn controls_accept_only_documented_values_and_preserve_two_byte_settings() {
        for (id, valid, invalid) in [
            (0x01, 0, 3),
            (0x0D, 4, 0),
            (0x16, 0x0501, 0x0500),
            (0x17, 2, 3),
            (0x18, 1, 256),
            (0x1A, 0x0A, 0x08),
            (0x1C, 1, 0),
            (0x1C, 2, 3),
            (0x1F, 0x5064, 0x5065),
            (0x23, 0, 3),
            (0x24, 0x0200, 2),
            (0x2E, 100, 101),
            (0x40, 0, 256),
        ] {
            assert!(valid_control(id, valid), "id={id:#x}");
            assert!(!valid_control(id, invalid), "id={id:#x}");
            assert!(!valid_control(id, valid | 0x10000), "id={id:#x}");
        }
        for id in 0..=255 {
            if is_toggle(id) {
                assert!(valid_control(id, 1) && valid_control(id, 2));
                assert!(!valid_control(id, 0) && !valid_control(id, 3));
            }
        }
        assert!(!valid_control(0x03, 1), "reset is not a user setting");
        assert!(
            !valid_control(0x2C, 1),
            "audiogram setup has a separate path"
        );
        assert!(
            !valid_control(0x1F, 75),
            "tone writes require the fixed second byte"
        );
    }

    #[test]
    fn stale_reports_do_not_confirm_a_new_request_and_timeout_preserves_device_state() {
        let mut pending = PendingSettings::default();
        let mut state = Snapshot::default();
        let start = Instant::now();
        assert!(pending.confirm_control(&mut state, 0x28, 2));
        pending.control_sent(&mut state, 0x28, 1, start);
        assert!(!pending.confirm_control(&mut state, 0x28, 2));
        assert_eq!(state.conversational_awareness_pending, Some(true));
        assert!(!pending.expire(
            &mut state,
            start + CONFIRM_TIMEOUT - Duration::from_millis(1)
        ));
        assert!(pending.expire(&mut state, start + CONFIRM_TIMEOUT));
        assert_eq!(state.device_controls.get(&0x28), Some(&2));
        assert!(!state.conversational_awareness);
        assert!(state.device_controls_pending.is_empty());
        assert!(state.conversational_awareness_pending.is_none());
        assert!(state.device_settings_error.is_some());
        assert!(pending.confirm_control(&mut state, 0x28, 1));
        assert!(state.conversational_awareness);
        assert_eq!(
            state.device_settings_error.as_deref(),
            Some("AirPods did not confirm the last request within 10 seconds.")
        );
        pending.control_sent(&mut state, 0x28, 2, start + CONFIRM_TIMEOUT);
        assert!(state.device_settings_error.is_none());
    }

    #[test]
    fn matching_report_confirms_only_its_setting_and_reconnect_forgets_stale_values() {
        let mut pending = PendingSettings::default();
        let mut state = Snapshot::default();
        let now = Instant::now();
        let eq = Equalizer {
            mode: 2,
            low: 40,
            mid: 50,
            high: 60,
        };
        pending.eq_sent(&mut state, eq, now);
        pending.control_sent(&mut state, 0x23, 2, now);
        assert!(state.eq.is_none());
        assert!(pending.confirm_eq(&mut state, eq));
        assert_eq!(state.eq, Some(eq));
        assert!(state.eq_pending.is_none());
        assert_eq!(state.device_controls_pending.get(&0x23), Some(&2));
        assert!(pending.confirm_control(&mut state, 0x23, 2));
        assert!(!pending.expire(&mut state, now + CONFIRM_TIMEOUT));
        state.transparency = Some(ntpods_ipc::TransparencySettings {
            enabled: true,
            left_amplification: 0.2,
            right_amplification: 0.3,
            left_tone: 0.0,
            right_tone: 0.0,
            left_conversation_boost: false,
            right_conversation_boost: false,
            left_ambient_noise_reduction: 0.5,
            right_ambient_noise_reduction: 0.5,
        });
        state.transparency_pending = true;
        state.transparency_error = Some("Previous pair's request failed.".into());
        let ear = ntpods_ipc::HardwareDiagnosticEar { measured_at_unix_seconds: 1,
            general: 0, measurement: 0, component_failures: Some(0), unknown_bits: 0 };
        state.hardware_diagnostics = Some(ntpods_ipc::HardwareDiagnostics { read_at_unix_seconds: 2, left: ear, right: ear });
        state.hardware_diagnostics_pending = true;
        state.hardware_diagnostics_error = Some("Previous pair.".into());
        pending.reset(&mut state);
        assert!(state.eq.is_none() && state.eq_pending.is_none());
        assert!(state.device_controls.is_empty() && state.device_controls_pending.is_empty());
        assert!(state.transparency.is_none() && !state.transparency_pending);
        assert!(state.transparency_error.is_none());
        assert!(state.hardware_diagnostics.is_none() && !state.hardware_diagnostics_pending && state.hardware_diagnostics_error.is_none());
        assert!(
            state.auto_pause,
            "device reconnect must not change the host preference"
        );
    }

    #[test]
    fn unchanged_matching_ca_report_still_publishes_confirmation() {
        let mut pending = PendingSettings::default();
        let mut state = Snapshot::default();
        assert!(pending.confirm_control(&mut state, 0x28, 2));
        pending.control_sent(&mut state, 0x28, 2, Instant::now());
        assert!(pending.confirm_control(&mut state, 0x28, 2));
        assert_eq!(state.conversational_awareness_pending, None);
        assert!(!pending.confirm_control(&mut state, 0x28, 2));
    }

    #[test]
    fn background_anc_writes_wait_for_the_requested_mode_even_after_timeout() {
        let mut pending = PendingSettings::default();
        let mut state = Snapshot::default();
        let now = Instant::now();
        assert_eq!(pending.anc_to_reassert(&state), None);
        pending.confirm_control(&mut state, 0x0D, 2);
        assert_eq!(pending.anc_to_reassert(&state), Some(2));
        pending.control_sent(&mut state, 0x0D, 3, now);
        assert_eq!(pending.anc_to_reassert(&state), None);
        pending.confirm_control(&mut state, 0x0D, 2);
        assert_eq!(
            pending.anc_to_reassert(&state),
            None,
            "an older reply cannot undo the request"
        );
        pending.expire(&mut state, now + CONFIRM_TIMEOUT);
        assert!(state.device_controls_pending.is_empty());
        assert_eq!(
            pending.anc_to_reassert(&state),
            None,
            "timeout does not prove the write failed"
        );
        pending.confirm_control(&mut state, 0x0D, 4);
        assert_eq!(pending.anc_to_reassert(&state), None);
        pending.confirm_control(&mut state, 0x0D, 3);
        assert_eq!(pending.anc_to_reassert(&state), Some(3));

        pending.control_attempted(&mut state, 0x0D, 2);
        assert_eq!(
            pending.anc_to_reassert(&state),
            None,
            "failed sends may still reach the device"
        );
        pending.reset(&mut state);
        assert_eq!(pending.anc_to_reassert(&state), None);
        pending.confirm_control(&mut state, 0x0D, 4);
        assert_eq!(pending.anc_to_reassert(&state), Some(4));
    }

    #[test]
    fn invalid_eq_and_control_reports_cannot_change_confirmed_state() {
        let mut pending = PendingSettings::default();
        let mut state = Snapshot::default();
        for eq in [
            Equalizer {
                mode: 0,
                low: 50,
                mid: 50,
                high: 50,
            },
            Equalizer {
                mode: 2,
                low: 101,
                mid: 50,
                high: 50,
            },
            Equalizer {
                mode: 2,
                low: 50,
                mid: 255,
                high: 50,
            },
            Equalizer {
                mode: 2,
                low: 50,
                mid: 50,
                high: 101,
            },
        ] {
            assert!(!pending.confirm_eq(&mut state, eq));
        }
        assert!(!pending.confirm_control(&mut state, 0x23, 3));
        assert!(state.eq.is_none() && state.device_controls.is_empty());
    }

    #[test]
    fn adaptive_strength_requires_confirmed_adaptive_without_an_unanswered_mode_change() {
        let mut pending = PendingSettings::default();
        let mut state = Snapshot::default();
        let now = Instant::now();
        assert_ne!(pending.anc_to_reassert(&state), Some(4));
        pending.confirm_control(&mut state, 0x0D, 3);
        assert_ne!(pending.anc_to_reassert(&state), Some(4));
        pending.control_sent(&mut state, 0x0D, 4, now);
        assert_ne!(pending.anc_to_reassert(&state), Some(4));
        pending.confirm_control(&mut state, 0x0D, 4);
        assert_eq!(pending.anc_to_reassert(&state), Some(4));
        pending.control_sent(&mut state, 0x0D, 3, now);
        pending.expire(&mut state, now + CONFIRM_TIMEOUT);
        assert_ne!(pending.anc_to_reassert(&state), Some(4));
        pending.confirm_control(&mut state, 0x0D, 4);
        assert_ne!(
            pending.anc_to_reassert(&state),
            Some(4),
            "an opposed report cannot resolve the outstanding request"
        );
        pending.reset(&mut state);
        pending.confirm_control(&mut state, 0x0D, 4);
        assert_eq!(pending.anc_to_reassert(&state), Some(4));
    }

    #[test]
    fn anc_uncertainty_stays_visible_after_failure_and_timeout_until_readback_or_reset() {
        let mut pending = PendingSettings::default();
        let mut state = Snapshot::default();
        let now = Instant::now();
        pending.confirm_control(&mut state, 0x0D, 4);
        pending.control_attempted(&mut state, 0x0D, 3);
        assert!(
            state.anc_unconfirmed,
            "a failed send may still reach the device"
        );
        pending.confirm_control(&mut state, 0x28, 1);
        pending.confirm_control(&mut state, 0x0D, 4);
        assert!(state.anc_unconfirmed);
        pending.confirm_control(&mut state, 0x0D, 3);
        assert!(!state.anc_unconfirmed);

        pending.control_sent(&mut state, 0x0D, 3, now);
        pending.expire(&mut state, now + CONFIRM_TIMEOUT);
        assert!(state.anc_unconfirmed && state.device_controls_pending.is_empty());
        assert!(
            pending.confirm_control(&mut state, 0x0D, 3),
            "an unchanged matching report must publish latch clearance"
        );
        assert!(!state.anc_unconfirmed);
        pending.control_attempted(&mut state, 0x0D, 4);
        pending.reset(&mut state);
        assert!(!state.anc_unconfirmed);
    }
}
