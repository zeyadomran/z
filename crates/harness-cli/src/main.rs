use clap::{Parser, Subcommand};
use harness_core::{
    openrouter::{OpenRouterConfig, OpenRouterProvider},
    *,
};
use serde_json::json;
use std::{path::PathBuf, process::ExitCode, time::Duration};
mod output;
use output::{Output, escaped, json_line};
#[derive(Parser)]
#[command(
    name = "harness",
    version,
    about = "Bounded read-only coding harness; explicit OpenRouter or offline scripted provider"
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Inspect a workspace. OpenRouter sends scoped context to the gateway/upstream.
    Ask {
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        data_dir: Option<PathBuf>,
        #[arg(long,default_value="scripted",value_parser=["scripted","openrouter"])]
        provider: String,
        /// Non-secret OpenRouter JSON configuration; unknown fields are rejected.
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        model: Option<String>,
        /// Exact catalog endpoint tag. Replaces configured route; disables fallbacks.
        #[arg(long)]
        upstream: Option<String>,
        #[arg(long)]
        session: Option<i64>,
        /// Explicitly resume this suspended/interrupted turn (requires --session).
        #[arg(long, requires = "session", conflicts_with = "prompt")]
        resume_turn: Option<i64>,
        #[arg(long,default_value="inspect",value_parser=["inspect","supervised"])]
        policy: String,
        #[arg(long)]
        instructions: Option<PathBuf>,
        #[arg(long, default_value_t = 30)]
        max_requests: usize,
        #[arg(long, default_value_t = 60)]
        max_tools: usize,
        #[arg(long, default_value_t = 1200)]
        active_seconds: u64,
        #[arg(long, default_value_t = 262144)]
        max_output_bytes: usize,
        #[arg(long, default_value_t = 128000)]
        context_tokens: usize,
        /// Versioned NDJSON output only; never consumes approval input.
        #[arg(long)]
        json: bool,
        #[arg(required_unless_present = "resume_turn")]
        prompt: Option<String>,
    },
}
#[tokio::main]
async fn main() -> ExitCode {
    match run(Args::parse()).await {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("harness: {}", escaped(&e));
            ExitCode::FAILURE
        }
    }
}
async fn run(args: Args) -> std::result::Result<u8, String> {
    let Command::Ask {
        workspace,
        data_dir,
        provider,
        config,
        model,
        upstream,
        session,
        resume_turn,
        policy,
        instructions,
        max_requests,
        max_tools,
        active_seconds,
        max_output_bytes,
        context_tokens,
        json: json_mode,
        prompt,
    } = args.command;
    let data_dir = data_dir
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("z-harness")))
        .ok_or("set --data-dir to a private local directory outside the workspace")?;
    let backend: Box<dyn Provider> = if provider == "openrouter" {
        let mut c = if let Some(path) = config {
            let bytes = std::fs::read(path).map_err(|_| "cannot read provider config")?;
            if bytes.len() > 16384 {
                return Err("provider config exceeds 16 KiB".into());
            }
            serde_json::from_slice::<OpenRouterConfig>(&bytes)
                .map_err(|_| "invalid provider config; see docs/openrouter.md")?
        } else {
            OpenRouterConfig::new(
                model
                    .clone()
                    .ok_or("OpenRouter requires --model or --config")?,
                upstream
                    .clone()
                    .ok_or("OpenRouter requires --upstream or --config")?,
            )
        };
        if let Some(model) = model {
            c.model = model;
        }
        if let Some(upstream) = upstream {
            c.upstreams = vec![upstream];
            c.allow_fallbacks = false;
        }
        Box::new(
            OpenRouterProvider::from_env(c)
                .await
                .map_err(|e| e.to_string())?,
        )
    } else {
        if config.is_some() || model.is_some() || upstream.is_some() {
            return Err("provider config/model/upstream require --provider openrouter".into());
        }
        Box::new(ScriptedProvider::new(vec![ScriptStep::reply(
            prompt.clone().unwrap_or_default(),
            "Scripted complete. No model request or tools executed.",
        )]))
    };
    let mut c = Config::new(workspace, data_dir);
    c.max_model_attempts = max_requests;
    c.max_tool_calls = max_tools;
    c.turn_timeout = Duration::from_secs(active_seconds);
    c.max_output_bytes = max_output_bytes;
    c.context_tokens = context_tokens;
    c.user_instructions = instructions;
    c.policy = if policy == "inspect" {
        Policy::Inspect
    } else {
        Policy::Supervised
    };
    let mut engine = Engine::open(c, backend).await.map_err(|e| e.to_string())?;
    let output = Output::new();
    let mut failure = output.failed.clone();
    let operation=async{
        let session=match session{Some(id)=>SessionId(id),None=>engine.create_session().await.map_err(|e|e.to_string())?};
        let mut turn=if let Some(id)=resume_turn{engine.resume(session,TurnId(id)).await}else{engine.submit(session,prompt.unwrap_or_default()).await}.map_err(|e|e.to_string())?;
        output.send(if json_mode{json_line(json!({"version":1,"kind":"accepted","session":session,"turn":turn.id}))}else{format!("session={} turn={} provider={}\n",session.0,turn.id.0,provider)})?;
        let mut progress=turn.progress_receiver();let mut displayed=String::new();
        let outcome=loop{
            tokio::select!{biased;
                o=turn.outcome()=>break o.map_err(|e|e.to_string())?,
                _=tokio::signal::ctrl_c()=>{turn.cancel();break turn.outcome().await.map_err(|e|e.to_string())?;},
                _=failure.changed()=>{turn.cancel();return Err("stdout observer lost; cancelling turn".into());},
                changed=progress.changed()=>{
                    if changed.is_err(){continue;}
                    let p=progress.borrow_and_update().clone();
                    let text=if json_mode{json_line(json!({"version":1,"kind":"progress","snapshot":p}))}else{
                        let delta=if p.text.starts_with(&displayed){p.text[displayed.len()..].to_owned()}else{format!("\n[attempt replaced; prior display is incomplete]\n{}",p.text)};
                        displayed=p.text;escaped(&delta)
                    };
                    if output.send(text).is_err(){turn.cancel();return Err("stdout stalled; cancelling turn".into());}
                }
            }
        };
        let usage=engine.usage(session).await.map_err(|e|e.to_string())?;
        Ok::<_,String>((session,outcome,usage))
    }.await;
    let shutdown = engine.shutdown().await;
    let (session, outcome, usage) = operation?;
    shutdown.map_err(|e| e.to_string())?;
    let final_output = if json_mode {
        json_line(
            json!({"version":1,"kind":"terminal","session":session,"outcome":outcome,"usage":usage}),
        )
    } else {
        let usage = usage
            .iter()
            .map(|u| {
                u.raw_usage
                    .as_ref()
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "unavailable (not zero)".into())
            })
            .collect::<Vec<_>>()
            .join("; ");
        format!(
            "\nstate={:?} reason={:?}\n{}\nusage={}\n{}\n",
            outcome.state,
            outcome.reason,
            escaped(
                outcome
                    .text
                    .as_deref()
                    .unwrap_or("No completed final answer.")
            ),
            escaped(&usage),
            guidance(&outcome.reason)
        )
    };
    output.finish(final_output).await?;
    Ok(match outcome.state {
        TurnState::Completed => 0,
        TurnState::Cancelled => 130,
        TurnState::SuspendedLimit => 3,
        _ => 1,
    })
}
fn guidance(r: &StopReason) -> &'static str {
    match r {
        StopReason::Authentication => {
            "Check the configured credential reference, permissions and account/key budget."
        }
        StopReason::Credits => {
            "OpenRouter credits/key budget are exhausted. No alternate billing route was used."
        }
        StopReason::NoUpstream => {
            "No eligible upstream. Review the explicit endpoint tag and privacy/capability restrictions."
        }
        StopReason::Throttled | StopReason::Unavailable | StopReason::RetryableTransport => {
            "Transient retries exhausted. Retry explicitly after service recovery; prior attempts may be billable."
        }
        StopReason::ContextLimit
        | StopReason::RequestLimit
        | StopReason::ToolLimit
        | StopReason::OutputLimit
        | StopReason::Deadline => {
            "Turn suspended. Resume with --session and --resume-turn, explicitly raising the relevant host limit. Compaction is not available yet."
        }
        StopReason::Protocol | StopReason::InvalidRequest => {
            "Provider protocol/configuration failure. Partial calls were not executed; inspect adapter configuration before retrying."
        }
        StopReason::Refused => {
            "The provider refused this request; its prompt remains in local history."
        }
        _ => "",
    }
}
