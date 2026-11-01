//! Native fixture boundary only: named synchronization events and retained
//! process handles. No timing sleeps or PID polling is used to prove shutdown.
#![allow(unsafe_code)]
#![allow(dead_code)] // Shared by the fixture executable and its integration driver.

use std::{
    ffi::OsStr,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
    },
    ptr::null,
    sync::atomic::{AtomicU64, Ordering},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{JobObjects::*, Threading::*},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
fn wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(Some(0)).collect()
}

fn owned(raw: HANDLE) -> OwnedHandle {
    assert!(
        !raw.is_null() && raw != INVALID_HANDLE_VALUE,
        "{}",
        std::io::Error::last_os_error()
    );
    // SAFETY: new unique Win32 handle; OwnedHandle closes exactly once.
    unsafe { OwnedHandle::from_raw_handle(raw) }
}

pub struct Event {
    handle: OwnedHandle,
    pub name: String,
}
impl Event {
    pub fn new() -> Self {
        let name = format!(
            "Local\\harness-fixture-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        // SAFETY: terminated UTF-16 name; default non-inheritable security.
        let handle = owned(unsafe { CreateEventW(null(), 1, 0, wide(&name).as_ptr()) });
        Self { handle, name }
    }
    pub fn open(name: &str) -> Self {
        // SAFETY: existing named event, no inheritance or retained pointers.
        let handle = owned(unsafe {
            OpenEventW(
                EVENT_MODIFY_STATE | SYNCHRONIZATION_SYNCHRONIZE,
                0,
                wide(name).as_ptr(),
            )
        });
        Self {
            handle,
            name: name.into(),
        }
    }
    pub fn set(&self) {
        // SAFETY: owned event handle.
        assert_ne!(unsafe { SetEvent(self.handle.as_raw_handle()) }, 0);
    }
    pub fn wait(&self) {
        // SAFETY: owned event handle. A missing handshake fails within 20s.
        assert_eq!(
            unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 20000) },
            WAIT_OBJECT_0,
            "fixture handshake timed out"
        );
    }
    pub fn inherited_sentinel() -> Self {
        let event = Self::new();
        // SAFETY: only this owned fixture event is made inheritable.
        assert_ne!(
            unsafe {
                SetHandleInformation(
                    event.handle.as_raw_handle(),
                    HANDLE_FLAG_INHERIT,
                    HANDLE_FLAG_INHERIT,
                )
            },
            0
        );
        event
    }
    pub fn handle_value(&self) -> String {
        (self.handle.as_raw_handle() as usize).to_string()
    }
    pub fn assert_unset(&self) {
        // SAFETY: owned event queried without blocking.
        assert_eq!(
            unsafe { WaitForSingleObject(self.handle.as_raw_handle(), 0) },
            WAIT_TIMEOUT
        );
    }
}

pub fn try_signal_inherited(value: &str) {
    let raw = value.parse::<usize>().unwrap() as HANDLE;
    // SAFETY: no memory dereference, kernel validates the handle. If the parent's
    // sentinel leaked, this signals the same object; otherwise failure is fine.
    unsafe {
        SetEvent(raw);
    }
}

pub fn restricted_host_job() -> OwnedHandle {
    // SAFETY: this is test-host setup, not a child launch or a production fallback.
    // Only the current fixture process is assigned before it attempts a launch.
    let job = owned(unsafe { CreateJobObjectW(null(), null()) });
    let mut restrictions: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    restrictions.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
    restrictions.BasicLimitInformation.ActiveProcessLimit = 1;
    assert_ne!(
        unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&restrictions as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&restrictions) as u32,
            )
        },
        0
    );
    assert_ne!(
        unsafe { AssignProcessToJobObject(job.as_raw_handle(), GetCurrentProcess()) },
        0
    );
    job
}

pub struct Process(OwnedHandle);
impl Process {
    pub fn open(pid: u32) -> Self {
        // SAFETY: only owned fixture PIDs; retained handle prevents PID reuse
        // from invalidating subsequent termination confirmation.
        Self(owned(unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) }))
    }
    pub fn assert_running(&self) {
        // SAFETY: live synchronization handle; zero timeout observes state.
        assert_eq!(
            unsafe { WaitForSingleObject(self.0.as_raw_handle(), 0) },
            WAIT_TIMEOUT
        );
    }
    pub fn assert_stopped(&self) {
        // SAFETY: bounded kernel wait, not a notification or PID-existence guess.
        assert_eq!(
            unsafe { WaitForSingleObject(self.0.as_raw_handle(), 10000) },
            WAIT_OBJECT_0,
            "owned process survived cleanup"
        );
    }
}

pub fn in_job() -> bool {
    let mut inside = 0;
    // SAFETY: pseudo handle used only for query; null asks about any job.
    assert_ne!(
        unsafe { IsProcessInJob(GetCurrentProcess(), std::ptr::null_mut(), &mut inside) },
        0
    );
    inside != 0
}
