//! Session-local call provenance. Nothing in this module survives reconnect.
use super::*;

#[derive(Clone)]
pub(super) struct Context {
    pub episode: String,
    pub call_gen: String,
    pub origin_token: String,
    pub returning: bool,
}

impl Context {
    pub fn purpose(&self) -> &'static str {
        if self.returning {
            "call_return"
        } else {
            "call_takeover"
        }
    }
    fn matches(&self, frame: &Frame) -> bool {
        frame.episode.as_ref() == Some(&self.episode)
            && frame.call_gen.as_ref() == Some(&self.call_gen)
    }
    fn bind_return(
        &self,
        transfer: &mut Transfer,
        generation: u64,
        request: u64,
        intent: u64,
    ) -> Result<(), &'static str> {
        if !transfer.tokens_current(generation, request, intent)
            || transfer.target != CompanionTarget::Pc
        {
            return Err("superseded");
        }
        let mut context = self.clone();
        context.returning = true;
        transfer.call = Some(context);
        Ok(())
    }
}

struct Origin {
    token: String,
    guard: ResumeGuard,
    native: Option<(u64, u64, u64)>,
}

impl Origin {
    fn current(&self, ctx: &Ctx) -> bool {
        self.guard.current(ctx)
            && ctx.connect_requested.explicit()
            && ctx.companion.lock().unwrap().native_open == self.native
            && policy(ctx, false)
    }
}

struct Lease {
    context: Context,
    generation: u64,
    authorization: u64,
    request: u64,
    intent: u64,
    last_state: Instant,
    quiet: bool,
    released: bool,
    ended: bool,
    pause_ready: bool,
    resuming: bool,
    pause: crate::media::SelectedPause,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum EndPlan {
    WaitForPause,
    ResumeCurrent,
    ReturnToPc,
}

fn end_plan(
    released: bool,
    pause_ready: bool,
    quiet: bool,
    fresh: bool,
    current: bool,
    permitted: bool,
) -> Result<EndPlan, &'static str> {
    if !quiet || !fresh || !current || !permitted {
        return Err("superseded");
    }
    Ok(if released {
        EndPlan::ReturnToPc
    } else if pause_ready {
        EndPlan::ResumeCurrent
    } else {
        EndPlan::WaitForPause
    })
}

fn lease_tokens_current(expected: [u64; 4], observed: [u64; 4]) -> bool {
    expected == observed
}

fn state_is_known(state: Option<&str>, sco: Option<&str>, bounded_takeover: bool) -> bool {
    state != Some("unknown") && (sco != Some("unknown") || bounded_takeover)
}

impl Lease {
    fn fences_current(&self, ctx: &Ctx, link: &Link) -> bool {
        let state = ctx.companion.lock().unwrap();
        let current = state.connected
            && lease_tokens_current(
                [
                    self.authorization,
                    self.generation,
                    self.request,
                    self.intent,
                ],
                [
                    state.authorization,
                    state.generation,
                    ctx.connect_requested.snapshot(),
                    bt::playback_intent(),
                ],
            );
        drop(state);
        current
            && ctx.mac() == link.device
            && !ctx.shutting_down.load(Ordering::Acquire)
            && ctx.connect_requested.is_current(self.request)
            && bt::playback_intent() == self.intent
            && policy(ctx, false)
    }
    fn current(&self, ctx: &Ctx, link: &Link) -> bool {
        self.fences_current(ctx, link) && !busy(ctx, link.device)
    }
}

fn policy(ctx: &Ctx, returning: bool) -> bool {
    let store = ctx.companion_preferences.lock().unwrap();
    store.consent()
        && store
            .get(ctx.mac(), "call_takeover_enabled")
            .is_some_and(|r| r.value == true)
        && (!returning
            || store
                .get(ctx.mac(), "call_return_enabled")
                .is_some_and(|r| r.value == true))
}

#[derive(Default)]
pub(super) struct Calls {
    origin: Option<Origin>,
    advertised: String,
    lease: Option<Lease>,
    episodes: HashSet<String>,
    next_busy: Option<Instant>,
}

impl Calls {
    pub fn revoke(&mut self) {
        self.origin = None;
        self.lease = None;
    }

    pub fn origin(
        &mut self,
        stream: &Framed,
        ctx: &Ctx,
        link: &Link,
        sid: &str,
        available: bool,
        authorization: u64,
    ) -> io::Result<()> {
        let _actions = reserve_authorized(&ctx.companion_actions, &ctx.companion, authorization)?;
        if ctx.mac() != link.device || ctx.shutting_down.load(Ordering::Acquire) {
            return Err(invalid());
        }
        if !available || self.lease.is_some() || !policy(ctx, false) {
            self.origin = None;
        } else if self
            .origin
            .as_ref()
            .is_none_or(|origin| !origin.current(ctx))
        {
            self.origin = None;
            if let Some(guard) =
                ResumeGuard::capture(ctx).filter(|_| ctx.connect_requested.explicit())
            {
                let native = ctx.companion.lock().unwrap().native_open;
                let native_mode =
                    ctx.state.lock().unwrap().playback_mode != ntpods_ipc::PlaybackMode::Windows;
                if !native_mode
                    || native.is_some_and(|(address, intent, _)| {
                        address == link.device && intent == bt::playback_intent()
                    })
                {
                    self.origin = Some(Origin {
                        token: hex(&random::<16>()?),
                        guard,
                        native,
                    });
                }
            }
        }
        let ready = self.origin.is_some();
        let token = self
            .origin
            .as_ref()
            .map_or(ZERO_EPISODE, |o| o.token.as_str());
        // A new token is sent once. A revoked token is explicitly withdrawn.
        if self.advertised != token {
            let mut frame = Frame::new("call_origin", ZERO_TX, link.device, Some(sid));
            frame.ready = Some(ready);
            frame.origin_token = Some(token.into());
            stream.send(&frame)?;
        }
        self.advertised = token.into();
        Ok(())
    }

    pub fn begin(
        &mut self,
        ctx: &Ctx,
        link: &Link,
        frame: &Frame,
        authorization: u64,
    ) -> Result<Transfer, &'static str> {
        let _actions = reserve_authorized(&ctx.companion_actions, &ctx.companion, authorization)
            .map_err(|_| "superseded")?;
        if self.lease.is_some()
            || self.episodes.len() >= 128
            || !self.episodes.insert(frame.episode.clone().unwrap())
        {
            return Err("invalid_state");
        }
        let origin = self.origin.as_ref().ok_or("unavailable")?;
        if frame.origin_token.as_ref() != Some(&origin.token) || !origin.current(ctx) {
            return Err("superseded");
        }
        eligible(ctx, link)?;
        let started = Instant::now();
        let context = Context {
            episode: frame.episode.clone().unwrap(),
            call_gen: frame.call_gen.clone().unwrap(),
            origin_token: origin.token.clone(),
            returning: false,
        };
        let state = ctx.companion.lock().unwrap();
        let transfer = Transfer {
            tx: frame.tx.clone(),
            target: CompanionTarget::Android,
            call: Some(context.clone()),
            authorization,
            request_id: None,
            generation: state.generation,
            request: ctx.connect_requested.snapshot(),
            audio_intent: bt::playback_intent(),
            phase: Phase::Pause,
            phase_deadline: started + PHASE_LIMIT,
            total_deadline: started + TOTAL_LIMIT,
        };
        self.lease = Some(Lease {
            context,
            generation: transfer.generation,
            authorization: state.authorization,
            request: transfer.request,
            intent: transfer.audio_intent,
            last_state: started,
            quiet: false,
            released: false,
            ended: false,
            pause_ready: false,
            resuming: false,
            pause: crate::media::SelectedPause::start(link.device),
        });
        drop(state);
        self.origin = None;
        publish_phase(ctx, Phase::Pause);
        Ok(transfer)
    }

    pub fn state(&mut self, frame: &Frame, bounded_takeover: bool) {
        if let Some(lease) = self.lease.as_mut() {
            if !lease.context.matches(frame)
                || !state_is_known(
                    frame.state.as_deref(),
                    frame.sco.as_deref(),
                    bounded_takeover,
                )
            {
                self.lease = None;
            } else {
                lease.last_state = Instant::now();
                lease.quiet = frame.state.as_deref() == Some("idle")
                    && frame.all_idle == Some(true)
                    && frame.sco.as_deref() == Some("idle");
            }
        }
    }

    pub fn sync(&mut self, ctx: &Ctx, link: &Link, transfer: &Transfer) {
        if let Some(lease) = self.lease.as_mut() {
            if transfer.call.as_ref().is_some_and(|call| {
                call.episode == lease.context.episode
                    && call.call_gen == lease.context.call_gen
                    && call.origin_token == lease.context.origin_token
            }) && transfer.current(ctx, link)
            {
                lease.request = transfer.request;
                lease.intent = transfer.audio_intent;
            }
        }
    }

    pub fn released(&mut self, transfer: &Transfer) {
        if let Some(lease) = self.lease.as_mut() {
            if transfer.call.as_ref().is_some_and(|call| {
                !call.returning
                    && call.episode == lease.context.episode
                    && call.call_gen == lease.context.call_gen
            }) && transfer.generation == lease.generation
                && transfer.request == lease.request
                && transfer.audio_intent == lease.intent
            {
                lease.released = true;
            }
        }
    }

    pub fn poll(&mut self, ctx: &Ctx, link: &Link) -> Result<bool, &'static str> {
        let Some(lease) = self.lease.as_mut() else {
            return Ok(false);
        };
        let now = Instant::now();
        let check_busy = self.next_busy.is_none_or(|next| now >= next);
        if check_busy {
            self.next_busy = Some(now + Duration::from_millis(500));
        }
        if !lease.fences_current(ctx, link)
            || check_busy && busy(ctx, link.device)
            || Instant::now().duration_since(lease.last_state) >= Duration::from_secs(30)
        {
            self.lease = None;
            return Err("superseded");
        }
        while let Some(receipt) = lease.pause.poll() {
            match receipt {
                crate::media::PauseReceipt::Paused | crate::media::PauseReceipt::Quiet => {
                    lease.pause_ready = true
                }
                crate::media::PauseReceipt::Failed => {
                    self.lease = None;
                    return Err("failed");
                }
                crate::media::PauseReceipt::Resumed(ok) => {
                    crate::log(&format!("companion: call player resume confirmed={ok}"));
                    self.lease = None;
                    return Ok(false);
                }
            }
        }
        Ok(lease.pause_ready)
    }

    pub fn end(&mut self, ctx: &Ctx, link: &Link, frame: &Frame) -> Result<EndPlan, &'static str> {
        let Some(lease) = self.lease.as_mut() else {
            return Err("invalid_state");
        };
        if !lease.context.matches(frame)
            || frame.origin_token.as_ref() != Some(&lease.context.origin_token)
            || frame.reason.as_deref() != Some("ended")
        {
            self.revoke();
            return Err("superseded");
        }
        let plan = end_plan(
            lease.released,
            lease.pause_ready,
            lease.quiet,
            Instant::now().duration_since(lease.last_state) < Duration::from_secs(30),
            lease.current(ctx, link),
            policy(ctx, true),
        )?;
        lease.ended = true;
        Ok(plan)
    }

    pub fn returning(&mut self, transfer: &mut Transfer) -> Result<(), &'static str> {
        let lease = self.lease.as_mut().ok_or("superseded")?;
        if !lease.ended || !lease.released {
            return Err("invalid_state");
        }
        lease
            .context
            .bind_return(transfer, lease.generation, lease.request, lease.intent)
    }

    pub fn resume(&mut self, ctx: &Ctx, link: &Link) -> Result<(), &'static str> {
        let _actions = ctx.companion_actions.lock().unwrap();
        let lease = self.lease.as_mut().ok_or("superseded")?;
        if !lease.ended
            || !lease.quiet
            || !lease.pause_ready
            || !lease.current(ctx, link)
            || !policy(ctx, true)
        {
            return Err("superseded");
        }
        let guard = ResumeGuard::capture(ctx).ok_or("unavailable")?;
        let authorization = lease.authorization;
        let last_state = lease.last_state;
        let resume_ctx = ctx.clone();
        if !lease.pause.resume(move || {
            Instant::now().duration_since(last_state) < Duration::from_secs(30)
                && guard.current(&resume_ctx)
                && policy(&resume_ctx, true)
                && {
                    let state = resume_ctx.companion.lock().unwrap();
                    state.connected && state.authorization == authorization
                }
        }) {
            return Err("failed");
        }
        lease.resuming = true;
        Ok(())
    }

    pub fn has_lease(&self) -> bool {
        self.lease.is_some()
    }
    pub fn needs_local_resume(&self) -> bool {
        self.lease.as_ref().is_some_and(|lease| {
            lease.ended && !lease.released && lease.pause_ready && !lease.resuming
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_call_before_pause_receipt_waits_without_releasing_or_resuming() {
        assert_eq!(
            end_plan(false, false, true, true, true, true),
            Ok(EndPlan::WaitForPause)
        );
        assert_eq!(
            end_plan(false, true, true, true, true, true),
            Ok(EndPlan::ResumeCurrent)
        );
    }

    #[test]
    fn release_receipt_allows_fresh_return_even_when_hfp_acquire_never_completed() {
        assert_eq!(
            end_plan(true, true, true, true, true, true),
            Ok(EndPlan::ReturnToPc)
        );
        for proof in [
            (false, true, true, true),
            (true, false, true, true),
            (true, true, false, true),
            (true, true, true, false),
        ] {
            assert!(end_plan(true, true, proof.0, proof.1, proof.2, proof.3).is_err());
        }
    }

    #[test]
    fn every_newer_local_or_session_owner_retires_the_old_call_fences() {
        let original = [3, 7, 13, 19];
        assert!(lease_tokens_current(original, original));
        for index in 0..4 {
            let mut newer = original;
            newer[index] += 1;
            assert!(!lease_tokens_current(original, newer));
        }
    }

    #[test]
    fn call_context_cannot_be_reused_for_a_new_episode_or_generation() {
        let context = Context {
            episode: "11".repeat(16),
            call_gen: "0000000000000001".into(),
            origin_token: "22".repeat(16),
            returning: false,
        };
        let mut state = Frame::new("call_state", ZERO_TX, 0xaabbccddeeff, None);
        state.episode = Some(context.episode.clone());
        state.call_gen = Some(context.call_gen.clone());
        assert!(context.matches(&state));
        state.call_gen = Some("0000000000000002".into());
        assert!(!context.matches(&state));
        state.call_gen = Some(context.call_gen.clone());
        state.episode = Some("33".repeat(16));
        assert!(!context.matches(&state));
        assert_eq!(context.purpose(), "call_takeover");
        assert_eq!(
            Context {
                returning: true,
                ..context
            }
            .purpose(),
            "call_return"
        );
    }

    #[test]
    fn a_processed_disconnect_between_call_end_and_return_cannot_bind_the_old_lease() {
        let context = Context {
            episode: "11".repeat(16),
            call_gen: "0000000000000001".into(),
            origin_token: "22".repeat(16),
            returning: false,
        };
        let now = Instant::now();
        let mut transfer = Transfer {
            tx: "0000000000000002".into(),
            target: CompanionTarget::Pc,
            call: None,
            authorization: 3,
            request_id: None,
            generation: 8,
            request: 14,
            audio_intent: 20,
            phase: Phase::Prepare,
            phase_deadline: now + PHASE_LIMIT,
            total_deadline: now + TOTAL_LIMIT,
        };
        assert!(context.bind_return(&mut transfer, 7, 13, 19).is_err());
        assert!(transfer.call.is_none());
        transfer.generation = 7;
        transfer.request = 13;
        transfer.audio_intent = 19;
        context.bind_return(&mut transfer, 7, 13, 19).unwrap();
        assert!(transfer
            .call
            .as_ref()
            .is_some_and(|call| call.returning && call.episode == context.episode));
    }

    #[test]
    fn bounded_takeover_heartbeat_cannot_invent_quiet_sco_or_known_telephony() {
        assert!(state_is_known(Some("ringing"), Some("unknown"), true));
        assert!(!state_is_known(Some("ringing"), Some("unknown"), false));
        assert!(!state_is_known(Some("unknown"), Some("idle"), true));
        assert!(!state_is_known(Some("unknown"), Some("unknown"), true));
        assert!(state_is_known(Some("idle"), Some("idle"), false));
    }
}
