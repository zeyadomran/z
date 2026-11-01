#![cfg(windows)]

use harness_core::execution::{Cancellation, LaunchSpec, Outcome, prepare};
use std::{ffi::OsString, time::Duration};

#[path = "support/launcher_native.rs"]
mod native;

fn spec(cwd: &std::path::Path, args: &[&str]) -> LaunchSpec {
    LaunchSpec {
        executable: env!("CARGO_BIN_EXE_launcher-fixture").into(),
        args: args.iter().map(OsString::from).collect(),
        cwd: cwd.into(),
        environment: vec![],
        timeout: Duration::from_secs(20),
        output_limit: 1024 * 1024,
    }
}

fn worker(
    spec: LaunchSpec,
    cancel: Cancellation,
) -> std::sync::mpsc::Receiver<harness_core::execution::ExecutionResult> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = prepare(spec).unwrap().run(&cancel);
        let _ = tx.send(result);
    });
    rx
}

fn pid(cwd: &std::path::Path, file: &str) -> native::Process {
    native::Process::open(
        std::fs::read_to_string(cwd.join(file))
            .unwrap()
            .parse()
            .unwrap(),
    )
}

struct Parent(std::process::Child);
impl Drop for Parent {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn ordinary_arguments_raw_streams_and_closed_stdin() {
    let temp = tempfile::tempdir().unwrap();
    let cwd = temp.path().join("space Unicode \u{03bb}");
    std::fs::create_dir(&cwd).unwrap();
    let exe = cwd.join("fixture \u{96ea}.exe");
    std::fs::copy(env!("CARGO_BIN_EXE_launcher-fixture"), &exe).unwrap();
    let values = [
        "",
        "space word",
        "\u{96ea}\u{03bb}",
        "a\"b",
        "trailing \\",
        "\\\\\"",
        "&|<>^%!",
    ];
    assert!(
        std::env::var_os("PATH").is_some(),
        "host PATH fixture required"
    );
    let mut args = vec![OsString::from("echo-environment")];
    args.extend(values.iter().map(OsString::from));
    let sentinel = native::Event::inherited_sentinel();
    let result = prepare(LaunchSpec {
        executable: exe,
        args,
        cwd: cwd.clone(),
        environment: vec![(
            "FIXTURE_UNRELATED_HANDLE".into(),
            sentinel.handle_value().into(),
        )],
        timeout: Duration::from_secs(20),
        output_limit: 65536,
    })
    .unwrap()
    .run(&Cancellation::default());
    assert_eq!(result.outcome, Outcome::Completed);
    assert_eq!(result.root_exit_code, Some(0));
    assert!(result.shutdown_confirmed && result.output_complete);
    assert!(!result.forced_termination);
    let output = String::from_utf8(result.stdout).unwrap();
    let mut lines = output.lines();
    assert_eq!(
        serde_json::from_str::<Vec<String>>(lines.next().unwrap()).unwrap(),
        values
    );
    assert_eq!(lines.next().unwrap(), cwd.to_str().unwrap());
    assert_eq!(result.stderr, [0, 255, 1, 254]);
    sentinel.assert_unset();
}

#[test]
fn cancellation_drains_both_streams_and_stops_the_tree_even_when_pipes_are_idle() {
    for mode in ["tree", "idle"] {
        let temp = tempfile::tempdir().unwrap();
        let ready = native::Event::new();
        let release = native::Event::new();
        let cancel = Cancellation::default();
        let result = worker(
            spec(temp.path(), &[mode, &ready.name, &release.name]),
            cancel.clone(),
        );
        ready.wait();
        let leaf = (mode == "tree").then(|| pid(temp.path(), "leaf.pid"));
        if let Some(leaf) = &leaf {
            leaf.assert_running();
        }
        cancel.cancel();
        let result = result.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(result.outcome, Outcome::Cancelled, "{result:?}");
        assert!(result.shutdown_confirmed && result.forced_termination);
        assert!(!result.output_complete);
        assert!(result.error.is_none(), "{result:?}");
        if let Some(leaf) = leaf {
            leaf.assert_stopped();
            assert_eq!(result.stdout, vec![0xab; 256 * 1024]);
            assert_eq!(result.stderr, vec![0xcd; 256 * 1024]);
        }
    }
}

#[test]
fn parent_death_at_creation_boundary_closes_job_before_any_child_code() {
    let temp = tempfile::tempdir().unwrap();
    let ready = native::Event::new();
    let release = native::Event::new();
    let mut parent = Parent(
        std::process::Command::new(env!("CARGO_BIN_EXE_launcher-fixture"))
            .args(["boundary", &ready.name, &release.name])
            .current_dir(temp.path())
            .spawn()
            .unwrap(),
    );
    ready.wait();
    let child = pid(temp.path(), "child.pid");
    child.assert_running();
    parent.0.kill().unwrap();
    parent.0.wait().unwrap();
    child.assert_stopped();
}

#[test]
fn root_exit_settles_then_cleans_lingering_descendant() {
    let temp = tempfile::tempdir().unwrap();
    let ready = native::Event::new();
    let root_release = native::Event::new();
    let leaf_release = native::Event::new();
    let mut launch = spec(
        temp.path(),
        &[
            "linger",
            &ready.name,
            &root_release.name,
            &leaf_release.name,
        ],
    );
    // Root completion transfers supervision to the fixed two-second settling
    // window even when the original command deadline falls inside that window.
    launch.timeout = Duration::from_millis(1500);
    let result = worker(launch, Cancellation::default());
    ready.wait();
    let leaf = pid(temp.path(), "leaf.pid");
    leaf.assert_running();
    root_release.set();
    let result = result.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(result.outcome, Outcome::CompletedWithCleanup, "{result:?}");
    assert_eq!(result.root_exit_code, Some(0));
    assert!(result.shutdown_confirmed && result.forced_termination);
    assert!(!result.output_complete);
    assert!(result.error.is_none(), "{result:?}");
    leaf.assert_stopped();
}

#[test]
fn launcher_works_inside_existing_parent_job() {
    let temp = tempfile::tempdir().unwrap();
    let result = prepare(spec(temp.path(), &["nested"]))
        .unwrap()
        .run(&Cancellation::default());
    assert_eq!(result.outcome, Outcome::Completed, "{result:?}");
    assert_eq!(result.root_exit_code, Some(0), "{result:?}");
    assert!(result.shutdown_confirmed && result.output_complete);
    assert_eq!(result.stdout, b"nested completed\n");
    // A host whose active-process allowance is exhausted must reject children.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_launcher-fixture"))
        .arg("restricted")
        .current_dir(temp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"restricted rejected\n");
}

#[test]
fn output_cap_timeout_and_pre_cancel_never_report_success() {
    let temp = tempfile::tempdir().unwrap();
    let mut limited = spec(temp.path(), &["limit"]);
    limited.output_limit = 10000;
    let result = prepare(limited).unwrap().run(&Cancellation::default());
    assert_eq!(result.outcome, Outcome::OutputLimited, "{result:?}");
    assert_eq!(result.stdout.len() + result.stderr.len(), 10000);
    assert!(result.shutdown_confirmed && result.forced_termination);
    let cancel = Cancellation::default();
    let child = prepare(spec(temp.path(), &["echo"])).unwrap();
    cancel.cancel();
    let result = child.run(&cancel);
    assert_eq!(result.outcome, Outcome::Cancelled);
    assert!(result.shutdown_confirmed && result.forced_termination);
    assert!(result.stdout.is_empty() && result.stderr.is_empty());
    let ready = native::Event::new();
    let release = native::Event::new();
    let mut timed = spec(temp.path(), &["idle", &ready.name, &release.name]);
    timed.timeout = Duration::from_millis(50);
    let child = prepare(timed).unwrap();
    let process = native::Process::open(child.process_id());
    let result = child.run(&Cancellation::default());
    assert_eq!(result.outcome, Outcome::TimedOut, "{result:?}");
    assert!(result.shutdown_confirmed && result.forced_termination);
    process.assert_stopped();
}

#[test]
fn unsupported_specs_fail_without_launch() {
    let temp = tempfile::tempdir().unwrap();
    let mut invalid = spec(temp.path(), &["echo"]);
    invalid.executable = "fixture.exe".into();
    assert!(prepare(invalid).is_err());
    let mut batch = spec(temp.path(), &["echo"]);
    batch.executable = temp.path().join("fixture.cmd");
    std::fs::write(&batch.executable, "@echo unsafe").unwrap();
    assert!(prepare(batch).is_err());
    let mut nul = spec(temp.path(), &["echo", "a\0b"]);
    assert!(prepare(nul.clone()).is_err());
    nul.args.clear();
    nul.environment = vec![
        ("NAME".into(), "first".into()),
        ("name".into(), "second".into()),
    ];
    assert!(prepare(nul).is_err());
}
