pub(super) const REPAIR_FRAMES: u64 = 960; // 20 ms at the required 48 kHz.

#[derive(Debug, PartialEq, Eq)]
pub(super) enum PacketAction {
    Keep,
    Pad(u32),
    Trim(u32),
    Reset,
}

#[derive(Default)]
pub(super) struct CaptureTimeline {
    expected: Option<u64>,
    last_qpc: Option<u64>,
}

impl CaptureTimeline {
    pub(super) fn observe(
        &mut self,
        index: u64,
        qpc: u64,
        frames: u32,
        discontinuity: bool,
        timestamp_error: bool,
    ) -> PacketAction {
        const IDLE_HNS: u64 = 2_500_000;
        if frames == 0 {
            return PacketAction::Keep;
        }
        let n = u64::from(frames);
        if timestamp_error || index.checked_add(n).is_none() {
            // A bad stamp does not invalidate the PCM itself.
            self.last_qpc = None;
            if discontinuity {
                let known = self.expected.take().is_some();
                return if known {
                    PacketAction::Reset
                } else {
                    PacketAction::Keep
                };
            }
            self.expected = self.expected.and_then(|next| next.checked_add(n));
            return PacketAction::Keep;
        }
        let next_qpc = (qpc != 0).then_some(qpc);
        let old_qpc = self.last_qpc;
        let mut action = PacketAction::Keep;
        if let Some(expected) = self.expected {
            if let (Some(last), Some(now)) = (old_qpc, next_qpc)
                && now < last
                && index < expected
                && index + n <= expected
            {
                // A delayed whole packet cannot move the accepted timeline backward.
                return PacketAction::Trim(frames);
            }
            // Steam can retain contiguous frame indices over a long producer pause.
            let stale = old_qpc
                .zip(next_qpc)
                .is_some_and(|(a, b)| a.abs_diff(b) > IDLE_HNS);
            let restarted = index == 0
                && expected != 0
                && discontinuity
                && old_qpc.zip(next_qpc).is_some_and(|(a, b)| b > a);
            if stale || restarted || index.abs_diff(expected) > REPAIR_FRAMES {
                action = PacketAction::Reset;
            } else if index > expected {
                action = PacketAction::Pad((index - expected) as u32);
            } else if index < expected {
                let overlap = (expected - index).min(n) as u32;
                if overlap == frames {
                    return PacketAction::Trim(frames);
                }
                action = PacketAction::Trim(overlap);
            }
            // A contiguous flagged 96-frame Steam packet is genuine data.
            // Its following packet's position reveals any missing frames.
        }
        self.expected = Some(index + n);
        self.last_qpc = next_qpc;
        action
    }
}

#[cfg(test)]
mod tests {
    use super::{CaptureTimeline, PacketAction::*};

    #[test]
    fn steam_short_flagged_packets_are_kept_and_only_the_measured_gap_is_filled() {
        let mut t = CaptureTimeline::default();
        let mut time = 900_000;
        for (index, frames, flag, expected) in [
            (0, 480, true, Keep),
            (480, 480, false, Keep),
            (960, 96, true, Keep),
            (1440, 480, false, Pad(384)),
            (1920, 480, false, Keep),
            (2400, 96, true, Keep),
            (2880, 480, false, Pad(384)),
        ] {
            assert_eq!(t.observe(index, time, frames, flag, false), expected);
            time += 100_000;
        }
    }

    #[test]
    fn handles_overlap_replay_and_real_clock_restarts() {
        let mut t = CaptureTimeline::default();
        assert_eq!(t.observe(6000, 200_000, 480, false, false), Keep);
        assert_eq!(t.observe(6000, 200_000, 480, false, false), Trim(480));
        assert_eq!(t.observe(6400, 300_000, 480, false, false), Trim(80));
        assert_eq!(t.observe(100, 50_000, 48, false, false), Trim(48));
        assert_eq!(t.observe(6880, 400_000, 480, false, false), Keep);
        assert_eq!(t.observe(10000, 750_000, 480, false, false), Reset);
        assert_eq!(t.observe(0, 850_000, 480, true, false), Reset);
        assert_eq!(t.observe(0, 950_000, 480, true, false), Reset);
        assert_eq!(t.observe(480, 8_062_036, 480, false, false), Reset);
        assert_eq!(t.observe(960, 8_162_036, 480, true, false), Keep);
    }

    #[test]
    fn timestamp_errors_keep_pcm_without_using_untrusted_indices() {
        let mut t = CaptureTimeline::default();
        assert_eq!(t.observe(1234, 99, 0, true, false), Keep);
        assert_eq!(t.observe(0, 20_000, 480, true, false), Keep);
        assert_eq!(t.observe(u64::MAX, 1, 480, false, true), Keep);
        assert_eq!(t.observe(960, 220_000, 480, false, false), Keep);
        assert_eq!(t.observe(999999, 1, 480, true, true), Reset);
        assert_eq!(t.observe(1234, 1, 480, true, true), Keep);
        assert_eq!(t.observe(123456, 520_000, 480, false, false), Keep);
        assert_eq!(t.observe(123936, 620_000, 480, false, false), Keep);
    }
}
