//! Device-free context for tests that exercise production session dispatch.
use super::*;

impl Ctx {
    pub(crate) fn for_test() -> Self {
        Self {
            dongle: Arc::default(),
            dongle_health: Arc::default(),
            companion: Arc::default(),
            companion_preferences: Arc::new(Mutex::new(companion_prefs::Store::for_test())),
            companion_actions: Arc::default(),
            state: Arc::new(Mutex::new(Snapshot::default())),
            battery_keys: Arc::default(),
            battery_cache: Arc::default(),
            settings_pending: Arc::default(),
            settings_write: Arc::default(),
            mic_preference: Arc::default(),
            #[cfg(feature = "experimental-playback")]
            playback_selection: Arc::new(Mutex::new(playback::Selection::new(
                ipc::PlaybackMode::Windows,
            ))),
            #[cfg(feature = "experimental-playback")]
            playback_buffer: Arc::default(),
            #[cfg(feature = "experimental-playback")]
            audio_change_busy: Arc::default(),
            mic_demand: Arc::default(),
            mic_clock: Instant::now(),
            hardware_check: Arc::default(),
            case_observed_tick: Arc::default(),
            hr_commands: Arc::default(),
            hr_generation: Arc::default(),
            head_tracking: Arc::default(),
            nearby_finder: Arc::new(Mutex::new(nearby_finder::Finder::new(String::new(), false))),
            nearby_last_heard_tick: Arc::default(),
            auto_pause: Arc::new(Mutex::new(media::AutoPausePolicy::new(true))),
            hr_db: hrdb::HrDb::for_test(),
            clients: Arc::default(),
            driver_cell: Arc::default(),
            shared_driver: Arc::default(),
            mic_on: Arc::default(),
            hq_mic_quiet: Arc::new(AtomicBool::new(true)),
            capture_active: Arc::default(),
            shutting_down: Arc::default(),
            receiver_done: Arc::default(),
            playback_done: Arc::default(),
            shutdown_cleanup_failed: Arc::default(),
            auto_mode: Arc::default(),
            hr_on: Arc::default(),
            hr_got_sample: Arc::default(),
            hr_stream_live: Arc::default(),
            hr_retrying: Arc::default(),
            connect_requested: Arc::new(ConnectionRequest::new(false)),
            case_return: Arc::default(),
            paired_target: Arc::default(),
            wants_reconnect: Arc::default(),
            wants_settings_refresh: Arc::default(),
            hearing_confirmation: Arc::default(),
            pipe: Arc::new(micpipe::MicPipeCell::for_test()),
            conv_duck: Arc::new(Mutex::new(volume::ConvDuck::new(25))),
            user_disconnected: Arc::default(),
            pending_audio_disconnect: Arc::default(),
            last_repair: Arc::default(),
            dev_name: Arc::default(),
            mac: Arc::default(),
        }
    }
}
