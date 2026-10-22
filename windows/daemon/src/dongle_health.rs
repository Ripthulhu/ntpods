//! Hearing profiles and saved diagnostics over USB, isolated from the native session.
use crate::{
    aap, device_models,
    dongle_rpc::{Cached, Session},
    hardware_check, hardware_diagnostics, hearing, hearing_metadata, Ctx,
};
use ntpods_ipc::{Command, DongleHealthState, DongleState, Event, Snapshot};
use std::collections::BTreeMap;
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex,
    },
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct Target {
    session: u64,
    generation: u32,
}
impl Target {
    fn of(d: &DongleState) -> Self {
        Self {
            session: d.session,
            generation: d.generation,
        }
    }
    fn id(self) -> String {
        format!("dongle:{}:{}", self.session, self.generation)
    }
    fn matches(self, d: &DongleState) -> bool {
        self.session != 0 && d.present && !d.ambiguous && self == Self::of(d)
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct FinderTarget {
    session: u64,
    pair: u64,
}
impl FinderTarget {
    fn of(d: &DongleState) -> Self {
        Self {
            session: d.session,
            pair: d.pair_token,
        }
    }
    fn id(self) -> String {
        format!("dongle-finder:{}:{}", self.session, self.pair)
    }
    fn matches(self, d: &DongleState) -> bool {
        self.session != 0 && self.pair != 0 && d.present && !d.ambiguous && self == Self::of(d)
    }
}
struct Job {
    target: Target,
    command: Command,
    epoch: u64,
}
struct Notice {
    target: Target,
    command: Command,
    error: String,
    at: Instant,
}
#[derive(Default)]
pub struct Controller {
    queued: Mutex<Option<Job>>,
    busy: AtomicBool,
    checking: AtomicBool,
    epoch: AtomicU64,
    finder_owner: AtomicU64,
    finder_next: AtomicU64,
    finder_target: Mutex<Option<FinderTarget>>,
    finder_stop: AtomicBool,
    notices: Mutex<[Option<Notice>; 4]>,
}

pub fn busy(ctx: &Ctx) -> bool {
    ctx.dongle_health.busy.load(Ordering::Acquire)
        || ctx.dongle_health.checking.load(Ordering::Acquire)
}

pub fn start_finder(ctx: &Ctx, device_id: &str) -> Option<u64> {
    let d = ctx.state.lock().unwrap().dongle.clone();
    let target = Target::of(&d);
    let finder = FinderTarget::of(&d);
    if !finder.matches(&d) || finder.id() != device_id || !d.selected || d.capabilities & 16 == 0 {
        return None;
    }
    let mut selected = ctx.dongle_health.finder_target.lock().unwrap();
    if selected.is_some() {
        return None;
    }
    let owner = ctx
        .dongle_health
        .finder_next
        .fetch_add(1, Ordering::AcqRel)
        .wrapping_add(1)
        .max(1);
    *selected = Some(finder);
    ctx.dongle_health
        .finder_owner
        .store(owner, Ordering::Release);
    drop(selected);
    if enqueue(
        ctx,
        &Command::DongleHealth {
            session: target.session,
            generation: target.generation,
            command: Box::new(Command::StartNearbyFinder {
                device_id: device_id.into(),
            }),
        },
    ) {
        Some(owner)
    } else {
        stop_finder(ctx, device_id, owner);
        None
    }
}

pub fn stop_finder(ctx: &Ctx, device_id: &str, owner: u64) -> bool {
    let mut selected = ctx.dongle_health.finder_target.lock().unwrap();
    if ctx.dongle_health.finder_owner.load(Ordering::Acquire) != owner
        || !selected.is_some_and(|target| target.id() == device_id)
    {
        return false;
    }
    *selected = None;
    ctx.dongle_health.finder_owner.store(0, Ordering::Release);
    ctx.dongle_health.finder_stop.store(true, Ordering::Release);
    true
}

fn result(ctx: &Ctx, command: &Command, error: Option<String>) {
    let (name, request_id) = match command {
        Command::SetHearingAidMode { request_id, .. } => ("set_hearing_aid_mode", *request_id),
        Command::SetMediaAssist { request_id, .. } => ("set_media_assist", *request_id),
        Command::SetHearingAid { request_id, .. } => ("set_hearing_aid", *request_id),
        Command::StartHardwareCheck { request_id, .. } => ("start_hardware_check", *request_id),
        Command::CancelHardwareCheck { request_id, .. } => ("cancel_hardware_check", *request_id),
        _ => return,
    };
    ctx.send_event(&Event::CommandResult {
        command: name.into(),
        request_id,
        ok: error.is_none(),
        message: error.unwrap_or_default(),
    });
}

fn supported(command: &Command) -> bool {
    is_settings(command)
        || matches!(
            command,
            Command::GetHearingAid
                | Command::SetHearingAid { .. }
                | Command::SetHearingAidMode { .. }
                | Command::SetMediaAssist { .. }
                | Command::GetHardwareDiagnostics
                | Command::GetHearingMetadata { .. }
                | Command::StartHardwareCheck { .. }
                | Command::CancelHardwareCheck { .. }
        )
}
fn setting(command: &Command) -> Option<(u8, u32)> {
    match command {
        Command::SetDeviceControl { id, value, .. } => Some((*id, *value)),
        Command::SetAnc { mode, .. } => Some((0x0d, u32::from(*mode))),
        Command::SetFeature { feature, on } => Some((
            *feature,
            crate::device_settings::toggle_value(*feature, *on),
        )),
        Command::SetControl { id, value } => Some((*id, u32::from(*value))),
        _ => None,
    }
}
fn is_settings(command: &Command) -> bool {
    setting(command).is_some()
        || matches!(
            command,
            Command::RefreshDeviceSettings
                | Command::SetDongleAudioProfile { .. }
                | Command::SetPlaybackMode { .. }
                | Command::SetPlaybackBitrate { .. }
                | Command::SetPlaybackGameMode { .. }
                | Command::GetDonglePerformance
                | Command::StartNearbyFinder { .. }
                | Command::StopNearbyFinder { .. }
        )
}
fn local_request(command: &Command) -> bool {
    matches!(
        command,
        Command::GetDonglePerformance
            | Command::StartNearbyFinder { .. }
            | Command::StopNearbyFinder { .. }
    )
}
fn pending(state: &mut DongleHealthState, command: &Command, on: bool, error: Option<String>) {
    if is_settings(command) {
        state.device_settings_pending = on;
        if error.is_some() {
            state.device_settings_error = error;
        }
        return;
    }
    match command {
        Command::GetHearingMetadata { .. } => {
            state.hearing_metadata_pending = on;
            state.hearing_metadata_error = error;
        }
        Command::GetHardwareDiagnostics | Command::StartHardwareCheck { .. } => {
            state.hardware_diagnostics_pending = on;
            state.hardware_diagnostics_error = error;
        }
        _ => {
            state.hearing_aid_pending = on;
            state.hearing_aid_error = error;
        }
    }
}
fn explicit_target(command: &Command) -> Option<&str> {
    match command {
        Command::SetDeviceControl {
            device_id: Some(device_id),
            ..
        }
        | Command::SetAnc {
            device_id: Some(device_id),
            ..
        }
        | Command::GetHearingMetadata { device_id }
        | Command::StartHardwareCheck { device_id, .. }
        | Command::CancelHardwareCheck { device_id, .. } => Some(device_id),
        _ => None,
    }
}

fn feature(command: &Command) -> usize {
    if is_settings(command) {
        return 3;
    }
    match command {
        Command::GetHardwareDiagnostics | Command::StartHardwareCheck { .. } => 1,
        Command::GetHearingMetadata { .. } => 2,
        _ => 0,
    }
}
fn is_pending(state: &DongleHealthState, command: &Command) -> bool {
    match feature(command) {
        1 => state.hardware_diagnostics_pending,
        2 => state.hearing_metadata_pending,
        3 => state.device_settings_pending,
        _ => state.hearing_aid_pending,
    }
}

pub fn apply(ctx: &Ctx, command: &Command) -> bool {
    if !matches!(command, Command::DongleHealth { .. }) {
        return false;
    }
    // Finder start/stop must pass through the command pipe's per-client lease.
    if matches!(command, Command::DongleHealth { command, .. } if matches!(command.as_ref(), Command::StartNearbyFinder { .. } | Command::StopNearbyFinder { .. }))
    {
        return true;
    }
    enqueue(ctx, command);
    true
}

fn enqueue(ctx: &Ctx, command: &Command) -> bool {
    let Command::DongleHealth {
        session,
        generation,
        command,
    } = command
    else {
        return false;
    };
    let target = Target {
        session: *session,
        generation: *generation,
    };
    // Reserve under the same state lock used by ordinary dongle controls.
    let state_guard = ctx.state.lock().unwrap();
    let d = &state_guard.dongle;
    let error = if !supported(command) {
        Some("This command is not available through the dongle.")
    } else if !target.matches(&d) || explicit_target(command).is_some_and(|id| id != target.id()) {
        Some("Dongle connection changed. Read the settings again.")
    } else {
        None
    };
    if let Some(error) = error {
        drop(state_guard);
        result(ctx, command, Some(error.into()));
        return false;
    }
    if let Command::CancelHardwareCheck { request_id, .. } = command.as_ref() {
        let active = d
            .health
            .hardware_check
            .as_ref()
            .is_some_and(|job| job.request_id == *request_id && job.is_active());
        drop(state_guard);
        if active {
            ctx.dongle_health.epoch.fetch_add(1, Ordering::AcqRel);
            result(ctx, command, None);
        } else {
            result(
                ctx,
                command,
                Some("This hardware check is no longer waiting.".into()),
            );
        }
        return true;
    }
    let local = local_request(command);
    let error = if (!local && (!d.connected || !d.controls_ready))
        || d.capabilities & if local { 16 } else { 8 } == 0
    {
        Some("Connect the dongle's AirPods before reading device data.")
    } else if !local && d.microphone_active {
        Some("Stop recording and wait for microphone recovery before changing device data.")
    } else if d.pending {
        Some("Wait for the current dongle connection or control request to finish.")
    } else if d
        .health
        .hardware_check
        .as_ref()
        .is_some_and(|job| job.is_active())
    {
        Some("Wait for the hardware check to finish, or stop waiting first.")
    } else if ctx
        .dongle_health
        .busy
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        Some("Wait for the current dongle request to finish.")
    } else {
        None
    };
    let observed = Instant::now();
    drop(state_guard);
    if let Some(error) = error {
        result(ctx, command, Some(error.into()));
        ctx.dongle_health.notices.lock().unwrap()[feature(command)] = Some(Notice {
            target,
            command: *command.clone(),
            error: error.into(),
            at: observed,
        });
        let mut state = ctx.state.lock().unwrap();
        // A rejected second request must not clear the first request's spinner.
        if target.matches(&state.dongle) {
            let on = is_pending(&state.dongle.health, command);
            pending(&mut state.dongle.health, command, on, Some(error.into()));
        }
        drop(state);
        ctx.push_state();
        return false;
    }
    let job = Job {
        target,
        command: *command.clone(),
        epoch: ctx.dongle_health.epoch.load(Ordering::Acquire),
    };
    *ctx.dongle_health.queued.lock().unwrap() = Some(job);
    let mut state = ctx.state.lock().unwrap();
    if target.matches(&state.dongle) {
        pending(&mut state.dongle.health, command, true, None);
    }
    drop(state);
    ctx.push_state();
    true
}

struct Guard<'a> {
    ctx: &'a Ctx,
    target: Target,
    epoch: u64,
    deadline: Instant,
    local: bool,
}
impl Guard<'_> {
    fn check(&self) -> Result<(), String> {
        let state = self.ctx.state.lock().unwrap();
        if self.local {
            return if self.ctx.shutting_down.load(Ordering::Acquire)
                || self.ctx.dongle_health.epoch.load(Ordering::Acquire) != self.epoch
                || Instant::now() >= self.deadline
                || !self.target.matches(&state.dongle)
                || state.dongle.pending
            {
                Err("Dongle request is no longer current.".into())
            } else {
                Ok(())
            };
        }
        request_gate(
            &state.dongle,
            self.target,
            self.ctx.shutting_down.load(Ordering::Acquire)
                || self.ctx.dongle_health.epoch.load(Ordering::Acquire) != self.epoch,
            Instant::now() >= self.deadline,
        )
    }
    fn exchange(&self, rpc: &mut Session, request: &[u8]) -> Result<Vec<u8>, String> {
        self.check()?;
        let response = rpc.exchange(request)?;
        self.check()?;
        Ok(response)
    }
    fn send(&self, rpc: &mut Session, packet: &[u8]) -> Result<(), String> {
        self.check()?;
        rpc.send_aacp(packet)?;
        self.check()
    }
}

fn request_gate(
    d: &DongleState,
    target: Target,
    cancelled: bool,
    timed_out: bool,
) -> Result<(), String> {
    if cancelled {
        Err("The dongle request was cancelled.".into())
    } else if timed_out {
        Err("The dongle settings request timed out; it was not retried.".into())
    } else if !target.matches(d) || !d.connected || !d.controls_ready {
        Err("Dongle connection changed. Read the settings again.".into())
    } else if d.microphone_active {
        Err("Stop recording and wait for microphone recovery before changing device data.".into())
    } else if d.pending {
        Err("Wait for the current dongle connection or control request to finish.".into())
    } else {
        Ok(())
    }
}

struct Check {
    coordinator: hardware_check::Coordinator,
    session: u64,
    peer: [u8; 6],
    target: String,
    id: u64,
    epoch: u64,
    started: Instant,
    last_read: Instant,
}
#[derive(Default)]
struct Worker {
    target: Target,
    pair_token: u64,
    health: DongleHealthState,
    modes: hearing::ModeCache,
    mode_revisions: [Option<u32>; 3],
    mode_fences: [Option<u32>; 3],
    ears: Option<(Instant, u32)>,
    ear_actions: crate::dongle_features::Ears,
    paused: Option<(u64, crate::media::SelectedPause)>,
    settings: Snapshot,
    setting_reports: BTreeMap<u8, u32>,
    setting_writes: crate::device_settings::PendingSettings,
    check: Option<Check>,
    finished: [Option<Instant>; 4],
}
impl Worker {
    fn reject(&mut self, notice: Notice) {
        if self.target == notice.target
            && self.finished[feature(&notice.command)].is_none_or(|finished| notice.at > finished)
        {
            let on = is_pending(&self.health, &notice.command);
            pending(&mut self.health, &notice.command, on, Some(notice.error));
        }
    }
    fn select(&mut self, d: &DongleState) {
        let target = Target::of(d);
        if self.target != target || self.pair_token != d.pair_token {
            let nearby_finder =
                if self.target.session == target.session && self.pair_token == d.pair_token {
                    self.health.nearby_finder.clone()
                } else {
                    Default::default()
                };
            let check = self
                .check
                .take()
                .filter(|check| check.session == target.session && d.present);
            *self = Self {
                target,
                pair_token: d.pair_token,
                health: DongleHealthState {
                    nearby_finder,
                    ..Default::default()
                },
                check,
                ..Default::default()
            };
        }
    }
    fn publish(&self, ctx: &Ctx) {
        let queued = ctx
            .dongle_health
            .queued
            .lock()
            .unwrap()
            .as_ref()
            .map(|job| (job.target, job.command.clone()));
        let mut health = self.health.clone();
        if let Some((target, command)) = queued.filter(|(target, _)| *target == self.target) {
            let _ = target;
            pending(&mut health, &command, true, None);
        }
        let mut state = ctx.state.lock().unwrap();
        if !self.target.matches(&state.dongle) || state.dongle.health == health {
            return;
        }
        state.dongle.health = health;
        drop(state);
        ctx.push_state();
    }
    fn sync_modes(&mut self) {
        let mode = self.modes.state();
        if self.health.hearing_aid_enabled != mode.hearing_enabled
            || self.health.hearing_aid_enrolled != mode.hearing_enrolled
        {
            self.health.hearing_aid_revision = self.health.hearing_aid_revision.wrapping_add(1);
        }
        if self.health.media_assist_enabled != mode.media_enabled {
            self.health.media_assist_revision = self.health.media_assist_revision.wrapping_add(1);
        }
        self.health.hearing_aid_enabled = mode.hearing_enabled;
        self.health.hearing_aid_enrolled = mode.hearing_enrolled;
        self.health.media_assist_enabled = mode.media_enabled;
        self.health.media_assist_available =
            mode.pme_voice_enabled.is_some() && mode.pme_media_enabled.is_some();
    }
    fn refresh_modes(&mut self, rpc: &mut Session) -> Result<(), String> {
        for slot in 2..=4 {
            self.observe_mode(slot, rpc.cached(slot)?);
        }
        self.sync_modes();
        Ok(())
    }
    fn observe_mode(&mut self, slot: u8, packet: Option<Cached>) {
        let missing: &[u8] = match slot {
            2 => &[4, 0, 4, 0, 9, 0, 0x2c],
            3 => &[4, 0, 4, 0, 9, 0, 0x33],
            4 => &[4, 0, 4, 0, 0x53, 0],
            _ => return,
        };
        let index = usize::from(slot - 2);
        let fence = &mut self.mode_fences[index];
        if let Some(previous) = *fence {
            if !packet
                .as_ref()
                .is_some_and(|value| value.revision != previous)
            {
                self.modes.invalidate_report(slot);
                return;
            }
            *fence = None;
        }
        self.mode_revisions[index] = packet.as_ref().map(|value| value.revision);
        self.modes.observe(
            packet
                .as_ref()
                .map_or(missing, |value| value.packet.as_slice()),
        );
    }
    fn refresh(&mut self, ctx: &Ctx, rpc: &mut Session) -> Result<(), String> {
        self.health.available = false;
        let metadata = rpc
            .cached(1)?
            .and_then(|packet| aap::parse_metadata(&packet.packet));
        self.observe_metadata(metadata);
        self.refresh_modes(rpc)?;
        self.refresh_ears(rpc)?;
        if rpc.extended() {
            self.refresh_settings(rpc)?;
            self.refresh_audio(rpc)?;
            self.poll_nearby(ctx, rpc);
        }
        self.health.available = true;
        Ok(())
    }
    fn refresh_ears(&mut self, rpc: &mut Session) -> Result<(), String> {
        let before_read = Instant::now();
        let ears = rpc.cached(5)?;
        self.ears = ears.as_ref().map(|value| (before_read, value.age_ms));
        self.health.earbuds_in_case = ears
            .and_then(|value| aap::parse_ear_detection(&value.packet))
            .and_then(|(left, right)| crate::diagnostic_controls::in_case([left, right]));
        Ok(())
    }
    fn observe_metadata(&mut self, metadata: Option<(String, String, String)>) {
        let (model, firmware, _) = metadata.unwrap_or_default();
        if self.health.model != model || self.health.firmware != firmware {
            self.health.hearing_aid_settings = None;
            self.health.hearing_aid_editable = false;
            self.health.hearing_metadata = None;
            self.health.hardware_diagnostics = None;
            self.setting_writes.reset(&mut self.settings);
            self.setting_reports.clear();
            self.sync_settings();
        }
        self.health.model = model;
        self.health.firmware = firmware;
        self.settings.model = self.health.model.clone();
        self.settings.firmware = self.health.firmware.clone();
        self.settings.connected = true;
        self.settings.link_stale = false;
    }
    fn sync_settings(&mut self) {
        self.health.device_controls = self.settings.device_controls.clone();
        self.health.device_controls_requested = self.settings.device_controls_requested.clone();
        self.health.device_controls_pending = self.settings.device_controls_pending.clone();
        self.health.device_settings_error = self.settings.device_settings_error.clone();
        self.health.allow_off_requested = self.settings.allow_off_requested;
        self.health.adaptive_strength_requested = self.settings.adaptive_strength_requested;
        self.health.anc_unconfirmed = self.settings.anc_unconfirmed;
    }
    fn observe_settings(&mut self, records: BTreeMap<u8, crate::dongle_features::Setting>) {
        self.settings
            .device_controls
            .retain(|id, _| records.contains_key(id));
        self.setting_reports
            .retain(|id, _| records.contains_key(id));
        for (id, record) in records {
            if self.setting_reports.get(&id).is_none_or(|old| {
                let advance = record.revision.wrapping_sub(*old);
                advance != 0 && advance < 0x8000_0000
            }) {
                self.setting_reports.insert(id, record.revision);
                self.setting_writes
                    .confirm_control(&mut self.settings, id, record.value);
            }
        }
        self.setting_writes
            .expire(&mut self.settings, Instant::now());
        self.sync_settings();
    }
    fn refresh_settings(&mut self, rpc: &mut Session) -> Result<(), String> {
        let bytes = rpc.feature(&[2])?;
        let records = crate::dongle_features::settings(&bytes)?;
        self.observe_settings(records);
        Ok(())
    }
    fn refresh_audio(&mut self, rpc: &mut Session) -> Result<(), String> {
        self.health.audio = Some(crate::dongle_features::audio(&rpc.feature(&[9])?)?);
        Ok(())
    }
    fn refresh_nearby(&mut self, rpc: &mut Session) -> Result<(), String> {
        let revision = self.health.nearby_finder.revision.wrapping_add(1);
        self.health.nearby_finder = crate::dongle_features::nearby(&rpc.feature(&[7])?, revision)?;
        Ok(())
    }
    fn poll_nearby(&mut self, ctx: &Ctx, rpc: &mut Session) {
        let owned = ctx.dongle_health.finder_owner.load(Ordering::Acquire) != 0
            && ctx
                .dongle_health
                .finder_target
                .lock()
                .unwrap()
                .is_some_and(|target| {
                    target.session == self.target.session && target.pair == self.pair_token
                });
        // Probe identity availability before the first search. Once a view closes,
        // an uncertain Stop must expire rather than being renewed by background polling.
        if owned
            || (!self.health.nearby_finder.available_identity && !self.health.nearby_finder.active)
        {
            let _ = self.refresh_nearby(rpc);
        }
    }
    fn audio_profile(
        &mut self,
        rpc: &mut Session,
        ctx: &Ctx,
        request: &[u8; 7],
    ) -> Result<(), String> {
        let outcome = rpc
            .feature(request)
            .and_then(|bytes| crate::dongle_features::audio(&bytes));
        self.health.audio = Some(match outcome {
            Ok(state) => state,
            Err(original) => {
                // A profile can restart AACP before its receipt is read. Query the same pair;
                // never replay the uncertain write or confuse a saved policy with ready audio.
                let deadline = Instant::now() + Duration::from_secs(2);
                loop {
                    let d = ctx.state.lock().unwrap().dongle.clone();
                    if d.session != self.target.session
                        || d.pair_token != self.pair_token
                        || !d.present
                        || d.ambiguous
                        || ctx.shutting_down.load(Ordering::Acquire)
                    {
                        return Err(original);
                    }
                    if let Ok(mut current) =
                        Session::open_local(ctx.clone(), d.session, d.generation)
                    {
                        if let Ok(state) = current
                            .feature(&[9])
                            .and_then(|bytes| crate::dongle_features::audio(&bytes))
                        {
                            if profile_matches(request, &state) {
                                break state;
                            }
                        }
                    }
                    if Instant::now() >= deadline {
                        return Err(original);
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
        });
        Ok(())
    }
    fn run_feature(
        &mut self,
        rpc: &mut Session,
        guard: &Guard<'_>,
        command: &Command,
    ) -> Result<(), String> {
        guard.check()?;
        if !rpc.extended() {
            return Err("Update the dongle firmware to use this control.".into());
        }
        let profile = |codec, variable, bitrate| {
            crate::dongle_features::audio_request(codec, variable, bitrate)
        };
        let request = match command {
            Command::SetDongleAudioProfile {
                codec,
                variable,
                bitrate,
            } => Some(profile(*codec, *variable, *bitrate)?),
            Command::SetPlaybackMode { mode } => Some(match mode {
                ntpods_ipc::PlaybackMode::Aac => profile(2, false, 220000)?,
                ntpods_ipc::PlaybackMode::AacEld => profile(3, false, 256000)?,
                ntpods_ipc::PlaybackMode::Windows => profile(0, false, 0)?,
            }),
            Command::SetPlaybackBitrate {
                mode,
                best_effort,
                kbps,
                variable,
            } => {
                let codec = match mode {
                    ntpods_ipc::PlaybackMode::Aac => 2,
                    ntpods_ipc::PlaybackMode::AacEld => 3,
                    _ => return Err("Choose AAC or AAC-ELD before changing bitrate.".into()),
                };
                if self
                    .health
                    .audio
                    .as_ref()
                    .is_none_or(|audio| audio.codec != codec)
                {
                    return Err("The active codec changed. Read the audio settings again.".into());
                }
                let bitrate = if *best_effort {
                    if codec == 2 {
                        220000
                    } else {
                        256000
                    }
                } else {
                    kbps.checked_mul(1000).ok_or("Unsupported bitrate.")?
                };
                Some(profile(codec, *variable, bitrate)?)
            }
            _ => None,
        };
        if let Some(request) = request {
            return self.audio_profile(rpc, guard.ctx, &request);
        }
        match command {
            Command::SetPlaybackGameMode { enabled } => {
                self.health.audio = Some(crate::dongle_features::audio(
                    &rpc.feature(&[4, u8::from(*enabled)])?,
                )?);
            }
            Command::GetDonglePerformance => {
                self.health.performance =
                    Some(crate::dongle_features::performance(&rpc.feature(&[5])?)?);
            }
            Command::StartNearbyFinder { .. } => {
                if !guard
                    .ctx
                    .dongle_health
                    .finder_target
                    .lock()
                    .unwrap()
                    .is_some_and(|target| {
                        target.session == self.target.session && target.pair == self.pair_token
                    })
                {
                    return Err("The nearby-finder view has closed.".into());
                }
                let revision = self.health.nearby_finder.revision.wrapping_add(1);
                self.health.nearby_finder =
                    crate::dongle_features::nearby(&rpc.feature(&[6])?, revision)?;
            }
            Command::StopNearbyFinder { .. } => {
                if !rpc.feature(&[8])?.is_empty() {
                    return Err("Unsupported finder response.".into());
                }
                self.refresh_nearby(rpc)?;
            }
            _ => return Err("Unsupported dongle feature.".into()),
        }
        Ok(())
    }
    fn write_setting(
        &mut self,
        rpc: &mut Session,
        guard: &Guard<'_>,
        id: u8,
        value: u32,
    ) -> Result<(), String> {
        guard.check()?;
        if !rpc.extended() || !device_models::control(&self.settings, id, value) {
            return Err("This setting is not available on the dongle's reported AirPods.".into());
        }
        let choice = device_models::unreported_choice(&self.settings, id, value);
        let token = self
            .setting_writes
            .begin_control(&mut self.settings, id, value, choice);
        self.sync_settings();
        self.publish(guard.ctx);
        let request = crate::dongle_features::setting_request(id, value)?;
        let sent = rpc.feature(&request).and_then(|reply| {
            if reply.is_empty() {
                Ok(())
            } else {
                Err("Unsupported setting acknowledgement.".into())
            }
        });
        self.setting_writes
            .finish_control(&mut self.settings, id, value, token, sent.is_ok());
        self.sync_settings();
        sent?;
        self.refresh_settings(rpc)
    }
    fn poll_ears(&mut self, ctx: &Ctx, rpc: &mut Session) -> Result<(), String> {
        let packet = rpc.cached(5)?;
        let action = self.ear_actions.observe(packet.as_ref());
        if action.is_err() {
            self.paused = None;
            return Ok(());
        }
        // A paused PC is USB-idle; phone ownership can change between audio refreshes.
        if self.paused.is_some() {
            self.health.audio = None;
            if !rpc.extended() || self.refresh_audio(rpc).is_err() || !rpc.current() {
                self.paused = None;
                return Ok(());
            }
        }
        let d = ctx.state.lock().unwrap().dongle.clone();
        let native = ctx.state.lock().unwrap().connected;
        let route = !native
            && self.target.matches(&d)
            && d.connected
            && d.controls_ready
            && self
                .health
                .audio
                .as_ref()
                .is_none_or(|audio| audio.input != 2)
            && (d.usb_playback || self.paused.is_some())
            && !d.microphone_active;
        let policy = ctx.auto_pause.lock().unwrap();
        let Some(permission) = policy.permission(route) else {
            self.paused = None;
            return Ok(());
        };
        match action.unwrap() {
            Some(crate::dongle_features::EarAction::Pause)
                if self.paused.is_none() && d.usb_playback =>
            {
                self.paused = Some((permission, crate::media::SelectedPause::start_dongle()));
            }
            Some(crate::dongle_features::EarAction::Resume) => {
                if let Some((recorded, mut paused)) = self.paused.take() {
                    if recorded == permission {
                        let current = ctx.clone();
                        let target = self.target;
                        paused.resume(move || {
                            let owns_audio =
                                Session::open(current.clone(), target.session, target.generation)
                                    .and_then(|mut rpc| {
                                        if !rpc.extended() {
                                            return Err(
                                                "Dongle audio ownership is unavailable.".into()
                                            );
                                        }
                                        let audio =
                                            crate::dongle_features::audio(&rpc.feature(&[9])?)?;
                                        Ok(rpc.current() && audio.input != 2)
                                    })
                                    .unwrap_or(false);
                            let state = current.state.lock().unwrap();
                            let valid = owns_audio
                                && !state.connected
                                && target.matches(&state.dongle)
                                && state.dongle.connected
                                && state.dongle.controls_ready
                                && !state.dongle.pending
                                && !state.dongle.microphone_active
                                && !current.shutting_down.load(Ordering::Acquire);
                            drop(state);
                            current.auto_pause.lock().unwrap().permission(valid) == Some(recorded)
                        });
                        // The ticket must remain alive until its asynchronous resume finishes.
                        self.paused = Some((recorded, paused));
                    }
                }
            }
            _ => (),
        }
        if self.paused.as_ref().is_some_and(|(_, ticket)| {
            matches!(
                ticket.poll(),
                Some(
                    crate::media::PauseReceipt::Quiet
                        | crate::media::PauseReceipt::Failed
                        | crate::media::PauseReceipt::Resumed(_)
                )
            )
        }) {
            self.paused = None;
        }
        Ok(())
    }
    fn check_active(&self) -> bool {
        self.check
            .as_ref()
            .is_some_and(|check| check.coordinator.is_active())
    }
    fn hearing_supported(&self) -> bool {
        device_models::find(&self.health.model).is_some_and(|profile| profile.hearing_aid)
    }
    fn mode(
        &mut self,
        rpc: &mut Session,
        guard: &Guard<'_>,
        mode: hearing::Mode,
        on: bool,
    ) -> Result<(), String> {
        guard.check()?;
        self.refresh_modes(rpc)?;
        let initial = self.modes.state();
        let plan = hearing::mode_plan(&self.modes, mode, on)?;
        self.health.hearing_aid_editable = false;
        let original = if on {
            let (_, value, _) =
                hearing::read_settings(&mut |request| guard.exchange(rpc, request))?;
            if (mode == hearing::Mode::MediaAssist
                || self.modes.state().hearing_enabled != Some(true))
                && crate::transparency::read_enabled(|request| guard.exchange(rpc, request))?
            {
                return Err(
                    "Turn off Custom Transparency before enabling hearing assistance.".into(),
                );
            }
            Some(value)
        } else {
            None
        };
        self.refresh_modes(rpc)?;
        if self.modes.state() != initial {
            return Err(
                "Hearing modes changed before the request started. Read them again.".into(),
            );
        }
        for packet in &plan.packets {
            let slot = match (packet[4], packet[6]) {
                (9, 0x2c) => Some(2),
                (9, 0x33) => Some(3),
                (0x53, _) => Some(4),
                _ => None,
            };
            if let Some(slot) = slot {
                let index = usize::from(slot - 2);
                self.mode_fences[index] = Some(
                    self.mode_revisions[index]
                        .ok_or("Read the current hearing modes before changing them.")?,
                );
                self.modes.invalidate_report(slot);
                self.sync_modes();
            }
            guard.send(rpc, packet)?;
        }
        if !plan.packets.is_empty() {
            // This requests reports; it does not substitute for readback.
            guard.send(rpc, &[4, 0, 4, 0, 0x0f, 0, 0xff, 0xff, 0xff, 0xff])?;
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            guard.check()?;
            self.refresh_modes(rpc)?;
            if hearing::mode_confirmed(&plan, &self.modes) {
                break;
            }
            if Instant::now() >= deadline {
                return Err("The mode request was sent, but the AirPods did not confirm it. Reconnect the dongle's AirPods and read settings again.".into());
            }
            thread::sleep(Duration::from_millis(100));
        }
        if let Some(original) = original {
            let (settings, editable) =
                hearing::verify_profile(&original, |request| guard.exchange(rpc, request))?;
            self.refresh_modes(rpc)?;
            if !hearing::mode_confirmed(&plan, &self.modes) {
                return Err(
                    "Hearing modes changed before the request finished. Read them again.".into(),
                );
            }
            self.health.hearing_aid_settings = Some(settings);
            self.health.hearing_aid_editable = editable;
        }
        Ok(())
    }
    fn edit(
        &mut self,
        rpc: &mut Session,
        guard: &Guard<'_>,
        expected: Option<ntpods_ipc::HearingAidSettings>,
        requested: Option<ntpods_ipc::HearingAidSettings>,
    ) -> Result<(), String> {
        let (Some(expected), Some(requested)) = (expected, requested) else {
            return Err("Read the complete hearing profile before editing it.".into());
        };
        if !expected.is_valid()
            || !requested.is_valid()
            || !self.health.hearing_aid_editable
            || self.health.hearing_aid_settings != Some(expected)
        {
            return Err(
                "Hearing settings changed or are not editable. Read them again before editing."
                    .into(),
            );
        }
        // Read before enabling: changing modes must not replace or rewrite the fitting.
        hearing::prepare_settings(|request| guard.exchange(rpc, request), &expected)?;
        self.refresh_modes(rpc)?;
        let was_enabled = self.modes.state().hearing_enabled;
        if was_enabled.is_none() {
            return Err("Wait for the AirPods to report Hearing Aid before editing.".into());
        }
        let operation = (|| {
            if was_enabled != Some(true) {
                self.mode(rpc, guard, hearing::Mode::HearingAid, true)?;
            }
            let expected_modes = self.modes.state();
            let (characteristic, preserved) =
                hearing::prepare_settings(|request| guard.exchange(rpc, request), &expected)?;
            self.refresh_modes(rpc)?;
            if self.modes.state() != expected_modes
                || self.modes.state().hearing_enabled != Some(true)
            {
                return Err(
                    "Hearing modes changed before the profile write. Read settings again.".into(),
                );
            }
            self.health.hearing_aid_editable = false;
            let confirmed = hearing::write_settings(
                |request| guard.exchange(rpc, request),
                characteristic,
                preserved,
                &requested,
            )?;
            self.refresh_modes(rpc)?;
            if self.modes.state() != expected_modes {
                return Err(
                    "The profile was written, but hearing modes changed. Read settings again."
                        .into(),
                );
            }
            self.health.hearing_aid_settings = Some(confirmed);
            self.health.hearing_aid_editable = true;
            Ok(())
        })();
        if let Err(error) = operation {
            self.health.hearing_aid_editable = false;
            if was_enabled == Some(false) && guard.check().is_ok() {
                return match self.mode(rpc, guard, hearing::Mode::HearingAid, false) {
                    Ok(()) => Err(format!("{error} Hearing Aid is confirmed off again.")),
                    Err(cleanup) => Err(format!(
                        "{error} Hearing Aid may still be active: {cleanup}"
                    )),
                };
            }
            return Err(error);
        }
        Ok(())
    }
    fn run(
        &mut self,
        rpc: &mut Session,
        guard: &Guard<'_>,
        command: &Command,
    ) -> Result<(), String> {
        guard.check()?;
        if local_request(command) {
            return self.run_feature(rpc, guard, command);
        }
        self.refresh(guard.ctx, rpc)?;
        if let Some((id, value)) = setting(command) {
            return self.write_setting(rpc, guard, id, value);
        }
        if matches!(command, Command::RefreshDeviceSettings) {
            return Ok(());
        }
        if is_settings(command) {
            return self.run_feature(rpc, guard, command);
        }
        if !self.hearing_supported() {
            return Err(
                "Hearing assistance and hardware checks are not supported by this reported model."
                    .into(),
            );
        }
        match command {
            Command::GetHearingAid => {
                self.health.hearing_aid_editable = false;
                let (characteristic, value, settings) =
                    hearing::read_settings(&mut |request| guard.exchange(rpc, request))?;
                self.health.hearing_aid_settings = Some(settings);
                self.health.hearing_aid_editable = hearing::editable_value(&value, characteristic);
                Ok(())
            }
            Command::SetHearingAidMode { on, .. } => {
                self.mode(rpc, guard, hearing::Mode::HearingAid, *on)
            }
            Command::SetMediaAssist { on, .. } => {
                self.mode(rpc, guard, hearing::Mode::MediaAssist, *on)
            }
            Command::SetHearingAid { on: false, .. } => {
                self.mode(rpc, guard, hearing::Mode::HearingAid, false)
            }
            Command::SetHearingAid {
                expected, settings, ..
            } => self.edit(rpc, guard, *expected, *settings),
            Command::GetHardwareDiagnostics => {
                self.health.hardware_diagnostics =
                    hardware_diagnostics::transaction(&self.health.model, |request| {
                        guard.exchange(rpc, request)
                    })?;
                self.health
                    .hardware_diagnostics
                    .ok_or("Saved hardware checks are not available on these AirPods.")?;
                Ok(())
            }
            Command::GetHearingMetadata { .. } => {
                self.health.hearing_metadata = Some(hearing_metadata::transaction(
                    || guard.check().is_ok(),
                    |request| guard.exchange(rpc, request),
                )?);
                Ok(())
            }
            Command::StartHardwareCheck {
                request_id,
                buds_in_case_confirmed,
                ..
            } => self.start_check(rpc, guard, *request_id, *buds_in_case_confirmed),
            _ => Err("This command is not available through the dongle.".into()),
        }
    }
    fn safe_check(&self, guard: &Guard<'_>, confirmed: bool) -> Result<(), String> {
        guard.check()?;
        let age = self
            .ears
            .map(|(time, age)| u64::from(age).saturating_add(time.elapsed().as_millis() as u64));
        let d = guard.ctx.state.lock().unwrap().dongle.clone();
        check_gate(&self.health, age, &d, confirmed)
    }
    fn start_check(
        &mut self,
        rpc: &mut Session,
        guard: &Guard<'_>,
        id: u64,
        confirmed: bool,
    ) -> Result<(), String> {
        if id == 0 {
            return Err("The hardware-check request has no identity.".into());
        }
        self.safe_check(guard, confirmed)?;
        let baseline = hardware_diagnostics::transaction(&self.health.model, |request| {
            guard.exchange(rpc, request)
        })?
        .ok_or("Read the saved hardware results before starting a check.")?;
        self.refresh(guard.ctx, rpc)?;
        self.safe_check(guard, confirmed)?;
        let target = guard.target.id();
        let mut coordinator = hardware_check::Coordinator::start(id, target.clone(), baseline, 0)?;
        if !coordinator.claim_trigger(id, &target, 0) {
            return Err("The hardware-check request was already sent.".into());
        }
        self.health.hardware_diagnostics = Some(baseline);
        self.health.hardware_check = Some(coordinator.status());
        self.publish(guard.ctx);
        let outcome = hardware_check::transaction(|request| {
            if request.first() == Some(&0x12) {
                self.refresh_ears(rpc)?;
            }
            self.safe_check(guard, confirmed)?;
            guard.exchange(rpc, request)
        });
        match outcome {
            Ok(()) => {}
            Err(error) => {
                coordinator.fail(id, &target, error.clone(), 0);
                self.health.hardware_check = Some(coordinator.status());
                return Err(error);
            }
        }
        self.check = Some(Check {
            coordinator,
            session: guard.target.session,
            peer: rpc.peer(),
            target,
            id,
            epoch: guard.epoch,
            started: Instant::now(),
            last_read: Instant::now(),
        });
        guard
            .ctx
            .dongle_health
            .checking
            .store(true, Ordering::Release);
        self.health.hardware_check = self.check.as_ref().map(|check| check.coordinator.status());
        Ok(())
    }
    fn poll_check(&mut self, ctx: &Ctx, rpc: Option<&mut Session>) {
        let Some(check) = self.check.as_mut() else {
            return;
        };
        let now = check.started.elapsed().as_millis() as u64;
        if !ctx.state.lock().unwrap().dongle.connected {
            check
                .coordinator
                .waiting_for_result(check.id, &check.target, now);
        }
        if ctx.dongle_health.epoch.load(Ordering::Acquire) != check.epoch {
            check.coordinator.cancel(check.id, &check.target);
        }
        check.coordinator.tick(now);
        if check.coordinator.is_active() && check.last_read.elapsed() >= Duration::from_secs(2) {
            if let Some(rpc) = rpc {
                check.last_read = Instant::now();
                if rpc.peer() != check.peer {
                    check.coordinator.cancel(check.id, &check.target);
                } else {
                    let guard = Guard {
                        ctx,
                        target: self.target,
                        epoch: check.epoch,
                        deadline: Instant::now() + Duration::from_secs(15),
                        local: false,
                    };
                    if guard.check().is_ok() {
                        match hardware_diagnostics::transaction(&self.health.model, |request| {
                            guard.exchange(rpc, request)
                        }) {
                            Ok(Some(record)) => {
                                check
                                    .coordinator
                                    .observe(check.id, &check.target, &record, now);
                                self.health.hardware_diagnostics = Some(record);
                            }
                            Err(error) if rpc.current() => {
                                check.coordinator.fail(check.id, &check.target, error, now)
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        self.health.hardware_check = Some(check.coordinator.status());
    }
}

fn profile_matches(request: &[u8; 7], state: &ntpods_ipc::DongleAudioState) -> bool {
    state.requested_codec == request[1]
        && state.variable == (request[2] != 0)
        && state.requested_bitrate == u32::from_le_bytes(request[3..7].try_into().unwrap())
}

fn check_gate(
    h: &DongleHealthState,
    ear_age: Option<u64>,
    d: &DongleState,
    confirmed: bool,
) -> Result<(), String> {
    if !matches!(h.model.as_str(), "A3063" | "A3064" | "A3065") || h.firmware != "9A348" {
        Err("In-case checks have not been verified on this model and firmware.".into())
    } else if !confirmed
        || h.earbuds_in_case != Some(true)
        || !ear_age.is_some_and(|age| age <= 30_000)
    {
        Err("Keep both AirPods inside the open case and out of your ears before starting.".into())
    } else if d.microphone_active || d.usb_playback || d.streaming || d.phone_connected {
        Err("Pause playback and recording, and disconnect phone audio before starting an in-case check.".into())
    } else {
        Ok(())
    }
}

pub fn start(ctx: Ctx) {
    thread::spawn(move || {
        crate::media::init();
        let mut worker = Worker::default();
        let mut last_refresh = Instant::now() - Duration::from_secs(3);
        let mut last_ears = Instant::now() - Duration::from_secs(1);
        let mut last_local = Instant::now() - Duration::from_secs(3);
        while !ctx.shutting_down.load(Ordering::Acquire) {
            let d = ctx.state.lock().unwrap().dongle.clone();
            worker.select(&d);
            {
                let mut selected = ctx.dongle_health.finder_target.lock().unwrap();
                if selected.is_some_and(|target| !target.matches(&d)) {
                    *selected = None;
                    ctx.dongle_health.finder_owner.store(0, Ordering::Release);
                }
            }
            if ctx.dongle_health.finder_stop.swap(false, Ordering::AcqRel)
                && d.present
                && !d.ambiguous
                && d.capabilities & 16 != 0
            {
                match Session::open_local(ctx.clone(), d.session, d.generation).and_then(
                    |mut rpc| {
                        rpc.feature(&[8])?;
                        worker.refresh_nearby(&mut rpc)
                    },
                ) {
                    Ok(()) => (),
                    Err(_) => ctx.dongle_health.finder_stop.store(true, Ordering::Release),
                }
            }
            if !d.connected
                || !d.controls_ready
                || ctx.state.lock().unwrap().connected
                || d.microphone_active
            {
                worker.paused = None;
                worker.ear_actions = Default::default();
            }
            let notices = std::mem::take(&mut *ctx.dongle_health.notices.lock().unwrap());
            for notice in notices.into_iter().flatten() {
                worker.reject(notice);
            }
            let job = ctx.dongle_health.queued.lock().unwrap().take();
            if let Some(job) = job {
                if worker.target == job.target {
                    pending(&mut worker.health, &job.command, true, None);
                    worker.publish(&ctx);
                    let guard = Guard {
                        ctx: &ctx,
                        target: job.target,
                        epoch: job.epoch,
                        deadline: Instant::now() + Duration::from_secs(60),
                        local: local_request(&job.command),
                    };
                    let outcome = guard
                        .check()
                        .and_then(|_| {
                            if local_request(&job.command) {
                                Session::open_local(
                                    ctx.clone(),
                                    job.target.session,
                                    job.target.generation,
                                )
                            } else {
                                Session::open(
                                    ctx.clone(),
                                    job.target.session,
                                    job.target.generation,
                                )
                            }
                        })
                        .and_then(|mut rpc| worker.run(&mut rpc, &guard, &job.command));
                    let error = outcome.err();
                    worker.finished[feature(&job.command)] = Some(Instant::now());
                    if error.is_some()
                        && matches!(
                            job.command,
                            Command::GetHearingAid
                                | Command::SetHearingAid { .. }
                                | Command::SetHearingAidMode { .. }
                                | Command::SetMediaAssist { .. }
                        )
                    {
                        worker.health.hearing_aid_editable = false;
                    }
                    pending(&mut worker.health, &job.command, false, error.clone());
                    result(&ctx, &job.command, error);
                } else {
                    result(
                        &ctx,
                        &job.command,
                        Some("Dongle connection changed. Read the settings again.".into()),
                    );
                }
                ctx.dongle_health.busy.store(false, Ordering::Release);
                last_refresh = Instant::now();
            } else if last_refresh.elapsed() >= Duration::from_secs(2)
                && d.present
                && !d.ambiguous
                && d.connected
                && !d.pending
                && d.controls_ready
                && d.capabilities & 8 != 0
            {
                last_refresh = Instant::now();
                last_local = Instant::now();
                match Session::open(ctx.clone(), d.session, d.generation) {
                    Ok(mut rpc) => {
                        if worker.refresh(&ctx, &mut rpc).is_err() {
                            worker.health.available = false;
                        }
                        worker.poll_check(&ctx, Some(&mut rpc));
                    }
                    Err(_) => worker.health.available = false,
                }
            }
            if last_local.elapsed() >= Duration::from_secs(2)
                && d.present
                && !d.ambiguous
                && d.capabilities & 16 != 0
                && !d.pending
                && !ctx.dongle_health.busy.load(Ordering::Acquire)
            {
                last_local = Instant::now();
                if let Ok(mut rpc) = Session::open_local(ctx.clone(), d.session, d.generation) {
                    let _ = worker.refresh_audio(&mut rpc);
                    worker.poll_nearby(&ctx, &mut rpc);
                }
            }
            if last_ears.elapsed() >= Duration::from_millis(250)
                && d.present
                && !d.ambiguous
                && d.connected
                && d.controls_ready
                && !d.pending
                && !d.microphone_active
                && !worker.check_active()
                && !ctx.dongle_health.busy.load(Ordering::Acquire)
            {
                last_ears = Instant::now();
                if let Ok(mut rpc) = Session::open(ctx.clone(), d.session, d.generation) {
                    if worker.poll_ears(&ctx, &mut rpc).is_err() {
                        worker.paused = None;
                    }
                } else {
                    worker.paused = None;
                }
            }
            worker.poll_check(&ctx, None);
            ctx.dongle_health
                .checking
                .store(worker.check_active(), Ordering::Release);
            worker.publish(&ctx);
            thread::sleep(Duration::from_millis(100));
        }
        ctx.dongle_health.queued.lock().unwrap().take();
        ctx.dongle_health.busy.store(false, Ordering::Release);
        ctx.dongle_health.checking.store(false, Ordering::Release);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn terminal_hardware_checks_do_not_block_ear_polling_or_lose_their_status() {
        let ear = ntpods_ipc::HardwareDiagnosticEar {
            measured_at_unix_seconds: 1,
            first_use_unix_seconds: None,
            general: 0,
            measurement: 0,
            component_failures: None,
            unknown_bits: 0,
        };
        let baseline = ntpods_ipc::HardwareDiagnostics {
            read_at_unix_seconds: 2,
            left: ear,
            right: ear,
        };
        for phase in ["completed", "cancelled", "error", "timed_out"] {
            let target = Target {
                session: 1,
                generation: 2,
            };
            let mut worker = Worker {
                target,
                check: Some(Check {
                    coordinator: hardware_check::Coordinator::start(8, target.id(), baseline, 0)
                        .unwrap(),
                    session: 1,
                    peer: [1, 2, 3, 4, 5, 6],
                    target: target.id(),
                    id: 8,
                    epoch: 0,
                    started: Instant::now(),
                    last_read: Instant::now(),
                }),
                ..Default::default()
            };
            assert!(worker.check_active());
            let check = worker.check.as_mut().unwrap();
            match phase {
                "completed" => {
                    assert!(check.coordinator.claim_trigger(8, &check.target, 1));
                    let mut record = baseline;
                    record.left.measured_at_unix_seconds += 1;
                    record.right.measured_at_unix_seconds += 1;
                    check.coordinator.observe(8, &check.target, &record, 2);
                }
                "cancelled" => check.coordinator.cancel(8, &check.target),
                "error" => check
                    .coordinator
                    .fail(8, &check.target, "Read failed.".into(), 2),
                "timed_out" => check.coordinator.tick(120_000),
                _ => unreachable!(),
            }
            worker.health.hardware_check = Some(check.coordinator.status());
            assert!(!worker.check_active(), "{phase}");
            assert!(worker.check.is_some());
            assert_eq!(worker.health.hardware_check.as_ref().unwrap().phase, phase);
        }
    }
    #[test]
    fn profile_reconciliation_checks_saved_policy_not_active_transport() {
        let request = crate::dongle_features::audio_request(3, true, 174000).unwrap();
        let mut state = ntpods_ipc::DongleAudioState {
            requested_codec: 3,
            variable: true,
            requested_bitrate: 174000,
            codec: 0,
            bitrate: 173600,
            pending: true,
            ..Default::default()
        };
        assert!(profile_matches(&request, &state));
        assert!(state.pending && !state.streaming);
        state.requested_bitrate = 173600;
        assert!(!profile_matches(&request, &state));
        state.requested_bitrate = 174000;
        state.variable = false;
        assert!(!profile_matches(&request, &state));
    }
    #[test]
    fn setting_readback_requires_a_new_revision_and_discards_missing_values() {
        let mut worker = Worker::default();
        let records = |revision, value| {
            BTreeMap::from([(
                0x23,
                crate::dongle_features::Setting {
                    age_ms: 60000,
                    revision,
                    value,
                },
            )])
        };
        worker.observe_settings(records(10, 0));
        let token = worker
            .setting_writes
            .begin_control(&mut worker.settings, 0x23, 2, false);
        worker
            .setting_writes
            .finish_control(&mut worker.settings, 0x23, 2, token, true);
        worker.observe_settings(records(10, 2));
        assert_eq!(worker.health.device_controls.get(&0x23), Some(&0));
        assert_eq!(worker.health.device_controls_pending.get(&0x23), Some(&2));
        worker.observe_settings(records(9, 2));
        assert_eq!(worker.health.device_controls_pending.get(&0x23), Some(&2));
        worker.observe_settings(records(11, 2));
        assert_eq!(worker.health.device_controls.get(&0x23), Some(&2));
        assert!(!worker.health.device_controls_pending.contains_key(&0x23));
        worker.observe_settings(BTreeMap::new());
        assert!(worker.health.device_controls.is_empty());
    }
    #[test]
    fn finder_survives_aacp_reconnect_but_never_pair_or_usb_replacement() {
        let mut worker = Worker::default();
        let mut d = DongleState {
            present: true,
            session: 10,
            pair_token: 20,
            generation: 1,
            ..Default::default()
        };
        let target = FinderTarget::of(&d);
        worker.select(&d);
        worker.health.nearby_finder.active = true;
        d.generation = 2;
        d.connected = false;
        assert!(target.matches(&d));
        worker.select(&d);
        assert!(worker.health.nearby_finder.active);
        d.pair_token = 21;
        assert!(!target.matches(&d));
        worker.select(&d);
        assert!(!worker.health.nearby_finder.active);
        d.session = 11;
        assert!(!target.matches(&d));
    }
    #[test]
    fn mode_confirmation_waits_for_a_new_device_report_not_cached_bytes() {
        let mut worker = Worker::default();
        let report = |revision| {
            Some(Cached {
                age_ms: 0,
                revision,
                packet: vec![4, 0, 4, 0, 9, 0, 0x2c, 1, 1, 0, 0],
            })
        };
        worker.observe_mode(2, report(7));
        assert_eq!(worker.modes.state().hearing_enabled, Some(true));
        worker.mode_fences[0] = Some(7);
        worker.observe_mode(2, report(7));
        assert_eq!(worker.modes.state().hearing_enabled, None);
        worker.observe_mode(2, None);
        assert_eq!(worker.modes.state().hearing_enabled, None);
        worker.observe_mode(2, report(8));
        assert_eq!(worker.modes.state().hearing_enabled, Some(true));
        assert!(worker.mode_fences[0].is_none());
        worker.mode_fences[0] = Some(u32::MAX);
        worker.observe_mode(2, report(1));
        assert_eq!(worker.modes.state().hearing_enabled, Some(true));
    }
    #[test]
    fn hardware_wait_survives_case_disconnect_but_not_usb_replacement() {
        let target = Target {
            session: 4,
            generation: 10,
        };
        let ear = ntpods_ipc::HardwareDiagnosticEar {
            measured_at_unix_seconds: 1,
            first_use_unix_seconds: None,
            general: 0,
            measurement: 0,
            component_failures: None,
            unknown_bits: 0,
        };
        let baseline = ntpods_ipc::HardwareDiagnostics {
            read_at_unix_seconds: 2,
            left: ear,
            right: ear,
        };
        let check = Check {
            coordinator: hardware_check::Coordinator::start(8, target.id(), baseline, 0).unwrap(),
            session: 4,
            peer: [1, 2, 3, 4, 5, 6],
            target: target.id(),
            id: 8,
            epoch: 0,
            started: Instant::now(),
            last_read: Instant::now(),
        };
        let mut worker = Worker {
            target,
            check: Some(check),
            ..Default::default()
        };
        let d = DongleState {
            present: true,
            session: 4,
            generation: 11,
            ..Default::default()
        };
        worker.select(&d);
        assert!(worker.check.as_ref().unwrap().coordinator.is_active());
        assert_eq!(worker.check.as_ref().unwrap().peer, [1, 2, 3, 4, 5, 6]);
        assert_eq!(worker.target.generation, 11);
        worker.select(&DongleState { session: 5, ..d });
        assert!(worker.check.is_none());
    }
    #[test]
    fn operation_guard_stops_stale_cancelled_timed_out_and_competing_work() {
        let target = Target {
            session: 1,
            generation: 2,
        };
        let d = DongleState {
            present: true,
            connected: true,
            controls_ready: true,
            session: 1,
            generation: 2,
            ..Default::default()
        };
        assert!(request_gate(&d, target, false, false).is_ok());
        assert!(request_gate(&d, target, true, false)
            .unwrap_err()
            .contains("cancelled"));
        assert!(request_gate(&d, target, false, true)
            .unwrap_err()
            .contains("timed out"));
        for changed in [
            DongleState {
                generation: 3,
                ..d.clone()
            },
            DongleState {
                microphone_active: true,
                ..d.clone()
            },
            DongleState {
                pending: true,
                ..d.clone()
            },
            DongleState {
                connected: false,
                ..d.clone()
            },
        ] {
            assert!(request_gate(&changed, target, false, false).is_err());
        }
    }
    #[test]
    fn rejected_reads_persist_but_cannot_replace_a_later_completion() {
        let target = Target {
            session: 1,
            generation: 2,
        };
        let mut worker = Worker {
            target,
            ..Default::default()
        };
        let rejected = Instant::now();
        worker.reject(Notice {
            target,
            command: Command::GetHearingAid,
            error: "Stop recording first.".into(),
            at: rejected,
        });
        assert_eq!(
            worker.health.hearing_aid_error.as_deref(),
            Some("Stop recording first.")
        );
        pending(&mut worker.health, &Command::GetHearingAid, false, None);
        worker.finished[0] = Some(rejected + Duration::from_millis(1));
        worker.reject(Notice {
            target,
            command: Command::GetHearingAid,
            error: "Old busy error.".into(),
            at: rejected,
        });
        assert!(worker.health.hearing_aid_error.is_none());
    }
    #[test]
    fn missing_or_changed_metadata_invalidates_cached_edit_permission() {
        let mut worker = Worker::default();
        worker.observe_metadata(Some((
            "A3063".into(),
            "9A348".into(),
            "private serial".into(),
        )));
        worker.health.hearing_aid_editable = true;
        worker.observe_metadata(None);
        assert!(worker.health.model.is_empty());
        assert!(worker.health.firmware.is_empty());
        assert!(!worker.health.hearing_aid_editable);
        assert!(!worker.hearing_supported());
    }
    #[test]
    fn session_and_generation_reject_old_or_ambiguous_targets() {
        let target = Target {
            session: 5,
            generation: 3,
        };
        let mut d = DongleState {
            present: true,
            session: 5,
            generation: 3,
            ..Default::default()
        };
        assert!(target.matches(&d));
        d.generation += 1;
        assert!(!target.matches(&d));
        d.generation = 3;
        d.session += 1;
        assert!(!target.matches(&d));
        d.session = 5;
        d.ambiguous = true;
        assert!(!target.matches(&d));
    }
    #[test]
    fn self_check_requires_exact_firmware_fresh_case_and_idle_routes() {
        let h = DongleHealthState {
            model: "A3063".into(),
            firmware: "9A348".into(),
            earbuds_in_case: Some(true),
            ..Default::default()
        };
        let d = DongleState::default();
        assert!(check_gate(&h, Some(30_000), &d, true).is_ok());
        for age in [None, Some(30_001)] {
            assert!(check_gate(&h, age, &d, true).is_err());
        }
        assert!(check_gate(&h, Some(0), &d, false).is_err());
        for state in [
            DongleState {
                microphone_active: true,
                ..d.clone()
            },
            DongleState {
                usb_playback: true,
                ..d.clone()
            },
            DongleState {
                streaming: true,
                ..d.clone()
            },
            DongleState {
                phone_connected: true,
                ..d.clone()
            },
        ] {
            assert!(check_gate(&h, Some(0), &state, true).is_err());
        }
        for changed in [
            DongleHealthState {
                model: "unknown".into(),
                ..h.clone()
            },
            DongleHealthState {
                firmware: "9A999".into(),
                ..h.clone()
            },
            DongleHealthState {
                earbuds_in_case: None,
                ..h.clone()
            },
            DongleHealthState {
                earbuds_in_case: Some(false),
                ..h.clone()
            },
        ] {
            assert!(check_gate(&changed, Some(0), &d, true).is_err());
        }
    }
    #[test]
    fn target_change_discards_profiles_and_mode_reports() {
        let mut worker = Worker::default();
        let d = DongleState {
            present: true,
            session: 1,
            generation: 3,
            ..Default::default()
        };
        worker.select(&d);
        worker.health.model = "A3063".into();
        worker.health.hearing_aid_editable = true;
        worker.modes.observe(&[4, 0, 4, 0, 9, 0, 0x2c, 1, 1, 0, 0]);
        worker.select(&DongleState { generation: 4, ..d });
        assert!(worker.health.model.is_empty());
        assert!(!worker.health.hearing_aid_editable);
        assert!(worker.modes.state().hearing_enabled.is_none());
    }
    #[test]
    fn nested_or_unrelated_commands_are_not_forwarded() {
        assert!(!supported(&Command::GetLoudSoundReduction));
        assert!(!supported(&Command::DongleControl {
            session: 1,
            generation: 2,
            opcode: 1,
            argument: 0
        }));
        assert!(supported(&Command::GetHardwareDiagnostics));
    }
}
