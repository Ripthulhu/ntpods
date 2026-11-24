use std::{
    io::{self, Read},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Layout {
    width: u16,
    speakers: [Option<(i16, i16, i16)>; 11],
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            width: 100,
            speakers: [None; 11],
        }
    }
}

impl Layout {
    fn parse(line: &[u8]) -> Option<Self> {
        if line.len() > 256 {
            return None;
        }
        let mut words = std::str::from_utf8(line).ok()?.split_ascii_whitespace();
        if words.next()? != "ROOM" {
            return None;
        }
        let width = words.next()?.parse().ok()?;
        if !(50..=500).contains(&width) {
            return None;
        }
        let mut layout = Self {
            width,
            ..Self::default()
        };
        for word in words {
            let mut fields = word.split(',');
            let bit: usize = fields.next()?.parse().ok()?;
            let az: i16 = fields.next()?.parse().ok()?;
            let el: i16 = fields.next()?.parse().ok()?;
            let db: i16 = fields.next().unwrap_or("0").parse().ok()?;
            if !matches!(bit, 0..=5 | 9 | 10)
                || fields.next().is_some()
                || !(-180..=180).contains(&az)
                || !(-90..=90).contains(&el)
                || !(-24..=0).contains(&db)
                || (bit == 3 && (az != 0 || el != 0))
                || layout.speakers[bit].is_some()
            {
                return None;
            }
            layout.speakers[bit] = Some((az, el, db));
        }
        Some(layout)
    }

    pub(super) fn placement(self, bit: u32, default_angle: f32) -> ([f32; 3], f32) {
        let configured = self.speakers[bit.trailing_zeros() as usize];
        let (az, el, db) = configured.map_or((default_angle, 0., 0.), |(az, el, db)| {
            (f32::from(az), f32::from(el), f32::from(db))
        });
        let (s, c) = az.to_radians().sin_cos();
        let mut bearing = (s * f32::from(self.width.min(300)) / 100.).atan2(c);
        if self.width > 300 {
            // Extend only the front pair, where Windows places stereo content in a 7.1 mix.
            let extra = (f32::from(self.width - 300) * 0.15).to_radians();
            match bit {
                1 => bearing -= extra,
                2 => bearing += extra,
                _ => {}
            }
        }
        let (s, c) = bearing.sin_cos();
        let (height, horizontal) = el.to_radians().sin_cos();
        (
            [s * horizontal, height, -c * horizontal],
            10_f32.powf(db / 20.),
        )
    }
}

#[derive(Default)]
pub(super) struct LiveRoom {
    pub stop: AtomicBool,
    pub format: AtomicU32,
    peaks: [AtomicU32; 8],
    saved: [Layout; 3],
    pub layout: Mutex<Option<Layout>>,
}

impl LiveRoom {
    pub fn initial(&self, mask: u32) -> Layout {
        match mask {
            0x3f => self.saved[0],
            0x60f => self.saved[1],
            0x63f => self.saved[2],
            _ => Layout::default(),
        }
    }

    pub fn publish_peaks(&self, peaks: [f32; 8]) {
        for (slot, peak) in self.peaks.iter().zip(peaks) {
            if peak > 0. {
                // Positive finite f32 values have the same ordering as their unsigned bit patterns.
                slot.fetch_max(peak.to_bits(), Ordering::Relaxed);
            }
        }
    }

    pub fn take_levels(&self, mask: u32) -> (u32, [i8; 8]) {
        let mut active = 0;
        let mut levels = [-60; 8];
        for (i, bit) in (0..11).filter(|&bit| mask & (1 << bit) != 0).enumerate() {
            let peak = f32::from_bits(self.peaks[i].swap(0, Ordering::Relaxed));
            if peak > 1e-6 {
                active |= 1 << bit;
            }
            let db = 20. * peak.max(0.001).log10();
            levels[i] = ((db / 3.).round() * 3.).clamp(-60., 0.) as i8;
        }
        (active, levels)
    }

    pub fn open(
        mut input: impl Read + Send + 'static,
        initial: &[String],
    ) -> io::Result<Arc<Self>> {
        let mut saved = [Layout::default(); 3];
        if !initial.is_empty() {
            if initial.len() != 3 {
                return Err(io::Error::other("Expected three saved surround layouts"));
            }
            for (entry, line) in saved.iter_mut().zip(initial) {
                *entry = Layout::parse(line.as_bytes())
                    .ok_or_else(|| io::Error::other("Invalid saved surround layout"))?;
            }
        }
        let room = Arc::new(Self {
            saved,
            ..Self::default()
        });
        let reader = room.clone();
        thread::Builder::new()
            .name("spatial-layout".into())
            .spawn(move || {
                let mut chunk = [0; 256];
                let mut line = Vec::with_capacity(256);
                let mut overflow = false;
                loop {
                    match input.read(&mut chunk) {
                        Ok(0) => break,
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                        Ok(n) => {
                            for &b in &chunk[..n] {
                                if b == b'\n' {
                                    if !overflow && let Some(layout) = Layout::parse(&line) {
                                        *reader.layout.lock().unwrap() = Some(layout);
                                    }
                                    line.clear();
                                    overflow = false;
                                } else if line.len() < 256 {
                                    line.push(b);
                                } else {
                                    overflow = true;
                                }
                            }
                        }
                    }
                }
                reader.stop.store(true, Ordering::Release);
            })?;
        Ok(room)
    }
}

#[cfg(test)]
mod tests {
    use super::super::{BLOCK, Orientation, RenderBed, Result, Surround};
    use super::*;

    #[test]
    fn initial_settings_follow_actual_mix_and_levels_retain_each_channels_peak() {
        let layouts = ["ROOM 75", "ROOM 151", "ROOM 500"].map(str::to_owned);
        let room = LiveRoom::open(io::empty(), &layouts).unwrap();
        assert_eq!(
            (
                room.initial(0x3f).width,
                room.initial(0x60f).width,
                room.initial(0x63f).width
            ),
            (75, 151, 500)
        );
        room.publish_peaks([0.5, 0., 0.1, 0., 1.2, 0., 0., 0.]);
        room.publish_peaks([0.001, 0., 0., 0., 0., 0.0001, 0., 0.]);
        let (mask, levels) = room.take_levels(0x60f);
        assert_eq!(mask, 0x605);
        assert_eq!(levels, [-6, -60, -21, -60, 0, -60, -60, -60]);
        assert_eq!(room.take_levels(0x60f), (0, [-60; 8]));
    }
    #[test]
    fn maximum_width_places_default_fronts_at_their_corresponding_sides() {
        let max = Layout::parse(b"ROOM 500").unwrap();
        for (front, front_angle, side, side_angle) in [(1, -30., 0x200, -90.), (2, 30., 0x400, 90.)]
        {
            let (front, front_gain) = max.placement(front, front_angle);
            let (side, side_gain) = max.placement(side, side_angle);
            assert_eq!(front_gain, side_gain);
            assert!(front.iter().zip(side).all(|(a, b)| (a - b).abs() < 1e-6));
        }
    }
    #[test]
    fn wide_layouts_render_the_configured_front_pair_and_preserve_surrounds_and_lfe() -> Result<()>
    {
        for (width, front) in [(300, 60), (400, 75), (500, 90)] {
            check_rendered_bearings(width, front)?;
        }
        Ok(())
    }
    fn check_rendered_bearings(width: u16, front: i16) -> Result<()> {
        let mut room = RenderBed::new(Surround::new(8, 0x63f)?, 0x63f);
        let mut reference = Surround::new(8, 0x63f)?;
        room.renderer.set_layout(
            Layout::parse(format!("ROOM {width} 0,-30,15,-6 1,30,0,-3").as_bytes()).unwrap(),
            &room.speaker_bits,
            0x63f,
        )?;
        reference.set_layout(
            Layout::parse(
                format!("ROOM 100 0,-{front},15,-6 1,{front},0,-3 4,-120,0,0 5,120,0,0").as_bytes(),
            )
            .unwrap(),
            &room.speaker_bits,
            0x63f,
        )?;
        let mut peak_error = 0_f32;
        // Check stereo content on just FL/FR as well as each of the eight discrete channels.
        for channel in 0..=8 {
            let mut input = [0.; BLOCK * 8];
            if channel == 8 {
                input[0] = 0.4;
                input[8 * (BLOCK / 2) + 1] = -0.3;
            } else {
                input[channel] = 0.4;
            }
            let mut out = [[0.; 2]; BLOCK];
            let mut expected = [[0.; 2]; BLOCK];
            room.renderer
                .render(&input, Orientation::default(), &mut out)?;
            reference.render(&input, Orientation::default(), &mut expected)?;
            assert!(expected.iter().flatten().any(|v| v.abs() > 1e-4));
            let difference = out
                .iter()
                .flatten()
                .zip(expected.iter().flatten())
                .map(|(a, b)| (a - b).abs())
                .fold(0., f32::max);
            assert!(
                difference < 1e-5,
                "channel {channel} PCM differs by {difference}"
            );
            peak_error = peak_error.max(difference);
        }
        println!(
            "{width}% width compared with explicit 7.1 bearings: peak PCM difference {peak_error:e}"
        );
        Ok(())
    }
    #[test]
    fn live_channel_move_and_gain_reach_rendered_pcm_with_a_smooth_lfe_transition() -> Result<()> {
        let mut bed = RenderBed::new(Surround::new(8, 0x63f)?, 0x63f);
        let config = Layout::parse(b"ROOM 100 2,90,20,-6 3,0,0,-12").unwrap();
        bed.renderer.set_layout(config, &bed.speaker_bits, 0x63f)?;
        let mut input = [0.; BLOCK * 8];
        let mut out = [[0.; 2]; BLOCK];
        for frame in input.chunks_exact_mut(8) {
            frame[3] = 0.4;
        }
        bed.renderer
            .render(&input, Orientation::default(), &mut out)?;
        assert!(out[0][0] > 0.099 && out[0][0] < 0.1);
        assert!(out.windows(2).all(|pair| pair[0][0] >= pair[1][0]));
        assert!((out[BLOCK - 1][0] - 0.1 * 10_f32.powf(-12. / 20.)).abs() < 1e-5);
        input.fill(0.);
        input[2] = 0.4;
        bed.renderer
            .render(&input, Orientation::default(), &mut out)?;
        let mut reference = Surround::new(8, 0x63f)?;
        let (s, c) = 20_f32.to_radians().sin_cos();
        reference
            .speakers
            .iter_mut()
            .find(|(i, _, _)| *i == 2)
            .unwrap()
            .1 = [c, s, 0.];
        let mut expected = [[0.; 2]; BLOCK];
        reference.render(&input, Orientation::default(), &mut expected)?;
        assert!(expected.iter().flatten().any(|n| n.abs() > 1e-4));
        for (a, b) in out.iter().flatten().zip(expected.iter().flatten()) {
            assert!((a - b * 10_f32.powf(-6. / 20.)).abs() < 1e-5);
        }
        Ok(())
    }
    #[test]
    fn eof_releases_session_and_oversized_or_invalid_edits_leave_last_layout() {
        let good = b"ROOM 500 0,-45,20,-9";
        let mut stream = [good.as_slice(), b"\nROOM 50"].concat();
        stream.extend([b' '; 300]);
        stream.extend_from_slice(b"\nROOM 99 1,0,91\nROOM 501\n");
        let room = LiveRoom::open(io::Cursor::new(stream), &[]).unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !room.stop.load(Ordering::Acquire) {
            assert!(std::time::Instant::now() < until);
            thread::yield_now();
        }
        assert_eq!(*room.layout.lock().unwrap(), Layout::parse(good));
    }
    #[test]
    fn sparse_layout_validates_before_replacing_and_keeps_lfe_centered() {
        let cfg = Layout::parse(b"ROOM 150 0,-30,45,-6 3,0,0,-24").unwrap();
        let (v, level) = cfg.placement(1, -30.);
        assert!((v[1] - 0.5_f32.sqrt()).abs() < 1e-5 && v[0] < -0.4 && v[2] < 0.);
        assert!((level - 0.501187).abs() < 1e-4);
        assert_eq!(cfg.placement(0x200, -90.).1, 1.);
        assert!((cfg.placement(8, 0.).1 - 0.0630957).abs() < 1e-5);
        for bad in [
            "ROOM 49",
            "ROOM 501",
            "ROOM 80 3,3,0",
            "ROOM 100 0,0,0 0,0,0",
            "ROOM 100 0,0,0,1",
            "ROOM 100 1,NaN,0",
            "ROOM 100 9,0,91",
        ] {
            assert!(Layout::parse(bad.as_bytes()).is_none(), "{bad}");
        }
        assert_eq!(Layout::parse(b"ROOM 100"), Some(Layout::default()));
    }
}
