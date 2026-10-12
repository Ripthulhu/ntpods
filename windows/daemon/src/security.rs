//! Named-pipe access is limited to the daemon's user and SYSTEM.
use std::{io, ptr};
use windows_sys::Win32::Foundation::{CloseHandle, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
    TOKEN_USER,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

fn current_user_sid() -> io::Result<String> {
    unsafe {
        let mut token = ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut len = 0;
        GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut len);
        if (len as usize) < std::mem::size_of::<TOKEN_USER>() {
            let err = io::Error::last_os_error();
            CloseHandle(token);
            return Err(err);
        }
        // TOKEN_USER requires pointer alignment.
        let mut storage = vec![0usize; (len as usize).div_ceil(std::mem::size_of::<usize>())];
        let ok = GetTokenInformation(token, TokenUser, storage.as_mut_ptr().cast(), len, &mut len);
        let err = io::Error::last_os_error();
        CloseHandle(token);
        if ok == 0 {
            return Err(err);
        }
        let user = &*storage.as_ptr().cast::<TOKEN_USER>();
        let mut sid = ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut sid) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut n = 0;
        while *sid.add(n) != 0 {
            n += 1;
        }
        let sid_text = String::from_utf16_lossy(std::slice::from_raw_parts(sid, n));
        LocalFree(sid.cast());
        Ok(sid_text)
    }
}

pub struct PipeSecurity(PSECURITY_DESCRIPTOR);

impl PipeSecurity {
    pub fn new() -> io::Result<Self> {
        let sid_text = current_user_sid()?;
        let sddl: Vec<u16> = format!("D:P(A;;GA;;;SY)(A;;GA;;;{sid_text})")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        unsafe {
            let mut descriptor = ptr::null_mut();
            if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                ptr::null_mut(),
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(descriptor))
        }
    }

    pub fn attributes(&mut self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        }
    }
}

impl Drop for PipeSecurity {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Security::Authorization::ConvertSecurityDescriptorToStringSecurityDescriptorW;
    use windows_sys::Win32::Security::DACL_SECURITY_INFORMATION;

    #[test]
    fn pipe_descriptor_grants_only_current_user_and_system() {
        let mut security = PipeSecurity::new().expect("create native pipe descriptor");
        let attributes = security.attributes();
        assert_eq!(attributes.bInheritHandle, 0);
        assert_eq!(attributes.lpSecurityDescriptor, security.0);
        let mut text = ptr::null_mut();
        let mut len = 0;
        unsafe {
            assert_ne!(
                ConvertSecurityDescriptorToStringSecurityDescriptorW(
                    security.0,
                    1,
                    DACL_SECURITY_INFORMATION,
                    &mut text,
                    &mut len,
                ),
                0,
                "read native DACL"
            );
            let encoded = std::slice::from_raw_parts(text, len as usize);
            let decoded = String::from_utf16_lossy(encoded)
                .trim_end_matches('\0')
                .to_owned();
            LocalFree(text.cast());
            let sid = current_user_sid().expect("current process token SID");
            let account = if sid == "S-1-5-18" { "SY" } else { &sid };
            // Exact round-trip proves there are no Everyone/Authenticated Users
            // grants, inherited ACEs, or a null DACL.
            assert_eq!(decoded, format!("D:P(A;;GA;;;SY)(A;;GA;;;{account})"));
        }
    }
}
