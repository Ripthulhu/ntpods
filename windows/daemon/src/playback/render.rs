use super::{pcm::SampleFormat, protocol::invalid};
use std::cell::{Cell, RefCell};
use std::io;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle, Thread};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_IO_PENDING, HANDLE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAG_OVERLAPPED, OPEN_EXISTING};
use windows_sys::Win32::System::Threading::{
    CreateEventW, ResetEvent, SetEvent, WaitForMultipleObjects, WaitForSingleObject, INFINITE,
};
use windows_sys::Win32::System::IO::{
    CancelIoEx, DeviceIoControl, GetOverlappedResult, OVERLAPPED,
};

const WAIT_CHANGE: u32 = 0x0022_6414;
const DIAGNOSTICS: u32 = 0x0022_6418;
static DIAGNOSTICS_UNAVAILABLE_LOGGED: AtomicBool = AtomicBool::new(false);

struct Handle(HANDLE);
// Overlapped requests each own their buffers and event; Windows handles are thread-safe.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}

impl Handle {
    fn event() -> io::Result<Self> {
        let handle = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Self(handle))
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

pub struct Render {
    handle: Arc<Handle>,
    event: Handle,
    address: u64,
    ready: Cell<bool>,
    format: Cell<Option<SampleFormat>>,
    diagnostics: RefCell<DiagnosticsTrace>,
}

#[derive(Default)]
struct DiagnosticsTrace {
    next: Option<Instant>,
    deadline: Option<Instant>,
    complete: bool,
    disabled: bool,
}

impl DiagnosticsTrace {
    fn begin(&mut self, now: Instant) {
        self.deadline = Some(now + Duration::from_secs(30));
        self.complete = false;
    }

    fn due(&mut self, now: Instant) -> bool {
        if self.disabled || self.deadline.is_none() {
            return false;
        }
        if self.deadline.is_some_and(|deadline| now >= deadline) {
            self.deadline = None;
            return false;
        }
        if self.next.is_some_and(|next| now < next) {
            return false;
        }
        self.next = Some(now + Duration::from_secs(1));
        if self.complete {
            self.deadline = None;
        }
        true
    }
}

#[derive(Debug)]
struct Diagnostics {
    generation: u32,
    flags: u32,
    level: i32,
    times: [u64; 5],
    counts: [u64; 9],
}

fn parse_diagnostics(data: &[u8], generation: u32) -> io::Result<Option<Diagnostics>> {
    if data.len() != 128
        || u32::from_le_bytes(data[..4].try_into().unwrap()) != 1
        || u32::from_le_bytes(data[8..12].try_into().unwrap()) & !15 != 0
    {
        return Err(invalid("Invalid render diagnostics response"));
    }
    let level = i32::from_le_bytes(data[12..16].try_into().unwrap());
    if !(-635 * 65536 / 10..=0).contains(&level) {
        return Err(invalid("Invalid render diagnostics volume"));
    }
    let observed_generation = u32::from_le_bytes(data[4..8].try_into().unwrap());
    if observed_generation != generation {
        return Ok(None);
    }
    Ok(Some(Diagnostics {
        generation: observed_generation,
        flags: u32::from_le_bytes(data[8..12].try_into().unwrap()),
        level,
        times: std::array::from_fn(|index| {
            u64::from_le_bytes(data[16 + index * 8..24 + index * 8].try_into().unwrap())
        }),
        counts: std::array::from_fn(|index| {
            u64::from_le_bytes(data[56 + index * 8..64 + index * 8].try_into().unwrap())
        }),
    }))
}

#[derive(Debug)]
pub struct ClockStatus {
    pub generation: u32,
    pub phase: u32,
    pub virtual_hns: u64,
}

impl Render {
    pub fn open(address: u64) -> io::Result<Self> {
        if address == 0 || address > 0xffff_ffff_ffff {
            return Err(invalid("Invalid playback device address"));
        }
        let handle = unsafe {
            CreateFileW(
                crate::wide(&format!(r"\\.\NTPodsPlayback_{address:012x}")).as_ptr(),
                0xc000_0000,
                0,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let result = Self {
            handle: Arc::new(Handle(handle)),
            event: Handle::event()?,
            address,
            ready: Cell::new(false),
            format: Cell::new(None),
            diagnostics: RefCell::new(DiagnosticsTrace::default()),
        };
        result.status()?;
        Ok(result)
    }

    fn ioctl(&self, code: u32, input: &[u8], output: &mut [u8]) -> io::Result<usize> {
        let mut returned = 0;
        let mut operation: OVERLAPPED = unsafe { std::mem::zeroed() };
        operation.hEvent = self.event.0;
        if !start_ioctl(
            self.handle.0,
            code,
            input,
            output,
            &mut operation,
            &mut returned,
        )? {
            if unsafe { GetOverlappedResult(self.handle.0, &operation, &mut returned, 1) } == 0 {
                return Err(io::Error::last_os_error());
            }
        }
        if returned as usize > output.len() {
            return Err(invalid("Invalid render response size"));
        }
        Ok(returned as usize)
    }

    pub fn status(&self) -> io::Result<[u32; 4]> {
        let mut data = [0; 48];
        if self.ioctl(0x0022_6404, &[], &mut data)? != data.len() {
            return Err(invalid("Truncated playback driver status"));
        }
        let status = parse_status(&data, self.address, self.ready.get())?;
        let format = status_format(&data)?;
        if self.format.get().is_some_and(|previous| previous != format) {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionReset,
                "Playback sample format changed",
            ));
        }
        self.format.set(Some(format));
        Ok(status)
    }

    pub fn format(&self) -> io::Result<SampleFormat> {
        self.format
            .get()
            .ok_or_else(|| invalid("Playback format has not been checked"))
    }

    pub fn clock(&self) -> io::Result<ClockStatus> {
        let mut data = [0; 64];
        if self.ioctl(0x0022_6410, &[], &mut data)? != data.len() {
            return Err(invalid("Truncated playback clock response"));
        }
        parse_clock(&data)
    }

    pub fn begin_startup_diagnostics(&self) {
        self.diagnostics.borrow_mut().begin(Instant::now());
    }

    pub fn finish_startup_diagnostics(&self) {
        self.diagnostics.borrow_mut().complete = true;
    }

    pub fn trace_startup_diagnostics(&self, generation: u32) {
        if !self.diagnostics.borrow_mut().due(Instant::now()) {
            return;
        }
        let mut data = [0; 128];
        let result = self
            .ioctl(DIAGNOSTICS, &[], &mut data)
            .and_then(|bytes| parse_diagnostics(&data[..bytes], generation));
        match result {
            Ok(Some(d)) => crate::log(&format!(
                "playback startup diagnostics: generation={} initialized={} subscribed={} muted={} minimum={} level_16_16={} packets={} missing={} source_zero={} source_nonzero={} gated={} gate_uninitialized={} gate_unsubscribed={} gate_muted={} gate_minimum={} first_packet_hns={} initialized_hns={} subscribed_hns={} first_source_nonzero_hns={} first_forwarded_nonzero_hns={}",
                d.generation, d.flags & 1 != 0, d.flags & 2 != 0, d.flags & 4 != 0,
                d.flags & 8 != 0, d.level, d.counts[0], d.counts[1], d.counts[2], d.counts[3],
                d.counts[4], d.counts[5], d.counts[6], d.counts[7], d.counts[8], d.times[0],
                d.times[1], d.times[2], d.times[3], d.times[4],
            )),
            Ok(None) => {}, // A producer transition invalidated this sample.
            Err(error) => {
                self.diagnostics.borrow_mut().disabled = true;
                if !DIAGNOSTICS_UNAVAILABLE_LOGGED.swap(true, Ordering::AcqRel) {
                    crate::log(&format!("playback startup diagnostics unavailable: {error}"));
                }
            }
        }
    }

    pub fn set_clock(&self, interval: u32, delta_ms: u32) -> io::Result<()> {
        let input = clock_request(self.clock()?.generation, interval, delta_ms)?;
        if self.ioctl(0x0022_a40c, &input, &mut [])? != 0 {
            return Err(invalid("Invalid playback clock response"));
        }
        Ok(())
    }

    pub fn cancel_clock(&self) -> io::Result<()> {
        self.set_clock(10, 0)
    }

    pub fn set_ready(&self, ready: bool) -> io::Result<()> {
        let input = [1u32.to_le_bytes(), u32::from(ready).to_le_bytes()].concat();
        if self.ioctl(0x0022_a408, &input, &mut [])? != 0 {
            return Err(invalid("Invalid provider response"));
        }
        self.ready.set(ready);
        Ok(())
    }

    pub fn read(&self, maximum: usize) -> io::Result<Vec<u8>> {
        let format = self.format()?;
        if maximum == 0 || maximum > format.read_bytes() || maximum % format.frame_bytes() != 0 {
            return Err(invalid("Invalid PCM read size"));
        }
        let mut pcm = vec![0; maximum];
        let bytes = self.ioctl(0x0022_6400, &[], &mut pcm)?;
        if bytes % format.frame_bytes() != 0 {
            return Err(invalid("Partial stereo PCM frame"));
        }
        pcm.truncate(bytes);
        Ok(pcm)
    }

    pub fn notifications(&self, target: Thread) -> io::Result<Option<Notifications>> {
        let mut output = [0; 4];
        let bytes = match self.ioctl(WAIT_CHANGE, &change_request(0), &mut output) {
            Ok(bytes) => bytes,
            Err(error) if matches!(error.raw_os_error(), Some(1 | 50)) => return Ok(None),
            Err(error) => return Err(error),
        };
        let sequence = change_reply(bytes, &output)?;
        let stop = Arc::new(Handle::event()?);
        let alive = Arc::new(AtomicBool::new(true));
        let handle = self.handle.clone();
        let worker_stop = stop.clone();
        let worker_alive = alive.clone();
        let worker = thread::Builder::new()
            .name("playback-pcm-ready".into())
            .spawn(move || {
                if let Err(error) = wait_changes(&handle, &worker_stop, &target, sequence) {
                    crate::log(&format!("playback: PCM notifications stopped: {error}"));
                }
                worker_alive.store(false, Ordering::Release);
                target.unpark();
            })?;
        Ok(Some(Notifications {
            stop,
            alive,
            worker: Some(worker),
        }))
    }
}

pub struct Notifications {
    stop: Arc<Handle>,
    alive: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl Notifications {
    pub fn available(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }
}

impl Drop for Notifications {
    fn drop(&mut self) {
        unsafe { SetEvent(self.stop.0) };
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn start_ioctl(
    handle: HANDLE,
    code: u32,
    input: &[u8],
    output: &mut [u8],
    operation: &mut OVERLAPPED,
    returned: &mut u32,
) -> io::Result<bool> {
    unsafe {
        if ResetEvent(operation.hEvent) == 0 {
            return Err(io::Error::last_os_error());
        }
        if DeviceIoControl(
            handle,
            code,
            if input.is_empty() {
                ptr::null()
            } else {
                input.as_ptr().cast()
            },
            input.len() as u32,
            if output.is_empty() {
                ptr::null_mut()
            } else {
                output.as_mut_ptr().cast()
            },
            output.len() as u32,
            returned,
            operation,
        ) != 0
        {
            return Ok(true);
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(ERROR_IO_PENDING as i32) {
            Ok(false)
        } else {
            Err(error)
        }
    }
}

fn change_request(sequence: u32) -> [u8; 8] {
    let mut input = [0; 8];
    input[..4].copy_from_slice(&1u32.to_le_bytes());
    input[4..].copy_from_slice(&sequence.to_le_bytes());
    input
}

fn change_reply(bytes: usize, output: &[u8; 4]) -> io::Result<u32> {
    let sequence = u32::from_le_bytes(*output);
    if bytes != output.len() || sequence == 0 {
        Err(invalid("Invalid playback notification response"))
    } else {
        Ok(sequence)
    }
}

fn wait_changes(
    handle: &Handle,
    stop: &Handle,
    target: &Thread,
    mut sequence: u32,
) -> io::Result<()> {
    let event = Handle::event()?;
    let _scheduling = super::scheduling::AudioScheduling::acquire();
    loop {
        let input = change_request(sequence);
        let mut output = [0; 4];
        let mut operation: OVERLAPPED = unsafe { std::mem::zeroed() };
        operation.hEvent = event.0;
        let mut bytes = 0;
        let immediate = start_ioctl(
            handle.0,
            WAIT_CHANGE,
            &input,
            &mut output,
            &mut operation,
            &mut bytes,
        )?;
        if !unsafe { wait_notification(handle.0, stop.0, &operation, &mut bytes, immediate)? } {
            return Ok(());
        }
        sequence = change_reply(bytes as usize, &output)?;
        target.unpark();
    }
}

// The operation and its buffers remain live until completion or cancellation drains.
unsafe fn wait_notification(
    handle: HANDLE,
    stop: HANDLE,
    operation: &OVERLAPPED,
    returned: &mut u32,
    immediate: bool,
) -> io::Result<bool> {
    if immediate {
        return match WaitForSingleObject(stop, 0) {
            WAIT_OBJECT_0 => Ok(false),
            WAIT_TIMEOUT => Ok(true),
            _ => Err(io::Error::last_os_error()),
        };
    }
    let wait = WaitForMultipleObjects(2, [stop, operation.hEvent].as_ptr(), 0, INFINITE);
    if wait == WAIT_OBJECT_0 + 1 {
        return if GetOverlappedResult(handle, operation, returned, 1) != 0 {
            Ok(true)
        } else {
            Err(io::Error::last_os_error())
        };
    }
    let error = io::Error::last_os_error();
    CancelIoEx(handle, operation);
    GetOverlappedResult(handle, operation, returned, 1);
    if wait == WAIT_OBJECT_0 {
        Ok(false)
    } else {
        Err(error)
    }
}

fn clock_request(generation: u32, interval: u32, delta_ms: u32) -> io::Result<Vec<u8>> {
    if !((interval == 10 && delta_ms == 0)
        || ([9, 11].contains(&interval) && (1..=500).contains(&delta_ms)))
    {
        return Err(invalid("Invalid playback clock adjustment"));
    }
    Ok([1, generation, interval, delta_ms]
        .into_iter()
        .flat_map(u32::to_le_bytes)
        .collect())
}

fn parse_clock(data: &[u8; 64]) -> io::Result<ClockStatus> {
    let value = |offset| u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
    let interval = value(8);
    let phase = value(12);
    let requested = value(32);
    let delta_ms = value(36);
    if value(0) != 1
        || ![9, 10, 11].contains(&interval)
        || phase > 5
        || ![9, 10, 11].contains(&requested)
        || delta_ms > 500
        || (phase != 2 && interval != 10)
    {
        return Err(invalid("Unsupported playback clock response"));
    }
    Ok(ClockStatus {
        generation: value(4),
        phase,
        virtual_hns: u64::from_le_bytes(data[24..32].try_into().unwrap()),
    })
}

fn status_format(data: &[u8; 48]) -> io::Result<SampleFormat> {
    let value = |offset| u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
    match (value(0), value(4), value(8), value(12)) {
        (3, 48_000, 2, 16) => Ok(SampleFormat::S16),
        (4, 48_000, 2, 32) => Ok(SampleFormat::Float32),
        _ => Err(invalid("Unsupported playback driver format")),
    }
}

fn parse_status(data: &[u8; 48], address: u64, expected_ready: bool) -> io::Result<[u32; 4]> {
    status_format(data)?;
    let values: Vec<_> = data
        .chunks_exact(4)
        .map(|v| u32::from_le_bytes(v.try_into().unwrap()))
        .collect();
    if values[10] > 1
        || values[11] > 1
        || u64::from_le_bytes(data[32..40].try_into().unwrap()) != address
    {
        return Err(invalid("Unsupported playback driver format"));
    }
    if values[11] == 0 {
        return Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "AirPods playback is offline",
        ));
    }
    if expected_ready && values[10] == 0 {
        return Err(io::Error::new(
            io::ErrorKind::ConnectionReset,
            "AirPods playback driver restarted",
        ));
    }
    Ok([values[4], values[5], values[6], values[7]])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_validate_abi_and_keep_prior_generation_counts_out_of_startup() {
        let mut data = [0u8; 128];
        data[..4].copy_from_slice(&1u32.to_le_bytes());
        data[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        data[8..12].copy_from_slice(&3u32.to_le_bytes());
        data[12..16].copy_from_slice(&(-10i32 * 65536).to_le_bytes());
        for index in 0..14 {
            data[16 + index * 8..24 + index * 8]
                .copy_from_slice(&(index as u64 + 100).to_le_bytes());
        }
        let parsed = parse_diagnostics(&data, u32::MAX).unwrap().unwrap();
        assert_eq!(parsed.flags, 3);
        assert_eq!(parsed.level, -10 * 65536);
        assert_eq!(parsed.times, [100, 101, 102, 103, 104]);
        assert_eq!(parsed.counts, [105, 106, 107, 108, 109, 110, 111, 112, 113]);
        assert!(parse_diagnostics(&data, 0).unwrap().is_none());
        for length in 0..data.len() {
            assert!(parse_diagnostics(&data[..length], u32::MAX).is_err());
        }
        let mut trailing = data.to_vec();
        trailing.push(0);
        assert!(parse_diagnostics(&trailing, u32::MAX).is_err());
        for (offset, value) in [
            (0, 2u32),
            (8, 16),
            (12, 1),
            (12, (-635i32 * 65536 / 10 - 1) as u32),
        ] {
            let mut malformed = data;
            malformed[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(parse_diagnostics(&malformed, u32::MAX).is_err());
        }
    }

    #[test]
    fn startup_diagnostics_are_bounded_and_restarts_do_not_bypass_one_second_rate_limit() {
        let now = Instant::now();
        let mut trace = DiagnosticsTrace::default();
        assert!(!trace.due(now));
        trace.begin(now);
        assert!(trace.due(now));
        assert!(!trace.due(now));
        trace.begin(now + Duration::from_millis(200));
        assert!(!trace.due(now + Duration::from_millis(999)));
        assert!(trace.due(now + Duration::from_secs(1)));
        trace.complete = true;
        assert!(!trace.due(now + Duration::from_millis(1999)));
        assert!(trace.due(now + Duration::from_secs(2)));
        assert!(!trace.due(now + Duration::from_secs(3)));
        trace.begin(now + Duration::from_secs(3));
        assert!(!trace.due(now + Duration::from_secs(33)));
        trace.begin(now + Duration::from_secs(34));
        trace.disabled = true;
        assert!(!trace.due(now + Duration::from_secs(34)));
    }

    #[test]
    fn float_and_legacy_status_require_matching_version_and_format() {
        for (version, bits, format) in [(3, 16, SampleFormat::S16), (4, 32, SampleFormat::Float32)]
        {
            let values = [version, 48_000, 2, bits, 1, 480, 0, u32::MAX, 42, 0, 1, 1];
            let bytes: Vec<_> = values.into_iter().flat_map(u32::to_le_bytes).collect();
            let mut data: [u8; 48] = bytes.try_into().unwrap();
            assert_eq!(status_format(&data).unwrap(), format);
            assert_eq!(
                parse_status(&data, 42, true).unwrap(),
                [1, 480, 0, u32::MAX]
            );
            for (offset, invalid_value) in [
                (0, 2u32),
                (0, 5),
                (4, 44_100),
                (8, 1),
                (12, if bits == 16 { 32 } else { 16 }),
            ] {
                let saved: [u8; 4] = data[offset..offset + 4].try_into().unwrap();
                data[offset..offset + 4].copy_from_slice(&invalid_value.to_le_bytes());
                assert!(status_format(&data).is_err());
                data[offset..offset + 4].copy_from_slice(&saved);
            }
        }
    }

    #[test]
    fn notification_abi_rejects_invalid_replies_and_preserves_wrap() {
        for sequence in [0u32, 1, u32::MAX] {
            let request = change_request(sequence);
            assert_eq!(&request[..4], &1u32.to_le_bytes());
            assert_eq!(&request[4..], &sequence.to_le_bytes());
        }
        for sequence in [1u32, u32::MAX] {
            assert_eq!(change_reply(4, &sequence.to_le_bytes()).unwrap(), sequence);
        }
        assert!(change_reply(4, &[0; 4]).is_err());
        assert!(change_reply(3, &1u32.to_le_bytes()).is_err());
        assert!(change_reply(5, &1u32.to_le_bytes()).is_err());
    }

    #[test]
    fn notification_stop_drains_pending_io_and_tolerates_completed_io() {
        use windows_sys::Win32::Foundation::{GetLastError, ERROR_OPERATION_ABORTED};
        use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile, PIPE_ACCESS_DUPLEX};
        use windows_sys::Win32::System::Pipes::{CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS};
        unsafe {
            for (complete, stop_first) in [(false, true), (true, false), (true, true)] {
                let name = crate::wide(&format!(
                    r"\\.\pipe\NTPods-pcm-notification-test-{}-{complete}-{stop_first}",
                    std::process::id()
                ));
                let server = Handle(CreateNamedPipeW(
                    name.as_ptr(),
                    PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
                    PIPE_REJECT_REMOTE_CLIENTS,
                    1,
                    64,
                    64,
                    0,
                    ptr::null(),
                ));
                assert_ne!(server.0, INVALID_HANDLE_VALUE);
                let client = Handle(CreateFileW(
                    name.as_ptr(),
                    0xc000_0000,
                    0,
                    ptr::null(),
                    OPEN_EXISTING,
                    0,
                    ptr::null_mut(),
                ));
                assert_ne!(client.0, INVALID_HANDLE_VALUE);
                let event = Handle::event().unwrap();
                let stop = Handle::event().unwrap();
                let mut operation: OVERLAPPED = std::mem::zeroed();
                operation.hEvent = event.0;
                let mut output = [0u8; 4];
                let mut bytes = 0;
                assert_eq!(
                    ReadFile(
                        server.0,
                        output.as_mut_ptr(),
                        4,
                        ptr::null_mut(),
                        &mut operation
                    ),
                    0
                );
                assert_eq!(GetLastError(), ERROR_IO_PENDING);
                if complete {
                    assert_ne!(
                        WriteFile(client.0, b"abcd".as_ptr(), 4, &mut bytes, ptr::null_mut()),
                        0
                    );
                }
                if stop_first {
                    assert_ne!(SetEvent(stop.0), 0);
                }
                assert_eq!(
                    wait_notification(server.0, stop.0, &operation, &mut bytes, false).unwrap(),
                    !stop_first
                );
                if !complete {
                    assert_eq!(GetOverlappedResult(server.0, &operation, &mut bytes, 0), 0);
                    assert_eq!(GetLastError(), ERROR_OPERATION_ABORTED);
                    assert_eq!(output, [0; 4]);
                } else if !stop_first {
                    assert_eq!(bytes, 4);
                    assert_eq!(&output, b"abcd");
                }
                assert_ne!(SetEvent(stop.0), 0);
                assert!(
                    !wait_notification(server.0, stop.0, &operation, &mut bytes, true).unwrap()
                );
            }
        }
    }

    #[test]
    fn clock_requests_reject_unbounded_or_ambiguous_adjustments() {
        for (interval, delta) in [(9, 1), (11, 500), (10, 0)] {
            let request = clock_request(u32::MAX, interval, delta).unwrap();
            assert_eq!(request.len(), 16);
            assert_eq!(&request[4..8], &u32::MAX.to_le_bytes());
        }
        for (interval, delta) in [(9, 0), (11, 501), (10, 1), (0, 1), (12, 50)] {
            assert!(clock_request(0, interval, delta).is_err());
        }
    }

    #[test]
    fn clock_response_preserves_generation_and_rejects_unknown_states() {
        let mut data = [0; 64];
        data[..4].copy_from_slice(&1u32.to_le_bytes());
        data[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        data[8..12].copy_from_slice(&10u32.to_le_bytes());
        data[24..32].copy_from_slice(&123_456_789u64.to_le_bytes());
        data[32..36].copy_from_slice(&11u32.to_le_bytes());
        for phase in 0u32..=5 {
            data[12..16].copy_from_slice(&phase.to_le_bytes());
            let snapshot = parse_clock(&data).unwrap();
            assert_eq!(snapshot.generation, u32::MAX);
            assert_eq!(snapshot.phase, phase);
            assert_eq!(snapshot.virtual_hns, 123_456_789);
        }
        data[12..16].copy_from_slice(&6u32.to_le_bytes());
        assert!(parse_clock(&data).is_err());
        data[12..16].copy_from_slice(&3u32.to_le_bytes());
        data[8..12].copy_from_slice(&11u32.to_le_bytes());
        assert!(parse_clock(&data).is_err());
    }

    #[test]
    fn power_loss_requires_negotiation_again() {
        let values = [3u32, 48000, 2, 16, 1, 0, 0, 2, 42, 0, 0, 1];
        let bytes: Vec<_> = values
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        let data = bytes.as_slice().try_into().unwrap();
        assert!(parse_status(data, 42, false).is_ok());
        assert_eq!(
            parse_status(data, 42, true).unwrap_err().kind(),
            io::ErrorKind::ConnectionReset
        );
    }

    #[test]
    fn status_requires_generation_aware_driver_and_preserves_wrap() {
        let mut values = [3u32, 48_000, 2, 16, 1, 480, 0, 0, 42, 0, 0, 1];
        let decode = |values: [u32; 12]| {
            let bytes: Vec<_> = values
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect();
            parse_status(bytes.as_slice().try_into().unwrap(), 42, false)
        };
        for generation in [0, 1, u32::MAX] {
            values[7] = generation;
            assert_eq!(decode(values).unwrap(), [1, 480, 0, generation]);
        }
        values[0] = 1;
        assert!(decode(values).is_err());
        values[0] = 3;
        values[2] = 1;
        assert!(decode(values).is_err());
        values[2] = 2;
        values[8] = 43;
        assert!(decode(values).is_err());
        values[8] = 42;
        values[11] = 0;
        assert_eq!(
            decode(values).unwrap_err().kind(),
            io::ErrorKind::NotConnected
        );
        values[11] = 1;
        values[10] = 2;
        assert!(decode(values).is_err());
    }
}
