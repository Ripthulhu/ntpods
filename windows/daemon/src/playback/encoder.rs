use super::protocol::{invalid, MAX_AU, PCM_BYTES};
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
}

fn read_header(stream: &mut impl Read) -> io::Result<()> {
    let mut header = [0; 12];
    stream.read_exact(&mut header)?;
    let asc_len = u16::from_le_bytes([header[10], header[11]]) as usize;
    if &header[..8] != b"NPELD001" || header[8..10] != [0xe0, 1] || !(1..=64).contains(&asc_len) {
        return Err(invalid("Invalid playback encoder header"));
    }
    let mut asc = vec![0; asc_len];
    stream.read_exact(&mut asc)?;
    // NPELD001 fixes 48 kHz stereo, 480 samples and the pinned SBR configuration.
    if asc != [0xf8, 0xe6, 0x51, 0x3a, 0xe0, 0x9e, 0] {
        return Err(invalid(
            "Encoder selected an unsupported AAC-ELD configuration",
        ));
    }
    Ok(())
}

impl Encoder {
    pub fn start() -> io::Result<Self> {
        let executable = std::env::current_exe()?.with_file_name("ntpods-eld-encoder.exe");
        Self::start_at(&executable)
    }

    fn start_at(executable: &Path) -> io::Result<Self> {
        let mut child = Command::new(executable)
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
            while let Ok(pcm) = pending.recv() {
                if stdin.write_all(&pcm).and_then(|_| stdin.flush()).is_err() {
                    break;
                }
            }
        });
        let reader = thread::spawn(move || {
            let header = read_header(&mut stdout);
            let success = header.is_ok();
            if ready_tx.send(header).is_err() || !success {
                return;
            }
            loop {
                let frame = (|| {
                    let mut length = [0; 2];
                    stdout.read_exact(&mut length)?;
                    let size = u16::from_le_bytes(length) as usize;
                    if size == 0 || size > MAX_AU {
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
        let encoder = Self {
            child,
            input: Some(input),
            output,
            threads: vec![writer, reader],
        };
        ready.recv_timeout(Duration::from_secs(3)).map_err(|_| {
            io::Error::new(io::ErrorKind::TimedOut, "Playback encoder did not start")
        })??;
        Ok(encoder)
    }

    pub fn write(&self, pcm: Vec<u8>) -> io::Result<()> {
        if pcm.len() != PCM_BYTES {
            return Err(invalid("Expected 480 stereo PCM frames"));
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
        assert!(read_header(&mut valid.as_slice()).is_ok());
        for size in 0..valid.len() {
            assert!(read_header(&mut &valid[..size]).is_err());
        }
        let mut bad = valid;
        bad[8] = 0;
        assert!(read_header(&mut bad.as_slice()).is_err());
        bad[8] = 0xe0;
        bad[14] = 0x50;
        assert!(read_header(&mut bad.as_slice()).is_err());
    }

    #[test]
    fn missing_worker_returns_an_error() {
        let nonexistent = std::env::current_exe()
            .unwrap()
            .with_file_name("missing-ntpods-eld-encoder.exe");
        assert!(Encoder::start_at(&nonexistent).is_err());
    }

    #[test]
    #[ignore = "requires the local FDK worker via NTPODS_ELD_ENCODER_TEST_PATH"]
    fn streams_real_worker_and_drops_it() {
        let executable = std::path::PathBuf::from(
            std::env::var_os("NTPODS_ELD_ENCODER_TEST_PATH")
                .expect("Set NTPODS_ELD_ENCODER_TEST_PATH to the local worker executable"),
        );
        assert!(executable.is_absolute());
        let encoder = Encoder::start_at(&executable).expect("Worker must return the pinned ASC");
        let process = encoder.child.as_handle().try_clone_to_owned().unwrap();
        let mut units = 0;
        for frame in 0..100 {
            let mut pcm = Vec::with_capacity(PCM_BYTES);
            for sample in 0..480 {
                let phase = std::f64::consts::TAU * (frame * 480 + sample) as f64 / 48_000.0;
                for frequency in [440.0, 880.0] {
                    pcm.extend_from_slice(
                        &(((phase * frequency).sin() * 6000.0) as i16).to_le_bytes(),
                    );
                }
            }
            encoder.write(pcm).expect("Worker must consume paced PCM");
            thread::sleep(Duration::from_millis(10));
            while let Some(unit) = encoder.read().expect("Worker must keep its output open") {
                assert!(!unit.is_empty() && unit.len() <= MAX_AU);
                units += 1;
            }
            if frame == 19 {
                assert!(units > 0, "Encoder buffered audio instead of streaming it");
            }
        }
        let deadline = Instant::now() + Duration::from_secs(1);
        while units < 100 && Instant::now() < deadline {
            if let Some(unit) = encoder.read().unwrap() {
                assert!(!unit.is_empty() && unit.len() <= MAX_AU);
                units += 1;
            } else {
                thread::sleep(Duration::from_millis(1));
            }
        }
        assert_eq!(
            units, 100,
            "One AU must arrive for each PCM block before EOF"
        );
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
