use clap::{Parser, Subcommand};
use harness_core::{
    openrouter::{OpenRouterConfig, OpenRouterProvider},
    *,
};
use serde_json::json;
use std::{path::PathBuf, process::ExitCode, time::Duration};
mod chatgpt;
mod output;
use output::{Output, Transcript, escaped, json_line};
#[derive(Parser)]
#[command(
    name = "harness",
    version,
    about = "Bounded coding harness; explicit OpenRouter, direct ChatGPT plan, or offline scripted provider"
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Manage this application's own ChatGPT registration and renewable session.
    Auth(chatgpt::AuthArgs),
    /// Discover models for a selected ChatGPT registration.
    Models(chatgpt::ModelsArgs),
    /// Cancel saved unresolved work locally without credentials or inference.
    Abandon {
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        data_dir: Option<PathBuf>,
        #[arg(long)]
        session: i64,
        #[arg(long)]
        turn: i64,
        #[arg(long)]
        json: bool,
    },
    /// Inspect a workspace. OpenRouter sends scoped context to the gateway/upstream.
    Ask(Box<AskArgs>),
}
#[derive(clap::Args)]
struct AskArgs {
    #[arg(long)]
    workspace: PathBuf,
    #[arg(long)]
    data_dir: Option<PathBuf>,
    #[arg(long,default_value="scripted",value_parser=["scripted","openrouter","chatgpt"])]
    provider: String,
    /// Non-secret OpenRouter JSON configuration; unknown fields are rejected.
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    model: Option<String>,
    /// Explicit saved ChatGPT registration label (or set it in --config).
    #[arg(long)]
    profile: Option<String>,
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
    #[arg(long, conflicts_with = "session")]
    instructions: Option<PathBuf>,
    #[arg(long, default_value_t = 30)]
    max_requests: usize,
    #[arg(long, default_value_t = 60)]
    max_tools: usize,
    #[arg(long, default_value_t = 1200)]
    active_seconds: u64,
    #[arg(long, default_value_t = 262144)]
    max_output_bytes: usize,
    #[arg(long, default_value_t = 524288)]
    max_request_bytes: usize,
    #[arg(long, default_value_t = 128000)]
    context_tokens: usize,
    /// Versioned NDJSON output only; never consumes approval input.
    #[arg(long)]
    json: bool,
    #[arg(required_unless_present = "resume_turn")]
    prompt: Option<String>,
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
    let command = match args.command {
        Command::Auth(a) => return chatgpt::auth(a).await,
        Command::Models(a) => return chatgpt::models(a).await,
        c => c,
    };
    if let Command::Abandon {
        workspace,
        data_dir,
        session,
        turn,
        json,
    } = command
    {
        let config = Config::new(workspace, local_data_dir(data_dir)?);
        let mut engine = Engine::open(config, ScriptedProvider::new(vec![]))
            .await
            .map_err(|e| e.to_string())?;
        let outcome = engine.abandon(SessionId(session), TurnId(turn)).await;
        let shutdown = engine.shutdown().await;
        let outcome = outcome.map_err(|e| e.to_string())?;
        shutdown.map_err(|e| e.to_string())?;
        let output = Output::new(json);
        output
            .finish(if json {
                json_line(
                    json!({"version":1,"kind":"terminal","session":session,"outcome":outcome}),
                )
            } else {
                format!(
                    "session={} turn={} state={:?} reason={:?}\n",
                    session, turn, outcome.state, outcome.reason
                )
            })
            .await?;
        return Ok(0);
    }
    let Command::Ask(ask) = command else {
        unreachable!()
    };
    let AskArgs {
        workspace,
        data_dir,
        provider,
        config,
        model,
        profile,
        upstream,
        session,
        resume_turn,
        policy,
        instructions,
        max_requests,
        max_tools,
        active_seconds,
        max_output_bytes,
        max_request_bytes,
        context_tokens,
        json: json_mode,
        prompt,
    } = *ask;
    let data_dir = local_data_dir(data_dir)?;
    let backend: Box<dyn Provider> = if provider == "chatgpt" {
        chatgpt::backend(config, model, profile, upstream).await?
    } else if provider == "openrouter" {
        if profile.is_some() {
            return Err("--profile requires --provider chatgpt".into());
        }
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
        if config.is_some() || model.is_some() || upstream.is_some() || profile.is_some() {
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
    c.max_request_bytes = max_request_bytes;
    c.context_tokens = context_tokens;
    c.user_instructions = instructions;
    c.policy = if policy == "inspect" {
        Policy::Inspect
    } else {
        Policy::Supervised
    };
    let route = if provider == "chatgpt" {
        let b = backend.binding();
        Some(
            json!({"registration":b.account_ref,"model":b.model,"billing_route":"chatgpt-plan","usage":"https://chatgpt.com/settings/usage"}),
        )
    } else {
        None
    };
    let mut engine = Engine::open(c, backend).await.map_err(|e| e.to_string())?;
    let output = Output::new(json_mode);
    let mut failure = output.failed.clone();
    let operation=async{
        let session=match session{Some(id)=>SessionId(id),None=>engine.create_session().await.map_err(|e|e.to_string())?};
        let mut turn=if let Some(id)=resume_turn{engine.resume(session,TurnId(id)).await}else{engine.submit(session,prompt.unwrap_or_default()).await}.map_err(|e|e.to_string())?;
        output.start(if json_mode{json_line(json!({"version":1,"kind":"accepted","session":session,"turn":turn.id,"route":route}))}else{format!("session={} turn={} provider={} {}\n",session.0,turn.id.0,provider,route.as_ref().map(|v|escaped(&v.to_string())).unwrap_or_default())})?;
        let mut progress=turn.progress_receiver();let mut transcript=Transcript::default();
        let outcome=loop{
            tokio::select!{biased;
                o=turn.outcome()=>break o.map_err(|e|e.to_string())?,
                _=tokio::signal::ctrl_c()=>{turn.cancel();break turn.outcome().await.map_err(|e|e.to_string())?;},
                _=failure.changed()=>{turn.cancel();return Err("stdout observer lost; cancelling turn".into());},
                changed=progress.changed()=>{
                    if changed.is_err(){continue;}
                    let p=progress.borrow_and_update().clone();
                    let text=if json_mode{json_line(json!({"version":1,"kind":"progress","snapshot":p}))}else{
                        transcript.observe(&p);transcript.text.clone()
                    };
                    if output.send(text).is_err(){turn.cancel();return Err("stdout observer lost; cancelling turn".into());}
                }
            }
        };
        if !json_mode {
            transcript.observe(&progress.borrow());
            if let Some(text)=&outcome.text { transcript.append(text); }
        }
        let usage=engine.usage(session).await.map_err(|e|e.to_string())?;
        Ok::<_,String>((session,outcome,usage,transcript.text))
    }.await;
    let shutdown = engine.shutdown().await;
    let (session, outcome, usage, transcript) = operation?;
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
            "{}\nstate={:?} reason={:?}\n{}usage={}\n{}\n",
            transcript,
            outcome.state,
            outcome.reason,
            if outcome.text.is_none() {
                "No completed final answer.\n"
            } else {
                ""
            },
            escaped(&usage),
            guidance(&outcome.reason)
        )
    };
    output.finish(final_output).await?;
    Ok(match outcome.state {
        TurnState::Completed => 0,
        TurnState::Cancelled => 130,
        TurnState::SuspendedLimit | TurnState::SuspendedRecovery => 3,
        _ => 1,
    })
}
fn local_data_dir(path: Option<PathBuf>) -> std::result::Result<PathBuf, String> {
    path.or_else(|| std::env::var_os("LOCALAPPDATA").map(|p| PathBuf::from(p).join("z-harness")))
        .ok_or_else(|| "set --data-dir to a private local directory outside the workspace".into())
}
fn guidance(r: &StopReason) -> &'static str {
    match r {
        StopReason::ConsentRequired => {
            "Direct ChatGPT plan usage was not granted. Run auth login chatgpt --profile <saved-profile> --enable-plan-usage, then explicitly resume."
        }
        StopReason::ReauthorizationRequired => {
            "ChatGPT credentials are unavailable or rotation is uncertain. Sign in with the same saved profile, then resume; do not create a replacement registration for this session."
        }
        StopReason::SubscriptionLimit => {
            "ChatGPT app usage is limited. Check https://chatgpt.com/settings/usage and resume after recovery. Reset time and remaining allowance are unknown; billing route was not changed."
        }
        StopReason::AccessBlocked => {
            "ChatGPT access is blocked. Check account/workspace eligibility, policy and region; repeated sign-in is not a fix. Live qualification remains issue #9."
        }
        StopReason::AccountDiagnosis => {
            "ChatGPT profile may be in use, its protected store unavailable, or account recovery did not authorize this route. Retry after local contention or diagnose grant/service state before explicitly resuming. No repeated OAuth loop."
        }
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
        | StopReason::Deadline => {
            "Turn suspended. Resume with --session and --resume-turn, raising the relevant limit, or use `harness abandon --workspace ... --session ... --turn ...`. Compaction is not available yet."
        }
        StopReason::OutputLimit => {
            "Turn suspended. For provider token exhaustion, raise max_output_tokens in the provider JSON; for cumulative local output, raise --max-output-bytes. Resume with --session and --resume-turn, or abandon locally. Reasoning counts toward the provider cap; prior attempts may be billable."
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
