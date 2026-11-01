use std::{
    fs::File,
    io,
    os::windows::{ffi::OsStringExt, io::AsRawHandle},
    path::PathBuf,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle, GetFinalPathNameByHandleW,
};
pub(super) fn final_path(file: &File) -> io::Result<PathBuf> {
    let mut buffer = vec![0u16; 32768];
    // SAFETY: borrowed valid file handle and writable buffer of the stated length.
    let n = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            0,
        )
    };
    if n == 0 {
        return Err(io::Error::last_os_error());
    }
    if n as usize >= buffer.len() {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(std::ffi::OsString::from_wide(&buffer[..n as usize]).into())
}
pub(super) fn links(file: &File) -> io::Result<u32> {
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: valid borrowed file handle and initialized output storage.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(info.nNumberOfLinks)
}

pub(super) fn local_volume(path: &std::path::Path) -> bool {
    use std::path::{Component, Prefix};
    let Some(Component::Prefix(prefix)) = path.components().next() else {
        return false;
    };
    let drive = match prefix.kind() {
        Prefix::Disk(d) | Prefix::VerbatimDisk(d) => d,
        _ => return false,
    };
    let root = [drive as u16, b':' as u16, b'\\' as u16, 0];
    // SAFETY: root is a NUL-terminated UTF-16 drive root valid for this call.
    let kind = unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(root.as_ptr()) };
    matches!(kind, 2 | 3 | 6)
}
