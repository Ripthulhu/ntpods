use super::{codec::Mode, protocol::invalid, scheduling::AudioScheduling};
use std::io::{self, Read, Write};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub struct Encoder {
    child: Child,
    input: Option<SyncSender<Vec<u8>>>,
    output: Receiver<io::Result<Vec<u8>>>,
    threads: Vec<JoinHandle<()>>,
    pcm_bytes: usize,
    config: Vec<u8>,
}

fn read_header(stream: &mut impl Read, mode: Mode) -> io::Result<Vec<u8>> {
    let mut header = [0; 12];
    stream.read_exact(&mut header)?;
    let asc_len = u16::from_le_bytes([header[10], header[11]]) as usize;
    let magic = match mode {
        Mode::Aac => b"NPAAC001",
        Mode::Eld => b"NPELD001",
    };
    if &header[..8] != magic
        || u16::from_le_bytes([header[8], header[9]]) as usize != mode.frame_samples()
        || !(1..=64).contains(&asc_len)
    {
        return Err(invalid("Invalid playback encoder header"));
    }
    let mut asc = vec![0; asc_len];
    stream.read_exact(&mut asc)?;
    let valid = match mode {
        Mode::Aac => asc == [0x11, 0x90, 0x56, 0xe5, 0],
        // Plain ELD or downsampled SBR, both 480 samples at 48 kHz stereo.
        Mode::Eld => {
            asc == [0xf8, 0xe6, 0x50, 0]
                || asc == [0xf8, 0xe6, 0x51, 0x3a, 0xe0, 0x9e, 0]
                || asc == [0xf8, 0xe6, 0x51, 0x36, 0xe0, 0x9e, 0]
        }
    };
    if !valid {
        return Err(invalid("Encoder selected an unsupported configuration"));
    }
    Ok(asc)
}

fn maximum_au(mode: Mode, bitrate: u32) -> io::Result<usize> {
    mode.bitrate(
        ntpods_ipc::PlaybackBitrate {
            variable: false,
            best_effort: false,
            kbps: bitrate / 1000,
        },
        0,
    )?;
    if bitrate % 1000 != 0 {
        return Err(invalid("Unsupported playback bitrate"));
    }
    Ok(match mode {
        Mode::Aac => bitrate as usize * 1024 / (48000 * 8) - 8,
        Mode::Eld => (bitrate as usize).div_ceil(800) + 1,
    })
}

impl Encoder {
    pub fn start(mode: Mode, bitrate: u32, variable: bool) -> io::Result<Self> {
        let name = match mode {
            Mode::Aac => "ntpods-aac-encoder.exe",
            Mode::Eld => "ntpods-eld-encoder.exe",
        };
        let executable = std::env::current_exe()?.with_file_name(name);
        Self::start_at(&executable, mode, bitrate, variable)
    }

    pub fn config(&self) -> &[u8] {
        &self.config
    }

    fn start_at(executable: &Path, mode: Mode, bitrate: u32, variable: bool) -> io::Result<Self> {
        let maximum = maximum_au(mode, bitrate)?;
        if variable && (mode != Mode::Eld || bitrate != 256_000) {
            return Err(invalid("Unsupported variable bitrate configuration"));
        }
        let mut command = Command::new(executable);
        if variable {
            command.arg("--vbr");
        } else {
            command.args(["--bitrate", &bitrate.to_string()]);
        }
        let mut child = command
            .creation_flags(0x0800_0000)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let mut stdin = child.stdin.take().unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let (input, pending) = mpsc::sync_channel::<Vec<u8>>(2);
        let (result, output) = mpsc::sync_channel(12);
        let (ready_tx, ready) = mpsc::sync_channel(1);
        let writer = thread::spawn(move || {
            let _scheduling = AudioScheduling::acquire();
            while let Ok(pcm) = pending.recv() {
                if stdin.write_all(&pcm).and_then(|_| stdin.flush()).is_err() {
                    break;
                }
            }
        });
        let reader = thread::spawn(move || {
            let _scheduling = AudioScheduling::acquire();
            let header = read_header(&mut stdout, mode);
            let success = header.is_ok();
            if ready_tx.send(header).is_err() || !success {
                return;
            }
            loop {
                let frame = (|| {
                    let mut length = [0; 2];
                    stdout.read_exact(&mut length)?;
                    let size = u16::from_le_bytes(length) as usize;
                    if size == 0 || size > maximum {
                        return Err(invalid("Oversized encoder frame"));
                    }
                    let mut frame = vec![0; size];
                    stdout.read_exact(&mut frame)?;
                    Ok(frame)
                })();
                let success = frame.is_ok();
                if result.try_send(frame).is_err() || !success {
                    break;
                }
            }
        });
        let mut encoder = Self {
            child,
            input: Some(input),
            output,
            threads: vec![writer, reader],
            pcm_bytes: mode.frame_samples() * 4,
            config: Vec::new(),
        };
        encoder.config = ready.recv_timeout(Duration::from_secs(3)).map_err(|_| {
            io::Error::new(io::ErrorKind::TimedOut, "Playback encoder did not start")
        })??;
        Ok(encoder)
    }

    pub fn write(&self, pcm: Vec<u8>) -> io::Result<()> {
        if pcm.len() != self.pcm_bytes {
            return Err(invalid("Invalid stereo PCM frame length"));
        }
        self.input
            .as_ref()
            .unwrap()
            .try_send(pcm)
            .map_err(|_| io::Error::other("Playback encoder is not consuming audio"))
    }

    pub fn read(&self) -> io::Result<Option<Vec<u8>>> {
        match self.output.try_recv() {
            Ok(frame) => frame.map(Some),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "Playback encoder exited",
            )),
        }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        self.input.take();
        let _ = self.child.kill();
        let _ = self.child.wait();
        for worker in self.threads.drain(..) {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::io::{AsHandle, AsRawHandle};
    use std::time::Instant;
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
    #[test]
    fn rejects_wrong_encoder_format_and_short_header() {
        let valid = [
            b"NPELD001".as_slice(),
            &[0xe0, 1, 7, 0, 0xf8, 0xe6, 0x51, 0x3a, 0xe0, 0x9e, 0],
        ]
        .concat();
        assert!(read_header(&mut valid.as_slice(), Mode::Eld).is_ok());
        for size in 0..valid.len() {
            assert!(read_header(&mut &valid[..size], Mode::Eld).is_err());
        }
        let mut bad = valid;
        bad[8] = 0;
        assert!(read_header(&mut bad.as_slice(), Mode::Eld).is_err());
        bad[8] = 0xe0;
        bad[14] = 0x50;
        assert!(read_header(&mut bad.as_slice(), Mode::Eld).is_err());
    }

    #[test]
    fn aac_header_and_frame_budget_are_codec_specific() {
        let header = [
            b"NPAAC001".as_slice(),
            &[0, 4, 5, 0, 0x11, 0x90, 0x56, 0xe5, 0],
        ]
        .concat();
        assert_eq!(
            read_header(&mut header.as_slice(), Mode::Aac).unwrap(),
            [0x11, 0x90, 0x56, 0xe5, 0]
        );
        assert!(read_header(&mut header.as_slice(), Mode::Eld).is_err());
        for size in 0..header.len() {
            assert!(read_header(&mut &header[..size], Mode::Aac).is_err());
        }
        assert_eq!(maximum_au(Mode::Aac, 96000).unwrap(), 248);
        assert_eq!(maximum_au(Mode::Aac, 512000).unwrap(), 1357);
        assert_eq!(maximum_au(Mode::Eld, 128000).unwrap(), 161);
        assert_eq!(maximum_au(Mode::Eld, 129000).unwrap(), 163);
        assert_eq!(maximum_au(Mode::Eld, 256000).unwrap(), 321);
        assert!(maximum_au(Mode::Aac, 95999).is_err());
        assert!(maximum_au(Mode::Eld, 257000).is_err());
    }

    #[test]
    fn plain_eld_header_keeps_rate_channels_and_frame_length_fixed() {
        let header = [
            b"NPELD001".as_slice(),
            &[0xe0, 1, 4, 0, 0xf8, 0xe6, 0x50, 0],
        ]
        .concat();
        assert_eq!(
            read_header(&mut header.as_slice(), Mode::Eld).unwrap(),
            [0xf8, 0xe6, 0x50, 0]
        );
        assert!(read_header(&mut header.as_slice(), Mode::Aac).is_err());
        for (index, value) in [(13, 0xe8), (14, 0x30), (14, 0x40), (15, 1)] {
            let mut bad = header.clone();
            bad[index] = value;
            assert!(read_header(&mut bad.as_slice(), Mode::Eld).is_err());
        }
        let mut trailing = header;
        trailing[10] = 5;
        trailing.push(0);
        assert!(read_header(&mut trailing.as_slice(), Mode::Eld).is_err());
    }

    #[test]
    fn missing_worker_returns_an_error() {
        let nonexistent = std::env::current_exe()
            .unwrap()
            .with_file_name("missing-ntpods-eld-encoder.exe");
        assert!(Encoder::start_at(&nonexistent, Mode::Eld, 256_000, false).is_err());
        for (mode, bitrate) in [(Mode::Aac, 256_000), (Mode::Eld, 192_000)] {
            let error = Encoder::start_at(&nonexistent, mode, bitrate, true)
                .err()
                .unwrap();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        }
    }

    #[test]
    #[ignore = "requires the local FDK worker via NTPODS_ELD_ENCODER_TEST_PATH"]
    fn accepts_every_exposed_worker_bitrate() {
        let executable =
            std::path::PathBuf::from(std::env::var_os("NTPODS_ELD_ENCODER_TEST_PATH").unwrap());
        for kbps in 128..=256 {
            drop(
                Encoder::start_at(&executable, Mode::Eld, kbps * 1000, false)
                    .unwrap_or_else(|error| panic!("{kbps} kbps: {error}")),
            );
        }
    }

    #[test]
    #[ignore = "requires the local FDK worker via NTPODS_ELD_ENCODER_TEST_PATH"]
    fn streams_real_worker_and_drops_it() {
        for mode in [Mode::Aac, Mode::Eld] {
            stream_and_drop(mode, false);
            drop_with_blocked_pipes(mode);
        }
        stream_and_drop(Mode::Eld, true);
    }

    fn drop_with_blocked_pipes(mode: Mode) {
        #[link(name = "ntdll")]
        extern "system" {
            fn NtSuspendProcess(process: windows_sys::Win32::Foundation::HANDLE) -> i32;
        }
        let variable = match mode {
            Mode::Aac => "NTPODS_AAC_ENCODER_TEST_PATH",
            Mode::Eld => "NTPODS_ELD_ENCODER_TEST_PATH",
        };
        let executable = std::path::PathBuf::from(std::env::var_os(variable).expect(variable));
        let encoder = Encoder::start_at(&executable, mode, 256000, false).unwrap();
        let process = encoder.child.as_handle().try_clone_to_owned().unwrap();
        // Freeze only this test's child so its stdin stops draining and stdout stays open.
        assert_eq!(
            unsafe { NtSuspendProcess(process.as_raw_handle().cast()) },
            0
        );
        let mut blocked = false;
        for _ in 0..100 {
            if encoder.write(vec![0; mode.frame_samples() * 4]).is_err() {
                blocked = true;
                break;
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert!(blocked, "Encoder input must be bounded");
        let (finished, completion) = mpsc::sync_channel(1);
        let cleanup = thread::spawn(move || {
            drop(encoder);
            let _ = finished.send(());
        });
        completion
            .recv_timeout(Duration::from_secs(3))
            .expect("Blocked encoder pipes survived Drop");
        cleanup.join().unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(process.as_raw_handle().cast(), 1000) },
            0
        );
    }

    fn stream_and_drop(mode: Mode, variable: bool) {
        let worker_path_variable = match mode {
            Mode::Aac => "NTPODS_AAC_ENCODER_TEST_PATH",
            Mode::Eld => "NTPODS_ELD_ENCODER_TEST_PATH",
        };
        let executable = std::path::PathBuf::from(
            std::env::var_os(worker_path_variable).expect(worker_path_variable),
        );
        assert!(executable.is_absolute());
        let encoder = Encoder::start_at(&executable, mode, 256_000, variable)
            .expect("Worker must return a supported ASC");
        let process = encoder.child.as_handle().try_clone_to_owned().unwrap();
        let mut units = 0;
        for frame in 0..100 {
            let mut pcm = Vec::with_capacity(mode.frame_samples() * 4);
            for sample in 0..mode.frame_samples() {
                let phase = std::f64::consts::TAU * (frame * mode.frame_samples() + sample) as f64
                    / 48_000.0;
                for frequency in [440.0, 880.0] {
                    pcm.extend_from_slice(
                        &(((phase * frequency).sin() * 6000.0) as i16).to_le_bytes(),
                    );
                }
            }
            encoder.write(pcm).expect("Worker must consume paced PCM");
            thread::sleep(Duration::from_secs_f64(
                mode.frame_samples() as f64 / 48000.0,
            ));
            while let Some(unit) = encoder.read().expect("Worker must keep its output open") {
                assert!(!unit.is_empty() && unit.len() <= maximum_au(mode, 256000).unwrap());
                units += 1;
            }
            if frame == 19 {
                assert!(units > 0, "Encoder buffered audio instead of streaming it");
            }
        }
        let deadline = Instant::now() + Duration::from_secs(1);
        let expected = match mode {
            Mode::Aac => 99,
            Mode::Eld => 100,
        };
        while units < expected && Instant::now() < deadline {
            if let Some(unit) = encoder.read().unwrap() {
                assert!(!unit.is_empty() && unit.len() <= maximum_au(mode, 256000).unwrap());
                units += 1;
            } else {
                thread::sleep(Duration::from_millis(1));
            }
        }
        assert_eq!(units, expected, "Unexpected encoder delay before EOF");
        assert_eq!(
            unsafe { WaitForSingleObject(process.as_raw_handle().cast(), 0) },
            258
        );
        let (finished, completion) = mpsc::sync_channel(1);
        let shutdown = thread::spawn(move || {
            drop(encoder);
            let _ = finished.send(());
        });
        completion
            .recv_timeout(Duration::from_secs(3))
            .expect("Encoder Drop must not deadlock");
        shutdown.join().unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(process.as_raw_handle().cast(), 1000) },
            0,
            "Encoder child survived Drop"
        );
    }
}
