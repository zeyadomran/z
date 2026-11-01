use super::{Cancellation, ExecutionResult, LaunchSpec, Outcome, native::Child};
use std::time::{Duration, Instant};

const SETTLE: Duration = Duration::from_secs(2);
const SHUTDOWN: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(10);

pub(super) fn run(mut child: Child, spec: &LaunchSpec, cancel: &Cancellation) -> ExecutionResult {
    let started = Instant::now();
    let mut root_exited = None;
    let mut stopping = None;
    let mut result = ExecutionResult {
        outcome: Outcome::Completed,
        root_exit_code: None,
        shutdown_confirmed: false,
        forced_termination: false,
        output_complete: false,
        stdout: Vec::new(),
        stderr: Vec::new(),
        error: None,
    };
    if cancel.is_cancelled() {
        result.outcome = Outcome::Cancelled;
    } else if let Err(error) = child.resume() {
        result.error = Some(error.to_string());
        result.outcome = Outcome::Failed;
    }
    let shutdown_wait = Cancellation::default();
    loop {
        let mut progress = false;
        let mut buffer = [0; 8192];
        let mut remaining = spec.output_limit - result.stdout.len() - result.stderr.len();
        let mut limited = remaining == 0;
        // Service both streams every round; a noisy writer cannot starve the other
        // stream, cancellation, process state checks, or shutdown deadline.
        for (pipe, output) in [
            (&mut child.stdout, &mut result.stdout),
            (&mut child.stderr, &mut result.stderr),
        ] {
            match pipe.read(&mut buffer) {
                Ok(count) => {
                    progress |= count != 0;
                    let retained = count.min(remaining);
                    output.extend_from_slice(&buffer[..retained]);
                    remaining -= retained;
                    limited |= remaining == 0;
                }
                Err(error) => {
                    result.error.get_or_insert(error.to_string());
                }
            }
        }
        match child.exit_code() {
            Ok(Some(code)) => {
                result.root_exit_code = Some(code);
                root_exited.get_or_insert_with(Instant::now);
            }
            Ok(None) => {}
            Err(error) => {
                result.error.get_or_insert(error.to_string());
            }
        }
        let active = match child.active() {
            Ok(active) => Some(active),
            Err(error) => {
                result.error.get_or_insert(error.to_string());
                None
            }
        };
        result.shutdown_confirmed = result.root_exit_code.is_some() && active == Some(0);
        let eof = child.stdout.eof && child.stderr.eof;
        if stopping.is_none() {
            result.outcome = if result.error.is_some() {
                Outcome::Failed
            } else if cancel.is_cancelled() {
                Outcome::Cancelled
            } else if limited {
                Outcome::OutputLimited
            } else if result.shutdown_confirmed && eof {
                Outcome::Completed
            } else if root_exited.is_some_and(|t| t.elapsed() >= SETTLE) {
                if active.is_some_and(|count| count > 0) {
                    Outcome::CompletedWithCleanup
                } else {
                    result.error.get_or_insert_with(|| {
                        "output EOF missing after root-exit settling".into()
                    });
                    Outcome::Failed
                }
            } else if started.elapsed() >= spec.timeout {
                Outcome::TimedOut
            } else {
                result.outcome
            };
            if result.outcome != Outcome::Completed {
                stopping = Some(Instant::now());
                result.forced_termination = true;
                if let Err(error) = child.terminate() {
                    result.error.get_or_insert(error.to_string());
                }
            }
        }
        if result.shutdown_confirmed && eof {
            result.output_complete =
                !result.forced_termination && !limited && result.error.is_none();
            break;
        }
        if stopping.is_some_and(|t| t.elapsed() >= SHUTDOWN) {
            result.error.get_or_insert_with(|| {
                "shutdown or output EOF not confirmed within 5 seconds".into()
            });
            break;
        }
        if !progress {
            // A signaled cancellation token must not busy-loop during shutdown.
            if stopping.is_some() {
                shutdown_wait.wait(POLL);
            } else {
                cancel.wait(POLL);
            }
        }
    }
    result
}
