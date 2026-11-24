//! Explicit, authenticated companion transfers; the PC coordinates both directions.
use crate::{bt, Command, Ctx, Event};
use ntpods_ipc::{CompanionStatus, CompanionTarget};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
#[path = "companion_calls.rs"]
mod calls;
#[path = "companion_media.rs"]
mod media_policy;
#[path = "companion_pairing.rs"]
mod pairing;
#[path = "dongle_presence.rs"]
mod presence;
use std::io;
use std::mem::{size_of, zeroed};
use std::path::PathBuf;
use std::ptr;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};
use windows_sys::core::GUID;
use windows_sys::Win32::Devices::Bluetooth::*;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Networking::WinSock::*;
use windows_sys::Win32::Security::Cryptography::*;

const SERVICE: &str = "bbc1e9f2-6f73-4c61-ae15-6e74706f6473";
const SERVICE_ID: GUID = GUID::from_u128(0xbbc1e9f2_6f73_4c61_ae15_6e74706f6473);
const ZERO_EPISODE: &str = "00000000000000000000000000000000";
const ZERO_TX: &str = "0000000000000000";
const MAX_FRAME: usize = 8192;
const PHASE_LIMIT: Duration = Duration::from_secs(30);
const TOTAL_LIMIT: Duration = Duration::from_secs(75);
const PEER_LIMIT: Duration = Duration::from_secs(45);

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Link {
    peer: u64,
    device: u64,
    secret: [u8; 32],
    held_away: bool,
}

impl Drop for Link {
    fn drop(&mut self) {
        wipe(&mut self.secret);
    }
}

#[derive(Default)]
pub(crate) struct State {
    link: Option<Link>,
    generation: u64,
    authorization: u64,
    connected: bool,
    phase: String,
    error: Option<String>,
    queued: Option<(CompanionTarget, u64)>,
    release: Option<Release>,
    native_counter: u64,
    native_open: Option<(u64, u64, u64)>,
    pairing: pairing::Pairing,
    output_override: u64,
    presentation: Option<std::sync::Arc<presence::Lease>>,
    presentation_peer: presence::Peer,
}

struct Release {
    address: u64,
    native_intent: u64,
    request: Option<u64>,
    intent: Option<u64>,
    control: Option<bool>,
    profiles: Option<bool>,
    native: Option<bool>,
    native_serial: Option<u64>,
}

impl Release {
    fn control_closed(&mut self, address: u64, request: u64, ok: bool) {
        if self.address == address && self.request == Some(request) {
            record(&mut self.control, ok);
        }
    }
    fn profiles_closed(&mut self, intent: u64, ok: bool) {
        if self.intent == Some(intent) {
            record(&mut self.profiles, ok);
        }
    }
    fn native_closed(&mut self, address: u64, intent: u64, serial: u64, ok: bool) {
        if self.address == address
            && self.native_intent == intent
            && self.native_serial == Some(serial)
        {
            record(&mut self.native, ok);
        }
    }
    fn complete(&self) -> Result<bool, &'static str> {
        let receipts = [self.control, self.profiles, self.native];
        if receipts.contains(&Some(false)) {
            Err("failed")
        } else {
            Ok(receipts.iter().all(|receipt| *receipt == Some(true)))
        }
    }
    fn ready(
        &self,
        driver_open: bool,
        microphone_quiet: bool,
        cleanup_pending: bool,
        native_active: bool,
    ) -> Result<bool, &'static str> {
        Ok(self.complete()?
            && !driver_open
            && microphone_quiet
            && !cleanup_pending
            && !native_active)
    }
}

impl State {
    fn transfer_phase(&mut self, phase: Phase) {
        self.phase = phase.status_name().into();
    }
    pub fn load() -> Self {
        let loaded = (|| {
            let path = path()?;
            let data = std::fs::read(path)?;
            if data.len() > 4096 {
                return Err(invalid());
            }
            let mut plain = protect(&data, false)?;
            let link = serde_json::from_slice::<Link>(&plain).map_err(|_| invalid());
            wipe(&mut plain);
            let link = link?;
            if !address_valid(link.peer) || !address_valid(link.device) || link.peer == link.device
            {
                return Err(invalid());
            }
            Ok(link)
        })();
        match loaded {
            Ok(link) => Self {
                link: Some(link),
                phase: "waiting".into(),
                ..Self::default()
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => Self::default(),
            Err(_) => Self {
                error: Some("Companion approval could not be read. Link the apps again.".into()),
                ..Self::default()
            },
        }
    }

    fn save(&self) -> io::Result<()> {
        let Some(link) = &self.link else {
            return match std::fs::remove_file(path()?) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
                _ => Ok(()),
            };
        };
        let mut plain = serde_json::to_vec(link).map_err(|_| invalid())?;
        let encrypted = protect(&plain, true);
        wipe(&mut plain);
        crate::atomic_file::write(&path()?, &encrypted?)
    }

    pub fn holds(&self, address: u64) -> bool {
        self.link
            .as_ref()
            .is_some_and(|link| link.device == address && link.held_away)
    }

    fn status(&self) -> CompanionStatus {
        CompanionStatus {
            linked: self.link.is_some(),
            connected: self.connected,
            device_id: self
                .link
                .as_ref()
                .map(|link| format!("{:012x}", link.device)),
            peer_address: self.link.as_ref().map(|link| mac(link.peer)),
            held_away: self.link.as_ref().is_some_and(|link| link.held_away),
            phase: if self.phase.is_empty() {
                "idle".into()
            } else {
                self.phase.clone()
            },
            error: self.error.clone(),
            pairing: self.pairing.status(),
        }
    }
}

pub fn publish(ctx: &Ctx) {
    publish_status(&ctx.companion, |status| ctx.send_event(&Event::CompanionStatus { status }));
}
fn publish_status(companion: &Mutex<State>, send: impl FnOnce(CompanionStatus)) {
    // Keep publication ordered with approval/window changes and their acknowledgements.
    let state = companion.lock().unwrap();
    send(state.status());
}

pub fn serializes(command: &Command) -> bool {
    matches!(
        command,
        Command::Connect { .. }
            | Command::Disconnect
            | Command::RepairConnection
            | Command::RebuildAudio
            | Command::PairingCompleted { .. }
            | Command::SetMicMode { .. }
            | Command::SetMicPolicy { .. }
            | Command::SetMicManualActive { .. }
            | Command::ToggleMicManual
            | Command::SetPlaybackMode { .. }
            | Command::RetryPlayback
            | Command::SetPlaybackBuffer { .. }
            | Command::SetPlaybackBitrate { .. }
            | Command::SetPlaybackLink { .. }
            | Command::SetPlaybackGameMode { .. }
            | Command::DongleControl {
                opcode: 1 | 2 | 4,
                ..
            }
    )
}

pub fn local_override(ctx: &Ctx, command: &Command) {
    if !serializes(command) && !matches!(command, Command::Shutdown) {
        return;
    }
    let mut state = ctx.companion.lock().unwrap();
    state.generation = state.generation.wrapping_add(1);
    state.output_override = state.output_override.wrapping_add(1);
    state.queued = None;
    state.release = None;
    state.phase = if state.connected { "idle" } else { "waiting" }.into();
}

pub fn pairing_override(ctx: &Ctx, command: &Command) { pairing::observe_local(ctx, command); }

pub fn local_connected(ctx: &Ctx) {
    let mut state = ctx.companion.lock().unwrap();
    if state.holds(ctx.mac()) {
        state.link.as_mut().unwrap().held_away = false;
        if state.save().is_err() {
            state.error = Some("Could not save the companion's local connection choice.".into());
        }
    }
}

pub fn microphone_started(ctx: &Ctx) {
    let mut state = ctx.companion.lock().unwrap();
    state.generation = state.generation.wrapping_add(1);
    state.output_override = state.output_override.wrapping_add(1);
    state.queued = None;
    state.release = None;
    state.phase = if state.connected { "idle" } else { "waiting" }.into();
}

#[derive(Clone)]
pub struct ResumeGuard {
    address: u64,
    generation: u64,
    authorization: u64,
    request: u64,
    intent: u64,
    native: Option<(u64, u64, u64)>,
    route: Option<MediaRoute>,
}

impl ResumeGuard {
    pub fn capture(ctx: &Ctx) -> Option<Self> {
        Self::capture_for(ctx, ctx.mac(), None)
    }

    fn capture_for(ctx: &Ctx, address: u64, route: Option<&MediaRoute>) -> Option<Self> {
        let route = match crate::dongle_routing::selected(ctx, address) {
            Some(Ok(_)) => Some(
                route
                    .filter(|(ticket, status, at)| {
                        ticket.current(ctx)
                            && at.elapsed() <= Duration::from_secs(1)
                            && status.peer == address.to_be_bytes()[2..]
                            && status.connected
                            && status.owned
                            && status.controls_ready
                            && !status.pending
                            && !status.releasing
                            && !status.busy
                    })?
                    .clone(),
            ),
            Some(Err(_)) => return None,
            None => {
                if !selected_ready(ctx, address) {
                    return None;
                }
                None
            }
        };
        if busy(ctx, address) {
            return None;
        }
        let state = ctx.companion.lock().unwrap();
        Some(Self {
            address,
            generation: state.generation,
            authorization: state.authorization,
            request: ctx.connect_requested.snapshot(),
            intent: bt::playback_intent(),
            native: state.native_open,
            route,
        })
    }

    pub fn current(&self, ctx: &Ctx) -> bool {
        self.current_route(ctx, self.route.as_ref())
    }

    fn current_route(&self, ctx: &Ctx, route: Option<&MediaRoute>) -> bool {
        let state = ctx.companion.lock().unwrap();
        let current = state.generation == self.generation
            && state.authorization == self.authorization
            && !state.holds(self.address)
            && state.native_open == self.native;
        drop(state);
        current
            && selected_pair(ctx, self.address)
            && !ctx.shutting_down.load(Ordering::Acquire)
            && ctx.connect_requested.is_current(self.request)
            && bt::playback_intent() == self.intent
            && match &self.route {
                Some((ticket, expected, _)) => {
                    ticket.current(ctx)
                        && route.is_some_and(|(current, status, at)| {
                            current.current(ctx)
                                && at.elapsed() <= Duration::from_secs(1)
                                && status.peer == expected.peer
                                && status.revision == expected.revision
                                && status.connected
                                && status.owned
                                && status.controls_ready
                                && !status.pending
                                && !status.releasing
                                && !status.busy
                        })
                }
                None => selected_ready(ctx, self.address),
            }
            && !busy(ctx, self.address)
    }
}

fn selected_ready(ctx: &Ctx, address: u64) -> bool {
    let snapshot = ctx.state.lock().unwrap().clone();
    if address == 0
        || snapshot.device_id != format!("{address:012x}")
        || !snapshot.connected
        || snapshot.link_stale
        || !snapshot.audio_output_ready
    {
        return false;
    }
    if snapshot.playback_mode != ntpods_ipc::PlaybackMode::Windows {
        #[cfg(feature = "experimental-playback")]
        {
            return ctx.connect_requested.explicit()
                && snapshot.playback_state == "ready"
                && crate::playback::active_address() == Some(address);
        }
        #[cfg(not(feature = "experimental-playback"))]
        {
            return false;
        }
    }
    true
}

pub fn apply_local(ctx: &Ctx, command: &Command) -> bool {
    if pairing::apply_local(ctx, command) { return true; }
    let result = match command {
        Command::CompanionStatus => {
            publish(ctx);
            crate::companion_prefs::publish(ctx);
            return true;
        }
        Command::CompanionLink {
            peer_address,
            secret_hex,
            device_id,
            request_id,
        } => {
            let _actions = ctx.companion_actions.lock().unwrap();
            let result = (|| {
                let peer = parse_mac(peer_address)
                    .ok_or("Use the bonded phone's Classic Bluetooth address.")?;
                let device = parse_device_id(device_id)
                    .ok_or("Select the paired AirPods before linking.")?;
                let secret = decode_hex::<32>(secret_hex)
                    .ok_or("Use a 32-byte lowercase hexadecimal companion secret.")?;
                if !selected_pair(ctx, device) || peer == device || !bonded(peer) {
                    return Err("Both the phone and selected AirPods must already be bonded.");
                }
                ctx.companion_preferences.lock().unwrap().set_consent(false)
                    .map_err(|_|"Local call consent could not be reset. Repair shared preferences before linking.")?;
                let mut state = ctx.companion.lock().unwrap();
                let held_away = state.holds(device);
                let old = state.link.replace(Link {
                    peer,
                    device,
                    secret,
                    held_away,
                });
                if state.save().is_err() {
                    state.link = old;
                    return Err("Could not protect the companion approval.");
                }
                state.generation = state.generation.wrapping_add(1);
                state.authorization = state.authorization.wrapping_add(1);
                state.presentation_peer = presence::Peer::default();
                state.connected = false;
                state.queued = None;
                state.release = None;
                state.error = None;
                state.phase = "waiting".into();
                Ok(())
            })();
            Some(("companion_link", *request_id, result))
        }
        Command::CompanionForget { request_id } => {
            let _actions = ctx.companion_actions.lock().unwrap();
            let _ = ctx.companion_preferences.lock().unwrap().set_consent(false);
            let mut state = ctx.companion.lock().unwrap();
            state.generation = state.generation.wrapping_add(1);
            state.authorization = state.authorization.wrapping_add(1);
            state.presentation_peer = presence::Peer::default();
            state.connected = false;
            state.queued = None;
            state.release = None;
            state.link = None;
            state.phase = "idle".into();
            let result = state
                .save()
                .map_err(|_| "Could not remove the protected companion approval.");
            Some(("companion_forget", *request_id, result))
        }
        Command::CompanionSwitch {
            target,
            device_id,
            request_id,
        } => {
            let _actions = ctx.companion_actions.lock().unwrap();
            let selected =
                parse_device_id(device_id).filter(|address| selected_pair(ctx, *address));
            let mut state = ctx.companion.lock().unwrap();
            let result = if selected.is_none()
                || state.link.as_ref().map(|link| link.device) != selected
            {
                Err("The companion is not linked for the selected AirPods.")
            } else if !state.connected {
                Err("Connect the approved companion app first.")
            } else if state.queued.is_some() || !matches!(state.phase.as_str(), "idle" | "waiting")
            {
                Err("A companion transfer is already pending.")
            } else {
                state.generation = state.generation.wrapping_add(1);
                state.queued = Some((*target, *request_id));
                Ok(())
            };
            if result.is_ok() {
                return true;
            }
            Some(("companion_switch", *request_id, result))
        }
        _ => return false,
    };
    if let Some((command, request_id, result)) = result {
        ctx.send_event(&Event::CommandResult {
            command: command.into(),
            request_id,
            ok: result.is_ok(),
            message: result.err().unwrap_or("Companion settings updated.").into(),
        });
    }
    publish(ctx);
    true
}

pub fn disconnect_reserved(ctx: &Ctx, request: u64, intent: Option<u64>) {
    if let Some(release) = ctx.companion.lock().unwrap().release.as_mut() {
        if release.address == ctx.mac() {
            release.request = Some(request);
            release.intent = intent;
            if intent.is_none() {
                release.profiles = Some(false);
            }
        }
    }
}

pub fn control_closed(ctx: &Ctx, address: u64, ok: bool) {
    let mut state = ctx.companion.lock().unwrap();
    if let Some(release) = state.release.as_mut() {
        let before = release.control;
        release.control_closed(address, ctx.connect_requested.snapshot(), ok);
        log_receipt_change("control", before, release.control);
    }
}

pub fn profiles_closed(ctx: &Ctx, intent: u64, ok: bool) {
    if let Some(release) = ctx.companion.lock().unwrap().release.as_mut() {
        let before = release.profiles;
        release.profiles_closed(intent, ok);
        log_receipt_change("profiles", before, release.profiles);
    }
}

pub fn native_started(ctx: &Ctx, address: u64, intent: u64) -> u64 {
    let mut state = ctx.companion.lock().unwrap();
    state.native_counter = state.native_counter.wrapping_add(1);
    let serial = state.native_counter;
    state.native_open = Some((address, intent, serial));
    if let Some(release) = state.release.as_mut() {
        if release.address == address && release.native_intent == intent {
            release.native = None;
            release.native_serial = Some(serial);
        }
    }
    serial
}

pub fn native_closed(ctx: &Ctx, address: u64, intent: u64, serial: u64, ok: bool) {
    let mut state = ctx.companion.lock().unwrap();
    if state.native_open == Some((address, intent, serial)) {
        state.native_open = None;
    }
    if let Some(release) = state.release.as_mut() {
        let before = release.native;
        release.native_closed(address, intent, serial, ok);
        log_receipt_change("native", before, release.native);
    }
}

fn receipt_name(receipt: Option<bool>) -> &'static str {
    match receipt {
        None => "pending",
        Some(true) => "closed",
        Some(false) => "failed",
    }
}

fn log_receipt_change(owner: &str, before: Option<bool>, after: Option<bool>) {
    if before != after {
        crate::log(&format!(
            "companion: release receipt owner={owner} state={}",
            receipt_name(after)
        ));
    }
}

fn record(value: &mut Option<bool>, ok: bool) {
    *value = Some(value.unwrap_or(true) && ok);
}

// The transport's approval epoch is fixed at authentication, not sampled again
// after waiting for a local command's reservation.
fn reserve_authorized<'a>(
    actions: &'a Mutex<()>,
    state: &Mutex<State>,
    authorization: u64,
) -> io::Result<MutexGuard<'a, ()>> {
    let reservation = actions.lock().unwrap();
    let state = state.lock().unwrap();
    if state.authorization != authorization || !state.connected {
        return Err(invalid());
    }
    Ok(reservation)
}

fn selected_pair(ctx: &Ctx, address: u64) -> bool {
    match crate::dongle_routing::selected(ctx, address) {
        Some(ticket) => ticket.is_ok(),
        None => ctx.mac() == address,
    }
}
fn route_connected(ctx: &Ctx, address: u64) -> bool {
    match crate::dongle_routing::selected(ctx, address) {
        Some(Ok(_)) => ctx.state.lock().unwrap().dongle.connected,
        Some(Err(_)) => false,
        None => selected_ready(ctx, address),
    }
}
fn busy(ctx: &Ctx, address: u64) -> bool {
    let snapshot = ctx.state.lock().unwrap().clone();
    ctx.capture_active.load(Ordering::Acquire)
        || ctx.mic_on.load(Ordering::Acquire)
        || snapshot.mic_capture_active
        || snapshot.mic_uplink_active
        || matches!(
            snapshot.mic_state.as_str(),
            "starting" | "capturing" | "stopping" | "recovering"
        )
        || snapshot.playback_buffer_target_ms.is_some()
        || crate::diagnostic_controls::active(&snapshot)
        || ctx.nearby_finder.lock().unwrap().active()
        || match crate::dongle_routing::selected(ctx, address) {
            Some(Ok(_)) => snapshot.dongle.microphone_active || snapshot.dongle.pending,
            Some(Err(_)) => true,
            None => crate::hfp_endpoints::Endpoints::new()
                .and_then(|endpoints| endpoints.busy(address))
                .unwrap_or(true),
        }
}

fn eligible(ctx: &Ctx, link: &Link) -> Result<(), &'static str> {
    if ctx.shutting_down.load(Ordering::Acquire)
        || !selected_pair(ctx, link.device)
        || (crate::dongle_routing::selected(ctx, link.device).is_none()
            && bt::paired_aap_name(link.device).is_none())
        || !bonded(link.peer)
    {
        return Err("unavailable");
    }
    if busy(ctx, link.device) {
        return Err("busy");
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    v: u8,
    #[serde(rename = "type")]
    kind: String,
    tx: String,
    device: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    sid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nonce: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    proof: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<CompanionTarget>,
    #[serde(skip_serializing_if = "Option::is_none")]
    phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ok: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    connected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    busy: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    can_release: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    can_acquire: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    caps: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rev: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    origin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mode: Option<ntpods_ipc::ConversationVolumeMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    first: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    second: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pause_media: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    left: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    right: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    purpose: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    episode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    call_gen: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    origin_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ready: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    hfp_ready: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sco: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    all_idle: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    probe: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    event: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    playing: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    known: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    available: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    media_gen: Option<String>,
}

impl Frame {
    fn new(kind: &str, tx: &str, device: u64, sid: Option<&str>) -> Self {
        Self {
            v: 2,
            kind: kind.into(),
            tx: tx.into(),
            device: mac(device),
            sid: sid.map(str::to_owned),
            nonce: None,
            proof: None,
            target: None,
            phase: None,
            ok: None,
            code: None,
            reason: None,
            connected: None,
            busy: None,
            can_release: None,
            can_acquire: None,
            caps: None,
            key: None,
            rev: None,
            origin: None,
            value: None,
            mode: None,
            first: None,
            second: None,
            pause_media: None,
            left: None,
            right: None,
            purpose: None,
            episode: None,
            call_gen: None,
            origin_token: None,
            ready: None,
            state: None,
            hfp_ready: None,
            sco: None,
            all_idle: None,
            probe: None,
            generation: None,
            event: None,
            playing: None,
            known: None,
            available: None,
            media_gen: None,
        }
    }

    fn preference(&self) -> io::Result<ntpods_ipc::CompanionPreference> {
        let key = self.key.as_deref().ok_or_else(invalid)?;
        let value = match key {
            "conversation_policy" => {
                serde_json::json!({"mode":self.mode.ok_or_else(invalid)?,"first":self.first.as_deref().ok_or_else(invalid)?,"second":self.second.as_deref().ok_or_else(invalid)?,"pause_media":self.pause_media.ok_or_else(invalid)?})
            }
            "desired_hold_actions" => {
                serde_json::json!({"left":self.left.as_deref().ok_or_else(invalid)?,"right":self.right.as_deref().ok_or_else(invalid)?})
            }
            _ => self.value.clone().ok_or_else(invalid)?,
        };
        let rev = self.rev.as_deref().ok_or_else(invalid)?;
        let origin = self.origin.as_deref().ok_or_else(invalid)?;
        if decode_hex::<8>(rev).is_none_or(|v| v == [0; 8])
            || !matches!(origin, "pc" | "android")
            || !crate::companion_prefs::valid(key, &value)
        {
            return Err(invalid());
        }
        Ok(ntpods_ipc::CompanionPreference {
            key: key.into(),
            rev: rev.into(),
            origin: origin.into(),
            value,
        })
    }

    fn parse(bytes: &[u8], device: u64, sid: Option<&str>) -> io::Result<Self> {
        if bytes.is_empty()
            || bytes.len() > MAX_FRAME
            || bytes.starts_with(&[0xef, 0xbb, 0xbf])
            || bytes.contains(&b'\r')
        {
            return Err(invalid());
        }
        let frame: Self = serde_json::from_slice(bytes).map_err(|_| invalid())?;
        let object: serde_json::Map<String, serde_json::Value> =
            serde_json::from_slice(bytes).map_err(|_| invalid())?;
        let extras: &[&str] = match frame.kind.as_str() {
            "hello" => &["nonce", "caps"],
            "challenge" => &["nonce", "proof", "sid", "caps"],
            "auth" => &["proof", "sid"],
            "authenticated" | "ping" | "release" | "acquire" => &["sid"],
            "status" => &["sid", "connected", "busy", "can_release", "can_acquire"],
            "switch" => &["sid", "target"],
            "media_query" => &["sid", "probe"],
            "media_state" => &[
                "sid",
                "probe",
                "generation",
                "event",
                "playing",
                "known",
                "connected",
                "available",
            ],
            "prepare" if frame.purpose.as_deref() == Some("media") => &[
                "sid",
                "target",
                "purpose",
                "episode",
                "call_gen",
                "origin_token",
                "media_gen",
            ],
            "prepare" => &[
                "sid",
                "target",
                "purpose",
                "episode",
                "call_gen",
                "origin_token",
            ],
            "pref" => match frame.key.as_deref() {
                Some("conversation_policy") => &[
                    "sid",
                    "key",
                    "rev",
                    "origin",
                    "mode",
                    "first",
                    "second",
                    "pause_media",
                ],
                Some("desired_hold_actions") => &["sid", "key", "rev", "origin", "left", "right"],
                Some(key) if crate::companion_prefs::KEYS.contains(&key) => {
                    &["sid", "key", "rev", "origin", "value"]
                }
                _ => return Err(invalid()),
            },
            "pref_ack" => &["sid", "key", "rev", "origin", "code"],
            "call_origin" => &["sid", "origin_token", "ready"],
            "call_begin" => &["sid", "episode", "call_gen", "origin_token"],
            "call_state" => &[
                "sid",
                "episode",
                "call_gen",
                "state",
                "hfp_ready",
                "sco",
                "all_idle",
            ],
            "call_end" => &["sid", "episode", "call_gen", "origin_token", "reason"],
            "result" => &["sid", "phase", "ok", "code"],
            "cancel" => &["sid", "reason"],
            _ => return Err(invalid()),
        };
        if frame.v != 2
            || frame.device != mac(device)
            || !address_valid(device)
            || decode_hex::<8>(&frame.tx).is_none()
            || object.len() != 4 + extras.len()
            || extras.iter().any(|key| !object.contains_key(*key))
            || object.values().any(|value| {
                value.is_null()
                    || value
                        .as_str()
                        .is_some_and(|text| text.contains(['\r', '\n', '\0']))
            })
        {
            return Err(invalid());
        }
        let zero = matches!(
            frame.kind.as_str(),
            "hello"
                | "challenge"
                | "auth"
                | "authenticated"
                | "ping"
                | "status"
                | "pref"
                | "pref_ack"
                | "call_origin"
                | "call_state"
                | "call_end"
                | "media_query"
                | "media_state"
        );
        if (frame.tx == ZERO_TX) != zero
            || frame
                .nonce
                .as_ref()
                .is_some_and(|value| decode_hex::<32>(value).is_none())
            || frame
                .proof
                .as_ref()
                .is_some_and(|value| decode_hex::<32>(value).is_none())
            || frame
                .sid
                .as_ref()
                .is_some_and(|value| decode_hex::<32>(value).is_none())
        {
            return Err(invalid());
        }
        if let Some(sid) = sid {
            if frame.sid.as_deref() != Some(sid) {
                return Err(invalid());
            }
        }
        if frame.kind == "hello" && frame.sid.is_some() {
            return Err(invalid());
        }
        if frame.kind == "result" {
            let success = match frame.phase.as_deref() {
                Some("prepare") => "ready",
                Some("release") => "released",
                Some("acquire" | "complete") => "acquired",
                Some("cancel") => "cancelled",
                _ => return Err(invalid()),
            };
            if if frame.ok == Some(true) {
                frame.code.as_deref() != Some(success)
                    && !(matches!(frame.phase.as_deref(), Some("acquire" | "complete"))
                        && frame.code.as_deref() == Some("hfp_ready"))
            } else {
                !matches!(
                    frame.code.as_deref(),
                    Some(
                        "busy"
                            | "unavailable"
                            | "invalid_state"
                            | "superseded"
                            | "timeout"
                            | "failed"
                    )
                )
            } {
                return Err(invalid());
            }
        }
        if frame.kind == "pref" {
            frame.preference()?;
        }
        if frame.kind == "pref_ack"
            && (!frame
                .key
                .as_deref()
                .is_some_and(|key| crate::companion_prefs::KEYS.contains(&key))
                || frame
                    .rev
                    .as_deref()
                    .and_then(decode_hex::<8>)
                    .is_none_or(|v| v == [0; 8])
                || !matches!(frame.origin.as_deref(), Some("pc" | "android"))
                || !matches!(
                    frame.code.as_deref(),
                    Some("applied" | "unchanged" | "unsupported" | "failed" | "stale")
                ))
        {
            return Err(invalid());
        }
        if matches!(frame.kind.as_str(), "hello" | "challenge")
            && frame
                .caps
                .as_deref()
                .and_then(decode_hex::<1>)
                .is_none_or(|v| v[0] & !7 != 0)
        {
            return Err(invalid());
        }
        if matches!(
            frame.kind.as_str(),
            "call_begin" | "call_state" | "call_end"
        ) && (frame
            .episode
            .as_deref()
            .and_then(decode_hex::<16>)
            .is_none_or(|v| v == [0; 16])
            || frame
                .call_gen
                .as_deref()
                .and_then(decode_hex::<8>)
                .is_none_or(|v| v == [0; 8]))
        {
            return Err(invalid());
        }
        if matches!(frame.kind.as_str(), "call_begin" | "call_end")
            && frame
                .origin_token
                .as_deref()
                .and_then(decode_hex::<16>)
                .is_none_or(|v| v == [0; 16])
        {
            return Err(invalid());
        }
        if frame.kind == "call_origin"
            && (frame.ready.is_none()
                || frame
                    .origin_token
                    .as_deref()
                    .and_then(decode_hex::<16>)
                    .is_none_or(|v| (v != [0; 16]) != frame.ready.unwrap_or(false)))
        {
            return Err(invalid());
        }
        if frame.kind == "call_state"
            && (!matches!(
                frame.state.as_deref(),
                Some("ringing" | "active" | "idle" | "unknown")
            ) || !matches!(frame.sco.as_deref(), Some("idle" | "connected" | "unknown"))
                || frame.hfp_ready.is_none()
                || frame.all_idle.is_none()
                || (frame.state.as_deref() == Some("idle")) != frame.all_idle.unwrap_or(false))
        {
            return Err(invalid());
        }
        if frame.kind == "call_end"
            && !matches!(frame.reason.as_deref(), Some("ended" | "invalidated"))
        {
            return Err(invalid());
        }
        if matches!(frame.kind.as_str(), "media_query" | "media_state") {
            if frame
                .probe
                .as_deref()
                .and_then(decode_hex::<8>)
                .is_none_or(|v| v == [0; 8])
            {
                return Err(invalid());
            }
            if frame.kind == "media_state"
                && (frame
                    .generation
                    .as_deref()
                    .and_then(decode_hex::<8>)
                    .is_none_or(|v| v == [0; 8])
                    || frame.event.as_deref().and_then(decode_hex::<8>).is_none())
            {
                return Err(invalid());
            }
        }
        if frame.kind == "prepare" {
            let ep = frame
                .episode
                .as_deref()
                .and_then(decode_hex::<16>)
                .ok_or_else(invalid)?;
            let gen = frame
                .call_gen
                .as_deref()
                .and_then(decode_hex::<8>)
                .ok_or_else(invalid)?;
            let origin = frame
                .origin_token
                .as_deref()
                .and_then(decode_hex::<16>)
                .ok_or_else(invalid)?;
            let valid = match frame.purpose.as_deref() {
                Some("manual") => ep == [0; 16] && gen == [0; 8] && origin == [0; 16],
                Some("media") => {
                    ep == [0; 16]
                        && gen == [0; 8]
                        && origin == [0; 16]
                        && frame
                            .media_gen
                            .as_deref()
                            .and_then(decode_hex::<8>)
                            .is_some_and(|v| v != [0; 8])
                }
                Some("call_takeover") => {
                    frame.target == Some(CompanionTarget::Android)
                        && ep != [0; 16]
                        && gen != [0; 8]
                        && origin != [0; 16]
                }
                Some("call_return") => {
                    frame.target == Some(CompanionTarget::Pc)
                        && ep != [0; 16]
                        && gen != [0; 8]
                        && origin != [0; 16]
                }
                _ => false,
            };
            if !valid {
                return Err(invalid());
            }
        }
        if frame.kind == "cancel"
            && !matches!(
                frame.reason.as_deref(),
                Some("user" | "timeout" | "lost" | "superseded" | "failed")
            )
        {
            return Err(invalid());
        }
        Ok(frame)
    }
}

fn transcript(
    device: u64,
    client: &str,
    server: &str,
    client_caps: &str,
    server_caps: &str,
) -> Vec<u8> {
    format!("NTPODS-COMPANION/2\nservice={SERVICE}\nclient=android\nserver=pc\ndevice={}\nclient_nonce={client}\nserver_nonce={server}\nclient_caps={client_caps}\nserver_caps={server_caps}\n", mac(device)).into_bytes()
}

fn digest(secret: Option<&[u8]>, data: &[u8]) -> io::Result<[u8; 32]> {
    unsafe {
        let mut algorithm = ptr::null_mut();
        if BCryptOpenAlgorithmProvider(
            &mut algorithm,
            BCRYPT_SHA256_ALGORITHM,
            ptr::null(),
            if secret.is_some() {
                BCRYPT_ALG_HANDLE_HMAC_FLAG
            } else {
                0
            },
        ) < 0
        {
            return Err(invalid());
        }
        let mut output = [0u8; 32];
        let key = secret.unwrap_or(&[]);
        let status = BCryptHash(
            algorithm,
            if secret.is_some() {
                key.as_ptr()
            } else {
                ptr::null()
            },
            key.len() as u32,
            data.as_ptr(),
            data.len() as u32,
            output.as_mut_ptr(),
            output.len() as u32,
        );
        BCryptCloseAlgorithmProvider(algorithm, 0);
        if status < 0 {
            Err(invalid())
        } else {
            Ok(output)
        }
    }
}

fn random<const N: usize>() -> io::Result<[u8; N]> {
    let mut output = [0; N];
    if unsafe {
        BCryptGenRandom(
            ptr::null_mut(),
            output.as_mut_ptr(),
            N as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    } < 0
    {
        Err(invalid())
    } else {
        Ok(output)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
fn decode_hex<const N: usize>(value: &str) -> Option<[u8; N]> {
    if value.len() != N * 2
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let mut output = [0; N];
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(output)
}
fn equal(left: &[u8; 32], right: &[u8; 32]) -> bool {
    left.iter()
        .zip(right)
        .fold(0, |difference, (a, b)| difference | (a ^ b))
        == 0
}
fn wipe(bytes: &mut [u8]) {
    for byte in bytes {
        unsafe { ptr::write_volatile(byte, 0) };
    }
}
fn address_valid(address: u64) -> bool {
    address != 0 && address < 0xffff_ffff_ffff && address != 0x0200_0000_0000
}
fn mac(address: u64) -> String {
    format!(
        "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
        address >> 40,
        (address >> 32) & 255,
        (address >> 24) & 255,
        (address >> 16) & 255,
        (address >> 8) & 255,
        address & 255
    )
}
fn parse_mac(value: &str) -> Option<u64> {
    if value.len() != 17
        || value.as_bytes().iter().enumerate().any(|(i, byte)| {
            if i % 3 == 2 {
                *byte != b':'
            } else {
                !byte.is_ascii_hexdigit()
            }
        })
    {
        return None;
    }
    let address = u64::from_str_radix(&value.replace(':', ""), 16).ok()?;
    address_valid(address).then_some(address)
}
fn parse_device_id(value: &str) -> Option<u64> {
    if value.len() != 12 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let address = u64::from_str_radix(value, 16).ok()?;
    address_valid(address).then_some(address)
}
fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "Invalid companion protocol or protected approval",
    )
}
fn path() -> io::Result<PathBuf> {
    Ok(
        PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or_else(invalid)?)
            .join("NTPods")
            .join("companion.dat"),
    )
}
fn protect(data: &[u8], encrypt: bool) -> io::Result<Vec<u8>> {
    if data.is_empty() || data.len() > 4096 {
        return Err(invalid());
    }
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: data.len() as u32,
            pbData: data.as_ptr().cast_mut(),
        };
        let mut output: CRYPT_INTEGER_BLOB = zeroed();
        let ok = if encrypt {
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
            Err(invalid())
        };
        wipe(bytes);
        LocalFree(output.pbData.cast());
        result
    }
}

fn bonded(address: u64) -> bool {
    unsafe {
        let mut info: BLUETOOTH_DEVICE_INFO = zeroed();
        info.dwSize = size_of::<BLUETOOTH_DEVICE_INFO>() as u32;
        info.Address.Anonymous.ullLong = address;
        BluetoothGetDeviceInfo(ptr::null_mut(), &mut info) == 0
            && info.fAuthenticated != 0
            && info.fRemembered != 0
    }
}

struct Winsock;
impl Winsock {
    fn open() -> io::Result<Self> {
        let mut data: WSADATA = unsafe { zeroed() };
        let result = unsafe { WSAStartup(0x0202, &mut data) };
        if result != 0 {
            Err(io::Error::from_raw_os_error(result))
        } else {
            Ok(Self)
        }
    }
}
impl Drop for Winsock {
    fn drop(&mut self) {
        unsafe {
            WSACleanup();
        }
    }
}

struct Socket(SOCKET);
impl Socket {
    fn nonblocking(&self) -> io::Result<()> {
        let mut enabled = 1;
        if unsafe { ioctlsocket(self.0, FIONBIO, &mut enabled) } == SOCKET_ERROR {
            Err(socket_error())
        } else {
            Ok(())
        }
    }
    fn secure(&self) -> io::Result<()> {
        let required = 1u32;
        for option in [SO_BTH_AUTHENTICATE, SO_BTH_ENCRYPT] {
            if unsafe {
                setsockopt(
                    self.0,
                    SOL_RFCOMM as i32,
                    option as i32,
                    (&required as *const u32).cast(),
                    size_of::<u32>() as i32,
                )
            } == SOCKET_ERROR
            {
                return Err(socket_error());
            }
        }
        Ok(())
    }
}
impl Drop for Socket {
    fn drop(&mut self) {
        unsafe {
            closesocket(self.0);
        }
    }
}
fn socket_error() -> io::Error {
    io::Error::from_raw_os_error(unsafe { WSAGetLastError() })
}
fn would_block(error: &io::Error) -> bool {
    error.raw_os_error() == Some(WSAEWOULDBLOCK)
}

struct Listener {
    socket: Socket,
    address: SOCKADDR_BTH,
    _winsock: Winsock,
    service_id: GUID,
}
impl Listener {
    fn open() -> io::Result<Self> {
        Self::open_service(SERVICE_ID)
    }
    fn open_service(service_id: GUID) -> io::Result<Self> {
        let winsock = Winsock::open()?;
        let raw = unsafe { socket(AF_BTH as i32, SOCK_STREAM, BTHPROTO_RFCOMM as i32) };
        if raw == INVALID_SOCKET {
            return Err(socket_error());
        }
        let socket = Socket(raw);
        socket.secure()?;
        socket.nonblocking()?;
        let mut address: SOCKADDR_BTH = unsafe { zeroed() };
        address.addressFamily = AF_BTH;
        address.port = u32::MAX; // BT_PORT_ANY: reserve an available RFCOMM channel.
        if unsafe {
            bind(
                raw,
                (&address as *const SOCKADDR_BTH).cast(),
                size_of::<SOCKADDR_BTH>() as i32,
            )
        } == SOCKET_ERROR
            || unsafe { listen(raw, 1) } == SOCKET_ERROR
        {
            return Err(socket_error());
        }
        let mut length = size_of::<SOCKADDR_BTH>() as i32;
        if unsafe { getsockname(raw, (&mut address as *mut SOCKADDR_BTH).cast(), &mut length) }
            == SOCKET_ERROR
            || length != size_of::<SOCKADDR_BTH>() as i32
        {
            return Err(socket_error());
        }
        let mut server = Self {
            socket,
            address,
            _winsock: winsock,
            service_id,
        };
        server.advertise(RNRSERVICE_REGISTER)?;
        Ok(server)
    }

    fn advertise(&mut self, operation: WSAESETSERVICEOP) -> io::Result<()> {
        let mut name = crate::wide("NTPods Companion");
        let mut service = self.service_id;
        let mut location: CSADDR_INFO = unsafe { zeroed() };
        location.LocalAddr.lpSockaddr = (&mut self.address as *mut SOCKADDR_BTH).cast();
        location.LocalAddr.iSockaddrLength = size_of::<SOCKADDR_BTH>() as i32;
        location.iSocketType = SOCK_STREAM;
        location.iProtocol = BTHPROTO_RFCOMM as i32;
        let mut query: WSAQUERYSETW = unsafe { zeroed() };
        query.dwSize = size_of::<WSAQUERYSETW>() as u32;
        query.lpszServiceInstanceName = name.as_mut_ptr();
        query.lpServiceClassId = &mut service;
        query.dwNameSpace = NS_BTH;
        query.dwNumberOfCsAddrs = 1;
        query.lpcsaBuffer = &mut location;
        if unsafe { WSASetServiceW(&query, operation, 0) } == SOCKET_ERROR {
            Err(socket_error())
        } else {
            Ok(())
        }
    }

    fn accept(&self, approved: u64) -> io::Result<Option<Framed>> {
        let mut peer: SOCKADDR_BTH = unsafe { zeroed() };
        let mut length = size_of::<SOCKADDR_BTH>() as i32;
        let raw = unsafe {
            accept(
                self.socket.0,
                (&mut peer as *mut SOCKADDR_BTH).cast(),
                &mut length,
            )
        };
        if raw == INVALID_SOCKET {
            let error = socket_error();
            return if would_block(&error) {
                Ok(None)
            } else {
                Err(error)
            };
        }
        let socket = Socket(raw);
        if length != size_of::<SOCKADDR_BTH>() as i32
            || peer.addressFamily != AF_BTH
            || peer.btAddr != approved
            || !bonded(approved)
        {
            return Ok(None);
        }
        socket.nonblocking()?;
        Ok(Some(Framed {
            socket,
            pending: Vec::new(),
        }))
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        let _ = self.advertise(RNRSERVICE_DELETE);
    }
}

struct Framed {
    socket: Socket,
    pending: Vec<u8>,
}
impl Framed {
    fn poll(&mut self, device: u64, sid: Option<&str>) -> io::Result<Option<Frame>> {
        self.poll_line()?.map(|line| Frame::parse(&line, device, sid)).transpose()
    }
    fn poll_line(&mut self) -> io::Result<Option<Vec<u8>>> {
        if let Some(line) = take_line(&mut self.pending)? {
            return Ok(Some(line));
        }
        let mut bytes = [0u8; 2048];
        let count = unsafe { recv(self.socket.0, bytes.as_mut_ptr(), bytes.len() as i32, 0) };
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Companion disconnected",
            ));
        }
        if count == SOCKET_ERROR {
            let error = socket_error();
            return if would_block(&error) {
                Ok(None)
            } else {
                Err(error)
            };
        }
        self.pending.extend_from_slice(&bytes[..count as usize]);
        take_line(&mut self.pending)
    }
    fn send(&self, frame: &Frame) -> io::Result<()> {
        self.send_json(frame)
    }
    fn send_json(&self, frame: &impl Serialize) -> io::Result<()> {
        let mut data = serde_json::to_vec(frame).map_err(|_| invalid())?;
        if data.len() > MAX_FRAME {
            return Err(invalid());
        }
        data.push(b'\n');
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut sent = 0;
        while sent < data.len() {
            let count = unsafe {
                send(
                    self.socket.0,
                    data[sent..].as_ptr(),
                    (data.len() - sent) as i32,
                    0,
                )
            };
            if count == SOCKET_ERROR {
                let error = socket_error();
                if !would_block(&error) || Instant::now() >= deadline {
                    return Err(error);
                }
                thread::sleep(Duration::from_millis(10));
            } else if count == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "Companion send stopped",
                ));
            } else {
                sent += count as usize;
            }
        }
        Ok(())
    }
    fn wait(&mut self, device: u64, sid: Option<&str>, deadline: Instant) -> io::Result<Frame> {
        self.wait_current(device, sid, deadline, &|| true)
    }
    fn wait_current(&mut self, device: u64, sid: Option<&str>, deadline: Instant, current: &impl Fn() -> bool) -> io::Result<Frame> {
        loop {
            if !current() { unsafe { shutdown(self.socket.0, SD_BOTH); } return Err(invalid()); }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Companion authentication timed out",
                ));
            }
            if let Some(frame) = self.poll(device, sid)? {
                return Ok(frame);
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}

fn take_line(pending: &mut Vec<u8>) -> io::Result<Option<Vec<u8>>> {
    if let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
        if end == 0 || end > MAX_FRAME {
            return Err(invalid());
        }
        let line = pending.drain(..=end).take(end).collect::<Vec<_>>();
        Ok(Some(line))
    } else if pending.len() > MAX_FRAME {
        Err(invalid())
    } else {
        Ok(None)
    }
}

fn authenticate(stream: &mut Framed, link: &Link) -> io::Result<(String, u8)> {
    authenticate_committing(stream, link, || true, || Ok(()))
}
fn authenticate_committing(stream: &mut Framed, link: &Link, current: impl Fn() -> bool, commit: impl FnOnce() -> io::Result<()>) -> io::Result<(String, u8)> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let hello = stream.wait_current(link.device, None, deadline, &current)?;
    if hello.kind != "hello" {
        return Err(invalid());
    }
    let server_nonce = hex(&random::<32>()?);
    let server_caps = format!(
        "{:02x}",
        decode_hex::<1>(hello.caps.as_deref().ok_or_else(invalid)?).ok_or_else(invalid)?[0] & 7
    );
    let transcript = transcript(
        link.device,
        hello.nonce.as_deref().ok_or_else(invalid)?,
        &server_nonce,
        hello.caps.as_deref().ok_or_else(invalid)?,
        &server_caps,
    );
    let sid = hex(&digest(None, &transcript)?);
    let mut challenge = Frame::new("challenge", ZERO_TX, link.device, Some(&sid));
    challenge.nonce = Some(server_nonce);
    challenge.caps = Some(server_caps);
    challenge.proof = Some(hex(&digest(
        Some(&link.secret),
        &[b"server\n".as_slice(), &transcript].concat(),
    )?));
    stream.send(&challenge)?;
    let auth = stream.wait_current(link.device, Some(&sid), deadline, &current)?;
    let expected = digest(
        Some(&link.secret),
        &[b"client\n".as_slice(), &transcript].concat(),
    )?;
    if auth.kind != "auth"
        || !equal(
            &expected,
            &decode_hex::<32>(auth.proof.as_deref().ok_or_else(invalid)?).ok_or_else(invalid)?,
        )
    {
        return Err(invalid());
    }
    commit()?;
    stream.send(&Frame::new(
        "authenticated",
        ZERO_TX,
        link.device,
        Some(&sid),
    ))?;
    Ok((
        sid,
        decode_hex::<1>(hello.caps.as_deref().ok_or_else(invalid)?).ok_or_else(invalid)?[0] & 7,
    ))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    RouteProbe,
    Pause,
    Prepare,
    PhoneRelease,
    PcRelease,
    PhoneAcquire,
    PcAcquire,
}

impl Phase {
    fn status_name(self) -> &'static str {
        match self {
            Self::RouteProbe | Self::Pause | Self::Prepare => "prepare",
            Self::PhoneRelease => "release_android",
            Self::PcRelease => "release_pc",
            Self::PhoneAcquire => "acquire_android",
            Self::PcAcquire => "acquire_pc",
        }
    }
}

fn publish_phase(ctx: &Ctx, phase: Phase) {
    ctx.companion.lock().unwrap().transfer_phase(phase);
    crate::log(&format!("companion: phase={}", phase.status_name()));
    publish(ctx);
}
struct DongleTransfer {
    ticket: crate::dongle_routing::Ticket,
    revision: u32,
    revision_at: Instant,
    preparing: bool,
    job: Mutex<Option<crate::dongle_routing::Job>>,
    result: Mutex<Option<Result<crate::dongle_routing::Status, &'static str>>>,
}
impl DongleTransfer {
    fn observed_current(&self, observed: Option<&MediaRoute>) -> bool {
        if self.preparing || self.revision == 0 {
            return true;
        }
        if self.poll().as_ref().is_some_and(Result::is_err) {
            return false;
        }
        let Some((ticket, status, _)) = observed else {
            return true;
        };
        if !ticket.same_physical(&self.ticket) {
            return false;
        }
        let latest = self.result.lock().unwrap();
        let expected = latest
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .map_or((self.revision, self.revision_at), |status| {
                (status.revision, status.received)
            });
        media_policy::routing_current(
            expected,
            (status.revision, status.received),
            self.ticket.pending_receipt(),
            status.receipt(),
        )
    }
    fn poll(&self) -> Option<Result<crate::dongle_routing::Status, &'static str>> {
        let mut result = self.result.lock().unwrap();
        if result.is_none() {
            *result = self.job.lock().unwrap().as_ref().and_then(|job| job.poll());
        }
        result.clone()
    }
}
struct Transfer {
    tx: String,
    target: CompanionTarget,
    call: Option<calls::Context>,
    media_gen: Option<u64>,
    media_source_gen: Option<u64>,
    dongle: Option<DongleTransfer>,
    media_fence: Option<std::sync::Arc<Mutex<(bool, Instant)>>>,
    authorization: u64,
    request_id: Option<u64>,
    generation: u64,
    request: u64,
    audio_intent: u64,
    phase: Phase,
    phase_deadline: Instant,
    total_deadline: Instant,
}

impl Transfer {
    fn enter(&mut self, phase: Phase) {
        self.phase = phase;
        self.phase_deadline = (Instant::now() + PHASE_LIMIT).min(self.total_deadline);
    }
    fn current(&self, ctx: &Ctx, link: &Link) -> bool {
        let (generation, authorization) = {
            let state = ctx.companion.lock().unwrap();
            (state.generation, state.authorization)
        };
        authorization == self.authorization
            && self.media_current()
            && (selected_pair(ctx, link.device) || self.transitioning(ctx))
            && self.dongle.as_ref().is_none_or(|route| {
                route.ticket.current(ctx)
                    && route
                        .result
                        .lock()
                        .unwrap()
                        .as_ref()
                        .is_none_or(Result::is_ok)
            })
            && !ctx.shutting_down.load(Ordering::Acquire)
            && self.tokens_current(
                generation,
                ctx.connect_requested.snapshot(),
                bt::playback_intent(),
            )
    }
    fn transitioning(&self, ctx: &Ctx) -> bool {
        self.dongle.as_ref().is_some_and(|route| {
            route.ticket.transitioning(ctx)
                && route.job.lock().unwrap().is_some()
                && route.result.lock().unwrap().is_none()
        })
    }
    fn media_current(&self) -> bool {
        self.media_fence.as_ref().is_none_or(|fence| {
            let (allowed, at) = *fence.lock().unwrap();
            allowed && at.elapsed() <= Duration::from_secs(1)
        })
    }
    fn tokens_current(&self, generation: u64, request: u64, audio_intent: u64) -> bool {
        generation == self.generation
            && request == self.request
            && audio_intent == self.audio_intent
    }
    fn accepts_result(&self, frame: &Frame) -> bool {
        let expected = match self.phase {
            Phase::Prepare => "prepare",
            Phase::PhoneRelease => "release",
            Phase::PhoneAcquire => "acquire",
            _ => return false,
        };
        frame.tx == self.tx
            && frame.kind == "result"
            && frame.phase.as_deref() == Some(expected)
            && (frame.ok != Some(true)
                || self.phase != Phase::PhoneAcquire
                || frame.code.as_deref()
                    == Some(if self.call.as_ref().is_some_and(|call| !call.returning) {
                        "hfp_ready"
                    } else {
                        "acquired"
                    }))
    }
    fn expired(&self, now: Instant) -> bool {
        now >= self.phase_deadline || now >= self.total_deadline
    }
}

fn status_frame(ctx: &Ctx, link: &Link, sid: &str) -> Frame {
    let unavailable = !selected_pair(ctx, link.device) || !bonded(link.peer);
    let busy = busy(ctx, link.device);
    let mut status = Frame::new("status", ZERO_TX, link.device, Some(sid));
    status.connected = Some(route_connected(ctx, link.device));
    status.busy = Some(busy);
    status.can_release = Some(!busy && !unavailable);
    status.can_acquire = Some(!busy && !unavailable);
    status
}

fn result_frame(link: &Link, sid: &str, tx: &str, phase: &str, outcome: Result<(), &str>) -> Frame {
    let mut frame = Frame::new("result", tx, link.device, Some(sid));
    frame.phase = Some(phase.into());
    frame.ok = Some(outcome.is_ok());
    frame.code = Some(
        match outcome {
            Ok(()) => {
                if phase == "prepare" {
                    "ready"
                } else if phase == "release" {
                    "released"
                } else if phase == "cancel" {
                    "cancelled"
                } else {
                    "acquired"
                }
            }
            Err(code) => code,
        }
        .into(),
    );
    frame
}

fn notify_local(ctx: &Ctx, request_id: Option<u64>, outcome: Result<(), &str>) {
    if let Some(request_id) = request_id {
        ctx.send_event(&Event::CommandResult {
            command: "companion_switch".into(),
            request_id,
            ok: outcome.is_ok(),
            message: match outcome {
                Ok(()) => "The destination audio route is connected.",
                Err("busy") => "Stop the microphone or call before switching.",
                Err("superseded") => "A newer local choice cancelled the transfer.",
                Err("timeout") => "The transfer timed out. Choose a device again.",
                Err("unavailable") => "The selected companion or audio route is unavailable.",
                _ => "The transfer could not be confirmed. Choose a device again.",
            }
            .into(),
        });
    }
}

fn finish(ctx: &Ctx, transfer: &Transfer, outcome: Result<(), &str>) {
    if let Some(fence) = &transfer.media_fence {
        *fence.lock().unwrap() = (false, Instant::now());
    }
    let mut state = ctx.companion.lock().unwrap();
    let receipt_summary = state.release.as_ref().map(|release| {
        format!(
            "companion: release receipts control={} profiles={} native={}",
            receipt_name(release.control),
            receipt_name(release.profiles),
            receipt_name(release.native)
        )
    });
    if state.generation == transfer.generation {
        state.release = None;
        state.phase = "idle".into();
        state.error = outcome
            .err()
            .map(|code| format!("Companion transfer: {code}. Choose a device again."));
    }
    drop(state);
    if let Some(summary) = receipt_summary {
        crate::log(&summary);
    }
    crate::log(&format!(
        "companion: complete ok={} code={}",
        outcome.is_ok(),
        outcome.err().unwrap_or("acquired")
    ));
    notify_local(ctx, transfer.request_id, outcome);
    publish(ctx);
}

fn begin_transfer(
    ctx: &Ctx,
    link: &Link,
    tx: String,
    target: CompanionTarget,
    request_id: Option<u64>,
    authorization: u64,
) -> Result<Transfer, &'static str> {
    let started = Instant::now();
    let _actions = reserve_authorized(&ctx.companion_actions, &ctx.companion, authorization)
        .map_err(|_| "superseded")?;
    eligible(ctx, link)?;
    let mut state = ctx.companion.lock().unwrap();
    state.error = None;
    let mut transfer = Transfer {
        tx,
        target,
        call: None,
        media_gen: None,
        media_source_gen: None,
        dongle: None,
        media_fence: None,
        authorization,
        request_id,
        generation: state.generation,
        request: ctx.connect_requested.snapshot(),
        audio_intent: bt::playback_intent(),
        phase: Phase::Prepare,
        phase_deadline: started + PHASE_LIMIT,
        total_deadline: started + TOTAL_LIMIT,
    };
    drop(state);
    if let Some(ticket) = crate::dongle_routing::selected(ctx, link.device) {
        let ticket = ticket?;
        transfer.dongle = Some(DongleTransfer {
            job: Mutex::new(Some(crate::dongle_routing::probe(
                ctx.clone(),
                ticket.clone(),
            ))),
            ticket,
            revision: 0,
            revision_at: Instant::now(),
            preparing: true,
            result: Mutex::new(None),
        });
        transfer.phase = Phase::RouteProbe;
    }
    Ok(transfer)
}

fn start_dongle_action(
    ctx: &Ctx,
    link: &Link,
    transfer: &mut Transfer,
    acquire: bool,
) -> Result<bool, &'static str> {
    if transfer.dongle.is_none() {
        return Ok(false);
    }
    {
        let mut state = ctx.companion.lock().unwrap();
        let link = state.link.as_mut().ok_or("unavailable")?;
        let previous = link.held_away;
        link.held_away = !acquire;
        if state.save().is_err() {
            state.link.as_mut().unwrap().held_away = previous;
            return Err("failed");
        }
    }
    let (generation, authorization, request, intent, address) = (
        transfer.generation,
        transfer.authorization,
        transfer.request,
        transfer.audio_intent,
        link.device,
    );
    let context = ctx.clone();
    let fence = transfer.media_fence.clone();
    let route = transfer.dongle.as_mut().unwrap();
    let ticket = route.ticket.clone();
    *route.result.lock().unwrap() = None;
    *route.job.get_mut().unwrap() = Some(crate::dongle_routing::act(
        ctx.clone(),
        route.ticket.clone(),
        route.revision,
        acquire,
        move || {
            let state = context.companion.lock().unwrap();
            let authorized = state.generation == generation && state.authorization == authorization;
            drop(state);
            authorized
                && (selected_pair(&context, address) || ticket.transitioning(&context))
                && context.connect_requested.is_current(request)
                && bt::playback_intent() == intent
                && fence.as_ref().is_none_or(|fence| {
                    let (allowed, at) = *fence.lock().unwrap();
                    allowed && at.elapsed() <= Duration::from_secs(1)
                })
        },
    ));
    Ok(true)
}

fn release_pc(ctx: &Ctx, link: &Link, transfer: &mut Transfer) -> Result<(), &'static str> {
    let _actions = ctx.companion_actions.lock().unwrap();
    if !transfer.current(ctx, link) || transfer.expired(Instant::now()) {
        return Err("superseded");
    }
    eligible(ctx, link)?;
    transfer.enter(Phase::PcRelease);
    if start_dongle_action(ctx, link, transfer, false)? {
        publish_phase(ctx, Phase::PcRelease);
        return Ok(());
    }
    {
        let mut state = ctx.companion.lock().unwrap();
        let old_hold = state.link.as_ref().ok_or("unavailable")?.held_away;
        state.link.as_mut().unwrap().held_away = true;
        if state.save().is_err() {
            state.link.as_mut().unwrap().held_away = old_hold;
            return Err("failed");
        }
        let native_serial = state
            .native_open
            .filter(|(address, _, _)| *address == link.device)
            .map(|(_, _, serial)| serial);
        state.release = Some(Release {
            address: link.device,
            native_intent: transfer.audio_intent,
            request: None,
            intent: None,
            control: None,
            profiles: None,
            native: native_serial.is_none().then_some(true),
            native_serial,
        });
    }
    publish_phase(ctx, Phase::PcRelease);
    crate::disconnect_device(ctx);
    transfer.request = ctx.connect_requested.snapshot();
    transfer.audio_intent = bt::playback_intent();
    Ok(())
}

fn released_pc(ctx: &Ctx, link: &Link, transfer: &Transfer) -> Result<bool, &'static str> {
    if let Some(route) = &transfer.dongle {
        return route
            .poll()
            .transpose()
            .map(|status| status.is_some_and(|s| s.released && !s.connected && !s.owned));
    }
    let driver_open = ctx.driver_cell.lock().unwrap().is_some();
    let microphone_quiet = ctx.hq_mic_quiet.load(Ordering::Acquire);
    let cleanup_pending = ctx.pending_audio_disconnect.lock().unwrap().is_pending();
    #[cfg(feature = "experimental-playback")]
    let native_active = crate::playback::active_address() == Some(link.device);
    #[cfg(not(feature = "experimental-playback"))]
    let native_active = false;
    let ready = {
        let state = ctx.companion.lock().unwrap();
        let release = state.release.as_ref().ok_or("superseded")?;
        release.ready(
            driver_open,
            microphone_quiet,
            cleanup_pending,
            native_active,
        )?
    };
    if ready {
        // The cached Classic connection flag is not audio ownership: idle
        // control profiles may retain the link after all audio owners close.
        crate::log(&format!(
            "companion: audio release confirmed classic_connected_advisory={}",
            bt::is_connected(link.device)
        ));
    }
    Ok(ready)
}

fn acquire_pc(ctx: &Ctx, link: &Link, transfer: &mut Transfer) -> Result<(), &'static str> {
    let _actions = ctx.companion_actions.lock().unwrap();
    if transfer.call.is_some() && transfer.dongle.is_none() && crate::dongle::inhibits(ctx, false) {
        return Err("superseded");
    }
    if !transfer.current(ctx, link) || transfer.expired(Instant::now()) {
        return Err("superseded");
    }
    eligible(ctx, link)?;
    transfer.enter(Phase::PcAcquire);
    if start_dongle_action(ctx, link, transfer, true)? {
        publish_phase(ctx, Phase::PcAcquire);
        return Ok(());
    }
    let _microphone = ctx.mic_preference.lock().unwrap();
    let mut snapshot = ctx.state.lock().unwrap();
    // A call ending must not pull the AirPods away from a newly attached dongle.
    // An explicit Phone/This PC choice retains its existing native behavior.
    if transfer.call.is_some() && crate::dongle::inhibits_native(&snapshot.dongle, false) {
        return Err("superseded");
    }
    {
        let mut state = ctx.companion.lock().unwrap();
        let old_hold = state.link.as_ref().ok_or("unavailable")?.held_away;
        state.link.as_mut().unwrap().held_away = false;
        if state.save().is_err() {
            state.link.as_mut().unwrap().held_away = old_hold;
            return Err("failed");
        }
    }
    publish_phase(ctx, Phase::PcAcquire);
    if !crate::request_audio_connection(ctx, &mut snapshot, "companion user choice") {
        return Err("failed");
    }
    transfer.request = ctx.connect_requested.snapshot();
    transfer.audio_intent = bt::playback_intent();
    drop(snapshot);
    ctx.push_state();
    Ok(())
}

fn acquired_pc(ctx: &Ctx, link: &Link, transfer: &Transfer) -> bool {
    if let Some(route) = &transfer.dongle {
        return route.poll().is_some_and(|result| {
            result.is_ok_and(|s| s.connected && s.controls_ready && s.owned && !s.pending)
        });
    }
    let snapshot = ctx.state.lock().unwrap().clone();
    snapshot.device_id == format!("{:012x}", link.device)
        && snapshot.connected
        && !snapshot.link_stale
        && snapshot.audio_output_ready
}

fn send_phase(
    stream: &Framed,
    ctx: &Ctx,
    link: &Link,
    sid: &str,
    transfer: &mut Transfer,
    kind: &str,
    phase: Phase,
) -> Result<(), &'static str> {
    let _actions = ctx.companion_actions.lock().unwrap();
    if !transfer.current(ctx, link) {
        return Err("superseded");
    }
    if transfer.expired(Instant::now()) {
        return Err("timeout");
    }
    eligible(ctx, link)?;
    if kind == "prepare"
        && transfer
            .dongle
            .as_ref()
            .is_some_and(|route| route.preparing)
    {
        transfer.enter(Phase::RouteProbe);
        return Ok(());
    }
    if phase != transfer.phase {
        transfer.enter(phase);
    }
    let frame = if kind == "prepare" {
        prepare_frame(link, sid, transfer)
    } else {
        Frame::new(kind, &transfer.tx, link.device, Some(sid))
    };
    stream.send(&frame).map_err(|_| "failed")?;
    publish_phase(ctx, phase);
    Ok(())
}

fn prepare_frame(link: &Link, sid: &str, transfer: &Transfer) -> Frame {
    let mut frame = Frame::new("prepare", &transfer.tx, link.device, Some(sid));
    frame.target = Some(transfer.target);
    frame.purpose = Some(
        transfer
            .call
            .as_ref()
            .map_or(
                if transfer.media_gen.is_some() {
                    "media"
                } else {
                    "manual"
                },
                |call| call.purpose(),
            )
            .into(),
    );
    frame.episode = Some(
        transfer
            .call
            .as_ref()
            .map_or(ZERO_EPISODE, |call| call.episode.as_str())
            .into(),
    );
    frame.call_gen = Some(
        transfer
            .call
            .as_ref()
            .map_or(ZERO_TX, |call| call.call_gen.as_str())
            .into(),
    );
    frame.media_gen = transfer
        .media_gen
        .map(|generation| format!("{generation:016x}"));
    frame.origin_token = Some(
        transfer
            .call
            .as_ref()
            .map_or(ZERO_EPISODE, |call| call.origin_token.as_str())
            .into(),
    );
    frame
}

fn complete_success(
    stream: &Framed,
    ctx: &Ctx,
    link: &Link,
    sid: &str,
    transfer: &Transfer,
) -> Result<(), &'static str> {
    let _actions = ctx.companion_actions.lock().unwrap();
    if !transfer.current(ctx, link) {
        return Err("superseded");
    }
    if transfer.expired(Instant::now()) {
        return Err("timeout");
    }
    eligible(ctx, link)?;
    let mut result = result_frame(link, sid, &transfer.tx, "complete", Ok(()));
    if transfer.call.as_ref().is_some_and(|call| !call.returning) {
        result.code = Some("hfp_ready".into());
    }
    stream.send(&result).map_err(|_| "failed")?;
    finish(ctx, transfer, Ok(()));
    Ok(())
}

fn cancel_transfer(
    stream: &Framed,
    ctx: &Ctx,
    link: &Link,
    sid: &str,
    transfer: Transfer,
    code: &str,
    cancelled: &mut HashSet<String>,
    calls: &mut calls::Calls,
) -> io::Result<()> {
    calls.retain_media_release(ctx, link, &transfer);
    crate::log(&format!("companion: cancel code={code}"));
    let mut frame = Frame::new("cancel", &transfer.tx, link.device, Some(sid));
    frame.reason = Some(
        if matches!(code, "superseded" | "timeout") {
            code
        } else {
            "failed"
        }
        .into(),
    );
    let sent = stream.send(&frame).and_then(|()| {
        stream.send(&result_frame(
            link,
            sid,
            &transfer.tx,
            "complete",
            Err(code),
        ))
    });
    cancelled.insert(transfer.tx.clone());
    finish(ctx, &transfer, Err(code));
    sent
}

fn preference_frame(link: &Link, sid: &str, record: &ntpods_ipc::CompanionPreference) -> Frame {
    let mut frame = Frame::new("pref", ZERO_TX, link.device, Some(sid));
    frame.key = Some(record.key.clone());
    frame.rev = Some(record.rev.clone());
    frame.origin = Some(record.origin.clone());
    match record.key.as_str() {
        "conversation_policy" => {
            let policy: ntpods_ipc::ConversationPolicy =
                serde_json::from_value(record.value.clone()).unwrap();
            frame.mode = Some(policy.mode);
            frame.first = Some(policy.first);
            frame.second = Some(policy.second);
            frame.pause_media = Some(policy.pause_media);
        }
        "desired_hold_actions" => {
            frame.left = record
                .value
                .get("left")
                .and_then(|v| v.as_str())
                .map(str::to_owned);
            frame.right = record
                .value
                .get("right")
                .and_then(|v| v.as_str())
                .map(str::to_owned);
        }
        _ => frame.value = Some(record.value.clone()),
    }
    frame
}

fn send_preference(
    stream: &Framed,
    ctx: &Ctx,
    link: &Link,
    sid: &str,
    record: &ntpods_ipc::CompanionPreference,
    authorization: u64,
) -> io::Result<()> {
    let _reservation = reserve_authorized(&ctx.companion_actions, &ctx.companion, authorization)?;
    if !selected_pair(ctx, link.device) {
        return Err(invalid());
    }
    stream.send(&preference_frame(link, sid, record))
}

fn receive_preference(
    stream: &Framed,
    ctx: &Ctx,
    link: &Link,
    sid: &str,
    frame: &Frame,
    offered: &mut HashSet<(String, String, String)>,
    authorization: u64,
) -> io::Result<()> {
    let _actions = reserve_authorized(&ctx.companion_actions, &ctx.companion, authorization)?;
    if !selected_pair(ctx, link.device) {
        return Err(invalid());
    }
    let record = frame.preference()?;
    let supported = crate::companion_prefs::supported(ctx, &record.key, &record.value);
    let merged = ctx.companion_preferences.lock().unwrap().receive(
        link.device,
        record.clone(),
        supported,
    )?;
    let code = match merged {
        crate::companion_prefs::Merge::Applied => {
            crate::companion_prefs::apply(ctx, &record.key, &record.value);
            if matches!(
                record.key.as_str(),
                "call_takeover_enabled" | "call_return_enabled"
            ) {
                microphone_started(ctx);
            }
            "applied"
        }
        crate::companion_prefs::Merge::Unchanged => "unchanged",
        crate::companion_prefs::Merge::Stale(winner) => {
            if crate::companion_prefs::supported(ctx, &winner.key, &winner.value)
                && offered.insert((
                    record.key.clone(),
                    record.rev.clone(),
                    record.origin.clone(),
                ))
            {
                stream.send(&preference_frame(link, sid, &winner))?;
            }
            "stale"
        }
        crate::companion_prefs::Merge::Unsupported => "unsupported",
        crate::companion_prefs::Merge::Failed => "failed",
    };
    let mut ack = Frame::new("pref_ack", ZERO_TX, link.device, Some(sid));
    ack.key = Some(record.key);
    ack.rev = Some(record.rev);
    ack.origin = Some(record.origin);
    ack.code = Some(code.into());
    stream.send(&ack)?;
    crate::companion_prefs::publish(ctx);
    Ok(())
}

type MediaRoute = (
    crate::dongle_routing::Ticket,
    crate::dongle_routing::Status,
    Instant,
);
pub(crate) fn retain_dongle_output(
    ctx: &Ctx,
    status: &crate::dongle_routing::Status,
    current: std::sync::Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<(), &'static str> {
    presence::arm(ctx, status, current)
}
fn local_media(
    ctx: &Ctx,
    link: &Link,
    observer: &mut media_policy::Observer,
    route: Option<&MediaRoute>,
    transitioning: bool,
) -> media_policy::Sample {
    let selected = selected_pair(ctx, link.device);
    let evidence = if selected || transitioning {
        let address = (!transitioning && crate::dongle_routing::selected(ctx, link.device).is_none())
            .then_some(link.device);
        crate::media::companion_evidence(address)
    } else {
        crate::media::CompanionEvidence { intent: Err(()), qualified: false }
    };
    let fresh = route
        .filter(|(ticket, _, at)| ticket.current(ctx) && at.elapsed() <= Duration::from_secs(1));
    let (route_known, connected) = if transitioning {
        (true, true)
    } else {
        match crate::dongle_routing::selected(ctx, link.device) {
            Some(Ok(_)) => fresh.map_or((false, false), |(_, status, _)| (true, status.connected)),
            Some(Err(_)) => (false, false),
            None => {
                let connected = selected_ready(ctx, link.device);
                (connected, connected)
            }
        }
    };
    let dongle = crate::dongle_routing::selected(ctx, link.device).is_some();
    let stable_output = !dongle || presence::selected_output(ctx);
    let continuing = transitioning && (!dongle || !presence::supported(ctx));
    let mut sample = observer.observe_qualified(
        evidence.intent,
        connected,
        selected && !busy(ctx, link.device) && route_known && stable_output,
        evidence.qualified,
        continuing,
    );
    sample.known = stable_output && route_known && (!connected || sample.known);
    sample
}
fn media_sample(frame: &Frame) -> io::Result<media_policy::Sample> {
    let number = |value: &Option<String>| {
        u64::from_str_radix(value.as_deref().ok_or_else(invalid)?, 16).map_err(|_| invalid())
    };
    Ok(media_policy::Sample {
        generation: number(&frame.generation)?,
        event: number(&frame.event)?,
        known: frame.known.ok_or_else(invalid)?,
        playing: frame.playing.ok_or_else(invalid)?,
        connected: frame.connected.ok_or_else(invalid)?,
        available: frame.available.ok_or_else(invalid)?,
    })
}

fn session(
    stream: &mut Framed,
    ctx: &Ctx,
    link: &Link,
    authorization: u64,
    sid: &str,
    caps: u8,
) -> io::Result<()> {
    let mut active: Option<Transfer> = None;
    let mut calls = calls::Calls::default();
    let mut seen = HashSet::new();
    let mut cancelled = HashSet::new();
    let mut offered = HashSet::new();
    let mut last_peer = Instant::now();
    let mut next_ping = Instant::now() + Duration::from_secs(10);
    let mut next_busy = Instant::now();
    let mut next_origin = Instant::now();
    let mut media_observer = media_policy::Observer::default();
    let mut media_policy = media_policy::Policy::default();
    let trace_flag = std::env::var("NTPODS_COMPANION_MEDIA_TRACE").ok();
    let mut media_trace = media_policy::Trace::new(trace_flag.as_deref());
    let mut trace_route = media_policy::TraceRoute::Waiting;
    let mut trace_choice = None;
    let mut media_epoch = ctx.companion.lock().unwrap().generation;
    let mut media_route: Option<MediaRoute> = None;
    let mut route_probe: Option<(crate::dongle_routing::Ticket, crate::dongle_routing::Job)> = None;
    let mut local = local_media(ctx, link, &mut media_observer, None, false);
    let mut phone_media: Option<(media_policy::Sample, Instant)> = None;
    let mut media_probe: Option<(String, Instant)> = None;
    let mut next_media = Instant::now();
    {
        let mut store = ctx.companion_preferences.lock().unwrap();
        store.peer_prefs = caps & 1 != 0;
        store.peer_calls = caps & 2 != 0;
    }
    crate::companion_prefs::publish(ctx);
    if caps & 1 != 0 {
        let records = {
            let mut store = ctx.companion_preferences.lock().unwrap();
            store.take_dirty(link.device);
            store.records(link.device)
        };
        for record in records {
            if crate::companion_prefs::supported(ctx, &record.key, &record.value) {
                send_preference(stream, ctx, link, sid, &record, authorization)?;
            }
        }
    }
    stream.send(&status_frame(ctx, link, sid))?;
    ctx.companion.lock().unwrap().presentation_peer.activity(Instant::now());
    let result = (|| loop {
        if ctx.shutting_down.load(Ordering::Acquire)
            || (!selected_pair(ctx, link.device)
                && !active
                    .as_ref()
                    .is_some_and(|transfer| transfer.transitioning(ctx))
                && !calls.waiting_media_release(ctx, link))
            || ctx.companion.lock().unwrap().authorization != authorization
            || !bonded(link.peer)
        {
            return Err(invalid());
        }
        let now = Instant::now();
        ctx.companion.lock().unwrap().presentation_peer.transition(
            (active.is_some() || calls.has_lease()).then_some(now + Duration::from_secs(1)));
        if now.duration_since(last_peer) >= PEER_LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Companion peer stopped responding",
            ));
        }
        if now >= next_ping {
            stream.send(&Frame::new("ping", ZERO_TX, link.device, Some(sid)))?;
            next_ping = now + Duration::from_secs(10);
        }
        if caps & 1 != 0 {
            let records = ctx
                .companion_preferences
                .lock()
                .unwrap()
                .take_dirty(link.device);
            for record in records {
                if crate::companion_prefs::supported(ctx, &record.key, &record.value) {
                    send_preference(stream, ctx, link, sid, &record, authorization)?;
                }
            }
        }
        if caps & 2 != 0 && now >= next_origin {
            calls.media_origin(ctx, link, active.as_ref());
            calls.origin(
                stream,
                ctx,
                link,
                sid,
                active
                    .as_ref()
                    .is_none_or(|transfer| transfer.media_gen.is_some()),
                authorization,
                media_route.as_ref(),
            )?;
            next_origin = now + Duration::from_millis(500);
        }
        let epoch = ctx.companion.lock().unwrap().generation;
        if epoch != media_epoch {
            media_epoch = epoch;
            media_policy = media_policy::Policy::default();
            trace_choice = None;
        }
        if active.is_some() || calls.has_lease() {
            media_policy.suspend();
        }
        if let Some((ticket, job)) = route_probe.as_ref() {
            if let Some(result) = job.poll() {
                trace_route = media_policy::TraceRoute::result(result.as_ref().err().copied());
                media_route = result
                    .ok()
                    .map(|status| (ticket.clone(), status, status.received));
                route_probe = None;
            }
        }
        if caps & 4 != 0 && now >= next_media {
            if route_probe.is_none() {
                if let Some(Ok(ticket)) = crate::dongle_routing::selected(ctx, link.device) {
                    route_probe = Some((
                        ticket.clone(),
                        crate::dongle_routing::probe(ctx.clone(), ticket),
                    ));
                }
            }
            local = local_media(
                ctx,
                link,
                &mut media_observer,
                media_route.as_ref(),
                active
                    .as_ref()
                    .is_some_and(|transfer| transfer.transitioning(ctx)),
            );
            let probe = hex(&random::<8>()?);
            if probe != ZERO_TX
                && media_probe
                    .as_ref()
                    .is_none_or(|(_, sent)| sent.elapsed() >= Duration::from_secs(1))
            {
                let mut frame = Frame::new("media_query", ZERO_TX, link.device, Some(sid));
                frame.probe = Some(probe.clone());
                stream.send(&frame)?;
                media_probe = Some((probe, Instant::now()));
            }
            next_media = now + Duration::from_millis(500);
        }
        if let Some(transfer) = active
            .as_ref()
            .filter(|transfer| transfer.media_gen.is_some())
        {
            let allowed = transfer
                .dongle
                .as_ref()
                .is_none_or(|route| route.observed_current(media_route.as_ref()))
                && phone_media.as_ref().is_some_and(|(phone, at)| {
                    at.elapsed() <= Duration::from_secs(1)
                        && transfer.media_source_gen.is_some_and(|source| {
                            media_policy::current(
                                if transfer.target == CompanionTarget::Android {
                                    media_policy::Target::Phone
                                } else {
                                    media_policy::Target::Pc
                                },
                                transfer.media_gen.unwrap(),
                                source,
                                local,
                                *phone,
                            )
                        })
                });
            if let Some(fence) = &transfer.media_fence {
                *fence.lock().unwrap() = (allowed, Instant::now());
            }
        }
        if let Some(line) = media_trace.record(now, || {
            let route_fresh = media_route.as_ref().is_some_and(|(ticket, _, at)| ticket.current(ctx) && at.elapsed() <= Duration::from_secs(1));
            let route = match crate::dongle_routing::selected(ctx, link.device) {
                None => media_policy::TraceRoute::Native,
                Some(Err(_)) => media_policy::TraceRoute::Unavailable,
                Some(Ok(_)) => trace_route,
            };
            media_policy::TraceRecord { capable: caps & 4 != 0, pc: local, phone: phone_media.map(|(sample, _)| sample),
                phone_fresh: phone_media.is_some_and(|(_, at)| at.elapsed() <= Duration::from_secs(1)), route,
                route_present: media_route.is_some(), route_fresh, probe_pending: route_probe.is_some(),
                busy: busy(ctx, link.device), call: calls.has_lease(), active: active.is_some(), choice: trace_choice }
        }) { crate::log(&line); }
        let mut requested = ctx
            .companion
            .lock()
            .unwrap()
            .queued
            .take()
            .map(|(target, id)| (None, target, Some(id), None));
        if requested.is_some() {
            media_policy.suspend();
        }
        if let Some(frame) = stream.poll(link.device, Some(sid))? {
            last_peer = Instant::now();
            ctx.companion.lock().unwrap().presentation_peer.activity(last_peer);
            match frame.kind.as_str() {
                "ping" => stream.send(&status_frame(ctx, link, sid))?,
                "status" => (),
                "media_state" if caps & 4 != 0 => {
                    if media_probe.as_ref().is_some_and(|(probe, sent)| {
                        frame.probe.as_ref() == Some(probe)
                            && sent.elapsed() <= Duration::from_secs(1)
                    }) {
                        let phone = media_sample(&frame)?;
                        if phone.known {
                            ctx.companion.lock().unwrap().presentation_peer.output(phone.connected, Instant::now());
                        }
                        phone_media = Some((phone, Instant::now()));
                        media_probe = None;
                        local = local_media(
                            ctx,
                            link,
                            &mut media_observer,
                            media_route.as_ref(),
                            active
                                .as_ref()
                                .is_some_and(|transfer| transfer.transitioning(ctx)),
                        );
                        if active.is_none() && requested.is_none() && !calls.has_lease() {
                            let choice = media_policy.observe(local, phone, Instant::now());
                            if choice.is_some() { trace_choice = choice; }
                            if let Some(target) = choice {
                                requested = Some((
                                    None,
                                    match target {
                                        media_policy::Target::Pc => CompanionTarget::Pc,
                                        media_policy::Target::Phone => CompanionTarget::Android,
                                    },
                                    None,
                                    Some(match target {
                                        media_policy::Target::Pc => local.generation,
                                        media_policy::Target::Phone => phone.generation,
                                    }),
                                ));
                            }
                        } else {
                            media_policy.suspend();
                        }
                    }
                }
                "pref" if caps & 1 != 0 => {
                    if offered.len() >= 512 {
                        return Err(invalid());
                    }
                    receive_preference(
                        stream,
                        ctx,
                        link,
                        sid,
                        &frame,
                        &mut offered,
                        authorization,
                    )?;
                }
                "pref_ack" if caps & 1 != 0 => (),
                "call_begin" if caps & 2 != 0 => {
                    if seen.len() >= 128 || !seen.insert(frame.tx.clone()) {
                        return Err(invalid());
                    }
                    // A distinct observed call forfeits the earlier call's return claim.
                    if calls.has_lease() {
                        calls.revoke();
                    }
                    let replacing_media = active
                        .as_ref()
                        .is_some_and(|transfer| transfer.media_gen.is_some());
                    let outcome = if active.is_some() && !replacing_media || requested.is_some() {
                        Err("busy")
                    } else {
                        match calls.begin_from_media_release(
                            ctx,
                            link,
                            &frame,
                            authorization,
                            active
                                .as_ref()
                                .filter(|transfer| transfer.media_gen.is_some()),
                        ) {
                            Err("unavailable") => calls.begin(ctx, link, &frame, authorization),
                            result => result,
                        }
                    };
                    match outcome {
                        Ok(mut transfer) => {
                            if replacing_media {
                                cancel_transfer(
                                    stream,
                                    ctx,
                                    link,
                                    sid,
                                    active.take().unwrap(),
                                    "superseded",
                                    &mut cancelled,
                                    &mut calls,
                                )?;
                                publish_phase(ctx, transfer.phase);
                            }
                            if transfer
                                .call
                                .as_ref()
                                .is_some_and(|call| call.source_released)
                            {
                                if let Err(code) = send_phase(
                                    stream,
                                    ctx,
                                    link,
                                    sid,
                                    &mut transfer,
                                    "prepare",
                                    Phase::Prepare,
                                ) {
                                    finish(ctx, &transfer, Err(code));
                                    calls.revoke();
                                    stream.send(&result_frame(
                                        link,
                                        sid,
                                        &transfer.tx,
                                        "complete",
                                        Err(code),
                                    ))?;
                                    continue;
                                }
                            }
                            active = Some(transfer);
                        }
                        Err(code) => stream.send(&result_frame(
                            link,
                            sid,
                            &frame.tx,
                            "complete",
                            Err(code),
                        ))?,
                    }
                }
                "call_state" if caps & 2 != 0 => {
                    let bounded_takeover = active.as_ref().is_some_and(|transfer| {
                        transfer.call.as_ref().is_some_and(|call| {
                            !call.returning
                                && frame.episode.as_ref() == Some(&call.episode)
                                && frame.call_gen.as_ref() == Some(&call.call_gen)
                        }) && transfer.current(ctx, link)
                            && !transfer.expired(Instant::now())
                    });
                    calls.state(&frame, bounded_takeover);
                }
                "call_end" if caps & 2 != 0 => {
                    if let Some(transfer) = active.as_ref().filter(|t| t.call.is_some()) {
                        calls.sync(ctx, link, transfer);
                        if transfer.phase == Phase::PcRelease
                            && transfer.current(ctx, link)
                            && released_pc(ctx, link, transfer) == Ok(true)
                        {
                            calls.released(transfer);
                        }
                    }
                    let outcome = calls.end(ctx, link, &frame);
                    if active.as_ref().is_some_and(|t| t.call.is_some()) {
                        cancel_transfer(
                            stream,
                            ctx,
                            link,
                            sid,
                            active.take().unwrap(),
                            "superseded",
                            &mut cancelled,
                            &mut calls,
                        )?;
                    }
                    match outcome {
                        Ok(calls::EndPlan::ReturnToPc) => {
                            if seen.len() >= 128 {
                                return Err(invalid());
                            }
                            let tx = fresh_transaction(&mut seen)?;
                            match begin_transfer(
                                ctx,
                                link,
                                tx.clone(),
                                CompanionTarget::Pc,
                                None,
                                authorization,
                            )
                            .and_then(|mut transfer| {
                                calls.returning(&mut transfer)?;
                                Ok(transfer)
                            }) {
                                Ok(mut transfer) => {
                                    if let Err(code) = send_phase(
                                        stream,
                                        ctx,
                                        link,
                                        sid,
                                        &mut transfer,
                                        "prepare",
                                        Phase::Prepare,
                                    ) {
                                        cancel_transfer(
                                            stream,
                                            ctx,
                                            link,
                                            sid,
                                            transfer,
                                            code,
                                            &mut cancelled,
                                            &mut calls,
                                        )?;
                                        calls.revoke();
                                    } else {
                                        active = Some(transfer);
                                    }
                                }
                                Err(code) => {
                                    stream.send(&result_frame(
                                        link,
                                        sid,
                                        &tx,
                                        "complete",
                                        Err(code),
                                    ))?;
                                    calls.revoke();
                                }
                            }
                        }
                        Ok(
                            calls::EndPlan::Finish
                            | calls::EndPlan::WaitForPause
                            | calls::EndPlan::ResumeCurrent,
                        ) => (),
                        Err(_) => calls.revoke(),
                    }
                }
                "switch" => {
                    if seen.len() >= 128 || !seen.insert(frame.tx.clone()) {
                        return Err(invalid());
                    }
                    calls.revoke();
                    if active.is_some() || requested.is_some() {
                        stream.send(&result_frame(
                            link,
                            sid,
                            &frame.tx,
                            "complete",
                            Err("busy"),
                        ))?;
                    } else {
                        let _actions = reserve_authorized(
                            &ctx.companion_actions,
                            &ctx.companion,
                            authorization,
                        )?;
                        microphone_started(ctx);
                        calls.revoke();
                        requested = Some((
                            Some(frame.tx),
                            frame.target.ok_or_else(invalid)?,
                            None,
                            None,
                        ));
                    }
                }
                "cancel" => {
                    if cancelled.contains(&frame.tx) {
                        calls.peer_cancelled_media(ctx, link, None, &frame.tx);
                        continue;
                    }
                    let Some(transfer) = active.take() else {
                        return Err(invalid());
                    };
                    if transfer.tx != frame.tx {
                        return Err(invalid());
                    }
                    calls.peer_cancelled_media(ctx, link, Some(&transfer), &frame.tx);
                    cancelled.insert(transfer.tx.clone());
                    let code = match frame.reason.as_deref() {
                        Some("timeout") => "timeout",
                        Some("user" | "superseded") => "superseded",
                        _ => "failed",
                    };
                    stream.send(&result_frame(
                        link,
                        sid,
                        &transfer.tx,
                        "complete",
                        Err(code),
                    ))?;
                    finish(ctx, &transfer, Err(code));
                    if transfer.media_gen.is_none() {
                        calls.revoke();
                    }
                }
                "result" => {
                    crate::log(&format!(
                        "companion: result phase={} ok={} code={}",
                        frame.phase.as_deref().unwrap_or("invalid"),
                        frame.ok == Some(true),
                        frame.code.as_deref().unwrap_or("invalid")
                    ));
                    if cancelled.contains(&frame.tx) {
                        continue;
                    }
                    let Some(transfer) = active.as_mut() else {
                        return Err(invalid());
                    };
                    if !transfer.accepts_result(&frame) {
                        return Err(invalid());
                    }
                    if !transfer.current(ctx, link)
                        || transfer.expired(Instant::now())
                        || transfer
                            .dongle
                            .as_ref()
                            .is_some_and(|route| !route.observed_current(media_route.as_ref()))
                    {
                        cancel_transfer(
                            stream,
                            ctx,
                            link,
                            sid,
                            active.take().unwrap(),
                            "superseded",
                            &mut cancelled,
                            &mut calls,
                        )?;
                        calls.revoke_preserving_media(ctx, link);
                        continue;
                    }
                    if frame.ok != Some(true) {
                        let transfer = active.take().unwrap();
                        let code = frame.code.as_deref().ok_or_else(invalid)?;
                        stream.send(&result_frame(
                            link,
                            sid,
                            &transfer.tx,
                            "complete",
                            Err(code),
                        ))?;
                        calls.retain_media_release(ctx, link, &transfer);
                        cancelled.insert(transfer.tx.clone());
                        finish(ctx, &transfer, Err(code));
                        // A naturally ended call may still recover a proven released source.
                        if transfer.call.as_ref().is_none_or(|call| call.returning) {
                            calls.revoke_preserving_media(ctx, link);
                        }
                        continue;
                    }
                    let advance = match transfer.phase {
                        Phase::Prepare if transfer.target == CompanionTarget::Android => {
                            if transfer
                                .call
                                .as_ref()
                                .is_some_and(|call| call.source_released)
                            {
                                send_phase(
                                    stream,
                                    ctx,
                                    link,
                                    sid,
                                    transfer,
                                    "acquire",
                                    Phase::PhoneAcquire,
                                )
                            } else {
                                release_pc(ctx, link, transfer)
                            }
                        }
                        Phase::Prepare => send_phase(
                            stream,
                            ctx,
                            link,
                            sid,
                            transfer,
                            "release",
                            Phase::PhoneRelease,
                        ),
                        Phase::PhoneRelease => acquire_pc(ctx, link, transfer),
                        Phase::PhoneAcquire => {
                            ctx.companion.lock().unwrap().presentation_peer.output(true, Instant::now());
                            let transfer = active.take().unwrap();
                            if let Err(code) = complete_success(stream, ctx, link, sid, &transfer) {
                                cancel_transfer(
                                    stream,
                                    ctx,
                                    link,
                                    sid,
                                    transfer,
                                    code,
                                    &mut cancelled,
                                    &mut calls,
                                )?;
                                calls.revoke_preserving_media(ctx, link);
                            }
                            continue;
                        }
                        _ => return Err(invalid()),
                    };
                    if let Some(transfer) = active.as_ref() {
                        calls.sync(ctx, link, transfer);
                    }
                    if let Err(code) = advance {
                        cancel_transfer(
                            stream,
                            ctx,
                            link,
                            sid,
                            active.take().unwrap(),
                            code,
                            &mut cancelled,
                            &mut calls,
                        )?;
                        calls.revoke_preserving_media(ctx, link);
                    }
                }
                _ => return Err(invalid()),
            }
        }
        if let Some((tx, target, request_id, media_gen)) = requested {
            if media_gen.is_none() {
                calls.revoke();
            }
            if active.is_some() {
                notify_local(ctx, request_id, Err("busy"));
            } else {
                let tx = if let Some(tx) = tx {
                    tx
                } else {
                    if seen.len() >= 128 {
                        notify_local(ctx, request_id, Err("unavailable"));
                        return Err(invalid());
                    }
                    fresh_transaction(&mut seen)?
                };
                match begin_transfer(ctx, link, tx.clone(), target, request_id, authorization) {
                    Ok(mut transfer) => {
                        transfer.media_gen = media_gen;
                        transfer.media_source_gen = media_gen.and_then(|_| {
                            if target == CompanionTarget::Android {
                                Some(local.generation)
                            } else {
                                phone_media.as_ref().map(|(phone, _)| phone.generation)
                            }
                        });
                        if media_gen.is_some() {
                            if let (Some(route), Some((_, status, at))) =
                                (transfer.dongle.as_mut(), media_route.as_ref())
                            {
                                if at.elapsed() <= Duration::from_secs(1) {
                                    route.revision = status.revision;
                                }
                            }
                        }
                        if media_gen.is_some() {
                            transfer.media_fence =
                                Some(std::sync::Arc::new(Mutex::new((true, Instant::now()))));
                        }
                        if let Err(code) = send_phase(
                            stream,
                            ctx,
                            link,
                            sid,
                            &mut transfer,
                            "prepare",
                            Phase::Prepare,
                        ) {
                            stream.send(&result_frame(link, sid, &tx, "complete", Err(code)))?;
                            finish(ctx, &transfer, Err(code));
                        } else {
                            active = Some(transfer);
                        }
                    }
                    Err(code) => {
                        stream.send(&result_frame(link, sid, &tx, "complete", Err(code)))?;
                        notify_local(ctx, request_id, Err(code));
                    }
                }
            }
        }
        if active
            .as_ref()
            .is_some_and(|transfer| transfer.phase == Phase::RouteProbe)
        {
            let result = active.as_ref().unwrap().dongle.as_ref().unwrap().poll();
            if let Some(result) = result {
                let outcome = result.and_then(|status| {
                    let transfer = active.as_mut().unwrap();
                    let route = transfer.dongle.as_mut().unwrap();
                    if status.busy {
                        return Err("busy");
                    }
                    if route.revision != 0 && route.revision != status.revision {
                        return Err("superseded");
                    }
                    if transfer.media_gen.is_some()
                        && transfer.target == CompanionTarget::Android
                        && status.connected
                        && crate::media::companion_player(None).is_ok_and(|player| player.is_some())
                    {
                        return Err("busy");
                    }
                    route.revision = status.revision;
                    route.revision_at = status.received;
                    route.preparing = false;
                    *route.job.get_mut().unwrap() = None;
                    *route.result.lock().unwrap() = None;
                    transfer.enter(Phase::Prepare);
                    send_phase(stream, ctx, link, sid, transfer, "prepare", Phase::Prepare)
                });
                if let Err(code) = outcome {
                    cancel_transfer(
                        stream,
                        ctx,
                        link,
                        sid,
                        active.take().unwrap(),
                        code,
                        &mut cancelled,
                        &mut calls,
                    )?;
                }
            }
        }
        if let Some(transfer) = active.as_ref().filter(|transfer| transfer.call.is_some()) {
            if let Some(route) = &transfer.dongle {
                let _ = route.poll();
            }
            calls.sync(ctx, link, transfer);
        }
        let pause_ready = match calls.poll(ctx, link) {
            Ok(ready) => ready,
            Err(_) => false,
        };
        if active.is_none() && calls.needs_local_resume() && calls.resume(ctx, link).is_err() {
            calls.revoke();
        }
        if let Some(transfer) = active.as_ref() {
            let code = if transfer.expired(Instant::now()) {
                Some("timeout")
            } else if !transfer.current(ctx, link)
                || transfer.call.is_some() && !calls.has_lease()
                || transfer
                    .dongle
                    .as_ref()
                    .is_some_and(|route| !route.observed_current(media_route.as_ref()))
            {
                Some("superseded")
            } else if now >= next_busy && !transfer.transitioning(ctx) && busy(ctx, link.device) {
                Some("busy")
            } else {
                None
            };
            if now >= next_busy {
                next_busy = now + Duration::from_millis(500);
            }
            if let Some(code) = code {
                cancel_transfer(
                    stream,
                    ctx,
                    link,
                    sid,
                    active.take().unwrap(),
                    code,
                    &mut cancelled,
                    &mut calls,
                )?;
                calls.revoke_preserving_media(ctx, link);
                continue;
            }
            if transfer.phase == Phase::Pause && pause_ready {
                if let Err(code) = send_phase(
                    stream,
                    ctx,
                    link,
                    sid,
                    active.as_mut().unwrap(),
                    "prepare",
                    Phase::Prepare,
                ) {
                    cancel_transfer(
                        stream,
                        ctx,
                        link,
                        sid,
                        active.take().unwrap(),
                        code,
                        &mut cancelled,
                        &mut calls,
                    )?;
                    calls.revoke_preserving_media(ctx, link);
                }
            } else if transfer.phase == Phase::PcRelease {
                match released_pc(ctx, link, transfer) {
                    Ok(true) => {
                        calls.released(transfer);
                        if let Err(code) = send_phase(
                            stream,
                            ctx,
                            link,
                            sid,
                            active.as_mut().unwrap(),
                            "acquire",
                            Phase::PhoneAcquire,
                        ) {
                            cancel_transfer(
                                stream,
                                ctx,
                                link,
                                sid,
                                active.take().unwrap(),
                                code,
                                &mut cancelled,
                                &mut calls,
                            )?;
                            calls.revoke_preserving_media(ctx, link);
                        }
                    }
                    Ok(false) => (),
                    Err(code) => {
                        cancel_transfer(
                            stream,
                            ctx,
                            link,
                            sid,
                            active.take().unwrap(),
                            code,
                            &mut cancelled,
                            &mut calls,
                        )?;
                        calls.revoke_preserving_media(ctx, link);
                    }
                }
            } else if transfer.phase == Phase::PcAcquire && acquired_pc(ctx, link, transfer) {
                calls.sync(ctx, link, transfer);
                let transfer = active.take().unwrap();
                if let Err(code) = complete_success(stream, ctx, link, sid, &transfer) {
                    cancel_transfer(
                        stream,
                        ctx,
                        link,
                        sid,
                        transfer,
                        code,
                        &mut cancelled,
                        &mut calls,
                    )?;
                    calls.revoke_preserving_media(ctx, link);
                } else if transfer.call.as_ref().is_some_and(|call| call.returning)
                    && calls.resume(ctx, link).is_err()
                {
                    calls.revoke_preserving_media(ctx, link);
                }
            }
        }
        thread::sleep(Duration::from_millis(50));
    })();
    calls.revoke();
    ctx.companion.lock().unwrap().presentation_peer.transition(None);
    if let Some(transfer) = active.take() {
        finish(ctx, &transfer, Err("failed"));
    }
    {
        let mut store = ctx.companion_preferences.lock().unwrap();
        store.peer_prefs = false;
        store.peer_calls = false;
    }
    crate::companion_prefs::publish(ctx);
    result
}

fn fresh_transaction(seen: &mut HashSet<String>) -> io::Result<String> {
    loop {
        let tx = hex(&random::<8>()?);
        if tx != ZERO_TX && seen.insert(tx.clone()) {
            return Ok(tx);
        }
    }
}

pub fn start(ctx: Ctx) {
    pairing::start(ctx.clone());
    thread::spawn(move || {
        crate::volume::init();
        while !ctx.shutting_down.load(Ordering::Acquire) {
            let configured = {
                let state = ctx.companion.lock().unwrap();
                state.link.clone().map(|link| (link, state.authorization))
            };
            let Some((link, authorization)) = configured else {
                thread::sleep(Duration::from_millis(250));
                continue;
            };
            if !selected_pair(&ctx, link.device) {
                thread::sleep(Duration::from_millis(250));
                continue;
            }
            let attempt: io::Result<()> = (|| {
                let server = Listener::open()?;
                loop {
                    if ctx.shutting_down.load(Ordering::Acquire)
                        || !selected_pair(&ctx, link.device)
                        || ctx.companion.lock().unwrap().authorization != authorization
                    {
                        return Ok(());
                    }
                    if let Some(mut stream) = server.accept(link.peer)? {
                        if let Ok((sid, caps)) = authenticate(&mut stream, &link) {
                            {
                                let selected = selected_pair(&ctx, link.device);
                                let mut state = ctx.companion.lock().unwrap();
                                if state.authorization != authorization
                                    || !selected
                                    || !bonded(link.peer)
                                {
                                    continue;
                                }
                                state.connected = true;
                                state.phase = "idle".into();
                                state.error = None;
                            }
                            crate::log("companion: authenticated");
                            publish(&ctx);
                            if let Err(error) =
                                session(&mut stream, &ctx, &link, authorization, &sid, caps)
                            {
                                let reason = match error.kind() {
                                    io::ErrorKind::InvalidData => "protocol_or_authorization",
                                    io::ErrorKind::TimedOut => "timeout",
                                    _ => "transport",
                                };
                                crate::log(&format!("companion: control stopped reason={reason}"));
                            }
                            let mut state = ctx.companion.lock().unwrap();
                            if state.authorization == authorization {
                                if let Some((_, id)) = state.queued.take() {
                                    notify_local(&ctx, Some(id), Err("unavailable"));
                                }
                                state.connected = false;
                                state.phase = "waiting".into();
                            }
                            drop(state);
                            publish(&ctx);
                        }
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            })();
            if attempt.is_err() {
                let mut state = ctx.companion.lock().unwrap();
                if state.authorization == authorization {
                    state.error = Some("The secure companion service is unavailable.".into());
                    state.connected = false;
                }
                drop(state);
                publish(&ctx);
                thread::sleep(Duration::from_secs(2));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    const DEVICE: u64 = 0xaabb_ccdd_eeff;
    const SID: &str = "628870c5318e3d519391f2a559fe37c94a7fec6cef8b028ea367504a86a582c8";

    fn link() -> Link {
        Link {
            peer: 0x1122_3344_5566,
            device: DEVICE,
            secret: [7; 32],
            held_away: false,
        }
    }
    fn transfer(phase: Phase) -> Transfer {
        let now = Instant::now();
        Transfer {
            tx: "123456789abcdef0".into(),
            target: CompanionTarget::Android,
            call: None,
            media_gen: None,
            media_source_gen: None,
            dongle: None,
            media_fence: None,
            authorization: 3,
            request_id: Some(1),
            generation: 9,
            request: 23,
            audio_intent: 13,
            phase,
            phase_deadline: now + PHASE_LIMIT,
            total_deadline: now + TOTAL_LIMIT,
        }
    }
    fn parsed(frame: &Frame) -> Frame {
        Frame::parse(&serde_json::to_vec(frame).unwrap(), DEVICE, Some(SID)).unwrap()
    }

    #[test]
    fn media_prepare_binds_the_destination_generation_without_a_call_lease() {
        let mut transfer = transfer(Phase::Prepare);
        transfer.media_gen = Some(17);
        let frame = parsed(&prepare_frame(&link(), SID, &transfer));
        assert_eq!(frame.purpose.as_deref(), Some("media"));
        assert_eq!(frame.media_gen.as_deref(), Some("0000000000000011"));
        assert_eq!(frame.episode.as_deref(), Some(ZERO_EPISODE));
        assert_eq!(frame.call_gen.as_deref(), Some(ZERO_TX));
        assert_eq!(frame.origin_token.as_deref(), Some(ZERO_EPISODE));
        transfer.media_gen = None;
        assert!(parsed(&prepare_frame(&link(), SID, &transfer))
            .media_gen
            .is_none());
    }

    #[test]
    fn expired_or_revoked_media_proof_cannot_authorize_a_delayed_worker() {
        let mut transfer = transfer(Phase::PcRelease);
        let fence = std::sync::Arc::new(Mutex::new((true, Instant::now())));
        transfer.media_fence = Some(fence.clone());
        assert!(transfer.media_current());
        *fence.lock().unwrap() = (true, Instant::now() - Duration::from_secs(2));
        assert!(!transfer.media_current());
        *fence.lock().unwrap() = (false, Instant::now());
        assert!(!transfer.media_current());
    }

    #[test]
    fn accepted_dongle_reconnect_supersedes_the_older_phone_release() {
        let ctx = Ctx::for_test();
        ctx.mac.store(DEVICE, Ordering::Release);
        {
            let mut state = ctx.companion.lock().unwrap();
            state.generation = 9;
            state.authorization = 3;
        }
        ctx.state.lock().unwrap().dongle = ntpods_ipc::DongleState {
            present: true,
            selected: true,
            session: 7,
            generation: 1,
            capabilities: 7 | 16 | 512,
            serial: Some("test-board".into()),
            pair_token: 1,
            paired_device_id: Some(format!("{DEVICE:012x}")),
            ..Default::default()
        };
        let mut pending = transfer(Phase::PhoneRelease);
        pending.target = CompanionTarget::Pc;
        pending.request = ctx.connect_requested.snapshot();
        pending.audio_intent = bt::playback_intent();
        assert!(pending.current(&ctx, &link()));
        crate::apply_command(
            &ctx,
            Command::DongleControl {
                session: 7,
                generation: 1,
                opcode: 1,
                argument: 0,
            },
        );
        assert!(ctx.state.lock().unwrap().dongle.pending);
        let release = result_frame(&link(), SID, &pending.tx, "release", Ok(()));
        assert!(pending.accepts_result(&release));
        assert!(!pending.current(&ctx, &link()));
        assert_eq!(acquire_pc(&ctx, &link(), &mut pending), Err("superseded"));
        assert_eq!(ctx.connect_requested.snapshot(), pending.request);
        assert_eq!(bt::playback_intent(), pending.audio_intent);
    }

    #[test]
    fn rejected_dongle_routes_and_anc_do_not_cancel_a_companion_transfer() {
        for (opcode, argument, generation, capabilities, pending) in [
            (1, 0, 2, 7, false),
            (1, 1, 1, 7, false),
            (1, 0, 1, 0, false),
            (1, 0, 1, 7, true),
            (3, 2, 1, 7, false),
        ] {
            let ctx = Ctx::for_test();
            ctx.state.lock().unwrap().dongle = ntpods_ipc::DongleState {
                present: true,
                selected: true,
                controls_ready: true,
                session: 7,
                generation: 1,
                capabilities,
                pending,
                ..Default::default()
            };
            let before = ctx.companion.lock().unwrap().generation;
            crate::apply_command(
                &ctx,
                Command::DongleControl {
                    session: 7,
                    generation,
                    opcode,
                    argument,
                },
            );
            assert_eq!(ctx.companion.lock().unwrap().generation, before);
        }
    }

    #[test]
    fn authentication_matches_the_shared_android_vector_and_binds_roles_and_device() {
        let key =
            decode_hex::<32>("000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f")
                .unwrap();
        let client = "202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f";
        let server = "404142434445464748494a4b4c4d4e4f505152535455565758595a5b5c5d5e5f";
        let transcript = transcript(DEVICE, client, server, "03", "03");
        assert_eq!(transcript.len(), 300);
        assert_eq!(hex(&digest(None, &transcript).unwrap()), SID);
        let server_proof =
            digest(Some(&key), &[b"server\n".as_slice(), &transcript].concat()).unwrap();
        let client_proof =
            digest(Some(&key), &[b"client\n".as_slice(), &transcript].concat()).unwrap();
        assert_eq!(
            hex(&server_proof),
            "5b4030aa163ea0085a1618f282393d0ed62817e38012461823cab87699b621ff"
        );
        assert_eq!(
            hex(&client_proof),
            "2a6ab41f49a296aae9cf74fb70c56c9998912c7e0c022b24ce2209f1c347fca4"
        );
        assert!(!equal(&server_proof, &client_proof));
        let mut wrong = client_proof;
        wrong[31] ^= 1;
        assert!(!equal(&client_proof, &wrong));
        assert!(!equal(
            &client_proof,
            &digest(
                Some(&[8; 32]),
                &[b"client\n".as_slice(), &transcript].concat()
            )
            .unwrap()
        ));
        assert_ne!(
            digest(None, &transcript).unwrap(),
            digest(
                None,
                &super::transcript(DEVICE - 1, client, server, "03", "03")
            )
            .unwrap()
        );
    }

    #[test]
    fn framing_preserves_split_and_coalesced_frames_and_refuses_overflow() {
        let first = serde_json::to_vec(&Frame::new("ping", ZERO_TX, DEVICE, Some(SID))).unwrap();
        let second = first.clone();
        for split in 0..=first.len() {
            let mut pending = first[..split].to_vec();
            assert!(take_line(&mut pending).unwrap().is_none());
            pending.extend_from_slice(&first[split..]);
            pending.push(b'\n');
            pending.extend_from_slice(&second);
            pending.push(b'\n');
            assert_eq!(take_line(&mut pending).unwrap().unwrap(), first);
            assert_eq!(take_line(&mut pending).unwrap().unwrap(), second);
            assert!(pending.is_empty());
        }
        assert!(take_line(&mut vec![b'x'; MAX_FRAME]).unwrap().is_none());
        assert!(take_line(&mut vec![b'x'; MAX_FRAME + 1]).is_err());
        let mut pending = vec![b'x'; MAX_FRAME + 1];
        pending.push(b'\n');
        assert!(take_line(&mut pending).is_err());
        assert!(take_line(&mut vec![b'\n']).is_err());
    }

    #[test]
    fn strict_schema_rejects_duplicate_extra_null_wrong_type_and_wrong_session() {
        let valid = serde_json::to_vec(&Frame::new("ping", ZERO_TX, DEVICE, Some(SID))).unwrap();
        assert!(Frame::parse(&valid, DEVICE, Some(SID)).is_ok());
        let text = String::from_utf8(valid).unwrap();
        for value in [
            text.replacen("\"v\":2", "\"v\":2,\"v\":1", 1),
            text.replacen("\"v\":2", "\"v\":2.0", 1),
            text.replacen("\"v\":2", "\"v\":true", 1),
            text.replacen("\"v\":2", "\"v\":2,\"unknown\":false", 1),
            text.replacen("\"v\":2", "\"v\":2,\"nonce\":null", 1),
            text.replace("AA:BB:CC:DD:EE:FF", "aa:bb:cc:dd:ee:ff"),
            text.replace(SID, &"0".repeat(64)),
            format!("\u{feff}{text}"),
            format!("{text}\r"),
            text.replace("0000000000000000", "123456789abcdef0"),
        ] {
            assert!(
                Frame::parse(value.as_bytes(), DEVICE, Some(SID)).is_err(),
                "Accepted invalid schema"
            );
        }
        assert!(Frame::parse(&[0xff], DEVICE, Some(SID)).is_err());
        let mut extra = Frame::new("ping", ZERO_TX, DEVICE, Some(SID));
        extra.target = Some(CompanionTarget::Pc);
        assert!(Frame::parse(&serde_json::to_vec(&extra).unwrap(), DEVICE, Some(SID)).is_err());
        assert!(parse_mac("02:00:00:00:00:00").is_none());
        assert!(parse_mac("FF:FF:FF:FF:FF:FF").is_none());
        assert!(parse_device_id("000000000000").is_none());
    }

    #[test]
    fn results_must_match_the_current_transaction_and_phase() {
        let mut transfer = transfer(Phase::Prepare);
        let ready = parsed(&result_frame(&link(), SID, &transfer.tx, "prepare", Ok(())));
        assert!(transfer.accepts_result(&ready));
        transfer.phase = Phase::PhoneRelease;
        assert!(!transfer.accepts_result(&ready));
        let released = parsed(&result_frame(&link(), SID, &transfer.tx, "release", Ok(())));
        assert!(transfer.accepts_result(&released));
        transfer.phase = Phase::PhoneAcquire;
        assert!(!transfer.accepts_result(&released));
        let acquired = parsed(&result_frame(&link(), SID, &transfer.tx, "acquire", Ok(())));
        assert!(transfer.accepts_result(&acquired));
        let unrelated = parsed(&result_frame(
            &link(),
            SID,
            "123456789abcdef1",
            "acquire",
            Ok(()),
        ));
        assert!(!transfer.accepts_result(&unrelated));
        for phase in [Phase::PcRelease, Phase::PcAcquire] {
            transfer.phase = phase;
            assert!(!transfer.accepts_result(&acquired));
        }
        let mut invalid_success = result_frame(&link(), SID, &transfer.tx, "release", Ok(()));
        invalid_success.code = Some("acquired".into());
        assert!(Frame::parse(
            &serde_json::to_vec(&invalid_success).unwrap(),
            DEVICE,
            Some(SID)
        )
        .is_err());
    }

    #[test]
    fn manual_and_call_prepares_bind_different_receipt_contracts() {
        let mut transfer = transfer(Phase::PhoneAcquire);
        let manual = parsed(&prepare_frame(&link(), SID, &transfer));
        assert_eq!(manual.purpose.as_deref(), Some("manual"));
        assert_eq!(manual.origin_token.as_deref(), Some(ZERO_EPISODE));
        let mut hfp = result_frame(&link(), SID, &transfer.tx, "acquire", Ok(()));
        hfp.code = Some("hfp_ready".into());
        assert!(!transfer.accepts_result(&parsed(&hfp)));
        transfer.call = Some(calls::Context {
            episode: "11".repeat(16),
            call_gen: "0000000000000003".into(),
            origin_token: "22".repeat(16),
            returning: false,
            source_released: false,
        });
        assert_eq!(
            parsed(&prepare_frame(&link(), SID, &transfer))
                .purpose
                .as_deref(),
            Some("call_takeover")
        );
        assert!(transfer.accepts_result(&parsed(&hfp)));
        assert!(!transfer.accepts_result(&parsed(&result_frame(
            &link(),
            SID,
            &transfer.tx,
            "acquire",
            Ok(())
        ))));
        transfer.call.as_mut().unwrap().returning = true;
        transfer.target = CompanionTarget::Pc;
        assert_eq!(
            parsed(&prepare_frame(&link(), SID, &transfer))
                .purpose
                .as_deref(),
            Some("call_return")
        );
        assert!(!transfer.accepts_result(&parsed(&hfp)));
    }

    #[test]
    fn published_phase_distinguishes_remote_release_from_local_acquisition() {
        let mut transfer = transfer(Phase::Prepare);
        let mut state = State::default();
        for (phase, expected) in [
            (Phase::Prepare, "prepare"),
            (Phase::PhoneRelease, "release_android"),
            (Phase::PcAcquire, "acquire_pc"),
            (Phase::PcRelease, "release_pc"),
            (Phase::PhoneAcquire, "acquire_android"),
        ] {
            transfer.enter(phase);
            state.transfer_phase(transfer.phase);
            let event = serde_json::to_value(Event::CompanionStatus {
                status: state.status(),
            })
            .unwrap();
            assert_eq!(event["status"]["phase"], expected);
        }
    }

    #[test]
    fn every_phase_rejects_stale_intent_and_total_deadline_cannot_extend() {
        for phase in [
            Phase::Prepare,
            Phase::PhoneRelease,
            Phase::PcRelease,
            Phase::PhoneAcquire,
            Phase::PcAcquire,
        ] {
            let mut transfer = transfer(phase);
            assert!(transfer.tokens_current(9, 23, 13));
            for tokens in [(10, 23, 13), (9, 27, 13), (9, 23, 15)] {
                assert!(!transfer.tokens_current(tokens.0, tokens.1, tokens.2));
            }
            assert!(!transfer.expired(transfer.phase_deadline - Duration::from_nanos(1)));
            assert!(transfer.expired(transfer.phase_deadline));
            let deadline = Instant::now() + Duration::from_secs(1);
            transfer.total_deadline = deadline;
            transfer.enter(Phase::PcAcquire);
            assert_eq!(transfer.phase_deadline, deadline);
            assert!(transfer.expired(deadline));
        }
    }

    #[test]
    fn processed_local_generation_prevents_a_waiting_old_success_receipt() {
        use std::sync::atomic::{AtomicU64, AtomicUsize};
        use std::sync::{Barrier, Mutex};
        let actions = Mutex::new(());
        let generation = AtomicU64::new(9);
        let successes = AtomicUsize::new(0);
        let waiting = Barrier::new(2);
        let transfer = transfer(Phase::PhoneAcquire);
        thread::scope(|scope| {
            let processed_local_action = actions.lock().unwrap();
            generation.store(10, Ordering::Release);
            let old_receipt = scope.spawn(|| {
                waiting.wait();
                let _reservation = actions.lock().unwrap();
                if transfer.tokens_current(generation.load(Ordering::Acquire), 23, 13) {
                    successes.fetch_add(1, Ordering::Relaxed);
                }
            });
            waiting.wait();
            drop(processed_local_action);
            old_receipt.join().unwrap();
        });
        assert_eq!(successes.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn an_old_authenticated_stream_cannot_reserve_after_forget_or_relink() {
        use std::sync::atomic::AtomicUsize;
        use std::sync::Barrier;
        for relinked in [false, true] {
            let actions = Mutex::new(());
            let state = Mutex::new(State {
                authorization: 7,
                connected: true,
                ..State::default()
            });
            let waiting = Barrier::new(2);
            let writes = AtomicUsize::new(0);
            thread::scope(|scope| {
                let local_choice = actions.lock().unwrap();
                let old_stream = scope.spawn(|| {
                    waiting.wait();
                    if let Ok(_reservation) = reserve_authorized(&actions, &state, 7) {
                        writes.fetch_add(1, Ordering::Relaxed);
                    }
                });
                {
                    let mut current = state.lock().unwrap();
                    current.authorization = 8;
                    current.connected = relinked;
                }
                waiting.wait();
                drop(local_choice);
                old_stream.join().unwrap();
            });
            assert_eq!(writes.load(Ordering::Relaxed), 0);
            assert_eq!(reserve_authorized(&actions, &state, 8).is_ok(), relinked);
        }
    }

    #[test]
    fn release_receipts_require_all_owners_and_ignore_stale_completions() {
        let mut release = Release {
            address: DEVICE,
            native_intent: 13,
            request: Some(24),
            intent: Some(14),
            control: None,
            profiles: None,
            native: None,
            native_serial: Some(3),
        };
        release.control_closed(DEVICE - 1, 24, true);
        release.control_closed(DEVICE, 20, true);
        release.profiles_closed(12, true);
        release.native_closed(DEVICE, 13, 2, true);
        assert_eq!(release.complete(), Ok(false));
        release.control_closed(DEVICE, 24, true);
        release.profiles_closed(14, true);
        assert_eq!(release.complete(), Ok(false));
        release.native_closed(DEVICE, 13, 3, true);
        assert_eq!(release.complete(), Ok(true));
        // Whole-device connection is advisory; only these actual audio-owner
        // observations participate in the release decision.
        assert_eq!(release.ready(false, true, false, false), Ok(true));
        for (driver, quiet, pending, native) in [
            (true, true, false, false),
            (false, false, false, false),
            (false, true, true, false),
            (false, true, false, true),
        ] {
            assert_eq!(release.ready(driver, quiet, pending, native), Ok(false));
        }
        release.control_closed(DEVICE, 24, false);
        release.control_closed(DEVICE, 24, true);
        assert_eq!(release.complete(), Err("failed"));
        assert_eq!(release.ready(false, true, false, false), Err("failed"));
    }

    #[test]
    fn setup_validation_failures_keep_request_correlation_without_exposing_the_secret() {
        for request_id in [0, u64::MAX] {
            for (peer, device, secret) in [
                ("invalid", "001122334455", "ab".repeat(32)),
                ("AA:BB:CC:DD:EE:FF", "invalid", "ab".repeat(32)),
                ("AA:BB:CC:DD:EE:FF", "001122334455", "AB".repeat(32)),
                ("AA:BB:CC:DD:EE:FF", "001122334455", "ab".repeat(32)),
            ] {
                let ctx = Ctx::for_test();
                let (sender, receiver) = std::sync::mpsc::sync_channel(8);
                ctx.clients.lock().unwrap().push(crate::ipc_server::ClientTx::new(sender));
                assert!(apply_local(
                    &ctx,
                    &Command::CompanionLink {
                        peer_address: peer.into(),
                        device_id: device.into(),
                        secret_hex: secret.clone(),
                        request_id,
                    }
                ));
                let event = receiver.try_recv().unwrap().bytes();
                assert!(!std::str::from_utf8(&event).unwrap().contains(&secret));
                assert!(
                    matches!(ntpods_ipc::from_line::<Event>(std::str::from_utf8(&event).unwrap()),
                    Some(Event::CommandResult { command, request_id: actual, ok: false, .. })
                    if command == "companion_link" && actual == request_id)
                );
                assert!(ctx.companion.lock().unwrap().link.is_none());
            }
        }
    }

    #[test]
    fn approved_secret_is_dpapi_protected_and_tampering_is_refused() {
        let mut plain = serde_json::to_vec(&link()).unwrap();
        let mut encrypted = protect(&plain, true).unwrap();
        assert_ne!(encrypted, plain);
        let mut decrypted = protect(&encrypted, false).unwrap();
        let loaded: Link = serde_json::from_slice(&decrypted).unwrap();
        assert_eq!(loaded.secret, [7; 32]);
        encrypted[0] ^= 1;
        assert!(protect(&encrypted, false).is_err());
        wipe(&mut plain);
        wipe(&mut decrypted);
    }
}

#[cfg(test)]
#[path = "companion_v2_tests.rs"]
mod v2_tests;
