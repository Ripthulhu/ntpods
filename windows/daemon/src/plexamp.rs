//! Local Plex Companion control; never send a global Play/Pause toggle.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::core::{implement, Interface, GUID, HSTRING, PCWSTR};
use windows::Data::Xml::Dom::{XmlDocument, XmlElement, XmlLoadSettings};
use windows::Win32::Foundation::BOOL;
use windows::Win32::Media::Audio::{
    AudioSessionDisconnectReason, AudioSessionState, AudioSessionStateActive,
    AudioSessionStateExpired, AudioSessionStateInactive, IAudioSessionControl2,
    IAudioSessionEvents, IAudioSessionEvents_Impl, IAudioSessionManager2,
};
use windows::Win32::System::Com::CLSCTX_ALL;
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};

const MAX_RESPONSE: usize = 32 * 1024;
static COMMAND_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, PartialEq, Eq)]
struct Timeline {
    state: String,
    key: String,
    item: String,
    queue: String,
    version: String,
    time: String,
}

impl Timeline {
    fn same_item(&self, other: &Self) -> bool {
        self.key == other.key
            && self.item == other.item
            && self.queue == other.queue
            && self.version == other.version
    }

    fn may_resume(&self, current: &Self) -> bool {
        current.state == "paused" && self.same_item(current)
    }
}

pub struct Paused {
    audio: IAudioSessionControl2,
    observer: IAudioSessionEvents,
    claim: Arc<RenderPauseClaim>,
    player: String,
    timeline: Timeline,
}

#[derive(Default)]
struct RenderPauseClaim(AtomicU8);

impl RenderPauseClaim {
    fn observe(&self, state: AudioSessionState) {
        let _ = self
            .0
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |previous| {
                Some(
                    if previous == 2
                        || state == AudioSessionStateExpired
                        || (state == AudioSessionStateActive && previous == 1)
                    {
                        2
                    } else if state == AudioSessionStateInactive {
                        1
                    } else if state == AudioSessionStateActive {
                        previous
                    } else {
                        2
                    },
                )
            });
    }

    fn may_resume(&self) -> bool {
        self.0.load(Ordering::Acquire) != 2
    }
}

#[implement(IAudioSessionEvents)]
struct RenderObserver(Arc<RenderPauseClaim>);

#[allow(non_snake_case)]
impl IAudioSessionEvents_Impl for RenderObserver_Impl {
    fn OnDisplayNameChanged(&self, _: &PCWSTR, _: *const GUID) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnIconPathChanged(&self, _: &PCWSTR, _: *const GUID) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnSimpleVolumeChanged(&self, _: f32, _: BOOL, _: *const GUID) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnChannelVolumeChanged(
        &self,
        _: u32,
        _: *const f32,
        _: u32,
        _: *const GUID,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnGroupingParamChanged(&self, _: *const GUID, _: *const GUID) -> windows::core::Result<()> {
        Ok(())
    }
    fn OnStateChanged(&self, state: AudioSessionState) -> windows::core::Result<()> {
        self.0.observe(state);
        Ok(())
    }
    fn OnSessionDisconnected(&self, _: AudioSessionDisconnectReason) -> windows::core::Result<()> {
        self.0.observe(AudioSessionStateExpired);
        Ok(())
    }
}

impl Drop for Paused {
    fn drop(&mut self) {
        let _ = unsafe {
            self.audio
                .UnregisterAudioSessionNotification(&self.observer)
        };
    }
}

fn plexamp_audio(address: Option<u64>) -> Option<IAudioSessionControl2> {
    unsafe {
        let device = crate::volume::output_device_for(address).ok()?;
        let manager = device
            .Activate::<IAudioSessionManager2>(CLSCTX_ALL, None)
            .ok()?;
        let sessions = manager.GetSessionEnumerator().ok()?;
        for i in 0..sessions.GetCount().ok()? {
            let session = sessions
                .GetSession(i)
                .ok()?
                .cast::<IAudioSessionControl2>()
                .ok()?;
            if session.GetState().ok()? != AudioSessionStateActive {
                continue;
            }
            let process = OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION,
                0,
                session.GetProcessId().ok()?,
            );
            if process.is_null() {
                continue;
            }
            let mut path = [0u16; 32768];
            let mut length = path.len() as u32;
            let read = QueryFullProcessImageNameW(process, 0, path.as_mut_ptr(), &mut length);
            CloseHandle(process);
            if read != 0
                && String::from_utf16_lossy(&path[..length as usize])
                    .rsplit('\\')
                    .next()
                    .is_some_and(|name| name.eq_ignore_ascii_case("Plexamp.exe"))
            {
                return Some(session);
            }
        }
    }
    None
}

fn request(path: &str, target: Option<&str>) -> Option<String> {
    let started = Instant::now();
    let mut stream = TcpStream::connect_timeout(
        &SocketAddr::from(([127, 0, 0, 1], 32500)),
        Duration::from_millis(100),
    )
    .ok()?;
    stream
        .set_read_timeout(Some(Duration::from_millis(100)))
        .ok()?;
    stream
        .set_write_timeout(Some(Duration::from_millis(100)))
        .ok()?;
    let target_header = match target {
        Some(id) if valid_player_id(id) => format!("X-Plex-Target-Client-Identifier: {id}\r\n"),
        Some(_) => return None,
        None => String::new(),
    };
    let command = COMMAND_ID.fetch_add(1, Ordering::Relaxed);
    let path = if target.is_some() {
        format!("{path}&commandID={command}")
    } else {
        path.into()
    };
    let pid = std::process::id();
    write!(stream, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:32500\r\nX-Plex-Client-Identifier: ntpods-ear-detection-{pid}\r\nX-Plex-Product: NTPods\r\n{target_header}Connection: close\r\n\r\n").ok()?;
    let mut bytes = Vec::new();
    loop {
        if bytes.len() > MAX_RESPONSE || started.elapsed() > Duration::from_millis(300) {
            return None;
        }
        if let Some(body) = response_body(&bytes) {
            return Some(body.to_owned());
        }
        let mut buffer = [0; 4096];
        let read = stream.read(&mut buffer).ok()?;
        if read == 0 {
            return None;
        }
        bytes.extend_from_slice(&buffer[..read]);
    }
}

fn response_body(bytes: &[u8]) -> Option<&str> {
    if bytes.len() > MAX_RESPONSE {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let (headers, body) = text.split_once("\r\n\r\n")?;
    let mut lines = headers.lines();
    let mut status = lines.next()?.split_whitespace();
    if !matches!(status.next()?, "HTTP/1.0" | "HTTP/1.1") || status.next()? != "200" {
        return None;
    }
    let mut length = None;
    for line in lines {
        let (name, value) = line.split_once(':')?;
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return None;
        }
        if name.eq_ignore_ascii_case("content-length") {
            if length.is_some() {
                return None;
            }
            length = Some(value.trim().parse::<usize>().ok()?);
        }
    }
    (body.len() == length?).then_some(body)
}

fn valid_player_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

fn elements(xml: &str, path: &str) -> Option<Vec<XmlElement>> {
    let settings = XmlLoadSettings::new().ok()?;
    settings.SetProhibitDtd(true).ok()?;
    settings.SetResolveExternals(false).ok()?;
    settings.SetMaxElementDepth(8).ok()?;
    let doc = XmlDocument::new().ok()?;
    doc.LoadXmlWithSettings(&HSTRING::from(xml), &settings)
        .ok()?;
    let nodes = doc.SelectNodes(&HSTRING::from(path)).ok()?;
    (0..nodes.Length().ok()?)
        .map(|i| nodes.Item(i).ok()?.cast().ok())
        .collect()
}

fn attribute(element: &XmlElement, name: &str) -> Option<String> {
    let value = element.GetAttribute(&HSTRING::from(name)).ok()?.to_string();
    (!value.is_empty()).then_some(value)
}

fn player(xml: &str) -> Option<String> {
    let players = elements(xml, "/MediaContainer/Player")?;
    if players.len() != 1 || attribute(&players[0], "product")? != "Plexamp" {
        return None;
    }
    let id = attribute(&players[0], "machineIdentifier")?;
    valid_player_id(&id).then_some(id)
}

fn timeline(xml: &str) -> Option<Timeline> {
    let rows = elements(xml, "/MediaContainer/Timeline[@type='music']")?;
    if rows.len() != 1 {
        return None;
    }
    let row = &rows[0];
    Some(Timeline {
        state: attribute(row, "state")?,
        key: attribute(row, "key")?,
        item: attribute(row, "playQueueItemID")?,
        queue: attribute(row, "playQueueID")?,
        version: attribute(row, "playQueueVersion")?,
        time: attribute(row, "time")?,
    })
}

pub fn pause(address: Option<u64>) -> Option<Paused> {
    let audio = plexamp_audio(address)?;
    let player = player(&request("/resources", None)?)?;
    let before = timeline(&request("/player/timeline/poll?wait=0", Some(&player))?)?;
    if before.state != "playing" {
        return None;
    }
    let claim = Arc::new(RenderPauseClaim::default());
    let observer: IAudioSessionEvents = RenderObserver(claim.clone()).into();
    unsafe {
        audio.RegisterAudioSessionNotification(&observer).ok()?;
    }
    let mut paused = Paused {
        audio,
        observer,
        claim,
        player,
        timeline: before,
    };
    request("/player/playback/pause?type=music", Some(&paused.player))?;
    let after = timeline(&request(
        "/player/timeline/poll?wait=0",
        Some(&paused.player),
    )?)?;
    if after.state != "paused" || !paused.timeline.same_item(&after) {
        return None;
    }
    paused.timeline = after;
    // The HTTP position can advance after Pause; actual render events track user playback.
    paused
        .claim
        .observe(unsafe { paused.audio.GetState().ok()? });
    Some(paused)
}

impl Paused {
    pub fn resume(self) -> bool {
        (|| -> Option<bool> {
            if !self.claim.may_resume() {
                crate::log("media: Plexamp resume refused: intervening playback or session change");
                return None;
            }
            if unsafe { self.audio.GetState().ok()? } == AudioSessionStateExpired {
                crate::log("media: Plexamp resume refused: audio session expired");
                return None;
            }
            if crate::media::is_playing() {
                crate::log("media: Plexamp resume refused: another media session is playing");
                return None;
            }
            if player(&request("/resources", None)?)? != self.player {
                crate::log("media: Plexamp resume refused: player changed");
                return None;
            }
            let current = timeline(&request(
                "/player/timeline/poll?wait=0",
                Some(&self.player),
            )?)?;
            if !self.timeline.may_resume(&current) {
                crate::log("media: Plexamp resume refused: track or playback state changed");
                return None;
            }
            request("/player/playback/play?type=music", Some(&self.player))?;
            Some(true)
        })()
        .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(state: &str, time: &str) -> Timeline {
        Timeline {
            state: state.into(),
            key: "/library/metadata/1".into(),
            item: "2".into(),
            queue: "3".into(),
            version: "1".into(),
            time: time.into(),
        }
    }

    #[test]
    fn resume_requires_the_same_paused_track_but_allows_reported_position_drift() {
        let paused = track("paused", "100");
        assert!(paused.may_resume(&track("paused", "100")));
        assert!(paused.may_resume(&track("paused", "479")));
        for field in ["state", "key", "item", "queue", "version"] {
            let mut current = track("paused", "100");
            match field {
                "state" => current.state = "playing".into(),
                "key" => current.key = "other".into(),
                "item" => current.item = "other".into(),
                "queue" => current.queue = "other".into(),
                _ => current.version = "2".into(),
            }
            assert!(!paused.may_resume(&current), "{field}");
        }
    }

    #[test]
    fn render_events_revoke_resume_after_manual_playback_even_if_paused_again() {
        let claim = RenderPauseClaim::default();
        claim.observe(AudioSessionStateActive);
        assert!(claim.may_resume());
        claim.observe(AudioSessionStateInactive);
        claim.observe(AudioSessionStateInactive);
        assert!(claim.may_resume());
        claim.observe(AudioSessionStateActive);
        claim.observe(AudioSessionStateInactive);
        assert!(!claim.may_resume());
        let expired = RenderPauseClaim::default();
        expired.observe(AudioSessionStateExpired);
        assert!(!expired.may_resume());
    }

    #[test]
    fn http_rejects_truncated_oversized_redirected_or_ambiguous_responses() {
        assert_eq!(
            response_body(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok"),
            Some("ok")
        );
        for response in [
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\no",
            "HTTP/1.1 302 Found\r\nContent-Length: 0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nContent-Length: 0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n",
            "HTTP/1.1 200 OK\r\n\r\n",
        ] {
            assert!(response_body(response.as_bytes()).is_none());
        }
        assert!(response_body(&vec![b'x'; MAX_RESPONSE + 1]).is_none());
        assert!(!valid_player_id("id\r\nInjected: header"));
        assert!(!valid_player_id(""));
    }

    #[test]
    fn xml_requires_one_local_player_and_one_complete_music_timeline() {
        crate::media::init();
        assert_eq!(player("<MediaContainer><Player product='Plexamp' machineIdentifier='test-player'/></MediaContainer>"), Some("test-player".into()));
        assert!(player("<MediaContainer><Player product='Other' machineIdentifier='test-player'/></MediaContainer>").is_none());
        assert!(player("<MediaContainer><Player product='Plexamp'/><Player product='Plexamp'/></MediaContainer>").is_none());
        assert!(player("<!DOCTYPE MediaContainer [<!ENTITY x SYSTEM 'file:///private'>]><MediaContainer>&x;</MediaContainer>").is_none());
        assert!(timeline(
            "<MediaContainer><Timeline type='music' state='paused'/></MediaContainer>"
        )
        .is_none());
        assert!(timeline(
            "<MediaContainer><Timeline type='video' state='paused'/></MediaContainer>"
        )
        .is_none());
        assert!(timeline("<MediaContainer").is_none());
        assert_eq!(timeline("<MediaContainer><Timeline type='music' state='paused' key='/library/metadata/1' playQueueItemID='2' playQueueID='3' playQueueVersion='1' time='100'/></MediaContainer>"), Some(track("paused", "100")));
    }

    #[test]
    #[ignore = "Requires local Plexamp with a loaded track; does not change playback"]
    fn local_player_and_timeline_can_be_read_without_credentials() {
        crate::media::init();
        let id = player(&request("/resources", None).expect("Plexamp resources")).unwrap();
        let snapshot = timeline(
            &request("/player/timeline/poll?wait=0", Some(&id)).expect("Plexamp timeline"),
        )
        .unwrap();
        assert!(matches!(snapshot.state.as_str(), "playing" | "paused"));
    }
}
