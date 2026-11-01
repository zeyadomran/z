//! Internal host-process capability. Not an approved or durable model tool.

use std::{ffi::OsString, io, path::PathBuf, time::Duration};

#[cfg(windows)]
mod encoding;
#[cfg(windows)]
mod native;
#[cfg(windows)]
mod supervise;

/// Exact launch inputs. No PATH search, shell conversion, or inherited environment.
#[derive(Clone, Debug)]
pub struct LaunchSpec {
    pub executable: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: PathBuf,
    pub environment: Vec<(OsString, OsString)>,
    pub timeout: Duration,
    /// Combined raw stdout/stderr cap. Retained data never exceeds this cap.
    pub output_limit: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Completed,
    CompletedWithCleanup,
    Cancelled,
    TimedOut,
    OutputLimited,
    Failed,
}

/// An exit code alone never proves that the managed tree stopped.
#[derive(Debug)]
pub struct ExecutionResult {
    pub outcome: Outcome,
    pub root_exit_code: Option<u32>,
    pub shutdown_confirmed: bool,
    pub forced_termination: bool,
    pub output_complete: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub error: Option<String>,
}

/// Cancellation has no dependency on output consumers or an async runtime.
#[derive(Clone, Default)]
pub struct Cancellation(std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>);

impl Cancellation {
    pub fn cancel(&self) {
        let (lock, wake) = &*self.0;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
        wake.notify_all();
    }

    pub fn is_cancelled(&self) -> bool {
        *self.0.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[cfg(windows)]
    fn wait(&self, duration: Duration) {
        let (lock, wake) = &*self.0;
        let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let _ = wake.wait_timeout_while(guard, duration, |cancelled| !*cancelled);
    }
}

/// Created suspended, already in a kill-on-close job. Drop kills the owned tree;
/// only `run` returns shutdown evidence. Do not release workspace ownership on drop.
pub struct PreparedChild {
    #[cfg(windows)]
    inner: native::Child,
    #[cfg(windows)]
    spec: LaunchSpec,
}

pub fn prepare(spec: LaunchSpec) -> io::Result<PreparedChild> {
    #[cfg(windows)]
    {
        let inner = native::prepare(&spec)?;
        Ok(PreparedChild { inner, spec })
    }
    #[cfg(not(windows))]
    {
        let _ = spec;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Windows launcher required",
        ))
    }
}

impl PreparedChild {
    #[cfg(windows)]
    pub fn process_id(&self) -> u32 {
        self.inner.pid
    }

    #[cfg(windows)]
    pub fn run(self, cancellation: &Cancellation) -> ExecutionResult {
        supervise::run(self.inner, &self.spec, cancellation)
    }
}
