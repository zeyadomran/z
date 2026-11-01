use crate::output::escaped;
use harness_core::*;
use std::io::{IsTerminal, Write};

pub fn interactive(json: bool) -> bool {
    !json && std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}
/// The engine has already durably suspended and released its OS lock. Reading
/// from the terminal cannot consume active-work budget or prevent shutdown.
pub async fn decide(
    engine: &mut Engine,
    session: SessionId,
    outcome: &TurnOutcome,
    failure: &mut tokio::sync::watch::Receiver<bool>,
) -> std::result::Result<Option<TurnHandle>, String> {
    let preview = if outcome.state == TurnState::AwaitingApproval {
        Some(
            engine
                .preview_patch(session, outcome.turn_id)
                .await
                .map_err(|e| e.to_string())?,
        )
    } else {
        None
    };
    let prompt = if let Some(p) = &preview {
        format!(
            "\nPatch approval (complete action, controls escaped):\n{}\nApprove [a], deny [d], cancel turn [c]: ",
            escaped(&serde_json::to_string_pretty(p).map_err(|e| e.to_string())?)
                .chars()
                .flat_map(|c| if c.is_ascii() {
                    c.to_string()
                } else {
                    c.escape_unicode().to_string()
                }
                .chars()
                .collect::<Vec<_>>())
                .collect::<String>()
        )
    } else {
        "\nUncertain patch effect; recorded artifacts are retained. Reconcile again [r] or abandon with unknown effects [c]: ".into()
    };
    std::io::stderr()
        .write_all(prompt.as_bytes())
        .and_then(|_| std::io::stderr().flush())
        .map_err(|_| "approval display unavailable")?;
    let (send, receive) = tokio::sync::oneshot::channel();
    // A detached terminal reader is intentional: Ctrl+C can finish durable
    // abandonment even while the OS is waiting for an unfinished input line.
    std::thread::spawn(move || {
        let mut line = String::new();
        let result = std::io::stdin().read_line(&mut line).map(|_| line);
        let _ = send.send(result);
    });
    let choice = tokio::select! { biased;
        _=tokio::signal::ctrl_c()=>"c".into(),
        _=failure.changed()=>"c".into(),
        result=receive=>result.map_err(|_|"approval input unavailable")?.map_err(|_|"approval input failed")?,
    };
    let next = match (choice.trim(), preview) {
        ("a", Some(p)) => Some(
            engine
                .decide_patch(&p.approval_id, ApprovalDecision::Approve)
                .await,
        ),
        ("d", Some(p)) => Some(
            engine
                .decide_patch(&p.approval_id, ApprovalDecision::Deny)
                .await,
        ),
        ("r", None) => Some(engine.resume(session, outcome.turn_id).await),
        _ => {
            engine
                .abandon(session, outcome.turn_id)
                .await
                .map_err(|e| e.to_string())?;
            None
        }
    };
    next.transpose().map_err(|e| e.to_string())
}
