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
    use windows_sys::Win32::Foundation::GENERIC_ALL;
    use windows_sys::Win32::Security::Authorization::ConvertStringSidToSidW;
    use windows_sys::Win32::Security::{
        EqualSid, GetAce, GetSecurityDescriptorControl, GetSecurityDescriptorDacl,
        ACCESS_ALLOWED_ACE, SE_DACL_PROTECTED,
    };

    #[test]
    fn pipe_descriptor_grants_only_current_user_and_system() {
        let mut security = PipeSecurity::new().expect("create native pipe descriptor");
        let attributes = security.attributes();
        assert_eq!(attributes.bInheritHandle, 0);
        assert_eq!(attributes.lpSecurityDescriptor, security.0);
        unsafe {
            let mut control = 0;
            let mut revision = 0;
            assert_ne!(
                GetSecurityDescriptorControl(security.0, &mut control, &mut revision),
                0,
            );
            assert_ne!(
                control & SE_DACL_PROTECTED,
                0,
                "DACL must block inheritance"
            );
            let mut present = 0;
            let mut defaulted = 0;
            let mut dacl = ptr::null_mut();
            assert_ne!(
                GetSecurityDescriptorDacl(security.0, &mut present, &mut dacl, &mut defaulted),
                0,
                "read native DACL"
            );
            assert_ne!(present, 0, "DACL must be present");
            assert!(!dacl.is_null(), "A null DACL would grant everyone access");
            assert_eq!(defaulted, 0);
            assert_eq!(
                (*dacl).AceCount,
                2,
                "Only SYSTEM and the user may have grants"
            );
            let user = current_user_sid().expect("current process token SID");
            for (index, expected) in ["S-1-5-18", user.as_str()].iter().enumerate() {
                let mut entry = ptr::null_mut();
                assert_ne!(GetAce(dacl, index as u32, &mut entry), 0);
                let ace = &*entry.cast::<ACCESS_ALLOWED_ACE>();
                assert_eq!(ace.Header.AceType, 0, "ACCESS_ALLOWED_ACE_TYPE");
                assert_eq!(
                    ace.Header.AceFlags, 0,
                    "ACE must not be inherited or inheritable"
                );
                assert_eq!(ace.Mask, GENERIC_ALL);
                // SDDL prints aliases such as LA for RID 500; binary SIDs preserve identity.
                let expected: Vec<u16> = expected.encode_utf16().chain(Some(0)).collect();
                let mut expected_sid = ptr::null_mut();
                assert_ne!(
                    ConvertStringSidToSidW(expected.as_ptr(), &mut expected_sid),
                    0
                );
                let matches = EqualSid(ptr::addr_of!(ace.SidStart).cast_mut().cast(), expected_sid);
                LocalFree(expected_sid);
                assert_ne!(matches, 0, "Unexpected trustee in pipe DACL");
            }
        }
    }
}
