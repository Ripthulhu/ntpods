//! Media events are session-local requests, never permissions recovered from a heartbeat.
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Sample {
    pub generation: u64,
    pub event: u64,
    pub known: bool,
    pub playing: bool,
    pub connected: bool,
    pub available: bool,
}

#[derive(Default)]
pub(crate) struct Observer {
    player: Option<Option<String>>,
    last_known: Option<(Option<String>, Instant)>,
    last_qualified: Option<(Option<String>, u64)>,
    generation: u64,
    event: u64,
}
impl Observer {
    pub fn observe_qualified(
        &mut self,
        intent: Result<Option<String>, ()>,
        connected: bool,
        available: bool,
        qualified: bool,
        transitioning: bool,
    ) -> Sample {
        let mut sample = self.observe(intent, connected, available);
        // Only an already owned, bounded routing job can retain its previously qualified intent.
        let continuing = transitioning
            && self
                .last_qualified
                .as_ref()
                .is_some_and(|(player, generation)| {
                    *generation == sample.generation && self.player.as_ref() == Some(player)
                });
        if qualified {
            self.last_qualified = self
                .player
                .clone()
                .map(|player| (player, sample.generation));
        } else if !transitioning {
            self.last_qualified = None;
        }
        sample.known &= qualified || continuing;
        sample.available &= sample.known;
        sample
    }

    pub fn observe(
        &mut self,
        player: Result<Option<String>, ()>,
        connected: bool,
        available: bool,
    ) -> Sample {
        let known = player.is_ok();
        let next = player.ok();
        let prior = self.player.as_ref().or_else(|| {
            self.last_known
                .as_ref()
                .filter(|(_, at)| at.elapsed() <= Duration::from_secs(2))
                .map(|(player, _)| player)
        });
        if self.generation == 0 || next != self.player {
            if prior.is_some()
                && next.as_ref().is_some_and(Option::is_some)
                && next.as_ref() != prior
            {
                self.event = self.event.wrapping_add(1).max(1);
            }
            self.generation = self.generation.wrapping_add(1).max(1);
        }
        if let Some(player) = &next {
            self.last_known = Some((player.clone(), Instant::now()));
        }
        self.player = next;
        Sample {
            generation: self.generation,
            event: self.event,
            known,
            playing: self.player.as_ref().is_some_and(Option::is_some),
            connected,
            available,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    Pc,
    Phone,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum TraceRoute {
    #[default]
    Waiting,
    Native,
    Ready,
    Busy,
    Stale,
    Unavailable,
    Failed,
}
impl TraceRoute {
    pub fn result(error: Option<&str>) -> Self {
        match error {
            None => Self::Ready,
            Some("busy") => Self::Busy,
            Some("stale") => Self::Stale,
            Some("unavailable") => Self::Unavailable,
            Some(_) => Self::Failed,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Native => "native",
            Self::Ready => "ready",
            Self::Busy => "busy",
            Self::Stale => "stale",
            Self::Unavailable => "unavailable",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct TraceRecord {
    pub capable: bool,
    pub pc: Sample,
    pub phone: Option<Sample>,
    pub phone_fresh: bool,
    pub route: TraceRoute,
    pub route_present: bool,
    pub route_fresh: bool,
    pub probe_pending: bool,
    pub busy: bool,
    pub call: bool,
    pub active: bool,
    pub choice: Option<Target>,
}
impl TraceRecord {
    fn line(self) -> String {
        let phone = self.phone.unwrap_or_default();
        let choice = match self.choice {
            None => "none",
            Some(Target::Pc) => "pc",
            Some(Target::Phone) => "phone",
        };
        format!("companion_media: cap={} pc_gen={} pc_event={} pc_known={} pc_playing={} pc_connected={} pc_available={} phone_sample={} phone_gen={} phone_event={} phone_known={} phone_playing={} phone_connected={} phone_available={} phone_fresh={} route={} route_present={} route_fresh={} probe_pending={} busy={} call={} active={} last_choice={}",
            self.capable, self.pc.generation, self.pc.event, self.pc.known, self.pc.playing, self.pc.connected, self.pc.available,
            self.phone.is_some(), phone.generation, phone.event, phone.known, phone.playing, phone.connected, phone.available,
            self.phone_fresh, self.route.name(), self.route_present, self.route_fresh, self.probe_pending, self.busy, self.call, self.active, choice)
    }
}

pub(crate) struct Trace {
    enabled: bool,
    count: u8,
    next: Option<Instant>,
    last: Option<TraceRecord>,
}
impl Trace {
    pub fn new(flag: Option<&str>) -> Self {
        Self {
            enabled: flag == Some("1"),
            count: 0,
            next: None,
            last: None,
        }
    }
    pub fn record(&mut self, now: Instant, sample: impl FnOnce() -> TraceRecord) -> Option<String> {
        if !self.enabled || self.count >= 64 || self.next.is_some_and(|next| now < next) {
            return None;
        }
        self.next = Some(now + Duration::from_secs(1));
        let sample = sample();
        if self.last == Some(sample) {
            return None;
        }
        self.last = Some(sample);
        self.count += 1;
        Some(sample.line())
    }
}

#[derive(Clone, Copy)]
struct PendingPlay {
    target: Target,
    event: u64,
    deadline: Instant,
}

pub(crate) fn routing_current(
    expected: (u32, Instant),
    observed: (u32, Instant),
    pending: Option<(u32, u32, u8)>,
    receipt: Option<(u32, u32, u8)>,
) -> bool {
    observed.1 < expected.1 || observed.0 == expected.0 || pending.is_some() && pending == receipt
}

pub(crate) fn current(
    target: Target,
    destination: u64,
    source: u64,
    pc: Sample,
    phone: Sample,
) -> bool {
    pc.known
        && phone.known
        && match target {
            Target::Phone => {
                phone.generation == destination
                    && phone.playing
                    && pc.generation == source
                    && (!pc.playing || !pc.connected)
            }
            Target::Pc => {
                pc.generation == destination
                    && pc.playing
                    && phone.generation == source
                    && (!phone.playing || !phone.connected)
            }
        }
}
#[derive(Default)]
pub(crate) struct Policy {
    pc_event: Option<u64>,
    phone_event: Option<u64>,
    pc_lost: Option<Instant>,
    phone_lost: Option<Instant>,
    pc_seen: bool,
    phone_seen: bool,
    pc_departure_used: bool,
    phone_departure_used: bool,
    pending: Option<PendingPlay>,
}
impl Policy {
    pub fn suspend(&mut self) {
        *self = Self::default();
    }

    pub fn observe(&mut self, pc: Sample, phone: Sample, now: Instant) -> Option<Target> {
        let pc_new = self
            .pc_event
            .replace(pc.event)
            .is_some_and(|old| old != pc.event);
        let phone_new = self
            .phone_event
            .replace(phone.event)
            .is_some_and(|old| old != phone.event);
        let pc_quiet = quiet(pc, &mut self.pc_lost, now);
        let phone_quiet = quiet(phone, &mut self.phone_lost, now);
        if pc.known && pc.connected {
            self.pc_seen = true;
            self.pc_departure_used = false;
        }
        if phone.known && phone.connected {
            self.phone_seen = true;
            self.phone_departure_used = false;
        }
        let pc_departure = self.pc_seen
            && !self.pc_departure_used
            && pc.known
            && !pc.connected
            && self
                .pc_lost
                .is_some_and(|at| now.duration_since(at) >= Duration::from_secs(3));
        let phone_departure = self.phone_seen
            && !self.phone_departure_used
            && phone.known
            && !phone.connected
            && self
                .phone_lost
                .is_some_and(|at| now.duration_since(at) >= Duration::from_secs(3));
        self.pc_departure_used |= pc_departure;
        self.phone_departure_used |= phone_departure;
        if pc_new && phone_new {
            self.pending = None;
            return None;
        }
        if pc_new || phone_new {
            self.pending = if pc_new && pc.event != 0 && pc.playing {
                Some(PendingPlay {
                    target: Target::Pc,
                    event: pc.event,
                    deadline: now + Duration::from_secs(2),
                })
            } else if phone_new && phone.event != 0 && phone.playing {
                Some(PendingPlay {
                    target: Target::Phone,
                    event: phone.event,
                    deadline: now + Duration::from_secs(2),
                })
            } else {
                None
            };
        }
        if let Some(pending) = self.pending {
            let (destination, source_quiet) = match pending.target {
                Target::Pc => (pc, phone_quiet),
                Target::Phone => (phone, pc_quiet),
            };
            if now >= pending.deadline || destination.event != pending.event || !destination.playing
            {
                self.pending = None;
            } else {
                if destination.known && destination.available && source_quiet {
                    self.pending = None;
                    return Some(pending.target);
                }
                return None;
            }
        }
        if pc_departure
            && phone.event != 0
            && phone.known
            && phone.playing
            && phone.available
            && pc_quiet
        {
            Some(Target::Phone)
        } else if phone_departure
            && pc.event != 0
            && pc.known
            && pc.playing
            && pc.available
            && phone_quiet
        {
            Some(Target::Pc)
        } else {
            None
        }
    }
}
fn quiet(sample: Sample, lost: &mut Option<Instant>, now: Instant) -> bool {
    if sample.connected || !sample.known {
        *lost = None;
    } else {
        lost.get_or_insert(now);
    }
    sample.known
        && (!sample.playing
            || lost.is_some_and(|at| now.duration_since(at) >= Duration::from_secs(3)))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn trace_record() -> TraceRecord {
        TraceRecord {
            capable: true,
            pc: Sample::default(),
            phone: None,
            phone_fresh: false,
            route: TraceRoute::Waiting,
            route_present: false,
            route_fresh: false,
            probe_pending: false,
            busy: false,
            call: false,
            active: false,
            choice: None,
        }
    }
    #[test]
    fn media_trace_is_explicitly_enabled_rate_limited_changed_only_and_bounded() {
        let now = Instant::now();
        for flag in [None, Some(""), Some("true"), Some(" 1"), Some("0")] {
            assert!(Trace::new(flag)
                .record(now, || panic!("Disabled tracing must not collect metadata"))
                .is_none());
        }
        let mut trace = Trace::new(Some("1"));
        assert!(trace.record(now, trace_record).is_some());
        assert!(trace
            .record(now, || panic!("Sampling must be rate limited"))
            .is_none());
        assert!(trace
            .record(now + Duration::from_secs(1), trace_record)
            .is_none());
        for second in 2..65 {
            assert!(trace
                .record(now + Duration::from_secs(second), || TraceRecord {
                    pc: Sample {
                        event: second,
                        ..Sample::default()
                    },
                    ..trace_record()
                })
                .is_some());
        }
        assert!(trace
            .record(now + Duration::from_secs(65), || panic!(
                "Session budget must stop metadata collection"
            ))
            .is_none());
    }
    #[test]
    fn media_trace_has_only_fixed_metadata_fields_and_sanitized_route_results() {
        assert_eq!(
            TraceRoute::result(Some("private player title or MAC")).name(),
            "failed"
        );
        assert_eq!(trace_record().line(), "companion_media: cap=true pc_gen=0 pc_event=0 pc_known=false pc_playing=false pc_connected=false pc_available=false phone_sample=false phone_gen=0 phone_event=0 phone_known=false phone_playing=false phone_connected=false phone_available=false phone_fresh=false route=waiting route_present=false route_fresh=false probe_pending=false busy=false call=false active=false last_choice=none");
    }
    fn s(event: u64, playing: bool, connected: bool) -> Sample {
        Sample {
            event,
            generation: event + 1,
            known: true,
            playing,
            connected,
            available: true,
        }
    }

    #[test]
    fn authoritative_pause_survives_long_unqualified_render_activity_and_creates_a_new_play() {
        let mut observer = Observer::default();
        let mut policy = Policy::default();
        let now = Instant::now();
        let phone = s(0, false, true);
        let initial =
            observer.observe_qualified(Ok(Some("Chrome".into())), false, true, true, false);
        assert_eq!(policy.observe(initial, phone, now), None);
        for second in 1..=5 {
            let paused = observer.observe_qualified(Ok(None), false, true, false, false);
            assert!(
                !paused.known,
                "stale render activity does not prove the source quiet"
            );
            assert!(!paused.playing);
            assert_eq!(
                policy.observe(paused, phone, now + Duration::from_secs(second)),
                None
            );
        }
        observer.last_known.as_mut().unwrap().1 -= Duration::from_secs(5);
        let resumed =
            observer.observe_qualified(Ok(Some("Chrome".into())), false, true, true, false);
        assert_eq!(resumed.event, 1);
        assert_eq!(
            policy.observe(resumed, phone, now + Duration::from_secs(6)),
            Some(Target::Pc)
        );
        assert_eq!(
            policy.observe(resumed, phone, now + Duration::from_secs(7)),
            None
        );
    }

    #[test]
    fn speaker_play_requires_fresh_selected_output_proof_within_the_original_play_window() {
        for (delay, expected) in [(1500, Some(Target::Pc)), (2500, None)] {
            let mut observer = Observer::default();
            let mut policy = Policy::default();
            let now = Instant::now();
            let phone = s(0, false, true);
            let paused = observer.observe_qualified(Ok(None), false, true, false, false);
            assert_eq!(policy.observe(paused, phone, now), None);
            let speakers =
                observer.observe_qualified(Ok(Some("Chrome".into())), false, true, false, false);
            assert_eq!(speakers.event, 1);
            assert!(!speakers.known && !speakers.available);
            assert_eq!(
                policy.observe(speakers, phone, now + Duration::from_secs(1)),
                None
            );
            let selected =
                observer.observe_qualified(Ok(Some("Chrome".into())), false, true, true, false);
            assert_eq!(
                policy.observe(
                    selected,
                    phone,
                    now + Duration::from_secs(1) + Duration::from_millis(delay)
                ),
                expected
            );
            assert_eq!(
                policy.observe(selected, phone, now + Duration::from_secs(5)),
                None
            );
        }
    }

    #[test]
    fn notification_render_and_later_route_recovery_never_invent_a_media_start() {
        let mut observer = Observer::default();
        let mut policy = Policy::default();
        let now = Instant::now();
        let phone = s(0, false, true);
        for (i, qualified) in [true, false, false, true].into_iter().enumerate() {
            let notification = observer.observe_qualified(Ok(None), false, true, qualified, false);
            assert_eq!(notification.event, 0);
            assert!(!notification.playing);
            assert_eq!(
                policy.observe(notification, phone, now + Duration::from_secs(i as u64)),
                None
            );
        }
        let mut reconnected = Observer::default();
        let already_playing =
            reconnected.observe_qualified(Ok(Some("Chrome".into())), false, true, true, false);
        assert_eq!(already_playing.event, 0);
    }

    #[test]
    fn own_transition_only_retains_the_same_previously_qualified_player_and_generation() {
        for next in [Ok(Some("Other".into())), Ok(None), Err(())] {
            let mut observer = Observer::default();
            observer.observe_qualified(Ok(Some("Chrome".into())), true, true, true, false);
            let same =
                observer.observe_qualified(Ok(Some("Chrome".into())), true, false, false, true);
            assert!(same.known && same.playing);
            assert!(!same.available);
            let changed = observer.observe_qualified(next, true, false, false, true);
            assert!(!changed.known && !changed.available);
        }
        let mut observer = Observer::default();
        observer.observe_qualified(Ok(None), true, true, true, false);
        assert!(
            observer
                .observe_qualified(Ok(None), true, false, false, true)
                .known
        );
        observer.observe_qualified(Ok(None), true, true, false, false);
        assert!(
            !observer
                .observe_qualified(Ok(None), true, false, false, true)
                .known
        );
    }
    #[test]
    fn reconnect_and_notifications_never_create_a_media_request() {
        let mut observer = Observer::default();
        assert_eq!(
            observer
                .observe(Ok(Some("player".into())), true, true)
                .event,
            0
        );
        assert_eq!(
            observer
                .observe(Ok(Some("player".into())), false, true)
                .event,
            0
        );
        observer.observe(Ok(None), true, true);
        assert_eq!(
            observer
                .observe(Ok(Some("player".into())), true, true)
                .event,
            1
        );
        let mut p = Policy::default();
        let now = Instant::now();
        assert_eq!(p.observe(s(9, true, true), s(3, true, true), now), None);
        assert_eq!(p.observe(s(9, true, true), s(4, true, true), now), None);
        assert_eq!(
            p.observe(s(9, false, true), s(4, true, true), now),
            Some(Target::Phone)
        );
        assert_eq!(p.observe(s(9, false, true), s(4, true, true), now), None);
        assert_eq!(
            p.observe(s(9, false, true), s(5, true, true), now),
            Some(Target::Phone)
        );
    }

    #[test]
    fn a_play_edge_waits_briefly_for_actual_rendering_without_replaying_old_playback() {
        let mut observer = Observer::default();
        observer.observe(Ok(None), false, true);
        observer.observe(Err(()), false, false);
        assert_eq!(
            observer
                .observe(Ok(Some("player".into())), false, true)
                .event,
            1
        );
        observer.observe(Err(()), false, false);
        assert_eq!(
            observer
                .observe(Ok(Some("player".into())), false, true)
                .event,
            1
        );
        observer.observe(Ok(None), false, true);
        observer.observe(Err(()), false, false);
        observer.last_known.as_mut().unwrap().1 -= Duration::from_secs(3);
        assert_eq!(
            observer
                .observe(Ok(Some("player".into())), false, true)
                .event,
            1
        );
    }
    #[test]
    fn loss_requires_actual_transport_evidence_and_grace_not_player_pause() {
        let now = Instant::now();
        let mut p = Policy::default();
        p.observe(s(1, true, true), s(1, false, true), now);
        assert_eq!(p.observe(s(1, true, false), s(2, true, true), now), None);
        assert_eq!(
            p.observe(
                s(1, true, false),
                s(3, true, true),
                now + Duration::from_secs(2)
            ),
            None
        );
        assert_eq!(
            p.observe(
                s(1, true, false),
                s(4, true, true),
                now + Duration::from_secs(3)
            ),
            Some(Target::Phone)
        );
        assert_eq!(
            p.observe(
                Sample {
                    known: false,
                    ..s(1, true, false)
                },
                s(5, true, true),
                now + Duration::from_secs(4)
            ),
            None
        );
    }
    #[test]
    fn simultaneous_starts_and_unclassified_targets_do_not_take_over() {
        let now = Instant::now();
        let mut p = Policy::default();
        p.observe(s(0, false, true), s(0, false, true), now);
        assert_eq!(p.observe(s(1, true, true), s(1, true, true), now), None);
        assert_eq!(
            p.observe(
                s(1, false, true),
                Sample {
                    available: false,
                    ..s(2, true, true)
                },
                now
            ),
            None
        );
    }

    #[test]
    fn fresh_play_waits_for_the_incumbents_pause_proof_once() {
        let now = Instant::now();
        for target in [Target::Pc, Target::Phone] {
            let mut policy = Policy::default();
            let observe = |policy: &mut Policy, destination, source, at| match target {
                Target::Pc => policy.observe(destination, source, at),
                Target::Phone => policy.observe(source, destination, at),
            };
            assert_eq!(
                observe(&mut policy, s(0, false, false), s(0, true, true), now),
                None
            );
            assert_eq!(
                observe(&mut policy, s(1, true, false), s(0, true, true), now),
                None
            );
            assert_eq!(
                observe(
                    &mut policy,
                    s(1, true, false),
                    s(0, false, true),
                    now + Duration::from_millis(500)
                ),
                Some(target)
            );
            assert_eq!(
                observe(
                    &mut policy,
                    s(1, true, false),
                    s(0, false, true),
                    now + Duration::from_millis(600)
                ),
                None
            );
        }
    }

    #[test]
    fn fresh_play_waits_for_route_knowledge_and_destination_availability() {
        let now = Instant::now();
        let mut policy = Policy::default();
        policy.observe(s(0, false, false), s(0, false, true), now);
        let waiting = Sample {
            known: false,
            available: false,
            ..s(1, true, false)
        };
        assert_eq!(policy.observe(waiting, s(0, false, true), now), None);
        assert_eq!(
            policy.observe(
                Sample {
                    known: true,
                    ..waiting
                },
                s(0, false, true),
                now + Duration::from_millis(500)
            ),
            None
        );
        assert_eq!(
            policy.observe(
                s(1, true, false),
                s(0, false, true),
                now + Duration::from_secs(1)
            ),
            Some(Target::Pc)
        );
    }

    #[test]
    fn heartbeats_cannot_extend_a_play_edge_into_a_later_pause() {
        let now = Instant::now();
        let mut policy = Policy::default();
        policy.observe(s(0, false, false), s(0, true, true), now);
        assert_eq!(
            policy.observe(s(1, true, false), s(0, true, true), now),
            None
        );
        for delay in [500, 1_000, 1_999] {
            assert_eq!(
                policy.observe(
                    s(1, true, false),
                    s(0, true, true),
                    now + Duration::from_millis(delay)
                ),
                None
            );
        }
        for delay in [2, 300] {
            assert_eq!(
                policy.observe(
                    s(1, true, false),
                    s(0, false, true),
                    now + Duration::from_secs(delay)
                ),
                None
            );
        }
    }

    #[test]
    fn stopping_playback_revokes_the_waiting_edge_and_only_a_new_start_can_retry() {
        let now = Instant::now();
        let mut policy = Policy::default();
        policy.observe(s(0, false, false), s(0, true, true), now);
        policy.observe(s(1, true, false), s(0, true, true), now);
        assert_eq!(
            policy.observe(
                s(1, false, false),
                s(0, false, true),
                now + Duration::from_millis(100)
            ),
            None
        );
        assert_eq!(
            policy.observe(
                s(1, true, false),
                s(0, false, true),
                now + Duration::from_millis(200)
            ),
            None
        );
        assert_eq!(
            policy.observe(
                s(2, true, false),
                s(0, false, true),
                now + Duration::from_millis(300)
            ),
            Some(Target::Pc)
        );
    }

    #[test]
    fn a_newer_opposite_play_replaces_the_waiting_destination() {
        let now = Instant::now();
        let mut policy = Policy::default();
        policy.observe(s(0, false, true), s(0, true, true), now);
        policy.observe(s(1, true, true), s(0, true, true), now);
        assert_eq!(
            policy.observe(
                s(1, true, true),
                s(1, true, true),
                now + Duration::from_millis(100)
            ),
            None
        );
        assert_eq!(
            policy.observe(
                s(1, false, true),
                s(1, true, true),
                now + Duration::from_millis(200)
            ),
            Some(Target::Phone)
        );
        assert_eq!(
            policy.observe(
                s(1, true, true),
                s(1, false, true),
                now + Duration::from_millis(300)
            ),
            None
        );
    }

    #[test]
    fn simultaneous_new_starts_discard_the_previous_waiting_edge() {
        let now = Instant::now();
        let mut policy = Policy::default();
        policy.observe(s(0, false, true), s(0, true, true), now);
        policy.observe(s(1, true, true), s(0, true, true), now);
        assert_eq!(
            policy.observe(
                s(2, true, true),
                s(1, true, true),
                now + Duration::from_millis(100)
            ),
            None
        );
        assert_eq!(
            policy.observe(
                s(2, true, true),
                s(1, false, true),
                now + Duration::from_millis(200)
            ),
            None
        );
    }

    #[test]
    fn blocked_calls_transfers_and_manual_epochs_require_a_new_start_after_baselining() {
        let now = Instant::now();
        for owner in ["call", "transfer", "manual"] {
            let mut policy = Policy::default();
            policy.observe(s(0, false, false), s(0, true, true), now);
            policy.observe(s(1, true, false), s(0, true, true), now);
            if owner == "manual" {
                policy = Policy::default();
            } else {
                policy.suspend();
            }
            assert_eq!(
                policy.observe(
                    s(1, true, false),
                    s(0, false, false),
                    now + Duration::from_millis(100)
                ),
                None,
                "{owner}"
            );
            assert_eq!(
                policy.observe(
                    s(1, true, false),
                    s(0, false, false),
                    now + Duration::from_secs(300)
                ),
                None,
                "{owner}"
            );
            policy.observe(
                s(1, false, false),
                s(0, false, false),
                now + Duration::from_secs(301),
            );
            assert_eq!(
                policy.observe(
                    s(2, true, false),
                    s(0, false, false),
                    now + Duration::from_secs(302)
                ),
                Some(Target::Pc),
                "{owner}"
            );
        }
    }

    #[test]
    fn new_source_play_after_release_revokes_the_old_acquisition() {
        let pc = s(1, false, true);
        let phone = s(2, true, true);
        assert!(current(
            Target::Phone,
            phone.generation,
            pc.generation,
            pc,
            phone
        ));
        assert!(current(
            Target::Phone,
            phone.generation,
            pc.generation,
            Sample {
                connected: false,
                ..pc
            },
            phone
        ));
        assert!(!current(
            Target::Phone,
            phone.generation,
            pc.generation,
            s(2, true, false),
            phone
        ));
        assert!(!current(
            Target::Pc,
            phone.generation,
            pc.generation,
            phone,
            s(2, true, false)
        ));
    }

    #[test]
    fn verified_departure_can_transfer_an_observed_play_once_after_grace() {
        let now = Instant::now();
        let mut policy = Policy::default();
        policy.observe(s(0, true, true), s(0, false, true), now);
        assert_eq!(
            policy.observe(s(0, true, true), s(1, true, true), now),
            None
        );
        assert_eq!(
            policy.observe(s(0, true, false), s(1, true, true), now),
            None
        );
        assert_eq!(
            policy.observe(
                s(0, true, false),
                s(1, true, true),
                now + Duration::from_secs(3)
            ),
            Some(Target::Phone)
        );
        assert_eq!(
            policy.observe(
                s(0, true, false),
                s(1, true, true),
                now + Duration::from_secs(4)
            ),
            None
        );
        let mut restarted = Policy::default();
        assert_eq!(
            restarted.observe(s(0, true, false), s(1, true, true), now),
            None
        );
        assert_eq!(
            restarted.observe(
                s(0, true, false),
                s(1, true, true),
                now + Duration::from_secs(3)
            ),
            None
        );
    }

    #[test]
    fn old_route_cache_cannot_undo_receipts_but_new_manual_revision_revokes() {
        let now = Instant::now();
        assert!(routing_current(
            (6, now),
            (5, now - Duration::from_millis(20)),
            None,
            None
        ));
        assert!(!routing_current(
            (6, now),
            (7, now + Duration::from_millis(20)),
            None,
            None
        ));
        assert!(routing_current(
            (5, now),
            (6, now),
            Some((6, 9, 1)),
            Some((6, 9, 1))
        ));
        assert!(!routing_current(
            (5, now),
            (6, now),
            Some((6, 9, 1)),
            Some((6, 10, 1))
        ));
        assert!(!routing_current((5, now), (6, now), Some((6, 9, 1)), None));
    }
}
