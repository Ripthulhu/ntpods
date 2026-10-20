//! Per-client named-pipe transport, bounded NDJSON framing and handle cleanup.

use super::{
    apply_command, log, probe_hearing_test_mode, publish_head_tracking, security,
    set_head_tracking, start_nearby_finder, stop_nearby_finder, volume, wide, Ctx,
};
use ntpods_ipc::{Command, PIPE_CMDS, PIPE_EVENTS};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use std::{io, ptr};
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_NO_DATA, ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{
    ReadFile, WriteFile, FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX,
};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_WAIT,
};

/// Owns a pipe HANDLE until its server and client worker release their Arcs.
struct Pipe(HANDLE);
impl Drop for Pipe {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}
// Win32 handles can cross threads; Arc keeps each connection alive for its I/O worker.
unsafe impl Send for Pipe {}
unsafe impl Sync for Pipe {}

/// Write the whole buffer to a pipe handle. Returns false if the client is gone.
unsafe fn write_all(h: HANDLE, buf: &[u8]) -> bool {
    let mut off = 0usize;
    while off < buf.len() {
        let mut written = 0u32;
        let ok = WriteFile(
            h,
            buf[off..].as_ptr(),
            (buf.len() - off) as u32,
            &mut written,
            ptr::null_mut(),
        );
        if ok == 0 || written == 0 {
            return false;
        }
        off += written as usize;
    }
    true
}

fn decode_command_chunk(acc: &mut Vec<u8>, bytes: &[u8]) -> Option<Vec<Command>> {
    if acc.len() + bytes.len() > 16 * 1024 {
        return None;
    }
    // Pipe reads may split a UTF-8 character; decode only complete JSON lines.
    acc.extend_from_slice(bytes);
    let mut commands = Vec::new();
    while let Some(nl) = acc.iter().position(|&byte| byte == b'\n') {
        if let Ok(command) = serde_json::from_slice::<Command>(&acc[..nl]) {
            commands.push(command);
        }
        acc.drain(..=nl);
    }
    Some(commands)
}

/// Per-client reader: parse NDJSON commands and apply them until it disconnects.
fn client_reader(pipe: Arc<Pipe>, ctx: Ctx) {
    volume::init(); // this thread applies StepVolume/ToggleMute (WASAPI needs COM)
    let h = pipe.0;
    let mut buf = [0u8; 4096];
    let mut acc = Vec::new();
    let mut head_tracking_owner = None;
    let mut nearby_finder_owner: Option<(String, u64)> = None;
    let mut hearing_probe_owner: Option<Arc<AtomicBool>> = None;
    loop {
        let mut read = 0u32;
        let ok = unsafe {
            ReadFile(
                h,
                buf.as_mut_ptr(),
                buf.len() as u32,
                &mut read,
                ptr::null_mut(),
            )
        };
        if ok == 0 || read == 0 {
            break; // disconnected
        }
        let Some(commands) = decode_command_chunk(&mut acc, &buf[..read as usize]) else {
            break; // Commands have a bounded input buffer.
        };
        for cmd in commands {
            match cmd {
                Command::ProbeHearingTestMode {
                    request_id,
                    device_id,
                    enable_check,
                } => {
                    if let Some(owner) = hearing_probe_owner.take() {
                        owner.store(false, Ordering::Release);
                    }
                    hearing_probe_owner =
                        probe_hearing_test_mode(&ctx, request_id, device_id, enable_check);
                }
                Command::SetHeadTracking { on } => {
                    head_tracking_owner = Some(set_head_tracking(&ctx, on));
                }
                Command::StartNearbyFinder { device_id } => {
                    if let Some(owner) = start_nearby_finder(&ctx, &device_id) {
                        nearby_finder_owner = Some((device_id, owner));
                    }
                }
                Command::StopNearbyFinder { device_id } => {
                    if nearby_finder_owner
                        .as_ref()
                        .is_some_and(|(id, _)| id == &device_id)
                    {
                        if let Some((id, owner)) = nearby_finder_owner.take() {
                            stop_nearby_finder(&ctx, &id, owner);
                        }
                    }
                }
                cmd => apply_command(&ctx, cmd),
            }
        }
    }
    if let Some(owner) = head_tracking_owner {
        let mut tracking = ctx.head_tracking.lock().unwrap();
        if tracking.release(owner) {
            publish_head_tracking(&ctx, &tracking);
        }
    }
    if let Some((device_id, owner)) = nearby_finder_owner {
        stop_nearby_finder(&ctx, &device_id, owner);
    }
    if let Some(owner) = hearing_probe_owner {
        owner.store(false, Ordering::Release);
    }
    // Dropping the final Arc closes the command pipe.
}

unsafe fn create_pipe(
    name: &[u16],
    sa: *const SECURITY_ATTRIBUTES,
    first: bool,
) -> io::Result<Arc<Pipe>> {
    let h = CreateNamedPipeW(
        name.as_ptr(),
        PIPE_ACCESS_DUPLEX
            | if first {
                FILE_FLAG_FIRST_PIPE_INSTANCE
            } else {
                0
            },
        PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
        16,
        4096,
        4096,
        0,
        sa,
    );
    if h == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(Arc::new(Pipe(h)))
}

unsafe fn accept(pipe: Arc<Pipe>) -> io::Result<Arc<Pipe>> {
    // ERROR_PIPE_CONNECTED = the client beat us to it (still a success).
    let ok = ConnectNamedPipe(pipe.0, ptr::null_mut());
    if ok == 0 && GetLastError() != ERROR_PIPE_CONNECTED {
        return Err(io::Error::last_os_error());
    }
    Ok(pipe)
}

unsafe fn serve(name: &str, mut connected: impl FnMut(Arc<Pipe>)) {
    let name = wide(name);
    let Ok(mut security) = security::PipeSecurity::new() else {
        log("IPC: cannot establish the current-user ACL; refusing connections");
        return;
    };
    let sa = security.attributes();
    // Keep the first server handle alive so another account cannot replace the pipe ACL.
    let first = match create_pipe(&name, &sa, true) {
        Ok(pipe) => pipe,
        Err(error) => {
            log(&format!(
                "IPC: cannot claim pipe ownership; refusing connections: {error}"
            ));
            return;
        }
    };
    // An unused anchor could take a real client's connection without serving it.
    match accept(first.clone()) {
        Ok(pipe) => connected(pipe),
        // A client can close before ConnectNamedPipe; the broken handle still owns the name.
        Err(error) if error.raw_os_error() == Some(ERROR_NO_DATA as i32) => {}
        Err(error) => {
            log(&format!("IPC: first connection failed: {error}"));
            return;
        }
    }
    loop {
        match create_pipe(&name, &sa, false).and_then(|pipe| accept(pipe)) {
            Ok(pipe) => connected(pipe),
            Err(_) => thread::sleep(Duration::from_secs(1)),
        };
    }
}

/// Separate event writers avoid serialization with command reads on synchronous handles.
/// Each client has a bounded queue and its own writer thread.
pub(super) unsafe fn events_server(ctx: Ctx) {
    serve(PIPE_EVENTS, |pipe| {
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(32);
        let n = {
            let mut cl = ctx.clients.lock().unwrap();
            cl.push(tx);
            cl.len()
        };
        log(&format!("events: client connected ({n} total)"));
        let p = pipe.clone();
        thread::spawn(move || {
            for msg in rx {
                if !unsafe { write_all(p.0, &msg) } {
                    break;
                }
            }
        });
        ctx.push_state(); // greet the newcomer
    });
}

/// Read each client's commands on a separate connection.
pub(super) unsafe fn cmds_server(ctx: Ctx) {
    serve(PIPE_CMDS, |pipe| {
        log("cmds: client connected");
        let c = ctx.clone();
        thread::spawn(move || client_reader(pipe, c));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Foundation::{ERROR_ACCESS_DENIED, GENERIC_READ, GENERIC_WRITE};
    use windows_sys::Win32::Storage::FileSystem::{CreateFileW, OPEN_EXISTING};

    fn test_pipe_name() -> Vec<u16> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        wide(&format!(
            "\\\\.\\pipe\\ntpods-ownership-test-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }

    unsafe fn client(name: &[u16]) -> Pipe {
        let handle = CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            ptr::null(),
            OPEN_EXISTING,
            0,
            ptr::null_mut(),
        );
        assert_ne!(
            handle,
            INVALID_HANDLE_VALUE,
            "{}",
            io::Error::last_os_error()
        );
        Pipe(handle)
    }

    #[test]
    fn preexisting_pipe_is_refused_even_when_it_allows_another_instance() {
        unsafe {
            let name = test_pipe_name();
            let mut security = security::PipeSecurity::new().unwrap();
            let sa = security.attributes();
            let existing = create_pipe(&name, ptr::null(), false).unwrap();
            let secondary = create_pipe(&name, &sa, false).unwrap();
            let error = create_pipe(&name, &sa, true)
                .err()
                .expect("must refuse existing name");
            assert_eq!(error.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));
            drop(secondary);
            drop(existing);
            assert!(create_pipe(&name, &sa, true).is_ok());
        }
    }

    #[test]
    fn retained_first_instance_serves_clients_and_keeps_ownership_after_disconnect() {
        unsafe {
            for early_close in [false, true] {
                let name = test_pipe_name();
                let mut security = security::PipeSecurity::new().unwrap();
                let sa = security.attributes();
                let first = create_pipe(&name, &sa, true).unwrap();
                let connection = client(&name);
                if early_close {
                    drop(connection);
                    let error = accept(first.clone()).err().expect("client already closed");
                    assert_eq!(error.raw_os_error(), Some(ERROR_NO_DATA as i32));
                } else {
                    let accepted = accept(first.clone()).unwrap();
                    assert!(write_all(accepted.0, b"hello"));
                    let mut bytes = [0u8; 5];
                    let mut read = 0;
                    assert_ne!(
                        ReadFile(
                            connection.0,
                            bytes.as_mut_ptr(),
                            5,
                            &mut read,
                            ptr::null_mut()
                        ),
                        0
                    );
                    assert_eq!((bytes, read), (*b"hello", 5));
                    drop(connection);
                    drop(accepted);
                }
                for _ in 0..2 {
                    let error = create_pipe(&name, &sa, true)
                        .err()
                        .expect("name must remain owned");
                    assert_eq!(error.raw_os_error(), Some(ERROR_ACCESS_DENIED as i32));
                    let next = create_pipe(&name, &sa, false).unwrap();
                    let connection = client(&name);
                    let accepted = accept(next).unwrap();
                    assert!(write_all(accepted.0, b"next"));
                    let mut bytes = [0u8; 4];
                    let mut read = 0;
                    assert_ne!(
                        ReadFile(
                            connection.0,
                            bytes.as_mut_ptr(),
                            4,
                            &mut read,
                            ptr::null_mut()
                        ),
                        0
                    );
                    assert_eq!((bytes, read), (*b"next", 4));
                }
            }
        }
    }

    #[test]
    fn fragmented_commands_preserve_non_ascii_names() {
        let name = "Zoë's 耳機 🎵";
        let line = ntpods_ipc::to_line(&super::Command::SetName {
            name: name.into(),
            request_id: 42,
        });
        for split in 0..line.len() {
            let mut buffer = Vec::new();
            let mut commands =
                super::decode_command_chunk(&mut buffer, &line.as_bytes()[..split]).unwrap();
            commands.extend(
                super::decode_command_chunk(&mut buffer, &line.as_bytes()[split..]).unwrap(),
            );
            assert!(
                matches!(&commands[..], [super::Command::SetName { name: actual, request_id: 42 }]
                if actual == name),
                "split at byte {split}"
            );
            assert!(buffer.is_empty());
        }
    }

    #[test]
    fn command_chunks_reject_invalid_utf8_and_keep_the_input_bound() {
        let mut buffer = Vec::new();
        let commands = super::decode_command_chunk(
            &mut buffer,
            b"{\"cmd\":\"set_name\",\"name\":\"\xff\"}\n{\"cmd\":\"ping\"}\n",
        )
        .unwrap();
        assert!(matches!(&commands[..], [super::Command::Ping]));
        assert!(
            super::decode_command_chunk(&mut buffer, &vec![b' '; 16 * 1024])
                .unwrap()
                .is_empty()
        );
        assert!(super::decode_command_chunk(&mut buffer, b"x").is_none());
        assert_eq!(buffer.len(), 16 * 1024);
    }
}
