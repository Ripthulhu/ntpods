use std::{collections::VecDeque, f64::consts::PI};

// Two independent 48 kHz endpoints. Timestamps measure relative rate; queue
// feedback remains available for endpoints whose driver omits clock samples.
const RATE: usize = 48_000;
const MAX_PPM: f64 = 1000.0;
const TAPS: usize = 48;
const LEFT: usize = TAPS / 2 - 1;
const PHASES: usize = 256;

#[derive(Default)]
struct Clock {
    anchor: Option<((u64, u64), (u64, u64))>,
    measured: Option<f64>,
    age: usize,
    target: Option<f64>,
    sum: f64,
    frames: usize,
    ppm: f64,
}

impl Clock {
    fn observe(&mut self, input: (u64, u64), output: (u64, u64), frequency: u64) {
        if frequency == 0 || input.1 == 0 || output.1 == 0 || input.1.abs_diff(output.1) > 5_000_000
        {
            return;
        }
        let next = Some((input, output));
        let Some((old_in, old_out)) = self.anchor else {
            self.anchor = next;
            return;
        };
        if input.1 < old_in.1 || output.1 < old_out.1 || input.0 < old_in.0 || output.0 < old_out.0
        {
            self.anchor = next;
            self.measured = None;
            return;
        }
        let (in_dt, out_dt) = (input.1 - old_in.1, output.1 - old_out.1);
        if in_dt.min(out_dt) < 40_000_000 {
            return;
        } // QPC units are 100 ns.
        self.anchor = next;
        let in_rate = (input.0 - old_in.0) as f64 / in_dt as f64;
        let out_rate = (output.0 - old_out.0) as f64 / out_dt as f64;
        let ppm = (in_rate / out_rate * frequency as f64 / RATE as f64 - 1.0) * 1e6;
        if ppm.abs() <= MAX_PPM {
            self.measured = Some(self.measured.map_or(ppm, |old| old + 0.25 * (ppm - old)));
            self.age = 0;
        }
    }

    fn ratio(&mut self, depth: usize, n: usize, prime: usize) -> f64 {
        self.age = self.age.saturating_add(n);
        // Average at consumption time across two seconds so individual 256-frame
        // HRTF blocks and different capture/render periods do not steer the pitch.
        self.sum += (depth as f64 - n as f64 * 0.5) * n as f64;
        self.frames += n;
        if self.frames >= RATE * 2 {
            let mean = self.sum / self.frames as f64;
            let target = *self.target.get_or_insert(mean);
            let measured = self.measured.filter(|_| self.age < RATE * 16);
            let deadband = if measured.is_some() {
                prime as f64 * 0.5
            } else {
                (prime / 64).max(12) as f64
            };
            let error = mean - target;
            let feedback = (error.abs() - deadband).max(0.0) * error.signum() * 3.0;
            self.ppm = (measured.unwrap_or(0.0) + feedback).clamp(-MAX_PPM, MAX_PPM);
            if self.ppm.abs() < 2.0 {
                self.ppm = 0.0;
            }
            self.sum = 0.0;
            self.frames = 0;
        }
        1.0 + self.ppm * 1e-6
    }
}

fn kernels() -> Vec<[f32; TAPS]> {
    (0..=PHASES)
        .map(|p| {
            let phase = p as f64 / PHASES as f64;
            let mut row = [0.0; TAPS];
            if p == 0 || p == PHASES {
                row[LEFT + usize::from(p == PHASES)] = 1.0;
            } else {
                for (i, tap) in row.iter_mut().enumerate() {
                    let x = i as f64 - LEFT as f64 - phase;
                    let angle = x * PI / (TAPS as f64 * 0.5);
                    let blackman = 0.42 + 0.5 * angle.cos() + 0.08 * (2.0 * angle).cos();
                    *tap = ((PI * x).sin() / (PI * x) * blackman) as f32;
                }
                let sum = row.iter().sum::<f32>();
                for tap in &mut row {
                    *tap /= sum;
                }
            }
            row
        })
        .collect()
}

#[derive(Default)]
pub(super) struct Stats {
    pub(super) underruns: u64,
    pub(super) silence: u64,
    pub(super) startup_silence: u64,
    pub(super) discarded: u64,
    pub(super) overflow: u64,
}

pub(super) struct Playout {
    // LEFT frames of filter history precede the first queued input frame.
    queue: VecDeque<[f32; 2]>,
    kernel: Vec<[f32; TAPS]>,
    prime: usize,
    capacity: usize,
    phase: f64,
    clock: Clock,
    running: bool,
    ending: bool,
    ever_started: bool,
    since_push: usize,
    pub(super) stats: Stats,
}

impl Playout {
    pub(super) fn new(prime_frames: usize, capacity: usize) -> Self {
        assert!(prime_frames > 0 && capacity >= prime_frames);
        let mut queue = VecDeque::with_capacity(capacity + LEFT);
        queue.resize(LEFT, [0.0; 2]);
        Self {
            queue,
            kernel: kernels(),
            prime: prime_frames,
            capacity,
            phase: 0.0,
            clock: Clock::default(),
            running: false,
            ending: false,
            ever_started: false,
            since_push: 0,
            stats: Stats::default(),
        }
    }

    pub(super) fn depth(&self) -> usize {
        self.queue.len() - LEFT
    }
    pub(super) fn correction_ppm(&self) -> f64 {
        self.clock.ppm
    }

    pub(super) fn observe_clocks(
        &mut self,
        source: (u64, u64),
        sink: (u64, u64),
        sink_frequency: u64,
    ) {
        self.clock.observe(source, sink, sink_frequency);
    }

    pub(super) fn reset(&mut self) {
        self.stats.discarded += self.depth() as u64;
        self.queue.clear();
        self.queue.resize(LEFT, [0.0; 2]);
        self.phase = 0.0;
        self.clock = Clock::default();
        self.since_push = 0;
        self.running = false;
        self.ending = false;
    }

    pub(super) fn push(&mut self, frames: &[[f32; 2]]) {
        if frames.is_empty() {
            return;
        }
        self.since_push = 0;
        self.ending = false;
        let excess = (self.depth() + frames.len()).saturating_sub(self.capacity);
        let skip = excess.saturating_sub(self.depth());
        if excess > 0 {
            self.queue.drain(..excess.min(self.depth()));
            self.stats.discarded += excess as u64;
            self.stats.overflow += excess as u64;
            self.phase = 0.0;
            self.clock = Clock::default();
            self.queue
                .iter_mut()
                .take(LEFT)
                .for_each(|frame| *frame = [0.0; 2]);
        }
        self.queue.extend(&frames[skip..]);
    }

    pub(super) fn finish_input(&mut self) {
        self.ending = true;
    }

    pub(super) fn pull(&mut self, out: &mut [[f32; 2]]) {
        if out.is_empty() {
            return;
        }
        self.since_push = self.since_push.saturating_add(out.len());
        let depth = self.depth();
        if !self.running
            && depth != 0
            && (depth >= self.prime || self.since_push > self.prime || self.ending)
        {
            self.running = true;
            self.ever_started = true;
        }
        let mut written = 0;
        if self.running {
            let ratio = self.clock.ratio(depth, out.len(), self.prime);
            let data = self.queue.make_contiguous();
            if ratio == 1.0 && self.phase == 0.0 {
                written = depth.min(out.len());
                out[..written].copy_from_slice(&data[LEFT..LEFT + written]);
                self.queue.drain(..written);
            } else {
                let mut position = self.phase;
                for sample in &mut *out {
                    let center = position as usize;
                    if center >= depth {
                        break;
                    }
                    let fp = (position - center as f64) * PHASES as f64;
                    let lo = fp as usize;
                    let blend = (fp - lo as f64) as f32;
                    let mut sum = [0.0; 2];
                    for ((a, b), frame) in self.kernel[lo]
                        .iter()
                        .zip(&self.kernel[lo + 1])
                        .zip(&data[center..])
                    {
                        let weight = a + (b - a) * blend;
                        sum[0] += weight * frame[0];
                        sum[1] += weight * frame[1];
                    }
                    *sample = sum;
                    position += ratio;
                    written += 1;
                }
                let used = (position as usize).min(depth);
                self.queue.drain(..used);
                self.phase = position - used as f64;
            }
            if written < out.len() {
                self.stats.underruns += u64::from(!self.ending);
                self.reset();
            }
        }
        out[written..].fill([0.0; 2]);
        let silent = (out.len() - written) as u64;
        self.stats.silence += silent;
        if !self.ever_started {
            self.stats.startup_silence += silent;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independent_clocks_remain_bounded_for_45_minutes() {
        for (ppm, use_timestamps) in [
            (200.0, true),
            (-200.0, true),
            (200.0, false),
            (-200.0, false),
            (0.0, true),
        ] {
            let mut clock = Clock::default();
            let (prime, capacity, render_n) = (1536, 4096, 480);
            let mut source = 0usize;
            let mut used = 0.0;
            let mut min = usize::MAX;
            let mut max = 0;
            let out_frames = 45 * 60 * RATE;
            let factor = 1.0 + ppm * 1e-6;
            for host in (render_n..=out_frames).step_by(render_n) {
                // Bursty 21.3 ms capture and 10 ms render callbacks; occasionally the
                // packet arrives one callback late. Positions are measured on separate clocks.
                let late = if (host / render_n) % 113 == 0 {
                    render_n
                } else {
                    0
                };
                let available = ((host - late) as f64 * factor) as usize / 1024 * 1024;
                source = source.max(available);
                if use_timestamps && host % RATE == 0 {
                    let qpc = host as u64 * 10_000_000 / RATE as u64 + 1;
                    clock.observe(
                        (((host as f64) * factor).round() as u64, qpc),
                        (host as u64 * 8, qpc),
                        RATE as u64 * 8,
                    );
                }
                let depth = source - used as usize;
                if used == 0.0 && depth < prime {
                    continue;
                }
                let ratio = clock.ratio(depth, render_n, prime);
                assert!(clock.ppm.abs() <= MAX_PPM);
                min = min.min(depth);
                max = max.max(depth);
                assert!(
                    depth >= render_n,
                    "underrun at {host}, {ppm}, timestamps {use_timestamps}"
                );
                assert!(
                    depth < capacity,
                    "overflow at {host}, {ppm}, timestamps {use_timestamps}"
                );
                used += ratio * render_n as f64;
            }
            println!(
                "45 min skew {ppm:+} ppm clocks={use_timestamps}: {min}..{max} buffered, final {:+.2} ppm",
                clock.ppm
            );
            if use_timestamps {
                assert!((clock.ppm - ppm).abs() < 3.0);
                if ppm == 0.0 {
                    assert_eq!(used.fract(), 0.0);
                }
            }
        }
    }

    #[test]
    fn fractional_filter_preserves_dc_and_upper_treble() {
        let table = kernels();
        let mut worst_db = 0.0_f64;
        for taps in &table {
            assert!((taps.iter().sum::<f32>() - 1.0).abs() < 1e-6);
            for hz in (18_000..=20_000).step_by(100) {
                let angle = 2.0 * PI * hz as f64 / RATE as f64;
                let (mut re, mut im) = (0.0, 0.0);
                for (index, tap) in taps.iter().enumerate() {
                    let (s, c) = (angle * index as f64).sin_cos();
                    re += f64::from(*tap) * c;
                    im += f64::from(*tap) * s;
                }
                worst_db = worst_db.max((20.0 * re.hypot(im).log10()).abs());
            }
        }
        println!("18–20 kHz worst fractional-phase magnitude error: {worst_db:.5} dB");
        assert!(worst_db < 0.02);

        let mut q = Playout::new(512, 4096);
        let frequency = 20_000.0 * 2.0 * PI / RATE as f64;
        let input: Vec<_> = (0..4096)
            .map(|i| [(i as f64 * frequency).sin() as f32, 0.25])
            .collect();
        q.push(&input);
        q.phase = 0.45;
        q.clock.ppm = 200.0;
        let mut out = vec![[0.0; 2]; 3001];
        for block in out.chunks_mut(479) {
            q.pull(block);
        }
        for (i, frame) in out.iter().enumerate().skip(TAPS) {
            let expected = ((0.45 + i as f64 * 1.0002) * frequency).sin() as f32;
            assert!((frame[0] - expected).abs() < 0.003);
            assert!((frame[1] - 0.25).abs() < 2e-6);
        }
        assert_eq!(q.stats.underruns, 0);
    }

    #[test]
    fn bit_exact_unity_then_drain_short_tail_without_replaying_it() {
        let mut q = Playout::new(96, 256);
        let input: Vec<_> = (0..137)
            .map(|i| [i as f32 / 225.0, -(i as f32) / 421.0])
            .collect();
        let mut all = Vec::new();
        q.push(&input);
        for count in [31, 63, 70] {
            let mut out = vec![[1.0; 2]; count];
            q.pull(&mut out);
            all.extend(out);
        }
        assert_eq!(&all[..137], &input);
        assert_eq!(&all[137..], &[[0.0; 2]; 27]);
        assert_eq!((q.depth(), q.stats.underruns, q.stats.discarded), (0, 1, 0));
        q.push(&[[0.3, -0.4]; 17]);
        let mut out = [[1.0; 2]; 64];
        q.pull(&mut out);
        assert_eq!(out, [[0.0; 2]; 64]);
        q.pull(&mut out);
        assert_eq!(out[..17], [[0.3, -0.4]; 17]);
        assert_eq!(out[17..], [[0.0; 2]; 47]);
        q.pull(&mut out);
        assert_eq!(out, [[0.0; 2]; 64]);
        assert_eq!(q.stats.startup_silence, 0);
    }

    #[test]
    fn overflow_gap_and_bad_timestamps_do_not_leak_old_state() {
        let mut q = Playout::new(96, 128);
        q.push(&[[0.1, 0.2]; 64]);
        q.push(&[[0.9, 0.7]; 160]);
        assert_eq!((q.stats.discarded, q.stats.overflow), (96, 96));
        assert_eq!(q.depth(), 128);
        let mut out = [[0.0; 2]; 64];
        q.pull(&mut out);
        assert_eq!(out, [[0.9, 0.7]; 64]);
        q.observe_clocks((10, 1), (10, 1), 48_000);
        q.observe_clocks((100, 50_000_001), (20, 50_000_001), 48_000);
        assert!(q.clock.measured.is_none());
        q.reset();
        assert_eq!(
            (
                q.stats.discarded,
                q.stats.overflow,
                q.depth(),
                q.correction_ppm()
            ),
            (160, 96, 0, 0.0)
        );
        q.pull(&mut out);
        assert_eq!(out, [[0.0; 2]; 64]);
    }
}
