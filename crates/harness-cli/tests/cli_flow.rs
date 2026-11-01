use harness_core::*;
use std::{future::Future, pin::Pin, process::Command};
use tokio::sync::watch;

// The unavailable credential boundary must never be invoked by local abandon.
struct UnavailableAccount;
impl Provider for UnavailableAccount {
    fn binding(&self) -> ProviderBinding {
        ProviderBinding {
            provider: "fixture-unavailable".into(),
            account_ref: "old-account".into(),
            model: "fixture".into(),
            options: "{}".into(),
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            context_tokens: 4096,
            output_tokens: 4096,
        }
    }
    fn respond<'a>(
        &'a mut self,
        _: ProviderRequest,
        _: usize,
        _: watch::Sender<Progress>,
    ) -> Pin<Box<dyn Future<Output = Result<ProviderResponse>> + Send + 'a>> {
        Box::pin(async { panic!("no request is permitted") })
    }
}

#[tokio::test]
async fn local_abandon_releases_saved_work_then_cli_continues_once() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let data = root.path().join("data");
    let config = Config::new(workspace.clone(), data.clone());
    let mut engine = Engine::open(config.clone(), UnavailableAccount)
        .await
        .unwrap();
    let session = engine.create_session().await.unwrap();
    let mut turn = engine
        .submit(session, "suspend before sending".into())
        .await
        .unwrap();
    assert_eq!(
        turn.outcome().await.unwrap().state,
        TurnState::SuspendedLimit
    );
    engine.shutdown().await.unwrap();
    let abandon = |path: &std::path::Path| {
        Command::new(env!("CARGO_BIN_EXE_harness"))
            .args(["abandon", "--workspace"])
            .arg(path)
            .arg("--data-dir")
            .arg(&data)
            .arg("--session")
            .arg(session.0.to_string())
            .arg("--turn")
            .arg(turn.id.0.to_string())
            .arg("--json")
            .output()
            .unwrap()
    };
    let foreign_workspace = root.path().join("foreign-workspace");
    std::fs::create_dir(&foreign_workspace).unwrap();
    assert!(!abandon(&foreign_workspace).status.success());
    let output = abandon(&workspace);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let terminal: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(terminal["outcome"]["state"], "cancelled");
    let mut engine = Engine::open(config, UnavailableAccount).await.unwrap();
    assert_eq!(
        engine
            .turn_outcome(session, turn.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        TurnState::Cancelled
    );
    assert!(engine.usage(session).await.unwrap().is_empty());
    engine.shutdown().await.unwrap();
    let run = |extra: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_harness"))
            .args(["ask", "--workspace"])
            .arg(&workspace)
            .arg("--data-dir")
            .arg(&data)
            .args(extra)
            .arg("hello")
            .output()
            .unwrap()
    };
    let first = run(&[]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let text = String::from_utf8(first.stdout).unwrap();
    assert_eq!(
        text.matches("Scripted complete. No model request or tools executed.")
            .count(),
        1
    );
    let saved = text
        .split_whitespace()
        .next()
        .unwrap()
        .strip_prefix("session=")
        .unwrap();
    let next = run(&["--session", saved, "--json"]);
    assert!(next.status.success());
    let records: Vec<serde_json::Value> = String::from_utf8(next.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(records.first().unwrap()["kind"], "accepted");
    assert_eq!(records.last().unwrap()["outcome"]["state"], "completed");
    let ignored_instructions = run(&[
        "--session",
        saved,
        "--instructions",
        "missing-instructions.txt",
    ]);
    assert_eq!(ignored_instructions.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&ignored_instructions.stderr).contains("cannot be used with"));
}
