use super::Result;
use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use wasapi::{
    Device, DeviceEnumerator, Direction, EventCallbacks, EventRegistration, SessionState,
};
use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Player,
    GlobalSystemMediaTransportControlsSessionManager as Players,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};
use windows_sys::Win32::{
    Foundation::CloseHandle,
    Storage::Packaging::Appx::GetApplicationUserModelId,
    System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    },
};

const REFRESH: Duration = Duration::from_millis(100);

struct Apartment;
impl Drop for Apartment {
    fn drop(&mut self) {
        wasapi::deinitialize();
    }
}

struct App {
    model: Option<String>,
    image: String,
}

impl App {
    fn for_pid(pid: u32) -> Option<Self> {
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            return None;
        }
        let mut image = [0u16; 32_768];
        let mut len = image.len() as u32;
        let mut model = [0u16; 512];
        let mut model_len = model.len() as u32;
        let (path_ok, model_result) = unsafe {
            let a = QueryFullProcessImageNameW(process, 0, image.as_mut_ptr(), &mut len);
            let b = GetApplicationUserModelId(process, &mut model_len, model.as_mut_ptr());
            CloseHandle(process);
            (a, b)
        };
        (path_ok != 0).then(|| Self {
            image: String::from_utf16_lossy(&image[..len as usize]),
            model: (model_result == 0 && model_len > 1)
                .then(|| String::from_utf16_lossy(&model[..model_len as usize - 1])),
        })
    }

    fn matches(&self, player: &str) -> bool {
        if let Some(model) = &self.model {
            return model.eq_ignore_ascii_case(player);
        }
        if player.is_empty() || player.contains('!') {
            return false;
        }
        if player.contains(['/', '\\']) {
            return self.image.eq_ignore_ascii_case(player);
        }
        let exe = self.image.rsplit(['/', '\\']).next().unwrap_or("");
        exe.eq_ignore_ascii_case(player)
            || exe.get(..exe.len().saturating_sub(4)).is_some_and(|stem| {
                exe.to_ascii_lowercase().ends_with(".exe") && stem.eq_ignore_ascii_case(player)
            })
    }
}

fn playback(player: &Player) -> Option<bool> {
    transport_state(player.GetPlaybackInfo().ok()?.PlaybackStatus().ok()?)
}

fn transport_state(status: Status) -> Option<bool> {
    match status {
        Status::Playing => Some(true),
        Status::Paused | Status::Stopped => Some(false),
        _ => None, // Changing/seeking and indeterminate state do not establish a pause.
    }
}

enum Event {
    Audio(String, bool),
    Media(Player, Option<bool>),
    Stop,
}

struct Session {
    app: Option<App>,
    active: bool,
    had_transport: bool,
    subscription: Option<EventRegistration>,
}

impl Session {
    fn set_active(&mut self, active: bool) {
        if !active {
            self.had_transport = false;
        }
        self.active = active;
    }

    fn playing<'a>(
        &mut self,
        players: impl Iterator<Item = (&'a str, Option<bool>)>,
        elsewhere: &HashSet<String>,
    ) -> Option<bool> {
        if !self.active {
            return Some(false);
        }
        let mut matches = players.filter(|(id, _)| {
            self.app.as_ref().is_some_and(|a| a.matches(id)) && !elsewhere.contains(*id)
        });
        if let Some((_, value)) = matches.next()
            && matches.next().is_none()
        {
            self.had_transport = true;
            return value;
        }
        // Losing a previously matched transport while its stream remains open is not a Play event.
        (!self.had_transport).then_some(true)
    }
}

struct Media {
    id: String,
    session: Player,
    playing: Option<bool>,
    token: Option<i64>,
}

impl Drop for Media {
    fn drop(&mut self) {
        if let Some(token) = self.token {
            let _ = self.session.RemovePlaybackInfoChanged(token);
        }
    }
}

#[derive(Default)]
struct Routes {
    local: HashMap<String, Session>,
    media: Vec<Media>,
    elsewhere: HashSet<String>,
    last_scan: Option<Instant>,
}

impl Routes {
    fn refresh(
        &mut self,
        source: &Device,
        enumerator: &DeviceEnumerator,
        manager: Option<&Players>,
        events: &mpsc::Sender<Event>,
    ) -> Result<()> {
        let list = source
            .get_iaudiosessionmanager()?
            .get_audiosessionenumerator()?;
        let mut live = HashSet::new();
        for index in 0..list.get_count()? {
            let stream = list.get_session(index)?;
            let pid = stream.get_process_id()?;
            if pid == 0 || pid == std::process::id() {
                continue;
            }
            let id = stream.get_session_instance_identifier()?;
            live.insert(id.clone());
            if let Some(route) = self.local.get_mut(&id) {
                if route.subscription.is_none() {
                    route.set_active(stream.get_state()? == SessionState::Active);
                }
            } else {
                let mut callbacks = EventCallbacks::new();
                let events = events.clone();
                let key = id.clone();
                callbacks.set_state_callback(move |state| {
                    let _ = events.send(Event::Audio(key.clone(), state == SessionState::Active));
                });
                let subscription = stream.register_session_notification(callbacks).ok();
                self.local.insert(
                    id,
                    Session {
                        app: App::for_pid(pid),
                        active: stream.get_state()? == SessionState::Active,
                        had_transport: false,
                        subscription,
                    },
                );
            }
        }
        self.local.retain(|key, _| live.contains(key));
        let Some(manager) = manager else {
            return Ok(());
        };
        if self.refresh_media(manager, events).is_err() {
            self.media.clear();
        }
        if !self.media.is_empty()
            && self
                .last_scan
                .is_none_or(|at| at.elapsed() >= Duration::from_secs(2))
        {
            self.check_ambiguity(source, enumerator);
        }
        Ok(())
    }

    fn refresh_media(&mut self, manager: &Players, events: &mpsc::Sender<Event>) -> Result<()> {
        let sessions = manager.GetSessions()?;
        let mut found = Vec::new();
        for index in 0..sessions.Size()? {
            let player = sessions.GetAt(index)?;
            let id = player.SourceAppUserModelId()?.to_string();
            if !self
                .local
                .values()
                .any(|r| r.app.as_ref().is_some_and(|a| a.matches(&id)))
            {
                continue;
            }
            found.push(player.clone());
            if let Some(m) = self.media.iter_mut().find(|m| m.session == player) {
                if m.token.is_none() {
                    m.playing = playback(&player);
                }
            } else {
                self.last_scan = None;
                let sender = events.clone();
                let token = player
                    .PlaybackInfoChanged(&TypedEventHandler::<Player, _>::new(move |s, _| {
                        if let Some(p) = s.as_ref() {
                            let _ = sender.send(Event::Media(p.clone(), playback(p)));
                        }
                        Ok(())
                    }))
                    .ok();
                self.media.push(Media {
                    id,
                    playing: playback(&player),
                    session: player,
                    token,
                });
            }
        }
        self.media.retain(|m| found.contains(&m.session));
        Ok(())
    }

    fn check_ambiguity(&mut self, source: &Device, enumerator: &DeviceEnumerator) {
        self.elsewhere.clear();
        self.last_scan = Some(Instant::now());
        let scan = (|| -> Result<()> {
            let source_id = source.get_id()?;
            let all = enumerator.get_device_collection(Direction::Render)?;
            for i in 0..all.get_nbr_devices()? {
                let dev = all.get_device_at_index(i)?;
                if dev.get_id()? == source_id {
                    continue;
                }
                let sessions = dev
                    .get_iaudiosessionmanager()?
                    .get_audiosessionenumerator()?;
                for i in 0..sessions.get_count()? {
                    let s = sessions.get_session(i)?;
                    if s.get_state()? != SessionState::Active {
                        continue;
                    }
                    let pid = s.get_process_id()?;
                    if pid == 0 || pid == std::process::id() {
                        continue;
                    }
                    let app = App::for_pid(pid);
                    for item in &self.media {
                        if app.as_ref().is_none_or(|a| a.matches(&item.id)) {
                            self.elsewhere.insert(item.id.clone());
                        }
                    }
                }
            }
            Ok(())
        })();
        if scan.is_err() {
            self.elsewhere
                .extend(self.media.iter().map(|m| m.id.clone()));
        }
    }

    fn playing(&mut self) -> Option<bool> {
        aggregate(self.local.values_mut().map(|route| {
            route.playing(
                self.media.iter().map(|m| (m.id.as_str(), m.playing)),
                &self.elsewhere,
            )
        }))
    }
}

fn aggregate(states: impl Iterator<Item = Option<bool>>) -> Option<bool> {
    let mut unknown = false;
    for state in states {
        match state {
            Some(true) => return Some(true),
            None => unknown = true,
            Some(false) => {}
        }
    }
    (!unknown).then_some(false)
}

#[derive(Default)]
struct Edges {
    last: Option<bool>,
    ever_played: bool,
}

impl Edges {
    fn observe(&mut self, playing: Option<bool>, latch: &Latches) {
        if let Some(value) = playing {
            if self.last.replace(value) == Some(false) && value {
                (if self.ever_played {
                    &latch.resume
                } else {
                    &latch.start
                })
                .store(true, Ordering::Release);
            }
            self.ever_played |= value;
        }
    }
}

#[derive(Default)]
struct Latches {
    start: AtomicBool,
    resume: AtomicBool,
}

pub(super) struct Transport {
    events: mpsc::Sender<Event>,
    latch: Arc<Latches>,
    worker: Option<JoinHandle<()>>,
    done: mpsc::Receiver<()>,
}

impl Transport {
    pub(super) fn watch(source: &Device) -> Result<Self> {
        let id = source.get_id()?;
        let (events, inbox) = mpsc::channel();
        let send = events.clone();
        let latches = Arc::new(Latches::default());
        let latch = Arc::clone(&latches);
        let (ready, start) = mpsc::sync_channel(1);
        let (complete, done) = mpsc::channel::<()>();
        let worker = thread::Builder::new()
            .name("source-playback-state".into())
            .spawn(move || {
                let _complete = complete;
                if let Err(error) = wasapi::initialize_mta().ok() {
                    let _ = ready.send(Err(error.to_string()));
                    return;
                }
                let _apartment = Apartment;
                let setup = (|| -> Result<_> {
                    let e = DeviceEnumerator::new()?;
                    let source = e.get_device(&id)?;
                    source.get_iaudiosessionmanager()?;
                    Ok((e, source))
                })();
                let (enumerator, source) = match setup {
                    Ok(pair) => pair,
                    Err(e) => {
                        let _ = ready.send(Err(e.to_string()));
                        return;
                    }
                };
                if ready.send(Ok(())).is_err() {
                    return;
                }
                let mut request = Players::RequestAsync().ok();
                let mut players = None;
                let mut routes = Routes::default();
                let mut edges = Edges::default();
                let mut deadline = Instant::now();
                let mut valid = false;
                loop {
                    if Instant::now() >= deadline {
                        // GetResults is valid only when the Windows async operation has completed.
                        if request
                            .as_ref()
                            .is_some_and(|r| r.Status().is_ok_and(|s| s.0 != 0))
                        {
                            players = request.take().and_then(|r| r.GetResults().ok());
                        }
                        valid = routes
                            .refresh(&source, &enumerator, players.as_ref(), &send)
                            .is_ok();
                        if valid {
                            edges.observe(routes.playing(), &latch);
                        } else {
                            edges.last = None;
                        }
                        deadline = Instant::now() + REFRESH;
                    }
                    match inbox.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                        Ok(Event::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Ok(Event::Audio(id, active)) => {
                            if let Some(s) = routes.local.get_mut(&id) {
                                s.set_active(active);
                            }
                        }
                        Ok(Event::Media(player, playing)) => {
                            if let Some(s) = routes.media.iter_mut().find(|s| s.session == player)
                                && s.playing != playing
                            {
                                s.playing = playing;
                                routes.check_ambiguity(&source, &enumerator);
                            }
                        }
                    }
                    if valid {
                        edges.observe(routes.playing(), &latch);
                    }
                }
                drop(inbox);
            })?;
        let watch = Self {
            events,
            latch: latches,
            worker: Some(worker),
            done,
        };
        start.recv_timeout(Duration::from_secs(3))??;
        Ok(watch)
    }

    pub(super) fn take_resume(&self) -> bool {
        self.latch.resume.swap(false, Ordering::AcqRel)
    }
    pub(super) fn take_start(&self) -> bool {
        self.latch.start.swap(false, Ordering::AcqRel)
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        let _ = self.events.send(Event::Stop);
        // An unresponsive Windows media provider cannot hold the audio thread's shutdown.
        if let Some(worker) = self.worker.take()
            && !matches!(
                self.done.recv_timeout(Duration::from_millis(350)),
                Err(mpsc::RecvTimeoutError::Timeout)
            )
        {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routed(image: &str) -> Session {
        Session {
            app: Some(App {
                model: None,
                image: format!("C:\\Players\\{image}.exe"),
            }),
            active: true,
            had_transport: false,
            subscription: None,
        }
    }

    #[test]
    fn first_play_and_resume_are_distinct_and_an_existing_player_does_not_recenter() {
        let mut edges = Edges::default();
        let latch = Latches::default();
        edges.observe(Some(true), &latch);
        assert!(!latch.start.load(Ordering::Relaxed) && !latch.resume.load(Ordering::Relaxed));
        for state in [None, Some(true), None] {
            edges.observe(state, &latch);
        }
        assert!(!latch.resume.load(Ordering::Relaxed));
        edges.observe(Some(false), &latch);
        edges.observe(None, &latch);
        edges.observe(Some(true), &latch);
        edges.observe(Some(true), &latch);
        assert!(latch.resume.swap(false, Ordering::AcqRel));
        assert!(!latch.resume.load(Ordering::Relaxed));
        let mut edges = Edges::default();
        edges.observe(Some(false), &latch);
        edges.observe(Some(true), &latch);
        assert!(latch.start.load(Ordering::Relaxed) && !latch.resume.load(Ordering::Relaxed));
    }

    #[test]
    fn paused_player_must_belong_to_the_source_and_another_active_stream_wins() {
        let mut app = routed("Player");
        let mut game = routed("Game");
        let elsewhere = HashSet::new();
        let controls = [("Player.exe", Some(false)), ("OtherApp", Some(true))];
        let this = app.playing(controls.iter().copied(), &elsewhere);
        assert_eq!(this, Some(false));
        assert_eq!(
            aggregate([this, game.playing(controls.iter().copied(), &elsewhere)].into_iter()),
            Some(true)
        );
        game.set_active(false);
        assert_eq!(
            aggregate([this, game.playing(controls.iter().copied(), &elsewhere)].into_iter()),
            Some(false)
        );
        assert_eq!(
            app.playing([("Player.exe", Some(true))].into_iter(), &elsewhere),
            Some(true)
        );
        assert_eq!(
            app.playing([("OtherPlayer.exe", Some(false))].into_iter(), &elsewhere),
            None
        );
        let mut fresh = routed("Vlc");
        assert_eq!(
            fresh.playing([("OtherPlayer.exe", Some(false))].into_iter(), &elsewhere),
            Some(true)
        );
        fresh.set_active(false);
        assert_eq!(fresh.playing(std::iter::empty(), &elsewhere), Some(false));
    }

    #[test]
    fn buffering_unmatched_and_multi_output_players_do_not_trigger_false_resume() {
        assert_eq!(transport_state(Status::Changing), None);
        assert_eq!(transport_state(Status::Opened), None);
        let mut route = routed("Player");
        let empty = HashSet::new();
        assert_eq!(
            route.playing([("Player", Some(false))].into_iter(), &empty),
            Some(false)
        );
        let duplicated = ["Player".to_owned()].into_iter().collect();
        assert_eq!(
            route.playing([("Player", Some(true))].into_iter(), &duplicated),
            None
        );
        assert_eq!(route.playing([("Player", None)].into_iter(), &empty), None);
        assert_eq!(
            route.playing(
                [("Player", Some(true)), ("Player", Some(false))].into_iter(),
                &empty
            ),
            None
        );
        assert_eq!(aggregate([None, Some(false)].into_iter()), None);
        assert_eq!(aggregate([None, Some(true)].into_iter()), Some(true));
        assert!(!route.app.as_ref().unwrap().matches("com.example!Player"));
    }
}
