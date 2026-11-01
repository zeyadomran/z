//! The only unsafe credential boundary: current-user DPAPI, protected user DACL,
//! durable same-directory replacement, and system-browser dispatch.
use super::*;
use std::{
    ffi::c_void,
    os::windows::ffi::OsStrExt,
    path::Path,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    Security::{Authorization::*, Cryptography::*, *},
    Storage::FileSystem::*,
    System::Threading::*,
    UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
};
fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}
fn failure() -> Error {
    auth_error("Windows credential protection failed")
}
struct Allocation(*mut c_void);
impl Drop for Allocation {
    fn drop(&mut self) {
        // SAFETY: these pointers are returned by LocalAlloc-backed Windows APIs
        // and are owned exactly once by this guard.
        unsafe {
            LocalFree(self.0);
        }
    }
}
pub fn private_directory(path: &Path) -> Result<()> {
    // SAFETY: token buffers are usize-aligned and sized by GetTokenInformation;
    // UTF-16 strings stay alive through each call. Descriptor/SID allocations
    // and the process-token handle are released on all paths that acquire them.
    unsafe {
        let mut token = null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return Err(failure());
        }
        let mut size = 0;
        GetTokenInformation(token, TokenUser, null_mut(), 0, &mut size);
        let mut buffer = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
        let ok = GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            size,
            &mut size,
        );
        CloseHandle(token);
        if ok == 0 {
            return Err(failure());
        }
        let user = &*(buffer.as_ptr().cast::<TOKEN_USER>());
        let mut sid = null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut sid) == 0 {
            return Err(failure());
        }
        let sid_memory = Allocation(sid.cast());
        let mut n = 0;
        while *sid.add(n) != 0 {
            n += 1;
        }
        let sid_text =
            String::from_utf16(std::slice::from_raw_parts(sid, n)).map_err(|_| failure())?;
        drop(sid_memory);
        let sddl = wide(std::ffi::OsStr::new(&format!(
            "D:P(A;OICI;FA;;;{sid_text})"
        )));
        let mut descriptor = null_mut();
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            null_mut(),
        ) == 0
        {
            return Err(failure());
        }
        let _descriptor = Allocation(descriptor);
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let path = wide(path.as_os_str());
        if CreateDirectoryW(path.as_ptr(), &attributes) == 0
            && GetLastError() != ERROR_ALREADY_EXISTS
        {
            return Err(failure());
        }
        let mut present = 0;
        let mut defaulted = 0;
        let mut acl = null_mut();
        if GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) == 0
            || present == 0
        {
            return Err(failure());
        }
        if SetNamedSecurityInfoW(
            path.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            acl,
            null_mut(),
        ) != ERROR_SUCCESS
        {
            return Err(failure());
        }
        Ok(())
    }
}
fn crypt(bytes: &[u8], encrypt: bool) -> Result<Vec<u8>> {
    // SAFETY: input references the live slice; DPAPI allocates output, copied
    // before its single LocalFree. UI and machine-wide protection are disabled.
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: bytes.len().try_into().map_err(|_| failure())?,
            pbData: bytes.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: null_mut(),
        };
        let ok = if encrypt {
            CryptProtectData(
                &input,
                null(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                null_mut(),
                null(),
                null(),
                null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 {
            return Err(failure());
        }
        let _memory = Allocation(output.pbData.cast());
        Ok(std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec())
    }
}
pub fn protect(bytes: &[u8]) -> Result<Vec<u8>> {
    crypt(bytes, true)
}
pub fn unprotect(bytes: &[u8]) -> Result<Vec<u8>> {
    crypt(bytes, false)
}
pub fn replace(from: &Path, to: &Path) -> Result<()> {
    let a = wide(from.as_os_str());
    let b = wide(to.as_os_str());
    if unsafe {
        MoveFileExW(
            a.as_ptr(),
            b.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(failure());
    }
    Ok(())
}
pub fn browser(url: &str) -> Result<()> {
    let operation = wide(std::ffi::OsStr::new("open"));
    let url = wide(std::ffi::OsStr::new(url));
    if unsafe {
        ShellExecuteW(
            null_mut(),
            operation.as_ptr(),
            url.as_ptr(),
            null(),
            null(),
            SW_SHOWNORMAL,
        )
    } as isize
        <= 32
    {
        return Err(auth_error("cannot open system browser"));
    }
    Ok(())
}
