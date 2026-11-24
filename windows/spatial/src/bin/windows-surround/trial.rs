use super::{Device, Io, RATE, Result, SampleType, StreamMode, room::RoomSpeech};
use wasapi::{AudioClient, AudioRenderClient};

pub(super) struct SpeechTrial {
    stream: AudioClient,
    render: AudioRenderClient,
    pcm: Vec<f32>,
    bytes: Vec<u8>,
    bed: RoomSpeech,
    sent: usize,
}

impl SpeechTrial {
    pub(super) fn open(source: &Device, bed: RoomSpeech) -> Result<Self> {
        let mut stream = source.get_iaudioclient()?;
        let format = stream.get_mixformat()?;
        if format.get_nchannels() != 8
            || format.get_dwchannelmask() != 0x63f
            || format.get_samplespersec() != RATE
            || format.get_subformat()? != SampleType::Float
            || format.get_blockalign() != 32
        {
            return Err("The speech comparison needs a 48 kHz 7.1 float source".into());
        }
        stream.initialize_client(
            &format,
            Io::Render,
            StreamMode::PollingShared {
                autoconvert: false,
                buffer_duration_hns: 600_000,
            },
            None,
        )?;
        let render = stream.get_audiorenderclient()?;
        let size = stream.get_buffer_size()? as usize;
        Ok(Self {
            stream,
            render,
            pcm: vec![0.0; size * 8],
            bytes: vec![0; size * 32],
            bed,
            sent: 0,
        })
    }

    pub(super) fn duration_secs(&self) -> u64 {
        let frames = 8 * self.bed.period_frames() + 3 * RATE as usize;
        frames.div_ceil(RATE as usize) as u64
    }

    pub(super) fn start(&mut self) -> Result<()> {
        self.pump()?;
        self.stream.start_stream()?;
        Ok(())
    }

    pub(super) fn pump(&mut self) -> Result<()> {
        let frames = self.stream.get_available_space_in_frames()? as usize;
        if frames == 0 {
            return Ok(());
        }
        let pause = RATE as usize;
        let phase = 4 * self.bed.period_frames();
        let pcm = &mut self.pcm[..frames * 8];
        pcm.fill(0.0);
        // Lead in, four dry phrases, one second of silence, four room phrases.
        for (start, room) in [(pause, false), (2 * pause + phase, true)] {
            let first = start.max(self.sent);
            let end = (start + phase).min(self.sent + frames);
            if first < end {
                self.bed.fill(
                    &mut pcm[(first - self.sent) * 8..(end - self.sent) * 8],
                    first - start,
                    room,
                );
            }
        }
        let bytes = &mut self.bytes[..frames * 32];
        for (word, sample) in bytes.chunks_exact_mut(4).zip(pcm) {
            word.copy_from_slice(&sample.to_le_bytes());
        }
        self.render.write_to_device(bytes, None)?;
        self.sent += frames;
        Ok(())
    }
}
