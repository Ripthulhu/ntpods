use crate::{device_settings, Command, Ctx, Event};
use ntpods_ipc::{CompanionPreference, CompanionPreferences, ConversationPolicy};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::{io, path::PathBuf};

pub const KEYS: &[&str] = &[
    "host_auto_pause",
    "theme",
    "conversation_policy",
    "connection_popup_enabled",
    "cache_disconnected_battery",
    "call_takeover_enabled",
    "call_return_enabled",
    "desired_microphone_side",
    "desired_ear_detection",
    "desired_noise_modes_cycle",
    "desired_swipe_duration",
    "desired_case_sounds",
    "desired_case_volume",
    "desired_allow_off",
    "desired_hold_actions",
];

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    host: BTreeMap<String, CompanionPreference>,
    devices: BTreeMap<String, BTreeMap<String, CompanionPreference>>,
    local_call_consent: bool,
    #[serde(default)]
    observed: BTreeMap<String, CompanionPreference>,
}

pub struct Store {
    saved: Saved,
    dirty: BTreeSet<String>,
    pub peer_prefs: bool,
    pub peer_calls: bool,
    error: Option<String>,
    storage: Option<PathBuf>,
    migrate_legacy: bool,
}

pub enum Merge {
    Applied,
    Unchanged,
    Stale(CompanionPreference),
    Unsupported,
    Failed,
}

fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "Invalid shared preference")
}
fn path() -> io::Result<PathBuf> {
    Ok(
        PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or_else(invalid)?)
            .join("NTPods/companion-preferences.json"),
    )
}
fn global(key: &str) -> bool {
    key == "theme"
}
fn desired(key: &str) -> bool {
    key.starts_with("desired_")
}
fn device(address: u64) -> String {
    format!("{address:012x}")
}
fn record(key: &str, value: Value) -> CompanionPreference {
    CompanionPreference {
        key: key.into(),
        rev: "0000000000000001".into(),
        origin: "pc".into(),
        value,
    }
}
fn revision(value: &str) -> Option<u64> {
    (value.len() == 16
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
    .then(|| u64::from_str_radix(value, 16).ok())
    .flatten()
    .filter(|n| *n != 0)
}
fn rank(record: &CompanionPreference) -> Option<(u64, u8)> {
    Some((
        revision(&record.rev)?,
        match record.origin.as_str() {
            "android" => 0,
            "pc" => 1,
            _ => return None,
        },
    ))
}
fn word(value: &Value, allowed: &[&str]) -> bool {
    value.as_str().is_some_and(|value| allowed.contains(&value))
}
pub fn valid(key: &str, value: &Value) -> bool {
    match key {
        "host_auto_pause"
        | "connection_popup_enabled"
        | "cache_disconnected_battery"
        | "call_takeover_enabled"
        | "call_return_enabled" => value.is_boolean(),
        "theme" => word(value, &["system", "light", "dark"]),
        "conversation_policy" => {
            serde_json::from_value::<ConversationPolicy>(value.clone()).is_ok_and(|v| v.is_valid())
        }
        "desired_microphone_side" => word(value, &["unset", "auto", "right", "left"]),
        "desired_ear_detection" | "desired_case_sounds" | "desired_allow_off" => {
            word(value, &["unset", "on", "off"])
        }
        "desired_swipe_duration" => word(value, &["unset", "0", "1", "2"]),
        "desired_case_volume" => value.as_str().is_some_and(|s| {
            s == "unset"
                || s.parse::<u8>()
                    .ok()
                    .is_some_and(|n| n <= 100 && s == n.to_string())
        }),
        "desired_noise_modes_cycle" => value.as_str().is_some_and(|s| {
            s == "unset"
                || s.len() == 2
                    && s.bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    && u8::from_str_radix(s, 16)
                        .ok()
                        .is_some_and(|n| n <= 15 && n.count_ones() >= 2)
        }),
        "desired_hold_actions" => value.as_object().is_some_and(|v| {
            v.len() == 2
                && v.get("left").zip(v.get("right")).is_some_and(|(l, r)| {
                    word(l, &["unset", "listening", "assistant"])
                        && word(r, &["unset", "listening", "assistant"])
                        && (l == "unset") == (r == "unset")
                })
        }),
        _ => false,
    }
}

impl Store {
    #[cfg(test)]
    pub(crate) fn for_test() -> Self {
        Self::load_at(None, false)
    }

    pub fn load() -> Self {
        Self::load_at(path().ok(), true)
    }
    fn load_at(storage: Option<PathBuf>, migrate_legacy: bool) -> Self {
        let mut error = None;
        let saved = match storage.as_ref().ok_or_else(invalid).and_then(std::fs::read) {
            Ok(bytes) if bytes.len() <= 65536 => serde_json::from_slice::<Saved>(&bytes)
                .ok()
                .filter(valid_saved)
                .unwrap_or_else(|| {
                    error = Some("Shared preferences could not be read.".into());
                    Saved::default()
                }),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Saved::default(),
            _ => {
                error = Some("Shared preferences could not be read.".into());
                Saved::default()
            }
        };
        let mut store = Self {
            saved,
            dirty: BTreeSet::new(),
            peer_prefs: false,
            peer_calls: false,
            error,
            storage,
            migrate_legacy,
        };
        if store.error.is_none() && !store.saved.host.contains_key("theme") {
            let theme = legacy(migrate_legacy, "winui-settings.json")
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .and_then(|v| v.get("ThemeIndex")?.as_u64())
                .map_or("system", |n| match n {
                    1 => "light",
                    2 => "dark",
                    _ => "system",
                });
            store
                .saved
                .host
                .insert("theme".into(), record("theme", json!(theme)));
            if store.persist().is_err() {
                store.saved.host.remove("theme");
                store.error = Some("Shared preferences could not be saved.".into());
            }
        }
        store
    }
    fn persist(&self) -> io::Result<()> {
        let bytes = serde_json::to_vec(&self.saved)?;
        if bytes.len() > 65536 {
            return Err(invalid());
        }
        crate::atomic_file::write(self.storage.as_ref().ok_or_else(invalid)?, &bytes)
    }
    pub fn ensure_device(&mut self, address: u64) -> io::Result<()> {
        if self.error.is_some() {
            return Err(invalid());
        }
        if address == 0 {
            return Ok(());
        }
        if address > 0xffff_ffff_ffff {
            return Err(invalid());
        }
        if self.saved.devices.contains_key(&device(address)) {
            return Ok(());
        }
        if self.saved.devices.len() >= 16 {
            return Err(invalid());
        }
        let mut entries = BTreeMap::new();
        let auto_pause =
            legacy(self.migrate_legacy, "auto-pause").is_none_or(|s| s.trim() != "false");
        let limit = legacy(self.migrate_legacy, "conversation-volume")
            .and_then(|s| s.trim().parse::<u8>().ok())
            .filter(|n| *n <= 100)
            .unwrap_or(25);
        for (key, value) in [
            ("host_auto_pause", json!(auto_pause)),
            (
                "conversation_policy",
                json!(ConversationPolicy::from_legacy(limit)),
            ),
            ("connection_popup_enabled", json!(true)),
            ("cache_disconnected_battery", json!(true)),
            ("call_takeover_enabled", json!(false)),
            ("call_return_enabled", json!(true)),
        ] {
            entries.insert(key.into(), record(key, value));
        }
        let choices = if self.migrate_legacy {
            device_settings::load_control_choices(address)
        } else {
            BTreeMap::new()
        };
        for key in KEYS.iter().copied().filter(|key| desired(key)) {
            let value = if key == "desired_allow_off" {
                (self
                    .migrate_legacy
                    .then(|| device_settings::load_allow_off_choice(address))
                    .flatten())
                .map(|v| json!(if v { "on" } else { "off" }))
                .unwrap_or(json!("unset"))
            } else if key == "desired_hold_actions" {
                json!({"left":"unset","right":"unset"})
            } else {
                json!("unset")
            };
            entries.insert(key.into(), record(key, value));
        }
        for (id, value) in choices {
            if let Some((key, value)) = from_control(id, value) {
                entries.insert(key.into(), record(key, value));
            }
        }
        self.saved.devices.insert(device(address), entries);
        if let Err(e) = self.persist() {
            self.saved.devices.remove(&device(address));
            return Err(e);
        }
        Ok(())
    }
    pub fn get(&self, address: u64, key: &str) -> Option<&CompanionPreference> {
        if !global(key) {
            self.saved.devices.get(&device(address))?.get(key)
        } else {
            self.saved.host.get(key)
        }
    }
    fn put(&mut self, address: u64, value: CompanionPreference) {
        if !global(&value.key) {
            self.saved
                .devices
                .entry(device(address))
                .or_default()
                .insert(value.key.clone(), value);
        } else {
            self.saved.host.insert(value.key.clone(), value);
        }
    }
    fn remove(&mut self, address: u64, key: &str) {
        if global(key) {
            self.saved.host.remove(key);
        } else if let Some(entries) = self.saved.devices.get_mut(&device(address)) {
            entries.remove(key);
        }
    }
    pub fn records(&self, address: u64) -> Vec<CompanionPreference> {
        KEYS.iter()
            .filter_map(|key| self.get(address, key).cloned())
            .collect()
    }
    pub fn status(&self, address: u64) -> CompanionPreferences {
        CompanionPreferences {
            device_id: (address != 0).then(|| device(address)),
            records: self.records(address),
            local_call_consent: self.saved.local_call_consent,
            peer_prefs: self.peer_prefs,
            peer_calls: self.peer_calls,
            error: self.error.clone(),
        }
    }
    pub fn local(
        &mut self,
        address: u64,
        key: &str,
        value: Value,
    ) -> io::Result<CompanionPreference> {
        if !valid(key, &value) {
            return Err(invalid());
        }
        if self.error.is_some() || !global(key) && address == 0 {
            return Err(invalid());
        }
        self.ensure_device(address)?;
        let old = self.get(address, key).cloned();
        if let Some(old) = old.as_ref().filter(|r| r.value == value) {
            return Ok(old.clone());
        }
        let clock_key = format!(
            "{}:{key}",
            if global(key) {
                "global".into()
            } else {
                device(address)
            }
        );
        let next = old
            .as_ref()
            .and_then(|r| revision(&r.rev))
            .unwrap_or(0)
            .max(
                self.saved
                    .observed
                    .get(&clock_key)
                    .and_then(|record| revision(&record.rev))
                    .unwrap_or(0),
            )
            .checked_add(1)
            .ok_or_else(invalid)?;
        let new = CompanionPreference {
            key: key.into(),
            rev: format!("{next:016x}"),
            origin: "pc".into(),
            value,
        };
        self.put(address, new.clone());
        if let Err(error) = self.persist() {
            if let Some(old) = old {
                self.put(address, old);
            } else {
                self.remove(address, key);
            }
            return Err(error);
        }
        self.dirty.insert(key.into());
        Ok(new)
    }
    pub fn merge(&mut self, address: u64, incoming: CompanionPreference) -> io::Result<Merge> {
        if !valid(&incoming.key, &incoming.value) || rank(&incoming).is_none() {
            return Err(invalid());
        }
        if self.error.is_some() || !global(&incoming.key) && address == 0 {
            return Err(invalid());
        }
        self.ensure_device(address)?;
        let old = self.get(address, &incoming.key).cloned();
        let observed_key = format!(
            "{}:{}",
            if global(&incoming.key) {
                "global".into()
            } else {
                device(address)
            },
            incoming.key
        );
        if let Some(observed) = self.saved.observed.get(&observed_key) {
            match rank(&incoming).cmp(&rank(observed)) {
                std::cmp::Ordering::Less => return Ok(Merge::Stale(observed.clone())),
                std::cmp::Ordering::Equal if incoming.value != observed.value => {
                    return Err(invalid())
                }
                _ => {}
            }
        }
        if let Some(old) = &old {
            match rank(&incoming).cmp(&rank(old)) {
                std::cmp::Ordering::Less => return Ok(Merge::Stale(old.clone())),
                std::cmp::Ordering::Equal if incoming.value == old.value => {
                    return Ok(Merge::Unchanged)
                }
                std::cmp::Ordering::Equal => return Err(invalid()),
                _ => {}
            }
        }
        self.put(address, incoming.clone());
        if self.persist().is_err() {
            if let Some(old) = old {
                self.put(address, old);
            } else {
                self.remove(address, &incoming.key);
            }
            return Ok(Merge::Failed);
        }
        Ok(Merge::Applied)
    }
    pub fn receive(
        &mut self,
        address: u64,
        incoming: CompanionPreference,
        supported: bool,
    ) -> io::Result<Merge> {
        if !valid(&incoming.key, &incoming.value) || rank(&incoming).is_none() {
            return Err(invalid());
        }
        // Local corruption/capacity/I/O failure is not a peer schema failure.
        if self.ensure_device(address).is_err() {
            return Ok(Merge::Failed);
        }
        if supported {
            return self.merge(address, incoming);
        }
        match self.observe_unsupported(address, &incoming) {
            Ok(()) => Ok(Merge::Unsupported),
            Err(error) if error.kind() == io::ErrorKind::InvalidData => Err(error),
            Err(_) => Ok(Merge::Failed),
        }
    }
    pub fn take_dirty(&mut self, address: u64) -> Vec<CompanionPreference> {
        std::mem::take(&mut self.dirty)
            .into_iter()
            .filter_map(|key| self.get(address, &key).cloned())
            .collect()
    }
    pub fn observe_unsupported(
        &mut self,
        address: u64,
        incoming: &CompanionPreference,
    ) -> io::Result<()> {
        if self.error.is_some()
            || !valid(&incoming.key, &incoming.value)
            || rank(incoming).is_none()
            || !global(&incoming.key) && address == 0
        {
            return Err(invalid());
        }
        self.ensure_device(address)?;
        if let Some(old) = self.get(address, &incoming.key) {
            if rank(old) == rank(incoming) && old.value != incoming.value {
                return Err(invalid());
            }
        }
        let key = format!(
            "{}:{}",
            if global(&incoming.key) {
                "global".into()
            } else {
                device(address)
            },
            incoming.key
        );
        let old = self.saved.observed.get(&key).cloned();
        if let Some(old) = old.as_ref() {
            if rank(old) == rank(incoming) && old.value != incoming.value {
                return Err(invalid());
            }
            if rank(old) >= rank(incoming) {
                return Ok(());
            }
        }
        self.saved.observed.insert(key.clone(), incoming.clone());
        if let Err(e) = self.persist() {
            if let Some(old) = old {
                self.saved.observed.insert(key, old);
            } else {
                self.saved.observed.remove(&key);
            }
            return Err(io::Error::new(io::ErrorKind::Other, e));
        }
        Ok(())
    }
    pub fn consent(&self) -> bool {
        self.saved.local_call_consent
    }
    pub fn set_consent(&mut self, enabled: bool) -> io::Result<()> {
        if self.error.is_some() {
            return Err(invalid());
        }
        let old = self.saved.local_call_consent;
        self.saved.local_call_consent = enabled;
        if let Err(error) = self.persist() {
            self.saved.local_call_consent = old;
            return Err(error);
        }
        Ok(())
    }
}

pub fn from_control(id: u8, value: u32) -> Option<(&'static str, Value)> {
    let pair = match id {
        0x01 => (
            "desired_microphone_side",
            json!(match value {
                0 => "auto",
                1 => "right",
                2 => "left",
                _ => return None,
            }),
        ),
        0x0a => (
            "desired_ear_detection",
            json!(if value == 2 {
                "on"
            } else if value == 1 {
                "off"
            } else {
                return None;
            }),
        ),
        0x16 => (
            "desired_hold_actions",
            json!({"left":match value&255 {1=>"listening",5=>"assistant",_=>return None},"right":match value>>8 {1=>"listening",5=>"assistant",_=>return None}}),
        ),
        0x1a => ("desired_noise_modes_cycle", json!(format!("{value:02x}"))),
        0x23 => ("desired_swipe_duration", json!(value.to_string())),
        0x31 => (
            "desired_case_sounds",
            json!(if value == 1 {
                "on"
            } else if value == 2 {
                "off"
            } else {
                return None;
            }),
        ),
        0x40 => ("desired_case_volume", json!(value.to_string())),
        _ => return None,
    };
    valid(pair.0, &pair.1).then_some(pair)
}
pub fn to_control(key: &str, value: &Value) -> Option<(u8, Option<u32>)> {
    let text = value.as_str().unwrap_or("");
    let id = match key {
        "desired_microphone_side" => 1,
        "desired_ear_detection" => 10,
        "desired_hold_actions" => 22,
        "desired_noise_modes_cycle" => 26,
        "desired_swipe_duration" => 35,
        "desired_case_sounds" => 49,
        "desired_case_volume" => 64,
        _ => return None,
    };
    if text == "unset" {
        return Some((id, None));
    }
    let n = match key {
        "desired_microphone_side" => match text {
            "auto" => 0,
            "right" => 1,
            "left" => 2,
            _ => return None,
        },
        "desired_ear_detection" => {
            if text == "on" {
                2
            } else {
                1
            }
        }
        "desired_case_sounds" => {
            if text == "on" {
                1
            } else {
                2
            }
        }
        "desired_noise_modes_cycle" => u32::from_str_radix(text, 16).ok()?,
        "desired_hold_actions" => {
            let left = value.get("left")?.as_str()?;
            let right = value.get("right")?.as_str()?;
            if left == "unset" {
                return Some((id, None));
            }
            (if left == "listening" { 1 } else { 5 })
                | ((if right == "listening" { 1 } else { 5 }) << 8)
        }
        _ => text.parse().ok()?,
    };
    Some((id, Some(n)))
}
pub fn supported(ctx: &Ctx, key: &str, value: &Value) -> bool {
    if !valid(key, value) {
        return false;
    }
    if !desired(key) {
        return true;
    }
    let state = ctx.state.lock().unwrap();
    if key == "desired_allow_off" {
        return device_settings::supports_silent_allow_off(&state.model);
    }
    let Some((id, n)) = to_control(key, value) else {
        return false;
    };
    let probe = n.unwrap_or(match id {
        1 | 35 | 64 => 0,
        22 => 0x0101,
        26 => 6,
        _ => 1,
    });
    crate::device_models::explicit_choice(&state, id, probe)
        && crate::device_models::control(&state, id, probe)
}
pub fn publish(ctx: &Ctx) {
    let mac = ctx.mac();
    let mut store = ctx.companion_preferences.lock().unwrap();
    let _ = store.ensure_device(mac);
    let preferences = store.status(mac);
    drop(store);
    ctx.send_event(&Event::CompanionPreferences { preferences });
}
pub fn value(ctx: &Ctx, key: &str) -> Option<Value> {
    ctx.companion_preferences
        .lock()
        .unwrap()
        .get(ctx.mac(), key)
        .map(|r| r.value.clone())
}
pub fn enabled(ctx: &Ctx, key: &str) -> bool {
    value(ctx, key).and_then(|v| v.as_bool()).unwrap_or(false)
}
pub fn apply(ctx: &Ctx, key: &str, value: &Value) {
    match key {
        "host_auto_pause" => {
            let _ = crate::update_auto_pause(
                &ctx.auto_pause,
                &ctx.state,
                value.as_bool().unwrap_or(true),
                || Ok(()),
            );
            ctx.push_state();
        }
        "conversation_policy" => {
            if let Ok(policy) = serde_json::from_value::<ConversationPolicy>(value.clone()) {
                ctx.conv_duck.lock().unwrap().set_policy(policy.clone());
                ctx.state.lock().unwrap().conversation_volume = policy
                    .first
                    .parse::<f32>()
                    .unwrap_or(25.0)
                    .round()
                    .clamp(0.0, 100.0)
                    as u8;
                ctx.push_state();
            }
        }
        key if desired(key) => {
            let changed = {
                let pending = ctx.settings_pending.lock().unwrap();
                let mut state = ctx.state.lock().unwrap();
                project_choice(&mut state, &pending, key, value)
            };
            if changed {
                ctx.push_state();
            }
        }
        _ => {}
    }
}
fn project_choice(
    state: &mut ntpods_ipc::Snapshot,
    pending: &device_settings::PendingSettings,
    key: &str,
    value: &Value,
) -> bool {
    if !valid(key, value) {
        return false;
    }
    if key == "desired_allow_off" {
        let choice = match value.as_str() {
            Some("on") => Some(true),
            Some("off") => Some(false),
            _ => None,
        };
        device_settings::project_allow_off_choice(state, pending, choice)
    } else if let Some((id, value)) = to_control(key, value) {
        device_settings::project_control_choice(state, pending, id, value)
    } else {
        false
    }
}
pub fn initialize(ctx: &Ctx) {
    {
        let mut store = ctx.companion_preferences.lock().unwrap();
        let _ = store.ensure_device(ctx.mac());
    }
    for key in KEYS {
        if let Some(v) = value(ctx, key) {
            apply(ctx, key, &v);
        }
    }
    publish(ctx);
}
pub fn record_control(ctx: &Ctx, address: u64, id: u8, n: u32) {
    if let Some((key, v)) = from_control(id, n) {
        if ctx
            .companion_preferences
            .lock()
            .unwrap()
            .local(address, key, v)
            .is_err()
        {
            ctx.state.lock().unwrap().device_settings_error =
                Some("Sent the setting, but could not save its shared choice.".into());
        }
        publish(ctx);
    }
}
pub fn record_allow_off(ctx: &Ctx, address: u64, on: bool) {
    let saved = ctx.companion_preferences.lock().unwrap().local(
        address,
        "desired_allow_off",
        json!(if on { "on" } else { "off" }),
    );
    if saved.is_err() {
        ctx.state.lock().unwrap().device_settings_error =
            Some("Sent the setting, but could not save its shared choice.".into());
    }
    publish(ctx);
}
pub fn allow_off_choice(ctx: &Ctx, address: u64, legacy: Option<bool>) -> Option<bool> {
    let mut store = ctx.companion_preferences.lock().unwrap();
    let _ = store.ensure_device(address);
    match store
        .get(address, "desired_allow_off")
        .and_then(|r| r.value.as_str())
    {
        Some("on") => Some(true),
        Some("off") => Some(false),
        Some("unset") => None,
        _ => legacy,
    }
}
pub fn choices(ctx: &Ctx, address: u64, mut old: BTreeMap<u8, u32>) -> BTreeMap<u8, u32> {
    let mut store = ctx.companion_preferences.lock().unwrap();
    let _ = store.ensure_device(address);
    for r in store.records(address) {
        if let Some((id, n)) = to_control(&r.key, &r.value) {
            if let Some(n) = n {
                old.insert(id, n);
            } else {
                old.remove(&id);
            }
        }
    }
    old
}
pub fn apply_local(ctx: &Ctx, cmd: &Command) -> bool {
    let (key, value, device_id, request_id) = match cmd {
        Command::CompanionSetPreference {
            key,
            value,
            device_id,
            request_id,
        } => (
            key.as_str(),
            value.clone(),
            device_id.as_deref(),
            *request_id,
        ),
        Command::SetAutoPause { on } => ("host_auto_pause", json!(on), None, 0),
        Command::SetConversationVolume { percent } => (
            "conversation_policy",
            json!(ConversationPolicy::from_legacy(*percent)),
            None,
            0,
        ),
        Command::CompanionCallConsent {
            enabled,
            device_id,
            request_id,
        } => {
            let _actions = ctx.companion_actions.lock().unwrap();
            let ok = u64::from_str_radix(device_id, 16).ok() == Some(ctx.mac())
                && ctx.mac() != 0
                && ctx
                    .companion_preferences
                    .lock()
                    .unwrap()
                    .set_consent(*enabled)
                    .is_ok();
            if ok {
                crate::companion::microphone_started(ctx);
            }
            ctx.send_event(&Event::CommandResult {
                command: "companion_call_consent".into(),
                request_id: *request_id,
                ok,
                message: if ok {
                    "Call consent updated."
                } else {
                    "Call consent could not be saved."
                }
                .into(),
            });
            publish(ctx);
            return true;
        }
        _ => return false,
    };
    let _actions = ctx.companion_actions.lock().unwrap();
    let current = device_id.is_none_or(|id| u64::from_str_radix(id, 16).ok() == Some(ctx.mac()));
    let outcome = if !current || !supported(ctx, key, &value) {
        Err(invalid())
    } else {
        ctx.companion_preferences
            .lock()
            .unwrap()
            .local(ctx.mac(), key, value.clone())
            .map(|_| ())
    };
    if outcome.is_ok() {
        apply(ctx, key, &value);
        if matches!(key, "call_takeover_enabled" | "call_return_enabled") {
            crate::companion::microphone_started(ctx);
        }
    }
    ctx.send_event(&Event::CommandResult {
        command: "companion_set_preference".into(),
        request_id,
        ok: outcome.is_ok(),
        message: if outcome.is_ok() {
            "Shared preference saved."
        } else {
            "Shared preference could not be saved or is unsupported."
        }
        .into(),
    });
    publish(ctx);
    true
}

fn legacy(enabled: bool, name: &str) -> Option<String> {
    enabled
        .then(|| std::env::var_os("LOCALAPPDATA"))
        .flatten()
        .and_then(|p| std::fs::read_to_string(PathBuf::from(p).join("NTPods").join(name)).ok())
}
fn valid_saved(saved: &Saved) -> bool {
    saved.observed.len() <= 241
        && saved.observed.iter().all(|(key, record)| {
            rank(record).is_some()
                && valid(&record.key, &record.value)
                && key.split_once(':').is_some_and(|(scope, key)| {
                    key == record.key
                        && if global(key) {
                            scope == "global"
                        } else {
                            scope.len() == 12
                                && scope
                                    .bytes()
                                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                                && u64::from_str_radix(scope, 16).is_ok_and(|n| n > 0)
                                && saved.devices.contains_key(scope)
                        }
                })
        })
        && saved.host.iter().all(|(key, r)| {
            global(key) && key == &r.key && valid(key, &r.value) && rank(r).is_some()
        })
        && saved.devices.len() <= 16
        && saved.devices.iter().all(|(address, entries)| {
            address.len() == 12
                && address
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                && u64::from_str_radix(address, 16).is_ok_and(|n| n > 0 && n <= 0xffff_ffff_ffff)
                && entries.len() <= KEYS.len()
                && entries.iter().all(|(key, r)| {
                    !global(key) && key == &r.key && valid(key, &r.value) && rank(r).is_some()
                })
        })
}

#[cfg(test)]
#[path = "companion_prefs_tests.rs"]
mod tests;
