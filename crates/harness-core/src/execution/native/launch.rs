use super::{
    Child, Handle, check,
    pipes::{Pipe, closed_stdin},
};
use crate::execution::{LaunchSpec, encoding};
use std::{
    io,
    mem::{size_of, zeroed},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{JobObjects::*, Threading::*},
};

struct Attributes {
    storage: Vec<usize>,
}
impl Attributes {
    fn new() -> io::Result<Self> {
        let mut bytes = 0;
        // SAFETY: documented size query; expected ERROR_INSUFFICIENT_BUFFER.
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), 2, 0, &mut bytes);
        }
        if bytes == 0 {
            return Err(io::Error::last_os_error());
        }
        // usize storage supplies pointer alignment as required by opaque list.
        let mut storage = vec![0; bytes.div_ceil(size_of::<usize>())];
        check(unsafe {
            InitializeProcThreadAttributeList(storage.as_mut_ptr().cast(), 2, 0, &mut bytes)
        })?;
        Ok(Self { storage })
    }
    fn ptr(&mut self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.storage.as_mut_ptr().cast()
    }
    fn handles(&mut self, key: u32, values: &[HANDLE]) -> io::Result<()> {
        // SAFETY: caller keeps values alive until this attribute list is dropped.
        check(unsafe {
            UpdateProcThreadAttribute(
                self.ptr(),
                0,
                key as usize,
                values.as_ptr().cast(),
                std::mem::size_of_val(values),
                null_mut(),
                null(),
            )
        })
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: initialized list, backing storage still alive.
        unsafe {
            DeleteProcThreadAttributeList(self.ptr());
        }
    }
}

pub(in super::super) fn prepare(spec: &LaunchSpec) -> io::Result<Child> {
    let mut encoded = encoding::encode(spec)?;
    // SAFETY: null security attributes create an unnamed NON-inheritable job.
    let job = Handle::new(unsafe { CreateJobObjectW(null(), null()) })?;
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    check(unsafe {
        SetInformationJobObject(
            job.raw(),
            JobObjectExtendedLimitInformation,
            (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    })?;
    let (stdout, stdout_write) = Pipe::pair()?;
    let (stderr, stderr_write) = Pipe::pair()?;
    let stdin = closed_stdin()?;
    let jobs = [job.raw()];
    let handles = [stdin.raw(), stdout_write.raw(), stderr_write.raw()];
    // Declare list after borrowed values: Drop deletes it before values expire.
    let mut attributes = Attributes::new()?;
    attributes.handles(PROC_THREAD_ATTRIBUTE_JOB_LIST, &jobs)?;
    attributes.handles(PROC_THREAD_ATTRIBUTE_HANDLE_LIST, &handles)?;
    let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
    startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = stdin.raw();
    startup.StartupInfo.hStdOutput = stdout_write.raw();
    startup.StartupInfo.hStdError = stderr_write.raw();
    startup.lpAttributeList = attributes.ptr();
    let mut process: PROCESS_INFORMATION = unsafe { zeroed() };
    // SAFETY: all buffers/attribute values remain live for this synchronous call.
    // Creation-time job assignment is mandatory; no retry/fallback can escape it.
    check(unsafe {
        CreateProcessW(
            encoded.executable.as_ptr(),
            encoded.command.as_mut_ptr(),
            null(),
            null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT
                | CREATE_UNICODE_ENVIRONMENT
                | CREATE_NO_WINDOW
                | CREATE_SUSPENDED,
            encoded.environment.as_ptr().cast(),
            encoded.cwd.as_ptr(),
            &startup.StartupInfo,
            &mut process,
        )
    })?;
    Ok(Child {
        job,
        process: Handle::new(process.hProcess)?,
        thread: Handle::new(process.hThread)?,
        pid: process.dwProcessId,
        stdout,
        stderr,
    })
}
