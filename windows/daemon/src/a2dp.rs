//! Restore the target AirPods' playback after stopping their microphone.
//!
//! A quiet microphone uplink can still leave one earbud silent. Reopen the audio
//! services using the same path as Restore audio; keep the virtual input present.

pub fn reset(mac: u64) -> (u32, u32) {
    crate::bt::reset_audio_services_for_cleanup(mac, false)
}
