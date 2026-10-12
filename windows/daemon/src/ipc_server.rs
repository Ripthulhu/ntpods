//! Per-client named-pipe transport, bounded NDJSON framing and handle cleanup.

use super::{
    apply_command, log, publish_head_tracking, security, set_head_tracking, volume, wide, Ctx,
};
use ntpods_ipc::{Command, PIPE_CMDS, PIPE_EVENTS};
use std::ptr;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile, PIPE_ACCESS_DUPLEX};
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
            if let Command::SetHeadTracking { on } = cmd {
                head_tracking_owner = Some(set_head_tracking(&ctx, on));
            } else {
                apply_command(&ctx, cmd);
            }
        }
    }
    if let Some(owner) = head_tracking_owner {
        let mut tracking = ctx.head_tracking.lock().unwrap();
        if tracking.release(owner) {
            publish_head_tracking(&ctx, &tracking);
        }
    }
    // Dropping the final Arc closes the command pipe.
}

/// Create one pipe instance and block until a client connects; returns its handle.
unsafe fn accept(name: &[u16], sa: *const SECURITY_ATTRIBUTES) -> Option<HANDLE> {
    let h = CreateNamedPipeW(
        name.as_ptr(),
        PIPE_ACCESS_DUPLEX,
        PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
        16,
        4096,
        4096,
        0,
        sa,
    );
    if h == INVALID_HANDLE_VALUE {
        thread::sleep(Duration::from_secs(1));
        return None;
    }
    // ERROR_PIPE_CONNECTED = the client beat us to it (still a success).
    let ok = ConnectNamedPipe(h, ptr::null_mut());
    if ok == 0 && GetLastError() != ERROR_PIPE_CONNECTED {
        CloseHandle(h);
        return None;
    }
    Some(h)
}

/// Separate event writers avoid serialization with command reads on synchronous handles.
/// Each client has a bounded queue and its own writer thread.
pub(super) unsafe fn events_server(ctx: Ctx) {
    let name = wide(PIPE_EVENTS);
    let Ok(mut security) = security::PipeSecurity::new() else {
        log("IPC: cannot establish the current-user ACL; refusing connections");
        return;
    };
    let sa = security.attributes();
    loop {
        let h = match accept(&name, &sa) {
            Some(h) => h,
            None => continue,
        };
        let pipe = Arc::new(Pipe(h));
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
    }
}

/// Read each client's commands on a separate connection.
pub(super) unsafe fn cmds_server(ctx: Ctx) {
    let name = wide(PIPE_CMDS);
    let Ok(mut security) = security::PipeSecurity::new() else {
        log("IPC: cannot establish the current-user ACL; refusing connections");
        return;
    };
    let sa = security.attributes();
    loop {
        let h = match accept(&name, &sa) {
            Some(h) => h,
            None => continue,
        };
        log("cmds: client connected");
        let pipe = Arc::new(Pipe(h));
        let c = ctx.clone();
        thread::spawn(move || client_reader(pipe, c));
    }
}

#[cfg(test)]
mod tests {
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
