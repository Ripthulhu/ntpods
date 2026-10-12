//! Shared IPC protocol between `ntpodsd` (the driver-owning daemon) and the
//! NTPods UIs (the tray + the full app). Newline-delimited JSON over a Windows
//! named pipe — see `../../../docs/windows/daemon-ipc/PLAN.md`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Two one-directional named pipes (a single duplex pipe deadlocks: a Windows
/// *synchronous* handle serializes I/O, so a blocking ReadFile for commands
/// stalls the WriteFile for events on the same handle). The daemon only WRITES
/// events on `PIPE_EVENTS` and only READS commands on `PIPE_CMDS`, so no handle
/// ever does both directions concurrently.
pub const PIPE_EVENTS: &str = r"\\.\pipe\NTPods-events";
pub const PIPE_CMDS: &str = r"\\.\pipe\NTPods-cmds";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MicMode {
    #[default]
    Automatic,
    KeepActive,
    Manual,
}

fn deserialize_stop_delay<'de, D: serde::Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    let seconds = u32::deserialize(d)?;
    if seconds <= 3600 {
        Ok(seconds)
    } else {
        Err(serde::de::Error::custom(
            "microphone delay exceeds 3600 seconds",
        ))
    }
}

/// Battery levels (percent), each optional — a packet may carry only some.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Battery {
    pub left: Option<u8>,
    pub right: Option<u8>,
    pub case: Option<u8>,
    pub headphone: Option<u8>,
    // Per-component charging flag (default false for backward-compatible parsing).
    #[serde(default)]
    pub left_charging: bool,
    #[serde(default)]
    pub right_charging: bool,
    #[serde(default)]
    pub case_charging: bool,
    #[serde(default)]
    pub headphone_charging: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatteryObservation {
    pub timestamp_ms: Option<u64>,
    pub live: bool,
    /// Last reported presence; independent of the retained battery percentage.
    #[serde(default)]
    pub present: Option<bool>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatteryFreshness {
    pub left: BatteryObservation,
    pub right: BatteryObservation,
    pub case: BatteryObservation,
    pub headphone: BatteryObservation,
}

/// The daemon's authoritative state, pushed to clients on connect and on change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub connected: bool,
    /// The AAP session is nominally up (`connected`) but the channel has gone
    /// silent — the AirPods are attached to Windows yet not talking to us, so the
    /// hi-res mic and every live notification are dead. The app surfaces "Repair
    /// connection" on this, because `connected` alone stays true forever in that
    /// state and used to hide the button exactly when it was needed.
    /// (serde default: older snapshots.)
    #[serde(default)]
    pub link_stale: bool,
    pub dev_name: String,
    /// Stable paired-device identity; never display it as the product name.
    #[serde(default)]
    pub device_id: String,
    pub battery: Battery,
    /// Independent report ages; None identifies a legacy snapshot without this metadata.
    #[serde(default)]
    pub battery_freshness: Option<BatteryFreshness>,
    /// A current paired AAP session or a recent identity-verified BLE report.
    #[serde(default)]
    pub battery_live: bool,
    /// Unix milliseconds of the last battery report; cached levels retain their age.
    #[serde(default)]
    pub battery_timestamp_ms: Option<u64>,
    /// Empty before a report, then "aap" or "ble". BLE does not imply audio connection.
    #[serde(default)]
    pub battery_source: String,
    /// Allow passive nearby-battery scans during playback; capture still inhibits scanning.
    #[serde(default)]
    pub ble_scan_while_connected: bool,
    /// Noise-control mode: 0 = unknown, 1 = off, 2 = ANC, 3 = transparency, 4 = adaptive.
    pub anc: u8,
    /// An attempted mode change still needs a matching report or a fresh session.
    #[serde(default)]
    pub anc_unconfirmed: bool,
    /// An app is currently recording from the virtual mic (hi-res stream on).
    pub mic_recording: bool,
    /// Running Windows capture clients; separate from AirPods stream state.
    #[serde(default)]
    pub mic_capture_active: bool,
    /// Stream is armed or has not yet been confirmed stopped.
    #[serde(default)]
    pub mic_uplink_active: bool,
    /// idle, starting, capturing, stopping, recovering, waiting_for_ear, unavailable, or error.
    #[serde(default)]
    pub mic_state: String,
    #[serde(default)]
    pub mic_error: Option<String>,
    /// The virtual mic driver is loaded (its NTPodsMic control device opens).
    /// False on Windows before 10 2004, where the installer skips the mic driver,
    /// or when its install failed; the app then greys out the mic controls.
    /// (serde default: older snapshots.)
    #[serde(default)]
    pub mic_available: bool,
    /// Global permission for the AirPods microphone, retained for older clients.
    pub auto_mode: bool,
    #[serde(default)]
    pub mic_mode: MicMode,
    #[serde(default, deserialize_with = "deserialize_stop_delay")]
    pub mic_stop_delay_secs: u32,
    /// Requested manual state; never restored after restart or disconnection.
    #[serde(default)]
    pub mic_manual_active: bool,
    /// Conversational Awareness: lower the volume automatically when you speak.
    pub conversational_awareness: bool,
    /// Requested setting awaiting a device report, which may arrive on reconnect.
    #[serde(default)]
    pub conversational_awareness_pending: Option<bool>,
    /// Host playback limit during speech; independent of AirPods settings.
    #[serde(
        default = "default_conversation_volume",
        deserialize_with = "deserialize_percent"
    )]
    pub conversation_volume: u8,
    /// Adaptive/Personalized Volume: adjust the volume to the environment.
    pub adaptive_volume: bool,
    /// Allow the "Off" option in noise control (vs. only ANC/Transparency/Adaptive).
    pub allow_off: bool,
    /// Allow Off sent successfully to Pro 3, but not yet reported by the AirPods.
    #[serde(default)]
    pub allow_off_requested: Option<bool>,
    /// Validated four-byte AACP settings, confirmed by the current device session.
    #[serde(default, deserialize_with = "deserialize_device_controls")]
    pub device_controls: BTreeMap<u8, u32>,
    #[serde(default, deserialize_with = "deserialize_device_controls")]
    pub device_controls_pending: BTreeMap<u8, u32>,
    /// Strength sent successfully, but not yet reported by the AirPods; never confirmed state.
    #[serde(default, deserialize_with = "deserialize_adaptive_strength_requested")]
    pub adaptive_strength_requested: Option<u32>,
    #[serde(default)]
    pub device_settings_error: Option<String>,
    #[serde(default)]
    pub eq: Option<Equalizer>,
    #[serde(default)]
    pub eq_pending: Option<Equalizer>,
    /// ATT handle 0x1B, independently confirmed; absent until a successful read.
    #[serde(default)]
    pub loud_sound_reduction: Option<bool>,
    #[serde(default)]
    pub loud_sound_reduction_pending: bool,
    #[serde(default)]
    pub loud_sound_reduction_error: Option<String>,
    /// Pause on ear removal and resume only playback paused by this app.
    #[serde(default = "default_auto_pause")]
    pub auto_pause: bool,
    /// System output volume 0..=100 (the default render endpoint — the AirPods
    /// when they're active). Owned by the daemon so it can duck for CA.
    pub volume: u8,
    /// The output is muted.
    pub muted: bool,
    /// Latest validated heart-rate BPM (AirPods Pro 3 RTBuddy). `None` when HR
    /// monitoring is off or no sample has been decoded yet. Opt-in (drains battery).
    pub heart_rate: Option<u16>,
    #[serde(default)]
    pub heart_rate_enabled: bool,
    /// Unix milliseconds of the last validated sample; null after expiry or disconnect.
    #[serde(default)]
    pub heart_rate_timestamp_ms: Option<u64>,
    /// Live movement is opt-in and is never restored after restart or disconnect.
    #[serde(default)]
    pub head_tracking_enabled: bool,
    /// idle, starting, streaming, waiting_for_ear, or error.
    #[serde(default = "default_head_tracking_state")]
    pub head_tracking_state: String,
    #[serde(default)]
    pub head_tracking_error: Option<String>,
    /// Device metadata parsed from the 0x1D packet (model number, firmware, serial).
    /// Empty until that packet arrives. The serial is sensitive — the app keeps
    /// this info hidden behind a reveal toggle. (serde default: older snapshots.)
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub firmware: String,
    #[serde(default)]
    pub serial: String,
    /// Every host the AirPods report themselves connected to (AAP 0x2E) — this PC
    /// plus whatever else is sharing them. Multipoint is invisible to Windows, so
    /// this list is the only place the user can see that a phone is also holding
    /// the buds. Empty until the first 0x2E arrives. (serde default: older snapshots.)
    #[serde(default)]
    pub multipoint: Vec<ConnectedDevice>,
}

fn default_auto_pause() -> bool {
    true
}

fn default_head_tracking_state() -> String {
    "idle".into()
}

fn default_conversation_volume() -> u8 {
    25
}

fn deserialize_percent<'de, D>(deserializer: D) -> Result<u8, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let percent = u8::deserialize(deserializer)?;
    if percent > 100 {
        return Err(serde::de::Error::custom("percent must be 0..=100"));
    }
    Ok(percent)
}

fn deserialize_adaptive_strength_requested<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<u32>::deserialize(deserializer)?;
    if value.is_some_and(|strength| strength > 100) {
        return Err(serde::de::Error::custom(
            "Adaptive strength must be between 0 and 100",
        ));
    }
    Ok(value)
}

fn deserialize_device_controls<'de, D>(deserializer: D) -> Result<BTreeMap<u8, u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Internally tagged events buffer JSON keys as strings before reading Snapshot.
    BTreeMap::<String, u32>::deserialize(deserializer)?
        .into_iter()
        .map(|(key, value)| {
            let id = key.parse::<u8>().map_err(serde::de::Error::custom)?;
            if key != id.to_string() {
                return Err(serde::de::Error::custom(
                    "device setting keys must be decimal bytes",
                ));
            }
            Ok((id, value))
        })
        .collect()
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            connected: false,
            link_stale: false,
            dev_name: String::new(),
            device_id: String::new(),
            battery: Battery::default(),
            battery_live: false,
            battery_freshness: Some(BatteryFreshness::default()),
            battery_timestamp_ms: None,
            battery_source: String::new(),
            ble_scan_while_connected: false,
            anc: 0,
            anc_unconfirmed: false,
            mic_recording: false,
            mic_capture_active: false,
            mic_uplink_active: false,
            mic_state: String::new(),
            mic_error: None,
            mic_available: false,
            auto_mode: false,
            mic_mode: MicMode::Automatic,
            mic_stop_delay_secs: 0,
            mic_manual_active: false,
            conversational_awareness: false,
            conversational_awareness_pending: None,
            conversation_volume: default_conversation_volume(),
            adaptive_volume: false,
            allow_off: false,
            allow_off_requested: None,
            device_controls: BTreeMap::new(),
            device_controls_pending: BTreeMap::new(),
            adaptive_strength_requested: None,
            device_settings_error: None,
            eq: None,
            eq_pending: None,
            loud_sound_reduction: None,
            loud_sound_reduction_pending: false,
            loud_sound_reduction_error: None,
            auto_pause: true,
            volume: 0,
            muted: false,
            heart_rate: None,
            heart_rate_enabled: false,
            heart_rate_timestamp_ms: None,
            head_tracking_enabled: false,
            head_tracking_state: default_head_tracking_state(),
            head_tracking_error: None,
            model: String::new(),
            firmware: String::new(),
            serial: String::new(),
            multipoint: Vec::new(),
        }
    }
}

/// Native AirPods music EQ: mode 1 recommended, 2 custom; bands 0..=100, neutral 50.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Equalizer {
    pub mode: u8,
    pub low: u8,
    pub mid: u8,
    pub high: u8,
}

/// One host connected to the AirPods, as reported by the buds themselves.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ConnectedDevice {
    /// 48-bit Bluetooth address, formatted `aa:bb:cc:dd:ee:ff`.
    pub address: String,
    /// True when this entry is one of our own radios.
    pub is_this_pc: bool,
    /// Best-effort human label: the Windows name when the device happens to be
    /// paired here, else a type guess, else "Unknown device".
    pub name: String,
    /// Device kind for the UI to pick an icon: "pc", "phone", "laptop", "watch",
    /// "tablet", "audio" or "unknown".
    pub kind: String,
}

/// A toggleable AAP control-command setting (the `id` byte of a 0x09 control
/// command). Values are sent as 0x01 (on) / 0x02 (off).
pub mod feature {
    pub const ADAPTIVE_VOLUME: u8 = 0x26;
    pub const CONVERSATIONAL_AWARENESS: u8 = 0x28;
    pub const ALLOW_OFF: u8 = 0x34;
}

/// Client → daemon. (Volume stays client-side via WASAPI — not the exclusive
/// resource — so it isn't routed through the daemon.)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Command-pipe liveness probe; produces no snapshot or notification.
    Ping,
    /// Sent on connect; the daemon replies with a `State` snapshot.
    Hello {
        kind: ClientKind,
    },
    /// Set noise-control mode (1..=4).
    SetAnc {
        mode: u8,
    },
    /// Set the hi-res mic mode (auto-enable and/or manual override).
    SetMicMode {
        auto: bool,
        manual: bool,
    },
    SetMicPolicy {
        mode: MicMode,
        #[serde(deserialize_with = "deserialize_stop_delay")]
        stop_delay_secs: u32,
    },
    SetMicManualActive {
        active: bool,
    },
    /// Toggle an AAP control-command setting (see the `feature` module).
    SetFeature {
        feature: u8,
        on: bool,
    },
    /// Set a raw AAP control-command value (opcode 0x09) — e.g. Adaptive noise
    /// strength (id 0x2E, value 0..=100). For settings that aren't on/off.
    SetControl {
        id: u8,
        value: u8,
    },
    /// Set an allowlisted AACP setting. Value contains four little-endian bytes.
    SetDeviceControl {
        id: u8,
        value: u32,
    },
    SetEq {
        mode: u8,
        low: u8,
        mid: u8,
        high: u8,
    },
    SetAutoPause {
        on: bool,
    },
    SetBleScanWhileConnected {
        enabled: bool,
    },
    SetConversationVolume {
        #[serde(deserialize_with = "deserialize_percent")]
        percent: u8,
    },
    /// Nudge the output volume by `delta` percent (the daemon owns volume).
    StepVolume {
        delta: i32,
    },
    /// Set the output volume to an absolute percent 0..=100 (e.g. a slider).
    SetVolume {
        percent: u8,
    },
    /// Mute/unmute the output.
    ToggleMute,
    /// Enable/disable AirPods Pro 3 heart-rate monitoring (opt-in; off by
    /// default because it drains battery). On sends the RTBuddy enable sequence;
    /// off sends the stop frame and clears `heart_rate`.
    SetHeartRate {
        on: bool,
    },
    /// Start/stop live movement for this command client; closing its pipe stops it.
    SetHeadTracking {
        on: bool,
    },
    GetLoudSoundReduction,
    SetLoudSoundReduction {
        on: bool,
    },
    /// AirPods Pro 3 hearing assistance (accessibility amplification). On enables
    /// hearing-assist over AAP (0x2C/0x33), switches to Transparency, and writes the
    /// full settings to the ATT/GATT (PSM 0x001F, handle 0x2A); off disables it.
    /// `left_eq`/`right_eq` are the per-person audiogram: 8 bands of hearing loss in
    /// dB HL (250/500/1k/2k/3k/4k/6k/8k Hz). `amplification` 0..=1 overall gain,
    /// `balance` -1(L)..=1(R), `tone` -1..=1, `ambient_noise_reduction`/`own_voice`
    /// 0..=1, plus conversation boost.
    SetHearingAid {
        on: bool,
        left_eq: Vec<f32>,
        right_eq: Vec<f32>,
        amplification: f32,
        balance: f32,
        tone: f32,
        conversation_boost: bool,
        ambient_noise_reduction: f32,
        own_voice: f32,
    },
    /// Start the AAP session (the user accepted the "connect?" prompt).
    Connect,
    /// Rebuild the Windows audio route without touching the AAP session: toggle
    /// the AirPods' audio services off and on, which is exactly what pressing
    /// Disconnect then Connect does by hand.
    ///
    /// For the failure where the buds go silent while EVERYTHING reports healthy —
    /// the AAP channel at Ok(2), the Bluetooth device connected, the endpoint OK,
    /// the AirPods still selected — and so nothing can detect it automatically.
    /// One click instead of changing noise mode and restarting the video.
    RebuildAudio,
    /// Force a clean reconnect (the "Repair connection" button): re-assert the
    /// connect request AND drop the current driver so the daemon reopens a fresh
    /// AAP session + ATT channel — used to recover a wedged / desynced link where
    /// the OS still shows the AirPods connected but our session is stale.
    RepairConnection,
    /// Refresh reported settings through a fresh AAP session, only while capture is idle.
    RefreshDeviceSettings,
    /// Release the AAP control session (the "Disconnect" button). Stops
    /// controlling the AirPods without stealing the audio link from the OS.
    Disconnect,
    /// Rename the AirPods (sends the 0x1A rename command).
    SetName {
        name: String,
        #[serde(default)]
        request_id: u64,
    },
    /// Request a fresh `State` snapshot.
    GetState,
    /// Stop the daemon too (e.g. from the tray's "Quit").
    Shutdown,
}

/// Daemon → client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// Full state, pushed on connect and whenever it changes.
    State(Snapshot),
    /// At most 25 Hz; signed sensor values, not degrees or calibrated acceleration.
    HeadTracking {
        timestamp_ms: u64,
        horizontal: i16,
        vertical: i16,
    },
    /// A notification for the client to render with its overlay UI.
    Overlay { title: String, body: String },
    /// The device is nearby (BLE) but not connected — the client shows a
    /// clickable card; a click sends `Command::Connect`.
    ConnectPrompt { name: String },
    CommandResult {
        command: String,
        #[serde(default)]
        request_id: u64,
        ok: bool,
        message: String,
    },
}

/// Which UI a client is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    Tray,
    App,
}

/// Serialize a message as one NDJSON line (trailing `\n`).
pub fn to_line<T: Serialize>(v: &T) -> String {
    let mut s = serde_json::to_string(v).unwrap_or_default();
    s.push('\n');
    s
}

/// Parse one NDJSON line into a message.
pub fn from_line<T: for<'de> Deserialize<'de>>(line: &str) -> Option<T> {
    serde_json::from_str(line.trim()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_presence_preserves_unknown_and_explicit_absence() {
        let legacy: BatteryObservation =
            serde_json::from_str(r#"{"timestamp_ms":1000,"live":false}"#).unwrap();
        assert_eq!(legacy.present, None);
        for present in [None, Some(false), Some(true)] {
            let observation = BatteryObservation { present, ..legacy };
            let text = serde_json::to_string(&observation).unwrap();
            assert_eq!(serde_json::from_str::<BatteryObservation>(&text).unwrap(), observation);
        }
        for invalid in ["0", "1", "\"false\"", "{}"] {
            assert!(serde_json::from_str::<BatteryObservation>(&format!(
                r#"{{"timestamp_ms":1000,"live":false,"present":{invalid}}}"#
            )).is_err());
        }
    }

    #[test]
    fn live_movement_event_has_bounded_signed_axes_and_legacy_state_defaults_off() {
        let event = Event::HeadTracking {
            timestamp_ms: 42,
            horizontal: -32768,
            vertical: 32767,
        };
        assert!(matches!(
            from_line::<Event>(&to_line(&event)),
            Some(Event::HeadTracking {
                timestamp_ms: 42,
                horizontal: -32768,
                vertical: 32767
            })
        ));
        for axis in ["-32769", "32768", "null", "1.5", "\"12\""] {
            assert!(from_line::<Event>(&format!(
                r#"{{"event":"head_tracking","timestamp_ms":42,"horizontal":{axis},"vertical":0}}"#
            ))
            .is_none());
        }
        assert!(
            from_line::<Event>(r#"{"event":"head_tracking","horizontal":0,"vertical":0}"#)
                .is_none()
        );
        let mut legacy = serde_json::to_value(Snapshot::default()).unwrap();
        for key in [
            "head_tracking_enabled",
            "head_tracking_state",
            "head_tracking_error",
        ] {
            legacy.as_object_mut().unwrap().remove(key);
        }
        let old: Snapshot = serde_json::from_value(legacy).unwrap();
        assert!(!old.head_tracking_enabled);
        assert_eq!(old.head_tracking_state, "idle");
        assert!(old.head_tracking_error.is_none());
    }

    #[test]
    fn microphone_policy_is_bounded_and_legacy_snapshots_remain_automatic() {
        assert!(matches!(
            from_line::<Command>(&to_line(&Command::Ping)),
            Some(Command::Ping)
        ));
        for mode in [MicMode::Automatic, MicMode::KeepActive, MicMode::Manual] {
            for delay in [0, 30, 3600] {
                let command = Command::SetMicPolicy {
                    mode,
                    stop_delay_secs: delay,
                };
                assert!(matches!(from_line::<Command>(&to_line(&command)),
                    Some(Command::SetMicPolicy { mode: actual, stop_delay_secs })
                    if mode == actual && delay == stop_delay_secs));
            }
        }
        for delay in ["-1", "3601", "4294967296", "1.5", "null", "\"30\""] {
            assert!(from_line::<Command>(&format!(
                r#"{{"cmd":"set_mic_policy","mode":"automatic","stop_delay_secs":{delay}}}"#
            ))
            .is_none());
        }
        let mut legacy = serde_json::to_value(Snapshot::default()).unwrap();
        for key in ["mic_mode", "mic_stop_delay_secs", "mic_manual_active"] {
            legacy.as_object_mut().unwrap().remove(key);
        }
        let state: Snapshot = serde_json::from_value(legacy).unwrap();
        assert_eq!(state.mic_mode, MicMode::Automatic);
        assert_eq!(state.mic_stop_delay_secs, 0);
        assert!(!state.mic_manual_active);
    }

    #[test]
    fn rename_results_preserve_correlation_and_older_requests_default_to_zero() {
        let event = Event::CommandResult {
            command: "set_name".into(),
            request_id: 42,
            ok: true,
            message: "Name request sent.".into(),
        };
        assert!(matches!(from_line::<Event>(&to_line(&event)),
            Some(Event::CommandResult { command, request_id: 42, ok: true, .. }) if command == "set_name"));
        assert!(matches!(
            from_line::<Command>(r#"{"cmd":"set_name","name":"AirPods"}"#),
            Some(Command::SetName { request_id: 0, .. })
        ));
        let command = Command::SetName {
            name: "AirPods".into(),
            request_id: 43,
        };
        assert!(matches!(
            from_line::<Command>(&to_line(&command)),
            Some(Command::SetName { request_id: 43, .. })
        ));
    }

    #[test]
    fn conversation_volume_accepts_percentages_and_defaults_for_older_clients() {
        for percent in [0, 25, 100] {
            let command = Command::SetConversationVolume { percent };
            assert!(matches!(from_line::<Command>(&to_line(&command)),
                Some(Command::SetConversationVolume { percent: actual }) if actual == percent));
        }
        let mut state = serde_json::to_value(Event::State(Snapshot::default())).unwrap();
        state.as_object_mut().unwrap().remove("conversation_volume");
        let Event::State(actual) = from_line(&to_line(&state)).unwrap() else {
            panic!("expected state")
        };
        assert_eq!(actual.conversation_volume, 25);
        for invalid in ["-1", "101", "256", "1.5", "true", "null", "\"25\""] {
            assert!(from_line::<Command>(&format!(
                r#"{{"cmd":"set_conversation_volume","percent":{invalid}}}"#
            ))
            .is_none());
            state["conversation_volume"] = serde_json::from_str(invalid).unwrap();
            assert!(from_line::<Event>(&to_line(&state)).is_none());
        }
    }

    #[test]
    fn tagged_state_roundtrip_preserves_numeric_keys_and_full_control_values() {
        let state = Snapshot {
            device_controls: [(0x24, 0x0200), (0x1F, 0x5064)].into(),
            allow_off_requested: Some(false),
            adaptive_strength_requested: Some(25),
            anc_unconfirmed: true,
            loud_sound_reduction: Some(false),
            loud_sound_reduction_pending: true,
            loud_sound_reduction_error: Some("AirPods did not apply the setting.".into()),
            eq: Some(Equalizer {
                mode: 2,
                low: 0,
                mid: 50,
                high: 100,
            }),
            ..Default::default()
        };
        let Event::State(actual) = from_line(&to_line(&Event::State(state.clone()))).unwrap()
        else {
            panic!("expected state");
        };
        assert_eq!(actual, state);
        let mut legacy = serde_json::to_value(Event::State(state)).unwrap();
        for field in [
            "anc_unconfirmed",
            "auto_pause",
            "device_controls",
            "device_controls_pending",
            "adaptive_strength_requested",
            "allow_off_requested",
            "eq",
            "eq_pending",
            "loud_sound_reduction",
            "loud_sound_reduction_pending",
            "loud_sound_reduction_error",
            "device_settings_error",
        ] {
            legacy.as_object_mut().unwrap().remove(field);
        }
        let Event::State(actual) = from_line(&to_line(&legacy)).unwrap() else {
            panic!("expected legacy state");
        };
        assert!(actual.auto_pause && actual.device_controls.is_empty() && actual.eq.is_none());
        assert!(!actual.anc_unconfirmed);
        assert!(actual.adaptive_strength_requested.is_none());
        assert!(actual.allow_off_requested.is_none());
        assert!(actual.loud_sound_reduction.is_none());
        assert!(!actual.loud_sound_reduction_pending);
        assert!(actual.loud_sound_reduction_error.is_none());
    }

    #[test]
    fn loud_sound_commands_require_a_boolean() {
        assert!(matches!(
            from_line::<Command>(&to_line(&Command::GetLoudSoundReduction)),
            Some(Command::GetLoudSoundReduction)
        ));
        for on in [false, true] {
            assert!(
                matches!(from_line::<Command>(&to_line(&Command::SetLoudSoundReduction { on })),
                Some(Command::SetLoudSoundReduction { on: actual }) if actual == on)
            );
        }
        for invalid in ["null", "0", "1", "\"true\"", "[]", "{}"] {
            assert!(from_line::<Command>(&format!(
                r#"{{"cmd":"set_loud_sound_reduction","on":{invalid}}}"#
            ))
            .is_none());
        }
        assert!(from_line::<Command>(r#"{"cmd":"set_loud_sound_reduction"}"#).is_none());
    }

    #[test]
    fn settings_maps_reject_noncanonical_keys_and_out_of_range_values() {
        for map in [
            r#"{"256":1}"#,
            r#"{"-1":1}"#,
            r#"{"031":1}"#,
            r#"{"eq":1}"#,
            r#"{"1":-1}"#,
            r#"{"1":4294967296}"#,
            "null",
            "[]",
        ] {
            let mut state = serde_json::to_value(Event::State(Snapshot::default())).unwrap();
            state["device_controls"] = serde_json::from_str(map).unwrap();
            assert!(from_line::<Event>(&to_line(&state)).is_none(), "{map}");
        }
    }

    #[test]
    fn anc_uncertainty_rejects_non_boolean_values() {
        for value in ["null", "0", "1", "\"true\"", "[]"] {
            let mut state = serde_json::to_value(Event::State(Snapshot::default())).unwrap();
            state["anc_unconfirmed"] = serde_json::from_str(value).unwrap();
            assert!(from_line::<Event>(&to_line(&state)).is_none(), "{value}");
        }
    }

    #[test]
    fn requested_strength_accepts_only_null_or_a_percentage() {
        for value in ["null", "0", "25", "50", "100"] {
            let mut state = serde_json::to_value(Event::State(Snapshot::default())).unwrap();
            state["adaptive_strength_requested"] = serde_json::from_str(value).unwrap();
            assert!(from_line::<Event>(&to_line(&state)).is_some(), "{value}");
        }
        for value in [
            "-1",
            "101",
            "4294967296",
            "1.5",
            "true",
            "\"50\"",
            "[]",
            "{}",
        ] {
            let mut state = serde_json::to_value(Event::State(Snapshot::default())).unwrap();
            state["adaptive_strength_requested"] = serde_json::from_str(value).unwrap();
            assert!(from_line::<Event>(&to_line(&state)).is_none(), "{value}");
        }
    }

    #[test]
    fn requested_allow_off_accepts_only_null_or_a_boolean() {
        for value in ["null", "true", "false"] {
            let mut state = serde_json::to_value(Event::State(Snapshot::default())).unwrap();
            state["allow_off_requested"] = serde_json::from_str(value).unwrap();
            assert!(from_line::<Event>(&to_line(&state)).is_some(), "{value}");
        }
        for value in ["0", "1", "1.5", "\"true\"", "[]", "{}"] {
            let mut state = serde_json::to_value(Event::State(Snapshot::default())).unwrap();
            state["allow_off_requested"] = serde_json::from_str(value).unwrap();
            assert!(from_line::<Event>(&to_line(&state)).is_none(), "{value}");
        }
    }
}
