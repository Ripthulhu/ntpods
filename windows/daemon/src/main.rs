//! NTPods background service: AAP connection, device state and microphone pipeline.
//! Serves the WinUI app over NDJSON pipes; see ../../docs/daemon-ipc/PLAN.md.
//! Runs without a console window.

#![windows_subsystem = "windows"]
#![allow(dead_code)]

mod a2dp;
mod aap;
mod atomic_file;
mod att_settings;
mod battery_ble;
mod bt;
mod capabilities;
mod case_return;
mod companion;
mod companion_prefs;
mod connection_request;
mod device_models;
mod device_settings;
mod devnode;
mod diagnostic_controls;
mod driver;
mod dongle;
mod dongle_hid;
mod dongle_protocol;
mod dongle_rpc;
mod dongle_features;
mod dongle_health;
mod eld;
mod gatt;
mod handsfree;
mod hardware_check;
mod hardware_diagnostics;
mod head_tracking;
mod hearing;
mod hearing_diagnostics;
mod hearing_metadata;
#[cfg(feature = "hearing-test-research")]
mod hearing_test_mode;
mod helper_launch;
mod hfp_endpoints;
mod hfp_guard;
mod hfp_profile;
mod host_clock;
mod hr;
mod hrdb;
mod ipc_server;
mod le;
mod media;
mod mic_lifecycle;
mod micpipe;
mod nearby_finder;
#[cfg(feature = "experimental-playback")]
mod playback;
mod plexamp;
mod rename;
mod rtbuddy;
mod security;
mod transparency;
mod volume;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use std::{io, ptr};

use connection_request::{
    pairing_allows_connect, pairing_allows_selection, ConnectionRequest, PairedTarget, PairingStage,
};
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

#[cfg(test)]
fn log(s: &str) {
    eprintln!("{s}");
}

#[cfg(not(test))]
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

fn selected_audio_output_ready(
    mode: ipc::PlaybackMode,
    playback_state: &str,
    selected: u64,
    windows_ready: bool,
    native_address: Option<u64>,
    audio_authorized: bool,
) -> bool {
    selected != 0
        && if mode == ipc::PlaybackMode::Windows {
            windows_ready
        } else {
            audio_authorized && playback_state == "ready" && native_address == Some(selected)
        }
}

fn should_publish_ble(
    restored: bool,
    changed: bool,
    lid_edge: bool,
    now_ms: u64,
    last_ms: u64,
) -> bool {
    lid_edge || restored && (changed || now_ms.saturating_sub(last_ms) >= 1000)
}

/// Each client has its own writer thread so a slow client cannot block the session.
type ClientTx = std::sync::mpsc::SyncSender<Vec<u8>>;

/// Everything the session, poll and IPC threads share.
#[derive(Clone)]
struct Ctx {
    dongle: Arc<dongle::Controller>,
    dongle_health: Arc<dongle_health::Controller>,
    companion: Arc<Mutex<companion::State>>,
    companion_preferences: Arc<Mutex<companion_prefs::Store>>,
    companion_actions: Arc<Mutex<()>>,
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
    hardware_check: Arc<Mutex<Option<diagnostic_controls::Job>>>,
    case_observed_tick: Arc<AtomicU64>,
    hr_commands: Arc<Mutex<()>>,
    hr_generation: Arc<AtomicU64>,
    head_tracking: Arc<Mutex<head_tracking::Tracker>>,
    nearby_finder: Arc<Mutex<nearby_finder::Finder>>,
    nearby_last_heard_tick: Arc<AtomicU64>,
    auto_pause: Arc<Mutex<media::AutoPausePolicy>>,
    hr_db: hrdb::HrDb,
    clients: Arc<Mutex<Vec<ClientTx>>>,
    driver_cell: Arc<Mutex<Option<driver::Driver>>>,
    shared_driver: Arc<driver::Shared>,
    mic_on: Arc<AtomicBool>,
    hq_mic_quiet: Arc<AtomicBool>,
    capture_active: Arc<AtomicBool>,
    shutting_down: Arc<AtomicBool>,
    receiver_done: Arc<AtomicBool>,
    playback_done: Arc<AtomicBool>,
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
    case_return: Arc<Mutex<case_return::CaseReturn>>,
    paired_target: Arc<Mutex<Option<PairedTarget>>>,
    /// Repair request consumed by `run_receiver` to reopen AAP and ATT after cleanup.
    wants_reconnect: Arc<AtomicBool>,
    /// Consumed by the receiver; unlike repair, this must never stop a capture.
    wants_settings_refresh: Arc<Mutex<Option<SettingsRefresh>>>,
    hearing_confirmation: Arc<Mutex<Option<HearingConfirmation>>>,
    pipe: Arc<micpipe::MicPipeCell>,
    /// Shared so disabling Conversation Awareness can restore volume without an end event.
    conv_duck: Arc<Mutex<volume::ConvDuck>>,
    /// Explicit Disconnect blocks automatic resume. Connect or Repair clears it.
    user_disconnected: Arc<AtomicBool>,
    /// Reserved at Disconnect, applied only after the receiver closes its channel.
    pending_audio_disconnect: Arc<Mutex<PendingAudioDisconnect>>,
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
    fn volume_target(&self) -> Option<u64> {
        let state = self.state.lock().unwrap();
        volume::output_address(state.playback_mode, self.mac())
    }
    fn nearby_scan_allowed(&self) -> bool {
        if self.shutting_down.load(Ordering::Acquire) {
            return false;
        }
        let (opt_in, connected, capture, uplink) = {
            let state = self.state.lock().unwrap();
            (
                state.ble_scan_while_connected,
                state.connected,
                state.mic_capture_active || self.capture_active.load(Ordering::Acquire),
                state.mic_uplink_active || self.mic_on.load(Ordering::Acquire),
            )
        };
        let finder_scan = {
            let mut finder = self.nearby_finder.lock().unwrap();
            let paused = finder.active() && (capture || uplink);
            if finder.set_paused(paused) {
                publish_nearby_finder(self, &finder);
            }
            finder.scanning()
        };
        let permitted = opt_in || finder_scan;
        battery_ble::scan_allowed(permitted, connected, capture, uplink)
            && (permitted || self.mac() == 0 || !bt::is_connected(self.mac()))
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
            if !s.connected
                || s.link_stale
                || !diagnostic_controls::case_report_fresh(
                    self.case_observed_tick.load(Ordering::Acquire),
                    self.mic_clock.elapsed().as_millis() as u64,
                )
            {
                s.earbuds_in_case = None;
            }
            let last_heard_tick = self.nearby_last_heard_tick.load(Ordering::Acquire);
            if s.nearby_finder.active && last_heard_tick != 0 {
                s.nearby_finder.last_heard_age_ms = Some(
                    (self.mic_clock.elapsed().as_millis() as u64)
                        .saturating_sub(last_heard_tick - 1),
                );
            }
            #[cfg(feature = "experimental-playback")]
            let native_address = playback::active_address();
            #[cfg(not(feature = "experimental-playback"))]
            let native_address = None;
            s.audio_output_ready = selected_audio_output_ready(
                s.playback_mode,
                &s.playback_state,
                self.mac(),
                s.audio_output_ready,
                native_address,
                self.connect_requested.explicit(),
            );
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
            let hearing_modes = hearing::mode_state();
            let live = s.connected && !s.link_stale;
            s.hearing_aid_enrolled = if live {
                hearing_modes.hearing_enrolled
            } else {
                None
            };
            s.media_assist_enabled = if live {
                hearing_modes.media_enabled
            } else {
                None
            };
            s.media_assist_revision = hearing_modes.revision;
            s.media_assist_available = live && hearing::cached_pme_packet().is_some();
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
        let (mac, mode, playback_state) = {
            let s = self.state.lock().unwrap();
            (self.mac(), s.playback_mode, s.playback_state.clone())
        };
        let target = volume::output_address(mode, mac);
        let vol = volume::get(target);
        let muted = volume::is_muted(target);
        let windows_ready =
            mode == ipc::PlaybackMode::Windows && volume::selected_output_ready(mac);
        #[cfg(feature = "experimental-playback")]
        let native_address = playback::active_address();
        #[cfg(not(feature = "experimental-playback"))]
        let native_address = None;
        let output_ready = selected_audio_output_ready(
            mode,
            &playback_state,
            mac,
            windows_ready,
            native_address,
            self.connect_requested.explicit(),
        );
        let changed = {
            let mut s = self.state.lock().unwrap();
            if self.mac() != mac || s.playback_mode != mode {
                return;
            }
            let mut c = volume::update_readback(&mut s, vol, muted);
            if s.playback_state == playback_state {
                c |= s.audio_output_ready != output_ready;
                s.audio_output_ready = output_ready;
            }
            c
        };
        if changed {
            self.push_state();
        }
    }
}

/// Identify an AAP 0x2E host from local radios, then Windows pairing metadata.
/// Unpaired hosts remain "Other device" with their address; flags do not identify a model.
fn describe_device(addr: u64, local_radios: &[u64]) -> ipc::ConnectedDevice {
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
    ipc::ConnectedDevice {
        address,
        is_this_pc: false,
        name: "Other device".into(),
        kind: "unknown".into(),
    }
}

fn multipoint_log(devices: &[ipc::ConnectedDevice]) -> String {
    let local = devices.iter().filter(|device| device.is_this_pc).count();
    format!(
        "multipoint: {} device(s) on the AirPods, {local} this PC",
        devices.len()
    )
}

fn new_other_host(
    previous: Option<(u64, &[(u64, [u8; 2])])>,
    request: u64,
    devices: &[(u64, [u8; 2])],
    local_radios: &[u64],
) -> bool {
    previous.is_some_and(|(previous_request, previous)| {
        // A new explicit PC choice accepts the hosts already present at that point.
        !(request & 3 == 3 && request != previous_request)
            && devices.iter().any(|(address, _)| {
                !local_radios.contains(address) && !previous.iter().any(|(old, _)| old == address)
            })
    })
}

/// Ask the OS to connect/disconnect the audio, off-thread (the call blocks for
/// seconds) and serialized, so a stale request can never undo a newer one.
fn audio_request(ctx: &Ctx, mac: u64, connect: bool, why: &'static str) {
    if let Some(intent) = bt::begin_audio_request(connect) {
        dispatch_audio_request(ctx, mac, intent, why);
    }
}

fn arm_native_playback(ctx: &Ctx, mac: u64, intent: u64) -> io::Result<()> {
    #[cfg(feature = "experimental-playback")]
    if intent & 1 != 0 && driver::native_audio_bound(mac) {
        if !bt::audio_request_is_current(intent) || ctx.mac() != mac ||
            ctx.shutting_down.load(Ordering::Acquire) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "Playback request superseded"));
        }
        let driver = ctx.shared_driver.open(mac)?;
        let supported = driver.listen_playback(mac, intent)?;
        // Keep the exclusive PDO handle alive until the control receiver adopts it.
        *ctx.driver_cell.lock().unwrap() = Some(driver);
        log(if supported { "playback: selected-peer signaling listener armed" }
            else { "playback: driver uses legacy outgoing signaling" });
    }
    #[cfg(not(feature = "experimental-playback"))]
    let _ = (ctx, mac, intent);
    Ok(())
}

fn dispatch_audio_request(ctx: &Ctx, mac: u64, intent: u64, why: &'static str) {
    if let Err(error) = arm_native_playback(ctx, mac, intent) {
        log(&format!("playback: signaling listener unavailable: {error}"));
        return;
    }
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
    if !ctx.pending_audio_disconnect.lock().unwrap().is_pending() {
        return false;
    }
    match hfp_guard::after_release(|| finish_audio_disconnect_after_hfp(ctx)) {
        Ok(failed) => failed,
        Err(error) => {
            if let Some(intent) = ctx.pending_audio_disconnect.lock().unwrap().token {
                companion::profiles_closed(ctx, intent, false);
            }
            ctx.shutdown_cleanup_failed.store(true, Ordering::Release);
            log(&format!(
                "hands-free restoration before disconnect: {error}"
            ));
            true
        }
    }
}

fn finish_audio_disconnect_after_hfp(ctx: &Ctx) -> bool {
    let intent = ctx.pending_audio_disconnect.lock().unwrap().take();
    let Some((intent, needs_rebuild)) = intent else {
        return false;
    };
    if intent & 1 != 0 && needs_rebuild {
        // The receiver has closed AAP before reaching here. Keep it closed
        // until recovery finishes so a newer Connect cannot skip stereo repair.
        if let Err(error) = arm_native_playback(ctx, ctx.mac(), intent) {
            log(&format!("playback: signaling listener unavailable before rebuild: {error}"));
            return true;
        }
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
                    persist_microphone_enabled(ctx, false);
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
            companion::profiles_closed(ctx, intent, sink == 0 && handsfree == 0);
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

    fn blocks_playback(&self) -> bool {
        self.is_pending() || self.needs_rebuild
    }

    fn take_repair_if_ready(&self, repair: &AtomicBool) -> Option<bool> {
        (!self.blocks_playback()).then(|| repair.swap(false, Ordering::AcqRel))
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

/// Full-target recovery for an explicit request or the final microphone fallback.
/// Never use it for an ordinary connection or AAP timeout.
fn force_audio_rebuild(mac: u64, cleanup: bool) {
    let (disabled, enabled) = if cleanup {
        bt::reset_audio_services_for_cleanup(mac, false)
    } else {
        bt::reset_audio_services(mac, false)
    };
    log(&format!(
        "audio recovery: disable={disabled} enable={enabled}"
    ));
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
                persist_microphone_enabled(
                    ctx,
                    enabled && !ctx.shutting_down.load(Ordering::Acquire),
                );
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

fn persist_preference(name: &str, contents: &[u8]) -> io::Result<()> {
    let result = (|| {
        let base = std::env::var_os("LOCALAPPDATA")
            .ok_or_else(|| io::Error::other("Local app data is unavailable"))?;
        atomic_file::write(
            &std::path::Path::new(&base).join("NTPods").join(name),
            contents,
        )
    })();
    if let Err(error) = &result {
        log(&format!(
            "Could not save {name}: {:?} (OS {:?})",
            error.kind(),
            error.raw_os_error()
        ));
    }
    result
}

fn persist_microphone_enabled(ctx: &Ctx, enabled: bool) {
    if persist_preference("microphone-enabled", enabled.to_string().as_bytes()).is_err() {
        ctx.overlay("Could not save the microphone setting for the next start.");
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
    persist_microphone_policy(ctx, preferences);
}

fn persist_microphone_policy(ctx: &Ctx, preferences: String) {
    if persist_preference("microphone-policy.json", preferences.as_bytes()).is_err() {
        ctx.overlay("Could not save microphone behavior for the next start.");
    }
}

fn microphone_connection_allowed(state: &Snapshot, explicit: bool) -> bool {
    state.connected
        && (explicit
            || (state.playback_mode == ipc::PlaybackMode::Windows && state.audio_output_ready))
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
    } else if state.hardware_diagnostics_pending
        || state.hearing_metadata_pending
        || diagnostic_controls::active(state)
    {
        Err("Hardware checks are being read")
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
        let connection_requested =
            microphone_connection_allowed(&state, ctx.connect_requested.explicit())
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
        persist_microphone_enabled(ctx, true);
    }
    persist_microphone_policy(ctx, preferences);
    Ok(active)
}

fn set_microphone_manual(ctx: &Ctx, active: bool, expected: Option<MicrophoneRequest>) {
    let connected = {
        let state = ctx.state.lock().unwrap();
        microphone_connection_allowed(&state, ctx.connect_requested.explicit())
            && device_models::microphone(&state)
            && playback_allows_microphone(&state)
    };
    let _preference = ctx.mic_preference.lock().unwrap();
    if expected.is_some_and(|request| !request.current(ctx)) {
        return;
    }
    let permitted = ctx.auto_mode.load(Ordering::Acquire)
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
// recovery latches. Presence alone never authorizes an outgoing connection.
fn claim_windows_connection(
    windows_connected: bool,
    requested: &ConnectionRequest,
    user_disconnected: &AtomicBool,
    shutting_down: &AtomicBool,
    paired_target: Option<&PairedTarget>,
) -> bool {
    windows_connected
        && !user_disconnected.load(Ordering::Acquire)
        && !shutting_down.load(Ordering::Acquire)
        && pairing_allows_selection(paired_target, requested, None)
        && requested.claim()
}

fn publish_mic(ctx: &Ctx, mic: &mic_lifecycle::Lifecycle, ear_allowed: bool) {
    use mic_lifecycle::Phase;
    {
        let _preference = ctx.mic_preference.lock().unwrap();
        if let Some(quiet) = mic.quiet_for_windows() {
            ctx.hq_mic_quiet.store(quiet, Ordering::Release);
        }
    }
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

fn microphone_quiet_for_windows(
    preference: &Mutex<()>,
    mic_on: &AtomicBool,
    quiet: &AtomicBool,
) -> io::Result<bool> {
    let _preference = match preference.try_lock() {
        Ok(guard) => guard,
        Err(std::sync::TryLockError::WouldBlock) => return Ok(false),
        Err(std::sync::TryLockError::Poisoned(_)) => {
            return Err(io::Error::other(
                "Microphone reservation state is unavailable",
            ))
        }
    };
    Ok(!mic_on.load(Ordering::Acquire) && quiet.load(Ordering::Acquire))
}

fn microphone_permitted(ctx: &Ctx) -> bool {
    ctx.auto_mode.load(Ordering::Acquire)
        && ctx.pipe.is_open()
        && !ctx.user_disconnected.load(Ordering::Acquire)
        && !ctx.wants_reconnect.load(Ordering::Acquire)
        && !ctx.shutting_down.load(Ordering::Acquire)
        && {
            let state = ctx.state.lock().unwrap();
            microphone_connection_allowed(&state, ctx.connect_requested.explicit())
                && device_models::microphone(&state)
                && playback_allows_microphone(&state)
        }
}

// Caller holds mic_preference; neither this check nor its commit sends Bluetooth I/O.
fn microphone_request_current(ctx: &Ctx, revision: u64, now: u64, ear_allowed: bool) -> bool {
    #[cfg(feature = "experimental-playback")]
    if playback::windows_audio_suspended(ctx.mac()) {
        return false;
    }
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
    #[cfg(feature = "experimental-playback")]
    mic.set_windows_suspended(playback::windows_audio_suspended(ctx.mac()));
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
            let _companion_action = ctx.companion_actions.lock().unwrap();
            companion::microphone_started(ctx);
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
                        ctx.hq_mic_quiet.store(false, Ordering::Release);
                        mic.start_attempted();
                        // Reserve the stream before releasing the preference lock.
                        ctx.state.lock().unwrap().mic_state = "starting".into();
                    }
                },
            );
            let sent = granted == Ok(true) && driver.send(&aap::START_AUDIO).is_ok();
            #[cfg(feature = "experimental-playback")]
            mic.set_windows_suspended(playback::windows_audio_suspended(ctx.mac()));
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
                if granted == Ok(false) || mic.yielding_to_windows() {
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
            stats.finish(if mic.yielding_to_windows() {
                "windows-profile-yield"
            } else if timed_out {
                "pcm-timeout"
            } else {
                "stop-requested"
            });
            let sent = driver.send(&aap::STOP_AUDIO).is_ok();
            log(&format!(
                "mic stop: STOP_AUDIO sent={sent}; waiting for 500ms quiet (2000ms drain limit)"
            ));
            if !sent {
                ctx.state.lock().unwrap().mic_error = Some(if mic.yielding_to_windows() {
                    "Could not pause the high-quality microphone.".into()
                } else {
                    "Microphone stop failed; recovering the connection.".into()
                });
            }
        }
        Action::RetryStop => {
            silence_mic(ctx);
            let sent = driver.send(&aap::STOP_AUDIO).is_ok();
            let (elapsed, packets, age) = mic.stop_progress(now);
            log(&format!("mic stop: one STOP_AUDIO retry sent={sent}; elapsed={elapsed}ms packets_after_stop={packets} last_audio_age={age:?}ms"));
        }
        Action::ResetAudio => {
            #[cfg(feature = "experimental-playback")]
            if playback::windows_audio_suspended(ctx.mac()) {
                mic.set_windows_suspended(true);
                silence_mic(ctx);
                publish_mic(ctx, mic, ear_allowed);
                return false;
            }
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
            #[cfg(feature = "experimental-playback")]
            if playback::windows_audio_suspended(ctx.mac()) {
                mic.set_windows_suspended(true);
                silence_mic(ctx);
                publish_mic(ctx, mic, ear_allowed);
                return false;
            }
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

fn publish_nearby_finder(ctx: &Ctx, finder: &nearby_finder::Finder) {
    let now_ms = ctx.mic_clock.elapsed().as_millis() as u64;
    let snapshot = finder.snapshot(now_ms);
    let last_heard_tick = snapshot
        .last_heard_age_ms
        .map(|age| now_ms.saturating_sub(age).saturating_add(1))
        .unwrap_or(0);
    ctx.nearby_last_heard_tick
        .store(last_heard_tick, Ordering::Release);
    let mut state = ctx.state.lock().unwrap();
    state.nearby_finder = snapshot;
    drop(state);
    ctx.push_state();
}

fn start_nearby_finder(ctx: &Ctx, device_id: &str) -> Option<u64> {
    if device_id.starts_with("dongle-finder:") { return dongle_health::start_finder(ctx, device_id); }
    if ctx.shutting_down.load(Ordering::Acquire) {
        return None;
    }
    let (current_id, mic_busy) = {
        let state = ctx.state.lock().unwrap();
        (
            state.device_id.clone(),
            state.mic_capture_active
                || state.mic_uplink_active
                || ctx.capture_active.load(Ordering::Acquire)
                || ctx.mic_on.load(Ordering::Acquire),
        )
    };
    if current_id != device_id || device_id.is_empty() {
        return None;
    }
    let mut finder = ctx.nearby_finder.lock().unwrap();
    let owner = finder.start(device_id)?;
    finder.set_paused(mic_busy);
    publish_nearby_finder(ctx, &finder);
    Some(owner)
}

fn stop_nearby_finder(ctx: &Ctx, device_id: &str, owner: u64) -> bool {
    if device_id.starts_with("dongle-finder:") { return dongle_health::stop_finder(ctx, device_id, owner); }
    let mut finder = ctx.nearby_finder.lock().unwrap();
    if !finder.stop(device_id, owner) {
        return false;
    }
    publish_nearby_finder(ctx, &finder);
    true
}

fn clear_nearby_finder(ctx: &Ctx) {
    let mut finder = ctx.nearby_finder.lock().unwrap();
    if finder.clear() {
        publish_nearby_finder(ctx, &finder);
    }
}

fn refresh_nearby_identity(ctx: &Ctx, mac: u64, available: bool) {
    let mut finder = ctx.nearby_finder.lock().unwrap();
    if finder.set_identity(&format!("{mac:012x}"), available) {
        publish_nearby_finder(ctx, &finder);
    }
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

fn command_targets_selected_pair(device_id: Option<&str>, selected: &str) -> bool {
    match device_id {
        None => true,
        Some(id) => {
            id.len() == 12
                && bt::parse_pairing_address(id).is_some()
                && id.eq_ignore_ascii_case(selected)
        }
    }
}

fn set_device_control(ctx: &Ctx, id: u8, value: u32, device_id: Option<&str>) {
    let _write = ctx.settings_write.lock().unwrap();
    // Pair selection holds settings_write too; a queued action must not reach its replacement.
    if !command_targets_selected_pair(device_id, &ctx.state.lock().unwrap().device_id) {
        return;
    }
    let selected_mac = ctx.mac();
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
            || state.link_stale
            || ctx.user_disconnected.load(Ordering::Acquire)
            || ctx.shutting_down.load(Ordering::Acquire)
            || driver.is_none()
        {
            Some("Connect the AirPods before changing this setting.")
        } else {
            None
        };
        state.device_settings_error = error.map(str::to_owned);
        error.is_none().then(|| {
            let explicit_choice = device_models::explicit_choice(&state, id, value);
            let unreported = device_models::unreported_choice(&state, id, value);
            (
                pending.begin_control(&mut state, id, value, unreported),
                explicit_choice,
            )
        })
    };
    if let Some((token, explicit_choice)) = token {
        let driver = driver.unwrap();
        let sent = driver.send(&aap::device_control_command(id, value)).is_ok();
        let (save_off, save_choice) = {
            let mut pending = ctx.settings_pending.lock().unwrap();
            let mut state = ctx.state.lock().unwrap();
            let current = pending.finish_control(&mut state, id, value, token, sent) && sent;
            (
                current && id == 0x34
                    && device_settings::supports_silent_allow_off(&state.model)
                    && pending.can_remember_choice(&state, id, value, token),
                current && explicit_choice && pending.can_remember_choice(&state, id, value, token),
            )
        };
        if save_off {
            if device_settings::save_allow_off_choice(selected_mac, value == 1).is_err() {
                ctx.state.lock().unwrap().device_settings_error =
                    Some("Sent the setting, but could not save it for reconnects.".into());
            } else {
                companion_prefs::record_allow_off(ctx, selected_mac, value == 1);
            }
        }
        if save_choice {
            if device_settings::save_control_choice(selected_mac, id, value).is_err() {
                ctx.state.lock().unwrap().device_settings_error = Some("Sent the setting, but could not save it for reconnects.".into());
            } else {
                companion_prefs::record_control(ctx, selected_mac, id, value);
            }
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
    mode: Option<hearing::Mode>,
    expected: Option<ipc::HearingAidSettings>,
    settings: Option<ipc::HearingAidSettings>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct HearingConfirmation {
    generation: u64,
    address: u64,
    connection: u64,
}

impl HearingConfirmation {
    fn matches(self, pending: Option<Self>, address: u64, connection: u64) -> bool {
        pending == Some(self) && address == self.address && connection == self.connection
    }
}

#[derive(Clone, Copy)]
enum SettingsRefresh {
    Manual,
    Hearing(HearingConfirmation),
}

impl SettingsRefresh {
    fn bypasses_cooldown(
        self,
        pending: Option<HearingConfirmation>,
        address: u64,
        connection: u64,
    ) -> bool {
        matches!(self, Self::Hearing(token) if token.matches(pending, address, connection))
    }
}

fn hearing_confirmation_current(ctx: &Ctx, token: HearingConfirmation) -> bool {
    token.matches(
        *ctx.hearing_confirmation.lock().unwrap(),
        ctx.mac(),
        ctx.connect_requested.snapshot(),
    ) && !ctx.user_disconnected.load(Ordering::Acquire)
        && !ctx.shutting_down.load(Ordering::Acquire)
}

fn confirm_hearing_mode(
    ctx: &Ctx,
    token: HearingConfirmation,
    verification: &hearing::ModeVerification,
) -> Result<Option<hearing::Applied>, String> {
    {
        let _write = ctx.settings_write.lock().unwrap();
        if !hearing::is_current(token.generation)
            || ctx.mac() != token.address
            || !ctx.connect_requested.is_current(token.connection)
        {
            return Ok(None);
        }
        *ctx.hearing_confirmation.lock().unwrap() = Some(token);
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut queued = false;
    loop {
        let _write = ctx.settings_write.lock().unwrap();
        if !hearing_confirmation_current(ctx, token) {
            return Ok(None);
        }
        if Instant::now() >= deadline {
            ctx.wants_settings_refresh.lock().unwrap().take();
            return Err("The mode change was sent, but the AirPods did not report its result. Refresh device settings to confirm it.".into());
        }
        let _preference = ctx.mic_preference.lock().unwrap();
        let mut state = ctx.state.lock().unwrap();
        if ctx.capture_active.load(Ordering::Acquire)
            || ctx.mic_on.load(Ordering::Acquire)
            || !matches!(state.mic_state.as_str(), "idle" | "unavailable")
            || ctx.pipe.status() != Some(0)
        {
            ctx.wants_settings_refresh.lock().unwrap().take();
            return Err("The mode change was sent. Finish recording and refresh device settings to confirm it.".into());
        }
        if state.connected
            && (!queued || hearing::session() != verification.session)
            && hearing::mode_reports_complete()
        {
            drop(state);
            let driver =
                ctx.driver_cell.lock().unwrap().clone().ok_or(
                    "The AirPods disconnected before the hearing modes could be confirmed.",
                )?;
            let generation = hearing::next_gen();
            drop(_preference);
            drop(_write);
            let result = hearing::verify_mode(&driver, generation, verification);
            match result {
                Ok(None) => continue,
                other => return other,
            }
        } else if !queued {
            queue_settings_refresh(
                &ctx.wants_settings_refresh,
                &mut state,
                Some(0),
                false,
                SettingsRefresh::Hearing(token),
            );
            if let Some(error) = &state.device_settings_error {
                return Err(error.clone());
            }
            queued = true;
        } else if ctx.wants_settings_refresh.lock().unwrap().is_none()
            && hearing::session() == verification.session
            && state.device_settings_error.is_some()
        {
            return Err(state.device_settings_error.clone().unwrap());
        }
        drop(state);
        drop(_preference);
        drop(_write);
        thread::sleep(Duration::from_millis(100));
    }
}

impl HearingRequest {
    fn command(&self) -> &'static str {
        match self.mode {
            Some(hearing::Mode::HearingAid) => "set_hearing_aid_mode",
            Some(hearing::Mode::MediaAssist) => "set_media_assist",
            None => "set_hearing_aid",
        }
    }
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
            request.mode.is_none()
                && request.on
                && (request.settings.is_none_or(|s| !s.is_valid())
                    || request.expected.is_none_or(|s| !s.is_valid()))
        }) {
            Some("Complete, valid audiograms and hearing settings are required.")
        } else if requested.is_some_and(|request| request.on && request.mode.is_none())
            && (state.hearing_aid_settings.is_none() || !hearing::has_current_read())
        {
            Some("Read the existing audiograms before enabling or editing hearing assistance.")
        } else if requested.is_some_and(|request| request.on && request.mode.is_none())
            && !hearing::can_edit_current_read()
        {
            Some("These hearing settings are read-only in NTPods.")
        } else if requested.is_some_and(|request| {
            request.mode.is_none()
                && request.on
                && !request.matches_baseline(state.hearing_aid_settings)
        }) {
            Some("Hearing settings changed since this edit began. Read them again before editing.")
        } else if requested.is_some_and(|request| request.on && request.mode.is_none())
            && hearing::reported_state().0.is_none()
        {
            Some("Reconnect to confirm whether hearing assistance is enabled before changing it.")
        } else {
            None
        };
        let generation = match error {
            Some(error) => Err(error.to_owned()),
            None => match requested.and_then(|request| request.mode.map(|mode| (mode, request.on)))
            {
                Some((mode, on)) => hearing::next_mode_gen(mode, on),
                None => Ok(hearing::next_gen()),
            },
        };
        state.hearing_aid_error = generation.as_ref().err().cloned();
        if let Err(error) = generation {
            if requested.is_none() {
                state.hearing_aid_settings = None;
            }
            if let Some(request) = requested {
                ctx.send_event(&Event::CommandResult {
                    command: request.command().into(),
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
            if ctx.hearing_confirmation.lock().unwrap().take().is_some() {
                ctx.wants_settings_refresh.lock().unwrap().take();
            }
            Some((
                driver.unwrap(),
                generation.unwrap(),
                authorized,
                ctx.mac(),
                ctx.connect_requested.snapshot(),
            ))
        }
    };
    ctx.push_state();
    if let Some((driver, gen, authorized, address, connection)) = work {
        let ctx = ctx.clone();
        thread::spawn(move || {
            let result = match requested {
                Some(request) if request.mode.is_some() => {
                    hearing::set_mode(&driver, gen, request.mode.unwrap(), request.on, authorized)
                }
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
            let token = HearingConfirmation {
                generation: gen,
                address,
                connection,
            };
            let confirming = matches!(&result, Ok(Some(hearing::Applied::Requested(_))));
            let result = match result {
                Ok(Some(hearing::Applied::Requested(verification))) => {
                    confirm_hearing_mode(&ctx, token, &verification)
                }
                other => other,
            };
            let _write = ctx.settings_write.lock().unwrap();
            if if confirming {
                !hearing_confirmation_current(&ctx, token)
            } else {
                !hearing::is_current(gen)
            } {
                if let Some(request) = requested {
                    ctx.send_event(&Event::CommandResult {
                        command: request.command().into(),
                        request_id: request.request_id,
                        ok: false,
                        message: String::new(),
                    });
                }
                return;
            }
            if confirming {
                *ctx.hearing_confirmation.lock().unwrap() = None;
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
                Ok(Some(hearing::Applied::Requested(_))) => unreachable!(),
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
                    command: request.command().into(),
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

#[cfg(any(test, feature = "hearing-test-research"))]
struct HearingProbeJob {
    owner: Arc<AtomicBool>,
    worker: thread::JoinHandle<bool>,
}

#[cfg(feature = "hearing-test-research")]
static HEARING_TEST_PROBE: Mutex<Option<HearingProbeJob>> = Mutex::new(None);

#[cfg(any(test, feature = "hearing-test-research"))]
fn drain_hearing_probe(
    jobs: &Mutex<Option<HearingProbeJob>>,
) -> (std::sync::MutexGuard<'_, Option<HearingProbeJob>>, bool) {
    let mut jobs = jobs.lock().unwrap();
    let restored = jobs.take().is_none_or(|job| {
        job.owner.store(false, Ordering::Release);
        job.worker.join().unwrap_or(false)
    });
    // Retain the reservation until teardown finishes; a new check must not race it.
    (jobs, restored)
}

#[cfg(feature = "hearing-test-research")]
fn pause_hearing_probe(ctx: &Ctx) -> std::sync::MutexGuard<'static, Option<HearingProbeJob>> {
    let (reservation, restored) = drain_hearing_probe(&HEARING_TEST_PROBE);
    if !restored {
        ctx.shutdown_cleanup_failed.store(true, Ordering::Release);
        log("hearing-test mode: cleanup could not verify restoration");
    }
    reservation
}

#[cfg(any(test, feature = "hearing-test-research"))]
fn hearing_test_probe_error(
    state: &Snapshot,
    device_id: &str,
    capturing: bool,
    changing: bool,
) -> Option<&'static str> {
    if !command_targets_selected_pair(Some(device_id), &state.device_id)
        || !state.connected
        || state.link_stale
    {
        Some("Connect the selected AirPods before checking hearing-test mode.")
    } else if !matches!(state.model.as_str(), "A3063" | "A3064" | "A3065") {
        Some("This hearing-test mode check requires AirPods Pro 3.")
    } else if changing
        || !state.audio_output_ready
        || !matches!(state.playback_state.as_str(), "ready" | "streaming")
    {
        Some("Wait for AirPods audio to finish reconnecting before checking hearing-test mode.")
    } else if capturing
        || state.mic_recording
        || state.mic_capture_active
        || state.mic_uplink_active
        || state.mic_state != "idle"
    {
        Some("Stop microphone use before checking hearing-test mode.")
    } else if state.hearing_aid_enabled != Some(false) || state.media_assist_enabled != Some(false)
    {
        Some("Confirm Hearing Aid and Media Assist are off before this check.")
    } else if state.hardware_diagnostics_pending
        || state.hearing_metadata_pending
        || diagnostic_controls::active(state)
    {
        Some("Wait for the device-data request to finish before checking hearing-test mode.")
    } else {
        None
    }
}

#[cfg(feature = "hearing-test-research")]
fn hearing_probe_transition_pending(ctx: &Ctx) -> bool {
    let changing = ctx.shutting_down.load(Ordering::Acquire)
        || ctx.user_disconnected.load(Ordering::Acquire)
        || ctx.wants_reconnect.load(Ordering::Acquire)
        || ctx
            .pending_audio_disconnect
            .lock()
            .unwrap()
            .blocks_playback();
    #[cfg(feature = "experimental-playback")]
    let changing = changing || ctx.audio_change_busy.load(Ordering::Acquire);
    changing
}

fn probe_hearing_test_mode(
    ctx: &Ctx,
    request_id: u64,
    device_id: String,
    enable_check: bool,
) -> Option<Arc<AtomicBool>> {
    let report = |result| {
        ctx.send_event(&Event::HearingTestModeProbe {
            request_id,
            device_id: device_id.clone(),
            enable_check,
            result,
        })
    };
    #[cfg(not(feature = "hearing-test-research"))]
    {
        report(ipc::HearingTestModeProbeResult {
            error: Some("Hearing-test mode checks require a research build.".into()),
            ..Default::default()
        });
        None
    }
    #[cfg(feature = "hearing-test-research")]
    {
        let mut jobs = HEARING_TEST_PROBE.lock().unwrap();
        if jobs.as_ref().is_some_and(|job| !job.worker.is_finished()) {
            report(ipc::HearingTestModeProbeResult {
                error: Some("A hearing-test mode check is already running.".into()),
                ..Default::default()
            });
            return None;
        }
        if let Some(previous) = jobs.take() {
            if !previous.worker.join().unwrap_or(false) {
                ctx.shutdown_cleanup_failed.store(true, Ordering::Release);
            }
        }
        let driver = ctx.driver_cell.lock().unwrap().clone();
        let state = ctx.state.lock().unwrap().clone();
        let error = hearing_test_probe_error(
            &state,
            &device_id,
            ctx.capture_active.load(Ordering::Acquire) || ctx.mic_on.load(Ordering::Acquire),
            hearing_probe_transition_pending(ctx),
        );
        if let Some(error) = error.or_else(|| {
            driver
                .is_none()
                .then_some("AirPods control channel is unavailable.")
        }) {
            report(ipc::HearingTestModeProbeResult {
                error: Some(error.into()),
                ..Default::default()
            });
            return None;
        }
        let alive = Arc::new(AtomicBool::new(true));
        let owner = alive.clone();
        let worker_ctx = ctx.clone();
        let target = device_id.clone();
        let result = thread::Builder::new()
            .name("hearing-test-mode-check".into())
            .spawn(move || {
                let _write = worker_ctx.settings_write.lock().unwrap();
                let _preference = worker_ctx.mic_preference.lock().unwrap();
                let cancelled = || {
                    let current = worker_ctx.state.lock().unwrap();
                    !owner.load(Ordering::Acquire)
                        || worker_ctx.shutting_down.load(Ordering::Acquire)
                        || worker_ctx.user_disconnected.load(Ordering::Acquire)
                        || hearing_test_probe_error(
                            &current,
                            &target,
                            worker_ctx.capture_active.load(Ordering::Acquire)
                                || worker_ctx.mic_on.load(Ordering::Acquire),
                            hearing_probe_transition_pending(&worker_ctx),
                        )
                        .is_some()
                        || current.volume != state.volume
                        || current.muted != state.muted
                };
                let result = hearing_test_mode::probe(&driver.unwrap(), enable_check, cancelled);
                let restored = !enable_check
                    || result.restoration_verified
                    || (!result.enable_sent
                        && !result.disable_sent
                        && result.initial_state == result.final_state);
                worker_ctx.send_event(&Event::HearingTestModeProbe {
                    request_id,
                    device_id: target,
                    enable_check,
                    result,
                });
                restored
            });
        match result {
            Ok(worker) => {
                *jobs = Some(HearingProbeJob {
                    owner: alive.clone(),
                    worker,
                })
            }
            Err(error) => {
                report(ipc::HearingTestModeProbeResult {
                    error: Some(error.to_string()),
                    ..Default::default()
                });
                return None;
            }
        }
        Some(alive)
    }
}

fn hardware_diagnostics_request(ctx: &Ctx) {
    let work = {
        let _write = ctx.settings_write.lock().unwrap();
        let _preference = ctx.mic_preference.lock().unwrap();
        let driver = ctx.driver_cell.lock().unwrap().clone();
        let mut state = ctx.state.lock().unwrap();
        if state.hardware_diagnostics_pending {
            return;
        }
        let error = if diagnostic_controls::active(&state) {
            Some("Wait for the requested hardware check to finish.")
        } else if !state.connected
            || state.link_stale
            || driver.is_none()
            || ctx.user_disconnected.load(Ordering::Acquire)
            || ctx.shutting_down.load(Ordering::Acquire)
        {
            Some("Connect the AirPods before reading hardware checks.")
        } else {
            hardware_diagnostics_read_error(
                &state,
                ctx.capture_active.load(Ordering::Acquire) || ctx.mic_on.load(Ordering::Acquire),
                ctx.pipe.status(),
            )
        };
        state.hardware_diagnostics_error = error.map(str::to_owned);
        error.is_none().then(|| {
            state.hardware_diagnostics_pending = true;
            (
                driver.unwrap(),
                hardware_diagnostics::token(),
                state.model.clone(),
            )
        })
    };
    ctx.push_state();
    if let Some((driver, token, model)) = work {
        let ctx = ctx.clone();
        thread::spawn(move || {
            let result = hardware_diagnostics::read(&driver, token, &model);
            let _write = ctx.settings_write.lock().unwrap();
            let mut state = ctx.state.lock().unwrap();
            if !token.is_current() || !state.connected {
                return;
            }
            state.hardware_diagnostics_pending = false;
            match result {
                Ok(Some(value)) => {
                    state.hardware_diagnostics = Some(value);
                }
                Err(error) => {
                    state.hardware_diagnostics_error = Some(error);
                }
                Ok(None) => return,
            }
            drop(state);
            drop(_write);
            ctx.push_state();
        });
    }
}

fn hardware_diagnostics_read_error(
    state: &Snapshot,
    capturing: bool,
    streams: Option<u32>,
) -> Option<&'static str> {
    if !device_models::hearing_aid(state) {
        Some("Hardware checks are not supported by this model.")
    } else if capturing
        || state.mic_capture_active
        || state.mic_uplink_active
        || state.mic_recording
        || !matches!(state.mic_state.as_str(), "idle" | "unavailable")
        || streams.is_some_and(|count| count != 0)
    {
        Some("Stop recording and wait for microphone recovery before reading hardware checks.")
    } else if state.hearing_aid_pending
        || state.transparency_pending
        || state.loud_sound_reduction_pending
        || state.hearing_metadata_pending
    {
        Some("Wait for the current settings request to finish.")
    } else {
        None
    }
}

fn transparency_request(
    ctx: &Ctx,
    requested: Option<ipc::TransparencySettings>,
    expected: Option<ipc::TransparencySettings>,
) {
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
        } else if requested.is_some() && (expected.is_none() || expected != state.transparency) {
            Some("Custom Transparency changed since this edit began. Read the settings again.")
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
            let result = transparency::settings(
                &driver,
                token,
                requested.as_ref(),
                expected.as_ref(),
                hearing_supported,
            );
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
    ctx.case_observed_tick.store(0, Ordering::Release);
    ctx.hr_generation.fetch_add(1, Ordering::AcqRel);
    let saved_allow_off = companion_prefs::allow_off_choice(ctx, ctx.mac(), device_settings::load_allow_off_choice(ctx.mac()));
    let mut pending = ctx.settings_pending.lock().unwrap();
    let mut state = ctx.state.lock().unwrap();
    let confirming = ctx
        .hearing_confirmation
        .lock()
        .unwrap()
        .is_some_and(|token| {
            ctx.mac() == token.address && ctx.connect_requested.is_current(token.connection)
        });
    pending.reset(&mut state);
    state.aacp_capabilities.clear();
    state.hearing_aid_pending = confirming;
    if device_settings::supports_silent_allow_off(&state.model) {
        state.allow_off_requested = saved_allow_off;
    }
}

fn load_control_choices_async(ctx: &Ctx) {
    let (address, session, model, firmware) = {
        let pending = ctx.settings_pending.lock().unwrap();
        let state = ctx.state.lock().unwrap();
        if !state.connected || device_models::find(&state.model).is_none() {
            return;
        }
        (
            ctx.mac(),
            pending.session(),
            state.model.clone(),
            state.firmware.clone(),
        )
    };
    let ctx = ctx.clone();
    thread::spawn(move || {
        let saved = companion_prefs::choices(&ctx, address, device_settings::load_control_choices(address));
        if saved.is_empty() {
            return;
        }
        let pending = ctx.settings_pending.lock().unwrap();
        let mut state = ctx.state.lock().unwrap();
        if ctx.mac() != address
            || pending.session() != session
            || !state.connected
            || state.model != model
            || state.firmware != firmware
        {
            return;
        }
        let changed = device_settings::apply_control_choices(&mut state, &pending, saved);
        drop(state);
        drop(pending);
        if changed {
            ctx.push_state();
        }
    });
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
    requested: &Mutex<Option<SettingsRefresh>>,
    state: &mut Snapshot,
    capture_streams: Option<u32>,
    capture_active: bool,
    reason: SettingsRefresh,
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
    *requested.lock().unwrap() = state.device_settings_error.is_none().then_some(reason);
}

/// Only the receiver calls this, before it can start a microphone stream.
fn take_settings_refresh(ctx: &Ctx, mic: &mic_lifecycle::Lifecycle) -> bool {
    let requested = ctx.wants_settings_refresh.lock().unwrap().take();
    let Some(requested) = requested else {
        return false;
    };
    // Query the driver: a new capture may not be in the 100 ms polled state yet.
    let capture_streams = ctx.pipe.status();
    let idle = settings_refresh_allowed(
        mic,
        capture_streams,
        ctx.capture_active.load(Ordering::Acquire),
    );
    let mut state = ctx.state.lock().unwrap();
    let confirming = requested.bypasses_cooldown(
        *ctx.hearing_confirmation.lock().unwrap(),
        ctx.mac(),
        ctx.connect_requested.snapshot(),
    );
    if matches!(requested, SettingsRefresh::Hearing(_)) && !confirming {
        return false;
    }
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
    } else if !confirming && last.is_some_and(|t| t.elapsed() < Duration::from_secs(5)) {
        Some("Wait a few seconds before refreshing settings again.".into())
    } else {
        // Hearing confirmation retains its driver handle, so it only reopens AACP/ATT.
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
        driver_state: Option<u32>,
        windows_connected: bool,
        mic_busy: bool,
        playback_active: bool,
        data_age: Duration,
    ) -> bool {
        if driver_state == Some(2) {
            self.disconnected_checks = 0;
            self.stale = false;
        } else if !windows_connected || (driver_state == Some(0) && !mic_busy && !playback_active) {
            // Retiring controls also releases audio intent; preserve an existing stream.
            self.disconnected_checks += 1;
        } else {
            self.disconnected_checks = 0;
            // Quiet AAP alone is normal; require a transport failure as well.
            if !mic_busy && data_age >= Duration::from_secs(30) {
                self.stale = true;
            }
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
            || state.playback_mode != ipc::PlaybackMode::Windows
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

// Caller holds state and mic_preference, as for an explicit Connect.
fn request_audio_connection(ctx: &Ctx, state: &mut Snapshot, why: &'static str) -> bool {
    let mut pending = ctx.pending_audio_disconnect.lock().unwrap();
    let Some(token) = bt::begin_audio_request(true) else {
        return false;
    };
    companion::local_connected(ctx);
    ctx.case_return.lock().unwrap().cancel();
    ctx.user_disconnected.store(false, Ordering::Release);
    ctx.connect_requested.store(true, Ordering::Release);
    state.mic_error = None;
    reserve_native_playback_transition(state);
    if let Some(token) = pending.queue_connect(token) {
        dispatch_audio_request(ctx, ctx.mac(), token, why);
    }
    true
}

fn resume_case_return(ctx: &Ctx, address: u64, received_ms: u64) {
    if dongle::inhibits(ctx, false) { return; }
    let _companion_action = ctx.companion_actions.lock().unwrap();
    #[cfg(feature = "hearing-test-research")]
    let Ok(probe) = HEARING_TEST_PROBE.try_lock() else {
        return;
    };
    #[cfg(feature = "hearing-test-research")]
    if probe.is_some() {
        return;
    }
    let finder = ctx.nearby_finder.lock().unwrap();
    if finder.active() {
        return;
    }
    let _preference = ctx.mic_preference.lock().unwrap();
    let mut state = ctx.state.lock().unwrap();
    if dongle::inhibits_native(&state.dongle, false)
        || ctx.mac() != address
        || ctx.connect_requested.explicit()
        || ctx.user_disconnected.load(Ordering::Acquire)
        || ctx.shutting_down.load(Ordering::Acquire)
        || !state.playback_native_driver
        || diagnostic_controls::active(&state)
        || !pairing_handoff_allowed(
            &state,
            ctx.capture_active.load(Ordering::Acquire),
            ctx.mic_on.load(Ordering::Acquire),
        )
        || ctx.pending_audio_disconnect.lock().unwrap().is_pending()
        || ctx.paired_target.lock().unwrap().is_some()
        || !ctx.case_return.lock().unwrap().take_open(
            address,
            ctx.connect_requested.snapshot(),
            true,
            received_ms,
            ctx.mic_clock.elapsed().as_millis() as u64,
        )
    {
        return;
    }
    if request_audio_connection(ctx, &mut state, "case return") {
        log("connection: resuming the previous PC session after case wake");
    }
    drop(state);
    drop(_preference);
    drop(finder);
    ctx.push_state();
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

fn pairing_handoff_allowed(state: &Snapshot, capture_active: bool, mic_on: bool) -> bool {
    // Switching pairs closes the microphone's recovery channel on either playback driver.
    !capture_active
        && !mic_on
        && !state.mic_capture_active
        && !state.mic_uplink_active
        && !matches!(
            state.mic_state.as_str(),
            "starting" | "capturing" | "stopping" | "recovering"
        )
        && state.playback_buffer_target_ms.is_none()
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
        Command::Connect { .. }
        | Command::RebuildAudio
        | Command::RepairConnection
        | Command::SetPlaybackMode { .. }
        | Command::RetryPlayback
        | Command::SetPlaybackBitrate { .. }
        | Command::SetPlaybackLink { .. }
        | Command::SetPlaybackGameMode { .. }
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

fn disconnect_device(ctx: &Ctx) {
    clear_nearby_finder(ctx);
    cancel_microphone_request(ctx);
    hearing::invalidate_session();
    silence_mic(ctx);
    {
        let _state = ctx.state.lock().unwrap();
        ctx.case_return.lock().unwrap().cancel();
        ctx.wants_settings_refresh.lock().unwrap().take();
        ctx.user_disconnected.store(true, Ordering::Release);
        ctx.connect_requested.store(false, Ordering::Release);
        let token = bt::begin_audio_request(false);
        if let Some(token) = token {
            ctx.pending_audio_disconnect.lock().unwrap().queue_disconnect(token);
        }
        companion::disconnect_reserved(ctx, ctx.connect_requested.snapshot(), token);
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
        companion::control_closed(ctx, ctx.mac(), true);
        let cleanup = ctx.clone();
        thread::spawn(move || finish_audio_disconnect(&cleanup));
    }
}

fn apply_command(ctx: &Ctx, cmd: Command) {
    // Commands may contain personal settings; do not log payloads.
    if ctx.shutting_down.load(Ordering::Acquire) {
        return;
    }
    if dongle::apply(ctx, &cmd) { return; }
    if dongle_health::apply(ctx, &cmd) { return; }
    if companion_prefs::apply_local(ctx, &cmd) || companion::apply_local(ctx, &cmd) {
        return;
    }
    let _companion_action = companion::serializes(&cmd)
        .then(|| ctx.companion_actions.lock().unwrap());
    companion::local_override(ctx, &cmd);
    if let Command::PairingCompleted { address } = &cmd {
        #[cfg(feature = "hearing-test-research")]
        let _probe_reservation = pause_hearing_probe(ctx);
        begin_pairing_handoff(ctx, address);
        return;
    }
    if matches!(&cmd, Command::Connect { device_id }
        if !command_targets_selected_pair(device_id.as_deref(), &ctx.state.lock().unwrap().device_id))
    {
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
    #[cfg(feature = "hearing-test-research")]
    let _probe_reservation = matches!(
        &cmd,
        Command::Shutdown
            | Command::Disconnect
            | Command::Connect { .. }
            | Command::RepairConnection
            | Command::RebuildAudio
    )
    .then(|| pause_hearing_probe(ctx));
    if ctx.shutting_down.load(Ordering::Acquire) {
        return;
    }
    // Reserve the check through intent creation; microphone START uses this lock too.
    let mut reconnect_guard = if matches!(
        &cmd,
        Command::Connect { .. } | Command::RebuildAudio | Command::RepairConnection
    ) {
        let guard = ctx.mic_preference.lock().unwrap();
        let mut state = ctx.state.lock().unwrap();
        // Pair selection also holds mic_preference. Keep this guard until the
        // connect intent is reserved so a queued island action cannot retarget.
        if matches!(&cmd, Command::Connect { device_id }
            if !command_targets_selected_pair(device_id.as_deref(), &state.device_id))
        {
            return;
        }
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
        Command::DongleControl { .. } | Command::DongleHealth { .. } => unreachable!(),
        Command::SetDongleAudioProfile { .. } | Command::GetDonglePerformance => {},
        Command::CompanionLink { .. } | Command::CompanionStatus
        | Command::CompanionSwitch { .. } | Command::CompanionForget
        | Command::CompanionSetPreference { .. } | Command::CompanionCallConsent { .. } => unreachable!(),
        Command::GetLoudSoundReduction => loud_sound_request(ctx, None),
        Command::GetTransparency => transparency_request(ctx, None, None),
        Command::SetTransparency { settings, expected } => {
            transparency_request(ctx, Some(settings), expected)
        }
        Command::SetLoudSoundReduction { on } => loud_sound_request(ctx, Some(on)),
        Command::Ping => {}
        Command::Hello { .. } | Command::GetState => ctx.push_state(),
        Command::SetAnc { mode, device_id } => {
            set_device_control(ctx, 0x0D, mode.into(), device_id.as_deref())
        }
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
        Command::SetPlaybackLink { settings } => {
            #[cfg(feature = "experimental-playback")]
            playback::set_link(ctx, settings, playback_request.unwrap());
            #[cfg(not(feature = "experimental-playback"))]
            {
                let _ = settings;
                ctx.state.lock().unwrap().playback_error =
                    Some("Playback codecs are unavailable in this build.".into());
                ctx.push_state();
            }
        }
        Command::SetPlaybackGameMode { enabled } => {
            #[cfg(feature = "experimental-playback")]
            playback::set_game_mode(ctx, enabled, playback_request.unwrap());
            #[cfg(not(feature = "experimental-playback"))]
            {
                let _ = enabled;
                ctx.state.lock().unwrap().playback_error =
                    Some("Playback codecs are unavailable in this build.".into());
                ctx.push_state();
            }
        }
        Command::SetFeature { feature, on } => {
            if device_settings::is_toggle(feature) {
                set_device_control(
                    ctx,
                    feature,
                    device_settings::toggle_value(feature, on),
                    None,
                );
            } else {
                ctx.state.lock().unwrap().device_settings_error =
                    Some("This device setting is not a toggle.".into());
                ctx.push_state();
            }
        }
        Command::SetControl { id, value } => {
            let value = u32::from(value) | if id == 0x1F { 0x5000 } else { 0 };
            set_device_control(ctx, id, value, None);
        }
        Command::SetDeviceControl {
            id,
            value,
            device_id,
        } => set_device_control(ctx, id, value, device_id.as_deref()),
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
                persist_preference("auto-pause", on.to_string().as_bytes())
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
                persist_preference(
                    "battery-scan-while-connected",
                    enabled.to_string().as_bytes(),
                )
            };
            if saved.is_err() {
                ctx.overlay("Could not save nearby battery updates for the next start.");
            }
            ctx.push_state();
        }
        Command::SetConversationVolume { percent } => {
            let saved = update_conversation_volume(&ctx.conv_duck, &ctx.state, percent, || {
                persist_preference("conversation-volume", percent.to_string().as_bytes())
            });
            if saved.is_err() {
                ctx.overlay("Could not save conversation volume for the next start.");
            }
            ctx.push_state();
        }
        Command::StepVolume { delta } => {
            let target = ctx.volume_target();
            {
                let mut duck = ctx.conv_duck.lock().unwrap();
                duck.manual_override(volume::step(target, delta).as_deref());
            }
            ctx.sync_volume();
        }
        Command::SetVolume { percent } => {
            let target = ctx.volume_target();
            {
                let mut duck = ctx.conv_duck.lock().unwrap();
                duck.manual_override(volume::set(target, percent.min(100)).as_deref());
            }
            ctx.sync_volume();
        }
        Command::ToggleMute => {
            let target = ctx.volume_target();
            {
                let mut duck = ctx.conv_duck.lock().unwrap();
                duck.manual_override(volume::toggle_mute(target).as_deref());
            }
            ctx.sync_volume();
        }
        Command::SetHeartRate { on } => set_heart_rate(ctx, on),
        Command::SetHeadTracking { on } => {
            set_head_tracking(ctx, on);
        }
        // These require command-pipe ownership and are handled by ipc_server.
        Command::StartNearbyFinder { .. } | Command::StopNearbyFinder { .. } => {}
        Command::ProbeHearingTestMode { .. } => {}
        Command::GetHearingAid => hearing_request(ctx, None),
        Command::GetHardwareDiagnostics => hardware_diagnostics_request(ctx),
        Command::GetHearingMetadata { device_id } => {
            diagnostic_controls::read_metadata(ctx, &device_id)
        }
        Command::StartHardwareCheck {
            request_id,
            device_id,
            buds_in_case_confirmed,
        } => diagnostic_controls::start_check(ctx, request_id, &device_id, buds_in_case_confirmed),
        Command::CancelHardwareCheck {
            request_id,
            device_id,
        } => diagnostic_controls::cancel_check(ctx, request_id, &device_id),
        Command::SetHearingAidMode { request_id, on } => hearing_request(
            ctx,
            Some(HearingRequest {
                request_id,
                on,
                mode: Some(hearing::Mode::HearingAid),
                expected: None,
                settings: None,
            }),
        ),
        Command::SetMediaAssist { request_id, on } => hearing_request(
            ctx,
            Some(HearingRequest {
                request_id,
                on,
                mode: Some(hearing::Mode::MediaAssist),
                expected: None,
                settings: None,
            }),
        ),
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
                    mode: None,
                    expected,
                    settings,
                }),
            );
        }
        Command::PairingCompleted { address } => {
            begin_pairing_handoff(ctx, &address);
        }
        Command::Connect { .. } => {
            {
                let mut state = ctx.state.lock().unwrap();
                #[cfg(feature = "experimental-playback")]
                if playback_request.is_some_and(|request| bt::playback_intent() != request.0) {
                    return;
                }
                request_audio_connection(ctx, &mut state, "user");
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
                if let Err(error) = arm_native_playback(ctx, ctx.mac(), intent) {
                    state.playback_error = Some(error.to_string());
                    return;
                }
                companion::local_connected(ctx);
                ctx.user_disconnected.store(false, Ordering::Release);
                ctx.case_return.lock().unwrap().cancel();
                ctx.connect_requested.store(true, Ordering::Release);
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
                ctx.case_return.lock().unwrap().cancel();
                ctx.connect_requested.store(true, Ordering::Release);
                ctx.wants_reconnect.store(true, Ordering::Release);
                state.mic_error = None;
                // Repair is also an explicit reconnect and must supersede a
                // queued Disconnect without skipping its playback recovery.
                let mut pending = ctx.pending_audio_disconnect.lock().unwrap();
                let Some(token) = bt::begin_audio_request(true) else {
                    return;
                };
                companion::local_connected(ctx);
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
                ctx.wants_settings_refresh.lock().unwrap().take();
                state.device_settings_error =
                    Some("Connect the AirPods before refreshing settings.".into());
            } else {
                queue_settings_refresh(
                    &ctx.wants_settings_refresh,
                    &mut state,
                    ctx.pipe.status(),
                    ctx.capture_active.load(Ordering::Acquire),
                    SettingsRefresh::Manual,
                );
            }
            drop(state);
            ctx.push_state();
        }
        Command::Disconnect => {
            disconnect_device(ctx);
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
            clear_nearby_finder(ctx);
            ctx.shutting_down.store(true, Ordering::Release);
            ctx.case_return.lock().unwrap().cancel();
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
                {
                    audio_done = bt::finish_audio_shutdown();
                    if audio_done {
                        break;
                    }
                }
                thread::sleep(Duration::from_millis(50));
            }
            let receiver_done = ctx.receiver_done.load(Ordering::Acquire)
                && ctx.playback_done.load(Ordering::Acquire);
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
        dongle: state.dongle.clone(),
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
        playback_link: state.playback_link,
        playback_game_mode: state.playback_game_mode,
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

fn begin_pairing_handoff(ctx: &Ctx, address: &str) {
    let Some(address) = bt::parse_pairing_address(address) else {
        return;
    };
    let request = {
        let _state = ctx.state.lock().unwrap();
        ctx.case_return.lock().unwrap().cancel();
        let request = ctx.connect_requested.renew();
        *ctx.paired_target.lock().unwrap() = Some(PairedTarget {
            address,
            request,
            stage: PairingStage::Validating,
        });
        request
    };
    let ctx = ctx.clone();
    thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if ctx.shutting_down.load(Ordering::Acquire)
                || !ctx.connect_requested.is_current(request)
            {
                return;
            }
            // Windows may finish installing the AAP service after PairAsync returns.
            if let Some(name) = bt::paired_aap_name(address) {
                #[cfg(feature = "experimental-playback")]
                let _pause = match playback::begin_automatic_pause(&ctx) {
                    Ok(pause) => {
                        if let Some(pause) = &pause {
                            if let Err(error) = pause.wait() {
                                log(&format!("pairing: could not restore playback: {error}"));
                                cancel_pairing_handoff(
                                    &ctx,
                                    request,
                                    "Could not restore playback. Try pairing again.",
                                );
                                return;
                            }
                        }
                        pause
                    }
                    Err(error) => {
                        log(&format!(
                            "pairing: could not pause buffer adjustment: {error}"
                        ));
                        cancel_pairing_handoff(
                            &ctx,
                            request,
                            "Could not restore playback. Try pairing again.",
                        );
                        return;
                    }
                };
                #[cfg(feature = "hearing-test-research")]
                let _probe_reservation = pause_hearing_probe(&ctx);
                let _mic = ctx.mic_preference.lock().unwrap();
                let state = ctx.state.lock().unwrap();
                if ctx.shutting_down.load(Ordering::Acquire)
                    || !ctx.connect_requested.is_current(request)
                {
                    return;
                }
                if !pairing_handoff_allowed(
                    &state,
                    ctx.capture_active.load(Ordering::Acquire),
                    ctx.mic_on.load(Ordering::Acquire),
                ) {
                    drop(state);
                    drop(_mic);
                    cancel_pairing_handoff(
                        &ctx,
                        request,
                        "Stop the microphone before connecting the paired AirPods.",
                    );
                    return;
                }
                ctx.connect_requested.store(false, Ordering::Release);
                ctx.user_disconnected.store(true, Ordering::Release);
                ctx.wants_reconnect.store(false, Ordering::Release);
                *ctx.paired_target.lock().unwrap() = Some(PairedTarget {
                    address,
                    request: ctx.connect_requested.snapshot(),
                    stage: PairingStage::Ready(name),
                });
                return;
            }
            if Instant::now() >= deadline {
                cancel_pairing_handoff(
                    &ctx,
                    request,
                    "Paired, but AirPods controls aren’t ready. Reconnect in Windows Bluetooth.",
                );
                return;
            }
            thread::sleep(Duration::from_millis(500));
        }
    });
}

fn cancel_pairing_handoff(ctx: &Ctx, request: u64, message: &str) {
    let _state = ctx.state.lock().unwrap();
    if ctx.connect_requested.is_current(request) && !ctx.shutting_down.load(Ordering::Acquire) {
        *ctx.paired_target.lock().unwrap() = None;
        ctx.overlay(message);
    }
}

// Receiver only, after the previous channel and its microphone cleanup have closed.
fn select_paired_device(ctx: &Ctx, mac: u64, name: &str, request: Option<u64>) -> bool {
    if request.is_none() && dongle::inhibits(ctx, false) { return false; }
    if mac == 0 || (mac == ctx.mac() && request.is_none()) {
        return false;
    }
    let keys = battery_ble::Keys::load(mac);
    let finder_identity_available = keys.is_some();
    #[cfg(feature = "experimental-playback")]
    let native_bound = driver::native_audio_bound(mac);
    let connected = bt::is_connected(mac);
    let _companion_action = ctx.companion_actions.lock().unwrap();
    let _write = ctx.settings_write.lock().unwrap();
    let _mic = ctx.mic_preference.lock().unwrap();
    let _hr = ctx.hr_commands.lock().unwrap();
    let mut pending = ctx.settings_pending.lock().unwrap();
    let mut diagnostic_job = ctx.hardware_check.lock().unwrap();
    let mut state = ctx.state.lock().unwrap();
    if state.connected
        || ctx.driver_cell.lock().unwrap().is_some()
        || ctx.shutting_down.load(Ordering::Acquire)
        || (ctx.user_disconnected.load(Ordering::Acquire) && request.is_none())
        || ctx.pending_audio_disconnect.lock().unwrap().is_pending()
        || ctx.wants_reconnect.load(Ordering::Acquire)
        || !pairing_allows_selection(
            ctx.paired_target.lock().unwrap().as_ref(),
            &ctx.connect_requested,
            request,
        )
        || request.is_some_and(|_| {
            !pairing_handoff_allowed(
                &state,
                ctx.capture_active.load(Ordering::Acquire),
                ctx.mic_on.load(Ordering::Acquire),
            )
        })
    {
        return false;
    }
    ctx.case_return.lock().unwrap().cancel();
    // Passive pair selection only cancels old audio work; it never authorizes playback.
    let intent = bt::begin_audio_request(request.is_some());
    if request.is_some() {
        ctx.user_disconnected.store(false, Ordering::Release);
    }
    ctx.connect_requested
        .store(request.is_some(), Ordering::Release);
    if request.is_none() && connected {
        ctx.connect_requested.claim();
    }
    let dispatch = if request.is_some() {
        let mut audio = ctx.pending_audio_disconnect.lock().unwrap();
        if mac != ctx.mac() {
            *audio = PendingAudioDisconnect::default();
        }
        intent.and_then(|intent| audio.queue_connect(intent))
    } else {
        None
    };
    ctx.hr_on.store(false, Ordering::Release);
    ctx.hr_generation.fetch_add(1, Ordering::AcqRel);
    ctx.hr_db.end_session();
    ctx.mic_demand.lock().unwrap().clear();
    ctx.pipe.revoke_stream();
    ctx.mic_on.store(false, Ordering::Release);
    hearing::invalidate_session();
    diagnostic_controls::select_pair(&mut diagnostic_job, &format!("{mac:012x}"));
    ctx.case_observed_tick.store(0, Ordering::Release);
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
    if let Some(request) = request {
        let mut target = ctx.paired_target.lock().unwrap();
        if let Some(target) = target.as_mut().filter(|target| target.request == request) {
            target.request = ctx.connect_requested.snapshot();
            target.stage = PairingStage::Selected;
        }
    }
    ctx.wants_settings_refresh.lock().unwrap().take();
    drop(state);
    drop(diagnostic_job);
    drop(pending);
    drop(_hr);
    drop(_mic);
    drop(_write);
    companion_prefs::initialize(ctx);
    refresh_nearby_identity(ctx, mac, finder_identity_available);
    rename::apply(name);
    ctx.push_state();
    if let Some(intent) = dispatch {
        dispatch_audio_request(ctx, mac, intent, "paired device");
    }
    true
}

fn receiver_idle_wait(
    mic_on: bool,
    head_tracking_enabled: bool,
    mic: &mic_lifecycle::Lifecycle,
    now: u64,
) -> Duration {
    let maximum = if mic_on || head_tracking_enabled {
        4
    } else {
        150
    };
    Duration::from_millis(u64::from(mic.wait_ms(now, maximum)))
}

fn initialize_control_session(
    current: impl Fn() -> bool,
    send: impl Fn(&[u8]) -> io::Result<()>,
    send_sensor: impl Fn(&[u8]) -> io::Result<()>,
    status: impl Fn() -> io::Result<u32>,
    pause: impl Fn(Duration),
    sequence: u16,
) -> bool {
    let descriptors = aap::request_all_descriptors(sequence, false);
    let packets: &[(&[u8], bool, bool, bool, Option<&str>)] = &[
        (&aap::HANDSHAKE, false, true, true, Some("handshake")),
        (
            &aap::SET_FEATURES,
            false,
            true,
            true,
            Some("feature request"),
        ),
        (
            &aap::REQUEST_NOTIFS,
            false,
            false,
            true,
            Some("notification request"),
        ),
        (
            &capabilities::REQUEST,
            false,
            false,
            false,
            Some("capability request"),
        ),
        (
            &aap::SOURCE_TYPE_MAC,
            false,
            false,
            false,
            Some("source identity"),
        ),
        (&descriptors, true, false, false, None),
        (&battery_ble::REQUEST_KEYS, false, false, false, None),
    ];
    for &(packet, sensor, wait, required, error_name) in packets {
        // Connect and initialization waits can outlive Disconnect or a newer request.
        if !current() {
            return false;
        }
        let result = if sensor {
            send_sensor(packet)
        } else {
            send(packet)
        };
        if let Err(error) = result {
            if let Some(name) = error_name {
                log(&format!("aap: {name} failed: {error}"));
            }
            if required {
                return false;
            }
        }
        if wait {
            pause(Duration::from_millis(300));
        }
    }
    if !current() {
        return false;
    }
    let transport = status();
    if !matches!(transport, Ok(2)) {
        log(&format!(
            "aap: initialization transport unavailable: {transport:?}"
        ));
        return false;
    }
    current()
}

fn release_failed_connection_request(
    requested: &ConnectionRequest,
    failures: &mut u32,
    request: u64,
) -> bool {
    *failures += 1;
    if *failures < 12 {
        return false;
    }
    *failures = 0;
    requested.release(request)
}

#[cfg(feature = "experimental-playback")]
fn arm_native_control_attempt(
    current: impl Fn() -> bool,
    arm: impl FnOnce() -> io::Result<bool>,
) -> io::Result<bool> {
    let superseded = || io::Error::new(io::ErrorKind::Interrupted, "Playback startup superseded");
    if !current() { return Err(superseded()); }
    let supported = arm()?;
    if !current() { return Err(superseded()); }
    Ok(supported)
}

#[cfg(feature = "experimental-playback")]
fn prime_handsfree_once(
    primed: &mut Option<u64>,
    request: u64,
    audio_intent: u64,
    mut current: impl FnMut() -> io::Result<()>,
    connect: impl FnOnce(&mut dyn FnMut() -> io::Result<()>) -> io::Result<hfp_endpoints::ControlStatus>,
) -> io::Result<Option<hfp_endpoints::ControlStatus>> {
    if request & 3 != 3 || audio_intent & 1 == 0 || *primed == Some(request) {
        return Ok(None);
    }
    current()?;
    *primed = Some(request);
    connect(&mut current).map(Some)
}

fn run_receiver(ctx: Ctx) {
    log("run_receiver: entered");
    if ctx.mac() == 0 {
        let found = discover_until_ready(
            || ctx.shutting_down.load(Ordering::Acquire),
            || {
                ctx.paired_target
                    .lock()
                    .unwrap()
                    .as_ref()
                    .filter(|target| target.current(&ctx.connect_requested))
                    .and_then(|target| match &target.stage {
                        PairingStage::Ready(name) => Some((target.address, name.clone())),
                        _ => None,
                    })
                    .or_else(bt::find_airpods)
            },
            || thread::park_timeout(Duration::from_secs(2)),
        );
        let Some((mac, name)) = found else {
            ctx.receiver_done.store(true, Ordering::Release);
            return;
        };
        select_paired_device(&ctx, mac, &name, None);
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
    let mut last_devices: Option<(u64, Vec<(u64, [u8; 2])>)> = None;
    let local_radios = bt::local_radio_addresses();
    log(&format!("bt: local radio count={}", local_radios.len()));
    // Bound connection retries, then require a fresh request or an existing Windows link.
    let mut reach_fails = 0u32;
    // Throttle checks for a restored Windows connection.
    let mut resume_poll = None;
    // Consecutive `Driver::open()` failures. Distinct from `reach_fails`: this
    // one is about the *devnode*, not the AirPods.
    let mut open_fails = 0u32;
    let give_up = |ctx: &Ctx, fails: &mut u32, request: u64| {
        let _state = ctx.state.lock().unwrap();
        if release_failed_connection_request(&ctx.connect_requested, fails, request) {
            let mut target = ctx.paired_target.lock().unwrap();
            if target
                .as_ref()
                .is_some_and(|target| target.request == request)
            {
                *target = None;
                ctx.user_disconnected.store(true, Ordering::Release);
            }
            log("run_receiver: gave up reaching AirPods — releasing");
        }
    };
    let mut previous_request = None;
    #[cfg(feature = "experimental-playback")]
    let mut primed_handsfree = None;
    let mut discovery_poll = None;
    loop {
        // No channel exists here, including before the first successful connect.
        companion::control_closed(&ctx, ctx.mac(), true);
        finish_audio_disconnect(&ctx);
        if ctx.shutting_down.load(Ordering::Acquire) {
            ctx.receiver_done.store(true, Ordering::Release);
            return;
        }
        let target = {
            let _state = ctx.state.lock().unwrap();
            let mut target = ctx.paired_target.lock().unwrap();
            if target
                .as_ref()
                .is_some_and(|target| !target.current(&ctx.connect_requested))
            {
                *target = None;
            }
            target.clone()
        };
        let mut selected = false;
        if let Some(target) = &target {
            if matches!(target.stage, PairingStage::Validating) {
                thread::sleep(Duration::from_millis(500));
                continue;
            }
            if let PairingStage::Ready(name) = &target.stage {
                selected = select_paired_device(&ctx, target.address, name, Some(target.request));
            }
        }
        if ctx.user_disconnected.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(500));
            continue;
        }
        if target.is_none()
            && cooldown_elapsed(discovery_poll, Instant::now(), Duration::from_secs(2))
        {
            discovery_poll = Some(Instant::now());
            if let Some((next, name)) = bt::find_airpods_for(ctx.mac()) {
                selected = next != ctx.mac() && select_paired_device(&ctx, next, &name, None);
            }
        }
        if selected {
            mic_ears = mic_lifecycle::WearState::default();
            hr_decoder.reset();
            last_anc = 0;
            last_devices = None;
            reach_fails = 0;
            open_fails = 0;
            resume_poll = None;
            previous_request = None;
            #[cfg(feature = "experimental-playback")]
            {
                primed_handsfree = None;
            }
        }
        let mac = ctx.mac();
        // Reopen AAP when Windows reconnects, even if a phone call exhausted retries.
        // BLE presence alone cannot signal return while the buds keep advertising.
        // Explicit Disconnect blocks this until Connect or Repair.
        if dongle::inhibits(&ctx, ctx.connect_requested.explicit()) {
            thread::sleep(Duration::from_millis(500));
            continue;
        }
        if !ctx.connect_requested.load(Ordering::Relaxed) {
            if !ctx.user_disconnected.load(Ordering::Relaxed)
                && !devnode::restart_required(mac)
                && cooldown_elapsed(resume_poll, Instant::now(), Duration::from_secs(5))
            {
                resume_poll = Some(Instant::now());
                let windows_connected = bt::is_connected(mac);
                if windows_connected {
                    let _companion_action = ctx.companion_actions.lock().unwrap();
                    let _state = ctx.state.lock().unwrap();
                    let previous = ctx.connect_requested.snapshot();
                    if claim_windows_connection(
                        windows_connected,
                        &ctx.connect_requested,
                        &ctx.user_disconnected,
                        &ctx.shutting_down,
                        ctx.paired_target.lock().unwrap().as_ref(),
                    ) {
                        ctx.case_return.lock().unwrap().passive_claim(
                            mac,
                            previous,
                            ctx.connect_requested.snapshot(),
                        );
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
                || !pairing_allows_connect(
                    ctx.paired_target.lock().unwrap().as_ref(),
                    &ctx.connect_requested,
                    mac,
                )
            {
                continue;
            }
            let mut pending = ctx.pending_audio_disconnect.lock().unwrap();
            let Some(repair) = pending.take_repair_if_ready(&ctx.wants_reconnect) else {
                // A newer Connect/Repair arrived while the previous reset ran.
                // Consume that rebuild before opening AAP or reserving another token.
                if !pending.is_pending() && ctx.connect_requested.explicit() {
                    // Preserve the newer explicit request while satisfying the saved repair.
                    if let Some(token) = bt::begin_audio_request(true) {
                        if let Err(error) = arm_native_playback(&ctx, mac, token) {
                            log(&format!("playback: signaling listener unavailable after repair: {error}"));
                            continue;
                        }
                        let _ = pending.queue_connect(token);
                    }
                }
                continue;
            };
            if repair && ctx.connect_requested.explicit() && !bt::is_connected(mac) {
                audio_request(&ctx, ctx.mac(), true, "repair");
            }
            ctx.connect_requested.snapshot()
        };
        if previous_request != Some(connection_request) {
            reach_fails = 0;
            previous_request = Some(connection_request);
            if devnode::restart_required(mac) {
                ctx.overlay("Restart Windows to finish recovering the AirPods driver.");
            }
        }
        // Opening a new handle during PnP restart can keep the old driver loaded.
        if devnode::recovering() {
            thread::sleep(Duration::from_millis(250));
            continue;
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
                if open_fails >= 4 && bt::is_connected(mac) {
                    let recovery_ctx = ctx.clone();
                    if ctx.shared_driver.request_recovery(mac, move |result| {
                        log(&format!("AAP driver recovery completed: {result:?}"));
                        if recovery_ctx.mac() != mac
                            || recovery_ctx.shutting_down.load(Ordering::Acquire)
                            || recovery_ctx.user_disconnected.load(Ordering::Acquire)
                            || recovery_ctx.state.lock().unwrap().connected
                        {
                            return;
                        }
                        match result {
                            devnode::RecoveryResult::RestartRequired => {
                                if devnode::restart_required(mac) {
                                    recovery_ctx.overlay(
                                        "Restart Windows to finish recovering the AirPods driver.",
                                    );
                                }
                            }
                            devnode::RecoveryResult::Failed => recovery_ctx.overlay(
                                "Could not recover the AirPods driver. Try Repair in NTPods.",
                            ),
                            devnode::RecoveryResult::Ready => {}
                        }
                    }) {
                        log("run_receiver: requested a targeted AAP problem-code check and recovery");
                        ctx.overlay("Recovering the AirPods driver…");
                    }
                }
                give_up(&ctx, &mut reach_fails, connection_request);
                thread::sleep(Duration::from_millis(1500));
                continue;
            }
        };
        reset_device_settings(&ctx);
        {
            let _companion_action = ctx.companion_actions.lock().unwrap();
            let _state = ctx.state.lock().unwrap();
            if !ctx.connect_requested.is_current(connection_request)
                || ctx.user_disconnected.load(Ordering::Acquire)
                || ctx.shutting_down.load(Ordering::Acquire)
                || ctx.mac() != mac
            {
                continue;
            }
            #[cfg(feature = "experimental-playback")]
            if _state.playback_native_driver && ctx.connect_requested.explicit() && matches!(
                _state.playback_mode, ipc::PlaybackMode::Aac | ipc::PlaybackMode::AacEld
            ) {
                let audio_intent = bt::playback_intent();
                let armed = arm_native_control_attempt(
                    || ctx.connect_requested.is_current(connection_request)
                        && ctx.connect_requested.explicit()
                        && !ctx.user_disconnected.load(Ordering::Acquire)
                        && !ctx.shutting_down.load(Ordering::Acquire)
                        && ctx.mac() == mac
                        && audio_intent & 1 != 0
                        && bt::audio_request_is_current(audio_intent)
                        && !ctx.pending_audio_disconnect.lock().unwrap().blocks_playback()
                        && pairing_allows_connect(ctx.paired_target.lock().unwrap().as_ref(),
                            &ctx.connect_requested, mac),
                    || driver.listen_playback(mac, audio_intent),
                );
                match armed {
                    Ok(true) => log("run_receiver: selected-peer signaling listener armed before AAP"),
                    Ok(false) => (), // Older drivers keep their existing outbound path.
                    Err(error) => {
                        drop(_state);
                        drop(_companion_action);
                        log(&format!("run_receiver: signaling listener unavailable before AAP: {error}"));
                        give_up(&ctx, &mut reach_fails, connection_request);
                        thread::sleep(Duration::from_millis(1500));
                        continue;
                    }
                }
            }
            *ctx.driver_cell.lock().unwrap() = Some(driver.clone());
        }
        // A passive native link may have vanished while the driver was opening.
        if !ctx.connect_requested.may_open_controls(
            connection_request,
            ctx.connect_requested.explicit() || bt::is_connected(mac),
        ) {
            *ctx.driver_cell.lock().unwrap() = None;
            ctx.connect_requested.release(connection_request);
            continue;
        }
        let startup_current = || {
            !ctx.user_disconnected.load(Ordering::Acquire)
                && !ctx.shutting_down.load(Ordering::Acquire)
                && ctx.mac() == mac
                && ctx.connect_requested.may_open_controls(
                    connection_request,
                    ctx.connect_requested.explicit() || bt::is_connected(mac),
                )
        };
        #[cfg(feature = "experimental-playback")]
        {
            let audio_intent = bt::playback_intent();
            let native = {
                let state = ctx.state.lock().unwrap();
                state.playback_native_driver
                    && matches!(
                        state.playback_mode,
                        ipc::PlaybackMode::Aac | ipc::PlaybackMode::AacEld
                    )
            };
            if native {
                let started = Instant::now();
                let result = prime_handsfree_once(
                    &mut primed_handsfree,
                    connection_request,
                    audio_intent,
                    || {
                        let state = ctx.state.lock().unwrap();
                        if !startup_current()
                            || !ctx.connect_requested.explicit()
                            || !bt::audio_request_is_current(audio_intent)
                            || !state.playback_native_driver
                            || !matches!(
                                state.playback_mode,
                                ipc::PlaybackMode::Aac | ipc::PlaybackMode::AacEld
                            )
                            || ctx
                                .pending_audio_disconnect
                                .lock()
                                .unwrap()
                                .blocks_playback()
                            || !pairing_allows_connect(
                                ctx.paired_target.lock().unwrap().as_ref(),
                                &ctx.connect_requested,
                                mac,
                            )
                        {
                            Err(io::Error::new(
                                io::ErrorKind::Interrupted,
                                "Hands-free startup was superseded",
                            ))
                        } else {
                            Ok(())
                        }
                    },
                    |current| hfp_endpoints::connect_control(mac, current),
                );
                match result {
                    Ok(Some(status)) => log(&format!(
                        "run_receiver: hands-free primed before AAP request_acknowledged={} jack_ready={} elapsed_ms={}",
                        status.request_acknowledged, status.jack_ready, started.elapsed().as_millis()
                    )),
                    Ok(None) => (),
                    Err(error) => log(&format!(
                        "run_receiver: hands-free startup unavailable after {} ms: {error}",
                        started.elapsed().as_millis()
                    )),
                }
            }
        }
        if !startup_current() {
            *ctx.driver_cell.lock().unwrap() = None;
            continue;
        }
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
        let initialized = initialize_control_session(
            startup_current,
            |packet| driver.send(packet),
            |packet| driver.send_sensor(packet),
            || driver.status(),
            thread::sleep,
            next_hr_seq(),
        );
        {
            let mut st = ctx.state.lock().unwrap();
            if !initialized || !startup_current() {
                *ctx.driver_cell.lock().unwrap() = None;
                let retry = startup_current();
                if retry {
                    st.connected = false;
                } else {
                    ctx.connect_requested.release(connection_request);
                }
                drop(st);
                let _ = driver.disconnect();
                if retry {
                    ctx.push_state();
                    give_up(&ctx, &mut reach_fails, connection_request);
                    thread::sleep(Duration::from_millis(1500));
                }
                continue;
            }
            reach_fails = 0; // reached them — reset the give-up counter
            st.connected = true;
            st.link_stale = false;
            battery_ble::invalidate(&mut st);
            st.anc = 0;
            let mut target = ctx.paired_target.lock().unwrap();
            if target.as_ref().is_some_and(|target| {
                matches!(target.stage, PairingStage::Selected)
                    && target.address == mac
                    && target.current(&ctx.connect_requested)
            }) {
                *target = None;
            }
        }
        let mut pending_card = true;
        load_control_choices_async(&ctx);
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
        let mut accessory_reports: HashMap<u8, Vec<u8>> = HashMap::new();
        let mut host_clock_sent = false;
        hr_decoder.reset(); // Device mappings belong to this connection.
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

        let mut we_paused: Option<(u64, media::PausedPlayback)> = None;
        let mut prev_ear = [false; 2]; // last [primary, secondary] in-ear state
        let mut last_status = Instant::now();
        let mut heartbeat = Instant::now();
        let mut established_request = connection_request;
        let mut control_link = ControlLinkStatus::default();
        let mut low_warned = false; // low-battery overlay fired (hysteresis)
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
        #[cfg(feature = "experimental-playback")]
        let mut startup_repair = None;
        loop {
            if ctx.shutting_down.load(Ordering::Acquire)
                || ctx.pending_audio_disconnect.lock().unwrap().is_pending()
            {
                break;
            }
            let local_media_route = {
                let state = ctx.state.lock().unwrap();
                state.audio_output_ready
                    && (state.playback_mode == ipc::PlaybackMode::Windows
                        || ctx.connect_requested.explicit())
            };
            if !local_media_route {
                we_paused = None;
            }
            #[cfg(feature = "experimental-playback")]
            if playback::startup_repair_pending(mac, bt::playback_intent()) {
                let disconnecting = ctx
                    .pending_audio_disconnect
                    .lock()
                    .unwrap()
                    .blocks_playback();
                let _preference = ctx.mic_preference.lock().unwrap();
                let mut state = ctx.state.lock().unwrap();
                let intent = bt::playback_intent();
                if ctx.mac() == mac
                    && ctx.connect_requested.explicit()
                    && !ctx.user_disconnected.load(Ordering::Acquire)
                    && !ctx.shutting_down.load(Ordering::Acquire)
                    && !disconnecting
                    && settings_refresh_allowed(
                        &mic,
                        ctx.pipe.status(),
                        ctx.capture_active.load(Ordering::Acquire),
                    )
                    && !ctx.mic_on.load(Ordering::Acquire)
                    && playback::take_startup_repair(mac, intent)
                {
                    startup_repair = Some(intent);
                    state.connected = false;
                    reserve_native_playback_transition(&mut state);
                    *ctx.driver_cell.lock().unwrap() = None;
                    log("run_receiver: playback startup repair requested — draining control session");
                    break;
                }
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
            // Zero asks the driver for its default timeout, so keep a 1 ms floor.
            let receive_wait = mic.wait_ms(clock.elapsed().as_millis() as u64, 100).max(1);
            if let Ok(n) = driver.recv(receive_wait, &mut buf) {
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
                            drop(current);
                            refresh_nearby_identity(&ctx, mac, true);
                        }
                        buf[..n].fill(0);
                        continue;
                    }
                    if aap::is_audio_packet(data) {
                        ctx.hq_mic_quiet.store(false, Ordering::Release);
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
                                "sensors: descriptors generation={} motion={:?} heart_rate={:?} sound_level={:?}",
                                sensors.generation,
                                sensors.motion.map(|s| s.id),
                                sensors.heart_rate,
                                sensors.sound_level,
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
                            let request = {
                                let _state = ctx.state.lock().unwrap();
                                let request = ctx.connect_requested.snapshot();
                                if new_other_host(
                                    last_devices
                                        .as_ref()
                                        .map(|(token, hosts)| (*token, hosts.as_slice())),
                                    request,
                                    &devs,
                                    &local_radios,
                                ) {
                                    ctx.case_return.lock().unwrap().revoke(mac, request);
                                }
                                request
                            };
                            if last_devices.as_ref().is_none_or(|(old_request, old)| {
                                *old_request != request || *old != devs
                            }) {
                                let list: Vec<ipc::ConnectedDevice> = devs
                                    .iter()
                                    .map(|(a, _)| describe_device(*a, &local_radios))
                                    .collect();
                                log(&multipoint_log(&list));
                                ctx.state.lock().unwrap().multipoint = list;
                                ctx.push_state();
                                last_devices = Some((request, devs));
                            }
                        }
                        if let Some(report) = aap::accessory::parse(data) {
                            let opcode = data[4];
                            if accessory_reports.get(&opcode).is_none_or(|old| old != data) {
                                log(&format!("accessory: {}", report.summary()));
                                accessory_reports.insert(opcode, data.to_vec());
                            }
                        }
                        if let Some((op, detail)) = aap::route_diag(data) {
                            log(&format!(
                                "route: 0x{op:02X} {} ({} bytes): {detail}",
                                aap::route_opcode_name(op),
                                data.len()
                            ));
                        }
                        if let Some(values) = capabilities::parse(data) {
                            let changed = {
                                let mut state = ctx.state.lock().unwrap();
                                let values = values.into_iter().collect();
                                if state.aacp_capabilities == values {
                                    false
                                } else {
                                    state.aacp_capabilities = values;
                                    true
                                }
                            };
                            if changed {
                                ctx.push_state();
                            }
                        }
                        if let Some(b) = aap::parse_battery(data) {
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
                                let target = ctx.volume_target();
                                let guard = companion::ResumeGuard::capture(&ctx);
                                let resume_ctx = ctx.clone();
                                ctx.conv_duck
                                    .lock()
                                    .unwrap()
                                    .on_status(status, ctx.mac(), target, move || guard.is_some_and(|g| g.current(&resume_ctx)));
                            }
                        }
                        if let Some((model, firmware, serial)) = aap::parse_metadata(data) {
                            let saved_allow_off = companion_prefs::allow_off_choice(&ctx, ctx.mac(), device_settings::load_allow_off_choice(ctx.mac()));
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
                                                saved_allow_off;
                                        }
                                    }
                                }
                                c
                            };
                            if changed {
                                log(&format!("device metadata: model={model}"));
                                load_control_choices_async(&ctx);
                                ctx.push_state();
                                if ctx.hr_on.load(Ordering::Acquire)
                                    && device_models::heart_rate(&ctx.state.lock().unwrap())
                                {
                                    spawn_hr_retry(&ctx);
                                }
                            }
                            if !host_clock_sent
                                && device_models::hearing_aid(&ctx.state.lock().unwrap())
                            {
                                host_clock_sent = true;
                                match host_clock::packet(std::time::SystemTime::now()) {
                                    Ok(packet) => {
                                        let started = Instant::now();
                                        let result = driver.send_sensor(&packet);
                                        log(&format!(
                                            "Host clock submission: elapsed_ms={} sent={}",
                                            started.elapsed().as_millis(),
                                            result.is_ok()
                                        ));
                                        if let Err(error) = result {
                                            log(&format!("Host clock submission failed: {error}"));
                                        }
                                    }
                                    Err(error) => log(error),
                                }
                            }
                        }
                        if let Some((primary, secondary)) = aap::parse_ear_detection(data) {
                            // AAP ear detection reports both buds, so case notifications need no BLE scan.
                            let now = [primary, secondary];
                            diagnostic_controls::observe_case(&ctx, now);
                            {
                                let _state = ctx.state.lock().unwrap();
                                ctx.case_return.lock().unwrap().observe_case(
                                    mac,
                                    ctx.connect_requested.snapshot(),
                                    diagnostic_controls::in_case(now),
                                    clock.elapsed().as_millis() as u64,
                                );
                            }
                            // A fresh paired AAP session can wake before BLE scanning resumes.
                            if now.iter().any(|ear| ear.in_ear()) {
                                resume_case_return(&ctx, mac, clock.elapsed().as_millis() as u64);
                            }
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
                                if let Some(permission) = policy.permission(local_media_route) {
                                    if all_in {
                                        if let Some((recorded, paused)) = we_paused.take() {
                                            if recorded == permission {
                                                let resumed = paused.resume();
                                                log(&format!(
                                                    "media: automatic resume submitted={resumed}"
                                                ));
                                            }
                                        }
                                    } else if was_wearing {
                                        if let Some(paused) =
                                            media::pause_playing(ctx.volume_target())
                                        {
                                            we_paused = Some((permission, paused));
                                            log("media: automatic pause confirmed");
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
                // Capture changes wake this wait; recovery keeps its existing deadlines.
                thread::park_timeout(receiver_idle_wait(
                    mic_on,
                    head_tracking_enabled,
                    &mic,
                    clock.elapsed().as_millis() as u64,
                ));
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
                // Keep working audio through AAP failures; retire a dead idle link below.
                let windows_connected = driver_connected || bt::is_connected(mac);
                let playback_active = {
                    let state = ctx.state.lock().unwrap();
                    let windows = state.playback_mode == ipc::PlaybackMode::Windows
                        && state.audio_output_ready;
                    #[cfg(feature = "experimental-playback")]
                    {
                        windows
                            || (playback::active_address() == Some(mac)
                                && state.playback_state == "ready")
                    }
                    #[cfg(not(feature = "experimental-playback"))]
                    {
                        windows
                    }
                };
                // A dead AAP channel cannot acknowledge a newer Connect.
                if driver_connected {
                    established_request = ctx.connect_requested.snapshot();
                }
                let release = control_link.observe(
                    st.ok(),
                    windows_connected,
                    ctx.mic_on.load(Ordering::Relaxed) || mic.cleanup_pending(),
                    playback_active,
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
                    log("run_receiver: AirPods connection lost — releasing");
                    ctx.overlay("Disconnected");
                    ctx.hr_db.end_session();
                    {
                        let _companion_action = ctx.companion_actions.lock().unwrap();
                        let mut s = ctx.state.lock().unwrap();
                        s.connected = false;
                        clear_heart_rate(&mut s);
                        // Preserve only the exact case-close retirement, never a newer command.
                        if ctx.connect_requested.release(established_request) {
                            if ctx.case_return.lock().unwrap().link_lost(
                                mac,
                                established_request,
                                ctx.connect_requested.snapshot(),
                                clock.elapsed().as_millis() as u64,
                            ) {
                                log("connection: previous PC session may return on verified case wake");
                            }
                        }
                    }
                    *ctx.driver_cell.lock().unwrap() = None;
                    hr_decoder.reset(); // drop HR carry across the reconnect
                    last_anc = 0;
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
                let receive_wait = mic.wait_ms(clock.elapsed().as_millis() as u64, 100).max(1);
                match driver.recv(receive_wait, &mut buf) {
                    Ok(n) if aap::is_audio_packet(&buf[..n]) => {
                        ctx.hq_mic_quiet.store(false, Ordering::Release);
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
        #[cfg(all(feature = "experimental-playback", feature = "hearing-test-research"))]
        let (_probe_drain, probe_restored) = if startup_repair.is_some() {
            let (reservation, restored) = drain_hearing_probe(&HEARING_TEST_PROBE);
            if !restored {
                ctx.shutdown_cleanup_failed.store(true, Ordering::Release);
                log("hearing-test mode: startup repair could not verify restoration");
            }
            (Some(reservation), restored)
        } else {
            (None, true)
        };
        #[cfg(all(
            feature = "experimental-playback",
            not(feature = "hearing-test-research")
        ))]
        let probe_restored = true;
        #[cfg(feature = "experimental-playback")]
        let _settings_drain = startup_repair.map(|_| ctx.settings_write.lock().unwrap());
        #[cfg(feature = "experimental-playback")]
        let _att_drain = startup_repair.map(|_| {
            hearing::invalidate_session();
            hearing::HEARING_STATE
                .lock()
                .unwrap_or_else(|error| error.into_inner())
        });
        let (stop_result, close_result, rebuild_failed) = close_control_session(
            mic.uplink_active,
            || driver.send(&aap::STOP_AUDIO),
            || driver.disconnect(),
            || finish_audio_disconnect(&ctx),
        );
        companion::control_closed(&ctx, mac, close_result.is_ok());
        if close_result.is_ok() {
            // Confirmed channel closure retires even an unfinished receiver drain.
            let _preference = ctx.mic_preference.lock().unwrap();
            ctx.hq_mic_quiet.store(true, Ordering::Release);
        }
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
            ctx.wants_settings_refresh.lock().unwrap().take();
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
            #[cfg(feature = "experimental-playback")]
            if startup_repair.is_some()
                && (!probe_restored
                    || stop_result.is_err()
                    || close_result.is_err()
                    || rebuild_failed
                    || unfinished_mic)
            {
                s.playback_state = "error".into();
                s.playback_error = Some("Could not finish audio recovery. Repair the connection before trying playback again.".into());
            }
        }
        ctx.push_state();
        #[cfg(feature = "experimental-playback")]
        if let Some(intent) = startup_repair {
            playback::finish_startup_repair(
                mac,
                intent,
                probe_restored
                    && stop_result.is_ok()
                    && close_result.is_ok()
                    && !rebuild_failed
                    && !unfinished_mic,
            );
        }
        thread::sleep(Duration::from_millis(250));
    }
}

/// Track real Windows capture clients independently of the selected uplink policy.
fn poll_mic(ctx: Ctx, receiver: Option<thread::Thread>) {
    loop {
        if ctx.shutting_down.load(Ordering::Acquire) {
            return;
        }
        ctx.conv_duck.lock().unwrap().poll();
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
            let _companion_action = active.then(|| ctx.companion_actions.lock().unwrap());
            let _preference = ctx.mic_preference.lock().unwrap();
            ctx.pipe.set_capture_active(active);
            let changed = ctx.capture_active.swap(active, Ordering::AcqRel) != active;
            if changed && active {
                companion::microphone_started(&ctx);
            }
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
    if let Some(code) = hfp_guard::run_requested() {
        std::process::exit(code);
    }
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
    if let Err(error) = hfp_guard::recover() {
        log(&format!("hands-free profile restoration: {error}"));
    }
    let handsfree_restore_failed = if let Err(error) = handsfree::recover() {
        log(&format!("hands-free endpoint restoration: {error}"));
        true
    } else {
        false
    };
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
    let battery_keys = battery_ble::Keys::load(mac);
    let finder_identity_available =
        mac != 0 && battery_keys.as_ref().is_some_and(|keys| keys.mac == mac);
    #[cfg(feature = "experimental-playback")]
    let playback_mode = playback::load_selection();
    #[cfg(not(feature = "experimental-playback"))]
    let playback_mode = ipc::PlaybackMode::Windows;
    pipe.set_feature_enabled(microphone_enabled);
    log(&format!("mic pipe opened: {}", pipe.is_open()));
    log(&format!("mic driver: {}", micpipe::driver_kind()));
    let companion_state = companion::State::load();
    let companion_held = companion_state.holds(mac);
    let ctx = Ctx {
        dongle: Arc::new(dongle::Controller::default()),
        dongle_health: Arc::new(dongle_health::Controller::default()),
        companion: Arc::new(Mutex::new(companion_state)),
        companion_preferences: Arc::new(Mutex::new(companion_prefs::Store::load())),
        companion_actions: Arc::new(Mutex::new(())),
        battery_keys: Arc::new(Mutex::new(battery_keys)),
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
            nearby_finder: ipc::NearbyFinderState {
                available_identity: finder_identity_available,
                ..Default::default()
            },
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
        hardware_check: Arc::new(Mutex::new(None)),
        case_observed_tick: Arc::new(AtomicU64::new(0)),
        hr_commands: Arc::new(Mutex::new(())),
        hr_generation: Arc::new(AtomicU64::new(0)),
        head_tracking: Arc::new(Mutex::new(head_tracking::Tracker::default())),
        nearby_finder: Arc::new(Mutex::new(nearby_finder::Finder::new(
            if mac == 0 {
                String::new()
            } else {
                format!("{mac:012x}")
            },
            finder_identity_available,
        ))),
        nearby_last_heard_tick: Arc::new(AtomicU64::new(0)),
        auto_pause: Arc::new(Mutex::new(media::AutoPausePolicy::new(auto_pause))),
        driver_cell: Arc::new(Mutex::new(None)),
        shared_driver: Arc::new(driver::Shared::default()),
        hr_db: hrdb::HrDb::open(log),
        mic_on: Arc::new(AtomicBool::new(false)),
        hq_mic_quiet: Arc::new(AtomicBool::new(true)),
        capture_active: Arc::new(AtomicBool::new(false)),
        shutting_down: Arc::new(AtomicBool::new(false)),
        receiver_done: Arc::new(AtomicBool::new(false)),
        playback_done: Arc::new(AtomicBool::new(!cfg!(feature = "experimental-playback"))),
        shutdown_cleanup_failed: Arc::new(AtomicBool::new(handsfree_restore_failed)),
        auto_mode: Arc::new(AtomicBool::new(microphone_enabled)),
        hr_on: Arc::new(AtomicBool::new(false)),
        hr_got_sample: Arc::new(AtomicBool::new(false)),
        hr_stream_live: Arc::new(AtomicBool::new(false)),
        hr_retrying: Arc::new(AtomicBool::new(false)),
        connect_requested: Arc::new(ConnectionRequest::new(false)),
        case_return: Arc::new(Mutex::new(case_return::CaseReturn::default())),
        paired_target: Arc::new(Mutex::new(None)),
        wants_reconnect: Arc::new(AtomicBool::new(false)),
        wants_settings_refresh: Arc::new(Mutex::new(None)),
        hearing_confirmation: Arc::new(Mutex::new(None)),
        pipe,
        conv_duck: Arc::new(Mutex::new(volume::ConvDuck::new(conversation_volume))),
        user_disconnected: Arc::new(AtomicBool::new(companion_held)),
        pending_audio_disconnect: Arc::new(Mutex::new(PendingAudioDisconnect::default())),
        last_repair: Arc::new(Mutex::new(None)),
        dev_name: Arc::new(Mutex::new(dev_name.clone())),
        mac: Arc::new(AtomicU64::new(mac)),
    };
    dongle::start(ctx.clone());
    dongle_health::start(ctx.clone());

    // Name the virtual mic after the connected device (elevated task, no UAC).
    if mac != 0 {
        rename::apply(&dev_name);
    }

    // IPC: two one-directional pipe servers (events out, commands in).
    companion_prefs::initialize(&ctx);
    companion::start(ctx.clone());
    {
        let c = ctx.clone();
        thread::spawn(move || unsafe { ipc_server::events_server(c) });
    }
    {
        let c = ctx.clone();
        thread::spawn(move || unsafe { ipc_server::cmds_server(c) });
    }
    // Public lid reports invite pairing; signal and battery data require the selected pair's key.
    // Verified case wake may resume one previously owned PC session.
    {
        let c = ctx.clone();
        let c_scan = ctx.clone();
        thread::spawn(move || {
            let last_published = Mutex::new(0u64);
            let lid_transitions = Mutex::new(battery_ble::PairedLidTransitions::default());
            let invitations = Mutex::new(battery_ble::Invitations::default());
            let scan_clock = c.mic_clock;
            le::watch_nearby(
                move |address, data, rssi, received_ms| {
                    if !c.nearby_scan_allowed() {
                        return;
                    }
                    let paired_mac = c.mac();
                    let (owned, report) = {
                        let keys = c.battery_keys.lock().unwrap();
                        let keys = keys.as_ref().filter(|keys| keys.mac == paired_mac);
                        (
                            keys.is_some_and(|keys| keys.matches_address(address)),
                            keys.and_then(|keys| keys.decode(paired_mac, address, data)),
                        )
                    };
                    let paired = owned && bt::paired_aap_name(paired_mac).is_some();
                    if let Some((model, lid_open)) = invitations.lock().unwrap().observe(
                        data,
                        c.mic_clock.elapsed().as_millis() as u64,
                        || paired,
                    ) {
                        c.send_event(&Event::PairingNearby { model, lid_open });
                    }
                    if c.mac() != paired_mac {
                        return;
                    }
                    // The rotating address must resolve with this pair's stored IRK.
                    if let Some(rssi) = rssi.filter(|_| owned) {
                        let mut finder = c.nearby_finder.lock().unwrap();
                        if finder.observe_verified(&format!("{paired_mac:012x}"), rssi, received_ms)
                        {
                            publish_nearby_finder(&c, &finder);
                        }
                    }
                    let Some(report) = report else {
                        return;
                    };
                    let finder_scan = c.nearby_finder.lock().unwrap().scanning();
                    let now_ms = unix_millis();
                    let mut published = last_published.lock().unwrap();
                    let bonded = report.lid_closed.is_some() && paired;
                    let (restored, changed, lid_edge) = {
                        let mut state = c.state.lock().unwrap();
                        if c.mac() != paired_mac
                            || !battery_ble::scan_allowed(
                                state.ble_scan_while_connected || finder_scan,
                                state.connected,
                                state.mic_capture_active
                                    || c.capture_active.load(Ordering::Acquire),
                                state.mic_uplink_active || c.mic_on.load(Ordering::Acquire),
                            )
                        {
                            return;
                        }
                        let mut lid_edge = false;
                        if report.lid_closed.is_some() {
                            if let Some((lid_open, sequence)) = lid_transitions
                                .lock()
                                .unwrap()
                                .observe(paired_mac, report.lid_closed, bonded)
                            {
                                // Queue a close before its battery snapshot replaces the island payload.
                                c.send_event(&Event::PairedLid {
                                    device_id: format!("{paired_mac:012x}"),
                                    lid_open,
                                    sequence,
                                });
                                lid_edge = true;
                            }
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
                        (restored, changed, lid_edge)
                    };
                    if should_publish_ble(restored, changed, lid_edge, now_ms, *published) {
                        *published = now_ms;
                        c.push_state();
                    }
                    drop(published);
                    if bonded && report.both_in_case && report.lid_closed == Some(true) {
                        let _state = c.state.lock().unwrap();
                        if c.mac() == paired_mac
                            && c.case_return.lock().unwrap().observe_closed(
                                paired_mac,
                                c.connect_requested.snapshot(),
                                true,
                                received_ms,
                                c.mic_clock.elapsed().as_millis() as u64,
                            )
                        {
                            log("connection: case close confirmed by the paired BLE report");
                        }
                    }
                    if bonded && report.lid_closed == Some(false) {
                        resume_case_return(&c, paired_mac, received_ms);
                    }
                },
                move || c_scan.nearby_scan_allowed(),
                scan_clock,
            );
        });
    }

    #[cfg(feature = "experimental-playback")]
    playback::start(ctx.clone());
    // AAP needs a paired device; Windows capture/driver status does not.
    let receiver = {
        let c = ctx.clone();
        Some(thread::spawn(move || run_receiver(c)).thread().clone())
    };
    {
        let c = ctx.clone();
        thread::spawn(move || poll_mic(c, receiver));
    }
    // Keep volume and playback activity tied to the selected output.
    {
        let c = ctx.clone();
        thread::spawn(move || {
            volume::init();
            // The Windows A2DP route may need a wake nudge. Native AVDTP owns its
            // stream startup; changing ANC there can interrupt the first audio.
            let mut was_playing = false;
            let mut last_wake = None;
            loop {
                c.sync_volume();
                let playing = volume::any_render_active(c.volume_target());
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
    #[test]
    fn windows_suspend_quiet_receipt_fences_a_start_reservation_without_blocking() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Mutex,
        };
        let preference = Mutex::new(());
        let mic_on = AtomicBool::new(false);
        let quiet = AtomicBool::new(true);
        assert!(super::microphone_quiet_for_windows(&preference, &mic_on, &quiet).unwrap());
        {
            let _reservation = preference.lock().unwrap();
            assert!(!super::microphone_quiet_for_windows(&preference, &mic_on, &quiet).unwrap());
            quiet.store(false, Ordering::Release);
        }
        // START can still be in flight even though mic_on has never become true.
        assert!(!mic_on.load(Ordering::Acquire));
        assert!(!super::microphone_quiet_for_windows(&preference, &mic_on, &quiet).unwrap());
        quiet.store(true, Ordering::Release);
        mic_on.store(true, Ordering::Release);
        assert!(!super::microphone_quiet_for_windows(&preference, &mic_on, &quiet).unwrap());
        mic_on.store(false, Ordering::Release);
        assert!(super::microphone_quiet_for_windows(&preference, &mic_on, &quiet).unwrap());
        let _ = std::panic::catch_unwind(|| {
            let _reservation = preference.lock().unwrap();
            panic!("Reservation failed");
        });
        assert!(super::microphone_quiet_for_windows(&preference, &mic_on, &quiet).is_err());
    }

    #[test]
    fn mode_probe_cleanup_finishes_before_teardown_and_blocks_a_replacement_probe() {
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex,
        };
        let owner = Arc::new(AtomicBool::new(true));
        let alive = owner.clone();
        let disabled = Arc::new(AtomicBool::new(false));
        let cleanup = disabled.clone();
        let worker = std::thread::spawn(move || {
            while alive.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            cleanup.store(true, Ordering::Release);
            true
        });
        let jobs = Mutex::new(Some(super::HearingProbeJob { owner, worker }));
        let (reservation, restored) = super::drain_hearing_probe(&jobs);
        assert!(restored && disabled.load(Ordering::Acquire));
        assert!(reservation.is_none() && jobs.try_lock().is_err());
        drop(reservation);
        assert!(jobs.try_lock().is_ok());
        let worker = std::thread::spawn(|| false);
        *jobs.lock().unwrap() = Some(super::HearingProbeJob {
            owner: Arc::new(AtomicBool::new(true)),
            worker,
        });
        assert!(!super::drain_hearing_probe(&jobs).1);
        assert!(super::drain_hearing_probe(&jobs).1);
    }

    #[test]
    fn hearing_mode_probe_requires_exact_pair_and_inactive_hearing_and_microphone_paths() {
        let mut state = ntpods_ipc::Snapshot {
            connected: true,
            device_id: "001122334455".into(),
            model: "A3063".into(),
            mic_state: "idle".into(),
            hearing_aid_enabled: Some(false),
            media_assist_enabled: Some(false),
            audio_output_ready: true,
            playback_state: "ready".into(),
            ..Default::default()
        };
        assert!(super::hearing_test_probe_error(&state, "001122334455", false, false).is_none());
        assert!(super::hearing_test_probe_error(&state, "001122334455", false, true).is_some());
        for phase in ["starting", "stopping", "recovering", "disconnected"] {
            state.playback_state = phase.into();
            assert!(
                super::hearing_test_probe_error(&state, "001122334455", false, false).is_some()
            );
        }
        state.playback_state = "ready".into();
        state.audio_output_ready = false;
        assert!(super::hearing_test_probe_error(&state, "001122334455", false, false).is_some());
        state.audio_output_ready = true;
        for target in ["001122334456", "", "000000000000", "00112233445G"] {
            assert!(super::hearing_test_probe_error(&state, target, false, false).is_some());
        }
        assert!(super::hearing_test_probe_error(&state, "001122334455", true, false).is_some());
        state.mic_uplink_active = true;
        assert!(super::hearing_test_probe_error(&state, "001122334455", false, false).is_some());
        state.mic_uplink_active = false;
        state.hearing_aid_enabled = None;
        assert!(super::hearing_test_probe_error(&state, "001122334455", false, false).is_some());
        state.hearing_aid_enabled = Some(false);
        state.media_assist_enabled = Some(true);
        assert!(super::hearing_test_probe_error(&state, "001122334455", false, false).is_some());
        state.media_assist_enabled = Some(false);
        state.model = "A2083".into();
        assert!(super::hearing_test_probe_error(&state, "001122334455", false, false).is_some());
    }

    #[cfg(feature = "experimental-playback")]
    #[test]
    fn native_control_retry_rearms_after_cleanup_and_rejects_superseded_attempts() {
        use std::{cell::Cell, io};
        let current = Cell::new(true);
        let listening = Cell::new(false);
        let arms = Cell::new(0);
        // AAP failure closes/disarms the handle between attempts. The same
        // explicit request still needs a fresh listener before each retry.
        for _ in 0..2 {
            assert!(super::arm_native_control_attempt(|| current.get(), || {
                arms.set(arms.get() + 1);
                listening.set(true);
                Ok(true)
            }).unwrap());
            assert!(listening.replace(false));
        }
        assert_eq!(arms.get(), 2);
        current.set(false);
        assert_eq!(super::arm_native_control_attempt(|| current.get(), || {
            panic!("An old request cannot rearm after handoff/disconnect")
        }).unwrap_err().kind(), io::ErrorKind::Interrupted);
        current.set(true);
        assert_eq!(super::arm_native_control_attempt(|| current.get(), || {
            current.set(false);
            Ok(true)
        }).unwrap_err().kind(), io::ErrorKind::Interrupted);
        current.set(true);
        assert_eq!(super::arm_native_control_attempt(|| current.get(), || {
            Err(io::ErrorKind::PermissionDenied.into())
        }).unwrap_err().kind(), io::ErrorKind::PermissionDenied);
    }

    #[cfg(feature = "experimental-playback")]
    #[test]
    fn handsfree_priming_requires_explicit_audio_and_current_generations() {
        use std::{cell::Cell, io};

        let connection = super::ConnectionRequest::new(false);
        assert!(connection.claim());
        let passive = connection.snapshot();
        let mut primed = None;
        assert!(super::prime_handsfree_once(
            &mut primed,
            passive,
            3,
            || panic!("Passive controls must not acquire audio"),
            |_| panic!("Passive controls must not connect HFP"),
        )
        .unwrap()
        .is_none());
        connection.store(true, Ordering::Release);
        let previous = connection.snapshot();
        connection.store(true, Ordering::Release);
        let current_request = connection.snapshot();
        let audio = Cell::new(7u64);
        for (request, intent) in [(previous, 7), (current_request, 3)] {
            let result = super::prime_handsfree_once(
                &mut primed,
                request,
                intent,
                || {
                    if connection.is_current(request) && audio.get() == intent {
                        Ok(())
                    } else {
                        Err(io::ErrorKind::Interrupted.into())
                    }
                },
                |_| panic!("A stale generation must not connect HFP"),
            );
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
            assert_eq!(primed, None);
        }
        assert!(super::prime_handsfree_once(
            &mut primed,
            current_request,
            8,
            || panic!("A disconnect intent must not be primed"),
            |_| panic!("A disconnect intent must not connect HFP"),
        )
        .unwrap()
        .is_none());
    }

    #[cfg(feature = "experimental-playback")]
    #[test]
    fn handsfree_priming_cancellation_reaches_the_write_and_does_not_repeat() {
        use std::{cell::Cell, io};

        let connection = super::ConnectionRequest::new(true);
        let request = connection.snapshot();
        let cancelled = Cell::new(false);
        let attempts = Cell::new(0);
        let writes = Cell::new(0);
        let mut primed = None;
        let result = super::prime_handsfree_once(
            &mut primed,
            request,
            3,
            || {
                if cancelled.get() {
                    Err(io::ErrorKind::Interrupted.into())
                } else {
                    Ok(())
                }
            },
            |current| {
                attempts.set(attempts.get() + 1);
                cancelled.set(true);
                current()?;
                writes.set(writes.get() + 1);
                unreachable!()
            },
        );
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
        assert_eq!(writes.get(), 0);
        cancelled.set(false);
        assert!(super::prime_handsfree_once(
            &mut primed,
            request,
            3,
            || Ok(()),
            |_| panic!("The same connection must not be primed twice"),
        )
        .unwrap()
        .is_none());
        assert_eq!(attempts.get(), 1);

        connection.store(true, Ordering::Release);
        let next = connection.snapshot();
        let status = super::prime_handsfree_once(
            &mut primed,
            next,
            7,
            || {
                if connection.is_current(next) {
                    Ok(())
                } else {
                    Err(io::ErrorKind::Interrupted.into())
                }
            },
            |current| {
                current()?;
                attempts.set(attempts.get() + 1);
                Ok(super::hfp_endpoints::ControlStatus {
                    request_acknowledged: true,
                    jack_ready: false,
                })
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(attempts.get(), 2);
        assert_eq!(primed, Some(next));
        assert!(status.request_acknowledged);
        assert!(!status.jack_ready);
    }

    #[test]
    fn explicit_commands_cannot_follow_selection_to_another_pair() {
        let first = "123456789ABC";
        let second = "23456789ABCD";
        for command in [
            ntpods_ipc::Command::SetAnc {
                mode: 2,
                device_id: Some(first.into()),
            },
            ntpods_ipc::Command::SetDeviceControl {
                id: 0x0A,
                value: 2,
                device_id: Some(first.into()),
            },
            ntpods_ipc::Command::Connect {
                device_id: Some(first.into()),
            },
        ] {
            let target = match &command {
                ntpods_ipc::Command::SetAnc { device_id, .. }
                | ntpods_ipc::Command::SetDeviceControl { device_id, .. }
                | ntpods_ipc::Command::Connect { device_id } => device_id.as_deref(),
                _ => unreachable!(),
            };
            assert!(super::command_targets_selected_pair(target, first));
            assert!(!super::command_targets_selected_pair(target, second));
            assert!(!super::command_targets_selected_pair(target, ""));
        }
        assert!(super::command_targets_selected_pair(
            Some(first),
            "123456789abc"
        ));
        for invalid in [
            "",
            "bad",
            "12:34:56:78:9A:BC",
            "000000000000",
            "FFFFFFFFFFFF",
        ] {
            assert!(!super::command_targets_selected_pair(
                Some(invalid),
                invalid
            ));
        }
        // Legacy commands without a target still use the current pair.
        assert!(super::command_targets_selected_pair(None, first));
        assert!(super::command_targets_selected_pair(None, second));
    }

    #[test]
    fn a_retired_connect_result_cannot_initialize_or_consume_a_newer_connect() {
        use std::cell::Cell;
        use std::sync::atomic::Ordering;

        let request = super::ConnectionRequest::new(true);
        let connecting = request.snapshot();
        // The blocking driver connect returns after another client pressed Connect.
        request.store(true, Ordering::Release);
        let newer = request.snapshot();
        let writes = Cell::new(0);
        let send = |_: &[u8]| {
            writes.set(writes.get() + 1);
            Ok(())
        };
        assert!(!super::initialize_control_session(
            || request.may_open_controls(connecting, true),
            send,
            send,
            || Ok(2),
            |_| {},
            1,
        ));
        assert_eq!(writes.get(), 0);
        assert!(!request.release(connecting));
        assert!(request.is_current(newer));
        assert!(request.explicit());
    }

    #[test]
    fn control_initialization_rechecks_intent_after_every_write() {
        use std::cell::Cell;
        use std::sync::atomic::Ordering;

        for retire_after in 1..=7 {
            for reason in 0..4 {
                let request = super::ConnectionRequest::new(reason != 2);
                if reason == 2 {
                    assert!(request.claim());
                }
                let connecting = request.snapshot();
                let windows_connected = Cell::new(true);
                let shutdown = Cell::new(false);
                let writes = Cell::new(0);
                let current = || {
                    !shutdown.get()
                        && request.may_open_controls(connecting, windows_connected.get())
                };
                let send = |_: &[u8]| {
                    writes.set(writes.get() + 1);
                    if writes.get() == retire_after {
                        match reason {
                            0 => request.store(false, Ordering::Release),
                            1 => request.store(true, Ordering::Release),
                            2 => windows_connected.set(false),
                            _ => shutdown.set(true),
                        }
                    }
                    Ok(())
                };
                let initialized =
                    super::initialize_control_session(current, send, send, || Ok(2), |_| {}, 1);
                assert!(!initialized);
                assert_eq!(writes.get(), retire_after);
                request.release(connecting);
                assert_eq!(request.explicit(), reason == 1);
            }
        }
    }

    #[test]
    fn a_new_connect_during_either_initialization_wait_retires_the_old_startup() {
        use std::cell::Cell;
        use std::sync::atomic::Ordering;

        for superseded_after_wait in 1..=2 {
            let request = super::ConnectionRequest::new(true);
            let connecting = request.snapshot();
            let writes = Cell::new(0);
            let waits = Cell::new(0);
            let send = |_: &[u8]| {
                writes.set(writes.get() + 1);
                Ok(())
            };
            assert!(!super::initialize_control_session(
                || request.may_open_controls(connecting, true),
                send,
                send,
                || Ok(2),
                |_| {
                    waits.set(waits.get() + 1);
                    if waits.get() == superseded_after_wait {
                        request.store(true, Ordering::Release);
                    }
                },
                1,
            ));
            assert_eq!(writes.get(), superseded_after_wait);
            assert!(!request.release(connecting));
            assert!(request.explicit());
        }
    }

    #[test]
    fn failed_required_control_writes_cannot_publish_a_connection() {
        use std::cell::Cell;

        for failed_write in 1..=3 {
            let writes = Cell::new(0);
            let send = |_: &[u8]| {
                writes.set(writes.get() + 1);
                if writes.get() == failed_write {
                    Err(std::io::Error::from(std::io::ErrorKind::NotConnected))
                } else {
                    Ok(())
                }
            };
            assert!(!super::initialize_control_session(
                || true,
                send,
                send,
                || panic!("failed startup must not reach the status check"),
                |_| {},
                1,
            ));
            assert_eq!(writes.get(), failed_write);
        }
    }

    #[test]
    fn optional_control_writes_require_a_live_transport_before_publication() {
        use std::cell::Cell;

        for failed_write in 4..=7 {
            for status in [None, Some(0), Some(1), Some(2)] {
                let writes = Cell::new(0);
                let send = |_: &[u8]| {
                    writes.set(writes.get() + 1);
                    if writes.get() == failed_write {
                        Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
                    } else {
                        Ok(())
                    }
                };
                assert_eq!(
                    super::initialize_control_session(
                        || true,
                        send,
                        send,
                        || status.ok_or_else(|| std::io::Error::from(std::io::ErrorKind::Other)),
                        |_| {},
                        1,
                    ),
                    status == Some(2),
                );
                assert_eq!(writes.get(), 7);
            }
        }
    }

    #[test]
    fn control_status_check_cannot_publish_or_release_a_newer_connect() {
        use std::sync::atomic::Ordering;

        let request = super::ConnectionRequest::new(true);
        let connecting = request.snapshot();
        assert!(!super::initialize_control_session(
            || request.may_open_controls(connecting, true),
            |_| Ok(()),
            |_| Ok(()),
            || {
                request.store(true, Ordering::Release);
                Ok(2)
            },
            |_| {},
            1,
        ));
        assert!(!request.release(connecting));
        assert!(request.explicit());
    }

    #[test]
    fn failed_control_initialization_preserves_connect_until_its_retry_limit() {
        use std::cell::Cell;
        use std::sync::atomic::Ordering;

        for failed_write in 0..=3 {
            let requested = super::ConnectionRequest::new(true);
            let request = requested.snapshot();
            let mut failures = 0;
            for attempt in 1..=12 {
                let writes = Cell::new(0);
                let send = |_: &[u8]| {
                    writes.set(writes.get() + 1);
                    if writes.get() == failed_write {
                        Err(std::io::Error::from(std::io::ErrorKind::NotConnected))
                    } else {
                        Ok(())
                    }
                };
                assert!(!super::initialize_control_session(
                    || requested.may_open_controls(request, true),
                    send,
                    send,
                    || Ok(0),
                    |_| {},
                    1,
                ));
                assert_eq!(
                    super::release_failed_connection_request(&requested, &mut failures, request),
                    attempt == 12,
                );
                assert_eq!(requested.load(Ordering::Acquire), attempt < 12);
                assert_eq!(requested.explicit(), attempt < 12);
            }
            assert_eq!(failures, 0);
        }
    }

    #[test]
    fn exhausted_control_retries_cannot_release_a_newer_connect() {
        use std::sync::atomic::Ordering;

        let requested = super::ConnectionRequest::new(true);
        let old = requested.snapshot();
        let mut failures = 11;
        requested.store(true, Ordering::Release);
        let newer = requested.snapshot();
        assert!(!super::release_failed_connection_request(
            &requested,
            &mut failures,
            old,
        ));
        assert!(requested.is_current(newer));
        assert!(requested.explicit());
        assert_eq!(failures, 0);
    }

    #[test]
    fn rapid_lid_edges_publish_even_when_battery_is_unchanged() {
        let publish = super::should_publish_ble;
        assert!(!publish(true, false, false, 100, 99));
        assert!(publish(true, false, true, 100, 99));
        assert!(publish(false, false, true, 101, 100));
        assert!(publish(true, true, false, 102, 101));
    }

    #[test]
    fn audio_output_readiness_requires_the_selected_route_and_native_authorization() {
        use ntpods_ipc::PlaybackMode;
        let selected = 0xaabbccddeeff;
        let ready = super::selected_audio_output_ready;
        assert!(!ready(
            PlaybackMode::Windows,
            "windows",
            0,
            true,
            None,
            true
        ));
        assert!(ready(
            PlaybackMode::Windows,
            "windows",
            selected,
            true,
            None,
            false
        ));
        assert!(!ready(
            PlaybackMode::Windows,
            "windows",
            selected,
            false,
            Some(selected),
            true
        ));
        assert!(ready(
            PlaybackMode::Aac,
            "ready",
            selected,
            false,
            Some(selected),
            true
        ));
        assert!(!ready(
            PlaybackMode::Aac,
            "ready",
            selected,
            true,
            Some(selected),
            false
        ));
        assert!(!ready(
            PlaybackMode::AacEld,
            "ready",
            selected,
            true,
            Some(selected),
            false
        ));
        assert!(!ready(
            PlaybackMode::Aac,
            "starting",
            selected,
            false,
            Some(selected),
            true
        ));
        assert!(!ready(
            PlaybackMode::AacEld,
            "ready",
            selected,
            true,
            Some(0x112233445566),
            true
        ));
    }

    #[test]
    fn routine_multipoint_log_omits_host_identities() {
        let devices = [
            ntpods_ipc::ConnectedDevice {
                address: "aa:bb:cc:dd:ee:ff".into(),
                is_this_pc: true,
                name: "Private PC".into(),
                kind: "pc".into(),
            },
            ntpods_ipc::ConnectedDevice {
                address: "11:22:33:44:55:66".into(),
                is_this_pc: false,
                name: "Private iPhone".into(),
                kind: "phone".into(),
            },
        ];
        let message = super::multipoint_log(&devices);
        assert_eq!(message, "multipoint: 2 device(s) on the AirPods, 1 this PC");
        for private in [
            "aa:bb:cc:dd:ee:ff",
            "11:22:33:44:55:66",
            "Private PC",
            "Private iPhone",
        ] {
            assert!(!message.contains(private));
        }
    }

    #[test]
    fn initial_host_inventory_preserves_the_explicit_pc_case_return() {
        let devices = [(1, [0, 0]), (2, [0, 0])];
        let mut policy = crate::case_return::CaseReturn::default();
        if super::new_other_host(None, 3, &devices, &[1]) {
            policy.revoke(10, 3);
        }
        policy.record_owned_playback(10, 3, 10);
        policy.observe_case(10, 3, Some(true), 20);
        assert!(policy.link_lost(10, 3, 4, 30));
        assert!(policy.take_open(10, 4, true, 40, 40));
        assert!(!super::new_other_host(
            Some((3, &devices)),
            3,
            &devices,
            &[1]
        ));
    }

    #[test]
    fn later_host_additions_revoke_until_a_new_explicit_pc_choice() {
        let pc = [(1, [0, 0])];
        let devices = [(1, [0, 0]), (2, [0, 0])];
        assert!(super::new_other_host(Some((3, &pc)), 3, &devices, &[1]));
        assert!(!super::new_other_host(Some((3, &pc)), 7, &devices, &[1]));
        // Passive Windows links do not grant a new audio choice.
        assert!(super::new_other_host(Some((3, &pc)), 9, &devices, &[1]));
        // An observed empty inventory is distinct from an unknown first inventory.
        assert!(super::new_other_host(Some((3, &[])), 3, &devices, &[1]));
        assert!(!super::new_other_host(Some((3, &[])), 3, &pc, &[1]));
    }

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
            Command::Connect { device_id: None },
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
            Command::SetPlaybackLink {
                settings: ntpods_ipc::PlaybackLinkSettings::default(),
            },
            Command::SetPlaybackGameMode { enabled: false },
            Command::SetPlaybackGameMode { enabled: true },
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
    fn saved_hardware_reads_require_idle_supported_buds_and_block_new_capture() {
        let mut state = super::Snapshot {
            connected: true,
            model: "A3063".into(),
            firmware: "9A348".into(),
            mic_state: "idle".into(),
            playback_state: "ready".into(),
            playback_native_driver: true,
            ..Default::default()
        };
        assert_eq!(
            super::hardware_diagnostics_read_error(&state, false, Some(0)),
            None
        );
        assert!(super::hardware_diagnostics_read_error(&state, true, Some(0)).is_some());
        assert!(super::hardware_diagnostics_read_error(&state, false, Some(1)).is_some());
        state.mic_state = "recovering".into();
        assert!(super::hardware_diagnostics_read_error(&state, false, Some(0)).is_some());
        state.mic_state = "idle".into();
        state.hearing_aid_pending = true;
        assert!(super::hardware_diagnostics_read_error(&state, false, Some(0)).is_some());
        state.hearing_aid_pending = false;
        state.hardware_diagnostics_pending = true;
        assert!(!super::playback_allows_microphone(&state));
        assert_eq!(
            super::manual_microphone_start_allowed(&state, true, true),
            Err("Hardware checks are being read")
        );
        state.hardware_diagnostics_pending = false;
        assert!(super::playback_allows_microphone(&state));
        state.hearing_metadata_pending = true;
        assert!(!super::playback_allows_microphone(&state));
        assert!(super::hardware_diagnostics_read_error(&state, false, Some(0)).is_some());
        state.hearing_metadata_pending = false;
        for phase in [
            "waiting_case_close",
            "waiting_result",
            "completed",
            "timed_out",
            "cancelled",
            "error",
        ] {
            state.hardware_check = Some(ntpods_ipc::HardwareCheckState {
                request_id: 1,
                phase: phase.into(),
                left_updated: false,
                right_updated: false,
                error: None,
            });
            assert_eq!(
                super::playback_allows_microphone(&state),
                !matches!(phase, "waiting_case_close" | "waiting_result")
            );
        }
        state.hardware_check = None;
        state.model = "A2084".into();
        assert!(super::hardware_diagnostics_read_error(&state, false, Some(0)).is_some());
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
            mode: None,
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
                super::receiver_idle_wait(
                    mic_on,
                    head_tracking_enabled,
                    &super::mic_lifecycle::Lifecycle::default(),
                    0,
                ),
                super::Duration::from_millis(expected_ms)
            );
        }
    }

    #[test]
    fn receiver_idle_wait_does_not_overshoot_microphone_recovery() {
        let mut mic = super::mic_lifecycle::Lifecycle::default();
        mic.tick(0, true, true);
        mic.start_attempted();
        mic.request_stop(100);
        for (now, expected_ms) in [(450, 150), (575, 25), (600, 0)] {
            assert_eq!(
                super::receiver_idle_wait(false, false, &mic, now),
                super::Duration::from_millis(expected_ms)
            );
        }
        assert_eq!(
            super::receiver_idle_wait(false, true, &mic, 599),
            super::Duration::from_millis(1)
        );
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
    fn pairing_preserves_mic_recovery_on_both_playback_drivers() {
        for native in [false, true] {
            let mut state = super::Snapshot {
                playback_native_driver: native,
                mic_state: "idle".into(),
                ..Default::default()
            };
            assert!(super::pairing_handoff_allowed(&state, false, false));
            assert!(!super::pairing_handoff_allowed(&state, true, false));
            assert!(!super::pairing_handoff_allowed(&state, false, true));
            state.mic_capture_active = true;
            assert!(!super::pairing_handoff_allowed(&state, false, false));
            state.mic_capture_active = false;
            state.mic_uplink_active = true;
            assert!(!super::pairing_handoff_allowed(&state, false, false));
            state.mic_uplink_active = false;
            // Capture and uplink can stop before Bluetooth recovery finishes.
            for phase in ["starting", "capturing", "stopping", "recovering"] {
                state.mic_state = phase.into();
                assert!(!super::pairing_handoff_allowed(&state, false, false));
            }
            state.mic_state = "idle".into();
            assert!(super::pairing_handoff_allowed(&state, false, false));
            state.playback_buffer_target_ms = Some(60);
            assert!(!super::pairing_handoff_allowed(&state, false, false));
        }
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
            assert!(!link.observe(
                Some(2),
                true,
                false,
                false,
                super::Duration::from_secs(seconds),
            ));
            assert!(!link.stale);
        }
    }

    #[test]
    fn recovered_transport_clears_stale_without_an_incoming_packet() {
        let mut link = super::ControlLinkStatus::default();
        assert!(!link.observe(None, true, false, false, super::Duration::from_secs(1)));
        assert!(!link.stale);
        assert!(!link.observe(None, true, false, false, super::Duration::from_secs(30)));
        assert!(link.stale);
        assert!(!link.observe(Some(2), true, false, false, super::Duration::from_secs(31)));
        assert!(!link.stale);
        // Capture recovery has its own deadline; idle checks must not replace it.
        assert!(!link.observe(None, true, true, false, super::Duration::from_secs(60)));
        assert!(!link.stale);
    }

    #[test]
    fn control_link_releases_after_three_consecutive_os_disconnects() {
        let mut link = super::ControlLinkStatus::default();
        let age = super::Duration::from_secs(60);
        assert!(!link.observe(None, false, false, false, age));
        assert!(!link.observe(None, false, false, false, age));
        assert!(!link.observe(None, true, false, false, age));
        assert!(!link.observe(None, false, false, false, age));
        assert!(!link.observe(None, false, false, false, age));
        assert!(link.observe(None, false, false, false, age));
    }

    #[test]
    fn disconnected_aap_retires_to_passive_controls_without_audio() {
        use std::sync::atomic::Ordering;

        let request = super::ConnectionRequest::new(true);
        let established = request.snapshot();
        let mut link = super::ControlLinkStatus::default();
        let age = super::Duration::from_secs(1);
        assert!(!link.observe(Some(0), true, false, false, age));
        assert!(!link.observe(Some(0), true, false, false, age));
        assert!(link.observe(Some(0), true, false, false, age));
        assert!(request.release(established));
        assert!(!request.load(Ordering::Acquire));
        assert!(request.claim());
        assert!(!request.explicit());
        request.store(true, Ordering::Release);
        assert!(!request.release(established));
        assert!(request.explicit());
    }

    #[test]
    fn aap_retirement_tolerates_recovery_unknown_status_and_microphone_cleanup() {
        let age = super::Duration::from_secs(60);
        for recovery in [None, Some(1), Some(2)] {
            let mut link = super::ControlLinkStatus::default();
            assert!(!link.observe(Some(0), true, false, false, age));
            assert!(!link.observe(Some(0), true, false, false, age));
            assert!(!link.observe(recovery, true, false, false, age));
            assert!(!link.observe(Some(0), true, false, false, age));
            assert!(!link.observe(Some(0), true, false, false, age));
            assert!(link.observe(Some(0), true, false, false, age));
        }
        let mut link = super::ControlLinkStatus::default();
        for _ in 0..60 {
            assert!(!link.observe(Some(0), true, true, false, age));
            assert!(!link.stale);
        }
        assert!(!link.observe(Some(0), true, false, false, age));
        assert!(!link.observe(Some(0), true, false, false, age));
        assert!(link.observe(Some(0), true, false, false, age));
    }

    #[test]
    fn dead_aap_preserves_working_playback_until_audio_is_gone() {
        let mut link = super::ControlLinkStatus::default();
        let age = super::Duration::from_secs(60);
        for _ in 0..60 {
            assert!(!link.observe(Some(0), true, false, true, age));
        }
        assert!(link.stale);
        assert!(!link.observe(Some(0), true, false, false, age));
        assert!(!link.observe(Some(0), true, false, false, age));
        assert!(link.observe(Some(0), true, false, false, age));

        let mut link = super::ControlLinkStatus::default();
        assert!(!link.observe(Some(0), false, true, true, age));
        assert!(!link.observe(Some(0), false, true, true, age));
        assert!(link.observe(Some(0), false, true, true, age));
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
        claim_windows_connection, cooldown_elapsed, media, mic_lifecycle, queue_settings_refresh,
        settings_refresh_allowed, update_auto_pause, utc_timestamp, SettingsRefresh, Snapshot,
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
    fn microphone_cannot_acquire_from_passive_controls_but_accepts_owned_windows_audio() {
        let mut state = Snapshot {
            connected: true,
            ..Default::default()
        };
        for mode in [
            ntpods_ipc::PlaybackMode::Windows,
            ntpods_ipc::PlaybackMode::Aac,
            ntpods_ipc::PlaybackMode::AacEld,
        ] {
            state.playback_mode = mode;
            state.audio_output_ready = false;
            assert!(!super::microphone_connection_allowed(&state, false));
            assert!(super::microphone_connection_allowed(&state, true));
            state.audio_output_ready = true;
            assert_eq!(
                super::microphone_connection_allowed(&state, false),
                mode == ntpods_ipc::PlaybackMode::Windows
            );
        }
        state.connected = false;
        assert!(!super::microphone_connection_allowed(&state, true));
    }

    #[test]
    fn passive_presence_cannot_create_an_outgoing_connection_request() {
        let requested = super::ConnectionRequest::new(false);
        let latched = AtomicBool::new(false);
        let shutdown = AtomicBool::new(false);
        for _ in 0..100 {
            assert!(!claim_windows_connection(
                false, &requested, &latched, &shutdown, None
            ));
            assert!(!requested.load(Ordering::Acquire));
        }
        // Explicit Connect still authorizes paging when there is no Windows link.
        requested.store(true, Ordering::Release);
        let completed = requested.snapshot();
        assert!(requested.release(completed));
        assert!(!claim_windows_connection(
            false, &requested, &latched, &shutdown, None
        ));
        requested.store(true, Ordering::Release);
        assert!(!requested.release(completed));
        assert!(requested.load(Ordering::Acquire));
    }

    #[test]
    fn pairing_reservation_blocks_windows_automatic_resume() {
        let requested = super::ConnectionRequest::new(false);
        let latched = AtomicBool::new(false);
        let shutdown = AtomicBool::new(false);
        let mut target = super::PairedTarget {
            address: 1,
            request: requested.snapshot(),
            stage: super::PairingStage::Validating,
        };
        for stage in [
            super::PairingStage::Validating,
            super::PairingStage::Ready("New buds".into()),
            super::PairingStage::Selected,
        ] {
            target.stage = stage;
            for _ in 0..10 {
                assert!(!claim_windows_connection(
                    true,
                    &requested,
                    &latched,
                    &shutdown,
                    Some(&target)
                ));
                assert!(target.current(&requested));
            }
        }
        requested.store(false, Ordering::Release);
        assert!(claim_windows_connection(
            true,
            &requested,
            &latched,
            &shutdown,
            Some(&target)
        ));
    }

    #[test]
    fn repeated_windows_resume_cannot_override_an_explicit_retry_latch() {
        let requested = super::ConnectionRequest::new(false);
        let latched = AtomicBool::new(true);
        let shutdown = AtomicBool::new(false);
        for _ in 0..100 {
            assert!(!claim_windows_connection(
                true, &requested, &latched, &shutdown, None
            ));
            assert!(!requested.load(Ordering::Acquire));
        }
        // Only an explicit Connect/Repair clears the latch. One automatic
        // request can then win; later Windows checks cannot duplicate it.
        latched.store(false, Ordering::Release);
        assert!(claim_windows_connection(
            true, &requested, &latched, &shutdown, None
        ));
        assert!(!claim_windows_connection(
            true, &requested, &latched, &shutdown, None
        ));
        requested.store(false, Ordering::Release);
        shutdown.store(true, Ordering::Release);
        assert!(!claim_windows_connection(
            true, &requested, &latched, &shutdown, None
        ));
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
    fn hearing_confirmation_survives_only_its_own_same_pair_refresh() {
        let token = super::HearingConfirmation {
            generation: 7,
            address: 11,
            connection: 3,
        };
        assert!(token.matches(Some(token), 11, 3));
        assert!(!token.matches(None, 11, 3));
        assert!(!token.matches(
            Some(super::HearingConfirmation {
                generation: 8,
                ..token
            }),
            11,
            3
        ));
        assert!(!token.matches(Some(token), 12, 3));
        assert!(!token.matches(Some(token), 11, 5));
        assert!(!token.matches(Some(token), 11, 2));
    }

    #[test]
    fn only_the_owned_hearing_refresh_bypasses_the_manual_cooldown() {
        let token = super::HearingConfirmation {
            generation: 7,
            address: 11,
            connection: 3,
        };
        let refresh = SettingsRefresh::Hearing(token);
        assert!(refresh.bypasses_cooldown(Some(token), 11, 3));
        assert!(!SettingsRefresh::Manual.bypasses_cooldown(Some(token), 11, 3));
        assert!(!refresh.bypasses_cooldown(None, 11, 3));
        assert!(!refresh.bypasses_cooldown(Some(token), 12, 3));
        assert!(!refresh.bypasses_cooldown(Some(token), 11, 4));
        assert!(!refresh.bypasses_cooldown(
            Some(super::HearingConfirmation {
                generation: 8,
                ..token
            }),
            11,
            3,
        ));
    }

    #[test]
    fn hearing_confirmation_refresh_still_rejects_recording() {
        let requested = Mutex::new(None);
        let token = super::HearingConfirmation {
            generation: 7,
            address: 11,
            connection: 3,
        };
        let mut state = Snapshot {
            mic_state: "idle".into(),
            ..Default::default()
        };
        for (streams, active) in [(None, false), (Some(1), false), (Some(0), true)] {
            queue_settings_refresh(
                &requested,
                &mut state,
                streams,
                active,
                SettingsRefresh::Hearing(token),
            );
            assert!(requested.lock().unwrap().is_none());
            assert!(state.device_settings_error.is_some());
        }
        queue_settings_refresh(
            &requested,
            &mut state,
            Some(0),
            false,
            SettingsRefresh::Hearing(token),
        );
        assert!(requested
            .lock()
            .unwrap()
            .take()
            .unwrap()
            .bypasses_cooldown(Some(token), 11, 3));
        assert!(
            requested.lock().unwrap().is_none(),
            "a confirmation refresh is consumed once"
        );
    }

    #[test]
    fn refresh_requested_during_capture_cannot_run_after_capture_closes() {
        let requested = Mutex::new(None);
        let mut state = Snapshot {
            mic_state: "idle".into(),
            auto_mode: false,
            ..Default::default()
        };
        queue_settings_refresh(
            &requested,
            &mut state,
            Some(1),
            false,
            SettingsRefresh::Manual,
        );
        assert!(state.device_settings_error.is_some());
        let mic = mic_lifecycle::Lifecycle::default();
        assert!(settings_refresh_allowed(&mic, Some(0), false));
        assert!(
            requested.lock().unwrap().take().is_none(),
            "closing capture must not revive a rejected request"
        );
        queue_settings_refresh(
            &requested,
            &mut state,
            Some(0),
            false,
            SettingsRefresh::Manual,
        );
        assert!(
            matches!(
                requested.lock().unwrap().take(),
                Some(SettingsRefresh::Manual)
            ),
            "a new idle request may proceed"
        );
        assert!(state.device_settings_error.is_none());
    }

    #[test]
    fn refresh_request_rejects_unknown_status_and_unfinished_capture_state() {
        let requested = Mutex::new(None);
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
            queue_settings_refresh(
                &requested,
                &mut state,
                streams,
                active,
                SettingsRefresh::Manual,
            );
            assert!(requested.lock().unwrap().is_none());
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
            queue_settings_refresh(
                &requested,
                &mut state,
                Some(0),
                false,
                SettingsRefresh::Manual,
            );
            assert!(requested.lock().unwrap().is_none(), "{phase}");
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
        assert!(policy.lock().unwrap().permission(true).is_some());
        assert!(state.lock().unwrap().auto_pause);
        assert!(*persisted.lock().unwrap());
    }
}
