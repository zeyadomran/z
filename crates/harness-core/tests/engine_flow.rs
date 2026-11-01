use harness_core::{
    Config, Engine, EventKind, EventSequence, Message, Role, ScriptStep, ScriptedProvider,
    TurnState,
};

// Extend this owner with real scoped tools/approvals as the subsequent milestones land.
#[tokio::test]
async fn completed_turn_and_history_survive_reopen() {
    let root = tempfile::tempdir().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let config = Config::new(workspace, root.path().join("data"));
    let script = ScriptedProvider::new(vec![ScriptStep::reply("hello", "M0 complete.")]);
    let mut engine = Engine::open(config.clone(), script).await.unwrap();
    let session = engine.create_session().await.unwrap();
    let mut turn = engine.submit(session, "hello".into()).await.unwrap();
    let outcome = turn.outcome().await.unwrap();
    assert_eq!(outcome.state, TurnState::Completed);
    assert_eq!(outcome.text.as_deref(), Some("M0 complete."));
    let events = engine.events(session, EventSequence(0)).await.unwrap();
    assert_eq!(
        events
            .iter()
            .map(|event| event.sequence.0)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(
        events[2].event,
        EventKind::TurnStopped {
            outcome: outcome.clone()
        }
    );
    assert_eq!(turn.latest_sequence(), EventSequence(3));
    engine.shutdown().await.unwrap();
    let mut reopened = Engine::open(
        config,
        ScriptedProvider::new(vec![ScriptStep::reply("again", "Still here.")]),
    )
    .await
    .unwrap();
    assert_eq!(
        reopened.history(session).await.unwrap(),
        vec![
            Message {
                role: Role::User,
                text: "hello".into()
            },
            Message {
                role: Role::Assistant,
                text: "M0 complete.".into()
            },
        ]
    );
    assert_eq!(
        reopened.turn_outcome(session, turn.id).await.unwrap(),
        Some(outcome)
    );
    assert_eq!(
        reopened.events(session, EventSequence(0)).await.unwrap(),
        events
    );
    let mut second = reopened.submit(session, "again".into()).await.unwrap();
    assert_eq!(
        second.outcome().await.unwrap().text.as_deref(),
        Some("Still here.")
    );
    assert_eq!(
        reopened
            .events(session, EventSequence(3))
            .await
            .unwrap()
            .iter()
            .map(|event| event.sequence.0)
            .collect::<Vec<_>>(),
        vec![4, 5, 6]
    );
    assert_eq!(reopened.history(session).await.unwrap().len(), 4);
    reopened.shutdown().await.unwrap();
}
