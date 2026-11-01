use super::{Intent, Metadata};
use crate::{Error, Result};
use std::{
    os::windows::{ffi::OsStrExt, fs::MetadataExt},
    path::Path,
    ptr,
};
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::{
        Authorization::ConvertSecurityDescriptorToStringSecurityDescriptorW,
        DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION, GetFileSecurityW,
        OWNER_SECURITY_INFORMATION,
    },
    Storage::FileSystem::{MOVEFILE_WRITE_THROUGH, MoveFileExW, ReplaceFileW, SetFileAttributesW},
};
fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}
fn error() -> Error {
    Error::Scope(format!(
        "native patch operation failed (Windows error {})",
        std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
    ))
}
pub(super) fn metadata(path: &Path, file: bool) -> Result<Metadata> {
    let attributes = std::fs::metadata(path)
        .map_err(|_| error())?
        .file_attributes();
    // Fail closed on read-only, reparse, encrypted, compressed, sparse, offline,
    // integrity and other attributes not qualified for this replacement path.
    if file && attributes & !(0x2 | 0x4 | 0x20 | 0x80 | 0x2000) != 0 {
        return Err(Error::Scope(
            "unsupported file attributes for patch preservation".into(),
        ));
    }
    let path = wide(path);
    let info = OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    let mut needed = 0;
    // SAFETY: valid terminated path and output length; first call obtains size.
    unsafe {
        GetFileSecurityW(path.as_ptr(), info, ptr::null_mut(), 0, &mut needed);
    }
    if needed == 0 || needed > 1024 * 1024 {
        return Err(error());
    }
    // u32 storage guarantees alignment for the self-relative security descriptor.
    let mut storage = vec![0u32; (needed as usize).div_ceil(4)];
    // SAFETY: aligned allocated output of at least needed bytes.
    if unsafe {
        GetFileSecurityW(
            path.as_ptr(),
            info,
            storage.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(error());
    }
    let mut sddl = ptr::null_mut();
    let mut length = 0;
    // SAFETY: valid security descriptor; API allocates the returned string.
    if unsafe {
        ConvertSecurityDescriptorToStringSecurityDescriptorW(
            storage.as_mut_ptr().cast(),
            1,
            info,
            &mut sddl,
            &mut length,
        )
    } == 0
    {
        return Err(error());
    }
    // SAFETY: the API returned length UTF-16 units including the terminator.
    let security = unsafe {
        String::from_utf16_lossy(std::slice::from_raw_parts(
            sddl,
            length.saturating_sub(1) as usize,
        ))
    };
    // SAFETY: allocation belongs to LocalAlloc per the API contract.
    unsafe {
        LocalFree(sddl.cast());
    }
    Ok(Metadata {
        attributes,
        security,
    })
}
pub(super) fn stage_attributes(path: &Path, metadata: &Metadata) -> Result<()> {
    let path = wide(path);
    // SAFETY: terminated path; attributes were restricted at preview time.
    if unsafe { SetFileAttributesW(path.as_ptr(), metadata.attributes) } == 0 {
        return Err(error());
    }
    Ok(())
}
pub(super) fn secure_stage(path: &Path, metadata: &Metadata) -> Result<()> {
    use windows_sys::Win32::Security::{
        Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW,
        GetSecurityDescriptorControl, PROTECTED_DACL_SECURITY_INFORMATION, SE_DACL_PROTECTED,
        SetFileSecurityW, UNPROTECTED_DACL_SECURITY_INFORMATION,
    };
    let inherited = self::metadata(path, true)?;
    if inherited.security.split("D:").next() != metadata.security.split("D:").next() {
        return Err(Error::Scope(
            "owner/group preservation unsupported for this target".into(),
        ));
    }
    let sddl: Vec<u16> = metadata.security.encode_utf16().chain(Some(0)).collect();
    let mut descriptor = ptr::null_mut();
    // SAFETY: terminated SDDL from GetFileSecurity; API owns output allocation.
    if unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1,
            &mut descriptor,
            ptr::null_mut(),
        )
    } == 0
    {
        return Err(error());
    }
    let result = (|| {
        let mut control = 0;
        let mut revision = 0;
        // SAFETY: valid allocated descriptor and initialized output pointers.
        if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0 {
            return Err(error());
        }
        let protection = if control & SE_DACL_PROTECTED != 0 {
            PROTECTED_DACL_SECURITY_INFORMATION
        } else {
            UNPROTECTED_DACL_SECURITY_INFORMATION
        };
        let path_wide = wide(path);
        // SAFETY: terminated path and valid descriptor. Set only the approved
        // DACL, before writing any source bytes into the newly created stage.
        if unsafe {
            SetFileSecurityW(
                path_wide.as_ptr(),
                DACL_SECURITY_INFORMATION | protection,
                descriptor,
            )
        } == 0
        {
            return Err(error());
        }
        // SetFileSecurityW clears informational auto-inheritance flags on the
        // stage. Require identical owner/group, ACEs and protection; native
        // replacement must still restore the full original final descriptor.
        fn staging_security(s: &str) -> String {
            let Some((owner, dacl)) = s.split_once("D:") else {
                return s.into();
            };
            let (flags, aces) = dacl.split_at(dacl.find('(').unwrap_or(dacl.len()));
            format!(
                "{owner}D:{}{aces}",
                flags.replace("AI", "").replace("AR", "")
            )
        }
        if staging_security(&self::metadata(path, true)?.security)
            != staging_security(&metadata.security)
        {
            return Err(Error::Scope(
                "staging security metadata did not match".into(),
            ));
        }
        Ok(())
    })();
    // SAFETY: allocation was returned by the conversion API above.
    unsafe {
        LocalFree(descriptor);
    }
    result
}
pub(super) fn replace(intent: &Intent, revalidate: impl Fn() -> Result<()>) -> Result<()> {
    let target = wide(&intent.prepared.target);
    let staging = wide(&intent.staging);
    let backup = wide(&intent.backup);
    let started = std::time::Instant::now();
    loop {
        // A sharing retry is another execution boundary: an editor may have
        // changed the file while its non-delete-sharing handle was open.
        revalidate()?;
        // SAFETY: all paths are terminated, distinct, same-directory validated
        // names. Flags never ignore ACL/metadata errors. Creates never replace.
        let ok = unsafe {
            if intent.prepared.before.is_some() {
                ReplaceFileW(
                    target.as_ptr(),
                    staging.as_ptr(),
                    backup.as_ptr(),
                    0,
                    ptr::null_mut(),
                    ptr::null_mut(),
                )
            } else {
                MoveFileExW(staging.as_ptr(), target.as_ptr(), MOVEFILE_WRITE_THROUGH)
            }
        };
        if ok != 0 {
            break;
        }
        let e = std::io::Error::last_os_error();
        if !matches!(e.raw_os_error(), Some(32 | 33))
            || started.elapsed() >= std::time::Duration::from_millis(500)
        {
            return Err(Error::Scope(format!(
                "native replacement failed (Windows error {}); reconcile recorded attempt",
                e.raw_os_error().unwrap_or(0)
            )));
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    // ReplaceFileW has no supported write-through flag. Flush the resulting
    // file explicitly; failure retains intent and triggers reconciliation.
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&intent.prepared.target)
        .and_then(|f| f.sync_all())
        .map_err(|_| error())?;
    Ok(())
}
