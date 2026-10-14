//! NTPods background service: AAP connection, device state and microphone pipeline.
//! Serves the WinUI app over NDJSON pipes; see ../../docs/daemon-ipc/PLAN.md.
//! Runs without a console window.

#![windows_subsystem = "windows"]
#![allow(dead_code)]

mod a2dp;
mod aap;
mod att_settings;
mod battery_ble;
mod bt;
mod connection_request;
mod device_models;
mod device_settings;
mod devnode;
mod driver;
mod eld;
mod gatt;
mod handsfree;
mod head_tracking;
mod hearing;
mod helper_launch;
mod hfp_endpoints;
mod hr;
mod hrdb;
mod ipc_server;
mod le;
mod media;
mod mic_lifecycle;
mod micpipe;
#[cfg(feature = "experimental-playback")]
mod playback;
mod rename;
mod rtbuddy;
mod security;
mod transparency;
mod volume;

use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use std::{io, ptr};

use connection_request::ConnectionRequest;
use mic_lifecycle::playback_allows_microphone;
use ntpods_ipc as ipc;
use ntpods_ipc::{to_line, Command, Event, Snapshot};

use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows_sys::Win32::System::Threading::CreateMutexW;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn cooldown_elapsed(last: Option<Instant>, now: Instant, cooldown: Duration) -> bool {
    last.is_none_or(|last| now.duration_since(last) >= cooldown)
}

/// ISO 8601 UTC with milliseconds, for matching timestamps across logs.
fn utc_timestamp(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    let (days, tod) = ((secs / 86_400) as i64, secs % 86_400);
    // Days since 1970-01-01 to a civil date (Howard Hinnant's civil_from_days).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60,
        d.subsec_millis()
    )
}

fn log(s: &str) {
    use std::io::Write;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(utc_timestamp)
        .unwrap_or_default();
    if let Ok(la) = std::env::var("LOCALAPPDATA") {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(format!("{la}\\NTPods\\daemon.log"))
        {
            let _ = writeln!(f, "{ts} {s}");
        }
    }
}

fn battery_text(b: &ntpods_ipc::Battery, connected: bool) -> String {
    if !connected {
        return "Disconnected".to_string();
    }
    let f = |v: Option<u8>| v.map(|p| format!("{p}%")).unwrap_or_else(|| "—".into());
    format!(
        "Left {}   Right {}   Case {}",
        f(b.left),
        f(b.right),
        f(b.case)
    )
}

/// Each client has its own writer thread so a slow client cannot block the session.
type ClientTx = std::sync::mpsc::SyncSender<Vec<u8>>;

/// Everything the session, poll and IPC threads share.
#[derive(Clone)]
struct Ctx {
    state: Arc<Mutex<Snapshot>>,
    battery_keys: Arc<Mutex<Option<battery_ble::Keys>>>,
    battery_cache: Arc<Mutex<battery_ble::Cache>>,
    settings_pending: Arc<Mutex<device_settings::PendingSettings>>,
    settings_write: Arc<Mutex<()>>,
    mic_preference: Arc<Mutex<()>>,
    #[cfg(feature = "experimental-playback")]
    playback_selection: Arc<Mutex<playback::Selection>>,
    #[cfg(feature = "experimental-playback")]
    playback_buffer: Arc<Mutex<Option<Arc<playback::BufferActive>>>>,
    #[cfg(feature = "experimental-playback")]
    audio_change_busy: Arc<AtomicBool>,
    mic_demand: Arc<Mutex<mic_lifecycle::Demand>>,
    mic_clock: Instant,
    hr_commands: Arc<Mutex<()>>,
    hr_generation: Arc<AtomicU64>,
    head_tracking: Arc<Mutex<head_tracking::Tracker>>,
    auto_pause: Arc<Mutex<media::AutoPausePolicy>>,
    hr_db: hrdb::HrDb,
    clients: Arc<Mutex<Vec<ClientTx>>>,
    driver_cell: Arc<Mutex<Option<driver::Driver>>>,
    shared_driver: Arc<driver::Shared>,
    mic_on: Arc<AtomicBool>,
    capture_active: Arc<AtomicBool>,
    shutting_down: Arc<AtomicBool>,
    receiver_done: Arc<AtomicBool>,
    playback_done: Arc<AtomicBool>,
    handsfree_done: Arc<AtomicBool>,
    shutdown_cleanup_failed: Arc<AtomicBool>,
    auto_mode: Arc<AtomicBool>,
    /// Opt-in gate for heart-rate decoding in `run_receiver`.
    hr_on: Arc<AtomicBool>,
    /// First decoded BPM sample, set by `run_receiver` and polled by the retry thread.
    /// Cleared before each enable attempt.
    hr_got_sample: Arc<AtomicBool>,
    /// An HR frame arrived, possibly without a reading. Keep a live stream open
    /// while the sensor warms up. Cleared before each enable attempt.
    hr_stream_live: Arc<AtomicBool>,
    /// Prevents overlapping heart-rate retries on the shared driver.
    hr_retrying: Arc<AtomicBool>,
    /// Connection intent and generation, shared with asynchronous workers.
    connect_requested: Arc<ConnectionRequest>,
    /// Repair request consumed by `run_receiver` to reopen AAP and ATT after cleanup.
    wants_reconnect: Arc<AtomicBool>,
    /// Consumed by the receiver; unlike repair, this must never stop a capture.
    wants_settings_refresh: Arc<AtomicBool>,
    pipe: Arc<micpipe::MicPipeCell>,
    /// Shared so disabling Conversation Awareness can restore volume without an end event.
    conv_duck: Arc<Mutex<volume::ConvDuck>>,
    /// Prevents overlapping audio-reclaim attempts.
    reclaiming: Arc<AtomicBool>,
    /// Explicit Disconnect blocks automatic resume. Connect or Repair clears it.
    user_disconnected: Arc<AtomicBool>,
    /// Reserved at Disconnect, applied only after the receiver closes its channel.
    pending_audio_disconnect: Arc<Mutex<PendingAudioDisconnect>>,
    /// Cooldown after a reclaim attempt, to avoid repeated requests during a phone call.
    last_reclaim: Arc<Mutex<Option<Instant>>>,
    /// Last repair request. Debounce prevents repeated handle reopens that can
    /// leave the devnode in Code 38.
    last_repair: Arc<Mutex<Option<Instant>>>,
    dev_name: Arc<Mutex<String>>,
    mac: Arc<AtomicU64>,
}

impl Ctx {
    fn mac(&self) -> u64 {
        self.mac.load(Ordering::Acquire)
    }
    fn battery_scan_allowed(&self) -> bool {
        if self.mac() == 0 || self.shutting_down.load(Ordering::Acquire) {
            return false;
        }
        let (opt_in, allowed) = {
            let state = self.state.lock().unwrap();
            (
                state.ble_scan_while_connected,
                battery_ble::scan_allowed(
                    state.ble_scan_while_connected,
                    state.connected,
                    state.mic_capture_active || self.capture_active.load(Ordering::Acquire),
                    state.mic_uplink_active || self.mic_on.load(Ordering::Acquire),
                ),
            )
        };
        allowed && (opt_in || !bt::is_connected(self.mac()))
    }
    /// Queue without waiting for writers; drop disconnected clients or full queues.
    fn send_event(&self, ev: &Event) {
        let bytes = to_line(ev).into_bytes();
        let mut clients = self.clients.lock().unwrap();
        clients.retain(|tx| tx.try_send(bytes.clone()).is_ok());
    }

    /// Broadcast the current state (with the live mic/auto flags folded in).
    fn push_state(&self) {
        let (mode, stop_delay_secs, manual_active) = {
            let demand = self.mic_demand.lock().unwrap();
            (demand.mode, demand.stop_delay_secs, demand.manual_active)
        };
        let snap = {
            let mut s = self.state.lock().unwrap();
            s.mic_recording = s.mic_state == "capturing"
                && self.mic_on.load(Ordering::Acquire)
                && self.capture_active.load(Ordering::Acquire);
            s.mic_mode = mode;
            s.mic_stop_delay_secs = stop_delay_secs;
            s.mic_manual_active = manual_active;
            s.mic_capture_active = self.capture_active.load(Ordering::Acquire);
            let (hearing_enabled, hearing_revision) = hearing::reported_state();
            s.hearing_aid_enabled = if s.connected && !s.link_stale {
                hearing_enabled
            } else {
                None
            };
            s.hearing_aid_revision = hearing_revision;
            if !s.connected || s.link_stale || !hearing::has_current_read() {
                s.hearing_aid_settings = None;
            }
            s.hearing_aid_editable =
                s.hearing_aid_settings.is_some() && hearing::can_edit_current_read();
            s.mic_available = self.pipe.is_open();
            if matches!(s.mic_state.as_str(), "idle" | "unavailable") {
                s.mic_state = if s.mic_available {
                    "idle"
                } else {
                    "unavailable"
                }
                .into();
            }
            s.auto_mode = self.auto_mode.load(Ordering::Relaxed);
            self.battery_cache
                .lock()
                .unwrap()
                .restore(&mut s, unix_millis());
            battery_ble::refresh_live(&mut s, unix_millis());
            s.dev_name = self.dev_name.lock().unwrap().clone();
            s.clone()
        };
        self.send_event(&Event::State(snap));
    }

    /// Broadcast a notification for clients to render.
    fn overlay(&self, body: &str) {
        self.send_event(&Event::Overlay {
            title: self.dev_name.lock().unwrap().clone(),
            body: body.to_string(),
        });
    }

    /// Publish volume/mute changes. The calling thread must have COM initialized.
    fn sync_volume(&self) {
        let vol = volume::get().unwrap_or(0);
        let muted = volume::is_muted();
        let changed = {
            let mut s = self.state.lock().unwrap();
            let c = s.volume != vol || s.muted != muted;
            s.volume = vol;
            s.muted = muted;
            c
        };
        if changed {
            self.push_state();
        }
    }
}

/// Local audio priority for reclaiming a lost connection: capture, playback, then idle.
/// The peer's priority is unknown, so retries are bounded to avoid repeated requests.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tier {
    /// An app is capturing through NTPods.
    Call,
    /// SMTC reports playback or a non-system Core Audio session is active.
    Playback,
    /// No active capture or playback; system sounds alone do not qualify.
    Background,
}

impl Tier {
    fn defends_audio(self) -> bool {
        self != Tier::Background
    }
    fn label(self) -> &'static str {
        match self {
            Tier::Call => "1/call",
            Tier::Playback => "2/playback",
            Tier::Background => "3/background",
        }
    }
}

fn recently_cased(report: Option<(u64, aap::Battery)>, now_ms: u64) -> bool {
    report.is_some_and(|(at, battery)| {
        now_ms >= at
            && now_ms - at <= 45_000
            && battery.left.is_some()
            && battery.right.is_some()
            && battery.left_charging
            && battery.right_charging
    })
}

/// Identify an AAP 0x2E host from local radios, then Windows pairing metadata.
/// Unpaired hosts remain "Other device" with their address; flags do not identify a model.
fn describe_device(addr: u64, flags: [u8; 2], local_radios: &[u64]) -> ipc::ConnectedDevice {
    let b = addr.to_be_bytes();
    let address = format!(
        "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        b[2], b[3], b[4], b[5], b[6], b[7]
    );
    if local_radios.contains(&addr) {
        return ipc::ConnectedDevice {
            address,
            is_this_pc: true,
            name: "This PC".into(),
            kind: "pc".into(),
        };
    }
    if let Some((name, cod)) = bt::paired_device(addr) {
        if !name.is_empty() {
            return ipc::ConnectedDevice {
                address,
                is_this_pc: false,
                kind: bt::kind_from_cod(cod).into(),
                name,
            };
        }
    }
    // The 0x2E flags do not identify a device class; see aap::flags_hex.
    let _ = flags;
    ipc::ConnectedDevice {
        address,
        is_this_pc: false,
        name: "Other device".into(),
        kind: "unknown".into(),
    }
}

/// What this host currently holds the audio route for. `media::is_playing()`
/// needs COM on the calling thread (run_receiver already did `media::init()`).
fn host_tier(ctx: &Ctx) -> Tier {
    if ctx.mic_on.load(Ordering::Acquire) && ctx.capture_active.load(Ordering::Acquire) {
        Tier::Call
    } else if media::is_playing() || volume::any_render_active() {
        // SMTC survives brief endpoint switches; Core Audio also covers calls and games.
        Tier::Playback
    } else {
        Tier::Background
    }
}

/// Ask the OS to connect/disconnect the audio, off-thread (the call blocks for
/// seconds) and serialized, so a stale request can never undo a newer one.
fn audio_request(mac: u64, connect: bool, why: &'static str) {
    if let Some(intent) = bt::begin_audio_request(connect) {
        dispatch_audio_request(mac, intent, why);
    }
}

fn dispatch_audio_request(mac: u64, intent: u64, why: &'static str) {
    let verb = if intent & 1 != 0 {
        "connect"
    } else {
        "disconnect"
    };
    thread::spawn(move || match bt::request_audio(mac, intent) {
        // Preserve each service's Win32 error code to distinguish connection failures.
        Some((a, b)) => {
            let ok = a == 0 || b == 0;
            log(&format!(
                "bt: {why} {verb} audio = {ok} (AudioSink={a} Handsfree={b})"
            ));
        }
        None => log(&format!(
            "bt: {why} {verb} audio superseded by a newer request — skipped"
        )),
    });
}

fn finish_audio_disconnect(ctx: &Ctx) -> bool {
    let intent = ctx.pending_audio_disconnect.lock().unwrap().take();
    let Some((intent, needs_rebuild)) = intent else {
        return false;
    };
    if intent & 1 != 0 && needs_rebuild {
        // The receiver has closed AAP before reaching here. Keep it closed
        // until recovery finishes so a newer Connect cannot skip stereo repair.
        let (disabled, enabled) = bt::reset_audio_services_for_request(ctx.mac(), false, intent);
        log(&format!(
            "bt: reconnect after disconnect — disable={disabled} enable={enabled}"
        ));
        if disabled == 0 && enabled == 0 {
            let mut pending = ctx.pending_audio_disconnect.lock().unwrap();
            pending.rebuild_finished(bt::audio_request_is_current(intent));
        } else {
            if fail_audio_rebuild(
                &ctx.mic_preference,
                &ctx.state,
                &ctx.pending_audio_disconnect,
                || bt::audio_request_is_current(intent),
                || {
                    apply_microphone_enabled(ctx, false);
                    persist_microphone_enabled(false);
                    ctx.user_disconnected.store(true, Ordering::Release);
                    ctx.connect_requested.store(false, Ordering::Release);
                    ctx.wants_reconnect.store(false, Ordering::Release);
                },
            ) {
                ctx.push_state();
                return true;
            }
        }
    } else {
        // Finish the saved disconnect before the receiver reports cleanup done.
        if let Some((sink, handsfree)) = bt::request_audio_for_cleanup(ctx.mac(), intent) {
            log(&format!(
                "bt: saved audio request completed (AudioSink={sink} Handsfree={handsfree})"
            ));
            return sink != 0 && handsfree != 0;
        }
    }
    false
}

fn fail_audio_rebuild(
    preference: &Mutex<()>,
    state: &Mutex<Snapshot>,
    pending: &Mutex<PendingAudioDisconnect>,
    current: impl FnOnce() -> bool,
    disable: impl FnOnce(),
) -> bool {
    // Match microphone commands: preference must be locked before state.
    let _preference = preference.lock().unwrap();
    let mut state = state.lock().unwrap();
    let _pending = pending.lock().unwrap();
    if !current() {
        return false;
    }
    disable();
    state.mic_state = "error".into();
    state.mic_error = Some("Could not restore playback after disconnecting. Repair the connection before enabling the microphone again.".into());
    true
}

#[derive(Default)]
struct PendingAudioDisconnect {
    token: Option<u64>,
    // A later Connect may supersede Disconnect before the profiles were disabled.
    // Conservatively rebuild once after an explicit disconnect, even if no mic
    // state was published yet. A stale completion cannot clear this obligation.
    needs_rebuild: bool,
}

impl PendingAudioDisconnect {
    fn queue_disconnect(&mut self, token: u64) {
        self.token = Some(token);
        self.needs_rebuild = true;
    }

    /// Return an ordinary Connect for immediate dispatch, or let the receiver
    /// finish closing the previous channel and rebuilding playback first.
    fn queue_connect(&mut self, token: u64) -> Option<u64> {
        if self.token.is_some() || self.needs_rebuild {
            self.token = Some(token);
            None
        } else {
            Some(token)
        }
    }

    fn is_pending(&self) -> bool {
        self.token.is_some()
    }

    fn take_repair_if_ready(&self, repair: &AtomicBool) -> Option<bool> {
        (!self.is_pending() && !self.needs_rebuild).then(|| repair.swap(false, Ordering::AcqRel))
    }

    fn take(&mut self) -> Option<(u64, bool)> {
        self.token.take().map(|token| (token, self.needs_rebuild))
    }

    fn rebuild_finished(&mut self, current: bool) {
        if current {
            self.needs_rebuild = false;
        }
    }
}

fn close_control_session(
    uplink_active: bool,
    stop: impl FnOnce() -> io::Result<()>,
    close: impl FnOnce() -> io::Result<()>,
    release_audio: impl FnOnce() -> bool,
) -> (io::Result<()>, io::Result<()>, bool) {
    let stopped = if uplink_active { stop() } else { Ok(()) };
    let closed = close();
    // A failed STOP or close must not leave the explicit disconnect queued.
    let recovery_failed = release_audio();
    (stopped, closed, recovery_failed)
}

/// Avoid repeated reclaim attempts while another host may be using the AirPods.
const RECLAIM_COOLDOWN: Duration = Duration::from_secs(60);

/// Waits total 39 s; a captured Teams handoff took about 13 s to restore AAP.
/// Recheck host activity between attempts and stop when it no longer needs audio.
const RECLAIM_BACKOFF_MS: [u64; 8] = [1_000, 2_000, 3_000, 4_000, 5_000, 6_000, 8_000, 10_000];

/// Full-target recovery for an explicit request or the final microphone fallback.
/// Never use it for an ordinary connection or AAP timeout.
fn force_audio_rebuild(mac: u64, cleanup: bool) {
    let (disabled, enabled) = if cleanup {
        bt::reset_audio_services_for_cleanup(mac, false)
    } else {
        bt::reset_audio_services(mac, false)
    };
    log(&format!(
        "reclaim: audio recovery — disable={disabled} enable={enabled}"
    ));
}

/// Wait for Windows to restore the link while local capture or playback remains active.
/// Keep the connection request until the link returns or retries end.
fn spawn_reclaim(ctx: &Ctx, tier: Tier, request: u64) {
    if !ctx.connect_requested.is_current(request) {
        return;
    }
    if ctx.reclaiming.swap(true, Ordering::SeqCst) {
        return; // a campaign is already running
    }
    log(&format!(
        "reclaim: host held tier {} — reclaiming the audio route",
        tier.label()
    ));
    let ctx = ctx.clone();
    thread::spawn(move || {
        media::init(); // is_playing() needs COM on this thread too
        let mut won = false;
        for (i, wait) in RECLAIM_BACKOFF_MS.iter().enumerate() {
            thread::sleep(Duration::from_millis(*wait));
            // The user pressed Disconnect, or the connect loop gave up.
            if !ctx.connect_requested.is_current(request)
                || !ctx.connect_requested.load(Ordering::Relaxed)
            {
                log("reclaim: no longer requested — standing down");
                break;
            }
            // Stop waiting if this PC no longer needs the audio route.
            if !host_tier(&ctx).defends_audio() {
                log("reclaim: host dropped to tier 3/background — standing down");
                break;
            }
            if bt::is_connected(ctx.mac()) {
                // A returning control link does not justify removing the
                // Windows audio services. Let the normal profile reconnect.
                log("reclaim: link back — leaving the Windows audio route in place");
                won = true;
                break;
            }
            // BluetoothSetServiceState can return error 87 while disconnected; wait for Windows.
            log(&format!(
                "reclaim: attempt {} — AirPods still away, waiting for the link",
                i + 1
            ));
        }
        let _state = ctx.state.lock().unwrap();
        let current = ctx.connect_requested.is_current(request);
        if !current {
            log("reclaim: superseded by a newer connection request");
        } else if won {
            log("reclaim: AirPods connected again");
            ctx.overlay("AirPods connected again");
        } else {
            // A new BLE visit or Windows connection can request another session.
            log("reclaim: gave up — the other device keeps the AirPods");
            ctx.connect_requested.release(request);
        }
        if current {
            *ctx.last_reclaim.lock().unwrap() = Some(Instant::now());
        }
        ctx.reclaiming.store(false, Ordering::SeqCst);
    });
}

/// Disabling capture closes the PCM gate before any Bluetooth recovery work.
fn silence_mic(ctx: &Ctx) {
    ctx.mic_on.store(false, Ordering::Release);
    let _ = ctx.pipe.set_enabled(false);
}

fn fail_mic_audio(
    ctx: &Ctx,
    driver: &driver::Driver,
    mic: &mut mic_lifecycle::Lifecycle,
    now: u64,
    stats: &mut MicStats,
    reason: &str,
) {
    silence_mic(ctx);
    mic.start_failed(now);
    let _ = driver.send(&aap::STOP_AUDIO);
    ctx.state.lock().unwrap().mic_error = Some("Microphone audio failed; capture stopped.".into());
    stats.finish(reason);
}

fn update_microphone_preference(
    gate: &Mutex<()>,
    enabled: bool,
    current: impl FnOnce() -> bool,
    apply: impl FnOnce(bool),
    persist: impl FnOnce(bool),
) {
    let _guard = gate.lock().unwrap();
    if !current() {
        return;
    }
    apply(enabled);
    persist(enabled);
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct MicrophoneRequest {
    revision: u64,
    address: u64,
    enabled: bool,
    mode: ipc::MicMode,
    stop_delay_secs: u32,
    manual_active: bool,
}

impl MicrophoneRequest {
    // Caller holds mic_preference so policy and permission form one snapshot.
    fn read(ctx: &Ctx) -> Self {
        let demand = ctx.mic_demand.lock().unwrap();
        Self {
            revision: ctx.pipe.permission_revision(),
            address: ctx.mac(),
            enabled: ctx.auto_mode.load(Ordering::Acquire),
            mode: demand.mode,
            stop_delay_secs: demand.stop_delay_secs,
            manual_active: demand.manual_active,
        }
    }

    fn current(self, ctx: &Ctx) -> bool {
        self.matches(Self::read(ctx), ctx.shutting_down.load(Ordering::Acquire))
    }

    fn matches(self, current: Self, shutting_down: bool) -> bool {
        !shutting_down && self == current
    }
}

fn set_microphone_enabled(
    ctx: &Ctx,
    enabled: bool,
    persist: bool,
    expected: Option<MicrophoneRequest>,
) {
    update_microphone_preference(
        &ctx.mic_preference,
        enabled,
        || {
            if expected.is_some_and(|request| !request.current(ctx)) {
                return false;
            }
            if enabled && !device_models::microphone(&ctx.state.lock().unwrap()) {
                ctx.state.lock().unwrap().mic_error = Some(
                    "High-quality microphone audio is not supported by this model and firmware."
                        .into(),
                );
                return false;
            }
            true
        },
        |enabled| apply_microphone_enabled(ctx, enabled),
        |enabled| {
            if persist {
                persist_microphone_enabled(enabled && !ctx.shutting_down.load(Ordering::Acquire));
            }
        },
    );
}

// Caller holds mic_preference, including when recovery already holds state.
fn apply_microphone_enabled(ctx: &Ctx, enabled: bool) {
    let enabled = enabled && !ctx.shutting_down.load(Ordering::Acquire);
    ctx.pipe.set_feature_enabled(enabled);
    ctx.auto_mode.store(enabled, Ordering::Release);
    if !enabled {
        ctx.mic_demand.lock().unwrap().clear();
        ctx.mic_on.store(false, Ordering::Release);
        ctx.pipe.revoke_stream();
    }
}

fn persist_microphone_enabled(enabled: bool) {
    if let Ok(base) = std::env::var("LOCALAPPDATA") {
        if std::fs::write(
            format!("{base}\\NTPods\\microphone-enabled"),
            enabled.to_string(),
        )
        .is_err()
        {
            log("mic: could not persist enable preference");
        }
    }
}

fn cancel_microphone_request(ctx: &Ctx) {
    let _preference = ctx.mic_preference.lock().unwrap();
    ctx.mic_demand.lock().unwrap().clear();
    ctx.mic_on.store(false, Ordering::Release);
    ctx.pipe.revoke_stream();
}

fn set_microphone_policy(
    ctx: &Ctx,
    mode: ntpods_ipc::MicMode,
    stop_delay_secs: u32,
    expected: Option<MicrophoneRequest>,
) {
    let _preference = ctx.mic_preference.lock().unwrap();
    if expected.is_some_and(|request| !request.current(ctx)) {
        return;
    }
    let preferences = {
        let mut demand = ctx.mic_demand.lock().unwrap();
        if !demand.configure(mode, stop_delay_secs) {
            return;
        }
        demand.preferences_json()
    };
    // Entering manual mode starts stopped, including when an app is recording.
    if mode == ntpods_ipc::MicMode::Manual && !ctx.mic_demand.lock().unwrap().manual_active {
        ctx.mic_on.store(false, Ordering::Release);
        ctx.pipe.revoke_stream();
    }
    persist_microphone_policy(preferences);
}

fn persist_microphone_policy(preferences: String) {
    if let Ok(base) = std::env::var("LOCALAPPDATA") {
        if std::fs::write(
            format!("{base}\\NTPods\\microphone-policy.json"),
            preferences,
        )
        .is_err()
        {
            log("mic: could not persist microphone policy");
        }
    }
}

fn hide_handsfree(state: &Snapshot) -> bool {
    state.connected
        && !state.link_stale
        && device_models::microphone(state)
        && state.mic_state != "error"
        && state.playback_native_driver
        && state.playback_available
        && (state.playback_state == "ready"
            || (matches!(state.mic_state.as_str(), "stopping" | "recovering")
                && matches!(state.playback_state.as_str(), "starting" | "stopping")))
}

fn handsfree_target(ctx: &Ctx) -> Option<u64> {
    if ctx.shutting_down.load(Ordering::Acquire)
        || ctx.user_disconnected.load(Ordering::Acquire)
        || !ctx.auto_mode.load(Ordering::Acquire)
        || !ctx.pipe.is_open()
    {
        return None;
    }
    let state = ctx.state.lock().unwrap();
    let address = ctx.mac();
    (address != 0
        && state
            .device_id
            .eq_ignore_ascii_case(&format!("{address:012x}"))
        && hide_handsfree(&state))
    .then_some(address)
}

fn manual_microphone_start_allowed(
    state: &Snapshot,
    available: bool,
    connection_requested: bool,
) -> Result<(), &'static str> {
    if !device_models::microphone(state) || !available {
        Err("Microphone unavailable")
    } else if !state.connected || state.link_stale || !connection_requested {
        Err("Connect AirPods to start the microphone")
    } else if state.hearing_aid_pending {
        Err("Hearing settings are changing")
    } else if !playback_allows_microphone(state) {
        Err("Playback is changing")
    } else if matches!(state.mic_state.as_str(), "stopping" | "recovering") {
        Err("Microphone is recovering")
    } else if state.mic_state == "error" {
        Err("Microphone needs attention")
    } else {
        Ok(())
    }
}

fn toggle_microphone_manual(
    ctx: &Ctx,
    may_start: bool,
    expected: Option<MicrophoneRequest>,
) -> Result<bool, &'static str> {
    let _preference = ctx.mic_preference.lock().unwrap();
    if expected.is_some_and(|request| !request.current(ctx)) {
        return Err("Microphone request changed; try again");
    }
    let available = ctx.pipe.is_open();
    let (allowed, stream_active) = {
        let state = ctx.state.lock().unwrap();
        let connection_requested = ctx.connect_requested.load(Ordering::Acquire)
            && !ctx.user_disconnected.load(Ordering::Acquire)
            && !ctx.wants_reconnect.load(Ordering::Acquire)
            && !ctx.shutting_down.load(Ordering::Acquire);
        (
            manual_microphone_start_allowed(&state, available, connection_requested),
            ctx.mic_on.load(Ordering::Acquire)
                || state.mic_uplink_active
                || matches!(state.mic_state.as_str(), "starting" | "capturing"),
        )
    };
    let (active, preferences) = {
        let mut demand = ctx.mic_demand.lock().unwrap();
        let active = demand.toggle_manual(
            ctx.mic_clock.elapsed().as_millis() as u64,
            ctx.auto_mode.load(Ordering::Acquire),
            ctx.capture_active.load(Ordering::Acquire),
            stream_active,
            if may_start {
                allowed
            } else {
                Err("Microphone state changed; try again")
            },
        )?;
        if active {
            ctx.pipe.set_feature_enabled(true);
            ctx.auto_mode.store(true, Ordering::Release);
        } else {
            ctx.mic_on.store(false, Ordering::Release);
            ctx.pipe.revoke_stream();
        }
        (active, demand.preferences_json())
    };
    if active {
        persist_microphone_enabled(true);
    }
    persist_microphone_policy(preferences);
    Ok(active)
}

fn set_microphone_manual(ctx: &Ctx, active: bool, expected: Option<MicrophoneRequest>) {
    let connected = {
        let state = ctx.state.lock().unwrap();
        state.connected && device_models::microphone(&state) && playback_allows_microphone(&state)
    };
    let _preference = ctx.mic_preference.lock().unwrap();
    if expected.is_some_and(|request| !request.current(ctx)) {
        return;
    }
    let permitted = ctx.auto_mode.load(Ordering::Acquire)
        && ctx.connect_requested.load(Ordering::Acquire)
        && !ctx.user_disconnected.load(Ordering::Acquire)
        && !ctx.wants_reconnect.load(Ordering::Acquire)
        && !ctx.shutting_down.load(Ordering::Acquire)
        && connected;
    let mut demand = ctx.mic_demand.lock().unwrap();
    if demand.mode != ntpods_ipc::MicMode::Manual {
        return;
    }
    demand.set_manual(active && permitted);
    if !demand.manual_active {
        ctx.mic_on.store(false, Ordering::Release);
        ctx.pipe.revoke_stream();
    }
}

// Call while holding ctx.state, which also serializes explicit disconnect and
// recovery latches. A late BLE advertisement must not create a new request.
fn claim_automatic_connection(
    requested: &ConnectionRequest,
    user_disconnected: &AtomicBool,
    shutting_down: &AtomicBool,
) -> bool {
    !user_disconnected.load(Ordering::Acquire)
        && !shutting_down.load(Ordering::Acquire)
        && requested.claim()
}

fn publish_mic(ctx: &Ctx, mic: &mic_lifecycle::Lifecycle, ear_allowed: bool) {
    use mic_lifecycle::Phase;
    let name = match mic.phase {
        Phase::Idle => {
            if !ctx.pipe.is_open() {
                "unavailable"
            } else if !ear_allowed
                && ctx.auto_mode.load(Ordering::Acquire)
                && ctx.capture_active.load(Ordering::Acquire)
            {
                "waiting_for_ear"
            } else {
                "idle"
            }
        }
        Phase::Starting => "starting",
        Phase::Capturing => "capturing",
        Phase::Stopping => "stopping",
        Phase::Recovering => "recovering",
        Phase::Error => "error",
    };
    let changed = {
        let mut s = ctx.state.lock().unwrap();
        let changed = s.mic_state != name || s.mic_uplink_active != mic.uplink_active;
        if mic.phase == Phase::Idle && matches!(s.mic_state.as_str(), "stopping" | "recovering") {
            s.mic_error = None;
        }
        s.mic_state = name.into();
        s.mic_uplink_active = mic.uplink_active;
        changed
    };
    if changed {
        ctx.push_state();
    }
}

fn microphone_permitted(ctx: &Ctx) -> bool {
    ctx.auto_mode.load(Ordering::Acquire)
        && ctx.pipe.is_open()
        && ctx.connect_requested.load(Ordering::Acquire)
        && !ctx.user_disconnected.load(Ordering::Acquire)
        && !ctx.wants_reconnect.load(Ordering::Acquire)
        && !ctx.shutting_down.load(Ordering::Acquire)
        && {
            let state = ctx.state.lock().unwrap();
            state.connected
                && device_models::microphone(&state)
                && playback_allows_microphone(&state)
        }
}

// Caller holds mic_preference; neither this check nor its commit sends Bluetooth I/O.
fn microphone_request_current(ctx: &Ctx, revision: u64, now: u64, ear_allowed: bool) -> bool {
    let permitted = microphone_permitted(ctx);
    ear_allowed
        && ctx.pipe.permission_revision() == revision
        && ctx.mic_demand.lock().unwrap().wanted(
            now,
            permitted,
            ctx.capture_active.load(Ordering::Acquire),
            ear_allowed,
        )
}

/// Returns true if this session must close. Stop restores the AirPods audio
/// services once, with one fallback reconnect if that fails.
fn service_mic(
    ctx: &Ctx,
    driver: &driver::Driver,
    mic: &mut mic_lifecycle::Lifecycle,
    now: u64,
    revision: &mut u64,
    stats: &mut MicStats,
    ear_allowed: bool,
) -> bool {
    use mic_lifecycle::{Action, Phase};
    let previous_phase = mic.phase;
    let (wanted, manual_cleared) = {
        // Read permission and intent together when a shortcut changes both.
        let _preference = ctx.mic_preference.lock().unwrap();
        let permitted = microphone_permitted(ctx);
        let mut demand = ctx.mic_demand.lock().unwrap();
        let was_manual = demand.manual_active;
        let wanted = demand.wanted(
            now,
            permitted,
            ctx.capture_active.load(Ordering::Acquire),
            ear_allowed,
        );
        (wanted, was_manual != demand.manual_active)
    };
    if manual_cleared {
        ctx.push_state();
    }
    let current_revision = ctx.pipe.permission_revision();
    let timed_out =
        current_revision == *revision && wanted && ear_allowed && mic.capture_timed_out(now);
    if current_revision != *revision {
        // An off/on toggle may finish between receiver polls.
        mic.retry_after_revocation();
    }
    let action = if current_revision != *revision && mic.accepts_pcm() {
        mic.request_stop(now);
        Action::Stop
    } else {
        mic.tick(now, wanted, ear_allowed)
    };
    *revision = current_revision;
    match action {
        Action::Start => {
            let started = Instant::now();
            stats.begin();
            ctx.state.lock().unwrap().mic_error = None;
            let mut granted = Ok(false);
            while_request_current(
                &ctx.mic_preference,
                || microphone_request_current(ctx, *revision, now, ear_allowed),
                || {
                    granted = ctx.pipe.set_enabled(true);
                    if granted == Ok(true) {
                        mic.start_attempted();
                        // Reserve the stream before releasing the preference lock.
                        ctx.state.lock().unwrap().mic_state = "starting".into();
                    }
                },
            );
            let sent = granted == Ok(true) && driver.send(&aap::START_AUDIO).is_ok();
            stats.start_sent(sent);
            if sent {
                let finished = now.saturating_add(started.elapsed().as_millis() as u64);
                let current = while_request_current(
                    &ctx.mic_preference,
                    || microphone_request_current(ctx, *revision, finished, ear_allowed),
                    || ctx.mic_on.store(true, Ordering::Release),
                );
                if current {
                    mic.start_sent(finished);
                    if ctx.capture_active.load(Ordering::Acquire) {
                        ctx.conv_duck.lock().unwrap().restore();
                    }
                } else {
                    silence_mic(ctx);
                    mic.request_stop(finished);
                    let _ = driver.send(&aap::STOP_AUDIO);
                    stats.finish("start-revoked");
                }
            } else {
                silence_mic(ctx);
                if mic.uplink_active {
                    let _ = driver.send(&aap::STOP_AUDIO);
                }
                if granted == Ok(false) {
                    mic.request_stop(now);
                } else {
                    mic.start_failed(now);
                    ctx.state.lock().unwrap().mic_error =
                        Some("Could not start the microphone stream.".into());
                }
                stats.finish("start-cancelled-or-failed");
            }
        }
        Action::Stop => {
            silence_mic(ctx);
            if timed_out {
                ctx.state.lock().unwrap().mic_error = Some(
                    "Microphone stopped delivering audio. Turn the microphone off and on to retry."
                        .into(),
                );
            }
            stats.finish(if timed_out {
                "pcm-timeout"
            } else {
                "stop-requested"
            });
            let sent = driver.send(&aap::STOP_AUDIO).is_ok();
            log(&format!(
                "mic stop: STOP_AUDIO sent={sent}; waiting for 500ms quiet (2000ms drain limit)"
            ));
            if !sent {
                ctx.state.lock().unwrap().mic_error =
                    Some("Microphone stop failed; recovering the connection.".into());
            }
        }
        Action::RetryStop => {
            silence_mic(ctx);
            let sent = driver.send(&aap::STOP_AUDIO).is_ok();
            let (elapsed, packets, age) = mic.stop_progress(now);
            log(&format!("mic stop: one STOP_AUDIO retry sent={sent}; elapsed={elapsed}ms packets_after_stop={packets} last_audio_age={age:?}ms"));
        }
        Action::ResetAudio => {
            silence_mic(ctx);
            publish_mic(ctx, mic, ear_allowed);
            let (elapsed, packets, age) = mic.stop_progress(now);
            log(&format!("mic stop: restoring AirPods audio services once; elapsed={elapsed}ms packets_after_stop={packets} last_audio_age={age:?}ms uplink_active={}", mic.uplink_active));
            let started = Instant::now();
            let (disable, enable) = a2dp::reset(ctx.mac());
            log(&format!(
                "mic stop: AirPods audio service reset codes {disable}/{enable}"
            ));
            mic.reset_completed(
                now + started.elapsed().as_millis() as u64,
                disable == 0 && enable == 0,
            );
        }
        Action::Reconnect => {
            silence_mic(ctx);
            // Latch before blocking recovery. BLE and receiver resume must not
            // reopen the session while the targeted audio rebuild is running.
            {
                let mut state = ctx.state.lock().unwrap();
                ctx.user_disconnected.store(true, Ordering::Release);
                ctx.connect_requested.store(false, Ordering::Release);
                ctx.wants_reconnect.store(false, Ordering::Release);
                state.mic_error = Some("Microphone stop required audio recovery. Reconnect in NTPods, then enable the microphone to try again.".into());
            }
            set_microphone_enabled(ctx, false, true, None);
            publish_mic(ctx, mic, ear_allowed);
            let (elapsed, packets, age) = mic.stop_progress(now);
            log(&format!("mic stop: reset did not confirm shutdown; elapsed={elapsed}ms packets_after_stop={packets} last_audio_age={age:?}ms; one targeted reconnect, explicit retry required"));
            #[cfg(feature = "experimental-playback")]
            playback::stop_buffer(ctx);
            let _ = driver.disconnect();
            force_audio_rebuild(ctx.mac(), true);
            // The fallback is the last attempt; a fresh control connection and
            // microphone enable require an explicit user action.
            return true;
        }
        Action::None => {
            if matches!(previous_phase, Phase::Stopping | Phase::Recovering)
                && matches!(mic.phase, Phase::Idle | Phase::Error)
                && !mic.uplink_active
            {
                let (elapsed, packets, age) = mic.stop_progress(now);
                log(&format!("mic stop: quiet confirmed; elapsed={elapsed}ms packets_after_stop={packets} last_audio_age={age:?}ms reset_used={}", previous_phase == Phase::Recovering));
                if previous_phase == Phase::Recovering && !matches!(driver.status(), Ok(2)) {
                    // Audio service recovery can also close AAP. End this session
                    // once; the normal loop reopens only under current user
                    // intent and never rebuilds the Windows audio services.
                    if mic.phase == Phase::Error {
                        // A fresh Lifecycle must not erase a failed-start latch
                        // while an application still has the capture open.
                        set_microphone_enabled(ctx, false, true, None);
                        ctx.state.lock().unwrap().mic_error = Some(
                            "Microphone stream failed. Enable the microphone to try again.".into(),
                        );
                    }
                    publish_mic(ctx, mic, ear_allowed);
                    ctx.wants_reconnect.store(false, Ordering::Release);
                    log("mic stop: audio is quiet but AAP closed during recovery; ending this control session once");
                    return true;
                }
            }
        }
    }
    publish_mic(ctx, mic, ear_allowed);
    false
}

// Heart-rate retry timing follows Android's HeartRateMonitor.
/// Allow 12 s for the first reading: confidence filtering drops warm-up samples,
/// and the first accepted sample takes about 6-8 s. Android waits 8 s.
const HR_FIRST_SAMPLE_TIMEOUT_MS: u64 = 12_000;
/// Gap after HRM_STATE before the stream start (Android's START_COMMAND_DELAY).
const HR_START_COMMAND_DELAY_MS: u64 = 120;

/// Increment per sensor command as iOS does; device validation is untested.
/// Keep 128..16383 to preserve the captured two-byte varint encoding.
static HR_SEQ: AtomicU16 = AtomicU16::new(0);

fn publish_head_tracking(ctx: &Ctx, tracking: &head_tracking::Tracker) {
    let mut state = ctx.state.lock().unwrap();
    let error = tracking.error.map(str::to_owned);
    if state.head_tracking_enabled == tracking.enabled
        && state.head_tracking_state == tracking.state
        && state.head_tracking_error == error
    {
        return;
    }
    state.head_tracking_enabled = tracking.enabled;
    state.head_tracking_state = tracking.state.into();
    state.head_tracking_error = error;
    drop(state);
    ctx.push_state();
}

fn set_head_tracking(ctx: &Ctx, on: bool) -> u64 {
    let mut tracking = ctx.head_tracking.lock().unwrap();
    let available = {
        let state = ctx.state.lock().unwrap();
        state.connected
            && !state.link_stale
            && device_models::head_tracking(&state)
            && !ctx.shutting_down.load(Ordering::Acquire)
    };
    let owner = tracking.request(on && available);
    if on && !available {
        tracking.state = "error";
        tracking.error = Some("Connect a supported AirPods model before starting head tracking.");
    }
    publish_head_tracking(ctx, &tracking);
    owner
}

fn service_head_tracking(
    ctx: &Ctx,
    driver: &driver::Driver,
    worn: bool,
    stopped: &AtomicBool,
    refresh: &rtbuddy::PendingMotion,
) {
    let mut tracking = ctx.head_tracking.lock().unwrap();
    if stopped.load(Ordering::Acquire) || ctx.shutting_down.load(Ordering::Acquire) {
        return;
    }
    if let Some(motion) = refresh.take() {
        tracking.refresh_services(motion);
    }
    let firmware = {
        let state = ctx.state.lock().unwrap();
        if !device_models::head_tracking(&state) {
            tracking.request(false);
        }
        state.firmware.clone()
    };
    tracking.update(
        ctx.mic_clock.elapsed().as_millis() as u64,
        worn,
        &firmware,
        |service, on, native| {
            head_tracking::control(next_hr_seq, service, on, native, refresh, |packet| {
                driver.send_sensor(packet).is_ok()
            })
        },
    );
    publish_head_tracking(ctx, &tracking);
}

fn receive_head_tracking(ctx: &Ctx, data: &[u8]) {
    // Optional sensor control must never hold up microphone decoding.
    let Ok(mut tracking) = ctx.head_tracking.try_lock() else {
        return;
    };
    if let Some(sample) = tracking.receive(data, ctx.mic_clock.elapsed().as_millis() as u64) {
        publish_head_tracking(ctx, &tracking);
        ctx.send_event(&Event::HeadTracking {
            timestamp_ms: unix_millis(),
            horizontal: sample.horizontal,
            vertical: sample.vertical,
        });
    }
}

fn close_head_tracking(ctx: &Ctx, driver: &driver::Driver, refresh: &rtbuddy::PendingMotion) {
    let mut tracking = ctx.head_tracking.lock().unwrap();
    if let Some(motion) = refresh.take() {
        tracking.refresh_services(motion);
    }
    tracking.request(false);
    tracking.update(0, false, "", |service, on, native| {
        head_tracking::control(next_hr_seq, service, on, native, refresh, |packet| {
            driver.send_sensor(packet).is_ok()
        })
    });
    tracking.disconnected();
    publish_head_tracking(ctx, &tracking);
}

struct HeadTrackingWorker {
    stopped: Arc<AtomicBool>,
    worn: Arc<AtomicBool>,
    motion_refresh: Arc<rtbuddy::PendingMotion>,
    heart_rate_refresh: Arc<AtomicU64>,
    thread: thread::JoinHandle<()>,
}

impl HeadTrackingWorker {
    fn start(ctx: Ctx, driver: driver::Driver, worn: bool) -> Self {
        let stopped = Arc::new(AtomicBool::new(false));
        let worn = Arc::new(AtomicBool::new(worn));
        let worker_stop = stopped.clone();
        let worker_worn = worn.clone();
        let motion_refresh = Arc::new(rtbuddy::PendingMotion::default());
        let worker_motion = motion_refresh.clone();
        let heart_rate_refresh = Arc::new(AtomicU64::new(0));
        let worker_heart_rate = heart_rate_refresh.clone();
        let thread = thread::spawn(move || {
            while !worker_stop.load(Ordering::Acquire) && !ctx.shutting_down.load(Ordering::Acquire)
            {
                let hr_refresh = worker_heart_rate.swap(0, Ordering::AcqRel);
                if hr_refresh != 0 {
                    let restart = {
                        let _gate = ctx.hr_commands.lock().unwrap();
                        // Enable attempts announce descriptors themselves; a late sample
                        // from the old stream must not restart an active attempt.
                        if claim_heart_rate_refresh(
                            hr_refresh,
                            |generation| {
                                hr_current(&ctx, generation)
                                    && !worker_stop.load(Ordering::Acquire)
                                    && !ctx.shutting_down.load(Ordering::Acquire)
                            },
                            &ctx.hr_retrying,
                            &ctx.hr_got_sample,
                        ) {
                            ctx.hr_generation.fetch_add(1, Ordering::AcqRel);
                            ctx.hr_stream_live.store(false, Ordering::Release);
                            clear_heart_rate(&mut ctx.state.lock().unwrap());
                            true
                        } else {
                            false
                        }
                    };
                    if restart {
                        ctx.push_state();
                        spawn_hr_retry(&ctx);
                    }
                }
                service_head_tracking(
                    &ctx,
                    &driver,
                    worker_worn.load(Ordering::Acquire),
                    &worker_stop,
                    &worker_motion,
                );
                thread::park_timeout(Duration::from_millis(100));
            }
        });
        Self {
            stopped,
            worn,
            motion_refresh,
            heart_rate_refresh,
            thread,
        }
    }

    fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.thread.thread().unpark();
    }

    fn join(self) -> Arc<rtbuddy::PendingMotion> {
        self.stop();
        let _ = self.thread.join();
        self.motion_refresh
    }
}

fn next_hr_seq() -> u16 {
    let n = HR_SEQ.fetch_add(1, Ordering::Relaxed);
    128 + (n % (16_384 - 128))
}
/// Backoff between attempts, indexed by attempt number (last value repeats).
const HR_RETRY_BACKOFF_MS: [u64; 3] = [500, 1_000, 2_000];
/// Bound HR retries without reconnecting the shared audio channel.
/// Toggle heart rate off/on to start another campaign.
const HR_MAX_ATTEMPTS: u32 = 4;

/// How one retry campaign ended.
enum HrOutcome {
    /// A validated sample arrived; the receiver continues decoding.
    Live,
    /// The request was disabled or superseded, or its session ended.
    Stopped,
    /// No driver or no validated sample before retries ran out.
    GiveUp,
}

/// Enable retries or stop both firmware-specific streams and clear the reading.
/// The generation prevents queued work from reviving a disabled stream.
fn set_heart_rate(ctx: &Ctx, on: bool) {
    let gate = ctx.hr_commands.lock().unwrap();
    let on = on && {
        let state = ctx.state.lock().unwrap();
        state.connected && device_models::heart_rate(&state)
    };
    let was = ctx.hr_on.swap(on, Ordering::AcqRel);
    if on != was {
        ctx.hr_generation.fetch_add(1, Ordering::AcqRel);
    }
    {
        let mut state = ctx.state.lock().unwrap();
        state.heart_rate_enabled = on;
        if !on {
            clear_heart_rate(&mut state);
        }
    }
    if !on && was {
        let driver = ctx.driver_cell.lock().unwrap().clone();
        if let Some(drv) = driver {
            let s = next_hr_seq() as u8;
            let _ = drv.send(&aap::hr_stream(s, aap::STREAM_HEART_RATE, 0));
            let _ = drv.send(&aap::hr_stream(
                s.wrapping_add(1),
                aap::STREAM_HEART_RATE_LEGACY,
                0,
            ));
        }
        ctx.hr_db.end_session();
    }
    drop(gate);
    if on && !was {
        spawn_hr_retry(ctx);
    }
    ctx.push_state();
}

fn hr_current(ctx: &Ctx, generation: u64) -> bool {
    ctx.hr_on.load(Ordering::Acquire)
        && ctx.hr_generation.load(Ordering::Acquire) == generation
        && ctx.connect_requested.load(Ordering::Acquire)
        && !ctx.user_disconnected.load(Ordering::Acquire)
        && !ctx.shutting_down.load(Ordering::Acquire)
        && {
            let state = ctx.state.lock().unwrap();
            state.connected && device_models::heart_rate(&state)
        }
}

fn claim_heart_rate_refresh(
    queued: u64,
    current: impl FnOnce(u64) -> bool,
    retrying: &AtomicBool,
    got_sample: &AtomicBool,
) -> bool {
    queued.checked_sub(1).is_some_and(current)
        && !retrying.load(Ordering::Acquire)
        && got_sample.swap(false, Ordering::AcqRel)
}

fn while_request_current(
    gate: &Mutex<()>,
    current: impl FnOnce() -> bool,
    send: impl FnOnce(),
) -> bool {
    let _guard = gate.lock().unwrap();
    if !current() {
        return false;
    }
    send();
    true
}

fn hr_send(ctx: &Ctx, generation: u64, drv: &driver::Driver, packet: &[u8]) -> bool {
    while_request_current(
        &ctx.hr_commands,
        || hr_current(ctx, generation),
        || {
            let _ = drv.send(packet);
        },
    )
}

fn clear_heart_rate(state: &mut Snapshot) -> bool {
    let changed = state.heart_rate.is_some() || state.heart_rate_timestamp_ms.is_some();
    state.heart_rate = None;
    state.heart_rate_timestamp_ms = None;
    changed
}

fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn record_heart_rate(state: &mut Snapshot, bpm: u16, now_ms: u64) {
    state.heart_rate = Some(bpm);
    state.heart_rate_timestamp_ms = Some(now_ms);
}

fn expire_heart_rate(state: &mut Snapshot, now_ms: u64) -> bool {
    if state
        .heart_rate_timestamp_ms
        .is_some_and(|t| now_ms.saturating_sub(t) >= 10_000)
    {
        clear_heart_rate(state)
    } else {
        false
    }
}

/// Keep initialization sleeps and 12 s sample waits off the command and receive threads.
/// `hr_retrying` permits only one worker on the shared driver.
fn spawn_hr_retry(ctx: &Ctx) {
    if ctx.hr_retrying.swap(true, Ordering::SeqCst) {
        return;
    }
    let ctx = ctx.clone();
    thread::spawn(move || loop {
        let generation = ctx.hr_generation.load(Ordering::Acquire);
        let _ = hr_retry_campaign(&ctx, generation);
        let _gate = ctx.hr_commands.lock().unwrap();
        if ctx.hr_on.load(Ordering::Acquire)
            && generation != ctx.hr_generation.load(Ordering::Acquire)
            && !ctx.shutting_down.load(Ordering::Acquire)
        {
            continue;
        }
        ctx.hr_retrying.store(false, Ordering::SeqCst);
        break;
    });
}

/// Retry HR enable on the existing channel, following Android's HeartRateMonitor.
/// An ACK alone does not prove data will arrive; an insufficient MTU can prevent it.
/// Stop retrying after a validated reading or HR_MAX_ATTEMPTS.
/// Never reconnect the shared audio/microphone channel for this optional sensor.
fn hr_retry_campaign(ctx: &Ctx, generation: u64) -> HrOutcome {
    let mut attempt: u32 = 0;
    while hr_current(ctx, generation) {
        ctx.hr_got_sample.store(false, Ordering::Relaxed);
        ctx.hr_stream_live.store(false, Ordering::Relaxed);
        let drv = match ctx.driver_cell.lock().unwrap().clone() {
            Some(d) => d,
            None => {
                log("HR: no driver — retry aborted (re-arms on reconnect)");
                return HrOutcome::GiveUp;
            }
        };
        // AACP 1.3 session init (connect0/caps0/connect4/caps4), re-sent every attempt
        // so each retry re-establishes the session before the enable.
        let init: [(&[u8], u64); 4] = [
            (&aap::HR_CONNECT_SERVICE_0, 180),
            (&aap::HR_CAPABILITIES_SERVICE_0, 220),
            (&aap::HR_CONNECT_SERVICE_4, 180),
            (&aap::HR_CAPABILITIES_SERVICE_4, 220),
        ];
        for (pkt, delay) in init {
            if !hr_current(ctx, generation) {
                return HrOutcome::Stopped;
            }
            if !hr_send(ctx, generation, &drv, pkt) {
                return HrOutcome::Stopped;
            }
            thread::sleep(Duration::from_millis(delay));
        }
        // LibrePods PR #702 enables the PPG engine (0x30) before starting 1 Hz reports.
        if !hr_send(ctx, generation, &drv, &aap::HR_ENABLE) {
            return HrOutcome::Stopped;
        }
        thread::sleep(Duration::from_millis(HR_START_COMMAND_DELAY_MS));
        // Firmware 8* uses interval service 19; 9* uses 84. Send both for compatibility.
        // Report IDs are discovered separately, with a 19/20/84 fallback.
        let s = next_hr_seq() as u8;
        if !hr_send(
            ctx,
            generation,
            &drv,
            &aap::hr_stream(s, aap::STREAM_HEART_RATE, aap::PERIOD_HEART_RATE_US),
        ) {
            return HrOutcome::Stopped;
        } // 84
        thread::sleep(Duration::from_millis(120));
        if !hr_send(
            ctx,
            generation,
            &drv,
            &aap::hr_stream(
                s.wrapping_add(1),
                aap::STREAM_HEART_RATE_LEGACY,
                aap::PERIOD_HEART_RATE_US,
            ),
        ) {
            return HrOutcome::Stopped;
        } // 19

        // Acknowledgments and empty frames do not confirm a usable heart-rate stream.
        let wait_until = Instant::now() + Duration::from_millis(HR_FIRST_SAMPLE_TIMEOUT_MS);
        while Instant::now() < wait_until {
            if !hr_current(ctx, generation) {
                return HrOutcome::Stopped;
            }
            if ctx.hr_got_sample.load(Ordering::Relaxed) {
                log("HR: stream live (reading decoded) — retries done");
                return HrOutcome::Live;
            }
            thread::sleep(Duration::from_millis(200));
        }

        attempt += 1;
        let streaming = ctx.hr_stream_live.load(Ordering::Relaxed);
        log(&format!(
            "HR retry: attempt={attempt} — no reading in {}s (stream_frames={streaming})",
            HR_FIRST_SAMPLE_TIMEOUT_MS / 1000
        ));
        // Exhausted sensor retries must not reconnect the shared audio channel.
        if attempt >= HR_MAX_ATTEMPTS {
            log(
                "HR: no reading after the enable retries (ACKs only) — giving up; \
                 toggle HR off/on to retry",
            );
            ctx.overlay("Heart rate unavailable");
            return HrOutcome::GiveUp;
        }
        let backoff =
            HR_RETRY_BACKOFF_MS[(attempt as usize - 1).min(HR_RETRY_BACKOFF_MS.len() - 1)];
        let nap_until = Instant::now() + Duration::from_millis(backoff);
        while Instant::now() < nap_until && hr_current(ctx, generation) {
            thread::sleep(Duration::from_millis(100));
        }
    }
    HrOutcome::Stopped
}

fn set_device_control(ctx: &Ctx, id: u8, value: u32) {
    let _write = ctx.settings_write.lock().unwrap();
    let driver = ctx.driver_cell.lock().unwrap().clone();
    let token = {
        let mut pending = ctx.settings_pending.lock().unwrap();
        let mut state = ctx.state.lock().unwrap();
        let error = if !device_settings::valid_control(id, value) {
            Some("Unsupported device setting or value.")
        } else if !device_models::control(&state, id, value) {
            Some("This setting is not supported or has not been reported by these AirPods.")
        } else if id == 0x2E && pending.anc_to_reassert(&state) != Some(4) {
            Some("Select Adaptive and wait for the AirPods to confirm it before changing its strength.")
        } else if !state.connected
            || ctx.user_disconnected.load(Ordering::Acquire)
            || ctx.shutting_down.load(Ordering::Acquire)
            || driver.is_none()
        {
            Some("Connect the AirPods before changing this setting.")
        } else {
            None
        };
        state.device_settings_error = error.map(str::to_owned);
        error
            .is_none()
            .then(|| pending.begin_control(&mut state, id, value))
    };
    if let Some(token) = token {
        let driver = driver.unwrap();
        let sent = driver.send(&aap::device_control_command(id, value)).is_ok();
        let save_off = {
            let mut pending = ctx.settings_pending.lock().unwrap();
            let mut state = ctx.state.lock().unwrap();
            pending.finish_control(&mut state, id, value, token, sent)
                && sent
                && id == 0x34
                && state.allow_off_requested.is_some()
        };
        if save_off && device_settings::save_allow_off_choice(ctx.mac(), value == 1).is_err() {
            ctx.state.lock().unwrap().device_settings_error =
                Some("Sent the setting, but could not save it for reconnects.".into());
        }
    }
    drop(_write);
    if id == ipc::feature::CONVERSATIONAL_AWARENESS && value == 2 {
        ctx.conv_duck.lock().unwrap().restore();
    }
    ctx.push_state();
}

fn set_equalizer(ctx: &Ctx, eq: ipc::Equalizer) {
    let _write = ctx.settings_write.lock().unwrap();
    let driver = ctx.driver_cell.lock().unwrap().clone();
    let token = {
        let mut pending = ctx.settings_pending.lock().unwrap();
        let mut state = ctx.state.lock().unwrap();
        let error = if !device_settings::valid_equalizer(eq) {
            Some("EQ mode must be recommended or custom, with each band between 0 and 100.")
        } else if !device_models::equalizer(&state) {
            Some("Custom EQ is not supported by this model and firmware.")
        } else if !state.connected
            || ctx.user_disconnected.load(Ordering::Acquire)
            || ctx.shutting_down.load(Ordering::Acquire)
            || driver.is_none()
        {
            Some("Connect the AirPods before changing EQ.")
        } else {
            None
        };
        state.device_settings_error = error.map(str::to_owned);
        error.is_none().then(|| pending.begin_eq(&mut state, eq))
    };
    if let Some(token) = token {
        let sent = driver.unwrap().send(&aap::eq_command(eq)).is_ok();
        let mut pending = ctx.settings_pending.lock().unwrap();
        pending.finish_eq(&mut ctx.state.lock().unwrap(), token, sent);
    }
    drop(_write);
    ctx.push_state();
}

fn loud_sound_request(ctx: &Ctx, requested: Option<bool>) {
    let work = {
        let _write = ctx.settings_write.lock().unwrap();
        let driver = ctx.driver_cell.lock().unwrap().clone();
        let mut state = ctx.state.lock().unwrap();
        if state.loud_sound_reduction_pending {
            return;
        }
        let error = if !device_models::loud_sound_reduction(&state) {
            Some("Loud Sound Reduction is not supported by this model.")
        } else if !state.connected
            || state.link_stale
            || ctx.user_disconnected.load(Ordering::Acquire)
            || ctx.shutting_down.load(Ordering::Acquire)
            || driver.is_none()
        {
            Some("Connect the AirPods before changing Loud Sound Reduction.")
        } else if requested.is_some() && state.loud_sound_reduction.is_none() {
            Some("Read Loud Sound Reduction from the AirPods before changing it.")
        } else {
            None
        };
        state.loud_sound_reduction_error = error.map(str::to_owned);
        if error.is_none() {
            state.loud_sound_reduction_pending = true;
            Some((driver.unwrap(), hearing::loud_sound_token()))
        } else {
            None
        }
    };
    ctx.push_state();
    if let Some((driver, token)) = work {
        let ctx = ctx.clone();
        thread::spawn(move || {
            let result = hearing::loud_sound_reduction(&driver, token, requested);
            {
                let _write = ctx.settings_write.lock().unwrap();
                let mut state = ctx.state.lock().unwrap();
                if !token.is_current() || !state.connected {
                    return;
                }
                state.loud_sound_reduction_pending = false;
                match result {
                    Ok(Some(value)) => {
                        state.loud_sound_reduction = Some(value);
                        state.loud_sound_reduction_error =
                            requested.filter(|requested| *requested != value).map(|_| {
                                "AirPods did not apply Loud Sound Reduction. Try again.".into()
                            });
                    }
                    Err(error) => state.loud_sound_reduction_error = Some(error),
                    Ok(None) => return,
                }
            }
            ctx.push_state();
        });
    }
}

#[derive(Clone, Copy)]
struct HearingRequest {
    request_id: u64,
    on: bool,
    expected: Option<ipc::HearingAidSettings>,
    settings: Option<ipc::HearingAidSettings>,
}

impl HearingRequest {
    fn matches_baseline(&self, current: Option<ipc::HearingAidSettings>) -> bool {
        self.expected.is_some() && self.expected == current
    }
}

fn hearing_request(ctx: &Ctx, requested: Option<HearingRequest>) {
    let disabling = requested.is_some_and(|request| !request.on);
    let work = {
        let _write = ctx.settings_write.lock().unwrap();
        // Reserve the idle check through pending publication against microphone START.
        let _preference = ctx.mic_preference.lock().unwrap();
        let driver = ctx.driver_cell.lock().unwrap().clone();
        let authorized = hearing::authorization();
        let mut state = ctx.state.lock().unwrap();
        let error = if !state.connected
            || state.link_stale
            || !device_models::hearing_aid(&state)
            || ctx.user_disconnected.load(Ordering::Acquire)
            || ctx.shutting_down.load(Ordering::Acquire)
            || driver.is_none()
        {
            Some("Connect supported AirPods before changing hearing assistance.")
        } else if !disabling && state.hearing_aid_pending {
            Some("Wait for the current hearing-settings request to finish.")
        } else if !disabling
            && (ctx.capture_active.load(Ordering::Acquire)
                || ctx.mic_on.load(Ordering::Acquire)
                || !matches!(state.mic_state.as_str(), "idle" | "unavailable")
                || ctx.pipe.status().is_some_and(|streams| streams != 0))
        {
            Some("Stop recording and wait for microphone recovery before reading or changing hearing settings.")
        } else if requested.is_some_and(|request| {
            request.on
                && (request.settings.is_none_or(|s| !s.is_valid())
                    || request.expected.is_none_or(|s| !s.is_valid()))
        }) {
            Some("Complete, valid audiograms and hearing settings are required.")
        } else if requested.is_some_and(|request| request.on)
            && (state.hearing_aid_settings.is_none() || !hearing::has_current_read())
        {
            Some("Read the existing audiograms before enabling or editing hearing assistance.")
        } else if requested.is_some_and(|request| request.on) && !hearing::can_edit_current_read() {
            Some("These hearing settings are read-only in NTPods.")
        } else if requested.is_some_and(|request| {
            request.on && !request.matches_baseline(state.hearing_aid_settings)
        }) {
            Some("Hearing settings changed since this edit began. Read them again before editing.")
        } else if requested.is_some_and(|request| request.on)
            && hearing::reported_state().0.is_none()
        {
            Some("Reconnect to confirm whether hearing assistance is enabled before changing it.")
        } else {
            None
        };
        state.hearing_aid_error = error.map(str::to_owned);
        if let Some(error) = error {
            if requested.is_none() {
                state.hearing_aid_settings = None;
            }
            if let Some(request) = requested {
                ctx.send_event(&Event::CommandResult {
                    command: "set_hearing_aid".into(),
                    request_id: request.request_id,
                    ok: false,
                    message: error.into(),
                });
            }
            None
        } else {
            if requested.is_none() {
                state.hearing_aid_settings = None;
            }
            state.hearing_aid_pending = true;
            Some((driver.unwrap(), hearing::next_gen(), authorized))
        }
    };
    ctx.push_state();
    if let Some((driver, gen, authorized)) = work {
        let ctx = ctx.clone();
        thread::spawn(move || {
            let result = match requested {
                Some(request) => hearing::apply(
                    &driver,
                    gen,
                    request.on,
                    authorized,
                    request.expected,
                    request.settings,
                ),
                None => {
                    hearing::read(&driver, gen).map(|value| value.map(hearing::Applied::Settings))
                }
            };
            let _write = ctx.settings_write.lock().unwrap();
            if !hearing::is_current(gen) {
                return;
            }
            let mut state = ctx.state.lock().unwrap();
            state.hearing_aid_pending = false;
            let outcome = match result {
                Ok(Some(hearing::Applied::Settings(settings))) => {
                    state.hearing_aid_settings = Some(settings);
                    state.hearing_aid_error = None;
                    Ok(())
                }
                Ok(Some(hearing::Applied::Disabled)) => {
                    state.hearing_aid_error = None;
                    Ok(())
                }
                Err(error) => {
                    log(&format!("Hearing settings: {error}"));
                    state.hearing_aid_settings = None;
                    state.hearing_aid_error = Some(error.clone());
                    Err(error)
                }
                Ok(None) => return,
            };
            drop(state);
            if let Some(request) = requested {
                ctx.send_event(&Event::CommandResult {
                    command: "set_hearing_aid".into(),
                    request_id: request.request_id,
                    ok: outcome.is_ok(),
                    message: outcome.err().unwrap_or_default(),
                });
            }
            drop(_write);
            ctx.push_state();
        });
    }
}

fn transparency_request(ctx: &Ctx, requested: Option<ipc::TransparencySettings>) {
    let work = {
        let _write = ctx.settings_write.lock().unwrap();
        let driver = ctx.driver_cell.lock().unwrap().clone();
        let mut state = ctx.state.lock().unwrap();
        if state.transparency_pending {
            return;
        }
        let error = if requested.is_some_and(|s| !s.is_valid()) {
            Some("Custom Transparency values are outside the supported range.")
        } else if !device_models::find(&state.model).is_some_and(|p| p.custom_transparency) {
            Some("Custom Transparency is not supported by this model.")
        } else if !state.connected
            || state.link_stale
            || ctx.user_disconnected.load(Ordering::Acquire)
            || ctx.shutting_down.load(Ordering::Acquire)
            || driver.is_none()
        {
            Some("Connect the AirPods before changing Custom Transparency.")
        } else if requested.is_some() && state.transparency.is_none() {
            Some("Read Custom Transparency from the AirPods before changing it.")
        } else {
            None
        };
        state.transparency_error = error.map(str::to_owned);
        if error.is_none() {
            state.transparency_pending = true;
            Some((
                driver.unwrap(),
                transparency::token(),
                device_models::hearing_aid(&state),
            ))
        } else {
            None
        }
    };
    ctx.push_state();
    if let Some((driver, token, hearing_supported)) = work {
        let ctx = ctx.clone();
        thread::spawn(move || {
            let result =
                transparency::settings(&driver, token, requested.as_ref(), hearing_supported);
            {
                let _write = ctx.settings_write.lock().unwrap();
                let mut state = ctx.state.lock().unwrap();
                if !token.is_current() || !state.connected {
                    return;
                }
                state.transparency_pending = false;
                match result {
                    Ok(Some(value)) => {
                        state.transparency = Some(value);
                        state.transparency_error = requested.filter(|s| *s != value)
                            .map(|_| "AirPods did not apply Custom Transparency. Read the settings and try again.".into());
                    }
                    Err(error) => {
                        log(&format!("Custom Transparency: {error}"));
                        state.transparency_error = Some(error);
                    }
                    Ok(None) => return,
                }
            }
            ctx.push_state();
        });
    }
}

fn reset_device_settings(ctx: &Ctx) {
    hearing::invalidate_session();
    ctx.hr_generation.fetch_add(1, Ordering::AcqRel);
    let mut pending = ctx.settings_pending.lock().unwrap();
    let mut state = ctx.state.lock().unwrap();
    pending.reset(&mut state);
    if device_settings::supports_silent_allow_off(&state.model) {
        state.allow_off_requested = device_settings::load_allow_off_choice(ctx.mac());
    }
}

fn settings_refresh_allowed(
    mic: &mic_lifecycle::Lifecycle,
    capture_streams: Option<u32>,
    capture_active: bool,
) -> bool {
    mic.phase == mic_lifecycle::Phase::Idle
        && !mic.cleanup_pending()
        && capture_streams == Some(0)
        && !capture_active
}

fn queue_settings_refresh(
    requested: &AtomicBool,
    state: &mut Snapshot,
    capture_streams: Option<u32>,
    capture_active: bool,
) {
    state.device_settings_error = if capture_streams.is_none() {
        Some(
            "Microphone status is unavailable. Try refreshing settings when it is available."
                .into(),
        )
    } else if capture_streams != Some(0)
        || capture_active
        || state.mic_capture_active
        || state.mic_state != "idle"
        || state.mic_uplink_active
        || state.mic_recording
    {
        Some("Finish recording and wait for microphone recovery before refreshing settings.".into())
    } else {
        None
    };
    // A request made during capture must not run after that capture finishes.
    requested.store(state.device_settings_error.is_none(), Ordering::Release);
}

/// Only the receiver calls this, before it can start a microphone stream.
fn take_settings_refresh(ctx: &Ctx, mic: &mic_lifecycle::Lifecycle) -> bool {
    if !ctx.wants_settings_refresh.swap(false, Ordering::AcqRel) {
        return false;
    }
    // Query the driver: a new capture may not be in the 100 ms polled state yet.
    let capture_streams = ctx.pipe.status();
    let idle = settings_refresh_allowed(
        mic,
        capture_streams,
        ctx.capture_active.load(Ordering::Acquire),
    );
    let mut state = ctx.state.lock().unwrap();
    let mut last = ctx.last_repair.lock().unwrap();
    state.device_settings_error = if !state.connected
        || !ctx.connect_requested.load(Ordering::Acquire)
        || ctx.user_disconnected.load(Ordering::Acquire)
        || ctx.wants_reconnect.load(Ordering::Acquire)
        || ctx.shutting_down.load(Ordering::Acquire)
    {
        Some("Connect the AirPods before refreshing settings.".into())
    } else if capture_streams.is_none() {
        Some(
            "Microphone status is unavailable. Try refreshing settings when it is available."
                .into(),
        )
    } else if !idle {
        Some("Finish recording and wait for microphone recovery before refreshing settings.".into())
    } else if last.is_some_and(|t| t.elapsed() < Duration::from_secs(5)) {
        Some("Wait a few seconds before refreshing settings again.".into())
    } else {
        *last = Some(Instant::now());
        state.connected = false;
        None
    };
    let accepted = state.device_settings_error.is_none();
    drop(last);
    drop(state);
    ctx.push_state();
    accepted
}

#[derive(Default)]
struct ControlLinkStatus {
    stale: bool,
    disconnected_checks: u32,
}

impl ControlLinkStatus {
    fn observe(
        &mut self,
        driver_connected: bool,
        windows_connected: bool,
        mic_active: bool,
        data_age: Duration,
    ) -> bool {
        if driver_connected {
            self.disconnected_checks = 0;
            self.stale = false;
        } else if windows_connected {
            self.disconnected_checks = 0;
            // Quiet AAP alone is normal; require a transport failure as well.
            if !mic_active && data_age >= Duration::from_secs(30) {
                self.stale = true;
            }
        } else {
            self.disconnected_checks += 1;
        }
        self.disconnected_checks >= 3
    }
}

fn reassert_confirmed_anc(ctx: &Ctx) -> bool {
    let Ok(_write) = ctx.settings_write.try_lock() else {
        return false;
    };
    let mode = {
        let pending = ctx.settings_pending.lock().unwrap();
        let state = ctx.state.lock().unwrap();
        if !state.connected
            || ctx.user_disconnected.load(Ordering::Acquire)
            || ctx.shutting_down.load(Ordering::Acquire)
        {
            return false;
        }
        pending
            .anc_to_reassert(&state)
            .filter(|mode| device_models::control(&state, 0x0D, u32::from(*mode)))
    };
    let Some(mode) = mode else {
        return false;
    };
    let driver = ctx.driver_cell.lock().unwrap().clone();
    driver.is_some_and(|driver| driver.send(&aap::anc_command(mode)).is_ok())
}

fn update_auto_pause(
    policy: &Mutex<media::AutoPausePolicy>,
    state: &Mutex<Snapshot>,
    on: bool,
    persist: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<()> {
    let mut policy = policy.lock().unwrap();
    policy.set_enabled(on);
    state.lock().unwrap().auto_pause = on;
    let result = persist();
    drop(policy);
    result
}

fn update_conversation_volume(
    duck: &Mutex<volume::ConvDuck>,
    state: &Mutex<Snapshot>,
    percent: u8,
    persist: impl FnOnce() -> std::io::Result<()>,
) -> std::io::Result<()> {
    if percent > 100 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "percent must be 0..=100",
        ));
    }
    let mut duck = duck.lock().unwrap();
    duck.set_limit(percent);
    state.lock().unwrap().conversation_volume = percent;
    // Serialize persistence with the preference so two clients cannot save out of order.
    let result = persist();
    drop(duck);
    result
}

fn send_name_request(
    name: &str,
    send: impl FnOnce() -> io::Result<()>,
) -> Result<(), &'static str> {
    if name.trim().is_empty() || name.len() > 64 {
        return Err("Use a name containing 1 to 64 UTF-8 bytes.");
    }
    send().map_err(|_| "Could not send the name. Check the AirPods connection.")
}

fn shutdown_exit_code(
    receiver_done: bool,
    audio_done: bool,
    cleanup_failed: bool,
    playback_restore_pending: io::Result<bool>,
) -> i32 {
    i32::from(
        !receiver_done || !audio_done || cleanup_failed || playback_restore_pending.unwrap_or(true),
    )
}

fn reserve_native_playback_transition(state: &mut Snapshot) {
    if state.playback_native_driver {
        state.playback_state = "starting".into();
        state.playback_bitrate_kbps = None;
    }
}

fn native_reconnect_allowed(state: &Snapshot, mic_on: bool) -> bool {
    state.playback_buffer_target_ms.is_none()
        && (!state.playback_native_driver
            || (!mic_on
                && !state.mic_uplink_active
                && !matches!(
                    state.mic_state.as_str(),
                    "starting" | "capturing" | "stopping" | "recovering"
                )))
}

#[cfg(feature = "experimental-playback")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BufferPauseFor {
    Playback,
    Microphone,
}

#[cfg(feature = "experimental-playback")]
fn microphone_toggle_starts(
    demand: &mut mic_lifecycle::Demand,
    now: u64,
    enabled: bool,
    capture: bool,
    stream_active: bool,
) -> bool {
    !demand.wanted(now, enabled, capture, true) && !stream_active
}

#[cfg(feature = "experimental-playback")]
fn buffer_pause_for(cmd: &Command, capture: bool, toggle_starts: bool) -> Option<BufferPauseFor> {
    match cmd {
        Command::Connect
        | Command::RebuildAudio
        | Command::RepairConnection
        | Command::SetPlaybackMode { .. }
        | Command::RetryPlayback
        | Command::SetPlaybackBitrate { .. }
        | Command::SetPlaybackBuffer {
            target_ms: Some(_), ..
        } => Some(BufferPauseFor::Playback),
        Command::SetMicMode { auto: true, .. }
        | Command::SetMicManualActive { active: true }
        | Command::SetMicPolicy {
            mode: ipc::MicMode::KeepActive,
            ..
        } => Some(BufferPauseFor::Microphone),
        Command::SetMicPolicy {
            mode: ipc::MicMode::Automatic,
            ..
        } if capture => Some(BufferPauseFor::Microphone),
        Command::ToggleMicManual if toggle_starts => Some(BufferPauseFor::Microphone),
        _ => None,
    }
}

#[cfg(feature = "experimental-playback")]
struct AudioChangeWorker(Arc<AtomicBool>);

#[cfg(feature = "experimental-playback")]
impl Drop for AudioChangeWorker {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

#[cfg(feature = "experimental-playback")]
fn defer_audio_change(
    busy: Arc<AtomicBool>,
    apply: impl FnOnce() + Send + 'static,
) -> io::Result<thread::JoinHandle<()>> {
    if busy
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "Audio is changing. Try again shortly.",
        ));
    }
    let worker = AudioChangeWorker(busy);
    thread::Builder::new()
        .name("playback-restore-command".into())
        .spawn(move || {
            let _worker = worker;
            apply();
        })
}

#[cfg(feature = "experimental-playback")]
fn audio_change_error(ctx: &Ctx, reason: BufferPauseFor, message: String) {
    let mut state = ctx.state.lock().unwrap();
    match reason {
        BufferPauseFor::Playback => state.playback_error = Some(message),
        BufferPauseFor::Microphone => state.mic_error = Some(message),
    }
    drop(state);
    ctx.push_state();
}

fn apply_command(ctx: &Ctx, cmd: Command) {
    // Commands may contain personal settings; do not log payloads.
    if ctx.shutting_down.load(Ordering::Acquire) {
        return;
    }
    #[cfg(feature = "experimental-playback")]
    let (pause_reason, toggle_starts, microphone_request, playback_request) =
        if buffer_pause_for(&cmd, true, true).is_some() {
            let _preference = ctx.mic_preference.lock().unwrap();
            let capture = ctx.capture_active.load(Ordering::Acquire);
            let toggle_starts = if matches!(&cmd, Command::ToggleMicManual) {
                let state = ctx.state.lock().unwrap();
                let stream_active = ctx.mic_on.load(Ordering::Acquire)
                    || state.mic_uplink_active
                    || matches!(state.mic_state.as_str(), "starting" | "capturing");
                microphone_toggle_starts(
                    &mut ctx.mic_demand.lock().unwrap(),
                    ctx.mic_clock.elapsed().as_millis() as u64,
                    ctx.auto_mode.load(Ordering::Acquire),
                    capture,
                    stream_active,
                )
            } else {
                false
            };
            let reason = buffer_pause_for(&cmd, capture, toggle_starts);
            let (microphone, playback) = if reason.is_some() {
                let selected = ctx.playback_selection.lock().unwrap();
                (
                    Some(MicrophoneRequest::read(ctx)),
                    Some((bt::playback_intent(), selected.revision, selected.bitrates)),
                )
            } else {
                (None, None)
            };
            (reason, toggle_starts, microphone, playback)
        } else {
            (None, false, None, None)
        };
    #[cfg(not(feature = "experimental-playback"))]
    let toggle_starts = true;
    #[cfg(not(feature = "experimental-playback"))]
    let microphone_request = None;
    #[cfg(feature = "experimental-playback")]
    let buffer_pause = if let Some(reason) = pause_reason {
        if ctx.audio_change_busy.load(Ordering::Acquire) {
            audio_change_error(ctx, reason, "Audio is changing. Try again shortly.".into());
            return;
        }
        match playback::begin_automatic_pause(ctx) {
            Ok(guard) => guard,
            Err(error) => {
                audio_change_error(
                    ctx,
                    reason,
                    format!("Could not restore the playback buffer: {error}"),
                );
                return;
            }
        }
    } else {
        None
    };
    #[cfg(feature = "experimental-playback")]
    if buffer_pause
        .as_ref()
        .is_some_and(|pause| pause.is_pending())
    {
        let worker_ctx = ctx.clone();
        let reason = pause_reason.unwrap();
        let result = defer_audio_change(ctx.audio_change_busy.clone(), move || {
            if let Err(error) = buffer_pause.as_ref().unwrap().wait() {
                audio_change_error(
                    &worker_ctx,
                    reason,
                    format!("Could not restore the playback buffer: {error}"),
                );
                return;
            }
            apply_prepared_command(
                &worker_ctx,
                cmd,
                toggle_starts,
                microphone_request,
                playback_request,
                buffer_pause,
            );
        });
        if let Err(error) = result {
            audio_change_error(ctx, reason, error.to_string());
        }
        return;
    }
    #[cfg(feature = "experimental-playback")]
    if let Some(pause) = &buffer_pause {
        if let Err(error) = pause.wait() {
            audio_change_error(
                ctx,
                pause_reason.unwrap(),
                format!("Could not restore the playback buffer: {error}"),
            );
            return;
        }
    }
    #[cfg(feature = "experimental-playback")]
    apply_prepared_command(
        ctx,
        cmd,
        toggle_starts,
        microphone_request,
        playback_request,
        buffer_pause,
    );
    #[cfg(not(feature = "experimental-playback"))]
    apply_prepared_command(ctx, cmd, toggle_starts, microphone_request);
}

fn apply_prepared_command(
    ctx: &Ctx,
    cmd: Command,
    toggle_starts: bool,
    microphone_request: Option<MicrophoneRequest>,
    #[cfg(feature = "experimental-playback")] playback_request: Option<(
        u64,
        u64,
        [ipc::PlaybackBitrate; 2],
    )>,
    #[cfg(feature = "experimental-playback")] _buffer_pause: Option<playback::AutomaticPause>,
) {
    if ctx.shutting_down.load(Ordering::Acquire) {
        return;
    }
    // Reserve the check through intent creation; microphone START uses this lock too.
    let mut reconnect_guard = if matches!(
        &cmd,
        Command::Connect | Command::RebuildAudio | Command::RepairConnection
    ) {
        let guard = ctx.mic_preference.lock().unwrap();
        let mut state = ctx.state.lock().unwrap();
        #[cfg(feature = "experimental-playback")]
        if microphone_request.is_some_and(|request| !request.current(ctx))
            || playback_request.is_some_and(|request| bt::playback_intent() != request.0)
        {
            return;
        }
        if !native_reconnect_allowed(&state, ctx.mic_on.load(Ordering::Acquire)) {
            state.playback_error = Some(if state.playback_buffer_target_ms.is_some() {
                "End the buffer trial before reconnecting audio. If restoration failed, disconnect first."
            } else {
                "Stop the microphone before reconnecting audio."
            }.into());
            drop(state);
            ctx.push_state();
            return;
        }
        state.playback_error = None;
        Some(guard)
    } else {
        None
    };
    match cmd {
        Command::GetLoudSoundReduction => loud_sound_request(ctx, None),
        Command::GetTransparency => transparency_request(ctx, None),
        Command::SetTransparency { settings } => transparency_request(ctx, Some(settings)),
        Command::SetLoudSoundReduction { on } => loud_sound_request(ctx, Some(on)),
        Command::Ping => {}
        Command::Hello { .. } | Command::GetState => ctx.push_state(),
        Command::SetAnc { mode } => set_device_control(ctx, 0x0D, mode.into()),
        Command::SetMicMode { auto, manual: _ } => {
            // The legacy flag remains the global enable switch.
            set_microphone_enabled(ctx, auto, true, microphone_request);
            ctx.push_state();
        }
        Command::SetMicPolicy {
            mode,
            stop_delay_secs,
        } => {
            set_microphone_policy(ctx, mode, stop_delay_secs, microphone_request);
            ctx.push_state();
        }
        Command::SetMicManualActive { active } => {
            set_microphone_manual(ctx, active, microphone_request);
            ctx.push_state();
        }
        Command::ToggleMicManual => {
            let result = toggle_microphone_manual(ctx, toggle_starts, microphone_request);
            ctx.push_state();
            if let Err(message) = result {
                ctx.overlay(message);
            }
        }
        Command::SetPlaybackMode { mode } => {
            #[cfg(feature = "experimental-playback")]
            playback::set_mode(ctx, Some(mode), playback_request.unwrap());
            #[cfg(not(feature = "experimental-playback"))]
            {
                let _ = mode;
                ctx.state.lock().unwrap().playback_error =
                    Some("Playback codecs are unavailable in this build.".into());
                ctx.push_state();
            }
        }
        Command::SetPlaybackBuffer {
            target_ms,
            restore_ms,
        } => {
            #[cfg(feature = "experimental-playback")]
            playback::set_buffer(ctx, target_ms, restore_ms, || {
                if target_ms.is_none() {
                    return true;
                }
                if microphone_request.is_some_and(|request| !request.current(ctx))
                    || _buffer_pause
                        .as_ref()
                        .is_some_and(|pause| !pause.is_current())
                {
                    return false;
                }
                let selected = ctx.playback_selection.lock().unwrap();
                playback_request.is_some_and(|request| {
                    request == (bt::playback_intent(), selected.revision, selected.bitrates)
                })
            });
            #[cfg(not(feature = "experimental-playback"))]
            {
                let _ = (target_ms, restore_ms);
                ctx.state.lock().unwrap().playback_buffer_error =
                    Some("Native playback is unavailable in this build.".into());
                ctx.push_state();
            }
        }
        Command::RetryPlayback => {
            #[cfg(feature = "experimental-playback")]
            playback::set_mode(ctx, None, playback_request.unwrap());
            #[cfg(not(feature = "experimental-playback"))]
            {
                ctx.state.lock().unwrap().playback_error =
                    Some("Playback codecs are unavailable in this build.".into());
                ctx.push_state();
            }
        }
        Command::SetPlaybackBitrate {
            mode,
            best_effort,
            kbps,
            variable,
        } => {
            #[cfg(feature = "experimental-playback")]
            playback::set_bitrate(
                ctx,
                mode,
                ipc::PlaybackBitrate {
                    best_effort,
                    kbps,
                    variable,
                },
                playback_request.unwrap(),
            );
            #[cfg(not(feature = "experimental-playback"))]
            {
                let _ = (mode, best_effort, kbps, variable);
                ctx.state.lock().unwrap().playback_error =
                    Some("Playback codecs are unavailable in this build.".into());
                ctx.push_state();
            }
        }
        Command::SetFeature { feature, on } => {
            if device_settings::is_toggle(feature) {
                set_device_control(ctx, feature, if on { 1 } else { 2 });
            } else {
                ctx.state.lock().unwrap().device_settings_error =
                    Some("This device setting is not a toggle.".into());
                ctx.push_state();
            }
        }
        Command::SetControl { id, value } => {
            let value = u32::from(value) | if id == 0x1F { 0x5000 } else { 0 };
            set_device_control(ctx, id, value);
        }
        Command::SetDeviceControl { id, value } => set_device_control(ctx, id, value),
        Command::SetEq {
            mode,
            low,
            mid,
            high,
        } => {
            set_equalizer(
                ctx,
                ipc::Equalizer {
                    mode,
                    low,
                    mid,
                    high,
                },
            );
        }
        Command::SetAutoPause { on } => {
            let saved = update_auto_pause(&ctx.auto_pause, &ctx.state, on, || {
                let base = std::env::var("LOCALAPPDATA").map_err(std::io::Error::other)?;
                std::fs::write(format!("{base}\\NTPods\\auto-pause"), on.to_string())
            });
            if saved.is_err() {
                ctx.overlay("Could not save automatic pause for the next start.");
            }
            ctx.push_state();
        }
        Command::SetBleScanWhileConnected { enabled } => {
            let saved = {
                let mut state = ctx.state.lock().unwrap();
                state.ble_scan_while_connected = enabled;
                std::env::var("LOCALAPPDATA")
                    .map_err(std::io::Error::other)
                    .and_then(|base| {
                        std::fs::write(
                            format!("{base}\\NTPods\\battery-scan-while-connected"),
                            enabled.to_string(),
                        )
                    })
            };
            if saved.is_err() {
                ctx.overlay("Could not save nearby battery updates for the next start.");
            }
            ctx.push_state();
        }
        Command::SetConversationVolume { percent } => {
            let saved = update_conversation_volume(&ctx.conv_duck, &ctx.state, percent, || {
                let base = std::env::var("LOCALAPPDATA").map_err(std::io::Error::other)?;
                std::fs::write(
                    format!("{base}\\NTPods\\conversation-volume"),
                    percent.to_string(),
                )
            });
            if saved.is_err() {
                ctx.overlay("Could not save conversation volume for the next start.");
            }
            ctx.push_state();
        }
        Command::StepVolume { delta } => {
            {
                let mut duck = ctx.conv_duck.lock().unwrap();
                duck.manual_override(volume::step(delta).as_deref());
            }
            ctx.sync_volume();
        }
        Command::SetVolume { percent } => {
            {
                let mut duck = ctx.conv_duck.lock().unwrap();
                duck.manual_override(volume::set(percent.min(100)).as_deref());
            }
            ctx.sync_volume();
        }
        Command::ToggleMute => {
            {
                let mut duck = ctx.conv_duck.lock().unwrap();
                duck.manual_override(volume::toggle_mute().as_deref());
            }
            ctx.sync_volume();
        }
        Command::SetHeartRate { on } => set_heart_rate(ctx, on),
        Command::SetHeadTracking { on } => {
            set_head_tracking(ctx, on);
        }
        Command::GetHearingAid => hearing_request(ctx, None),
        Command::SetHearingAid {
            request_id,
            on,
            expected,
            settings,
        } => {
            hearing_request(
                ctx,
                Some(HearingRequest {
                    request_id,
                    on,
                    expected,
                    settings,
                }),
            );
        }
        Command::Connect => {
            {
                let mut state = ctx.state.lock().unwrap();
                #[cfg(feature = "experimental-playback")]
                if playback_request.is_some_and(|request| bt::playback_intent() != request.0) {
                    return;
                }
                ctx.user_disconnected.store(false, Ordering::Release);
                ctx.connect_requested.store(true, Ordering::Release);
                state.mic_error = None;
                let mut pending = ctx.pending_audio_disconnect.lock().unwrap();
                let Some(token) = bt::begin_audio_request(true) else {
                    return;
                };
                reserve_native_playback_transition(&mut state);
                if let Some(token) = pending.queue_connect(token) {
                    dispatch_audio_request(ctx.mac(), token, "user");
                }
            }
            drop(reconnect_guard.take());
            if ctx.receiver_done.load(Ordering::Acquire) {
                finish_audio_disconnect(ctx);
            }
        }
        Command::RebuildAudio => {
            // Silent playback can occur with a healthy AAP session; request playback recovery.
            log("cmd: rebuild audio route (user)");
            ctx.overlay("Rebuilding the audio route…");
            let (mac, intent) = {
                let mut state = ctx.state.lock().unwrap();
                #[cfg(feature = "experimental-playback")]
                if playback_request.is_some_and(|request| bt::playback_intent() != request.0) {
                    return;
                }
                let Some(intent) = bt::begin_audio_request(true) else {
                    return;
                };
                reserve_native_playback_transition(&mut state);
                (ctx.mac(), intent)
            };
            thread::spawn(move || {
                let (disabled, enabled) =
                    bt::reset_audio_services_for_user_request(mac, false, intent);
                log(&format!(
                    "audio recovery: disable={disabled} enable={enabled}"
                ));
            });
        }
        Command::RepairConnection => {
            // Repeated handle reopens can leave the devnode in Code 38; debounce repairs.
            // The receiver reopens AAP and ATT after microphone cleanup.
            {
                const REPAIR_DEBOUNCE: Duration = Duration::from_secs(5);
                let mut last = ctx.last_repair.lock().unwrap();
                if last.is_some_and(|t| t.elapsed() < REPAIR_DEBOUNCE) {
                    log("cmd: repair connection — ignored (one is already in flight)");
                    return;
                }
                *last = Some(Instant::now());
            }
            log("cmd: repair connection — forcing a clean reconnect");
            {
                let mut state = ctx.state.lock().unwrap();
                #[cfg(feature = "experimental-playback")]
                if playback_request.is_some_and(|request| bt::playback_intent() != request.0) {
                    return;
                }
                ctx.user_disconnected.store(false, Ordering::Release);
                ctx.connect_requested.store(true, Ordering::Release);
                ctx.wants_reconnect.store(true, Ordering::Release);
                state.mic_error = None;
                // Repair is also an explicit reconnect and must supersede a
                // queued Disconnect without skipping its playback recovery.
                let mut pending = ctx.pending_audio_disconnect.lock().unwrap();
                let Some(token) = bt::begin_audio_request(true) else {
                    return;
                };
                reserve_native_playback_transition(&mut state);
                let _ = pending.queue_connect(token);
            }
        }
        Command::RefreshDeviceSettings => {
            let mut state = ctx.state.lock().unwrap();
            if !state.connected
                || !ctx.connect_requested.load(Ordering::Acquire)
                || ctx.user_disconnected.load(Ordering::Acquire)
                || ctx.wants_reconnect.load(Ordering::Acquire)
            {
                ctx.wants_settings_refresh.store(false, Ordering::Release);
                state.device_settings_error =
                    Some("Connect the AirPods before refreshing settings.".into());
            } else {
                queue_settings_refresh(
                    &ctx.wants_settings_refresh,
                    &mut state,
                    ctx.pipe.status(),
                    ctx.capture_active.load(Ordering::Acquire),
                );
            }
            drop(state);
            ctx.push_state();
        }
        Command::Disconnect => {
            cancel_microphone_request(ctx);
            hearing::invalidate_session();
            silence_mic(ctx);
            // Release AAP and request audio disconnect after cleanup; block automatic reconnect.
            {
                let _state = ctx.state.lock().unwrap();
                ctx.wants_settings_refresh.store(false, Ordering::Release);
                ctx.user_disconnected.store(true, Ordering::Release);
                ctx.connect_requested.store(false, Ordering::Release);
                if let Some(token) = bt::begin_audio_request(false) {
                    ctx.pending_audio_disconnect
                        .lock()
                        .unwrap()
                        .queue_disconnect(token);
                }
            }
            ctx.hr_db.end_session();
            {
                let mut state = ctx.state.lock().unwrap();
                state.connected = false;
                clear_heart_rate(&mut state);
            }
            *ctx.driver_cell.lock().unwrap() = None;
            ctx.push_state();
            ctx.overlay("Disconnected");
            if ctx.receiver_done.load(Ordering::Acquire) {
                finish_audio_disconnect(ctx);
            }
        }
        Command::SetName { name, request_id } => {
            let driver = ctx.driver_cell.lock().unwrap().clone();
            let result = send_name_request(&name, || {
                driver
                    .as_ref()
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::NotConnected, "AirPods are not connected")
                    })?
                    .send(&aap::build_rename(&name))
            });
            ctx.send_event(&Event::CommandResult {
                command: "set_name".into(),
                request_id,
                ok: result.is_ok(),
                message: match result {
                    Ok(()) => {
                        "Name sent to AirPods. Windows may show the old name until you reconnect."
                            .into()
                    }
                    Err(message) => message.into(),
                },
            });
        }
        Command::Shutdown => {
            hearing::invalidate_session();
            ctx.shutting_down.store(true, Ordering::Release);
            bt::begin_audio_shutdown();
            set_microphone_enabled(ctx, false, false, None);
            silence_mic(ctx);
            set_head_tracking(ctx, false);
            // Allows the stop drain and two audio-service operations, each of
            // which took about 15 seconds in hardware testing.
            let deadline = Instant::now() + Duration::from_secs(60);
            let mut audio_done = false;
            while Instant::now() < deadline {
                if ctx.receiver_done.load(Ordering::Acquire)
                    && ctx.playback_done.load(Ordering::Acquire)
                    && ctx.handsfree_done.load(Ordering::Acquire)
                {
                    audio_done = bt::finish_audio_shutdown();
                    if audio_done {
                        break;
                    }
                }
                thread::sleep(Duration::from_millis(50));
            }
            let receiver_done = ctx.receiver_done.load(Ordering::Acquire)
                && ctx.playback_done.load(Ordering::Acquire)
                && ctx.handsfree_done.load(Ordering::Acquire);
            if !receiver_done || !audio_done {
                log("shutdown: recovery deadline expired; closing process handles");
            }
            let playback_pending = bt::playback_restore_pending();
            if !matches!(playback_pending, Ok(false)) {
                log("shutdown: playback restoration is still pending");
            }
            std::process::exit(shutdown_exit_code(
                receiver_done,
                audio_done,
                ctx.shutdown_cleanup_failed.load(Ordering::Acquire),
                playback_pending,
            ));
        }
    }
}

/// Arrival timing of the 0x58 mic uplink, summarised to the log every 30 s.
#[derive(Default)]
struct MicStats {
    capture_start: Option<Instant>,
    first_packet_ms: Option<u128>,
    first_pcm_ms: Option<u128>,
    capture_packets: u64,
    invalid_packets: u64,
    capture_aus: u64,
    decoded_samples: u64,
    delivered_samples: u64,
    window_start: Option<Instant>,
    last: Option<Instant>,
    packets: u32,
    aus: u32,
    samples: usize,
    gaps_ms: Vec<f64>,
    last_timestamp: Option<u32>,
    timestamp_ticks: u64,
    timestamp_intervals: u32,
    timestamp_repeats: u32,
    timestamp_backwards: u32,
}

impl MicStats {
    fn access_unit(&mut self, timestamp: u32) {
        if let Some(previous) = self.last_timestamp.replace(timestamp) {
            let delta = timestamp.wrapping_sub(previous);
            // Measure modular deltas only; timestamp units and reset semantics
            // are unverified, so these counters never change decoded audio.
            match delta {
                0 => self.timestamp_repeats = self.timestamp_repeats.saturating_add(1),
                1..=0x7fff_ffff => {
                    self.timestamp_ticks = self.timestamp_ticks.saturating_add(u64::from(delta));
                    self.timestamp_intervals = self.timestamp_intervals.saturating_add(1);
                }
                _ => self.timestamp_backwards = self.timestamp_backwards.saturating_add(1),
            }
        }
    }
    fn begin(&mut self) {
        *self = Self::default();
        self.capture_start = Some(Instant::now());
    }
    fn start_sent(&self, sent: bool) {
        log(&format!(
            "mic capture: START_AUDIO sent={sent} send_ms={}",
            self.capture_start.map_or(0, |t| t.elapsed().as_millis())
        ));
    }
    fn pcm_delivered(&mut self, samples: usize) {
        if let Some(start) = self.capture_start {
            self.first_pcm_ms.get_or_insert(start.elapsed().as_millis());
        }
        self.delivered_samples = self.delivered_samples.saturating_add(samples as u64);
    }
    fn finish(&mut self, reason: &str) {
        if let Some(start) = self.capture_start.take() {
            log(&format!("mic capture: {reason} elapsed_ms={} first_packet_ms={:?} first_pcm_ms={:?} packets={} invalid_packets={} aus={} decoded_frames={} delivered_frames={}",
                start.elapsed().as_millis(), self.first_packet_ms, self.first_pcm_ms,
                self.capture_packets, self.invalid_packets, self.capture_aus,
                self.decoded_samples, self.delivered_samples));
        }
    }
    fn packet(&mut self, valid: bool, aus: u32, samples: usize) {
        let now = Instant::now();
        if let Some(start) = self.capture_start {
            self.first_packet_ms
                .get_or_insert(now.duration_since(start).as_millis());
        }
        self.capture_packets = self.capture_packets.saturating_add(1);
        self.invalid_packets = self.invalid_packets.saturating_add(u64::from(!valid));
        self.capture_aus = self.capture_aus.saturating_add(aus as u64);
        self.decoded_samples = self.decoded_samples.saturating_add(samples as u64);
        if let Some(prev) = self.last {
            self.gaps_ms
                .push(now.duration_since(prev).as_secs_f64() * 1000.0);
        }
        self.last = Some(now);
        let start = *self.window_start.get_or_insert(now);
        self.packets += 1;
        self.aus += aus;
        self.samples += samples;
        let secs = now.duration_since(start).as_secs_f64();
        if secs < 30.0 {
            return;
        }
        let mut g = std::mem::take(&mut self.gaps_ms);
        g.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let pct = |p: f64| {
            g.get(((g.len() as f64 - 1.0) * p) as usize)
                .copied()
                .unwrap_or(0.0)
        };
        let over = |ms: f64| g.iter().filter(|&&x| x > ms).count();
        log(&format!(
            "mic stats: {:.1}s pkts={} aus={} ({:.2}/pkt) rate={:.0} Hz gap ms p50={:.1} p95={:.1} p99={:.1} max={:.1} >64ms={} >100ms={}",
            secs,
            self.packets,
            self.aus,
            self.aus as f64 / self.packets.max(1) as f64,
            self.samples as f64 / secs,
            pct(0.5),
            pct(0.95),
            pct(0.99),
            g.last().copied().unwrap_or(0.0),
            over(64.0),
            over(100.0),
        ));
        log(&format!(
            "mic clock: forward_ticks={} intervals={} repeated={} backwards={}",
            self.timestamp_ticks,
            self.timestamp_intervals,
            self.timestamp_repeats,
            self.timestamp_backwards
        ));
        self.timestamp_ticks = 0;
        self.timestamp_intervals = 0;
        self.timestamp_repeats = 0;
        self.timestamp_backwards = 0;
        self.window_start = Some(now);
        self.packets = 0;
        self.aus = 0;
        self.samples = 0;
    }
}

/// Retry discovery until a device is found or shutdown is requested.
fn discover_until_ready<T>(
    mut stopped: impl FnMut() -> bool,
    mut discover: impl FnMut() -> Option<T>,
    mut wait: impl FnMut(),
) -> Option<T> {
    while !stopped() {
        if let Some(device) = discover() {
            return Some(device);
        }
        wait();
    }
    None
}

fn reset_pair_snapshot(state: &mut Snapshot, mac: u64, name: &str) {
    *state = Snapshot {
        device_id: format!("{mac:012x}"),
        dev_name: name.into(),
        auto_mode: state.auto_mode,
        auto_pause: state.auto_pause,
        conversation_volume: state.conversation_volume,
        ble_scan_while_connected: state.ble_scan_while_connected,
        mic_available: state.mic_available,
        mic_capture_active: state.mic_capture_active,
        mic_mode: state.mic_mode,
        mic_stop_delay_secs: state.mic_stop_delay_secs,
        volume: state.volume,
        muted: state.muted,
        playback_mode: state.playback_mode,
        playback_preferred_mode: state.playback_preferred_mode,
        playback_aac_bitrate: state.playback_aac_bitrate,
        playback_aac_eld_bitrate: state.playback_aac_eld_bitrate,
        playback_available: false,
        playback_state: if state.playback_mode == ipc::PlaybackMode::Windows {
            "windows"
        } else {
            "disconnected"
        }
        .into(),
        ..Snapshot::default()
    };
}

// Receiver only, after the previous channel and its microphone cleanup have closed.
fn select_paired_device(ctx: &Ctx, mac: u64, name: &str) -> bool {
    if mac == 0 || mac == ctx.mac() {
        return false;
    }
    let keys = battery_ble::Keys::load(mac);
    #[cfg(feature = "experimental-playback")]
    let native_bound = driver::native_audio_bound(mac);
    let connected = bt::is_connected(mac);
    let _write = ctx.settings_write.lock().unwrap();
    let _mic = ctx.mic_preference.lock().unwrap();
    let _hr = ctx.hr_commands.lock().unwrap();
    let mut pending = ctx.settings_pending.lock().unwrap();
    let mut state = ctx.state.lock().unwrap();
    if state.connected
        || ctx.driver_cell.lock().unwrap().is_some()
        || ctx.shutting_down.load(Ordering::Acquire)
        || ctx.user_disconnected.load(Ordering::Acquire)
        || ctx.pending_audio_disconnect.lock().unwrap().is_pending()
        || ctx.wants_reconnect.load(Ordering::Acquire)
    {
        return false;
    }
    // Invalidate queued audio work; this does not connect or reset a Windows service.
    let _ = bt::begin_audio_request(true);
    ctx.connect_requested.store(connected, Ordering::Release);
    ctx.hr_on.store(false, Ordering::Release);
    ctx.hr_generation.fetch_add(1, Ordering::AcqRel);
    ctx.hr_db.end_session();
    ctx.mic_demand.lock().unwrap().clear();
    ctx.pipe.revoke_stream();
    ctx.mic_on.store(false, Ordering::Release);
    hearing::invalidate_session();
    pending.reset(&mut state);
    reset_pair_snapshot(&mut state, mac, name);
    #[cfg(feature = "experimental-playback")]
    {
        state.playback_native_driver = native_bound;
        state.playback_available = native_bound;
    }
    *ctx.battery_cache.lock().unwrap() = battery_ble::Cache::default();
    *ctx.battery_keys.lock().unwrap() = keys;
    *ctx.dev_name.lock().unwrap() = name.into();
    ctx.mac.store(mac, Ordering::Release);
    ctx.wants_settings_refresh.store(false, Ordering::Release);
    drop(state);
    drop(pending);
    drop(_hr);
    drop(_mic);
    drop(_write);
    rename::apply(name);
    ctx.push_state();
    true
}

fn receiver_idle_wait(mic_on: bool, head_tracking_enabled: bool) -> Duration {
    Duration::from_millis(if mic_on || head_tracking_enabled {
        4
    } else {
        150
    })
}

fn run_receiver(ctx: Ctx) {
    log("run_receiver: entered");
    if ctx.mac() == 0 {
        let found = discover_until_ready(
            || ctx.shutting_down.load(Ordering::Acquire),
            bt::find_airpods,
            || thread::park_timeout(Duration::from_secs(2)),
        );
        let Some((mac, name)) = found else {
            ctx.receiver_done.store(true, Ordering::Release);
            return;
        };
        select_paired_device(&ctx, mac, &name);
    }
    let mut buf = [0u8; 8192];
    let clock = ctx.mic_clock;
    // Uplink timing, logged every 30 s while the mic is on: the virtual mic's ring
    // only holds ~100 ms, so arrival gaps longer than that come out as silence.
    let mut mic_stats = MicStats::default();
    // Audio recovery can reopen AAP; keep the both-out gate across those sessions.
    let mut mic_ears = mic_lifecycle::WearState::default();
    // RTBuddy heart-rate decoder (inert unless `hr_on`). Carry is reset per
    // connection so a partial frame never straddles a reconnect.
    let mut hr_decoder = hr::RtBuddyHeartRateDecoder::new();
    media::init(); // COM (MTA) for the SMTC ear-detection auto-pause
    volume::init(); // COM for the CA volume duck (same MTA)
    log("run_receiver: media init done");
    let mut last_anc = 0u8;
    // Match local radios against AAP 0x2E reports to identify this PC.
    let mut last_devices: Vec<(u64, [u8; 2])> = Vec::new();
    let local_radios = bt::local_radio_addresses();
    log(&format!(
        "bt: local radio(s): {}",
        local_radios
            .iter()
            .map(|a| format!("{a:012x}"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    // Bound connection retries, then require a fresh request or an existing Windows link.
    let mut reach_fails = 0u32;
    // Throttle checks for a restored Windows connection.
    let mut resume_poll = None;
    // Consecutive `Driver::open()` failures. Distinct from `reach_fails`: this
    // one is about the *devnode*, not the AirPods.
    let mut open_fails = 0u32;
    let give_up = |ctx: &Ctx, fails: &mut u32, request: u64| {
        *fails += 1;
        if *fails >= 12 {
            *fails = 0;
            if ctx.connect_requested.release(request) {
                log("run_receiver: gave up reaching AirPods — releasing");
            }
        }
    };
    let mut previous_request = None;
    let mut discovery_poll = None;
    loop {
        // No channel exists here, including before the first successful connect.
        finish_audio_disconnect(&ctx);
        if ctx.shutting_down.load(Ordering::Acquire) {
            ctx.receiver_done.store(true, Ordering::Release);
            return;
        }
        if ctx.user_disconnected.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(500));
            continue;
        }
        if cooldown_elapsed(discovery_poll, Instant::now(), Duration::from_secs(2)) {
            discovery_poll = Some(Instant::now());
            if let Some((next, name)) = bt::find_airpods_for(ctx.mac()) {
                if next != ctx.mac() && select_paired_device(&ctx, next, &name) {
                    mic_ears = mic_lifecycle::WearState::default();
                    hr_decoder.reset();
                    last_anc = 0;
                    last_devices.clear();
                    reach_fails = 0;
                    open_fails = 0;
                    resume_poll = None;
                    previous_request = None;
                }
            }
        }
        let mac = ctx.mac();
        // Reopen AAP when Windows reconnects, even if a phone call exhausted retries.
        // BLE presence alone cannot signal return while the buds keep advertising.
        // Explicit Disconnect blocks this until Connect or Repair.
        if !ctx.connect_requested.load(Ordering::Relaxed) {
            if !ctx.user_disconnected.load(Ordering::Relaxed)
                && cooldown_elapsed(resume_poll, Instant::now(), Duration::from_secs(5))
            {
                resume_poll = Some(Instant::now());
                if bt::is_connected(mac) {
                    let _state = ctx.state.lock().unwrap();
                    if claim_automatic_connection(
                        &ctx.connect_requested,
                        &ctx.user_disconnected,
                        &ctx.shutting_down,
                    ) {
                        log("run_receiver: Windows has the AirPods again — resuming the session");
                        continue;
                    }
                }
            }
            thread::sleep(Duration::from_millis(500));
            continue;
        }
        let connection_request = {
            let _state = ctx.state.lock().unwrap();
            if !ctx.connect_requested.load(Ordering::Acquire)
                || ctx.user_disconnected.load(Ordering::Acquire)
                || ctx.shutting_down.load(Ordering::Acquire)
            {
                continue;
            }
            let mut pending = ctx.pending_audio_disconnect.lock().unwrap();
            let Some(repair) = pending.take_repair_if_ready(&ctx.wants_reconnect) else {
                // A newer Connect/Repair arrived while the previous reset ran.
                // Consume that rebuild before opening AAP or reserving another token.
                if !pending.is_pending() {
                    // An automatic audio request can supersede a reset too.
                    // Preserve its connect intent while satisfying the saved repair.
                    if let Some(token) = bt::begin_audio_request(true) {
                        let _ = pending.queue_connect(token);
                    }
                }
                continue;
            };
            if repair && !bt::is_connected(mac) {
                audio_request(ctx.mac(), true, "repair");
            }
            ctx.connect_requested.snapshot()
        };
        if previous_request != Some(connection_request) {
            reach_fails = 0;
            previous_request = Some(connection_request);
        }
        let driver = match ctx.shared_driver.open(mac) {
            Ok(d) => {
                log("run_receiver: driver opened");
                open_fails = 0;
                d
            }
            Err(_) => {
                log("run_receiver: driver open FAILED");
                ctx.state.lock().unwrap().connected = false;
                *ctx.driver_cell.lock().unwrap() = None;
                ctx.push_state();
                // A disconnected device may have no devnode. With a Windows link,
                // request a rate-limited Code 38 check; the task leaves healthy nodes alone.
                open_fails += 1;
                if open_fails >= 4 && bt::is_connected(mac) && devnode::request_recovery(mac) {
                    log("run_receiver: driver wedged while the OS still has the AirPods — requested an elevated devnode restart");
                    ctx.overlay("Recovering the AirPods driver…");
                }
                give_up(&ctx, &mut reach_fails, connection_request);
                thread::sleep(Duration::from_millis(1500));
                continue;
            }
        };
        reset_device_settings(&ctx);
        *ctx.driver_cell.lock().unwrap() = Some(driver.clone());
        let connected = driver.connect(mac, aap::PSM_AACP).unwrap_or(false);
        log(&format!("run_receiver: connect({mac:#x}) = {connected}"));
        if !connected {
            *ctx.driver_cell.lock().unwrap() = None;
            let _ = driver.disconnect();
            ctx.state.lock().unwrap().connected = false;
            ctx.push_state();
            give_up(&ctx, &mut reach_fails, connection_request);
            thread::sleep(Duration::from_millis(1500));
            continue;
        }
        reach_fails = 0; // reached them — reset the give-up counter
        let _ = driver.send(&aap::HANDSHAKE);
        thread::sleep(Duration::from_millis(300));
        let _ = driver.send(&aap::SET_FEATURES);
        thread::sleep(Duration::from_millis(300));
        let _ = driver.send(&aap::REQUEST_NOTIFS);
        let _ = driver.send_sensor(&aap::request_all_descriptors(next_hr_seq(), false));
        let _ = driver.send(&battery_ble::REQUEST_KEYS);
        {
            let mut st = ctx.state.lock().unwrap();
            st.connected = true;
            st.link_stale = false;
            battery_ble::invalidate(&mut st);
            st.anc = 0;
        }
        let mut pending_card = true;
        ctx.push_state();
        log("run_receiver: handshake done, connected=true");
        // Opt-in GATT probe: wakes hearing assistance and blocks AAP while scanning.
        // Keep it out of normal operation; run at most once per daemon process.
        if std::env::var_os("NTPODS_GATT_PROBE").is_some() {
            static GATT_PROBED: AtomicBool = AtomicBool::new(false);
            if !GATT_PROBED.swap(true, Ordering::Relaxed) {
                for line in gatt::probe(&driver) {
                    log(&line);
                }
            }
        }
        let mut sensors = rtbuddy::Discovery::default();
        hr_decoder.set_report_service(None); // Device mappings belong to this connection.
                                             // Reuse bounded retries when heart-rate monitoring survives a reconnect.
        if ctx.hr_on.load(Ordering::Relaxed) {
            spawn_hr_retry(&ctx);
        }
        let mut decoder: Option<eld::Decoder> = None;
        let mut mic_announced = false;
        let mut mic = mic_lifecycle::Lifecycle::default();
        let mut mic_revision = ctx.pipe.permission_revision();
        silence_mic(&ctx);
        publish_mic(&ctx, &mic, mic_ears.allows_capture());

        let head_worker =
            HeadTrackingWorker::start(ctx.clone(), driver.clone(), mic_ears.allows_capture());

        let mut we_paused: Option<(u64, media::PausedSession)> = None;
        let mut prev_ear = [false; 2]; // last [primary, secondary] in-ear state
        let mut last_status = Instant::now();
        let mut heartbeat = Instant::now();
        let mut control_link = ControlLinkStatus::default();
        let mut low_warned = false; // low-battery overlay fired (hysteresis)
        let mut last_battery_report = None;
        let mut case_low_warned = false; // case low-battery overlay fired
                                         // Per-bud ear status, to notify on the transition into the case.
        let mut prev_status = [aap::EarStatus::Disconnected; 2];
        // HR diagnostics (logged every ~3s while monitoring).
        let mut hr_last_log = Instant::now();
        let (mut hr_bytes, mut hr_frames, mut hr_samples) = (0usize, 0u32, 0u32);
        // Count heart-rate signatures separately from motion on the same transport.
        let mut hr_type19 = 0u32;
        let mut hr_type14 = 0u32; // activity-classifier (svc 14) frames
                                  // Throttle diagnostics for non-connected driver states.
        let mut status_diag = Instant::now();
        // Active capture uses its own PCM deadline and recovery policy.
        let mut last_data = Instant::now();
        // Poll ATT (PSM 0x001F) diagnostics and log only changes.
        let mut att_poll = Instant::now();
        let mut last_att: (i32, u32, u32, i32, u32) = (0, 0, 0, 0, 0);
        loop {
            if ctx.shutting_down.load(Ordering::Acquire)
                || ctx.pending_audio_disconnect.lock().unwrap().is_pending()
            {
                break;
            }
            if take_settings_refresh(&ctx, &mic) {
                log("run_receiver: refreshing settings with an idle microphone");
                *ctx.driver_cell.lock().unwrap() = None;
                break;
            }
            head_worker
                .worn
                .store(mic_ears.allows_capture(), Ordering::Release);
            if service_mic(
                &ctx,
                &driver,
                &mut mic,
                clock.elapsed().as_millis() as u64,
                &mut mic_revision,
                &mut mic_stats,
                mic_ears.allows_capture(),
            ) {
                break;
            }
            if !mic.accepts_pcm() {
                decoder = None;
                mic_announced = false;
            }
            if att_poll.elapsed() >= Duration::from_millis(1500) {
                att_poll = Instant::now();
                if let Ok(d) = driver.att_diag() {
                    if d != last_att {
                        let was_open = last_att.4;
                        last_att = d;
                        let _ = was_open;
                        log(&format!(
                            "ATT: register=0x{:08X} registered={} indications={} accept=0x{:08X} channel_open={}",
                            d.0 as u32, d.1, d.2, d.3 as u32, d.4
                        ));
                    }
                }
            }
            // The user pressed Disconnect (connect_requested cleared) — release.
            if !ctx.connect_requested.load(Ordering::Relaxed) {
                log("run_receiver: disconnect requested — releasing");
                *ctx.driver_cell.lock().unwrap() = None;
                break;
            }
            // The user pressed "Repair connection": drop + reopen the driver for a
            // fresh AAP session + ATT channel. connect_requested stays set, so the
            // outer loop reconnects instead of releasing.
            if !mic.cleanup_pending() && ctx.wants_reconnect.load(Ordering::Acquire) {
                log("run_receiver: repair requested — reopening AACP channel");
                *ctx.driver_cell.lock().unwrap() = None;
                break;
            }
            let mut got_data = false;
            if let Ok(n) = driver.recv(100, &mut buf) {
                if n > 0 {
                    got_data = true;
                    last_data = Instant::now();
                    if control_link.stale {
                        control_link.stale = false;
                        ctx.state.lock().unwrap().link_stale = false;
                        ctx.push_state();
                        log("run_receiver: AAP channel talking again — link_stale cleared");
                    }
                    let data = &buf[..n];
                    if battery_ble::is_key_response(data) {
                        if let Some(keys) = battery_ble::parse_keys(mac, data) {
                            let mut current = ctx.battery_keys.lock().unwrap();
                            if current.as_ref() != Some(&keys) {
                                if keys.save().is_err() {
                                    log("battery: paired BLE keys available for this session; saving failed");
                                }
                                *current = Some(keys);
                            }
                        }
                        buf[..n].fill(0);
                        continue;
                    }
                    if aap::is_audio_packet(data) {
                        mic.audio_packet(clock.elapsed().as_millis() as u64);
                    }
                    // Hi-res mic: decode the 0x58 uplink AUs → feed the virtual mic.
                    if ctx.mic_on.load(Ordering::Relaxed) {
                        if aap::is_audio_packet(data) {
                            let prepared = (|| -> Result<bool, ()> {
                                let fresh = decoder.is_none();
                                if fresh {
                                    decoder = Some(eld::Decoder::new().ok_or(())?);
                                }
                                let interrupted = decoder
                                    .as_mut()
                                    .unwrap()
                                    .prepare_packet(clock.elapsed().as_millis() as u64)?;
                                if fresh || interrupted {
                                    ctx.pipe.flush()?;
                                    if interrupted {
                                        log("mic: packet gap; decoder and queued PCM reset");
                                    }
                                    ctx.pipe.write(&[0i16; 3840])?;
                                }
                                Ok(true)
                            })();
                            match prepared {
                                Ok(true) => {}
                                Ok(false) => {
                                    decoder = None;
                                    continue;
                                }
                                Err(()) => {
                                    decoder = None;
                                    fail_mic_audio(
                                        &ctx,
                                        &driver,
                                        &mut mic,
                                        clock.elapsed().as_millis() as u64,
                                        &mut mic_stats,
                                        "decoder-reset-failed",
                                    );
                                    continue;
                                }
                            }
                            let mut out: Vec<i16> = Vec::new();
                            let mut aus = 0u32;
                            let mut decode_failed = false;
                            let framed = aap::for_each_au(data, |timestamp, au| {
                                mic_stats.access_unit(timestamp);
                                aus += 1;
                                if !decode_failed {
                                    if let Some(dec) = decoder.as_mut() {
                                        match dec.decode(au) {
                                            Ok(samples) => out.extend_from_slice(samples),
                                            Err(()) => decode_failed = true,
                                        }
                                    }
                                }
                            });
                            let valid = framed && !decode_failed;
                            if !valid {
                                out.clear();
                                decoder = None;
                                if ctx.pipe.flush().is_err() {
                                    fail_mic_audio(
                                        &ctx,
                                        &driver,
                                        &mut mic,
                                        clock.elapsed().as_millis() as u64,
                                        &mut mic_stats,
                                        "invalid-packet-flush-failed",
                                    );
                                    continue;
                                }
                            }
                            mic_stats.packet(valid, aus, out.len());
                            if !out.is_empty() {
                                let delivered = match ctx.pipe.write(&out) {
                                    Ok(delivered) => delivered,
                                    Err(()) => {
                                        fail_mic_audio(
                                            &ctx,
                                            &driver,
                                            &mut mic,
                                            clock.elapsed().as_millis() as u64,
                                            &mut mic_stats,
                                            "output-failed",
                                        );
                                        continue;
                                    }
                                };
                                if delivered {
                                    mic_stats.pcm_delivered(out.len());
                                }
                                mic.pcm_received(clock.elapsed().as_millis() as u64);
                                publish_mic(&ctx, &mic, mic_ears.allows_capture());
                                // Decoded PCM confirms readiness even without a recording client.
                                let _preference = ctx.mic_preference.lock().unwrap();
                                if !mic_announced && ctx.mic_on.load(Ordering::Acquire) {
                                    mic_announced = true;
                                    ctx.overlay("Microphone ready — high quality");
                                }
                            }
                        }
                    } else if decoder.is_some() {
                        decoder = None;
                        mic_announced = false; // mic released — next session re-announces
                                               // poll_mic can close the output gate during recv. Keep
                                               // this capture's counters until service_mic logs STOP.
                    }
                    if !aap::is_microphone_message(data) {
                        let old_motion = sensors.motion;
                        let old_heart_rate = sensors.heart_rate;
                        let old_generation = sensors.generation;
                        if sensors.feed(data) {
                            log(&format!(
                                "sensors: descriptors generation={} motion={:?} heart_rate={:?}",
                                sensors.generation,
                                sensors.motion.map(|s| s.id),
                                sensors.heart_rate,
                            ));
                            let restarted = old_generation != sensors.generation;
                            if restarted || old_motion != sensors.motion {
                                head_worker.motion_refresh.publish(sensors.motion);
                            }
                            if restarted || old_heart_rate != sensors.heart_rate {
                                hr_decoder.set_report_service(sensors.heart_rate);
                                if !ctx.hr_retrying.load(Ordering::Acquire) {
                                    head_worker.heart_rate_refresh.store(
                                        ctx.hr_generation.load(Ordering::Acquire).wrapping_add(1),
                                        Ordering::Release,
                                    );
                                }
                            }
                        }
                        if !head_worker.motion_refresh.is_pending() {
                            receive_head_tracking(&ctx, data);
                        }
                        // Heart rate (opt-in): feed each chunk into the RTBuddy
                        // decoder; publish the latest validated BPM. Inert when off.
                        if ctx.hr_on.load(Ordering::Relaxed) {
                            hr_bytes += data.len();
                            if hr::contains_frame_prefix(data) {
                                hr_frames += 1;
                                ctx.hr_stream_live.store(true, Ordering::Relaxed);
                                // Count activity-classifier (type 14: `08 0e 1a`) frames
                                // too — diagnostic only; they don't block heart rate.
                                if data.windows(3).any(|w| w == [0x08, 0x0e, 0x1a]) {
                                    hr_type14 += 1;
                                }
                                // Count HR records without logging sensor payloads.
                                if data.windows(4).any(|w| w == [0x08, 0x13, 0x1a, 0x12]) {
                                    hr_type19 += 1;
                                }
                            }
                            let samples = hr_decoder.feed(data);
                            hr_samples += samples.len() as u32;
                            if !samples.is_empty() {
                                // Rendezvous with the retry thread: the stream is live.
                                ctx.hr_got_sample.store(true, Ordering::Relaxed);
                            }
                            if let Some(sample) = samples.last() {
                                let mut state = ctx.state.lock().unwrap();
                                if ctx.hr_on.load(Ordering::Acquire) {
                                    for sample in &samples {
                                        ctx.hr_db.sample(ctx.mac(), *sample);
                                    }
                                    record_heart_rate(&mut state, sample.bpm, unix_millis());
                                    drop(state);
                                    ctx.push_state();
                                }
                            }
                            if hr_last_log.elapsed() >= Duration::from_secs(3) {
                                log(&format!(
                                "HR diag: bytes={hr_bytes} prefix={hr_frames} type14={hr_type14} type19={hr_type19} bpm_samples={hr_samples}"
                            ));
                                hr_last_log = Instant::now();
                                hr_bytes = 0;
                                hr_frames = 0;
                                hr_type19 = 0;
                                hr_type14 = 0;
                                hr_samples = 0;
                            }
                        } else {
                            let changed = clear_heart_rate(&mut ctx.state.lock().unwrap());
                            if changed {
                                ctx.push_state();
                            }
                        }
                        // AAP reports other hosts even while Windows still shows connected.
                        if let Some(devs) = aap::parse_connected_devices(data) {
                            if devs != last_devices {
                                let list: Vec<ipc::ConnectedDevice> = devs
                                    .iter()
                                    .map(|(a, flags)| describe_device(*a, *flags, &local_radios))
                                    .collect();
                                log(&format!(
                                    "multipoint: {} device(s) on the AirPods: {}",
                                    list.len(),
                                    list.iter()
                                        .zip(devs.iter())
                                        .map(|(d, (_, f))| {
                                            format!(
                                                "{} [{}] {} flags={}",
                                                d.address,
                                                d.kind,
                                                d.name,
                                                aap::flags_hex(*f)
                                            )
                                        })
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                ));
                                ctx.state.lock().unwrap().multipoint = list;
                                ctx.push_state();
                                last_devices = devs;
                            }
                        }
                        if let Some((op, dump)) = aap::route_diag(data) {
                            log(&format!(
                                "route: 0x{op:02X} {} ({} bytes): {dump}",
                                aap::route_opcode_name(op),
                                data.len()
                            ));
                        }
                        if let Some(b) = aap::parse_battery(data) {
                            last_battery_report = Some((clock.elapsed().as_millis() as u64, b));
                            let batt_text = {
                                let mut s = ctx.state.lock().unwrap();
                                b.apply_to_state(&mut s, unix_millis());
                                battery_text(&s.battery, s.connected)
                            };
                            ctx.push_state();
                            if pending_card {
                                ctx.overlay(&format!("Connected  ·  {batt_text}"));
                                pending_card = false;
                            }
                            // Low-battery notification: warn once when either bud
                            // falls to <=20%, re-arm only after it recovers above 25%
                            // (hysteresis so it doesn't spam around the threshold).
                            let low = {
                                let s = ctx.state.lock().unwrap();
                                let ages = s.battery_freshness.unwrap_or_default();
                                [
                                    s.battery.left.filter(|_| ages.left.live),
                                    s.battery.right.filter(|_| ages.right.live),
                                ]
                                .into_iter()
                                .flatten()
                                .min()
                            };
                            if let Some(min) = low {
                                if min <= 20 && !low_warned {
                                    ctx.overlay(&format!("Battery low — {min}%"));
                                    low_warned = true;
                                } else if min > 25 {
                                    low_warned = false;
                                }
                            }
                            // Case low battery (separate, quieter threshold).
                            if let Some(cl) = {
                                let s = ctx.state.lock().unwrap();
                                s.battery.case.filter(|_| {
                                    s.battery_freshness.is_some_and(|ages| ages.case.live)
                                })
                            } {
                                if cl <= 15 && !case_low_warned {
                                    ctx.overlay(&format!("Case battery low — {cl}%"));
                                    case_low_warned = true;
                                } else if cl > 20 {
                                    case_low_warned = false;
                                }
                            }
                        }
                        if hearing::observe_packet(data) {
                            ctx.push_state();
                        }
                        if let Some((id, value)) = aap::parse_device_control(data) {
                            let changed = {
                                let mut pending = ctx.settings_pending.lock().unwrap();
                                let mut state = ctx.state.lock().unwrap();
                                let changed = pending.confirm_control(&mut state, id, value);
                                if id == 0x34
                                    && device_settings::valid_control(id, value)
                                    && device_settings::supports_silent_allow_off(&state.model)
                                    && device_settings::save_allow_off_choice(ctx.mac(), value == 1)
                                        .is_err()
                                {
                                    log("Could not save the reported Allow Off preference");
                                }
                                changed
                            };
                            if changed {
                                ctx.push_state();
                            }
                            if id == 0x0D {
                                let mode = value as u8;
                                if last_anc != 0 && mode != last_anc {
                                    ctx.overlay(aap::anc_name(mode));
                                }
                                last_anc = mode;
                            }
                        }
                        if let Some(eq) = aap::parse_eq(data) {
                            let changed = {
                                let mut pending = ctx.settings_pending.lock().unwrap();
                                pending.confirm_eq(&mut ctx.state.lock().unwrap(), eq)
                            };
                            if changed {
                                ctx.push_state();
                            }
                        }
                        if let Some(status) = aap::parse_conversational_awareness(data) {
                            // Preserve call volume during capture and undo any active duck.
                            if ctx.mic_on.load(Ordering::Acquire)
                                && ctx.capture_active.load(Ordering::Acquire)
                            {
                                ctx.conv_duck.lock().unwrap().restore();
                            } else {
                                ctx.conv_duck.lock().unwrap().on_status(status);
                            }
                        }
                        if let Some((model, firmware, serial)) = aap::parse_metadata(data) {
                            let changed = {
                                let _pending = ctx.settings_pending.lock().unwrap();
                                let mut s = ctx.state.lock().unwrap();
                                let c = s.model != model
                                    || s.firmware != firmware
                                    || s.serial != serial;
                                if c {
                                    s.model = model.clone();
                                    s.firmware = firmware;
                                    s.serial = serial;
                                    if device_settings::supports_silent_allow_off(&model)
                                        && !s.device_controls_pending.contains_key(&0x34)
                                        && s.allow_off_requested.is_none()
                                    {
                                        if let Some(&value) = s.device_controls.get(&0x34) {
                                            // A report can arrive before the model identity.
                                            if device_settings::save_allow_off_choice(
                                                ctx.mac(),
                                                value == 1,
                                            )
                                            .is_err()
                                            {
                                                log("Could not save the reported Allow Off preference");
                                            }
                                        } else {
                                            // Restore the choice without claiming a report or resending it.
                                            s.allow_off_requested =
                                                device_settings::load_allow_off_choice(ctx.mac());
                                        }
                                    }
                                }
                                c
                            };
                            if changed {
                                log(&format!("device metadata: model={model}"));
                                ctx.push_state();
                                if ctx.hr_on.load(Ordering::Acquire)
                                    && device_models::heart_rate(&ctx.state.lock().unwrap())
                                {
                                    spawn_hr_retry(&ctx);
                                }
                            }
                        }
                        if let Some((primary, secondary)) = aap::parse_ear_detection(data) {
                            // AAP ear detection reports both buds, so case notifications need no BLE scan.
                            let now = [primary, secondary];
                            for i in 0..2 {
                                if now[i] == aap::EarStatus::InCase
                                    && prev_status[i] != aap::EarStatus::InCase
                                {
                                    ctx.overlay("AirPod in case");
                                    break; // one overlay even if both go in at once
                                }
                            }
                            prev_status = now;
                            let new_ear = aap::in_ear_state(prev_ear, [primary, secondary]);
                            mic_ears.observe(
                                [primary, secondary]
                                    .map(|ear| (!ear.is_transitional()).then_some(ear.in_ear())),
                            );
                            if !mic_ears.allows_capture() {
                                // Removal clears manual/idle holds before recovery.
                                let _preference = ctx.mic_preference.lock().unwrap();
                                ctx.mic_demand.lock().unwrap().clear();
                                silence_mic(&ctx);
                            }
                            if new_ear != prev_ear {
                                // Ear handoff can restart the encoder during one Windows capture.
                                // Drop both AAC and resampler history before any resumed audio.
                                decoder = None;
                                if ctx.mic_on.load(Ordering::Acquire) {
                                    if ctx.pipe.flush().is_err() {
                                        fail_mic_audio(
                                            &ctx,
                                            &driver,
                                            &mut mic,
                                            clock.elapsed().as_millis() as u64,
                                            &mut mic_stats,
                                            "ear-change-flush-failed",
                                        );
                                    }
                                    log("mic: ear state changed; decoder and queued PCM reset");
                                }
                                let all_in = new_ear[0] && new_ear[1];
                                let was_wearing = prev_ear[0] || prev_ear[1];
                                // Serialize with disabling, so no resume can run after it returns.
                                let policy = ctx.auto_pause.lock().unwrap();
                                if let Some(permission) = policy.permission() {
                                    if all_in {
                                        if let Some((recorded, paused)) = we_paused.take() {
                                            if recorded == permission {
                                                paused.resume();
                                            }
                                        }
                                    } else if was_wearing {
                                        if let Some(paused) = media::pause_playing_session() {
                                            we_paused = Some((permission, paused));
                                        }
                                    }
                                } else {
                                    we_paused = None;
                                }
                                prev_ear = new_ear;
                            }
                        }
                    }
                }
            }
            if !got_data {
                // The idle delay would otherwise collect 50 Hz motion in bursts.
                let mic_on = ctx.mic_on.load(Ordering::Relaxed);
                let head_tracking_enabled =
                    !mic_on && ctx.state.lock().unwrap().head_tracking_enabled;
                // Capture changes wake this wait without shortening recovery.
                thread::park_timeout(receiver_idle_wait(mic_on, head_tracking_enabled));
            }
            // Reset decoder history without STOP/START, which can reconnect weak links.
            // Normal capture stop still uses service_mic.
            if !ctx.mic_on.load(Ordering::Relaxed) {
                decoder = None;
                mic_announced = false;
            }
            // Record quiet sessions without treating silence as a failed link.
            if heartbeat.elapsed() >= Duration::from_secs(60) {
                heartbeat = Instant::now();
                log(&format!(
                    "heartbeat: aap_data_age={}ms status={:?} mic={}",
                    last_data.elapsed().as_millis(),
                    driver.status(),
                    ctx.mic_on.load(Ordering::Relaxed)
                ));
            }
            if last_status.elapsed() >= Duration::from_secs(1) {
                last_status = Instant::now();
                let st = driver.status();
                if !matches!(st, Ok(2)) && status_diag.elapsed() >= Duration::from_secs(2) {
                    let both_out = !prev_ear[0] && !prev_ear[1];
                    log(&format!(
                        "status diag: st={st:?} both_out={both_out} fails={} data_age={}ms",
                        control_link.disconnected_checks,
                        last_data.elapsed().as_millis()
                    ));
                    status_diag = Instant::now();
                }
                let driver_connected = matches!(st, Ok(2));
                let was_stale = control_link.stale;
                // Preserve the session through transient AAP failures while Windows
                // still has the device; confirmed OS disconnects release it below.
                let release = control_link.observe(
                    driver_connected,
                    driver_connected || bt::is_connected(mac),
                    ctx.mic_on.load(Ordering::Relaxed),
                    last_data.elapsed(),
                );
                if control_link.stale != was_stale {
                    ctx.state.lock().unwrap().link_stale = control_link.stale;
                    ctx.push_state();
                    log(if control_link.stale {
                        "run_receiver: AAP transport unavailable with mic idle — link_stale set"
                    } else {
                        "run_receiver: AAP transport recovered — link_stale cleared"
                    });
                }
                if release {
                    // Local capture or playback permits a bounded wait for the connection to return.
                    let tier = host_tier(&ctx);
                    let cased =
                        recently_cased(last_battery_report, clock.elapsed().as_millis() as u64);
                    let cooling = ctx
                        .last_reclaim
                        .lock()
                        .unwrap()
                        .is_some_and(|t| t.elapsed() < RECLAIM_COOLDOWN);
                    log(&format!(
                        "run_receiver: AirPods no longer connected (OS) — releasing (host tier {}{})",
                        tier.label(),
                        if cooling { ", reclaim cooling down" } else { "" }
                    ));
                    ctx.overlay("Disconnected");
                    ctx.hr_db.end_session();
                    {
                        let mut s = ctx.state.lock().unwrap();
                        s.connected = false;
                        clear_heart_rate(&mut s);
                    }
                    *ctx.driver_cell.lock().unwrap() = None;
                    hr_decoder.reset(); // drop HR carry across the reconnect
                    last_anc = 0;
                    if tier.defends_audio() && !cooling && !cased {
                        // Keep AAP requested during recovery; the worker clears it on timeout.
                        spawn_reclaim(&ctx, tier, connection_request);
                    } else {
                        // A charging case disconnect is deliberate, even if a
                        // player keeps running on the PC. Leave the buds asleep.
                        ctx.connect_requested.release(connection_request);
                    }
                    ctx.push_state();
                    break;
                }
            }
        }
        head_worker.stop();
        ctx.conv_duck.lock().unwrap().restore();
        silence_mic(&ctx);
        if ctx.shutting_down.load(Ordering::Acquire) && mic.cleanup_pending() {
            // Quitting must restore playback too. Drain packets while the same
            // stop state machine performs its one reset and optional reconnect.
            // Do not decode, publish payloads or run unrelated session logic.
            let deadline = Instant::now() + Duration::from_secs(45);
            while mic.cleanup_pending() && Instant::now() < deadline {
                // Explicit disconnect closes both audio profiles; it does not
                // need to restore stereo through a disable/enable transaction.
                if ctx.pending_audio_disconnect.lock().unwrap().is_pending() {
                    break;
                }
                if service_mic(
                    &ctx,
                    &driver,
                    &mut mic,
                    clock.elapsed().as_millis() as u64,
                    &mut mic_revision,
                    &mut mic_stats,
                    mic_ears.allows_capture(),
                ) {
                    break;
                }
                match driver.recv(100, &mut buf) {
                    Ok(n) if aap::is_audio_packet(&buf[..n]) => {
                        mic.audio_packet(clock.elapsed().as_millis() as u64)
                    }
                    Err(_) => thread::sleep(Duration::from_millis(50)),
                    _ => {}
                }
            }
        }
        let explicit_disconnect = ctx.pending_audio_disconnect.lock().unwrap().is_pending();
        let unfinished_mic = mic.cleanup_pending() && !explicit_disconnect;
        if unfinished_mic {
            set_microphone_enabled(&ctx, false, true, None);
            {
                let mut state = ctx.state.lock().unwrap();
                ctx.user_disconnected.store(true, Ordering::Release);
                ctx.connect_requested.store(false, Ordering::Release);
                ctx.wants_reconnect.store(false, Ordering::Release);
                if mic.phase != mic_lifecycle::Phase::Error || state.mic_error.is_none() {
                    state.mic_error = Some("Microphone cleanup did not finish. Reconnect in NTPods before enabling the microphone again.".into());
                }
            }
            mic.phase = mic_lifecycle::Phase::Error;
            log("mic: session closed before microphone cleanup finished; explicit retry required");
        }
        let motion_refresh = head_worker.join();
        close_head_tracking(&ctx, &driver, &motion_refresh);
        cancel_microphone_request(&ctx);
        mic_stats.finish("session-closed");
        #[cfg(feature = "experimental-playback")]
        playback::stop_buffer(&ctx);
        let (stop_result, close_result, rebuild_failed) = close_control_session(
            mic.uplink_active,
            || driver.send(&aap::STOP_AUDIO),
            || driver.disconnect(),
            || finish_audio_disconnect(&ctx),
        );
        if explicit_disconnect {
            if close_result.is_ok() && !rebuild_failed {
                mic.uplink_active = false;
                mic.phase = mic_lifecycle::Phase::Idle;
            } else {
                mic.phase = mic_lifecycle::Phase::Error;
                set_microphone_enabled(&ctx, false, true, None);
            }
        }
        if stop_result.is_err() || close_result.is_err() {
            log("mic: stop or channel close failed during session cleanup");
        }
        if ctx.shutting_down.load(Ordering::Acquire)
            && (unfinished_mic || stop_result.is_err() || close_result.is_err() || rebuild_failed)
        {
            ctx.shutdown_cleanup_failed.store(true, Ordering::Release);
        }
        *ctx.driver_cell.lock().unwrap() = None;
        reset_device_settings(&ctx);
        {
            let mut s = ctx.state.lock().unwrap();
            if close_result.is_ok() {
                s.playback_buffer_target_ms = None;
            }
            ctx.wants_settings_refresh.store(false, Ordering::Release);
            s.connected = false;
            clear_heart_rate(&mut s);
            s.mic_uplink_active = mic.uplink_active && close_result.is_err();
            s.mic_recording = false;
            if close_result.is_err() {
                s.mic_state = "error".into();
                s.mic_error = Some("Could not confirm channel closure. Microphone output is silent; reconnect the AirPods before trying again.".into());
            } else if mic.phase == mic_lifecycle::Phase::Error
                && ctx.user_disconnected.load(Ordering::Acquire)
            {
                s.mic_state = "error".into();
            } else {
                s.mic_state = "idle".into();
            }
        }
        ctx.push_state();
        thread::sleep(Duration::from_millis(250));
    }
}

/// Track real Windows capture clients independently of the selected uplink policy.
fn poll_mic(ctx: Ctx, receiver: Option<thread::Thread>) {
    loop {
        if ctx.shutting_down.load(Ordering::Acquire) {
            return;
        }
        let settings_expired = {
            let mut pending = ctx.settings_pending.lock().unwrap();
            pending.expire(&mut ctx.state.lock().unwrap(), Instant::now())
        };
        if settings_expired {
            ctx.push_state();
        }
        let hr_expired = expire_heart_rate(&mut ctx.state.lock().unwrap(), unix_millis());
        if hr_expired {
            ctx.push_state();
        }
        let battery_expired =
            battery_ble::refresh_live(&mut ctx.state.lock().unwrap(), unix_millis());
        if battery_expired {
            ctx.push_state();
        }
        let status = ctx.pipe.status();
        let active = status.is_some_and(|streams| streams > 0);
        let availability_changed = ctx.state.lock().unwrap().mic_available != status.is_some();
        let capture_changed = {
            let _preference = ctx.mic_preference.lock().unwrap();
            ctx.pipe.set_capture_active(active);
            let changed = ctx.capture_active.swap(active, Ordering::AcqRel) != active;
            let permitted = microphone_permitted(&ctx);
            let mut demand = ctx.mic_demand.lock().unwrap();
            if permitted {
                demand.observe_capture(ctx.mic_clock.elapsed().as_millis() as u64, active);
            } else {
                demand.clear();
            }
            let stop_immediately = changed
                && !active
                && demand.mode == ntpods_ipc::MicMode::Automatic
                && demand.stop_delay_secs == 0;
            if status.is_none() || stop_immediately {
                if status.is_none() {
                    demand.clear();
                }
                ctx.mic_on.store(false, Ordering::Release);
                ctx.pipe.revoke_stream();
            }
            changed
        };
        if capture_changed {
            if active && ctx.mic_on.load(Ordering::Acquire) {
                ctx.conv_duck.lock().unwrap().restore();
            }
            log(&format!("capture demand: active={active}"));
            if let Some(receiver) = &receiver {
                receiver.unpark();
            }
        }
        if capture_changed || availability_changed {
            ctx.push_state();
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn main() {
    if let Some(code) = handsfree::run_requested() {
        std::process::exit(code);
    }
    if let Some(code) = helper_launch::run_requested() {
        std::process::exit(code);
    }

    // Single instance: never run two daemons over the one exclusive driver.
    unsafe {
        let name = wide("Local\\NTPodsDaemonSingleton");
        let _ = CreateMutexW(ptr::null(), 0, name.as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
    }

    // Create per-user storage before logging; the installer only creates machine-wide paths.
    if let Ok(la) = std::env::var("LOCALAPPDATA") {
        let _ = std::fs::create_dir_all(format!("{la}\\NTPods"));
    }
    log("=== ntpodsd start ===");
    if let Err(error) = bt::restore_experimental_audio() {
        log(&format!("playback restoration: {error}"));
    }
    let (mac, dev_name) = match bt::find_airpods() {
        Some((m, n)) => (m, n),
        None => (0, "AirPods".to_string()),
    };
    log(&format!("find_airpods: mac={mac:#x} name='{dev_name}'"));

    let microphone_enabled = std::env::var("LOCALAPPDATA")
        .ok()
        .and_then(|p| std::fs::read_to_string(format!("{p}\\NTPods\\microphone-enabled")).ok())
        .is_some_and(|s| s.trim() == "true");
    let mic_demand = std::env::var("LOCALAPPDATA")
        .ok()
        .and_then(|p| std::fs::read_to_string(format!("{p}\\NTPods\\microphone-policy.json")).ok())
        .and_then(|text| mic_lifecycle::Demand::from_preferences_json(&text))
        .unwrap_or_default();
    let auto_pause = std::env::var("LOCALAPPDATA")
        .ok()
        .and_then(|p| std::fs::read_to_string(format!("{p}\\NTPods\\auto-pause")).ok())
        .is_none_or(|s| s.trim() != "false");
    let conversation_volume = std::env::var("LOCALAPPDATA")
        .ok()
        .and_then(|p| std::fs::read_to_string(format!("{p}\\NTPods\\conversation-volume")).ok())
        .and_then(|value| value.trim().parse::<u8>().ok())
        .filter(|value| *value <= 100)
        .unwrap_or(25);
    let ble_scan_while_connected = std::env::var("LOCALAPPDATA")
        .ok()
        .and_then(|p| {
            std::fs::read_to_string(format!("{p}\\NTPods\\battery-scan-while-connected")).ok()
        })
        .is_some_and(|value| value.trim() == "true");
    let pipe = Arc::new(micpipe::MicPipeCell::new());
    #[cfg(feature = "experimental-playback")]
    let playback_mode = playback::load_selection();
    #[cfg(not(feature = "experimental-playback"))]
    let playback_mode = ipc::PlaybackMode::Windows;
    pipe.set_feature_enabled(microphone_enabled);
    log(&format!("mic pipe opened: {}", pipe.is_open()));
    log(&format!("mic driver: {}", micpipe::driver_kind()));
    let ctx = Ctx {
        battery_keys: Arc::new(Mutex::new(battery_ble::Keys::load(mac))),
        battery_cache: Arc::new(Mutex::new(battery_ble::Cache::default())),
        state: Arc::new(Mutex::new(Snapshot {
            dev_name: dev_name.clone(),
            device_id: if mac == 0 {
                String::new()
            } else {
                format!("{mac:012x}")
            },
            auto_mode: microphone_enabled,
            auto_pause,
            conversation_volume,
            ble_scan_while_connected,
            mic_available: pipe.is_open(),
            mic_state: "idle".into(),
            playback_mode,
            playback_preferred_mode: Some(playback_mode),
            playback_available: false,
            playback_state: if playback_mode == ipc::PlaybackMode::Windows {
                "windows"
            } else {
                "disconnected"
            }
            .into(),
            ..Default::default()
        })),
        clients: Arc::new(Mutex::new(Vec::new())),
        settings_pending: Arc::new(Mutex::new(device_settings::PendingSettings::default())),
        settings_write: Arc::new(Mutex::new(())),
        #[cfg(feature = "experimental-playback")]
        playback_buffer: Arc::new(Mutex::new(None)),
        #[cfg(feature = "experimental-playback")]
        audio_change_busy: Arc::new(AtomicBool::new(false)),
        mic_preference: Arc::new(Mutex::new(())),
        #[cfg(feature = "experimental-playback")]
        playback_selection: Arc::new(Mutex::new(playback::Selection::new(playback_mode))),
        mic_demand: Arc::new(Mutex::new(mic_demand)),
        mic_clock: Instant::now(),
        hr_commands: Arc::new(Mutex::new(())),
        hr_generation: Arc::new(AtomicU64::new(0)),
        head_tracking: Arc::new(Mutex::new(head_tracking::Tracker::default())),
        auto_pause: Arc::new(Mutex::new(media::AutoPausePolicy::new(auto_pause))),
        driver_cell: Arc::new(Mutex::new(None)),
        shared_driver: Arc::new(driver::Shared::default()),
        hr_db: hrdb::HrDb::open(log),
        mic_on: Arc::new(AtomicBool::new(false)),
        capture_active: Arc::new(AtomicBool::new(false)),
        shutting_down: Arc::new(AtomicBool::new(false)),
        receiver_done: Arc::new(AtomicBool::new(false)),
        playback_done: Arc::new(AtomicBool::new(!cfg!(feature = "experimental-playback"))),
        handsfree_done: Arc::new(AtomicBool::new(false)),
        shutdown_cleanup_failed: Arc::new(AtomicBool::new(false)),
        auto_mode: Arc::new(AtomicBool::new(microphone_enabled)),
        hr_on: Arc::new(AtomicBool::new(false)),
        hr_got_sample: Arc::new(AtomicBool::new(false)),
        hr_stream_live: Arc::new(AtomicBool::new(false)),
        hr_retrying: Arc::new(AtomicBool::new(false)),
        connect_requested: Arc::new(ConnectionRequest::new(false)),
        wants_reconnect: Arc::new(AtomicBool::new(false)),
        wants_settings_refresh: Arc::new(AtomicBool::new(false)),
        pipe,
        conv_duck: Arc::new(Mutex::new(volume::ConvDuck::new(conversation_volume))),
        reclaiming: Arc::new(AtomicBool::new(false)),
        user_disconnected: Arc::new(AtomicBool::new(false)),
        pending_audio_disconnect: Arc::new(Mutex::new(PendingAudioDisconnect::default())),
        last_reclaim: Arc::new(Mutex::new(None)),
        last_repair: Arc::new(Mutex::new(None)),
        dev_name: Arc::new(Mutex::new(dev_name.clone())),
        mac: Arc::new(AtomicU64::new(mac)),
    };

    // Name the virtual mic after the connected device (elevated task, no UAC).
    if mac != 0 {
        rename::apply(&dev_name);
    }

    // IPC: two one-directional pipe servers (events out, commands in).
    {
        let c = ctx.clone();
        thread::spawn(move || unsafe { ipc_server::events_server(c) });
    }
    {
        let c = ctx.clone();
        thread::spawn(move || unsafe { ipc_server::cmds_server(c) });
    }
    // Idle BLE reports are resolved to our paired AirPods before use. Keeping
    // the audio connection separate lets a closed case report without waking it.
    {
        let c = ctx.clone();
        let c_scan = ctx.clone();
        let prox = Arc::new(Mutex::new((mac, battery_ble::Visit::default())));
        thread::spawn(move || {
            let last_published = Mutex::new(0u64);
            le::watch_nearby(
                move |address, data| {
                    if !c.battery_scan_allowed() {
                        return;
                    }
                    let paired_mac = c.mac();
                    let report = c
                        .battery_keys
                        .lock()
                        .unwrap()
                        .as_ref()
                        .and_then(|keys| keys.decode(paired_mac, address, data));
                    let Some(report) = report else {
                        return;
                    };
                    let now_ms = unix_millis();
                    let mut published = last_published.lock().unwrap();
                    let (connected, restored, changed) = {
                        let mut state = c.state.lock().unwrap();
                        if c.mac() != paired_mac
                            || !battery_ble::scan_allowed(
                                state.ble_scan_while_connected,
                                state.connected,
                                state.mic_capture_active
                                    || c.capture_active.load(Ordering::Acquire),
                                state.mic_uplink_active || c.mic_on.load(Ordering::Acquire),
                            )
                        {
                            return;
                        }
                        let visible_battery = |s: &ipc::Snapshot| {
                            (
                                s.battery,
                                s.battery_live,
                                s.battery_source == "ble",
                                s.battery_freshness.map(|f| {
                                    (f.left.live, f.right.live, f.case.live, f.headphone.live)
                                }),
                            )
                        };
                        let before = visible_battery(&state);
                        let mut cache = c.battery_cache.lock().unwrap();
                        cache.observe(report.battery, now_ms);
                        let restored = cache.restore(&mut state, now_ms);
                        let changed = before != visible_battery(&state);
                        (state.connected, restored, changed)
                    };
                    if restored && (changed || now_ms.saturating_sub(*published) >= 1000) {
                        *published = now_ms;
                        c.push_state();
                    }
                    // Battery advertisements alone never page a sleeping case.
                    let mut visit_state = prox.lock().unwrap();
                    if visit_state.0 != paired_mac {
                        *visit_state = (paired_mac, battery_ble::Visit::default());
                    }
                    let visit = &mut visit_state.1;
                    if connected {
                        visit.observe_connected(&report, c.mic_clock.elapsed().as_millis() as u64);
                        return;
                    }
                    if !visit.observe(&report, c.mic_clock.elapsed().as_millis() as u64) {
                        return;
                    }
                    let state = c.state.lock().unwrap();
                    if c.mac() != paired_mac
                        || state.connected
                        || !claim_automatic_connection(
                            &c.connect_requested,
                            &c.user_disconnected,
                            &c.shutting_down,
                        )
                    {
                        return;
                    }
                    visit.mark_acted();
                    drop(visit_state);
                    let mut pending = c.pending_audio_disconnect.lock().unwrap();
                    let Some(token) = bt::begin_audio_request(true) else {
                        return;
                    };
                    if let Some(token) = pending.queue_connect(token) {
                        dispatch_audio_request(paired_mac, token, "auto");
                    }
                    drop(pending);
                    drop(state);
                    log("ble: verified AirPods available outside the closed case; auto-connecting");
                },
                move || {
                    c_scan.battery_scan_allowed() && c_scan.battery_keys.lock().unwrap().is_some()
                },
            );
        });
    }

    #[cfg(feature = "experimental-playback")]
    playback::start(ctx.clone());
    handsfree::start(ctx.clone());
    // AAP needs a paired device; Windows capture/driver status does not.
    let receiver = {
        let c = ctx.clone();
        Some(thread::spawn(move || run_receiver(c)).thread().clone())
    };
    {
        let c = ctx.clone();
        thread::spawn(move || poll_mic(c, receiver));
    }
    // Poll volume even without paired AirPods so clients reflect the current output.
    {
        let c = ctx.clone();
        thread::spawn(move || {
            volume::init();
            // A playback-start ANC nudge has recovered silent audio before.
            // It runs only on the playback edge; no reply is required.
            let mut was_playing = false;
            let mut last_wake = None;
            loop {
                c.sync_volume();
                let playing = volume::any_render_active();
                if playing
                    && !was_playing
                    && cooldown_elapsed(last_wake, Instant::now(), Duration::from_secs(3))
                {
                    if reassert_confirmed_anc(&c) {
                        last_wake = Some(Instant::now());
                        log("audio: playback started — waking the AirPods (re-assert noise mode)");
                    }
                }
                was_playing = playing;
                thread::sleep(Duration::from_millis(500));
            }
        });
    }

    log("threads spawned; serving events + cmds pipes");
    loop {
        thread::sleep(Duration::from_secs(3600));
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "experimental-playback")]
    #[test]
    fn deferred_restore_keeps_reader_available_and_cancels_stale_start() {
        use super::{defer_audio_change, update_microphone_preference};
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        use std::sync::{mpsc, Arc, Mutex};

        let busy = Arc::new(AtomicBool::new(false));
        let revision = Arc::new(AtomicU64::new(1));
        let enabled = Arc::new(AtomicBool::new(false));
        let gate = Arc::new(Mutex::new(()));
        let (waiting_tx, waiting_rx) = mpsc::channel();
        let (restore_tx, restore_rx) = mpsc::channel();
        let worker = {
            let (revision, enabled, gate) = (revision.clone(), enabled.clone(), gate.clone());
            let expected = revision.load(Ordering::Acquire);
            defer_audio_change(busy.clone(), move || {
                waiting_tx.send(()).unwrap();
                restore_rx.recv().unwrap();
                update_microphone_preference(
                    &gate,
                    true,
                    || revision.load(Ordering::Acquire) == expected,
                    |on| enabled.store(on, Ordering::Release),
                    |_| panic!("superseded start persisted"),
                );
            })
            .unwrap()
        };
        waiting_rx.recv().unwrap();
        assert_eq!(
            defer_audio_change(busy.clone(), || panic!("second transition ran"))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::WouldBlock,
        );
        // Stop runs next on the same command reader while restoration is still waiting.
        update_microphone_preference(
            &gate,
            false,
            || true,
            |on| {
                revision.fetch_add(1, Ordering::AcqRel);
                enabled.store(on, Ordering::Release);
            },
            |_| {},
        );
        assert!(busy.load(Ordering::Acquire));
        assert!(!enabled.load(Ordering::Acquire));
        restore_tx.send(()).unwrap();
        worker.join().unwrap();
        assert!(!enabled.load(Ordering::Acquire));
        assert!(!busy.load(Ordering::Acquire));
        defer_audio_change(busy.clone(), || {})
            .unwrap()
            .join()
            .unwrap();
        assert!(!busy.load(Ordering::Acquire));
    }

    #[cfg(feature = "experimental-playback")]
    #[test]
    fn buffer_restoration_precedes_start_and_route_changes_but_never_stop_controls() {
        use super::{buffer_pause_for, BufferPauseFor, Command};
        use ntpods_ipc::{MicMode, PlaybackMode};

        for command in [
            Command::Connect,
            Command::RebuildAudio,
            Command::RepairConnection,
            Command::SetPlaybackMode {
                mode: PlaybackMode::Aac,
            },
            Command::RetryPlayback,
            Command::SetPlaybackBitrate {
                mode: PlaybackMode::AacEld,
                best_effort: true,
                kbps: 256,
                variable: true,
            },
            Command::SetPlaybackBuffer {
                target_ms: Some(120),
                restore_ms: Some(150),
            },
        ] {
            assert_eq!(
                buffer_pause_for(&command, false, false),
                Some(BufferPauseFor::Playback)
            );
        }
        for command in [
            Command::SetMicMode {
                auto: true,
                manual: false,
            },
            Command::SetMicManualActive { active: true },
            Command::SetMicPolicy {
                mode: MicMode::KeepActive,
                stop_delay_secs: 0,
            },
        ] {
            assert_eq!(
                buffer_pause_for(&command, false, false),
                Some(BufferPauseFor::Microphone)
            );
        }
        for capture in [false, true] {
            assert_eq!(
                buffer_pause_for(
                    &Command::SetMicPolicy {
                        mode: MicMode::Automatic,
                        stop_delay_secs: 0
                    },
                    capture,
                    false
                ),
                capture.then_some(BufferPauseFor::Microphone),
            );
            for toggle_starts in [false, true] {
                assert_eq!(
                    buffer_pause_for(&Command::ToggleMicManual, capture, toggle_starts),
                    toggle_starts.then_some(BufferPauseFor::Microphone),
                );
                for command in [
                    Command::Disconnect,
                    Command::Shutdown,
                    Command::SetMicMode {
                        auto: false,
                        manual: true,
                    },
                    Command::SetMicManualActive { active: false },
                    Command::SetMicPolicy {
                        mode: MicMode::Manual,
                        stop_delay_secs: 0,
                    },
                    Command::SetPlaybackBuffer {
                        target_ms: None,
                        restore_ms: None,
                    },
                    Command::SetVolume { percent: 10 },
                    Command::ToggleMute,
                ] {
                    assert_eq!(buffer_pause_for(&command, capture, toggle_starts), None);
                }
            }
        }
    }

    #[cfg(feature = "experimental-playback")]
    #[test]
    fn microphone_toggle_restoration_respects_pending_intent_and_stop_delay() {
        use super::{mic_lifecycle::Demand, microphone_toggle_starts};
        use ntpods_ipc::MicMode;

        let mut demand = Demand::default();
        assert!(microphone_toggle_starts(&mut demand, 0, true, false, false));
        assert!(!microphone_toggle_starts(&mut demand, 0, true, true, false));
        demand.configure(MicMode::Automatic, 2);
        assert!(!microphone_toggle_starts(
            &mut demand,
            100,
            true,
            false,
            false
        ));
        assert!(!microphone_toggle_starts(
            &mut demand,
            2099,
            true,
            false,
            false
        ));
        assert!(microphone_toggle_starts(
            &mut demand,
            2100,
            true,
            false,
            false
        ));
        for mode in [MicMode::Manual, MicMode::KeepActive] {
            demand.configure(mode, 0);
            demand.set_manual(true);
            assert!(!microphone_toggle_starts(
                &mut demand,
                3000,
                true,
                false,
                false
            ));
            assert!(microphone_toggle_starts(
                &mut demand,
                3000,
                false,
                false,
                false
            ));
        }
        assert!(!microphone_toggle_starts(
            &mut demand,
            3000,
            false,
            false,
            true
        ));

        demand.configure(MicMode::Manual, 0);
        demand.set_manual(true);
        assert!(!microphone_toggle_starts(
            &mut demand,
            3000,
            true,
            false,
            false
        ));
        // A stop classified before another client stopped must not become an unguarded start.
        demand.set_manual(false);
        assert!(demand
            .toggle_manual(3000, true, false, false, Err("retry"))
            .is_err());
        demand.set_manual(true);
        assert_eq!(
            demand.toggle_manual(3000, true, false, false, Err("retry")),
            Ok(false)
        );
    }

    #[test]
    fn pending_hearing_settings_block_new_microphones_without_stopping_capture() {
        for (native, playback) in [(false, "windows"), (true, "ready")] {
            let mut state = super::Snapshot {
                connected: true,
                model: "A3063".into(),
                firmware: "9A348".into(),
                mic_state: "idle".into(),
                playback_native_driver: native,
                playback_state: playback.into(),
                ..Default::default()
            };
            assert!(super::playback_allows_microphone(&state));
            state.hearing_aid_pending = true;
            assert!(!super::playback_allows_microphone(&state));
            assert_eq!(
                super::manual_microphone_start_allowed(&state, true, true),
                Err("Hearing settings are changing")
            );
            // Only the Off request can become pending once the microphone is reserved.
            for phase in ["starting", "capturing", "stopping", "recovering"] {
                state.mic_state = phase.into();
                assert!(super::playback_allows_microphone(&state));
            }
            state.mic_state = "idle".into();
            state.mic_uplink_active = true;
            assert!(super::playback_allows_microphone(&state));
            state.playback_buffer_target_ms = Some(60);
            assert!(!super::playback_allows_microphone(&state));
            state.playback_buffer_target_ms = None;
            state.mic_uplink_active = false;
            state.hearing_aid_pending = false;
            assert!(super::playback_allows_microphone(&state));
        }
    }

    #[test]
    fn hearing_edits_keep_the_callers_baseline_across_other_clients_reads() {
        let baseline: ntpods_ipc::HearingAidSettings = serde_json::from_value(serde_json::json!({
            "left_eq": [5,10,15,20,25,30,35,40], "right_eq": [10,15,20,25,30,35,40,45],
            "left_amplification": -0.25, "right_amplification": 0.5,
            "left_tone": -0.5, "right_tone": 0.25,
            "left_conversation_boost": true, "right_conversation_boost": false,
            "left_ambient_noise_reduction": 0.25, "right_ambient_noise_reduction": 0.75,
            "own_voice": 0.5,
        }))
        .unwrap();
        let draft = ntpods_ipc::HearingAidSettings {
            left_tone: 0.5,
            ..baseline
        };
        let request = super::HearingRequest {
            request_id: 1,
            on: true,
            expected: Some(baseline),
            settings: Some(draft),
        };
        assert!(request.matches_baseline(Some(baseline)));
        let mut newer = baseline;
        newer.left_eq[0] = 40.;
        assert!(
            !request.matches_baseline(Some(newer)),
            "another client's refresh must not authorize stale curves"
        );
        assert!(!request.matches_baseline(None));
        assert!(!super::HearingRequest {
            expected: None,
            ..request
        }
        .matches_baseline(Some(baseline)));
    }

    #[test]
    fn buffer_trial_and_failed_restore_block_microphone_and_reconnect() {
        let mut state = super::Snapshot {
            playback_native_driver: true,
            playback_state: "ready".into(),
            ..Default::default()
        };
        assert!(super::playback_allows_microphone(&state));
        assert!(super::native_reconnect_allowed(&state, false));
        state.playback_buffer_target_ms = Some(60);
        for error in [None, Some("Restore failed".to_owned())] {
            state.playback_buffer_error = error;
            assert!(!super::playback_allows_microphone(&state));
            assert!(!super::native_reconnect_allowed(&state, false));
        }
    }

    #[test]
    fn receiver_keeps_motion_and_microphone_waits_short() {
        for (mic_on, head_tracking_enabled, expected_ms) in [
            (false, false, 150),
            (false, true, 4),
            (true, false, 4),
            (true, true, 4),
        ] {
            assert_eq!(
                super::receiver_idle_wait(mic_on, head_tracking_enabled),
                super::Duration::from_millis(expected_ms)
            );
        }
    }

    #[test]
    fn handsfree_fallback_remains_for_unsupported_or_failed_routes() {
        let mut state = super::Snapshot {
            connected: true,
            model: "A3063".into(),
            firmware: "9A348".into(),
            mic_state: "idle".into(),
            playback_native_driver: true,
            playback_available: true,
            playback_state: "ready".into(),
            ..Default::default()
        };
        assert!(super::hide_handsfree(&state));
        for model in ["A2084", "unknown", ""] {
            state.model = model.into();
            assert!(!super::hide_handsfree(&state));
        }
        state.model = "A3063".into();
        state.firmware = "7B21".into();
        assert!(!super::hide_handsfree(&state));
        state.firmware = "9A348".into();
        state.mic_state = "error".into();
        assert!(!super::hide_handsfree(&state));
        state.mic_state = "recovering".into();
        for phase in ["ready", "starting", "stopping"] {
            state.playback_state = phase.into();
            assert!(super::hide_handsfree(&state));
        }
        for phase in ["error", "unavailable", "windows", "disconnected"] {
            state.playback_state = phase.into();
            assert!(!super::hide_handsfree(&state));
        }
        state.playback_state = "ready".into();
        state.playback_native_driver = false;
        assert!(!super::hide_handsfree(&state));
        state.playback_native_driver = true;
        state.playback_available = false;
        assert!(!super::hide_handsfree(&state));
        state.playback_available = true;
        state.link_stale = true;
        assert!(!super::hide_handsfree(&state));
        state.link_stale = false;
        state.connected = false;
        assert!(!super::hide_handsfree(&state));
    }

    #[test]
    fn reconnect_reserves_playback_before_a_waiting_capture_can_start() {
        let mut state = super::Snapshot {
            playback_native_driver: true,
            playback_state: "ready".into(),
            mic_capture_active: true,
            ..Default::default()
        };
        assert!(super::native_reconnect_allowed(&state, false));
        assert!(super::playback_allows_microphone(&state));
        super::reserve_native_playback_transition(&mut state);
        assert!(!super::playback_allows_microphone(&state));
        state.playback_native_driver = false;
        state.playback_state = "windows".into();
        super::reserve_native_playback_transition(&mut state);
        assert_eq!(state.playback_state, "windows");
    }

    #[test]
    fn native_reconnect_preserves_the_mic_recovery_owner() {
        let mut state = super::Snapshot {
            playback_native_driver: true,
            mic_capture_active: true,
            ..Default::default()
        };
        assert!(super::native_reconnect_allowed(&state, false));
        assert!(!super::native_reconnect_allowed(&state, true));
        state.mic_uplink_active = true;
        assert!(!super::native_reconnect_allowed(&state, false));
        state.mic_uplink_active = false;
        for phase in ["starting", "capturing", "stopping", "recovering"] {
            state.mic_state = phase.into();
            assert!(!super::native_reconnect_allowed(&state, false));
        }
        state.playback_native_driver = false;
        assert!(super::native_reconnect_allowed(&state, true));
    }

    #[test]
    fn native_playback_starts_before_waiting_microphone_clients() {
        let mut state = super::Snapshot {
            playback_native_driver: true,
            ..Default::default()
        };
        for phase in [
            "disconnected",
            "starting",
            "stopping",
            "error",
            "unavailable",
        ] {
            state.playback_state = phase.into();
            assert!(!super::playback_allows_microphone(&state));
        }
        state.playback_state = "ready".into();
        assert!(super::playback_allows_microphone(&state));
        state.playback_native_driver = false;
        state.playback_state = "disconnected".into();
        assert!(super::playback_allows_microphone(&state));
    }
    #[test]
    fn shortcut_starts_only_with_a_healthy_supported_connection() {
        let mut state = ntpods_ipc::Snapshot {
            model: "A3063".into(),
            firmware: "9A348".into(),
            connected: true,
            mic_state: "idle".into(),
            ..Default::default()
        };
        assert!(super::manual_microphone_start_allowed(&state, true, true).is_ok());
        assert!(super::manual_microphone_start_allowed(&state, false, true).is_err());
        assert!(super::manual_microphone_start_allowed(&state, true, false).is_err());
        state.connected = false;
        assert!(super::manual_microphone_start_allowed(&state, true, true).is_err());
        state.connected = true;
        state.link_stale = true;
        assert!(super::manual_microphone_start_allowed(&state, true, true).is_err());
        state.link_stale = false;
        for phase in ["stopping", "recovering", "error"] {
            state.mic_state = phase.into();
            assert!(super::manual_microphone_start_allowed(&state, true, true).is_err());
        }
        state.mic_state = "idle".into();
        for phase in ["starting", "stopping"] {
            state.playback_state = phase.into();
            assert!(super::manual_microphone_start_allowed(&state, true, true).is_err());
        }
        state.playback_state = "ready".into();
        assert!(super::manual_microphone_start_allowed(&state, true, true).is_ok());
        for model in ["", "A2084"] {
            state.model = model.into();
            assert!(super::manual_microphone_start_allowed(&state, true, true).is_err());
        }
        state.model = "A3063".into();
        state.firmware = "7B20".into();
        assert!(super::manual_microphone_start_allowed(&state, true, true).is_err());
    }

    #[test]
    fn quiet_connected_control_link_remains_usable_without_echoes() {
        let mut link = super::ControlLinkStatus::default();
        for seconds in 1..=300 {
            assert!(!link.observe(true, true, false, super::Duration::from_secs(seconds)));
            assert!(!link.stale);
        }
    }

    #[test]
    fn recovered_transport_clears_stale_without_an_incoming_packet() {
        let mut link = super::ControlLinkStatus::default();
        assert!(!link.observe(false, true, false, super::Duration::from_secs(1)));
        assert!(!link.stale);
        assert!(!link.observe(false, true, false, super::Duration::from_secs(30)));
        assert!(link.stale);
        assert!(!link.observe(true, true, false, super::Duration::from_secs(31)));
        assert!(!link.stale);
        // Capture recovery has its own deadline; idle checks must not replace it.
        assert!(!link.observe(false, true, true, super::Duration::from_secs(60)));
        assert!(!link.stale);
    }

    #[test]
    fn control_link_releases_after_three_consecutive_os_disconnects() {
        let mut link = super::ControlLinkStatus::default();
        let age = super::Duration::from_secs(60);
        assert!(!link.observe(false, false, false, age));
        assert!(!link.observe(false, false, false, age));
        assert!(!link.observe(false, true, false, age));
        assert!(!link.observe(false, false, false, age));
        assert!(!link.observe(false, false, false, age));
        assert!(link.observe(false, false, false, age));
    }

    #[test]
    fn changing_pairs_clears_device_data_but_keeps_windows_preferences() {
        let mut state = ntpods_ipc::Snapshot {
            device_id: "001122334455".into(),
            dev_name: "First pair".into(),
            connected: true,
            model: "A3063".into(),
            firmware: "9A348".into(),
            serial: "private".into(),
            auto_mode: true,
            auto_pause: false,
            mic_mode: ntpods_ipc::MicMode::KeepActive,
            mic_stop_delay_secs: 30,
            mic_manual_active: true,
            mic_uplink_active: true,
            heart_rate_enabled: true,
            heart_rate: Some(80),
            head_tracking_enabled: true,
            anc: 4,
            allow_off_requested: Some(true),
            device_controls: [(0x0D, 4), (0x34, 1)].into(),
            battery: ntpods_ipc::Battery {
                case: Some(90),
                ..Default::default()
            },
            battery_live: true,
            battery_timestamp_ms: Some(100),
            volume: 43,
            playback_mode: ntpods_ipc::PlaybackMode::Aac,
            playback_preferred_mode: Some(ntpods_ipc::PlaybackMode::AacEld),
            playback_available: true,
            playback_state: "ready".into(),
            ..Default::default()
        };
        super::reset_pair_snapshot(&mut state, 0xaabbccddeeff, "Second pair");
        assert_eq!(state.device_id, "aabbccddeeff");
        assert_eq!(state.dev_name, "Second pair");
        assert!(state.auto_mode);
        assert!(!state.auto_pause);
        assert_eq!(state.mic_mode, ntpods_ipc::MicMode::KeepActive);
        assert_eq!(state.mic_stop_delay_secs, 30);
        assert_eq!(state.volume, 43);
        assert_eq!(state.playback_mode, ntpods_ipc::PlaybackMode::Aac);
        assert_eq!(
            state.playback_preferred_mode,
            Some(ntpods_ipc::PlaybackMode::AacEld)
        );
        assert!(!state.playback_available);
        assert_eq!(state.playback_state, "disconnected");
        assert!(!state.connected);
        assert!(!super::device_models::microphone(&state));
        assert!(state.model.is_empty() && state.firmware.is_empty() && state.serial.is_empty());
        assert!(state.device_controls.is_empty());
        assert_eq!(state.battery, ntpods_ipc::Battery::default());
        assert!(!state.battery_live && state.battery_timestamp_ms.is_none());
        assert!(!state.mic_manual_active && !state.mic_uplink_active);
        assert!(!state.heart_rate_enabled && state.heart_rate.is_none());
        assert!(!state.head_tracking_enabled);
        assert_eq!(state.anc, 0);
        assert!(state.allow_off_requested.is_none());
    }

    #[test]
    fn shutdown_succeeds_only_after_confirmed_cleanup() {
        for receiver_done in [false, true] {
            for audio_done in [false, true] {
                for failed in [false, true] {
                    assert_eq!(
                        super::shutdown_exit_code(receiver_done, audio_done, failed, Ok(false))
                            == 0,
                        receiver_done && audio_done && !failed
                    );
                }
            }
        }
    }

    #[test]
    fn shutdown_fails_with_a_retained_or_unreadable_playback_journal() {
        use std::io::Write;
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "ntpods-shutdown-{}-{unique}.json",
            std::process::id()
        ));
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.write_all(br#"{"address":1,"sink":true,"handsfree":true}"#)
            .unwrap();
        drop(file);
        assert_eq!(
            super::shutdown_exit_code(true, true, false, path.try_exists()),
            1
        );
        std::fs::remove_file(&path).unwrap();
        assert_eq!(
            super::shutdown_exit_code(true, true, false, path.try_exists()),
            0
        );
        assert_eq!(
            super::shutdown_exit_code(
                true,
                true,
                false,
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "unreadable journal"
                ))
            ),
            1
        );
    }

    #[test]
    fn only_a_recent_complete_both_charging_report_suppresses_reclaim() {
        let packet = [4, 0, 4, 0, 4, 0, 2, 2, 1, 61, 5, 1, 4, 1, 63, 5, 1];
        let battery = super::aap::parse_battery(&packet).unwrap();
        assert!(super::recently_cased(Some((1_000, battery)), 1_000));
        assert!(super::recently_cased(Some((1_000, battery)), 21_000));
        assert!(super::recently_cased(Some((1_000, battery)), 46_000));
        assert!(!super::recently_cased(Some((1_000, battery)), 46_001));
        assert!(!super::recently_cased(Some((1_000, battery)), 999));
        assert!(!super::recently_cased(None, 1_000));
        let mut single = battery;
        single.right_charging = false;
        assert!(!super::recently_cased(Some((1_000, single)), 1_001));
        let partial = super::aap::parse_battery(&[4, 0, 4, 0, 4, 0, 1, 4, 1, 63, 5, 1]).unwrap();
        assert!(!super::recently_cased(Some((1_000, partial)), 1_001));
    }

    #[test]
    fn stop_during_blocked_start_prevents_late_pcm_permission() {
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        use std::sync::{mpsc, Arc, Mutex};
        let gate = Arc::new(Mutex::new(()));
        let revision = Arc::new(AtomicU64::new(0));
        let output = Arc::new(AtomicBool::new(false));
        let (sent_tx, sent_rx) = mpsc::channel();
        let (complete_tx, complete_rx) = mpsc::channel();
        let worker = {
            let (gate, revision, output) = (gate.clone(), revision.clone(), output.clone());
            std::thread::spawn(move || {
                assert!(super::while_request_current(
                    &gate,
                    || revision.load(Ordering::Acquire) == 0,
                    || {}
                ));
                sent_tx.send(()).unwrap();
                complete_rx.recv().unwrap(); // delayed Bluetooth START completion
                super::while_request_current(
                    &gate,
                    || revision.load(Ordering::Acquire) == 0,
                    || output.store(true, Ordering::Release),
                )
            })
        };
        sent_rx.recv().unwrap();
        {
            // Manual Stop can finish while Bluetooth START is still blocked.
            let _guard = gate.lock().unwrap();
            revision.fetch_add(1, Ordering::AcqRel);
            output.store(false, Ordering::Release);
        }
        complete_tx.send(()).unwrap();
        assert!(!worker.join().unwrap());
        assert!(!output.load(Ordering::Acquire));
    }

    #[test]
    fn discovery_retries_after_startup_and_stops_during_shutdown() {
        let mut attempts = 0;
        let found = super::discover_until_ready(
            || false,
            || {
                attempts += 1;
                (attempts == 3).then_some(42)
            },
            || {},
        );
        assert_eq!(found, Some(42));
        assert_eq!(attempts, 3);
        let stopped = std::cell::Cell::new(false);
        assert_eq!(
            super::discover_until_ready(|| stopped.get(), || None::<u64>, || stopped.set(true)),
            None
        );
    }

    #[test]
    fn equal_heart_rate_samples_renew_freshness_and_expiry_keeps_enabled() {
        let mut state = super::Snapshot {
            heart_rate_enabled: true,
            ..Default::default()
        };
        super::record_heart_rate(&mut state, 72, 1_000);
        super::record_heart_rate(&mut state, 72, 9_000);
        assert!(!super::expire_heart_rate(&mut state, 11_000));
        assert_eq!(state.heart_rate_timestamp_ms, Some(9_000));
        assert!(super::expire_heart_rate(&mut state, 19_000));
        assert!(state.heart_rate.is_none() && state.heart_rate_timestamp_ms.is_none());
        assert!(state.heart_rate_enabled);
    }

    #[test]
    fn name_requests_validate_bytes_and_propagate_send_failure() {
        assert!(super::send_name_request(&"é".repeat(32), || Ok(())).is_ok());
        for name in ["é".repeat(33), String::new(), "  ".into()] {
            assert!(
                super::send_name_request(&name, || panic!("invalid name must not send")).is_err()
            );
        }
        assert!(super::send_name_request("AirPods", || Err(
            std::io::ErrorKind::NotConnected.into()
        ))
        .is_err());
    }

    #[test]
    fn failed_rebuild_waits_for_mic_commands_before_locking_state() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{mpsc, Arc, Mutex};
        use std::time::{Duration, Instant};

        for superseded in [false, true] {
            let preference = Arc::new(Mutex::new(()));
            let state = Arc::new(Mutex::new(super::Snapshot::default()));
            let pending = Arc::new(Mutex::new(super::PendingAudioDisconnect::default()));
            let current = Arc::new(AtomicBool::new(true));
            let disabled = Arc::new(AtomicBool::new(false));
            let (started_tx, started_rx) = mpsc::channel();
            let (done_tx, done_rx) = mpsc::channel();
            let microphone_command = preference.lock().unwrap();
            let worker = {
                let (preference, state, pending, current, disabled) = (
                    preference.clone(),
                    state.clone(),
                    pending.clone(),
                    current.clone(),
                    disabled.clone(),
                );
                std::thread::spawn(move || {
                    started_tx.send(()).unwrap();
                    let failed = super::fail_audio_rebuild(
                        &preference,
                        &state,
                        &pending,
                        || current.load(Ordering::Acquire),
                        || disabled.store(true, Ordering::Release),
                    );
                    done_tx.send(failed).unwrap();
                })
            };
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
            let deadline = Instant::now() + Duration::from_millis(100);
            while Instant::now() < deadline {
                // A microphone command already holding preference must still reach state.
                assert!(state.try_lock().is_ok());
                std::thread::yield_now();
            }
            assert!(done_rx.try_recv().is_err());
            current.store(!superseded, Ordering::Release);
            drop(microphone_command);
            assert_eq!(
                done_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
                !superseded
            );
            worker.join().unwrap();
            assert_eq!(disabled.load(Ordering::Acquire), !superseded);
            let state = state.lock().unwrap();
            assert_eq!(state.mic_state == "error", !superseded);
            assert_eq!(state.mic_error.is_some(), !superseded);
        }
    }

    #[test]
    fn delayed_microphone_enable_cannot_overwrite_a_stop_or_new_policy() {
        use super::{update_microphone_preference, MicrophoneRequest};
        use std::sync::Mutex;

        let before = MicrophoneRequest {
            revision: 7,
            address: 1,
            enabled: true,
            mode: ntpods_ipc::MicMode::Automatic,
            stop_delay_secs: 0,
            manual_active: false,
        };
        let gate = Mutex::new(());
        for after in [
            MicrophoneRequest {
                revision: 8,
                ..before
            },
            MicrophoneRequest {
                address: 2,
                ..before
            },
            MicrophoneRequest {
                enabled: false,
                ..before
            },
            MicrophoneRequest {
                mode: ntpods_ipc::MicMode::KeepActive,
                ..before
            },
            MicrophoneRequest {
                stop_delay_secs: 10,
                ..before
            },
            MicrophoneRequest {
                manual_active: true,
                ..before
            },
        ] {
            update_microphone_preference(
                &gate,
                true,
                || {
                    assert!(gate.try_lock().is_err());
                    before.matches(after, false)
                },
                |_| panic!("stale enable applied"),
                |_| panic!("stale enable persisted"),
            );
        }
        assert!(!before.matches(before, true));
        assert!(before.matches(before, false));
    }

    #[test]
    fn microphone_preference_and_persistence_cannot_cross_during_disable() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{mpsc, Arc, Mutex};
        let gate = Arc::new(Mutex::new(()));
        let permission = Arc::new(AtomicBool::new(false));
        let preference = Arc::new(AtomicBool::new(false));
        let stored = Arc::new(AtomicBool::new(false));
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = {
            let (gate, permission, preference, stored) = (
                gate.clone(),
                permission.clone(),
                preference.clone(),
                stored.clone(),
            );
            std::thread::spawn(move || {
                super::update_microphone_preference(
                    &gate,
                    true,
                    || true,
                    |on| {
                        permission.store(on, Ordering::Release);
                        started_tx.send(()).unwrap();
                        release_rx.recv().unwrap();
                        preference.store(on, Ordering::Release);
                    },
                    |on| stored.store(on, Ordering::Release),
                )
            })
        };
        started_rx.recv().unwrap();
        assert!(gate.try_lock().is_err());
        let disable = {
            let (gate, permission, preference, stored) = (
                gate.clone(),
                permission.clone(),
                preference.clone(),
                stored.clone(),
            );
            std::thread::spawn(move || {
                super::update_microphone_preference(
                    &gate,
                    false,
                    || true,
                    |on| {
                        permission.store(on, Ordering::Release);
                        preference.store(on, Ordering::Release);
                    },
                    |on| stored.store(on, Ordering::Release),
                )
            })
        };
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        disable.join().unwrap();
        assert!(!permission.load(Ordering::Acquire));
        assert!(!preference.load(Ordering::Acquire));
        assert!(!stored.load(Ordering::Acquire));
    }

    #[test]
    fn heart_rate_refresh_preserves_stopped_and_newer_requests() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let retrying = AtomicBool::new(false);
        let got_sample = AtomicBool::new(true);
        let queued = 8; // Owner 7, queued while its stream was live.
        for (enabled, generation) in [(false, 7), (true, 9)] {
            assert!(!super::claim_heart_rate_refresh(
                queued,
                |owner| enabled && owner == generation,
                &retrying,
                &got_sample,
            ));
            assert!(got_sample.load(Ordering::Acquire));
        }
        assert!(!super::claim_heart_rate_refresh(
            0,
            |_| panic!("empty queue has no owner"),
            &retrying,
            &got_sample,
        ));
    }

    #[test]
    fn heart_rate_refresh_cannot_restart_an_active_or_failed_campaign() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let retrying = AtomicBool::new(true);
        let got_sample = AtomicBool::new(true); // A late report during a new attempt.
        assert!(!super::claim_heart_rate_refresh(
            8,
            |owner| owner == 7,
            &retrying,
            &got_sample,
        ));
        assert!(got_sample.load(Ordering::Acquire));

        retrying.store(false, Ordering::Release);
        got_sample.store(false, Ordering::Release);
        assert!(!super::claim_heart_rate_refresh(
            8,
            |owner| owner == 7,
            &retrying,
            &got_sample,
        ));
        got_sample.store(true, Ordering::Release);
        assert!(super::claim_heart_rate_refresh(
            8,
            |owner| owner == 7,
            &retrying,
            &got_sample,
        ));
        assert!(!got_sample.load(Ordering::Acquire));
        assert!(!super::claim_heart_rate_refresh(
            8,
            |owner| owner == 7,
            &retrying,
            &got_sample,
        ));
    }

    #[test]
    fn stopped_heart_rate_worker_cannot_send_after_the_stop_command() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{mpsc, Arc, Mutex};
        let gate = Arc::new(Mutex::new(()));
        let enabled = Arc::new(AtomicBool::new(true));
        let sent = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let start = {
            let (gate, enabled, sent) = (gate.clone(), enabled.clone(), sent.clone());
            std::thread::spawn(move || {
                super::while_request_current(
                    &gate,
                    || enabled.load(Ordering::Acquire),
                    || {
                        tx.send(()).unwrap();
                        resume_rx.recv().unwrap();
                        sent.lock().unwrap().push("start");
                    },
                )
            })
        };
        rx.recv().unwrap();
        let stop = {
            let (gate, enabled, sent) = (gate.clone(), enabled.clone(), sent.clone());
            std::thread::spawn(move || {
                let _guard = gate.lock().unwrap();
                enabled.store(false, Ordering::Release);
                sent.lock().unwrap().push("stop");
            })
        };
        resume_tx.send(()).unwrap();
        start.join().unwrap();
        stop.join().unwrap();
        assert!(!super::while_request_current(
            &gate,
            || enabled.load(Ordering::Acquire),
            || sent.lock().unwrap().push("late start")
        ));
        assert_eq!(*sent.lock().unwrap(), ["start", "stop"]);
    }

    use super::{
        claim_automatic_connection, cooldown_elapsed, media, mic_lifecycle, queue_settings_refresh,
        settings_refresh_allowed, update_auto_pause, utc_timestamp, Snapshot,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    #[test]
    fn microphone_clock_stats_count_wraps_without_assigning_units_or_restarting() {
        let mut stats = super::MicStats::default();
        for timestamp in [u32::MAX - 10, 169, 349, 349, 100, 280] {
            stats.access_unit(timestamp);
        }
        assert_eq!(stats.timestamp_ticks, 540);
        assert_eq!(stats.timestamp_intervals, 3);
        assert_eq!(stats.timestamp_repeats, 1);
        assert_eq!(stats.timestamp_backwards, 1);
        stats.begin();
        stats.access_unit(42);
        assert_eq!(stats.timestamp_intervals, 0);
        assert_eq!(stats.timestamp_ticks, 0);
    }

    #[test]
    fn explicit_disconnect_stops_and_closes_before_releasing_audio_even_on_failure() {
        use std::cell::RefCell;
        for (stop_fails, close_fails) in [(false, false), (true, false), (false, true)] {
            let calls = RefCell::new(Vec::new());
            let (stopped, closed, recovery_failed) = super::close_control_session(
                true,
                || {
                    calls.borrow_mut().push("stop");
                    if stop_fails {
                        Err(std::io::Error::other("stop failed"))
                    } else {
                        Ok(())
                    }
                },
                || {
                    calls.borrow_mut().push("close");
                    if close_fails {
                        Err(std::io::Error::other("close failed"))
                    } else {
                        Ok(())
                    }
                },
                || {
                    calls.borrow_mut().push("disconnect audio");
                    false
                },
            );
            assert_eq!(*calls.borrow(), ["stop", "close", "disconnect audio"]);
            assert_eq!(stopped.is_err(), stop_fails);
            assert_eq!(closed.is_err(), close_fails);
            assert!(!recovery_failed);
        }
    }

    #[test]
    fn idle_control_session_disconnects_without_starting_microphone_recovery() {
        use std::cell::RefCell;
        let calls = RefCell::new(Vec::new());
        let (stopped, closed, recovery_failed) = super::close_control_session(
            false,
            || panic!("no microphone stream to stop"),
            || {
                calls.borrow_mut().push("close");
                Ok(())
            },
            || {
                calls.borrow_mut().push("disconnect audio");
                true
            },
        );
        assert!(stopped.is_ok() && closed.is_ok());
        assert!(recovery_failed);
        assert_eq!(*calls.borrow(), ["close", "disconnect audio"]);
    }

    #[test]
    fn repeated_connect_cannot_cancel_the_required_post_disconnect_rebuild() {
        let mut pending = super::PendingAudioDisconnect::default();
        assert_eq!(pending.queue_connect(1), Some(1));
        pending.queue_disconnect(2);
        assert_eq!(pending.queue_connect(3), None);
        assert_eq!(pending.take(), Some((3, true)));
        assert_eq!(pending.queue_connect(5), None);
        pending.rebuild_finished(false); // Earlier worker was superseded.
        assert_eq!(pending.take(), Some((5, true)));
        pending.rebuild_finished(true);
        assert_eq!(pending.queue_connect(7), Some(7));
        assert!(!pending.is_pending());
    }

    #[test]
    fn idle_disconnect_is_consumed_once_and_later_connect_still_restores_playback() {
        let mut pending = super::PendingAudioDisconnect::default();
        pending.queue_disconnect(2);
        assert_eq!(pending.take(), Some((2, true)));
        assert_eq!(pending.take(), None);
        assert_eq!(pending.queue_connect(3), None);
        assert_eq!(pending.take(), Some((3, true)));
        // A new Disconnect while that Connect recovers always wins.
        pending.queue_disconnect(4);
        pending.rebuild_finished(false);
        assert_eq!(pending.take(), Some((4, true)));
        assert_eq!(pending.queue_connect(5), None);
    }

    #[test]
    fn repair_waits_for_the_rebuild_that_superseded_an_earlier_connect() {
        let mut pending = super::PendingAudioDisconnect::default();
        let repair = AtomicBool::new(true);
        pending.queue_disconnect(2);
        assert_eq!(pending.queue_connect(3), None);
        assert_eq!(pending.take(), Some((3, true)));
        assert_eq!(pending.take_repair_if_ready(&repair), None);
        // Repair arrives while Connect's reset is in progress.
        assert_eq!(pending.queue_connect(5), None);
        pending.rebuild_finished(false);
        assert_eq!(pending.take_repair_if_ready(&repair), None);
        assert!(repair.load(Ordering::Acquire));
        assert_eq!(pending.take(), Some((5, true)));
        pending.rebuild_finished(true);
        assert_eq!(pending.take_repair_if_ready(&repair), Some(true));
        assert_eq!(pending.take_repair_if_ready(&repair), Some(false));
    }

    #[test]
    fn conversation_volume_validates_and_serializes_state_with_persistence() {
        let duck = Mutex::new(super::volume::ConvDuck::default());
        let state = Mutex::new(Snapshot::default());
        assert!(super::update_conversation_volume(&duck, &state, 101, || {
            panic!("invalid input must not be persisted")
        })
        .is_err());
        assert_eq!(state.lock().unwrap().conversation_volume, 25);
        super::update_conversation_volume(&duck, &state, 0, || {
            assert!(
                duck.try_lock().is_err(),
                "keep preference updates ordered until saved"
            );
            assert_eq!(state.lock().unwrap().conversation_volume, 0);
            Ok(())
        })
        .unwrap();
        assert!(super::update_conversation_volume(&duck, &state, 100, || {
            Err(std::io::Error::other("fixture save failure"))
        })
        .is_err());
        assert_eq!(state.lock().unwrap().conversation_volume, 100);
    }

    #[test]
    fn polling_starts_immediately_without_backdating_the_clock() {
        let first_poll = Instant::now();
        for seconds in [3, 5] {
            let cooldown = Duration::from_secs(seconds);
            assert!(cooldown_elapsed(None, first_poll, cooldown));
            assert!(!cooldown_elapsed(Some(first_poll), first_poll, cooldown));
            assert!(!cooldown_elapsed(
                Some(first_poll),
                first_poll + cooldown - Duration::from_millis(1),
                cooldown,
            ));
            assert!(cooldown_elapsed(
                Some(first_poll),
                first_poll + cooldown,
                cooldown,
            ));
        }
    }

    #[test]
    fn utc_timestamp_is_iso8601() {
        assert_eq!(
            utc_timestamp(Duration::from_secs(0)),
            "1970-01-01T00:00:00.000Z"
        );
        assert_eq!(
            utc_timestamp(Duration::from_secs(951_782_400)),
            "2000-02-29T00:00:00.000Z"
        );
        assert_eq!(
            utc_timestamp(Duration::from_millis(1_790_705_162_071)),
            "2026-09-29T18:06:02.071Z"
        );
        assert_eq!(
            utc_timestamp(Duration::from_secs(1_798_761_599)),
            "2026-12-31T23:59:59.000Z"
        );
    }

    #[test]
    fn repeated_ble_or_windows_resume_cannot_override_an_explicit_retry_latch() {
        let requested = super::ConnectionRequest::new(false);
        let latched = AtomicBool::new(true);
        let shutdown = AtomicBool::new(false);
        for _ in 0..100 {
            assert!(!claim_automatic_connection(&requested, &latched, &shutdown));
            assert!(!requested.load(Ordering::Acquire));
        }
        // Only an explicit Connect/Repair clears the latch. One automatic
        // request can then win; later advertisements cannot duplicate it.
        latched.store(false, Ordering::Release);
        assert!(claim_automatic_connection(&requested, &latched, &shutdown));
        assert!(!claim_automatic_connection(&requested, &latched, &shutdown));
        requested.store(false, Ordering::Release);
        shutdown.store(true, Ordering::Release);
        assert!(!claim_automatic_connection(&requested, &latched, &shutdown));
        assert!(!requested.load(Ordering::Acquire));
    }

    #[test]
    fn settings_refresh_checks_live_capture_even_when_the_snapshot_was_idle() {
        let mic = mic_lifecycle::Lifecycle::default();
        assert!(settings_refresh_allowed(&mic, Some(0), false));
        for streams in [Some(1), Some(2), None] {
            assert!(!settings_refresh_allowed(&mic, streams, false));
        }
        assert!(!settings_refresh_allowed(&mic, Some(0), true));
    }

    #[test]
    fn refresh_requested_during_capture_cannot_run_after_capture_closes() {
        let requested = AtomicBool::new(false);
        let mut state = Snapshot {
            mic_state: "idle".into(),
            auto_mode: false,
            ..Default::default()
        };
        queue_settings_refresh(&requested, &mut state, Some(1), false);
        assert!(state.device_settings_error.is_some());
        let mic = mic_lifecycle::Lifecycle::default();
        assert!(settings_refresh_allowed(&mic, Some(0), false));
        assert!(
            !requested.swap(false, Ordering::AcqRel),
            "closing capture must not revive a rejected request"
        );
        queue_settings_refresh(&requested, &mut state, Some(0), false);
        assert!(
            requested.swap(false, Ordering::AcqRel),
            "a new idle request may proceed"
        );
        assert!(state.device_settings_error.is_none());
    }

    #[test]
    fn refresh_request_rejects_unknown_status_and_unfinished_capture_state() {
        let requested = AtomicBool::new(false);
        let idle = Snapshot {
            mic_state: "idle".into(),
            ..Default::default()
        };
        for (mut state, streams, active) in [
            (idle.clone(), None, false),
            (idle.clone(), Some(0), true),
            (
                Snapshot {
                    mic_capture_active: true,
                    ..idle.clone()
                },
                Some(0),
                false,
            ),
            (
                Snapshot {
                    mic_uplink_active: true,
                    ..idle.clone()
                },
                Some(0),
                false,
            ),
            (
                Snapshot {
                    mic_recording: true,
                    ..idle.clone()
                },
                Some(0),
                false,
            ),
        ] {
            queue_settings_refresh(&requested, &mut state, streams, active);
            assert!(!requested.load(Ordering::Acquire));
            assert!(state.device_settings_error.is_some());
        }
        for phase in [
            "starting",
            "capturing",
            "stopping",
            "recovering",
            "error",
            "unavailable",
            "",
        ] {
            let mut state = Snapshot {
                mic_state: phase.into(),
                ..idle.clone()
            };
            queue_settings_refresh(&requested, &mut state, Some(0), false);
            assert!(!requested.load(Ordering::Acquire), "{phase}");
            assert!(state.device_settings_error.is_some());
        }
    }

    #[test]
    fn settings_refresh_cannot_close_a_starting_or_unfinished_microphone_stream() {
        use mic_lifecycle::Phase;
        let mut mic = mic_lifecycle::Lifecycle::default();
        for phase in [
            Phase::Starting,
            Phase::Capturing,
            Phase::Stopping,
            Phase::Recovering,
            Phase::Error,
        ] {
            mic.phase = phase;
            assert!(!settings_refresh_allowed(&mic, Some(0), false), "{phase:?}");
        }
        mic.phase = Phase::Idle;
        mic.audio_packet(0);
        assert!(
            !settings_refresh_allowed(&mic, Some(0), false),
            "unexpected audio still requires cleanup"
        );
    }

    #[test]
    fn auto_pause_clients_cannot_reorder_policy_snapshot_and_persistence() {
        let policy = Arc::new(Mutex::new(media::AutoPausePolicy::new(true)));
        let state = Arc::new(Mutex::new(Snapshot::default()));
        let persisted = Arc::new(Mutex::new(true));
        let (inside_tx, inside_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let first = {
            let (policy, state, persisted) = (policy.clone(), state.clone(), persisted.clone());
            thread::spawn(move || {
                update_auto_pause(&policy, &state, false, || {
                    inside_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    *persisted.lock().unwrap() = false;
                    Ok(())
                })
                .unwrap()
            })
        };
        inside_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(!state.lock().unwrap().auto_pause);
        assert!(
            policy.try_lock().is_err(),
            "persistence must keep the policy guard"
        );
        let (started_tx, started_rx) = mpsc::channel();
        let second = {
            let (policy, state, persisted) = (policy.clone(), state.clone(), persisted.clone());
            thread::spawn(move || {
                started_tx.send(()).unwrap();
                update_auto_pause(&policy, &state, true, || {
                    *persisted.lock().unwrap() = true;
                    Ok(())
                })
                .unwrap();
            })
        };
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        release_tx.send(()).unwrap();
        first.join().unwrap();
        second.join().unwrap();
        assert!(policy.lock().unwrap().permission().is_some());
        assert!(state.lock().unwrap().auto_pause);
        assert!(*persisted.lock().unwrap());
    }
}
