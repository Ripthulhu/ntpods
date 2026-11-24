//! Session-local call provenance. Nothing in this module survives reconnect.
use super::*;

#[derive(Clone)]
pub(super) struct Context {
    pub episode: String,
    pub call_gen: String,
    pub origin_token: String,
    pub returning: bool,
    pub source_released: bool,
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
        if self.source_released
            || !transfer.tokens_current(generation, request, intent)
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
    fn current(&self, ctx: &Ctx, route: Option<&MediaRoute>) -> bool {
        self.guard.current_route(ctx, route)
            && (self.guard.route.is_some()
                || ctx.connect_requested.explicit()
                    && ctx.companion.lock().unwrap().native_open == self.native)
            && policy(ctx, self.guard.address, false)
    }
}

struct CallRoute {
    ticket: crate::dongle_routing::Ticket,
    revision: u32,
    next: Option<u32>,
}
impl CallRoute {
    fn current(&self, ctx: &Ctx, observed: Option<&MediaRoute>) -> bool {
        if !self.ticket.current(ctx) && !self.ticket.transitioning(ctx) {
            return false;
        }
        let fresh = observed.filter(|(ticket, _, at)| {
            ticket.current(ctx) && at.elapsed() <= Duration::from_secs(1)
        });
        fresh.map_or(
            self.next.is_some() && self.ticket.transitioning(ctx),
            |(_, status, _)| {
                !status.busy
                    && (status.revision == self.revision || self.next == Some(status.revision))
            },
        )
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
    pause: Option<crate::media::SelectedPause>,
    route: Option<CallRoute>,
    return_allowed: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum EndPlan {
    WaitForPause,
    ResumeCurrent,
    ReturnToPc,
    Finish,
}

fn end_plan(
    released: bool,
    pause_ready: bool,
    quiet: bool,
    fresh: bool,
    current: bool,
    permitted: bool,
    return_allowed: bool,
) -> Result<EndPlan, &'static str> {
    if !quiet || !fresh || !current || !permitted {
        return Err("superseded");
    }
    Ok(if !return_allowed {
        if !released {
            return Err("invalid_state");
        }
        EndPlan::Finish
    } else if released {
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
            && selected_pair(ctx, link.device)
            && !ctx.shutting_down.load(Ordering::Acquire)
            && ctx.connect_requested.is_current(self.request)
            && bt::playback_intent() == self.intent
            && policy(ctx, link.device, false)
    }
    fn current(&self, ctx: &Ctx, link: &Link) -> bool {
        self.fences_current(ctx, link) && !busy(ctx, link.device)
    }
}

fn policy(ctx: &Ctx, address: u64, returning: bool) -> bool {
    let store = ctx.companion_preferences.lock().unwrap();
    store.consent()
        && store
            .get(address, "call_takeover_enabled")
            .is_some_and(|r| r.value == true)
        && (!returning
            || store
                .get(address, "call_return_enabled")
                .is_some_and(|r| r.value == true))
}

#[derive(Default)]
pub(super) struct Calls {
    origin: Option<Origin>,
    advertised: String,
    lease: Option<Lease>,
    episodes: HashSet<String>,
    next_busy: Option<Instant>,
    route: std::sync::Arc<Mutex<Option<MediaRoute>>>,
    released_origin: Option<ReleasedMediaOrigin>,
    release_candidate: Option<MediaReleaseCandidate>,
}

struct MediaReleaseCandidate {
    tx: String,
    address: u64,
    generation: u64,
    authorization: u64,
    request: u64,
    intent: u64,
    ticket: crate::dongle_routing::Ticket,
    receipt: (u32, u32, u8),
    deadline: Instant,
    peer_confirmed: bool,
}
impl MediaReleaseCandidate {
    fn confirms(&self, ctx: &Ctx, observed: &MediaRoute) -> bool {
        let (ticket, status, at) = observed;
        self.peer_confirmed
            && Instant::now() < self.deadline
            && ticket.current(ctx)
            && self.ticket.same_physical(ticket)
            && at.elapsed() <= Duration::from_secs(1)
            && status.receipt() == Some(self.receipt)
            && status.released
            && !status.connected
            && !status.owned
            && !status.pending
            && !status.busy
    }
    fn current(&self, ctx: &Ctx, link: &Link) -> bool {
        let state = ctx.companion.lock().unwrap();
        let current = state.connected
            && state.generation == self.generation
            && state.authorization == self.authorization;
        drop(state);
        current
            && self.address == link.device
            && Instant::now() < self.deadline
            && !ctx.shutting_down.load(Ordering::Acquire)
            && ctx.connect_requested.is_current(self.request)
            && bt::playback_intent() == self.intent
            && policy(ctx, self.address, false)
    }
}

struct ReleasedMediaOrigin {
    token: String,
    tx: String,
    address: u64,
    generation: u64,
    authorization: u64,
    request: u64,
    intent: u64,
    route: CallRoute,
    deadline: Instant,
}
impl ReleasedMediaOrigin {
    fn current(&self, ctx: &Ctx, observed: Option<&MediaRoute>) -> bool {
        let state = ctx.companion.lock().unwrap();
        let authorized = state.connected
            && state.generation == self.generation
            && state.authorization == self.authorization;
        drop(state);
        authorized
            && Instant::now() < self.deadline
            && selected_pair(ctx, self.address)
            && ctx.connect_requested.is_current(self.request)
            && bt::playback_intent() == self.intent
            && policy(ctx, self.address, false)
            && self.route.current(ctx, observed)
            && observed.is_some_and(|(_, status, at)| {
                at.elapsed() <= Duration::from_secs(1)
                    && status.released
                    && !status.connected
                    && !status.owned
            })
            && crate::media::companion_player_state().is_ok_and(|player| player.is_none())
    }
}

impl Calls {
    pub fn revoke(&mut self) {
        self.origin = None;
        self.lease = None;
        self.released_origin = None;
        self.release_candidate = None;
    }

    pub fn revoke_preserving_media(&mut self, ctx: &Ctx, link: &Link) {
        self.origin = None;
        self.lease = None;
        self.released_origin = None;
        if self
            .release_candidate
            .as_ref()
            .is_some_and(|candidate| !candidate.current(ctx, link))
            || !crate::media::companion_player_state().is_ok_and(|player| player.is_none())
        {
            self.release_candidate = None;
        }
    }

    pub fn retain_media_release(&mut self, ctx: &Ctx, link: &Link, transfer: &Transfer) {
        if self.lease.is_some()
            || transfer.media_gen.is_none()
            || transfer.target != CompanionTarget::Android
            || !matches!(transfer.phase, Phase::PcRelease | Phase::PhoneAcquire)
            || self
                .release_candidate
                .as_ref()
                .is_some_and(|candidate| candidate.tx == transfer.tx)
        {
            return;
        }
        let Some(route) = transfer.dongle.as_ref() else {
            return;
        };
        let receipt = route
            .result
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .and_then(|status| status.receipt())
            .or_else(|| route.ticket.pending_receipt());
        let Some(receipt) = receipt.filter(|(revision, _, action)| {
            *action == 1 && *revision == route.revision.wrapping_add(1).max(1)
        }) else {
            return;
        };
        let candidate = MediaReleaseCandidate {
            tx: transfer.tx.clone(),
            address: link.device,
            generation: transfer.generation,
            authorization: transfer.authorization,
            request: transfer.request,
            intent: transfer.audio_intent,
            ticket: route.ticket.clone(),
            receipt,
            deadline: Instant::now() + Duration::from_secs(5),
            peer_confirmed: false,
        };
        if candidate.current(ctx, link) {
            self.release_candidate = Some(candidate);
        }
    }

    pub fn peer_cancelled_media(
        &mut self,
        ctx: &Ctx,
        link: &Link,
        transfer: Option<&Transfer>,
        tx: &str,
    ) {
        if let Some(transfer) = transfer.filter(|transfer| transfer.tx == tx) {
            self.retain_media_release(ctx, link, transfer);
        }
        if let Some(candidate) = self
            .release_candidate
            .as_mut()
            .filter(|candidate| candidate.tx == tx)
        {
            if candidate.current(ctx, link) {
                candidate.peer_confirmed = true;
            }
        }
    }

    pub fn waiting_media_release(&self, ctx: &Ctx, link: &Link) -> bool {
        self.lease.is_none()
            && self
                .release_candidate
                .as_ref()
                .is_some_and(|candidate| candidate.current(ctx, link))
    }

    pub fn media_origin(&mut self, ctx: &Ctx, link: &Link, transfer: Option<&Transfer>) {
        if self.lease.is_some() {
            self.released_origin = None;
            self.release_candidate = None;
            return;
        }
        if self
            .release_candidate
            .as_ref()
            .is_some_and(|candidate| !candidate.current(ctx, link))
        {
            self.release_candidate = None;
        }
        let Some(transfer) = transfer.filter(|t| {
            !t.expired(Instant::now())
                && t.media_gen.is_some()
                && t.target == CompanionTarget::Android
                && matches!(t.phase, Phase::PcRelease | Phase::PhoneAcquire)
        }) else {
            if self.release_candidate.is_none() {
                self.released_origin = None;
            }
            if let Some(candidate) = self
                .release_candidate
                .as_ref()
                .filter(|candidate| candidate.peer_confirmed)
            {
                let observed = self.route.lock().unwrap().clone();
                if let Some((ticket, status, _)) =
                    observed.filter(|observed| candidate.confirms(ctx, observed))
                {
                    if self.released_origin.as_ref().is_none_or(|origin| {
                        origin.tx != candidate.tx || origin.route.revision != status.revision
                    }) {
                        let Ok(token) = random::<16>() else {
                            return;
                        };
                        self.released_origin = Some(ReleasedMediaOrigin {
                            token: hex(&token),
                            tx: candidate.tx.clone(),
                            address: candidate.address,
                            generation: candidate.generation,
                            authorization: candidate.authorization,
                            request: candidate.request,
                            intent: candidate.intent,
                            route: CallRoute {
                                ticket,
                                revision: status.revision,
                                next: None,
                            },
                            deadline: candidate.deadline,
                        });
                    }
                }
            }
            return;
        };
        if self.lease.is_some() || !policy(ctx, link.device, false) {
            self.released_origin = None;
            return;
        }
        let Some(job) = transfer.dongle.as_ref() else {
            self.released_origin = None;
            return;
        };
        let Some(Ok(status)) = job.poll().filter(|result| {
            result.as_ref().is_ok_and(|status| {
                status.released
                    && !status.connected
                    && !status.owned
                    && status.revision == job.revision.wrapping_add(1).max(1)
            })
        }) else {
            self.released_origin = None;
            return;
        };
        let mut cached = self.route.lock().unwrap();
        if cached.as_ref().is_none_or(|old| status.received >= old.2) {
            *cached = Some((job.ticket.clone(), status, status.received));
        }
        drop(cached);
        if self.released_origin.as_ref().is_none_or(|origin| {
            origin.tx != transfer.tx || origin.route.revision != status.revision
        }) {
            let Ok(token) = random::<16>() else {
                self.released_origin = None;
                return;
            };
            self.released_origin = Some(ReleasedMediaOrigin {
                token: hex(&token),
                tx: transfer.tx.clone(),
                address: link.device,
                generation: transfer.generation,
                authorization: transfer.authorization,
                request: transfer.request,
                intent: transfer.audio_intent,
                route: CallRoute {
                    ticket: job.ticket.clone(),
                    revision: status.revision,
                    next: None,
                },
                deadline: transfer.total_deadline,
            });
        }
    }

    pub fn origin(
        &mut self,
        stream: &Framed,
        ctx: &Ctx,
        link: &Link,
        sid: &str,
        available: bool,
        authorization: u64,
        route: Option<&MediaRoute>,
    ) -> io::Result<()> {
        let mut cached = self.route.lock().unwrap();
        if let Some(incoming) = route {
            if cached
                .as_ref()
                .is_none_or(|old| incoming.1.peer != old.1.peer || incoming.2 >= old.2)
            {
                *cached = Some(incoming.clone());
            }
        } else if cached
            .as_ref()
            .is_some_and(|old| old.2.elapsed() > Duration::from_secs(1))
        {
            *cached = None;
        }
        let observed = cached.clone();
        drop(cached);
        let route = observed.as_ref();
        let _actions = reserve_authorized(&ctx.companion_actions, &ctx.companion, authorization)?;
        if (!selected_pair(ctx, link.device) && !self.waiting_media_release(ctx, link))
            || ctx.shutting_down.load(Ordering::Acquire)
        {
            return Err(invalid());
        }
        if !available || self.lease.is_some() || !policy(ctx, link.device, false) {
            self.origin = None;
        } else if self
            .origin
            .as_ref()
            .is_none_or(|origin| !origin.current(ctx, route))
        {
            self.origin = None;
            if let Some(guard) = ResumeGuard::capture_for(ctx, link.device, route)
                .filter(|guard| guard.route.is_some() || ctx.connect_requested.explicit())
            {
                let native = ctx.companion.lock().unwrap().native_open;
                let native_mode =
                    ctx.state.lock().unwrap().playback_mode != ntpods_ipc::PlaybackMode::Windows;
                if guard.route.is_some()
                    || !native_mode
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
        let released = self
            .released_origin
            .as_ref()
            .filter(|origin| origin.current(ctx, route));
        let ready = self.origin.is_some() || released.is_some();
        let token = self
            .origin
            .as_ref()
            .map(|o| o.token.as_str())
            .or_else(|| released.map(|o| o.token.as_str()))
            .unwrap_or(ZERO_EPISODE);
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
        let observed = self.route.lock().unwrap().clone();
        if frame.origin_token.as_ref() != Some(&origin.token)
            || !origin.current(ctx, observed.as_ref())
        {
            return Err("superseded");
        }
        eligible(ctx, link)?;
        let started = Instant::now();
        let context = Context {
            episode: frame.episode.clone().unwrap(),
            call_gen: frame.call_gen.clone().unwrap(),
            origin_token: origin.token.clone(),
            returning: false,
            source_released: false,
        };
        let state = ctx.companion.lock().unwrap();
        let transfer = Transfer {
            tx: frame.tx.clone(),
            target: CompanionTarget::Android,
            call: Some(context.clone()),
            media_gen: None,
            media_source_gen: None,
            dongle: origin
                .guard
                .route
                .as_ref()
                .map(|(ticket, status, _)| DongleTransfer {
                    ticket: ticket.clone(),
                    revision: status.revision,
                    revision_at: status.received,
                    preparing: true,
                    job: Mutex::new(Some(crate::dongle_routing::probe(
                        ctx.clone(),
                        ticket.clone(),
                    ))),
                    result: Mutex::new(None),
                }),
            media_fence: None,
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
            pause: Some(if origin.guard.route.is_some() {
                crate::media::SelectedPause::start_dongle()
            } else {
                crate::media::SelectedPause::start(link.device)
            }),
            route: origin
                .guard
                .route
                .as_ref()
                .map(|(ticket, status, _)| CallRoute {
                    ticket: ticket.clone(),
                    revision: status.revision,
                    next: None,
                }),
            return_allowed: true,
        });
        drop(state);
        self.origin = None;
        publish_phase(ctx, Phase::Pause);
        Ok(transfer)
    }

    pub fn begin_from_media_release(
        &mut self,
        ctx: &Ctx,
        link: &Link,
        frame: &Frame,
        authorization: u64,
        media: Option<&Transfer>,
    ) -> Result<Transfer, &'static str> {
        let _actions = reserve_authorized(&ctx.companion_actions, &ctx.companion, authorization)
            .map_err(|_| "superseded")?;
        let origin = self.released_origin.as_ref().ok_or("unavailable")?;
        let observed = self.route.lock().unwrap().clone();
        let valid_media = media.map_or_else(
            || {
                self.release_candidate.as_ref().is_some_and(|candidate| {
                    candidate.tx == origin.tx
                        && candidate.peer_confirmed
                        && candidate.current(ctx, link)
                })
            },
            |media| {
                !media.expired(Instant::now())
                    && origin.tx == media.tx
                    && media.tokens_current(origin.generation, origin.request, origin.intent)
            },
        );
        if !valid_media
            || origin.address != link.device
            || frame.origin_token.as_ref() != Some(&origin.token)
            || !origin.current(ctx, observed.as_ref())
        {
            return Err("superseded");
        }
        if self.lease.is_some()
            || self.episodes.len() >= 128
            || !self.episodes.insert(frame.episode.clone().unwrap())
        {
            return Err("invalid_state");
        }
        eligible(ctx, link)?;
        let now = Instant::now();
        let context = Context {
            episode: frame.episode.clone().unwrap(),
            call_gen: frame.call_gen.clone().unwrap(),
            origin_token: origin.token.clone(),
            returning: false,
            source_released: true,
        };
        let transfer = Transfer {
            tx: frame.tx.clone(),
            target: CompanionTarget::Android,
            call: Some(context.clone()),
            media_gen: None,
            media_source_gen: None,
            dongle: Some(DongleTransfer {
                ticket: origin.route.ticket.clone(),
                revision: origin.route.revision,
                revision_at: observed
                    .as_ref()
                    .map_or(Instant::now(), |(_, status, _)| status.received),
                preparing: true,
                job: Mutex::new(Some(crate::dongle_routing::probe(
                    ctx.clone(),
                    origin.route.ticket.clone(),
                ))),
                result: Mutex::new(None),
            }),
            media_fence: None,
            authorization,
            request_id: None,
            generation: origin.generation,
            request: origin.request,
            audio_intent: origin.intent,
            phase: Phase::Prepare,
            phase_deadline: now + PHASE_LIMIT,
            total_deadline: now + TOTAL_LIMIT,
        };
        self.lease = Some(Lease {
            context,
            generation: origin.generation,
            authorization,
            request: origin.request,
            intent: origin.intent,
            last_state: now,
            quiet: false,
            released: true,
            ended: false,
            pause_ready: true,
            resuming: false,
            pause: None,
            route: Some(CallRoute {
                ticket: origin.route.ticket.clone(),
                revision: origin.route.revision,
                next: None,
            }),
            return_allowed: false,
        });
        self.origin = None;
        self.released_origin = None;
        self.release_candidate = None;
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
                if let (Some(route), Some(job)) = (lease.route.as_mut(), transfer.dongle.as_ref()) {
                    if let Some(Ok(status)) = job.result.lock().unwrap().as_ref() {
                        if route.revision != status.revision || route.next.is_some() {
                            route.revision = status.revision;
                            route.next = None;
                            *self.route.lock().unwrap() =
                                Some((job.ticket.clone(), *status, status.received));
                        }
                    } else {
                        route.next = matches!(transfer.phase, Phase::PcRelease | Phase::PcAcquire)
                            .then_some(job.revision.wrapping_add(1).max(1));
                    }
                }
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
                if let (Some(route), Some(job)) = (lease.route.as_mut(), transfer.dongle.as_ref()) {
                    if let Some(Ok(status)) = job.result.lock().unwrap().as_ref() {
                        if route.revision != status.revision || route.next.is_some() {
                            route.revision = status.revision;
                            route.next = None;
                            *self.route.lock().unwrap() =
                                Some((job.ticket.clone(), *status, status.received));
                        }
                    }
                }
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
        let observed = self.route.lock().unwrap().clone();
        let transitioning = lease
            .route
            .as_ref()
            .is_some_and(|route| route.next.is_some() && route.ticket.transitioning(ctx));
        if !lease.fences_current(ctx, link)
            || lease
                .route
                .as_ref()
                .is_some_and(|route| !route.current(ctx, observed.as_ref()))
            || check_busy && !transitioning && busy(ctx, link.device)
            || Instant::now().duration_since(lease.last_state) >= Duration::from_secs(30)
        {
            self.lease = None;
            return Err("superseded");
        }
        while let Some(receipt) = lease.pause.as_ref().and_then(|pause| pause.poll()) {
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
            policy(ctx, link.device, lease.return_allowed)
                && lease
                    .route
                    .as_ref()
                    .is_none_or(|route| route.current(ctx, self.route.lock().unwrap().as_ref())),
            lease.return_allowed,
        )?;
        if plan == EndPlan::Finish {
            self.lease = None;
        } else {
            lease.ended = true;
        }
        Ok(plan)
    }

    pub fn returning(&mut self, transfer: &mut Transfer) -> Result<(), &'static str> {
        let lease = self.lease.as_mut().ok_or("superseded")?;
        if !lease.ended || !lease.released || !lease.return_allowed {
            return Err("invalid_state");
        }
        lease
            .context
            .bind_return(transfer, lease.generation, lease.request, lease.intent)?;
        if let (Some(route), Some(target)) = (&lease.route, transfer.dongle.as_mut()) {
            target.revision = route.revision;
        }
        Ok(())
    }

    pub fn resume(&mut self, ctx: &Ctx, link: &Link) -> Result<(), &'static str> {
        let _actions = ctx.companion_actions.lock().unwrap();
        let lease = self.lease.as_mut().ok_or("superseded")?;
        if !lease.ended
            || !lease.return_allowed
            || !lease.quiet
            || !lease.pause_ready
            || !lease.current(ctx, link)
            || !policy(ctx, link.device, true)
        {
            return Err("superseded");
        }
        let observed = self.route.lock().unwrap().clone();
        let guard =
            ResumeGuard::capture_for(ctx, link.device, observed.as_ref()).ok_or("unavailable")?;
        let authorization = lease.authorization;
        let last_state = lease.last_state;
        let resume_ctx = ctx.clone();
        let route = self.route.clone();
        let address = link.device;
        if !lease.pause.as_mut().ok_or("superseded")?.resume(move || {
            Instant::now().duration_since(last_state) < Duration::from_secs(30)
                && guard.current_route(&resume_ctx, route.lock().unwrap().as_ref())
                && policy(&resume_ctx, address, true)
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
            lease.return_allowed
                && lease.ended
                && !lease.released
                && lease.pause_ready
                && !lease.resuming
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dongle_route() -> (Ctx, MediaRoute) {
        let ctx = Ctx::for_test();
        {
            let mut state = ctx.state.lock().unwrap();
            let d = &mut state.dongle;
            d.present = true;
            d.session = 1;
            d.pair_token = 1;
            d.generation = 1;
            d.capabilities = 16 | 512;
            d.serial = Some("call-test-pico".into());
            d.paired_device_id = Some("010203040506".into());
        }
        let ticket = crate::dongle_routing::selected(&ctx, 0x010203040506)
            .unwrap()
            .unwrap();
        let mut b = [0; 32];
        b[0] = 1;
        b[1] = 32;
        b[2] = 2 | 32 | 64;
        b[4] = 9;
        b[8] = 1;
        b[12..18].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        let status = crate::dongle_routing::Status::decode(&b).unwrap();
        (ctx, (ticket, status, status.received))
    }

    #[test]
    fn a_dongle_call_origin_requires_current_owned_route_for_its_exact_pair() {
        let (ctx, route) = dongle_route();
        // The selected USB pair need not be the daemon's native Bluetooth selection.
        assert_ne!(ctx.mac(), 0x010203040506);
        assert!(ResumeGuard::capture_for(&ctx, 0x010203040506, Some(&route)).is_some());
        let mut stale = route.clone();
        stale.2 = Instant::now() - Duration::from_secs(2);
        assert!(ResumeGuard::capture_for(&ctx, 0x010203040506, Some(&stale)).is_none());
        let mut other = route.clone();
        other.1.peer[0] ^= 1;
        assert!(ResumeGuard::capture_for(&ctx, 0x010203040506, Some(&other)).is_none());
        let mut unowned = route;
        unowned.1.owned = false;
        assert!(ResumeGuard::capture_for(&ctx, 0x010203040506, Some(&unowned)).is_none());
    }

    #[test]
    fn later_manual_routing_retires_the_call_resume_permission() {
        let (ctx, route) = dongle_route();
        let guard = ResumeGuard::capture_for(&ctx, 0x010203040506, Some(&route)).unwrap();
        assert!(guard.current_route(&ctx, Some(&route)));
        let mut manual = route.clone();
        manual.1.revision += 1;
        assert!(!guard.current_route(&ctx, Some(&manual)));
        let lease = CallRoute {
            ticket: route.0.clone(),
            revision: route.1.revision,
            next: None,
        };
        assert!(lease.current(&ctx, Some(&route)));
        assert!(!lease.current(&ctx, Some(&manual)));
        let mut missing = route;
        missing.2 = Instant::now() - Duration::from_secs(2);
        assert!(!lease.current(&ctx, Some(&missing)));
    }

    #[test]
    fn an_already_released_media_source_has_no_pc_return_or_resume_claim() {
        assert_eq!(
            end_plan(true, true, true, true, true, true, false),
            Ok(EndPlan::Finish)
        );
        assert_eq!(
            end_plan(false, true, true, true, true, true, false),
            Err("invalid_state")
        );
        assert_eq!(
            end_plan(true, true, false, true, true, true, false),
            Err("superseded")
        );
        assert_eq!(
            end_plan(true, true, true, true, false, true, false),
            Err("superseded")
        );
    }

    #[test]
    fn a_retired_media_candidate_needs_peer_cancel_and_its_exact_fresh_release_receipt() {
        let (ctx, old) = dongle_route();
        let mut candidate = MediaReleaseCandidate {
            tx: "11".repeat(8),
            address: 0x010203040506,
            generation: 1,
            authorization: 1,
            request: 1,
            intent: 1,
            ticket: old.0.clone(),
            receipt: (10, 77, 1),
            deadline: Instant::now() + Duration::from_secs(5),
            peer_confirmed: false,
        };
        // The control-only personality gets a fresh USB session; physical identity alone is not proof.
        {
            let mut state = ctx.state.lock().unwrap();
            state.dongle.session = 2;
            state.dongle.pair_token = 2;
        }
        let current = crate::dongle_routing::selected(&ctx, 0x010203040506)
            .unwrap()
            .unwrap();
        let mut b = [0; 32];
        b[0] = 1;
        b[1] = 32;
        b[2] = 8;
        b[4] = 10;
        b[8] = 3;
        b[18] = 1;
        b[28] = 77;
        b[12..18].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        let status = crate::dongle_routing::Status::decode(&b).unwrap();
        let route = (current, status, status.received);
        assert!(!candidate.confirms(&ctx, &route));
        candidate.peer_confirmed = true;
        assert!(candidate.confirms(&ctx, &route));
        for (at, value) in [(4, 11), (18, 2), (28, 78)] {
            let mut other = b;
            other[at] = value;
            let status = crate::dongle_routing::Status::decode(&other).unwrap();
            assert!(!candidate.confirms(&ctx, &(route.0.clone(), status, status.received)));
        }
        let mut stale = route.clone();
        stale.2 = Instant::now() - Duration::from_secs(2);
        assert!(!candidate.confirms(&ctx, &stale));
        ctx.state.lock().unwrap().dongle.serial = Some("another-pico".into());
        let other = crate::dongle_routing::selected(&ctx, 0x010203040506)
            .unwrap()
            .unwrap();
        assert!(!candidate.confirms(&ctx, &(other, route.1, route.2)));
        candidate.deadline = Instant::now() - Duration::from_millis(1);
        assert!(!candidate.confirms(&ctx, &route));
    }

    #[test]
    fn a_short_call_before_pause_receipt_waits_without_releasing_or_resuming() {
        assert_eq!(
            end_plan(false, false, true, true, true, true, true),
            Ok(EndPlan::WaitForPause)
        );
        assert_eq!(
            end_plan(false, true, true, true, true, true, true),
            Ok(EndPlan::ResumeCurrent)
        );
    }

    #[test]
    fn release_receipt_allows_fresh_return_even_when_hfp_acquire_never_completed() {
        assert_eq!(
            end_plan(true, true, true, true, true, true, true),
            Ok(EndPlan::ReturnToPc)
        );
        for proof in [
            (false, true, true, true),
            (true, false, true, true),
            (true, true, false, true),
            (true, true, true, false),
        ] {
            assert!(end_plan(true, true, proof.0, proof.1, proof.2, proof.3, true).is_err());
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
            source_released: false,
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
            source_released: false,
        };
        let now = Instant::now();
        let mut transfer = Transfer {
            tx: "0000000000000002".into(),
            target: CompanionTarget::Pc,
            call: None,
            media_gen: None,
            media_source_gen: None,
            dongle: None,
            media_fence: None,
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
