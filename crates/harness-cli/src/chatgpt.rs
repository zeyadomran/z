use crate::output::{Output, escaped, json_line};
use clap::{Args, Subcommand};
use harness_core::{
    Provider,
    auth::{ChatGptAuth, PRODUCT_NAME},
    chatgpt::{ChatGptConfig, ChatGptProvider},
};
use serde_json::json;
use std::path::PathBuf;
#[derive(Args)]
pub struct AuthArgs {
    #[command(subcommand)]
    pub command: AuthCommand,
}
#[derive(Subcommand)]
pub enum AuthCommand {
    /// Continue with ChatGPT using this application's own registration.
    Login {
        #[arg(value_parser=["chatgpt"])]
        provider: String,
        /// Stable local registration label; use a separate label for another account/workspace.
        #[arg(long)]
        profile: String,
        /// Explicitly request direct plan usage consent again.
        #[arg(long)]
        enable_plan_usage: bool,
    },
    /// Show saved registration and grant state without making a network request.
    Status {
        #[arg(long)]
        profile: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Revoke the renewable session and clear local tokens, retaining history.
    Logout {
        #[arg(value_parser=["chatgpt"])]
        provider: String,
        #[arg(long)]
        profile: String,
        #[arg(long)]
        json: bool,
    },
    /// Explicitly rotate a saved renewable session; never switches account/billing.
    Refresh {
        #[arg(value_parser=["chatgpt"])]
        provider: String,
        #[arg(long)]
        profile: String,
        #[arg(long)]
        json: bool,
    },
}
#[derive(Args)]
pub struct ModelsArgs {
    #[arg(long,value_parser=["chatgpt"])]
    provider: String,
    #[arg(long)]
    profile: String,
    #[arg(long)]
    json: bool,
}
async fn print(value: serde_json::Value, json: bool) -> Result<u8, String> {
    Output::new(json)
        .finish(if json {
            json_line(json!({"version":1,"kind":"chatgpt","data":value}))
        } else {
            format!(
                "{}\n",
                escaped(&serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?)
            )
        })
        .await?;
    Ok(0)
}
pub async fn auth(args: AuthArgs) -> Result<u8, String> {
    let mut auth = ChatGptAuth::local().map_err(|e| e.to_string())?;
    match args.command {
        AuthCommand::Login {
            profile,
            enable_plan_usage,
            ..
        } => {
            eprintln!(
                "Continue with ChatGPT for {PRODUCT_NAME}. Direct account eligibility remains unqualified until M6; no inference is part of sign-in."
            );
            let login = auth
                .begin_login(&profile, enable_plan_usage)
                .await
                .map_err(|e| e.to_string())?;
            login.open_browser().map_err(|e| e.to_string())?;
            let result = tokio::select! {r=login.finish()=>r.map_err(|e|e.to_string())?,_=tokio::signal::ctrl_c()=>return Err("sign-in cancelled; callback listener closed".into())};
            print(json!({"account":result,"route":"chatgpt-plan","usage":"https://chatgpt.com/settings/usage"}),false).await
        }
        AuthCommand::Status { profile, json } => {
            let statuses = if let Some(p) = profile {
                vec![auth.status(&p).map_err(|e| e.to_string())?]
            } else {
                auth.profiles().map_err(|e| e.to_string())?
            };
            print(
                json!({"accounts":statuses,"live_qualification":"unproven; see issue #9"}),
                json,
            )
            .await
        }
        AuthCommand::Logout { profile, json, .. } => {
            let confirmed = tokio::select! {r=auth.logout(&profile)=>r.map_err(|e|e.to_string())?,_=tokio::signal::ctrl_c()=>return Err("logout interrupted; requests remain stopped; rerun logout to finish local cleanup; remote revocation unconfirmed; https://chatgpt.com/settings".into())};
            print(json!({"profile":profile,"local_logout":true,"remote_revocation_confirmed":confirmed,"settings":"https://chatgpt.com/settings"}),json).await
        }
        AuthCommand::Refresh { profile, json, .. } => {
            let status = tokio::select! {r=auth.refresh(&profile)=>r.map_err(|e|e.to_string())?,_=tokio::signal::ctrl_c()=>return Err("refresh cancelled; if transmission began, fresh authorization is required".into())};
            print(
                serde_json::to_value(status).map_err(|e| e.to_string())?,
                json,
            )
            .await
        }
    }
}
pub async fn models(args: ModelsArgs) -> Result<u8, String> {
    let mut auth = ChatGptAuth::local().map_err(|e| e.to_string())?;
    let catalog = auth
        .catalog(&args.profile)
        .await
        .map_err(|e| e.to_string())?;
    print(json!({"profile":args.profile,"catalog":catalog,"route":"chatgpt-plan","limits":"configure per exact slug; catalog does not supply context limits","usage":"https://chatgpt.com/settings/usage"}),args.json).await
}
pub async fn backend(
    path: Option<PathBuf>,
    model: Option<String>,
    profile: Option<String>,
    upstream: Option<String>,
) -> Result<Box<dyn Provider>, String> {
    if upstream.is_some() {
        return Err("ChatGPT has no upstream override".into());
    }
    let path=path.ok_or("ChatGPT requires --config with model, profile, context_tokens, response_headroom and limits_source")?;
    let bytes = std::fs::read(path).map_err(|_| "cannot read ChatGPT config")?;
    if bytes.len() > 16384 {
        return Err("ChatGPT config exceeds 16 KiB".into());
    }
    let mut c: ChatGptConfig = serde_json::from_slice(&bytes)
        .map_err(|_| "invalid ChatGPT config; see docs/chatgpt-subscription/usage.md")?;
    if let Some(m) = model {
        if m != c.model {
            return Err(
                "model override must match configured model limits; update --config explicitly"
                    .into(),
            );
        }
        c.model = m;
    }
    if let Some(p) = profile {
        c.profile = p;
    }
    let auth = ChatGptAuth::local().map_err(|e| e.to_string())?;
    Ok(Box::new(
        ChatGptProvider::new(auth, c)
            .await
            .map_err(|e| e.to_string())?,
    ))
}
