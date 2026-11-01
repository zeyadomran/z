use super::{Handle, check};
use std::{
    ffi::OsStr,
    io,
    mem::size_of,
    ptr::{null, null_mut},
    sync::atomic::{AtomicU64, Ordering},
};
use windows_sys::Win32::{
    Foundation::*, Security::SECURITY_ATTRIBUTES, Storage::FileSystem::*, System::Pipes::*,
};

static NEXT_PIPE: AtomicU64 = AtomicU64::new(0);

pub(super) fn inheritable() -> SECURITY_ATTRIBUTES {
    SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    }
}

pub(in super::super) struct Pipe {
    handle: Handle,
    pub eof: bool,
}

impl Pipe {
    pub(super) fn pair() -> io::Result<(Self, Handle)> {
        let name = format!(
            r"\\.\pipe\harness-{}-{}",
            std::process::id(),
            NEXT_PIPE.fetch_add(1, Ordering::Relaxed)
        );
        let name = crate::execution::encoding::wide(OsStr::new(&name))?;
        // SAFETY: NUL-terminated name, no retained pointers. FIRST_PIPE_INSTANCE
        // fails closed on collision. Local-only byte server has nonblocking reads.
        let handle = Handle::new(unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_INBOUND | FILE_FLAG_FIRST_PIPE_INSTANCE,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_NOWAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                0,
                65536,
                0,
                null(),
            )
        })?;
        let attrs = inheritable();
        let writer = Handle::new(unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_WRITE,
                0,
                &attrs,
                OPEN_EXISTING,
                0,
                null_mut(),
            )
        })?;
        // The local client is already connected. No blocking connect is possible.
        let ok = unsafe { ConnectNamedPipe(handle.raw(), null_mut()) };
        if ok == 0 && unsafe { GetLastError() } != ERROR_PIPE_CONNECTED {
            check(ok)?;
        }
        Ok((Self { handle, eof: false }, writer))
    }

    /// Fair bounded read; PIPE_NOWAIT leaves no pending I/O to cancel or join.
    pub(in super::super) fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self.eof {
            return Ok(0);
        }
        let mut count = 0;
        // SAFETY: live exclusive byte-read handle, buffer valid for synchronous
        // call; PIPE_NOWAIT returns immediately when no bytes are available.
        let ok = unsafe {
            ReadFile(
                self.handle.raw(),
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                &mut count,
                null_mut(),
            )
        };
        if ok != 0 {
            return Ok(count as usize);
        }
        match unsafe { GetLastError() } {
            ERROR_NO_DATA => Ok(0),
            ERROR_BROKEN_PIPE => {
                self.eof = true;
                Ok(0)
            }
            _ => Err(io::Error::last_os_error()),
        }
    }
}

pub(super) fn closed_stdin() -> io::Result<Handle> {
    let (mut reader, mut writer) = (null_mut(), null_mut());
    // SAFETY: output pointers valid, default non-inheritable handles.
    check(unsafe { CreatePipe(&mut reader, &mut writer, null(), 0) })?;
    let reader = Handle::new(reader)?;
    let writer = Handle::new(writer)?;
    check(unsafe { SetHandleInformation(reader.raw(), HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) })?;
    drop(writer);
    Ok(reader)
}
