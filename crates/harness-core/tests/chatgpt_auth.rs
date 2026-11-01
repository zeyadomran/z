#![cfg(windows)]
use harness_core::*;
use serde_json::json;
#[path = "support/chatgpt.rs"]
mod fixture;
use fixture::*;
#[tokio::test]
async fn registration_identity_rotation_and_logout_survive_reopening_the_protected_vault() {
    let server = Server::start(vec![]).await;
    let root = tempfile::tempdir().unwrap();
    let mut auth = server.auth(root.path());
    let host = auth.host_id().unwrap();
    assert!(host.starts_with("urn:uuid:"));
    let initial = server.login(&auth, "one").await.unwrap();
    let identity = initial.identity.unwrap();
    assert_eq!(identity.client_id, "issued-one");
    assert_eq!(identity.subject, "subject-one");
    assert!(initial.direct_use);
    assert_eq!(
        auth.models("one").await.unwrap()[0].display_name,
        "Fixture model"
    );
    let protected = std::fs::read(root.path().join("credentials.chatgpt/one.vault")).unwrap();
    assert!(!protected.windows(14).any(|w| w == b"fixture-access"));
    drop(auth);
    let mut auth = server.auth(root.path());
    assert_eq!(host, auth.host_id().unwrap());
    assert_eq!(auth.status("one").unwrap().identity, Some(identity.clone()));
    assert_eq!(
        auth.refresh("one").await.unwrap().identity,
        Some(identity.clone())
    );
    // A separate handle cannot change/logout a profile held by a pending login.
    let pending = auth.begin_login("one", false).await.unwrap();
    assert!(matches!(
        server.auth(root.path()).refresh("one").await,
        Err(Error::Busy)
    ));
    let mut logout_auth = server.auth(root.path());
    {
        use std::{future::Future, task::Poll};
        let logout = logout_auth.logout("one");
        tokio::pin!(logout);
        std::future::poll_fn(|cx| {
            assert!(logout.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }
    assert!(server.finish_login(pending, "one").await.is_err());
    assert_eq!(auth.status("one").unwrap().state, "logout_requested");
    assert!(auth.models("one").await.is_err());
    server.login(&auth, "one").await.unwrap();
    server.state.lock().unwrap().subject = "different-subject".into();
    assert!(server.login(&auth, "one").await.is_err());
    assert_eq!(auth.status("one").unwrap().identity, Some(identity.clone()));
    server.state.lock().unwrap().subject = "subject-one".into();
    assert!(auth.logout("one").await.unwrap());
    assert!(server.state.lock().unwrap().revoke);
    drop(auth);
    let auth = server.auth(root.path());
    let status = auth.status("one").unwrap();
    assert_eq!(status.state, "logged_out");
    assert!(!status.direct_use);
    assert_eq!(status.identity, Some(identity));
    assert_eq!(host, auth.host_id().unwrap());
    assert!(server.login(&auth, "one").await.unwrap().direct_use);
    server.close().await;
}
#[tokio::test]
async fn rejected_callback_or_id_token_cannot_replace_a_verified_account() {
    let server = Server::start(vec![]).await;
    let root = tempfile::tempdir().unwrap();
    let auth = server.auth(root.path());
    server.login(&auth, "one").await.unwrap();
    let before = auth.status("one").unwrap().identity;
    server.state.lock().unwrap().bad_signature = true;
    assert!(server.login(&auth, "one").await.is_err());
    assert_eq!(auth.status("one").unwrap().identity, before);
    for override_claims in [
        json!({"nonce":"wrong"}),
        json!({"aud":"other-client"}),
        json!({"iss":"https://untrusted.invalid"}),
        json!({"exp":1}),
        json!({"aud":["issued-one","other"],"azp":"other"}),
        json!({"iat":jsonwebtoken::get_current_timestamp()+300}),
        json!({"sub":""}),
    ] {
        server.state.lock().unwrap().token_override = Some(override_claims);
        assert!(server.login(&auth, "one").await.is_err());
        assert_eq!(auth.status("one").unwrap().identity, before);
    }
    for invalid in [
        "state=wrong&code=fixture-code",
        "state={state}&state={state}&code=fixture-code",
        "state=wrong&error=access_denied",
        "state={state}&error=access_denied",
        "state={state}&code=fixture-code&client_id=other",
    ] {
        let login = auth.begin_login("one", false).await.unwrap();
        let u = reqwest::Url::parse(login.authorization_url()).unwrap();
        let q: std::collections::BTreeMap<_, _> = u
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        let callback = format!(
            "{}?{}",
            q["redirect_uri"],
            invalid.replace("{state}", &q["state"])
        );
        let send = tokio::spawn(async move {
            reqwest::Client::builder()
                .no_proxy()
                .build()
                .unwrap()
                .get(callback)
                .send()
                .await
                .unwrap()
        });
        assert!(login.finish().await.is_err());
        send.await.unwrap();
        assert_eq!(auth.status("one").unwrap().identity, before);
    }
    let expired = auth.begin_login("one", false).await.unwrap();
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(301)).await;
    assert!(expired.finish().await.is_err());
    tokio::time::resume();
    server.close().await;
}
#[tokio::test]
async fn uncertain_or_terminal_rotation_is_never_replayed_and_missing_scope_disables_inference() {
    let server = Server::start(vec![]).await;
    let root = tempfile::tempdir().unwrap();
    let mut auth = server.auth(root.path());
    server.state.lock().unwrap().direct = false;
    assert!(!server.login(&auth, "one").await.unwrap().direct_use);
    assert!(auth.models("one").await.is_err());
    server.state.lock().unwrap().direct = true;
    server.login(&auth, "one").await.unwrap();
    for response in [
        Reply {
            status: 200,
            body: "not json".into(),
            kind: "application/json",
        },
        Reply::json(400, json!({"error":"invalid_grant"})),
    ] {
        server.state.lock().unwrap().refresh_reply = Some(response);
        assert!(auth.refresh("one").await.is_err());
        let count = server.state.lock().unwrap().refreshes;
        drop(auth);
        auth = server.auth(root.path());
        assert!(auth.models("one").await.is_err());
        assert_eq!(server.state.lock().unwrap().refreshes, count);
        assert!(!auth.status("one").unwrap().direct_use);
        server.login(&auth, "one").await.unwrap();
    }
    server.state.lock().unwrap().refresh_reply =
        Some(Reply::json(503, json!({"error":"temporarily_unavailable"})));
    assert!(auth.refresh("one").await.is_err());
    assert!(auth.status("one").unwrap().direct_use);
    assert!(auth.models("one").await.is_ok());
    server.close().await;
}
#[tokio::test]
async fn cancelled_refresh_and_failed_atomic_save_leave_only_unusable_generations() {
    let server = Server::start(vec![]).await;
    let root = tempfile::tempdir().unwrap();
    let mut auth = server.auth(root.path());
    server.login(&auth, "one").await.unwrap();
    let (entered, wait) = tokio::sync::oneshot::channel();
    let (release, gate) = tokio::sync::oneshot::channel();
    server.state.lock().unwrap().pause = Some((entered, gate));
    {
        let refresh = auth.refresh("one");
        tokio::pin!(refresh);
        tokio::select! {r=&mut refresh=>panic!("refresh completed before controlled cancellation: {r:?}"),_=wait=>{}}
        // Dropping after the server received a refresh simulates interruption
        // at an uncertain external boundary, without sleeping or killing a user process.
    }
    release.send(()).unwrap();
    drop(auth);
    let mut auth = server.auth(root.path());
    assert_eq!(auth.status("one").unwrap().state, "rotation_uncertain");
    assert!(auth.models("one").await.is_err());
    server.login(&auth, "one").await.unwrap();
    let (entered, wait) = tokio::sync::oneshot::channel();
    let (release, gate) = tokio::sync::oneshot::channel();
    server.state.lock().unwrap().pause = Some((entered, gate));
    let target = root.path().join("credentials.chatgpt/one.vault");
    let saved = root
        .path()
        .join("credentials.chatgpt/owned-interrupted-record");
    {
        let refresh = auth.refresh("one");
        tokio::pin!(refresh);
        tokio::select! {r=&mut refresh=>panic!("refresh completed before persistence fault: {r:?}"),_=wait=>{}}
        std::fs::rename(&target, &saved).unwrap();
        std::fs::create_dir(&target).unwrap();
        release.send(()).unwrap();
        assert!(
            refresh
                .await
                .unwrap_err()
                .to_string()
                .contains("credential")
        );
    }
    std::fs::remove_dir(&target).unwrap();
    std::fs::rename(saved, target).unwrap();
    drop(auth);
    let mut auth = server.auth(root.path());
    assert_eq!(auth.status("one").unwrap().state, "rotation_uncertain");
    let sent = server.state.lock().unwrap().refreshes;
    assert!(auth.models("one").await.is_err());
    assert_eq!(server.state.lock().unwrap().refreshes, sent);
    server.close().await;
}
