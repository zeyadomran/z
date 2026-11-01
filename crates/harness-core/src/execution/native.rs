//! All launcher FFI is confined here and its children. Handles are exclusively
//! owned, call buffers outlive calls, and no asynchronous kernel I/O is issued.
#![allow(unsafe_code)]

mod launch;
mod pipes;
pub(super) use launch::prepare;
use pipes::Pipe;
use std::{
    io,
    mem::{size_of, zeroed},
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    ptr::null_mut,
};
use windows_sys::Win32::{
    Foundation::*,
    System::{JobObjects::*, Threading::*},
};

pub(super) struct Handle(OwnedHandle);
impl Handle {
    fn new(raw: HANDLE) -> io::Result<Self> {
        if raw.is_null() || raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: successful Win32 creation transferred this unique handle to us.
        Ok(Self(unsafe { OwnedHandle::from_raw_handle(raw) }))
    }
    fn raw(&self) -> HANDLE {
        self.0.as_raw_handle()
    }
}

fn check(ok: i32) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub(super) struct Child {
    // Last job handle closes on drop, including panic and unconfirmed shutdown.
    job: Handle,
    process: Handle,
    thread: Handle,
    pub pid: u32,
    pub stdout: Pipe,
    pub stderr: Pipe,
}

impl Child {
    pub fn resume(&self) -> io::Result<()> {
        // SAFETY: live primary-thread handle from CREATE_SUSPENDED.
        if unsafe { ResumeThread(self.thread.raw()) } == u32::MAX {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
    pub fn terminate(&self) -> io::Result<()> {
        // SAFETY: owned job; no breakaway flags were enabled.
        check(unsafe { TerminateJobObject(self.job.raw(), 1) })
    }
    pub fn active(&self) -> io::Result<u32> {
        // SAFETY: zero is a valid initial representation; correctly sized output.
        let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { zeroed() };
        check(unsafe {
            QueryInformationJobObject(
                self.job.raw(),
                JobObjectBasicAccountingInformation,
                (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                null_mut(),
            )
        })?;
        Ok(info.ActiveProcesses)
    }
    pub fn exit_code(&self) -> io::Result<Option<u32>> {
        // SAFETY: owned process handle, nonblocking state query. Exit 259 is valid
        // once signaled; never confuse STILL_ACTIVE with an actual exit code.
        match unsafe { WaitForSingleObject(self.process.raw(), 0) } {
            WAIT_OBJECT_0 => {
                let mut code = 0;
                check(unsafe { GetExitCodeProcess(self.process.raw(), &mut code) })?;
                Ok(Some(code))
            }
            WAIT_TIMEOUT => Ok(None),
            _ => Err(io::Error::last_os_error()),
        }
    }
}
