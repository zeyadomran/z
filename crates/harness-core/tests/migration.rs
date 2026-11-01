use harness_core::*;
use rusqlite::{Connection, params};
#[tokio::test]
async fn m0_migration_keeps_history_binding_and_consistent_backup() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let data = root.path().join("data");
    std::fs::create_dir(&data).unwrap();
    let c = Connection::open(data.join("harness.sqlite")).unwrap();
    c.execute_batch(include_str!("../src/schema.sql")).unwrap();
    c.pragma_update(None, "application_id", 0x5a484152_i64)
        .unwrap();
    c.pragma_update(None, "user_version", 1).unwrap();
    let w = std::fs::canonicalize(&workspace)
        .unwrap()
        .to_str()
        .unwrap()
        .to_lowercase();
    c.execute("INSERT INTO sessions(id,workspace,provider,account_ref,model,options) VALUES(1,?1,'scripted','local','script-v1','{}')",[&w]).unwrap();
    c.execute("INSERT INTO epochs(id,session_id,state,prefix,tool_schema) VALUES(1,1,'active','M0 fixed','[]')",[]).unwrap();
    c.execute("INSERT INTO turns(id,session_id,workspace,epoch_id,state,limits) VALUES(1,1,?1,1,'completed','{}')",[&w]).unwrap();
    for (role, text) in [("user", "old prompt"), ("assistant", "old answer")] {
        c.execute(
            "INSERT INTO messages(turn_id,epoch_id,role,content) VALUES(1,1,?1,?2)",
            params![role, text],
        )
        .unwrap();
    }
    c.close().unwrap();
    let mut engine = Engine::open(
        Config::new(workspace, data.clone()),
        ScriptedProvider::new(vec![ScriptStep::reply("new prompt", "new answer")]),
    )
    .await
    .unwrap();
    assert_eq!(
        engine.history(SessionId(1)).await.unwrap(),
        vec![
            Message {
                role: Role::User,
                text: "old prompt".into()
            },
            Message {
                role: Role::Assistant,
                text: "old answer".into()
            }
        ]
    );
    assert_eq!(
        engine
            .protocol_history(SessionId(1))
            .await
            .unwrap()
            .epoch
            .prefix,
        "M0 fixed"
    );
    let mut turn = engine
        .submit(SessionId(1), "new prompt".into())
        .await
        .unwrap();
    assert_eq!(turn.outcome().await.unwrap().state, TurnState::Completed);
    engine.shutdown().await.unwrap();
    let backup = Connection::open(data.join("harness.v1.backup.sqlite")).unwrap();
    assert_eq!(
        backup
            .pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        backup
            .query_row("SELECT count(*) FROM messages", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(
        backup
            .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
}
