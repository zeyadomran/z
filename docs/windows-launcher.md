# Internal Windows launcher

Issue #12 provides the process-ownership prerequisite for M3 (#6). No engine,
provider, store, model tool, approval UI, or command CLI calls this module yet.
Both required provider routes remain part of the full application plan.

## API and ownership

`harness_core::execution::prepare(LaunchSpec)` validates the explicit executable,
arguments, cwd, environment, timeout, and combined output cap. It creates an
unnamed, non-inheritable kill-on-close job **before** `CreateProcessW`. The
`STARTUPINFOEXW` job-list attribute establishes membership at process creation;
the handle-list attribute permits only stdin, stdout, and stderr inheritance.
There is no create-then-assign path, breakaway option, or unowned fallback.

The returned `PreparedChild` is suspended, already owned by the job, and exposes
its process ID. `run(self, &Cancellation)` resumes it and synchronously owns
supervision until a result is available. M3 should run this on a blocking worker.
Cancelling before `run` terminates without resuming. A prepared child must be run
or dropped promptly; its execution timeout starts at `run`, not at preparation.
Dropping closes the job and requests tree termination, but provides **no shutdown
confirmation**. Explicitly run/cancel and inspect the result before releasing
workspace ownership. Caller panic, abrupt parent death, and normal drop all
close the non-inheritable job handle.

Approval and durable intent must precede `prepare` in M3: creating a suspended
process is already an external effect, even before its primary thread runs.

`LaunchSpec` requires existing absolute local-drive `.exe` and cwd paths. No PATH
lookup, implicit cwd search, batch conversion, or shell parsing occurs. Argument
quoting follows ordinary Microsoft CRT rules, including empty arguments, embedded
quotes, and trailing backslashes. Programs with custom command-line grammars
need a separate, explicit integration. NULs, relative paths, non-executable
extensions, invalid/duplicate environment keys, and overlong command lines are
rejected. This is not a workspace containment or executable-identity check.

Environment entries replace the inherited environment entirely; empty means an
empty block. The caller supplies any required OS/tool variables. This avoids
accidental implicit inheritance but does not identify or scrub credentials from
caller-supplied values. Runtime limits are greater than zero and at most 24 hours;
the combined raw capture cap is greater than zero and at most 64 MiB. These are
internal API bounds, not final M3 product defaults.

Launch uses `CREATE_NO_WINDOW`, a closed-write-end stdin pipe (immediate EOF),
and separate raw stdout/stderr byte pipes. GUI executables can still create their
own windows; no-window is a console-creation flag, not a UI sandbox.

## Draining and results

A single supervisor multiplexes both streams, reading at most 8 KiB from each
per iteration. The local named-pipe read ends use `PIPE_NOWAIT`; child write ends
retain blocking byte-stream semantics. Empty reads return immediately. There are
no outstanding overlapped operations, reader threads, blocking read-to-EOF calls,
or I/O cancellation joins. This deliberately uses bounded synchronous polling,
not overlapped I/O. While idle, a condition-variable wait wakes on cancellation
or after at most 10 ms. Continuous output cannot starve state/deadline checks.

The combined cap bounds retained bytes. At the cap the job is terminated and
additional bytes are drained/discarded until EOF or the shutdown deadline. No
stream ordering is claimed between stdout and stderr. Capture is in memory;
there is no observer queue or output consumer that can block cancellation.

Normal root exit starts a fixed two-second settling interval, independent of the
remaining command timeout. Surviving managed descendants are then terminated.
Cancellation, timeout, output cap, and supervision errors also request immediate
`TerminateJobObject`. Shutdown/draining has a separate five-second bound. These
are supervisor bounds under ordinary OS scheduling, not guarantees against a
hung kernel, filesystem, process-creation call, or resource exhaustion.

`ExecutionResult` separates:

- `outcome`: `Completed`, `CompletedWithCleanup`, `Cancelled`, `TimedOut`,
  `OutputLimited`, or `Failed`. `Completed` does not mean exit code zero.
- `root_exit_code`: recorded only after the process handle is signaled; even
  exit code 259 is then a real exit code rather than inferred `STILL_ACTIVE`.
- `shutdown_confirmed`: root termination **and** queried job active-process count
  zero. Notifications and EOF alone never establish shutdown.
- `forced_termination`: a job termination request was issued. Partial external
  effects and lost application buffers are possible.
- `output_complete`: both EOFs observed without forced termination, limit, or
  supervision error. Forced cleanup is conservatively incomplete even if both
  pipes eventually reach EOF.
- Separate raw byte vectors and optional OS/supervision error text. Shutdown
  errors remain visible alongside the original cancellation/limit reason.

An unconfirmed shutdown must leave the eventual M3 durable attempt interrupted
and block workspace reuse until reconciliation. A result with an error is not
clean success even if root exit is zero. Closing the job at return still requests
cleanup if explicit confirmation failed.

## Focused fixture evidence

`cargo test --locked -p harness-core --test windows_launcher` uses the public
launcher and an owned `launcher-fixture` executable, temporary directories, named
event handshakes, and retained kernel process handles. It owns these native
behaviors; the existing engine-flow owner remains unchanged:

- Spaces/Unicode executable and cwd, argument round-trip, closed stdin, invalid
  UTF-8/NUL output bytes, and exclusion of an unrelated inheritable event handle.
- Two concurrent 256 KiB writes (each larger than pipe capacity), cancellation
  with a living descendant, and cancellation while both pipes are idle.
- Parent killed after successful creation and before any child instruction runs;
  the already-open child process handle becomes signaled. This is the observable
  creation boundary, not fault injection inside the kernel's `CreateProcessW`.
- Root exit zero with a gated surviving descendant; fixed settling returns
  `CompletedWithCleanup` and a signaled descendant handle.
- Launch from an existing job, and fail-closed creation when a separate parent
  job has exhausted its active-process allowance. Arbitrary third-party parent
  job configurations are not all qualified.
- Output cap, execution deadline, pre-resume cancellation, and unsupported launch
  specifications. Tests use no arbitrary sleeps, stress repetition, credentials,
  model inference, or user-owned processes.

The fixture binary is a test asset, not an application entry point or release
deliverable. Final packaging must select `harness.exe`, not all built binaries.
Native fixture success does not establish application installation, Windows
version coverage, full CLI cancellation, recovery, or live provider qualification.

Local validation on Windows 11 Home x64, build 26300, Rust 1.99.1/MSVC:
formatting, Clippy with warnings denied, all 15 workspace functional tests
(eight existing plus seven launcher flows), release build, and all three
repository-hook tests passed. The initial argument-flow assertion first failed
with `launcher not implemented`, then passed through the native implementation.
A release CLI smoke with separate owned workspace/data directories completed
the existing scripted M0 turn; it did not execute a model command.
Repository CI belongs to the PR's actual head and is reported separately; its
seven required contexts and existing hooks are unchanged. No Windows app release
qualification or paid inference was performed.

## Remaining M3 work and host limits

M3 owns normalized executable/interpreter approval and revalidation, workspace
ownership, credential filtering, durable attempt intent/results, artifact
spooling and flush-before-commit, head/tail previews, decoding/redaction, output
retrieval, observer-loss integration, and recovery. No output is persisted here;
therefore no SQLite mock or duplicate durable engine flow is introduced.

Jobs group lifecycle; they do not restrict filesystem/network access, user
privileges, or every OS-mediated process launch. No PTY, graceful console signal,
persistent build server, hostile-process sandbox, or Linux/macOS execution is
promised. Unsafe Rust is denied workspace-wide and allowed only inside the
documented native launcher and native fixture boundary modules.

## Native references

The implementation was checked against live Microsoft documentation:

- [Creation attributes](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-updateprocthreadattribute):
  job list (Windows 10+), explicit handle list, and attribute-value lifetimes.
- [CreateProcessW](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessw)
  and [CRT argument parsing](https://learn.microsoft.com/en-us/cpp/c-language/parsing-c-command-line-arguments).
- [Named-pipe wait modes](https://learn.microsoft.com/en-us/windows/win32/ipc/named-pipe-type-read-and-wait-modes):
  immediate empty reads and the distinction from overlapped I/O.
- [Nested jobs](https://learn.microsoft.com/en-us/windows/win32/procthread/nested-jobs)
  and [job state queries](https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-queryinformationjobobject).

Bindings use `windows-sys` 0.61.2 (MIT OR Apache-2.0), already present transitively
in the M0 lockfile; this change adds the narrowly scoped direct feature set.
