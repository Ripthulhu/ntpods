use std::time::Duration;

#[derive(Default)]
pub(super) struct Timing {
    frames: u64,
    samples: u64,
    queue_total: u128,
    queue_max: u32,
    late_total_us: u128,
    late_max_us: u64,
    sends: u64,
    send_total_us: u128,
    send_max_us: u64,
}

impl Timing {
    pub(super) fn frame(&mut self, samples: usize, queued_frames: u32, lateness: Duration) {
        self.frames = self.frames.saturating_add(1);
        self.samples = self.samples.saturating_add(samples as u64);
        self.queue_total = self.queue_total.saturating_add(u128::from(queued_frames));
        self.queue_max = self.queue_max.max(queued_frames);
        let late_us = micros(lateness);
        self.late_total_us = self.late_total_us.saturating_add(u128::from(late_us));
        self.late_max_us = self.late_max_us.max(late_us);
    }

    pub(super) fn send(&mut self, elapsed: Duration) {
        self.sends = self.sends.saturating_add(1);
        let elapsed_us = micros(elapsed);
        self.send_total_us = self.send_total_us.saturating_add(u128::from(elapsed_us));
        self.send_max_us = self.send_max_us.max(elapsed_us);
    }

    pub(super) fn summary(&self) -> Option<String> {
        if self.frames == 0 {
            return None;
        }
        let queue_mean = self.queue_total as f64 / self.frames as f64;
        // These are local queue and call timings, not arrival times at the earbuds.
        Some(format!(
            "frames={} samples={} queue_frames_mean={:.1} queue_frames_max={} queue_ms_mean={:.3} queue_ms_max={:.3} late_us_mean={} late_us_max={} sends={} send_us_mean={} send_us_max={}",
            self.frames,
            self.samples,
            queue_mean,
            self.queue_max,
            queue_mean / 48.0,
            f64::from(self.queue_max) / 48.0,
            self.late_total_us / u128::from(self.frames),
            self.late_max_us,
            self.sends,
            self.send_total_us / u128::from(self.sends.max(1)),
            self.send_max_us,
        ))
    }
}

fn micros(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_queue_duration_for_48khz_pcm_and_aggregates_local_timings() {
        let mut timing = Timing::default();
        timing.frame(480, 480, Duration::from_micros(500));
        timing.frame(480, 960, Duration::from_micros(1500));
        timing.send(Duration::from_micros(200));
        timing.send(Duration::from_micros(800));
        assert_eq!(timing.summary().unwrap(),
            "frames=2 samples=960 queue_frames_mean=720.0 queue_frames_max=960 queue_ms_mean=15.000 queue_ms_max=20.000 late_us_mean=1000 late_us_max=1500 sends=2 send_us_mean=500 send_us_max=800");
    }

    #[test]
    fn empty_session_is_silent_and_unsent_frame_has_no_send_time() {
        let mut timing = Timing::default();
        assert!(timing.summary().is_none());
        timing.frame(1024, 1024, Duration::ZERO);
        let summary = timing.summary().unwrap();
        assert!(summary.contains("queue_ms_mean=21.333 queue_ms_max=21.333"));
        assert!(summary.ends_with("sends=0 send_us_mean=0 send_us_max=0"));
    }

    #[test]
    fn counters_and_duration_conversion_do_not_wrap() {
        let mut timing = Timing {
            frames: u64::MAX,
            samples: u64::MAX,
            queue_total: u128::MAX,
            late_total_us: u128::MAX,
            sends: u64::MAX,
            send_total_us: u128::MAX,
            ..Timing::default()
        };
        timing.frame(480, 480, Duration::MAX);
        timing.send(Duration::MAX);
        assert_eq!(timing.frames, u64::MAX);
        assert_eq!(timing.samples, u64::MAX);
        assert_eq!(timing.sends, u64::MAX);
        assert_eq!(timing.queue_total, u128::MAX);
        assert_eq!(timing.late_total_us, u128::MAX);
        assert_eq!(timing.send_total_us, u128::MAX);
        assert_eq!(timing.late_max_us, u64::MAX);
        assert_eq!(timing.send_max_us, u64::MAX);
        assert!(timing.summary().is_some());
    }
}
