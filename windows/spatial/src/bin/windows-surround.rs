use audionimbus::{
    AudioBufferMut, AudioBufferRef, AudioSettings, BinauralEffect, BinauralEffectParams,
    BinauralEffectSettings, Context, Direction, Hrtf, HrtfInterpolation,
};
use std::{
    error::Error,
    time::{Duration, Instant},
};
use wasapi::{
    Device, DeviceCollection, DeviceEnumerator, Direction as Io, SampleType, StreamMode, WaveFormat,
};
#[path = "windows-surround/endpoint.rs"]
mod endpoint;
#[path = "windows-surround/live_room.rs"]
mod live_room;
#[path = "windows-surround/motion.rs"]
mod motion;
use live_room::{Layout, LiveRoom};
use std::sync::{Arc, atomic::Ordering};
#[path = "windows-surround/pose.rs"]
mod pose;
use motion::Motion;
use pose::Orientation;
#[path = "windows-surround/profiles.rs"]
mod profiles;
use profiles::Profile;
#[path = "windows-surround/front.rs"]
mod front;
#[cfg(test)]
#[path = "windows-surround/localization.rs"]
mod localization;
#[path = "windows-surround/meter.rs"]
mod meter;
#[path = "windows-surround/playout.rs"]
mod playout;
#[path = "windows-surround/room.rs"]
mod room;
#[path = "windows-surround/session.rs"]
mod session;
#[cfg(test)]
#[path = "windows-surround/soak.rs"]
mod soak;
#[path = "windows-surround/timeline.rs"]
mod timeline;
#[path = "windows-surround/transport.rs"]
mod transport;
#[path = "windows-surround/trial.rs"]
mod trial;
use playout::Playout;
use timeline::{CaptureTimeline, PacketAction};

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const RATE: u32 = 48_000;
const BLOCK: usize = 256;
const OUT_FRAME_BYTES: usize = 2 * size_of::<f32>();
const TONES: [f64; 8] = [431.0, 523.0, 613.0, 83.0, 701.0, 829.0, 947.0, 1073.0];

// The vector follows the ascending-bit interleaving of WAVEFORMATEXTENSIBLE.
// LFE stays centered. For 5.1, back and side masks both represent the surround pair.
fn speaker_angles(channels: u16, mask: u32) -> Result<&'static [Option<f32>]> {
    match (channels, mask) {
        (6, 0x3f | 0x60f) => Ok(&[
            Some(-30.0),
            Some(30.0),
            Some(0.0),
            None,
            Some(-110.0),
            Some(110.0),
        ]),
        (8, 0x63f) => Ok(&[
            Some(-30.0),
            Some(30.0),
            Some(0.0),
            None,
            Some(-150.0),
            Some(150.0),
            Some(-90.0),
            Some(90.0),
        ]),
        _ => Err(format!(
            "Expected a 5.1 or 7.1 mix, got {channels} channels and mask {mask:#04x}"
        )
        .into()),
    }
}

struct Surround {
    hrtf: Hrtf,
    speakers: Vec<(usize, [f32; 3], BinauralEffect)>,
    channels: usize,
    gains: [f32; 8],
    target_gains: [f32; 8],
}

impl Surround {
    fn new(channels: u16, mask: u32) -> Result<Self> {
        Self::with_profile(channels, mask, Profile::default())
    }

    fn preferred(channels: u16, mask: u32) -> Result<Self> {
        match Profile::selected().and_then(|p| Ok((p, Self::with_profile(channels, mask, p)?))) {
            Ok((profile, renderer)) => {
                println!("Ear profile: {profile}");
                Ok(renderer)
            }
            Err(error) => {
                eprintln!("Using the built-in ear profile for this session ({error}).");
                Self::new(channels, mask)
            }
        }
    }

    fn with_profile(channels: u16, mask: u32, profile: Profile) -> Result<Self> {
        let angles = speaker_angles(channels, mask)?;
        let context = Context::default();
        let audio = AudioSettings {
            sampling_rate: RATE,
            frame_size: BLOCK as u32,
        };
        let hrtf = profile.load(&context, &audio)?;
        let settings = BinauralEffectSettings { hrtf: hrtf.clone() };
        let mut speakers = Vec::new();
        for (index, angle) in angles.iter().enumerate() {
            if let Some(angle) = angle {
                let (x, forward) = angle.to_radians().sin_cos();
                speakers.push((
                    index,
                    [x, 0.0, -forward],
                    BinauralEffect::try_new(&context, &audio, &settings)?,
                ));
            }
        }
        Ok(Self {
            hrtf,
            speakers,
            channels: channels.into(),
            gains: [1.; 8],
            target_gains: [1.; 8],
        })
    }

    fn reset(&mut self) {
        for (_, _, effect) in &mut self.speakers {
            effect.reset();
        }
    }

    fn set_layout(&mut self, layout: Layout, bits: &[u32], mask: u32) -> Result<()> {
        let defaults = speaker_angles(self.channels as u16, mask)?;
        for (index, direction, _) in &mut self.speakers {
            (*direction, self.target_gains[*index]) =
                layout.placement(bits[*index], defaults[*index].unwrap());
        }
        self.target_gains[3] = layout.placement(8, 0.).1;
        Ok(())
    }

    fn render(
        &mut self,
        pcm: &[f32],
        orientation: Orientation,
        stereo: &mut [[f32; 2]; BLOCK],
    ) -> Result<()> {
        assert_eq!(pcm.len(), self.channels * BLOCK);
        let mut mono = [0.0; BLOCK];
        let mut planes = [0.0; 2 * BLOCK];
        let mut buffer = AudioBufferMut::try_new(&mut planes, 2)?;
        let step = |i: usize| (self.target_gains[i] - self.gains[i]) / BLOCK as f32;
        let mut lfe = self.gains[3];
        let lfe_step = step(3);
        for (out, input) in stereo.iter_mut().zip(pcm.chunks_exact(self.channels)) {
            lfe += lfe_step;
            *out = [input[3] * lfe * 0.5; 2];
        }
        for (index, direction, effect) in &mut self.speakers {
            for (sample, frame) in mono.iter_mut().zip(pcm.chunks_exact(self.channels)) {
                *sample = frame[*index];
            }
            let [x, y, z] = orientation.listener_direction(*direction);
            let params = BinauralEffectParams {
                direction: Direction::new(x, y, z),
                interpolation: HrtfInterpolation::Bilinear,
                spatial_blend: 1.0,
                hrtf: self.hrtf.clone(),
                peak_delays: None,
            };
            effect.apply(&params, &AudioBufferRef::try_new(&mono, 1)?, &mut buffer)?;
            let mut gain = self.gains[*index];
            let gain_step = step(*index);
            for (i, out) in stereo.iter_mut().enumerate() {
                gain += gain_step;
                out[0] += buffer.channel(0).unwrap()[i] * gain;
                out[1] += buffer.channel(1).unwrap()[i] * gain;
            }
        }
        self.gains = self.target_gains;
        for out in stereo {
            out[0] *= 0.5;
            out[1] *= 0.5;
        }
        Ok(())
    }
}

fn describe(device: &Device) -> Result<String> {
    let fmt = device.get_iaudioclient()?.get_mixformat()?;
    Ok(format!(
        "{} | {}ch {} Hz {}-bit {:?}, mask {:#05x}",
        device.get_friendlyname()?,
        fmt.get_nchannels(),
        fmt.get_samplespersec(),
        fmt.get_bitspersample(),
        fmt.get_subformat()?,
        fmt.get_dwchannelmask()
    ))
}

fn check_ingress(device: &Device) -> Result<()> {
    let mut input = device.get_iaudioclient()?;
    let mut output = device.get_iaudioclient()?;
    let format = input.get_mixformat()?;
    let channels = format.get_nchannels() as usize;
    let frame_bytes = channels * size_of::<f32>();
    speaker_angles(format.get_nchannels(), format.get_dwchannelmask())?;
    if format.get_samplespersec() != RATE
        || format.get_subformat()? != SampleType::Float
        || format.get_blockalign() as usize != frame_bytes
    {
        return Err("The channel test needs a 48 kHz float32 surround mix".into());
    }
    let period = input.get_device_period()?.0;
    input.initialize_client(
        &format,
        Io::Capture,
        StreamMode::PollingShared {
            autoconvert: false,
            buffer_duration_hns: period,
        },
        None,
    )?;
    output.initialize_client(
        &format,
        Io::Render,
        StreamMode::EventsShared {
            autoconvert: false,
            buffer_duration_hns: period,
        },
        None,
    )?;
    let capture = input.get_audiocaptureclient()?;
    let render = output.get_audiorenderclient()?;
    let event = output.set_get_eventhandle()?;
    let mut input_bytes = vec![0; input.get_buffer_size()? as usize * frame_bytes];
    let mut output_bytes = vec![0; output.get_buffer_size()? as usize * frame_bytes];
    let mut phase = 0u32;
    let mut received = 0u32;
    let mut components = [[[0.0f64; 2]; 8]; 8];
    let radians: Vec<_> = TONES[..channels]
        .iter()
        .map(|hz| hz * std::f64::consts::TAU / f64::from(RATE))
        .collect();
    render.write_to_device(&output_bytes, None)?;
    input.start_stream()?;
    output.start_stream()?;
    let end = Instant::now() + Duration::from_secs(1);
    while Instant::now() < end {
        event.wait_for_event(200)?;
        while capture.get_next_packet_size()?.unwrap_or(0) != 0 {
            let (frames, info) = capture.read_from_device(&mut input_bytes)?;
            if frames == 0 {
                break;
            }
            received += frames;
            for (i, frame) in input_bytes[..frames as usize * frame_bytes]
                .chunks_exact(frame_bytes)
                .enumerate()
            {
                let clock = (info.index + i as u64) as f64;
                for (tone, step) in radians.iter().enumerate() {
                    let (s, c) = (clock * step).sin_cos();
                    for (channel, word) in frame.chunks_exact(4).enumerate() {
                        let sample = f32::from_le_bytes(word.try_into()?) as f64;
                        components[channel][tone][0] += sample * s;
                        components[channel][tone][1] += sample * c;
                    }
                }
            }
        }
        let frames = output.get_available_space_in_frames()? as usize;
        if frames == 0 {
            continue;
        }
        let bytes = &mut output_bytes[..frames * frame_bytes];
        for frame in bytes.chunks_exact_mut(frame_bytes) {
            for (step, word) in radians.iter().zip(frame.chunks_exact_mut(4)) {
                let sample = ((f64::from(phase) * step).sin() * 0.0004) as f32;
                word.copy_from_slice(&sample.to_le_bytes());
            }
            phase += 1;
        }
        render.write_to_device(bytes, None)?;
    }
    output.stop_stream()?;
    input.stop_stream()?;
    if received < RATE / 2 {
        return Err(format!("Only {received} frames returned through loopback").into());
    }
    for (i, bands) in components[..channels].iter().enumerate() {
        let levels: Vec<_> = bands[..channels]
            .iter()
            .map(|[s, c]| s.hypot(*c) * 2.0 / f64::from(received))
            .collect();
        let leak = levels
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, amp)| *amp)
            .fold(0.0, f64::max);
        let separation = 20.0 * (levels[i] / leak.max(1e-12)).log10();
        println!(
            "Loopback channel {i}: {} Hz, {:.1} dB over other markers",
            TONES[i], separation
        );
        if levels[i] < 1e-5 || !separation.is_finite() || separation < 20.0 {
            return Err(format!("Channel {i} failed the separation check").into());
        }
    }
    println!("Verified {channels} discrete channels over {received} captured frames");
    Ok(())
}

fn stereo_output(sink: &Device) -> Result<wasapi::AudioClient> {
    let mut output = sink.get_iaudioclient()?;
    let format = output.get_mixformat()?;
    if format.get_nchannels() != 2 || format.get_samplespersec() != RATE {
        return Err("Choose a stereo 48 kHz output".into());
    }
    let stereo = WaveFormat::new(32, 32, SampleType::Float, RATE as usize, 2, Some(3));
    output.initialize_client(
        &stereo,
        Io::Render,
        StreamMode::EventsShared {
            autoconvert: true,
            buffer_duration_hns: output.get_device_period()?.0,
        },
        None,
    )?;
    Ok(output)
}

struct RenderBed {
    renderer: Surround,
    pcm: Vec<f32>,
    filled: usize,
    stereo: [[f32; 2]; BLOCK],
    speaker_bits: Vec<u32>,
    peaks: [f32; 8],
}

impl RenderBed {
    fn new(renderer: Surround, mask: u32) -> Self {
        let pcm = vec![0.0; renderer.channels * BLOCK];
        let speaker_bits = (0..32)
            .map(|i| 1u32 << i)
            .filter(|bit| mask & bit != 0)
            .collect();
        Self {
            renderer,
            pcm,
            filled: 0,
            stereo: [[0.0; 2]; BLOCK],
            speaker_bits,
            peaks: [0.; 8],
        }
    }

    fn push(
        &mut self,
        pcm: &[u8],
        padding: usize,
        orientation: &impl Fn() -> Orientation,
        playout: &mut Playout,
    ) -> Result<u32> {
        let stride = self.renderer.channels * 4;
        let zero = [0u8; 32];
        self.peaks.fill(0.);
        for frame in std::iter::repeat_n(&zero[..stride], padding).chain(pcm.chunks_exact(stride)) {
            let start = self.filled * self.renderer.channels;
            for (channel, word) in frame.chunks_exact(4).enumerate() {
                let sample = f32::from_le_bytes(word.try_into()?);
                let value = if sample.is_finite() { sample } else { 0.0 };
                self.peaks[channel] = self.peaks[channel].max(value.abs());
                self.pcm[start + channel] = value;
            }
            self.filled += 1;
            if self.filled == BLOCK {
                self.renderer
                    .render(&self.pcm, orientation(), &mut self.stereo)?;
                playout.push(&self.stereo);
                self.filled = 0;
            }
        }
        Ok(self
            .speaker_bits
            .iter()
            .zip(self.peaks)
            .fold(
                0,
                |mask, (&bit, peak)| {
                    if peak > 1e-6 { mask | bit } else { mask }
                },
            ))
    }

    fn finish(&mut self, orientation: Orientation, playout: &mut Playout) -> Result<()> {
        if self.filled > 0 {
            self.pcm[self.filled * self.renderer.channels..].fill(0.0);
            self.renderer
                .render(&self.pcm, orientation, &mut self.stereo)?;
            playout.push(&self.stereo);
            self.filled = 0;
        }
        let remaining = self
            .renderer
            .speakers
            .iter()
            .map(|(_, _, fx)| fx.tail_size())
            .max()
            .unwrap_or(0);
        // Flush through the same overlap/add path as normal continued playback.
        // Some SDK tails differ from applying subsequent silent input blocks.
        self.pcm.fill(0.0);
        for first in (0..remaining).step_by(BLOCK) {
            self.renderer
                .render(&self.pcm, orientation, &mut self.stereo)?;
            playout.push(&self.stereo[..BLOCK.min(remaining - first)]);
        }
        self.renderer.reset();
        playout.finish_input();
        Ok(())
    }
}

fn bridge_queue_limits(
    source_period: usize,
    sink_period: usize,
    capture_frames: usize,
    output_frames: usize,
) -> (usize, usize) {
    let prime = source_period + sink_period + BLOCK;
    // Steam can deliver three packets together. Reserve room above normal playout
    // for that captured batch, one repaired timestamp gap and HRTF block rounding.
    let capture_burst = capture_frames.max(3 * source_period);
    let capacity = (prime + capture_burst + timeline::REPAIR_FRAMES as usize + BLOCK)
        .max(2048)
        .max(output_frames + BLOCK);
    (prime, capacity)
}

fn bridge(
    source: &Device,
    sink: &Device,
    seconds: Option<u64>,
    orientation: impl Fn() -> Orientation,
    mut trial: Option<trial::SpeechTrial>,
    live: Option<&LiveRoom>,
    mut poll_transport: impl FnMut() -> bool,
) -> Result<()> {
    if source.get_direction() != Io::Render || sink.get_direction() != Io::Render {
        return Err(
            "Select two playback endpoints for the surround source and stereo output".into(),
        );
    }
    if source.get_id()? == sink.get_id()? {
        return Err("Choose a separate stereo output to avoid audio feedback".into());
    }
    let mut input = source.get_iaudioclient()?;
    let format = input.get_mixformat()?;
    let renderer = if trial.is_some() {
        // The dry/room speech control matches its levels with the built-in HRTF.
        Surround::new(format.get_nchannels(), format.get_dwchannelmask())?
    } else {
        Surround::preferred(format.get_nchannels(), format.get_dwchannelmask())?
    };
    let frame_bytes = renderer.channels * size_of::<f32>();
    if format.get_samplespersec() != RATE
        || format.get_subformat()? != SampleType::Float
        || format.get_bitspersample() != 32
        || format.get_blockalign() as usize != frame_bytes
    {
        return Err("The prototype needs a 48 kHz float32 surround mix".into());
    }
    let mut bed = RenderBed::new(renderer, format.get_dwchannelmask());
    let mut layout = Layout::default();
    if let Some(room) = live {
        layout = room.initial(format.get_dwchannelmask());
        bed.renderer
            .set_layout(layout, &bed.speaker_bits, format.get_dwchannelmask())?;
        bed.renderer.gains = bed.renderer.target_gains;
    }
    let output = stereo_output(sink)?;
    let source_period = input.get_device_period()?.0;
    input.initialize_client(
        &format,
        Io::Capture,
        StreamMode::PollingShared {
            autoconvert: false,
            buffer_duration_hns: source_period,
        },
        None,
    )?;
    let capture = input.get_audiocaptureclient()?;
    let render = output.get_audiorenderclient()?;
    let event = output.set_get_eventhandle()?;
    let sink_clock = output.get_audioclock().ok().and_then(|clock| {
        let frequency = clock.get_frequency().ok()?;
        Some((clock, frequency))
    });
    let capture_frames = input.get_buffer_size()? as usize;
    let output_frames = output.get_buffer_size()? as usize;
    let period_frames = |hns: i64| (hns as u64 * u64::from(RATE)).div_ceil(10_000_000) as usize;
    let (prime, capacity) = bridge_queue_limits(
        period_frames(source_period),
        period_frames(output.get_device_period()?.0),
        capture_frames,
        output_frames,
    );
    println!(
        "Buffer: starting at {prime} frames, capacity {capacity}, capture {capture_frames}, output {output_frames}"
    );
    let mut playout = Playout::new(prime, capacity);
    let mut capture_bytes = vec![0; capture_frames * frame_bytes];
    let mut output_bytes = vec![0; output_frames * OUT_FRAME_BYTES];
    let mut stereo = vec![[0.0; 2]; output_frames];
    let mut timeline = CaptureTimeline::default();
    let mut physical = if trial.is_some() {
        Some(meter::Meter::open(sink)?)
    } else {
        None
    };
    let stop = (unsafe { windows_sys::Win32::System::Console::GetConsoleCP() } != 0)
        .then(session::ConsoleStop::install)
        .transpose()?;
    let _scheduling = session::AudioScheduling::acquire();
    let (
        mut captured,
        mut padding,
        mut trimmed,
        mut flags,
        mut resets,
        mut partial_drops,
        mut clips,
    ) = (0u64, 0u64, 0u64, 0u64, 0u64, 0u64, 0u64);
    let mut nonzero = [0u64; 2];
    let (mut all_channels, mut recent_channels) = (0u32, 0u32);
    let mut last_input = None;
    let period = Duration::from_nanos(source_period as u64 * 100);
    render.write_to_device(&output_bytes, None)?;
    input.start_stream()?;
    output.start_stream()?;
    println!("SPATIAL_READY");
    if let Some(room) = live {
        room.format
            .store(format.get_dwchannelmask(), Ordering::Release);
    }
    if let Some(feed) = trial.as_mut() {
        feed.start()?;
    }
    let start = Instant::now();
    let mut last_render_event = start;
    let mut next_clocks = start;
    let mut next_report = start + Duration::from_secs(15);
    while seconds.is_none_or(|s| start.elapsed() < Duration::from_secs(s))
        && !stop.as_ref().is_some_and(session::ConsoleStop::requested)
    {
        let event_result = event.wait_for_event(200);
        if !poll_transport() {
            break;
        }
        if let Some(room) = live
            && let Some(new) = room.layout.try_lock().ok().and_then(|g| *g)
            && new != layout
        {
            bed.renderer
                .set_layout(new, &bed.speaker_bits, format.get_dwchannelmask())?;
            layout = new;
        }
        match event_result {
            Ok(()) => last_render_event = Instant::now(),
            Err(wasapi::WasapiError::EventTimeout)
                if last_render_event.elapsed() < Duration::from_secs(2) =>
            {
                // An invalidated USB endpoint reports its error here even if it stops signaling.
                output.get_current_padding()?;
                continue;
            }
            Err(error) => return Err(error.into()),
        }
        if let Some(feed) = trial.as_mut() {
            feed.pump()?;
        }
        if let Some(meter) = physical.as_mut() {
            meter.poll(orientation().yaw_degrees())?;
        }
        // An unusually busy capture endpoint must return control to the sink each period.
        for _ in 0..32 {
            if capture.get_next_packet_size()?.unwrap_or(0) == 0 {
                break;
            }
            let (frames, info) = capture.read_from_device(&mut capture_bytes)?;
            if frames == 0 {
                break;
            }
            captured += u64::from(frames);
            flags += u64::from(info.flags.data_discontinuity);
            let action = timeline.observe(
                info.index,
                info.timestamp,
                frames,
                info.flags.data_discontinuity,
                info.flags.timestamp_error,
            );
            let (pad, skip) = match action {
                PacketAction::Keep => (0, 0),
                PacketAction::Pad(n) => {
                    padding += u64::from(n);
                    (n, 0)
                }
                PacketAction::Trim(n) => {
                    trimmed += u64::from(n);
                    (0, n)
                }
                PacketAction::Reset => {
                    resets += 1;
                    partial_drops += bed.filled as u64;
                    bed.filled = 0;
                    bed.renderer.reset();
                    playout.reset();
                    (0, 0)
                }
            };
            if skip == frames {
                continue;
            }
            let now = Instant::now();
            last_input = Some(now);
            if now >= next_clocks && !info.flags.timestamp_error {
                if let Some((clock, frequency)) = &sink_clock
                    && let Ok(position) = clock.get_position()
                {
                    playout.observe_clocks((info.index, info.timestamp), position, *frequency);
                }
                next_clocks = now + Duration::from_secs(1);
            }
            let bytes = &capture_bytes[skip as usize * frame_bytes..frames as usize * frame_bytes];
            let mask = bed.push(bytes, pad as usize, &orientation, &mut playout)?;
            all_channels |= mask;
            recent_channels |= mask;
            if let Some(room) = live {
                room.publish_peaks(bed.peaks);
            }
        }
        let frames = output.get_available_space_in_frames()? as usize;
        if frames > 0 {
            // Loopback can stop returning packets when the last producer closes.
            // Drain the final incomplete HRTF block before the playout buffer runs out.
            if last_input.is_some_and(|t| t.elapsed() >= period) && playout.depth() < frames + 24 {
                bed.finish(orientation(), &mut playout)?;
                last_input = None;
            }
            playout.pull(&mut stereo[..frames]);
            let packet = &mut output_bytes[..frames * OUT_FRAME_BYTES];
            for (pair, bytes) in stereo[..frames]
                .iter()
                .zip(packet.chunks_exact_mut(OUT_FRAME_BYTES))
            {
                for (channel, (&sample, dst)) in
                    pair.iter().zip(bytes.chunks_exact_mut(4)).enumerate()
                {
                    nonzero[channel] += u64::from(sample != 0.0);
                    clips += u64::from(sample.abs() > 1.0);
                    dst.copy_from_slice(&sample.clamp(-1.0, 1.0).to_le_bytes());
                }
            }
            render.write_to_device(packet, None)?;
        }
        if live.is_none() && Instant::now() >= next_report {
            println!(
                "At {:.0}s: source mask {recent_channels:#05x}, queue {} frames, correction {:+.1} ppm, gap fill {padding}, resets {resets}, underruns {}, discarded {} (capacity {}, reset queue {}, reset partial {partial_drops})",
                start.elapsed().as_secs_f64(),
                playout.depth(),
                playout.correction_ppm(),
                playout.stats.underruns,
                playout.stats.discarded + partial_drops,
                playout.stats.overflow,
                playout.stats.discarded - playout.stats.overflow
            );
            recent_channels = 0;
            next_report = Instant::now() + Duration::from_secs(15);
        }
    }
    output.stop_stream()?;
    input.stop_stream()?;
    if let Some(meter) = physical {
        meter.finish()?;
    }
    println!(
        "Captured {captured} frames, source mask {all_channels:#05x}, nonzero L/R samples {}/{}, silence fill {} ({} at startup), underruns {}, discarded {} (capacity {}, reset queue {}, reset partial {partial_drops}), discontinuity flags {flags}, gap fill {padding}, overlap trimmed {trimmed}, actual resets {resets}, clipped samples {clips}, final queue {}, correction {:+.1} ppm",
        nonzero[0],
        nonzero[1],
        playout.stats.silence,
        playout.stats.startup_silence,
        playout.stats.underruns,
        playout.stats.discarded + partial_drops,
        playout.stats.overflow,
        playout.stats.discarded - playout.stats.overflow,
        playout.depth(),
        playout.correction_ppm()
    );
    Ok(())
}

#[test]
fn coalesced_steam_startup_keeps_repaired_frames_and_original_start_time() -> Result<()> {
    // Position and batch boundaries from ingress-review/timeline-probe.txt.
    // The trace contains no PCM. A known LFE signal makes every drop testable.
    let (prime, capacity) = bridge_queue_limits(480, 480, 480, 480);
    assert_eq!(prime, 1216);
    let simulate = |limit| -> Result<(usize, u64, Vec<f32>)> {
        let mut p = Playout::new(prime, limit);
        let mut bed = RenderBed::new(Surround::new(8, 0x63f)?, 0x63f);
        let mut stamps = CaptureTimeline::default();
        let mut wave = Vec::new();
        let mut first = None;
        for (event, batch) in [
            &[(0, 480, true), (480, 480, false), (960, 96, true)][..],
            &[],
            &[(1440, 480, false), (1920, 480, false), (2400, 96, true)],
            &[(2880, 480, false)],
        ]
        .into_iter()
        .enumerate()
        {
            for &(index, frames, flag) in batch {
                let pad = match stamps.observe(
                    index,
                    (index * 10_000_000 / u64::from(RATE)) + 1,
                    frames,
                    flag,
                    false,
                ) {
                    PacketAction::Keep => 0,
                    PacketAction::Pad(n) => n as usize,
                    action => panic!("Unexpected {action:?}"),
                };
                let mut packet = vec![0; frames as usize * 32];
                for (i, frame) in packet.chunks_exact_mut(32).enumerate() {
                    frame[12..16].copy_from_slice(
                        &(((index as usize + i + 1) as f32) / 4096.0).to_le_bytes(),
                    );
                }
                bed.push(&packet, pad, &Orientation::default, &mut p)?;
            }
            let mut out = [[0.0; 2]; 480];
            p.pull(&mut out);
            if out.iter().any(|frame| frame[0] != 0.0) {
                first.get_or_insert(event);
            }
            if first.is_some() {
                wave.extend(out.into_iter().map(|pair| pair[0]));
            }
        }
        let startup_overflow = p.stats.overflow;
        bed.finish(Orientation::default(), &mut p)?;
        let mut rest = vec![[0.0; 2]; p.depth()];
        p.pull(&mut rest);
        wave.extend(rest.iter().map(|pair| pair[0]));
        Ok((first.unwrap(), startup_overflow, wave))
    };
    let (old_first, old_loss, _) = simulate(2048)?;
    let (first, loss, output) = simulate(capacity)?;
    assert!(old_loss > 0);
    assert_eq!((first, loss), (old_first, 0));
    for (i, value) in output.iter().copied().enumerate().take(3360) {
        let absent = (1056..1440).contains(&i) || (2496..2880).contains(&i);
        assert_eq!(
            value,
            if absent {
                0.0
            } else {
                (i + 1) as f32 / 16384.0
            }
        );
    }
    Ok(())
}

#[test]
fn idle_drains_partial_front_channel_and_the_binaural_tail() -> Result<()> {
    let mut bed = RenderBed::new(Surround::new(8, 0x63f)?, 0x63f);
    let mut playout = Playout::new(1024, 4096);
    let mut samples = vec![0.0f32; (BLOCK + 249) * 8];
    samples[BLOCK * 8 + 248 * 8 + 1] = 0.2;
    let bytes: Vec<_> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    assert_eq!(bed.push(&bytes, 0, &Orientation::default, &mut playout)?, 2);
    assert_eq!(bed.filled, 249);
    bed.finish(Orientation::default(), &mut playout)?;
    let mut actual = vec![[0.0; 2]; (playout.depth().div_ceil(BLOCK) + 1) * BLOCK];
    playout.pull(&mut actual);
    assert_eq!(
        (bed.filled, playout.depth(), playout.stats.underruns),
        (0, 0, 0)
    );

    // Rendering explicit silence after the last real frame is the reference for the SDK tail.
    samples.resize(actual.len() * 8, 0.0);
    let mut reference = Surround::new(8, 0x63f)?;
    let mut expected = [[0.0; 2]; BLOCK];
    for (idx, (input, output)) in samples
        .chunks_exact(BLOCK * 8)
        .zip(actual.chunks_exact(BLOCK))
        .enumerate()
    {
        reference.render(input, Orientation::default(), &mut expected)?;
        for (received, predicted) in output.iter().flatten().zip(expected.iter().flatten()) {
            assert!(
                (received - predicted).abs() < 2e-5,
                "block {idx}, received {received}, reference {predicted}"
            );
        }
    }
    assert!(actual[BLOCK * 2..].iter().flatten().any(|v| v.abs() > 1e-5));
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if let [flag, serial, action, initial @ ..] = &args[..]
        && flag == "--dongle-serial"
        && (initial.is_empty() || matches!(action.as_str(), "listen-managed" | "listen-managed-return"))
        && matches!(
            action.as_str(),
            "listen-managed" | "listen-managed-return" | "calibrate-managed" | "compare-managed"
        )
    {
        if action == "calibrate-managed" {
            let stop = session::stop_on_input(std::io::stdin())?;
            let result = motion::calibrate(
                serial,
                |phase, seconds| println!("CAL_{phase}_{seconds}"),
                || stop.load(std::sync::atomic::Ordering::Acquire),
            );
            if let Err(error) = &result {
                let retry = match error.to_string().as_str() {
                    "Hold your head still facing forward before looking up" => "CAL_RETRY_FACE",
                    "Hold your head still after returning to center"
                    | "Return to the same forward direction after looking up" => "CAL_RETRY_RETURN",
                    "Look straight up without turning left or right"
                    | "Look straight up without tilting to one side"
                    | "Look up farther and hold the position" => "CAL_RETRY_UP",
                    motion::NO_PAIRED_MOTION => "CAL_RETRY_MOTION",
                    _ => "CAL_RETRY_CONNECTION",
                };
                println!("{retry}");
            }
            if let Ok(Some(bud)) = &result {
                let saved = match bud {
                    motion::PrimaryBud::Left => "CAL_SAVED_LEFT",
                    motion::PrimaryBud::Right => "CAL_SAVED_RIGHT",
                };
                println!("{saved}");
                println!("CAL_SAVED");
            }
            return result.map(|_| ());
        }
        wasapi::initialize_mta().ok()?;
        let enumerator = DeviceEnumerator::new()?;
        if action == "compare-managed" {
            let devices = enumerator.get_device_collection(Io::Render)?;
            let sink = find_dongle(&devices, Some(serial))?;
            let motion = open_motion(&sink)?;
            let speech = std::env::current_exe()?.with_file_name("front-voice.wav");
            return profiles::compare(&sink, &motion, &speech, true);
        }
        let room = LiveRoom::open(std::io::stdin(), initial)?;
        // On USB personality changes the dongle HID arrives before its WASAPI endpoint.
        // Re-enumerate inside one process, without reopening a stream or changing a default.
        let until = Instant::now() + Duration::from_secs(8);
        let (source, sink) = loop {
            if room.stop.load(Ordering::Acquire) {
                return Ok(());
            }
            let endpoints = || -> Result<_> {
                let devices = enumerator.get_device_collection(Io::Render)?;
                find_pair(&devices, Some(serial))
            };
            match endpoints() {
                Ok(pair) => break pair,
                Err(_) if action == "listen-managed-return" && Instant::now() < until => {
                    std::thread::sleep(Duration::from_millis(200));
                }
                Err(error) => return Err(error),
            }
        };
        return track(source, sink, None, Some(room));
    }
    if let [flag, serial, action] = &args[..]
        && flag == "--dongle-serial"
        && matches!(action.as_str(), "listen" | "calibrate" | "compare-profiles")
    {
        if let Err(error) = run(&args[2..], Some(serial)) {
            eprintln!("{error}\nPress Enter to close.");
            let _ = std::io::stdin().read_line(&mut String::new());
            return Err(error);
        }
        return Ok(());
    }
    run(&args, None)
}

fn run(args: &[String], serial: Option<&str>) -> Result<()> {
    const USAGE: &str = "Usage: windows-surround --dongle-serial SERIAL listen|calibrate|compare-profiles\n       windows-surround listen [SECONDS] | calibrate [DONGLE] | compare-profiles [DONGLE [SPEECH.wav]] | reset-profile | tracked SOURCE DONGLE [SECONDS] | list | self-test | check-ingress SOURCE | bridge SOURCE SINK SECONDS YAW | front DONGLE SECONDS | motion DONGLE SECONDS | compare SOURCE DONGLE SPEECH.wav";
    if args == ["help"] || args == ["--help"] {
        println!("{USAGE}");
        return Ok(());
    }
    if args == ["reset-profile"] {
        Profile::default().set_selected()?;
        println!("The built-in ear profile is selected.");
        return Ok(());
    }
    if args == ["self-test"] {
        verify_renderer()?;
        let exe = std::env::current_exe()?;
        if exe.with_file_name("H3.sofa").is_file() && exe.with_file_name("H20.sofa").is_file() {
            profiles::self_test(&exe.with_file_name("front-voice.wav"))?;
        }
        return Ok(());
    }
    wasapi::initialize_mta().ok()?;
    let enumerator = DeviceEnumerator::new()?;
    let devices = enumerator.get_device_collection(Io::Render)?;
    let select = |id: &str| -> Result<Device> {
        let device = match id.parse::<u32>() {
            Ok(index) => devices.get_device_at_index(index)?,
            Err(_) => enumerator.get_device(id)?,
        };
        if device.get_direction() != Io::Render {
            return Err("Choose a playback endpoint from the list".into());
        }
        Ok(device)
    };
    match args {
        [command] if command == "list" => {
            for i in 0..devices.get_nbr_devices()? {
                let device = devices.get_device_at_index(i)?;
                match describe(&device) {
                    Ok(info) => println!("{i}: {info} | {}", device.get_id()?),
                    Err(error) => eprintln!("{i}: {error}"),
                }
            }
        }
        [command, source] if command == "check-ingress" => {
            let source = select(source)?;
            println!("Input: {}", describe(&source)?);
            check_ingress(&source)?;
        }
        [command, source, sink, seconds, yaw] if command == "bridge" => {
            let seconds = seconds.parse()?;
            let yaw: f32 = yaw.parse()?;
            if !(1..=30).contains(&seconds) || !yaw.is_finite() {
                return Err("Supply 1–30 seconds and a finite head yaw in degrees".into());
            }
            let source = select(source)?;
            let sink = select(sink)?;
            println!(
                "Input: {}\nOutput: {}",
                describe(&source)?,
                describe(&sink)?
            );
            let orientation = Orientation::from_yaw(yaw);
            bridge(
                &source,
                &sink,
                Some(seconds),
                || orientation,
                None,
                None,
                || true,
            )?;
        }
        [command, source, sink, rest @ ..] if command == "tracked" && rest.len() <= 1 => {
            track(
                select(source)?,
                select(sink)?,
                run_duration(rest.first())?,
                None,
            )?;
        }
        [command, rest @ ..] if command == "listen" && rest.len() <= 1 => {
            let (source, sink) = find_pair(&devices, serial)?;
            track(source, sink, run_duration(rest.first())?, None)?;
        }
        [command, rest @ ..]
            if (command == "calibrate" && rest.len() <= 1)
                || (command == "compare-profiles" && rest.len() <= 2) =>
        {
            let sink = match rest.first() {
                Some(id) => select(id)?,
                None => find_dongle(&devices, serial)?,
            };
            if command == "calibrate" {
                motion::calibrate(&endpoint::dongle_serial(&sink)?, |_, _| {}, || false)?;
            } else {
                let speech = match rest.get(1) {
                    Some(file) => std::path::PathBuf::from(file),
                    None => std::env::current_exe()?.with_file_name("front-voice.wav"),
                };
                let motion = open_motion(&sink)?;
                profiles::compare(&sink, &motion, &speech, false)?;
            }
        }
        [command, source, sink, speech] if command == "compare" => {
            let (source, sink) = (select(source)?, select(sink)?);
            let trial = trial::SpeechTrial::open(&source, room::RoomSpeech::load(speech)?)?;
            let seconds = trial.duration_secs();
            println!(
                "Speech trial: {seconds} seconds.\nInput: {}\nOutput: {}",
                describe(&source)?,
                describe(&sink)?
            );
            let motion = open_motion(&sink)?;
            let result = bridge(
                &source,
                &sink,
                Some(seconds),
                || motion.orientation(),
                Some(trial),
                None,
                || true,
            );
            println!("{}", motion.stats());
            result?;
        }
        [command, sink, seconds] if command == "front" || command == "motion" => {
            let sink = select(sink)?;
            let seconds = listen_seconds(seconds)?;
            let motion = open_motion(&sink)?;
            if command == "front" {
                front::listen(&sink, seconds, &motion)?;
            } else {
                std::thread::sleep(Duration::from_secs(seconds));
            }
            println!("{}", motion.stats());
        }
        _ => return Err(USAGE.into()),
    }
    Ok(())
}

fn find_pair(devices: &DeviceCollection, serial: Option<&str>) -> Result<(Device, Device)> {
    let mut sources = Vec::new();
    for i in 0..devices.get_nbr_devices()? {
        let dev = devices.get_device_at_index(i)?;
        if dev
            .get_interface_friendlyname()
            .is_ok_and(|name| name.eq_ignore_ascii_case("Steam Streaming Speakers"))
        {
            sources.push(dev);
        }
    }
    if sources.len() != 1 {
        return Err(format!("Found {} Steam speaker endpoints. Use list and tracked SOURCE DONGLE to select your pair.", sources.len()).into());
    }
    Ok((sources.remove(0), find_dongle(devices, serial)?))
}

fn find_dongle(devices: &DeviceCollection, serial: Option<&str>) -> Result<Device> {
    let mut sinks = Vec::new();
    for i in 0..devices.get_nbr_devices()? {
        let dev = devices.get_device_at_index(i)?;
        if endpoint::dongle_serial(&dev)
            .is_ok_and(|found| serial.is_none_or(|requested| found.eq_ignore_ascii_case(requested)))
        {
            sinks.push(dev);
        }
    }
    if sinks.len() != 1 {
        if serial.is_some() {
            return Err("The selected dongle has no unique Windows audio output. Connect its AirPods and try again.".into());
        }
        return Err(format!(
            "Found {} USB AirPods outputs. Use list and select the dongle by its endpoint ID.",
            sinks.len()
        )
        .into());
    }
    Ok(sinks.remove(0))
}

fn run_duration(arg: Option<&String>) -> Result<Option<u64>> {
    arg.map(|s| {
        let secs = s.parse::<u64>()?;
        (1..=86_400)
            .contains(&secs)
            .then_some(secs)
            .ok_or_else(|| "Choose 1–86400 seconds, or omit the time to listen until Ctrl+C".into())
    })
    .transpose()
}

fn track(
    source: Device,
    sink: Device,
    seconds: Option<u64>,
    managed: Option<Arc<LiveRoom>>,
) -> Result<()> {
    let mix = source.get_iaudioclient()?.get_mixformat()?;
    speaker_angles(mix.get_nchannels(), mix.get_dwchannelmask())?;
    let serial = endpoint::dongle_serial(&sink)?;
    println!(
        "Input: {}\nOutput: {}\nRoute the game or player to the input above. Press Ctrl+C to stop.",
        describe(&source)?,
        describe(&sink)?
    );
    let monitor = transport::Transport::watch(&source)?;
    let motion = Motion::open(&serial, managed.is_some())?;
    if let Some(room) = &managed {
        motion.monitor_room(room.clone())?;
    }
    // Start audio even when the first motion report depends on the stream starting.
    println!("Face the screen on startup. Playback resumes centered on your current direction.");
    let mut reanchors = 0u64;
    let result = bridge(
        &source,
        &sink,
        seconds,
        || motion.orientation(),
        None,
        managed.as_deref(),
        || {
            if managed
                .as_ref()
                .is_some_and(|room| room.stop.load(Ordering::Acquire))
            {
                return false;
            }
            let first_play = monitor.take_start();
            let resumed = monitor.take_resume();
            if first_play || resumed {
                motion.reanchor_on_resume();
                if resumed {
                    reanchors += 1;
                    if managed.is_none() {
                        println!("Playback resumed. Setting forward from a fresh head position.");
                    }
                }
            }
            true
        },
    );
    println!("{}; {reanchors} playback resume anchors", motion.stats());
    result
}

fn listen_seconds(arg: &str) -> Result<u64> {
    let seconds = arg.parse()?;
    (1..=120)
        .contains(&seconds)
        .then_some(seconds)
        .ok_or_else(|| "Choose 1–120 seconds".into())
}

fn open_motion(sink: &Device) -> Result<Motion> {
    let serial = endpoint::dongle_serial(sink)?;
    println!(
        "Face the screen to set the starting direction. Waiting for motion from dongle {serial}."
    );
    let motion = Motion::open(&serial, false)?;
    motion.wait_ready(Duration::from_secs(6))?;
    Ok(motion)
}

fn verify_renderer() -> Result<()> {
    for (channels, mask) in [(6, 0x3f), (6, 0x60f), (8, 0x63f)] {
        let mut renderer = Surround::new(channels, mask)?;
        let mut input = vec![0.0; BLOCK * channels as usize];
        let mut output = [[0.0; 2]; BLOCK];
        let cases = [
            (0, 0.0),
            (1, 0.0),
            (2, 45.0),
            (2, -45.0),
            (3, 90.0),
            (4, 0.0),
            (5, 0.0),
            (6, 0.0),
            (7, 0.0),
        ];
        for (speaker, yaw) in cases
            .into_iter()
            .filter(|(index, _)| *index < channels as usize)
        {
            renderer.reset();
            input.fill(0.0);
            input[speaker] = 0.1;
            renderer.render(&input, Orientation::from_yaw(yaw), &mut output)?;
            let [l, r] = output
                .iter()
                .fold([0.0, 0.0], |[l, r], x| [l + x[0] * x[0], r + x[1] * x[1]]);
            let ratio = 10.0 * (l / r).log10();
            assert!(ratio.is_finite());
            match (speaker, yaw) {
                (0 | 4 | 6, _) | (2, 45.0) => assert!(ratio > 2.0),
                (1 | 5 | 7, _) | (2, -45.0) => assert!(ratio < -2.0),
                (3, _) => assert!(ratio.abs() < 0.001),
                _ => unreachable!(),
            }
            println!(
                "{channels}ch {mask:#05x} channel {speaker} yaw {yaw:+}°, left/right {ratio:+.2} dB"
            );
        }
    }
    Ok(())
}
