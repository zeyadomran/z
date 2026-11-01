use clap::{Parser, Subcommand};
use harness_core::{Config, Engine, ScriptStep, ScriptedProvider, SessionId, TurnState};
use std::{
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
};

#[derive(Parser)]
#[command(
    name = "harness",
    version,
    about = "M0 local scripted harness; no tools or live providers"
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Persist a deterministic credential-free turn.
    Ask {
        #[arg(long)]
        workspace: PathBuf,
        /// Existing local, private storage outside the workspace; default: LOCALAPPDATA/z-harness.
        #[arg(long)]
        data_dir: Option<PathBuf>,
        #[arg(long, default_value = "scripted", value_parser = ["scripted"])]
        provider: String,
        /// Continue a completed session with the same workspace/provider binding.
        #[arg(long)]
        session: Option<i64>,
        prompt: String,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(Args::parse()).await {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            let _ = writeln!(io::stderr(), "harness: {}", escaped(&error));
            ExitCode::FAILURE
        }
    }
}

async fn run(args: Args) -> Result<u8, String> {
    let Command::Ask {
        workspace,
        data_dir,
        provider: _,
        session,
        prompt,
    } = args.command;
    let data_dir = data_dir
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("z-harness")))
        .ok_or("set --data-dir to a private local directory outside the workspace")?;
    let script = ScriptedProvider::new(vec![ScriptStep::reply(
        prompt.clone(),
        "M0 complete. No tools executed.",
    )]);
    let mut engine = Engine::open(Config::new(workspace, data_dir), script)
        .await
        .map_err(|e| e.to_string())?;
    let operation = async {
        let session = match session {
            Some(id) => SessionId(id),
            None => engine.create_session().await?,
        };
        let mut turn = engine.submit(session, prompt).await?;
        let outcome = tokio::select! {
            outcome = turn.outcome() => outcome,
            signal = tokio::signal::ctrl_c() => {
                turn.cancel();
                if signal.is_err() { return Err(harness_core::Error::Worker); }
                turn.outcome().await
            }
        }?;
        Ok::<_, harness_core::Error>((session, outcome))
    }
    .await;
    // Every return after open joins cleanup, including input/output/cancellation failures.
    let shutdown = engine.shutdown().await;
    let (session, outcome) = operation.map_err(|e| e.to_string())?;
    shutdown.map_err(|e| e.to_string())?;
    let output = format!(
        "session={} turn={} state={:?}\n{}\nusage=unavailable\n",
        session.0,
        outcome.turn_id.0,
        outcome.state,
        escaped(outcome.text.as_deref().unwrap_or("No completed response."))
    );
    // There is no active work at this point; a broken output pipe still fails the CLI.
    io::stdout()
        .write_all(output.as_bytes())
        .map_err(|e| format!("output failed: {e}"))?;
    Ok(match outcome.state {
        TurnState::Completed => 0,
        TurnState::Cancelled => 130,
        TurnState::SuspendedLimit => 3,
        _ => 1,
    })
}

fn escaped(text: &str) -> String {
    text.chars().flat_map(|c| {
        if c.is_control() || matches!(c, '\u{061c}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}') {
            c.escape_unicode().collect::<Vec<_>>()
        } else { vec![c] }
    }).collect()
}
