//! NDJSON protocol between `ntpodsd` and the WinUI app over Windows named pipes.
//! See `../../docs/daemon-ipc/PLAN.md`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Separate event and command pipes avoid synchronous-handle serialization:
/// a blocking ReadFile would otherwise stall WriteFile on the same handle.
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackMode {
    #[default]
    Windows,
    Aac,
    AacEld,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaybackBitrate {
    pub best_effort: bool,
    pub kbps: u32,
    #[serde(default)]
    pub variable: bool,
}
impl Default for PlaybackBitrate {
    fn default() -> Self {
        Self {
            best_effort: true,
            kbps: 256,
            variable: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareDiagnosticEar {
    pub measured_at_unix_seconds: u64,
    pub general: u8,
    pub measurement: u8,
    /// Present only for a supported model with a dated, completed check.
    pub component_failures: Option<u8>,
    pub unknown_bits: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareDiagnostics {
    pub read_at_unix_seconds: u64,
    pub left: HardwareDiagnosticEar,
    pub right: HardwareDiagnosticEar,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TransparencySettings {
    pub enabled: bool,
    pub left_amplification: f32,
    pub right_amplification: f32,
    pub left_tone: f32,
    pub right_tone: f32,
    pub left_conversation_boost: bool,
    pub right_conversation_boost: bool,
    pub left_ambient_noise_reduction: f32,
    pub right_ambient_noise_reduction: f32,
}

impl TransparencySettings {
    pub fn is_valid(&self) -> bool {
        [
            self.left_amplification,
            self.right_amplification,
            self.left_tone,
            self.right_tone,
        ]
        .iter()
        .all(|v| v.is_finite() && (-1.0..=1.0).contains(v))
            && [
                self.left_ambient_noise_reduction,
                self.right_ambient_noise_reduction,
            ]
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
    }
}

/// Complete confirmed hearing settings; absent data must never become a zero audiogram.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct HearingAidSettings {
    pub left_eq: [f32; 8],
    pub right_eq: [f32; 8],
    pub left_amplification: f32,
    pub right_amplification: f32,
    pub left_tone: f32,
    pub right_tone: f32,
    pub left_conversation_boost: bool,
    pub right_conversation_boost: bool,
    pub left_ambient_noise_reduction: f32,
    pub right_ambient_noise_reduction: f32,
    pub own_voice: f32,
}

impl HearingAidSettings {
    pub fn is_valid(&self) -> bool {
        self.left_eq
            .iter()
            .chain(&self.right_eq)
            .all(|v| v.is_finite() && (0.0..=90.0).contains(v))
            && [self.left_amplification, self.right_amplification]
                .iter()
                .all(|v| v.is_finite() && (-1.5..=1.5).contains(v))
            && [self.left_tone, self.right_tone]
                .iter()
                .all(|v| v.is_finite() && (-1.0..=1.0).contains(v))
            && [
                self.left_ambient_noise_reduction,
                self.right_ambient_noise_reduction,
                self.own_voice,
            ]
            .iter()
            .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
    }
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

/// Battery percentages; a packet may report only some components.
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

/// Temporary signal view for the selected, IRK-verified pair. RSSI is not distance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NearbyFinderState {
    #[serde(default)]
    pub revision: u64,
    pub active: bool,
    pub paused: bool,
    pub available_identity: bool,
    pub rssi: Option<i16>,
    pub trend: String,
    pub last_heard_age_ms: Option<u64>,
}

impl Default for NearbyFinderState {
    fn default() -> Self {
        Self {
            revision: 0,
            active: false,
            paused: false,
            available_identity: false,
            rssi: None,
            trend: "unknown".into(),
            last_heard_age_ms: None,
        }
    }
}

/// The daemon's authoritative state, pushed to clients on connect and on change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub connected: bool,
    /// Idle AAP transport has failed while Windows still reports a connection.
    /// Silence alone does not mark the link stale.
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
    /// The NTPodsMic control device opens and supports flushing queued PCM.
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
    /// Allow the Off listening mode on models that expose this setting.
    pub allow_off: bool,
    /// Allow Off sent successfully to Pro 3, but not yet reported by the AirPods.
    #[serde(default)]
    pub allow_off_requested: Option<bool>,
    /// Validated four-byte AACP settings, confirmed by the current device session.
    #[serde(default, deserialize_with = "deserialize_device_controls")]
    pub device_controls: BTreeMap<u8, u32>,
    /// Last successfully sent choices without a current device report.
    #[serde(default, deserialize_with = "deserialize_device_controls")]
    pub device_controls_requested: BTreeMap<u8, u32>,
    #[serde(default, deserialize_with = "deserialize_device_controls")]
    pub device_controls_pending: BTreeMap<u8, u32>,
    /// Raw capability IDs and values reported by the current AACP connection.
    #[serde(default, deserialize_with = "deserialize_device_controls")]
    pub aacp_capabilities: BTreeMap<u8, u32>,
    /// Strength sent successfully, but not yet reported by the AirPods; never confirmed state.
    #[serde(default, deserialize_with = "deserialize_adaptive_strength_requested")]
    pub adaptive_strength_requested: Option<u32>,
    #[serde(default)]
    pub device_settings_error: Option<String>,
    #[serde(default)]
    pub eq: Option<Equalizer>,
    #[serde(default)]
    pub eq_pending: Option<Equalizer>,
    /// Confirmed AAP 0x2C state; unknown until the current connection reports it.
    #[serde(default)]
    pub hearing_aid_enabled: Option<bool>,
    #[serde(default)]
    pub hearing_aid_revision: u64,
    #[serde(default)]
    pub hearing_aid_enrolled: Option<bool>,
    #[serde(default)]
    pub media_assist_enabled: Option<bool>,
    #[serde(default)]
    pub media_assist_revision: u64,
    #[serde(default)]
    pub media_assist_available: bool,
    #[serde(default)]
    pub hearing_aid_settings: Option<HearingAidSettings>,
    #[serde(default)]
    pub hearing_aid_editable: bool,
    #[serde(default)]
    pub hearing_aid_pending: bool,
    #[serde(default)]
    pub hearing_aid_error: Option<String>,
    #[serde(default)]
    pub hardware_diagnostics: Option<HardwareDiagnostics>,
    #[serde(default)]
    pub hardware_diagnostics_pending: bool,
    #[serde(default)]
    pub hardware_diagnostics_error: Option<String>,
    /// ATT handle 0x1B, independently confirmed; absent until a successful read.
    #[serde(default)]
    pub loud_sound_reduction: Option<bool>,
    #[serde(default)]
    pub loud_sound_reduction_pending: bool,
    #[serde(default)]
    pub loud_sound_reduction_error: Option<String>,
    #[serde(default)]
    pub transparency: Option<TransparencySettings>,
    #[serde(default)]
    pub transparency_pending: bool,
    #[serde(default)]
    pub transparency_error: Option<String>,
    /// Pause on ear removal and resume only playback paused by this app.
    #[serde(default = "default_auto_pause")]
    pub auto_pause: bool,
    /// Volume 0..=100 for native AirPods playback, otherwise the default output.
    /// The daemon owns volume changes and Conversation Awareness ducking.
    pub volume: u8,
    /// The output is muted.
    pub muted: bool,
    #[serde(default)]
    pub playback_mode: PlaybackMode,
    /// Saved codec preference; playback_mode is the active or attempted codec.
    #[serde(default)]
    pub playback_preferred_mode: Option<PlaybackMode>,
    /// windows, disconnected, starting, ready, stopping, or error.
    #[serde(default = "default_playback_state")]
    pub playback_state: String,
    /// The selected AirPods have an active Windows render endpoint or ready native output.
    #[serde(default)]
    pub audio_output_ready: bool,
    #[serde(default)]
    pub playback_error: Option<String>,
    #[serde(default)]
    pub playback_available: bool,
    #[serde(default)]
    pub playback_native_driver: bool,
    #[serde(default)]
    pub playback_aac_bitrate: PlaybackBitrate,
    #[serde(default)]
    pub playback_aac_eld_bitrate: PlaybackBitrate,
    #[serde(default)]
    pub playback_bitrate_kbps: Option<u32>,
    #[serde(default)]
    pub playback_delay_reporting: bool,
    /// Earbud-reported delay in 0.1 ms units, not measured end-to-end latency.
    #[serde(default)]
    pub playback_delay_tenths_ms: Option<u16>,
    /// Requested trial target, retained on restore failure; not device readback.
    #[serde(default)]
    pub playback_buffer_target_ms: Option<u16>,
    #[serde(default)]
    pub playback_buffer_error: Option<String>,
    /// Latest validated BPM; absent while disabled, before a sample or after expiry.
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
    /// Explicit nearby session. No position or distance is inferred.
    #[serde(default)]
    pub nearby_finder: NearbyFinderState,
    /// Metadata from packet 0x1D, retained across reconnects to the same pair.
    /// The serial is sensitive and remains hidden until revealed in the app.
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub firmware: String,
    #[serde(default)]
    pub serial: String,
    /// Hosts reported by AAP 0x2E, including connections Windows cannot observe.
    /// Empty until the first report, or when reading an older snapshot.
    #[serde(default)]
    pub multipoint: Vec<ConnectedDevice>,
}

fn default_auto_pause() -> bool {
    true
}

fn default_head_tracking_state() -> String {
    "idle".into()
}

fn default_playback_state() -> String {
    "windows".into()
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
            device_controls_requested: BTreeMap::new(),
            device_controls_pending: BTreeMap::new(),
            aacp_capabilities: BTreeMap::new(),
            adaptive_strength_requested: None,
            device_settings_error: None,
            eq: None,
            eq_pending: None,
            hearing_aid_enabled: None,
            hearing_aid_revision: 0,
            hearing_aid_enrolled: None,
            media_assist_enabled: None,
            media_assist_revision: 0,
            media_assist_available: false,
            hearing_aid_settings: None,
            hearing_aid_editable: false,
            hearing_aid_pending: false,
            hearing_aid_error: None,
            hardware_diagnostics: None,
            hardware_diagnostics_pending: false,
            hardware_diagnostics_error: None,
            loud_sound_reduction: None,
            loud_sound_reduction_pending: false,
            loud_sound_reduction_error: None,
            transparency: None,
            transparency_pending: false,
            transparency_error: None,
            auto_pause: true,
            volume: 0,
            muted: false,
            playback_mode: PlaybackMode::Windows,
            playback_preferred_mode: None,
            playback_state: default_playback_state(),
            audio_output_ready: false,
            playback_error: None,
            playback_available: false,
            playback_native_driver: false,
            playback_aac_bitrate: PlaybackBitrate::default(),
            playback_aac_eld_bitrate: PlaybackBitrate::default(),
            playback_bitrate_kbps: None,
            playback_delay_reporting: false,
            playback_delay_tenths_ms: None,
            playback_buffer_target_ms: None,
            playback_buffer_error: None,
            heart_rate: None,
            heart_rate_enabled: false,
            heart_rate_timestamp_ms: None,
            head_tracking_enabled: false,
            head_tracking_state: default_head_tracking_state(),
            head_tracking_error: None,
            nearby_finder: NearbyFinderState::default(),
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
    /// "This PC", a Windows pairing name, or "Other device" for an unpaired host.
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

/// Client commands, including volume changes, are handled by the daemon.
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
    /// Toggle the stream and enter Manual mode in one operation.
    ToggleMicManual,
    SetPlaybackMode {
        mode: PlaybackMode,
    },
    RetryPlayback,
    /// Temporary buffer trial with an explicit restoration baseline; null ends it.
    SetPlaybackBuffer {
        target_ms: Option<u16>,
        #[serde(default)]
        restore_ms: Option<u16>,
    },
    SetPlaybackBitrate {
        mode: PlaybackMode,
        best_effort: bool,
        kbps: u32,
        #[serde(default)]
        variable: bool,
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
    /// Opt-in heart-rate monitoring; disabling stops reporting and clears the reading.
    SetHeartRate {
        on: bool,
    },
    /// Start/stop live movement for this command client; closing its pipe stops it.
    SetHeadTracking {
        on: bool,
    },
    /// The command pipe owns this temporary scan until Stop or disconnection.
    StartNearbyFinder {
        device_id: String,
    },
    StopNearbyFinder {
        device_id: String,
    },
    GetLoudSoundReduction,
    GetTransparency,
    SetTransparency {
        #[serde(default)]
        expected: Option<TransparencySettings>,
        settings: TransparencySettings,
    },
    SetLoudSoundReduction {
        on: bool,
    },
    /// Read both existing audiograms and per-ear controls without enabling or writing.
    GetHearingAid,
    /// Read the last saved hardware checks; never starts a self-test.
    GetHardwareDiagnostics,
    /// Toggle the stored fitting without editing its audiogram or adjustments.
    SetHearingAidMode {
        request_id: u64,
        on: bool,
    },
    /// Toggle the stored voice/media fitting while preserving its curves.
    SetMediaAssist {
        request_id: u64,
        on: bool,
    },
    /// Enabling or editing requires complete settings and a read in this connection.
    /// Disabling remains available without settings, including after a read failure.
    SetHearingAid {
        #[serde(default)]
        request_id: u64,
        on: bool,
        #[serde(default)]
        expected: Option<HearingAidSettings>,
        #[serde(default)]
        settings: Option<HearingAidSettings>,
    },
    /// Request AAP and Windows audio connections. An optional device ID binds
    /// an island action to the pair shown when the user pressed Connect.
    Connect {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device_id: Option<String>,
    },
    /// Select the exact Classic device after Windows pairing succeeds.
    PairingCompleted {
        address: String,
    },
    /// Rebuild playback without requesting AAP repair.
    RebuildAudio,
    /// Reopen AAP and ATT after microphone cleanup and any pending audio recovery.
    RepairConnection,
    /// Refresh reported settings through a fresh AAP session, only while capture is idle.
    RefreshDeviceSettings,
    /// Disconnect AAP and audio after microphone cleanup; block automatic reconnect.
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
    /// Report a nearby device that the client may offer to connect.
    ConnectPrompt { name: String },
    /// Public lid report; the model is an invitation hint, not a paired identity.
    PairingNearby { model: u16, lid_open: bool },
    /// Authenticated selected-pair lid edge, ordered within this daemon session.
    PairedLid {
        device_id: String,
        lid_open: bool,
        sequence: u64,
    },
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
    fn pairing_invitation_contains_only_the_public_model_and_lid_hint() {
        for lid_open in [false, true] {
            let line = to_line(&Event::PairingNearby {
                model: 0x2720,
                lid_open,
            });
            assert_eq!(
                line,
                format!(
                    "{{\"event\":\"pairing_nearby\",\"model\":10016,\"lid_open\":{lid_open}}}\n"
                )
            );
            assert!(matches!(
                from_line::<Event>(&line),
                Some(Event::PairingNearby { model: 0x2720, lid_open: actual }) if actual == lid_open
            ));
        }
        for malformed in [
            r#"{"event":"pairing_nearby","model":65536,"lid_open":true}"#,
            r#"{"event":"pairing_nearby","model":-1,"lid_open":true}"#,
            r#"{"event":"pairing_nearby","model":10016}"#,
            r#"{"event":"pairing_nearby","model":10016,"lid_open":1}"#,
        ] {
            assert!(from_line::<Event>(malformed).is_none());
        }
    }

    #[test]
    fn paired_lid_edges_carry_exact_owner_and_session_order() {
        for (lid_open, sequence) in [(true, 1), (false, 2)] {
            let line = to_line(&Event::PairedLid {
                device_id: "001122334455".into(),
                lid_open,
                sequence,
            });
            assert_eq!(
                line,
                format!(
                    "{{\"event\":\"paired_lid\",\"device_id\":\"001122334455\",\"lid_open\":{lid_open},\"sequence\":{sequence}}}\n"
                )
            );
            assert!(matches!(
                from_line::<Event>(&line),
                Some(Event::PairedLid { device_id, lid_open: actual, sequence: actual_sequence })
                    if device_id == "001122334455" && actual == lid_open && actual_sequence == sequence
            ));
        }
        for malformed in [
            r#"{"event":"paired_lid","device_id":"001122334455","lid_open":true}"#,
            r#"{"event":"paired_lid","device_id":"001122334455","sequence":1}"#,
            r#"{"event":"paired_lid","lid_open":true,"sequence":1}"#,
            r#"{"event":"paired_lid","device_id":"001122334455","lid_open":1,"sequence":1}"#,
            r#"{"event":"paired_lid","device_id":"001122334455","lid_open":true,"sequence":-1}"#,
        ] {
            assert!(from_line::<Event>(malformed).is_none());
        }
    }

    #[test]
    fn connect_accepts_legacy_request_and_optional_pair_identity() {
        let legacy = r#"{"cmd":"connect"}"#;
        assert!(matches!(
            from_line::<Command>(legacy),
            Some(Command::Connect { device_id: None })
        ));
        assert_eq!(
            to_line(&Command::Connect { device_id: None }),
            format!("{legacy}\n")
        );

        let bound = Command::Connect {
            device_id: Some("001122334455".into()),
        };
        let line = to_line(&bound);
        assert_eq!(
            line,
            "{\"cmd\":\"connect\",\"device_id\":\"001122334455\"}\n"
        );
        assert!(matches!(
            from_line::<Command>(&line),
            Some(Command::Connect { device_id: Some(id) }) if id == "001122334455"
        ));
        for malformed in [
            r#"{"cmd":"connect","device_id":42}"#,
            r#"{"cmd":"connect","device_id":true}"#,
            r#"{"cmd":"connect","device_id":[]}"#,
        ] {
            assert!(from_line::<Command>(malformed).is_none());
        }
    }

    #[test]
    fn finder_requires_pair_identity_and_defaults_off_for_legacy_snapshots() {
        for cmd in [
            Command::StartNearbyFinder {
                device_id: "001122334455".into(),
            },
            Command::StopNearbyFinder {
                device_id: "001122334455".into(),
            },
        ] {
            let line = to_line(&cmd);
            assert!(line.contains("\"device_id\":\"001122334455\""));
            assert!(from_line::<Command>(&line).is_some());
        }
        for malformed in [
            r#"{"cmd":"start_nearby_finder"}"#,
            r#"{"cmd":"stop_nearby_finder","device_id":null}"#,
            r#"{"cmd":"start_nearby_finder","device_id":42}"#,
        ] {
            assert!(from_line::<Command>(malformed).is_none());
        }
        let mut old = serde_json::to_value(Snapshot::default()).unwrap();
        old.as_object_mut().unwrap().remove("nearby_finder");
        let snapshot: Snapshot = serde_json::from_value(old).unwrap();
        assert_eq!(snapshot.nearby_finder, NearbyFinderState::default());
        let finder = NearbyFinderState {
            revision: u64::MAX - 1,
            active: true,
            paused: false,
            available_identity: true,
            rssi: Some(-62),
            trend: "stronger".into(),
            last_heard_age_ms: Some(1_234),
        };
        assert_eq!(
            serde_json::from_str::<NearbyFinderState>(&serde_json::to_string(&finder).unwrap())
                .unwrap(),
            finder
        );
        let mut previous = serde_json::to_value(&finder).unwrap();
        previous.as_object_mut().unwrap().remove("revision");
        let previous: NearbyFinderState = serde_json::from_value(previous).unwrap();
        assert_eq!(previous.revision, 0);
    }

    #[test]
    fn hearing_settings_require_complete_finite_curves_and_never_infer_defaults() {
        let settings = HearingAidSettings {
            left_eq: [5.; 8],
            right_eq: [25.; 8],
            left_amplification: -0.25,
            right_amplification: 0.5,
            left_tone: -0.5,
            right_tone: 0.25,
            left_conversation_boost: true,
            right_conversation_boost: false,
            left_ambient_noise_reduction: 0.25,
            right_ambient_noise_reduction: 0.75,
            own_voice: 0.5,
        };
        assert!(settings.is_valid());
        for amplification in [-1.5, -1.25, 1.25, 1.5] {
            assert!(HearingAidSettings {
                left_amplification: amplification,
                right_amplification: -amplification,
                ..settings
            }
            .is_valid());
        }
        for amplification in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.5001, 1.5001] {
            assert!(!HearingAidSettings {
                left_amplification: amplification,
                ..settings
            }
            .is_valid());
            assert!(!HearingAidSettings {
                right_amplification: amplification,
                ..settings
            }
            .is_valid());
        }
        for tone in [-1.0001, 1.0001] {
            assert!(!HearingAidSettings {
                left_tone: tone,
                ..settings
            }
            .is_valid());
            assert!(!HearingAidSettings {
                right_tone: tone,
                ..settings
            }
            .is_valid());
        }
        let encoded = serde_json::to_value(settings).unwrap();
        for key in encoded.as_object().unwrap().keys() {
            let mut missing = encoded.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert!(serde_json::from_value::<HearingAidSettings>(missing).is_err());
        }
        for size in [0, 7, 9] {
            let mut partial = encoded.clone();
            partial["left_eq"] = serde_json::json!(vec![5.; size]);
            assert!(serde_json::from_value::<HearingAidSettings>(partial).is_err());
        }
        for value in [f32::NAN, f32::INFINITY, -1., 91.] {
            let mut invalid = settings;
            invalid.right_eq[0] = value;
            assert!(!invalid.is_valid());
        }
        let legacy = r#"{"cmd":"set_hearing_aid","on":true,"left_eq":[0,0,0,0,0,0,0,0],"right_eq":[0,0,0,0,0,0,0,0],"amplification":0.5}"#;
        assert!(matches!(
            from_line::<Command>(legacy),
            Some(Command::SetHearingAid {
                on: true,
                settings: None,
                ..
            })
        ));
        assert!(matches!(
            from_line::<Command>(r#"{"cmd":"set_hearing_aid","on":false}"#),
            Some(Command::SetHearingAid {
                on: false,
                settings: None,
                ..
            })
        ));
    }

    #[test]
    fn playback_selection_is_bounded_and_legacy_state_uses_windows() {
        for mode in [
            PlaybackMode::Windows,
            PlaybackMode::Aac,
            PlaybackMode::AacEld,
        ] {
            let command = Command::SetPlaybackMode { mode };
            assert!(matches!(from_line::<Command>(&to_line(&command)),
                Some(Command::SetPlaybackMode { mode: actual }) if actual == mode));
            let state = Snapshot {
                playback_mode: mode,
                playback_preferred_mode: Some(PlaybackMode::AacEld),
                playback_state: "error".into(),
                playback_error: Some("Playback failed".into()),
                playback_available: true,
                ..Default::default()
            };
            let Event::State(actual) = from_line(&to_line(&Event::State(state.clone()))).unwrap()
            else {
                panic!("state")
            };
            assert_eq!(actual, state);
        }
        assert!(matches!(
            from_line::<Command>(&to_line(&Command::RetryPlayback)),
            Some(Command::RetryPlayback)
        ));
        for mode in ["null", "true", "0", "\"eld\"", "\"lossless\"", "\"AAC\""] {
            assert!(from_line::<Command>(&format!(
                r#"{{"cmd":"set_playback_mode","mode":{mode}}}"#
            ))
            .is_none());
        }
        let mut old = serde_json::to_value(Snapshot::default()).unwrap();
        for key in [
            "playback_mode",
            "playback_preferred_mode",
            "playback_state",
            "playback_error",
            "playback_available",
            "playback_native_driver",
        ] {
            old.as_object_mut().unwrap().remove(key);
        }
        let state: Snapshot = serde_json::from_value(old).unwrap();
        assert_eq!(state.playback_mode, PlaybackMode::Windows);
        assert_eq!(state.playback_preferred_mode, None);
        assert_eq!(state.playback_state, "windows");
        assert!(!state.playback_available && state.playback_error.is_none());
        assert!(!state.playback_native_driver);
    }

    #[test]
    fn playback_delay_and_buffer_contract_preserves_unknown_and_zero() {
        let mut old = serde_json::to_value(Snapshot::default()).unwrap();
        for key in [
            "playback_delay_reporting",
            "playback_delay_tenths_ms",
            "playback_buffer_target_ms",
            "playback_buffer_error",
        ] {
            old.as_object_mut().unwrap().remove(key);
        }
        let legacy: Snapshot = serde_json::from_value(old).unwrap();
        assert!(!legacy.playback_delay_reporting);
        assert!(legacy.playback_delay_tenths_ms.is_none());
        assert!(legacy.playback_buffer_target_ms.is_none());
        assert!(legacy.playback_buffer_error.is_none());

        for delay in [None, Some(0), Some(1500), Some(u16::MAX)] {
            let state = Snapshot {
                playback_delay_reporting: true,
                playback_delay_tenths_ms: delay,
                playback_buffer_target_ms: Some(60),
                playback_buffer_error: Some("Restore failed.".into()),
                ..Default::default()
            };
            let actual: Snapshot = from_line(&to_line(&state)).unwrap();
            assert_eq!(actual, state);
        }
        for target_ms in [None, Some(60), Some(80), Some(100), Some(120), Some(140)] {
            let restore_ms = target_ms.map(|_| 150);
            let command = Command::SetPlaybackBuffer {
                target_ms,
                restore_ms,
            };
            assert!(matches!(from_line::<Command>(&to_line(&command)),
                Some(Command::SetPlaybackBuffer { target_ms: actual, restore_ms: baseline })
                if actual == target_ms && baseline == restore_ms));
        }
        assert!(matches!(
            from_line::<Command>(r#"{"cmd":"set_playback_buffer","target_ms":null}"#),
            Some(Command::SetPlaybackBuffer {
                target_ms: None,
                restore_ms: None
            })
        ));
        for invalid in ["-1", "65536", "1.5", "true", "\"60\""] {
            for key in ["target_ms", "restore_ms"] {
                assert!(from_line::<Command>(&format!(
                    r#"{{"cmd":"set_playback_buffer","{key}":{invalid}}}"#
                ))
                .is_none());
            }
            for key in ["playback_delay_tenths_ms", "playback_buffer_target_ms"] {
                let mut state = serde_json::to_value(Snapshot::default()).unwrap();
                state[key] = serde_json::from_str(invalid).unwrap();
                assert!(serde_json::from_value::<Snapshot>(state).is_err());
            }
        }
    }

    #[test]
    fn preferred_playback_mode_accepts_null_but_rejects_invalid_codecs() {
        let mut value = serde_json::to_value(Snapshot::default()).unwrap();
        assert!(serde_json::from_value::<Snapshot>(value.clone())
            .unwrap()
            .playback_preferred_mode
            .is_none());
        for invalid in ["true", "0", "\"eld\"", "\"lossless\"", "\"AAC\""] {
            value["playback_preferred_mode"] = serde_json::from_str(invalid).unwrap();
            assert!(serde_json::from_value::<Snapshot>(value.clone()).is_err());
        }
    }

    #[test]
    fn component_presence_preserves_unknown_and_explicit_absence() {
        let legacy: BatteryObservation =
            serde_json::from_str(r#"{"timestamp_ms":1000,"live":false}"#).unwrap();
        assert_eq!(legacy.present, None);
        for present in [None, Some(false), Some(true)] {
            let observation = BatteryObservation { present, ..legacy };
            let text = serde_json::to_string(&observation).unwrap();
            assert_eq!(
                serde_json::from_str::<BatteryObservation>(&text).unwrap(),
                observation
            );
        }
        for invalid in ["0", "1", "\"false\"", "{}"] {
            assert!(serde_json::from_str::<BatteryObservation>(&format!(
                r#"{{"timestamp_ms":1000,"live":false,"present":{invalid}}}"#
            ))
            .is_err());
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
        assert_eq!(
            to_line(&Command::ToggleMicManual),
            "{\"cmd\":\"toggle_mic_manual\"}\n"
        );
        assert!(matches!(
            from_line::<Command>(&to_line(&Command::ToggleMicManual)),
            Some(Command::ToggleMicManual)
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
            device_controls_requested: [(0x23, 2)].into(),
            aacp_capabilities: [(0x11, 1), (0xC0, 2), (0x30, u32::MAX)].into(),
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
            "device_controls_requested",
            "device_controls_pending",
            "aacp_capabilities",
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
        assert!(actual.device_controls_requested.is_empty());
        assert!(actual.aacp_capabilities.is_empty());
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
            for field in [
                "device_controls",
                "device_controls_requested",
                "aacp_capabilities",
            ] {
                let mut state = serde_json::to_value(Event::State(Snapshot::default())).unwrap();
                state[field] = serde_json::from_str(map).unwrap();
                assert!(
                    from_line::<Event>(&to_line(&state)).is_none(),
                    "{field}: {map}"
                );
            }
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
