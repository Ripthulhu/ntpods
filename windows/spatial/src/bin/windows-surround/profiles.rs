use super::{BLOCK, Device, Motion, RATE, Result, Surround, front, pose::Orientation, room};
use audionimbus::{AudioSettings, Context, Hrtf, HrtfSettings, Sofa};
use std::{
    collections::hash_map::RandomState,
    fmt, fs,
    hash::BuildHasher,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

// Limit saved filter compensation to ±6 dB, including when a custom test clip is used.
const GAIN_RANGE: std::ops::RangeInclusive<f32> = 0.5..=2.0;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum Kind {
    Steam,
    H3,
    H20,
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub(super) struct Profile {
    kind: Kind,
    gain: f32,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            kind: Kind::Steam,
            gain: 1.0,
        }
    }
}

impl fmt::Display for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self.kind {
            Kind::Steam => "Steam Audio default",
            Kind::H3 => "SADIE II H3",
            Kind::H20 => "SADIE II H20",
        })
    }
}

fn directory() -> Result<PathBuf> {
    Ok(PathBuf::from(
        std::env::var_os("LOCALAPPDATA").ok_or("Windows local app data is unavailable")?,
    )
    .join("AirPods Spatial Audio")
    .join("hrtf"))
}

impl Profile {
    fn parse(text: &str) -> Result<Self> {
        let mut parts = text.split_whitespace();
        let kind = match parts.next() {
            Some("H3") => Kind::H3,
            Some("H20") => Kind::H20,
            _ => return Err("Unrecognized saved ear profile".into()),
        };
        let gain: f32 = parts.next().ok_or("Missing saved profile level")?.parse()?;
        if !GAIN_RANGE.contains(&gain) || parts.next().is_some() {
            return Err("Invalid saved ear-profile level".into());
        }
        Ok(Self { kind, gain })
    }

    pub(super) fn selected() -> Result<Self> {
        match fs::File::open(directory()?.join("selected.txt")) {
            Ok(file) => {
                let mut text = String::new();
                file.take(65).read_to_string(&mut text)?;
                if text.len() > 64 {
                    return Err("Invalid ear-profile preference".into());
                }
                Self::parse(&text)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub(super) fn set_selected(self) -> Result<()> {
        let dir = directory()?;
        let path = dir.join("selected.txt");
        if self.kind == Kind::Steam {
            return match fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            };
        }
        fs::create_dir_all(&dir)?;
        let temp = dir.join(format!(
            "selected-{:x}.tmp",
            RandomState::new().hash_one(())
        ));
        let mut file = fs::File::create_new(&temp)?;
        let result = writeln!(file, "{:?} {}", self.kind, self.gain);
        drop(file);
        if let Err(e) = result.and_then(|()| fs::rename(&temp, &path)) {
            let _ = fs::remove_file(temp);
            return Err(e.into());
        }
        Ok(())
    }

    pub(super) fn load(self, context: &Context, audio: &AudioSettings) -> Result<Hrtf> {
        let (name, expected) = match self.kind {
            Kind::Steam => return Ok(Hrtf::try_new(context, audio, &HrtfSettings::default())?),
            Kind::H3 => ("H3.sofa", 11_654_654),
            Kind::H20 => ("H20.sofa", 8_753_949),
        };
        let bundled = std::env::current_exe()?.with_file_name(name);
        let path = if bundled.is_file() {
            bundled
        } else {
            directory()?.join(name)
        };
        let mut data = Vec::new();
        fs::File::open(&path)
            .map_err(|e| {
                format!(
                    "Cannot open {} ({e}). Run prepare-hrtf-profiles.py.",
                    path.display()
                )
            })?
            .take(expected + 1)
            .read_to_end(&mut data)?;
        if data.len() as u64 != expected || !data.starts_with(b"\x89HDF\r\n\x1a\n") {
            return Err(format!("Invalid {name}. Run prepare-hrtf-profiles.py again.").into());
        }
        Ok(Hrtf::try_new(
            context,
            audio,
            &HrtfSettings {
                sofa_information: Some(Sofa::Buffer(data)),
                volume: self.gain,
                ..HrtfSettings::default()
            },
        )?)
    }
}

const ORDERS: [[usize; 3]; 6] = [
    [0, 1, 2],
    [0, 2, 1],
    [1, 0, 2],
    [1, 2, 0],
    [2, 0, 1],
    [2, 1, 0],
];

struct Scene {
    voice: Vec<f32>,
    step: Vec<f32>,
    directions: [usize; 3],
}

impl Scene {
    fn new(voice: Vec<f32>, seed: u64) -> Self {
        // Three identical footfalls on one side distinguish front/side/rear without a left/right giveaway.
        let side = [0, 6, 4].map(|speaker| speaker + ((seed / 6) & 1) as usize);
        let directions = ORDERS[(seed % 6) as usize].map(|i| side[i]);
        let mut random = 0x65a9b4f1u32;
        let mut low = 0.0;
        let step = (0..RATE as usize / 4)
            .map(|i| {
                random ^= random << 13;
                random ^= random >> 17;
                random ^= random << 5;
                let noise = random as i32 as f32 / i32::MAX as f32;
                low += 0.13 * (noise - low);
                let t = i as f32 / RATE as f32;
                let attack = (t / 0.004).min(1.0);
                let body = (std::f32::consts::TAU * 105.0 * t).sin() * (-33.0 * t).exp();
                attack * (0.05 * body + (0.075 * low + 0.045 * noise) * (-22.0 * t).exp())
            })
            .collect();
        Self {
            voice,
            step,
            directions,
        }
    }

    fn frames(&self) -> usize {
        self.voice.len() + 5 * RATE as usize
    }

    fn fill(&self, start: usize, bed: &mut [f32; BLOCK * 8]) {
        bed.fill(0.0);
        let rate = RATE as usize;
        for (f, speaker) in bed.chunks_exact_mut(8).enumerate() {
            let Some(t) = (start + f).checked_sub(rate / 2) else {
                continue;
            };
            if let Some(&v) = self.voice.get(t) {
                speaker[2] = v;
                continue;
            }
            if let Some(i) = t.checked_sub(self.voice.len() + rate / 2)
                && let (Some(&dir), Some(&s)) =
                    (self.directions.get(i / rate), self.step.get(i % rate))
            {
                speaker[dir] = s;
            }
        }
    }
}

// ITU-R BS.1770 K-weighting at 48 kHz. All trials use the same signal and duration.
#[derive(Default)]
struct Weight {
    shelf: [f64; 2],
    highpass: [f64; 2],
}
impl Weight {
    fn apply(&mut self, x: f32) -> f64 {
        fn stage(x: f64, s: &mut [f64; 2], b: [f64; 3], a: [f64; 2]) -> f64 {
            let y = b[0] * x + s[0];
            s[0] = b[1] * x - a[0] * y + s[1];
            s[1] = b[2] * x - a[1] * y;
            y
        }
        let y = stage(
            f64::from(x),
            &mut self.shelf,
            [1.53512485958697, -2.69169618940638, 1.19839281085285],
            [-1.69065929318241, 0.73248077421585],
        );
        stage(
            y,
            &mut self.highpass,
            [1.0, -2.0, 1.0],
            [-1.99004745483398, 0.99007225036621],
        )
    }
}

fn measure(scene: &Scene, profile: Profile) -> Result<(f64, f32)> {
    let mut renderer = Surround::with_profile(8, 0x63f, profile)?;
    let mut bed = [0.0; BLOCK * 8];
    let mut stereo = [[0.0; 2]; BLOCK];
    let mut weights = [Weight::default(), Weight::default()];
    let mut energy = 0.0;
    let mut peak = 0.0_f32;
    for start in (0..scene.frames()).step_by(BLOCK) {
        scene.fill(start, &mut bed);
        renderer.render(&bed, Orientation::default(), &mut stereo)?;
        for pair in &stereo[..BLOCK.min(scene.frames() - start)] {
            for (channel, &sample) in pair.iter().enumerate() {
                energy += weights[channel].apply(sample).powi(2);
                peak = peak.max(sample.abs());
            }
        }
    }
    if !energy.is_finite() || energy <= 0.0 || !peak.is_finite() || peak <= 0.0 {
        return Err("Could not measure profile comparison levels".into());
    }
    Ok((energy, peak))
}

fn match_levels(scene: &Scene) -> Result<([Profile; 3], f32)> {
    let mut profiles = [Kind::Steam, Kind::H3, Kind::H20].map(|kind| Profile { kind, gain: 1.0 });
    let mut reference = None;
    let mut peak = 0.0_f32;
    for profile in &mut profiles {
        let (energy, raw_peak) = measure(scene, *profile)?;
        profile.gain = (*reference.get_or_insert(energy) / energy).sqrt() as f32;
        if !GAIN_RANGE.contains(&profile.gain) {
            return Err("Unreasonable profile level correction".into());
        }
        peak = peak.max(raw_peak * profile.gain);
    }
    Ok((profiles, (0.08 / peak).min(1.0)))
}

pub(super) fn compare(sink: &Device, motion: &Motion, speech: &Path, managed: bool) -> Result<()> {
    let random = RandomState::new();
    let scene = Scene::new(room::load_voice(speech)?, random.hash_one("directions"));
    let (all, trim) = match_levels(&scene)?;
    let profiles = ORDERS[(random.hash_one("profiles") % 6) as usize].map(|i| all[i]);
    println!(
        "Face the screen. Each option plays the same front voice and three footsteps. Listen for a natural voice and judge each step's direction. Turn your head during each option."
    );
    let audition = |slot: usize| -> Result<()> {
        println!("Playing {}.", char::from(b'A' + slot as u8));
        if managed {
            println!("PROFILE_PLAYING");
        }
        let mut renderer = Surround::with_profile(8, 0x63f, profiles[slot])?;
        let mut input = [0.0; BLOCK * 8];
        let mut start = 0;
        front::play_blocks(sink, scene.frames(), |block, _| {
            scene.fill(start, &mut input);
            renderer.render(&input, motion.orientation(), block)?;
            for sample in block.iter_mut().flatten() {
                *sample *= trim;
            }
            start += BLOCK;
            Ok(())
        })
    };
    if !managed {
        for i in 0..3 {
            audition(i)?;
        }
    }
    loop {
        if managed {
            println!("PROFILE_READY");
        } else {
            print!(
                "A/B/C to replay, save A/B/C to use a favorite, or Enter to keep your current profile: "
            );
        }
        io::stdout().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        let answer = answer.trim().to_ascii_lowercase();
        if answer.is_empty() {
            break;
        }
        let (save, label) = answer
            .strip_prefix("save ")
            .map_or((false, answer.as_str()), |s| (true, s.trim()));
        let slot = match label {
            "a" => 0,
            "b" => 1,
            "c" => 2,
            _ => continue,
        };
        if save {
            profiles[slot].set_selected()?;
            if managed {
                println!("PROFILE_SAVED");
            }
            println!("Selected {} for future playback.", profiles[slot]);
            break;
        }
        audition(slot)?;
    }
    println!(
        "A was {}, B was {}, C was {}.",
        profiles[0], profiles[1], profiles[2]
    );
    let names = [
        "front left",
        "front right",
        "center",
        "bass",
        "rear left",
        "rear right",
        "side left",
        "side right",
    ];
    println!(
        "Footsteps in order: {}.",
        scene.directions.map(|i| names[i]).join(", ")
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preference_rejects_unknown_or_unbounded_values() {
        for s in [
            "H3 NaN",
            "H20 inf",
            "H3 8",
            "H20 0.01",
            "../H3 1",
            "H3 1 extra",
            "H3",
        ] {
            assert!(Profile::parse(s).is_err(), "{s}");
        }
        assert_eq!(Profile::parse("H20 1.25\n").unwrap().gain, 1.25);
    }
    #[test]
    fn scenes_reuse_same_footfall_in_three_distinct_directions() {
        for seed in 0..12 {
            let s = Scene::new(vec![0.03; 120], seed);
            let mut bed = [0.0; BLOCK * 8];
            assert_ne!(s.directions[0], s.directions[2]);
            let mut samples = vec![];
            for (i, &channel) in s.directions.iter().enumerate() {
                s.fill(s.voice.len() + (i + 1) * RATE as usize, &mut bed);
                let sample: Vec<_> = bed
                    .chunks_exact(8)
                    .map(|f| {
                        assert!(f.iter().enumerate().all(|(n, &x)| n == channel || x == 0.0));
                        f[channel]
                    })
                    .collect();
                assert!(sample.iter().any(|&x| x != 0.0));
                if i != 0 {
                    assert_eq!(sample, samples);
                }
                samples = sample;
            }
        }
    }
    #[test]
    #[ignore = "requires SADIE II files from prepare-hrtf-profiles.py"]
    fn installed_profiles_produce_level_matched_binaural_audio() -> Result<()> {
        self_test(&Path::new(env!("CARGO_MANIFEST_DIR")).join("front-voice.wav"))
    }
}

pub(super) fn self_test(path: &Path) -> Result<()> {
    let scene = Scene::new(room::load_voice(path)?, 4);
    let (profiles, trim) = match_levels(&scene)?;
    let reference = measure(&scene, profiles[0])?.0;
    let mut lfe = [0.0; BLOCK * 8];
    for (i, frame) in lfe.chunks_exact_mut(8).enumerate() {
        frame[3] = (i as f32 - 128.0) / 2048.0;
    }
    let mut reference_bass = [[0.0; 2]; BLOCK];
    Surround::new(8, 0x63f)?.render(&lfe, Orientation::default(), &mut reference_bass)?;
    assert!(reference_bass.iter().all(|p| p[0] == p[1]) && reference_bass[0][0] != 0.0);
    for profile in profiles {
        let (energy, peak) = measure(&scene, profile)?;
        let error = 10.0 * (energy / reference).log10();
        println!(
            "{profile} gain {:.4}, matched {error:+.3} dB, peak {:.4}",
            profile.gain,
            peak * trim
        );
        assert!(error.abs() < 0.05, "{profile} {error}");
        assert!(peak * trim <= 0.0801);
        let mut bass = [[0.0; 2]; BLOCK];
        Surround::with_profile(8, 0x63f, profile)?.render(
            &lfe,
            Orientation::default(),
            &mut bass,
        )?;
        assert_eq!(bass, reference_bass, "LFE changed with {profile}");
    }
    Ok(())
}
