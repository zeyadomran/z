use crate::{
    store::Store,
    workspace::{
        Workspace, hash,
        patch::{Intent, Prepared, Reconciliation},
    },
    *,
};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::Serialize;

pub(crate) const POLICY_VERSION: &str = "supervised-patches-v1";
#[derive(Clone, Debug, Serialize)]
pub struct PatchPreview {
    pub approval_id: String,
    pub session: SessionId,
    pub turn: TurnId,
    pub call_id: String,
    pub workspace: String,
    pub policy_version: String,
    pub arguments: serde_json::Value,
    pub before_sha256: Option<String>,
    pub after_sha256: String,
    pub metadata: serde_json::Value,
    pub instructions: Vec<Instruction>,
    #[serde(skip)]
    pub(crate) prepared: Prepared,
    #[serde(skip)]
    pub(crate) batch: i64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalDecision {
    Approve,
    Deny,
}
pub(crate) struct Authorization {
    pub preview: PatchPreview,
    pub decision: ApprovalDecision,
}
pub(crate) async fn preview(
    store: &Store,
    config: &Config,
    binding: ProviderBinding,
    session: SessionId,
    turn: TurnId,
) -> Result<PatchPreview> {
    if config.policy != Policy::Supervised {
        return Err(Error::Scope("inspect mode denies patches".into()));
    }
    let workspace = config.workspace_identity()?;
    let w = workspace.clone();
    let (approval_id, batch, call_id, arguments) = store.call(move |c| {
        records::binding(c, session, &w, &binding)?;
        let state: String = c.query_row("SELECT state FROM turns WHERE id=?1 AND session_id=?2", params![turn.0,session.0], |r|r.get(0))?;
        if state != "awaiting_approval" { return Err(Error::Busy); }
        let row = c.query_row("SELECT batch_id,provider_id,name,arguments FROM tool_calls WHERE turn_id=?1 AND state!='resolved' ORDER BY ordinal LIMIT 1", [turn.0], |r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?)))?;
        if row.2 != "apply_patch" { return Err(Error::Busy); }
        let id: String = c.query_row("SELECT lower(hex(randomblob(32)))", [], |r|r.get(0))?;
        Ok((id,row.0,row.1,row.3))
    }).await?;
    let workspace_tools = Workspace::new(config);
    let prepared = tokio::task::spawn_blocking(move || workspace_tools.prepare_patch(&arguments))
        .await
        .map_err(|_| Error::Worker)??;
    Ok(PatchPreview {
        approval_id,
        session,
        turn,
        call_id,
        workspace,
        policy_version: POLICY_VERSION.into(),
        arguments: serde_json::to_value(&prepared.args)?,
        before_sha256: prepared.before.as_deref().map(hash),
        after_sha256: hash(&prepared.after),
        metadata: serde_json::json!({"original":prepared.before_metadata,"parent":prepared.parent_metadata,"create_inherits_parent":prepared.before.is_none()}),
        instructions: prepared.instructions.clone(),
        prepared,
        batch,
    })
}
pub(crate) async fn last_intent(
    store: &Store,
    batch: i64,
    call: String,
) -> Result<Option<(i64, Intent)>> {
    store.call(move |c| {
        let row: Option<(i64,String)> = c.query_row("SELECT p.id,p.intent FROM patch_attempts p JOIN tool_calls tc ON tc.id=p.call_id WHERE tc.batch_id=?1 AND tc.provider_id=?2 ORDER BY p.id DESC LIMIT 1", params![batch,call], |r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        row.map(|(id,text)|Ok((id,serde_json::from_str(&text)?))).transpose()
    }).await
}
pub(crate) async fn evidence(
    store: &Store,
    attempt: i64,
    state: &'static str,
    detail: String,
) -> Result<()> {
    let write = || {
        let detail = detail.clone();
        store.call(move |c| {
            c.execute(
                "UPDATE patch_attempts SET state=?1,evidence=?2 WHERE id=?3",
                params![state, detail, attempt],
            )?;
            Ok(())
        })
    };
    let result = write().await;
    if matches!(result, Err(Error::Busy)) {
        write().await
    } else {
        result
    }
}
pub(crate) async fn reconcile(
    store: &Store,
    config: &Config,
    batch: i64,
    call: String,
) -> Result<Option<Reconciliation>> {
    let Some((id, intent)) = last_intent(store, batch, call).await? else {
        return Ok(None);
    };
    let w = Workspace::new(config);
    let result = tokio::task::spawn_blocking(move || w.reconcile_patch(&intent))
        .await
        .map_err(|_| Error::Worker)?;
    let state = match result {
        Reconciliation::Applied => "applied",
        Reconciliation::Unchanged => "unchanged",
        Reconciliation::Unknown => "unknown",
    };
    evidence(
        store,
        id,
        state,
        format!("reconciled bytes and supported metadata: {state}; recorded artifacts retained"),
    )
    .await?;
    Ok(Some(result))
}
pub(crate) async fn execute(
    store: &Store,
    config: &Config,
    authorization: Authorization,
    stopped: impl Fn() -> Option<StopReason>,
) -> Result<serde_json::Value> {
    if let Some(reason) = stopped() {
        return Ok(
            serde_json::json!({"status":"cancelled","reason":reason,"error":"stopped before patch intent"}),
        );
    }
    let preview = authorization.preview;
    let w = Workspace::new(config);
    let prepared = preview.prepared.clone();
    let check = tokio::task::spawn_blocking(move || w.revalidate(&prepared))
        .await
        .map_err(|_| Error::Worker)?;
    if check.is_err() {
        return Ok(
            serde_json::json!({"status":"stale","error":"approval invalidated; reread and propose a fresh patch"}),
        );
    }
    let known = journal::instruction_keys(store, preview.turn).await?;
    let instructions: Vec<_> = preview
        .instructions
        .iter()
        .filter(|i| !known.contains(&(i.origin.clone(), i.sha256.clone())))
        .cloned()
        .collect();
    let parent = preview.prepared.target.parent().ok_or(Error::Worker)?;
    let mut intent = Intent {
        prepared: preview.prepared.clone(),
        staging: parent.join(format!(".harness-patch-{}-stage", preview.approval_id)),
        backup: parent.join(format!(".harness-patch-{}-backup", preview.approval_id)),
        expected_metadata: preview.prepared.before_metadata.clone(),
    };
    let initial = serde_json::to_string(&intent)?;
    let binding = serde_json::to_string(&preview)?;
    let turn = preview.turn;
    let attempt=store.call(move |c| {
        let tx=c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let id:i64=tx.query_row("SELECT id FROM tool_calls WHERE batch_id=?1 AND provider_id=?2 AND state!='resolved'",params![preview.batch,preview.call_id],|r|r.get(0))?;
        tx.execute("UPDATE tool_calls SET state='intent' WHERE id=?1",[id])?;
        tx.execute("INSERT INTO patch_attempts(call_id,approval_id,binding,intent,state) VALUES(?1,?2,?3,?4,'intent')",params![id,preview.approval_id,binding,initial])?;
        let attempt=tx.last_insert_rowid();
        records::event(&tx,preview.session,turn,EventKind::ApprovalResolved{turn_id:turn,call_id:preview.call_id.clone(),approval_id:preview.approval_id.clone(),approved:true})?;
        tx.execute("UPDATE turns SET tool_count=tool_count+1 WHERE id=?1",[turn.0])?;
        tx.commit()?; Ok(attempt)
    }).await?;
    let w = Workspace::new(config);
    let (staged, next) = tokio::task::spawn_blocking(move || {
        let result = w.stage_patch(&mut intent);
        (result, intent)
    })
    .await
    .map_err(|_| Error::Worker)?;
    intent = next;
    if let Err(e) = staged {
        evidence(store, attempt, "unchanged", e.to_string()).await?;
        return Ok(serde_json::json!({"status":"failed","error":e.to_string(),"attempt":attempt}));
    }
    let serialized = serde_json::to_string(&intent)?;
    store
        .call(move |c| {
            c.execute(
                "UPDATE patch_attempts SET intent=?1,state='staged' WHERE id=?2",
                params![serialized, attempt],
            )?;
            Ok(())
        })
        .await?;
    if let Some(reason) = stopped() {
        evidence(
            store,
            attempt,
            "unchanged",
            "cancelled before native replacement; staging retained".into(),
        )
        .await?;
        return Ok(serde_json::json!({"status":"cancelled","reason":reason,"attempt":attempt}));
    }
    let w = Workspace::new(config);
    let outcome = tokio::task::spawn_blocking(move || w.replace_patch(&intent))
        .await
        .map_err(|_| Error::Worker)?;
    match outcome {
        Ok(()) => {
            evidence(
                store,
                attempt,
                "applied",
                "native replacement verified: bytes and supported metadata; artifacts retained"
                    .into(),
            )
            .await?;
            Ok(
                serde_json::json!({"status":"succeeded","attempt":attempt,"after_sha256":preview.after_sha256,"instructions":instructions}),
            )
        }
        Err(e) => {
            evidence(store, attempt, "unknown", e.to_string()).await?;
            Ok(
                serde_json::json!({"status":"unknown","error":"replacement requires metadata-aware reconciliation","attempt":attempt}),
            )
        }
    }
}
pub(crate) async fn reconcile_for_abandon(
    store: &Store,
    config: &Config,
    session: SessionId,
    turn: TurnId,
) -> Result<()> {
    let Some((batch, calls)) = crate::pending::batch(store, turn).await? else {
        return Ok(());
    };
    let ms = journal::budget(store, turn).await?.active_ms;
    for saved in calls {
        if saved.resolved || saved.call.name != "apply_patch" {
            continue;
        }
        if let Some(Reconciliation::Applied) =
            reconcile(store, config, batch, saved.call.id.clone()).await?
        {
            let result = reconciled_result(store, turn, batch, saved.call.id.clone()).await?;
            journal::tool_result(store, session, turn, batch, saved.call.id, result, ms).await?;
        }
    }
    Ok(())
}
pub(crate) async fn reconciled_result(
    store: &Store,
    turn: TurnId,
    batch: i64,
    call: String,
) -> Result<String> {
    let (attempt, intent) = last_intent(store, batch, call)
        .await?
        .ok_or(Error::Worker)?;
    let known = journal::instruction_keys(store, turn).await?;
    let instructions: Vec<_> = intent
        .prepared
        .instructions
        .into_iter()
        .filter(|i| !known.contains(&(i.origin.clone(), i.sha256.clone())))
        .collect();
    Ok(serde_json::json!({"status":"succeeded","reconciled":true,"attempt":attempt,"after_sha256":hash(&intent.prepared.after),"verification":"whole raw bytes and supported metadata","instructions":instructions}).to_string())
}
