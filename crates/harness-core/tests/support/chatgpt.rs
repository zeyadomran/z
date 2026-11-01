use harness_core::auth::*;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
};
pub struct Reply {
    pub status: u16,
    pub body: String,
    pub kind: &'static str,
}
impl Reply {
    pub fn json(status: u16, v: Value) -> Self {
        Self {
            status,
            body: v.to_string(),
            kind: "application/json",
        }
    }
}
pub struct State {
    pub nonce: String,
    pub subject: String,
    pub direct: bool,
    pub token_override: Option<Value>,
    pub refresh_reply: Option<Reply>,
    pub replies: VecDeque<Reply>,
    pub requests: Vec<(String, Value)>,
    pub refreshes: usize,
    pub revoke: bool,
    pub token_generation: usize,
    pub pause: Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>,
    pub challenge: String,
    pub bad_signature: bool,
}
pub struct Server {
    pub base: String,
    pub state: Arc<Mutex<State>>,
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}
impl Server {
    pub async fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State {
            nonce: String::new(),
            subject: "subject-one".into(),
            direct: true,
            token_override: None,
            refresh_reply: None,
            replies: replies.into(),
            requests: vec![],
            refreshes: 0,
            revoke: false,
            token_generation: 0,
            pause: None,
            challenge: String::new(),
            bad_signature: false,
        }));
        let (stop, mut rx) = oneshot::channel();
        let shared = state.clone();
        let origin = base.clone();
        let task = tokio::spawn(async move {
            loop {
                let (stream, _) =
                    tokio::select! { _=&mut rx=>break, r=listener.accept()=>r.unwrap() };
                tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    handle(stream, &origin, &shared),
                )
                .await
                .unwrap();
            }
        });
        Self {
            base,
            state,
            stop,
            task,
        }
    }
    pub fn auth(&self, root: &std::path::Path) -> ChatGptAuth {
        ChatGptAuth::loopback_fixture(root.join("credentials.chatgpt"), self.base.clone()).unwrap()
    }
    pub async fn login(
        &self,
        auth: &ChatGptAuth,
        profile: &str,
    ) -> harness_core::Result<AccountStatus> {
        let pending = auth.begin_login(profile, false).await?;
        self.finish_login(pending, profile).await
    }
    pub async fn finish_login(
        &self,
        pending: Login,
        profile: &str,
    ) -> harness_core::Result<AccountStatus> {
        let url = reqwest::Url::parse(pending.authorization_url()).unwrap();
        let q: BTreeMap<_, _> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["resource"], "https://api.openai.com/v1");
        assert_eq!(q["code_challenge"].len(), 43);
        {
            let mut s = self.state.lock().unwrap();
            s.nonce = q["nonce"].clone();
            s.challenge = q["code_challenge"].clone();
        }
        let client = if q["client_id"] == "dynamic_agent_client" {
            format!("issued-{profile}")
        } else {
            q["client_id"].clone()
        };
        let callback = format!(
            "{}?state={}&code=fixture-code&client_id={client}",
            q["redirect_uri"], q["state"]
        );
        let browser = tokio::spawn(async move {
            reqwest::Client::builder()
                .no_proxy()
                .build()
                .unwrap()
                .get(callback)
                .send()
                .await
                .unwrap()
        });
        let result = pending.finish().await;
        browser.await.unwrap();
        result
    }
    pub async fn close(self) {
        let _ = self.stop.send(());
        self.task.await.unwrap();
    }
}
async fn handle(mut stream: TcpStream, base: &str, state: &Arc<Mutex<State>>) {
    let mut bytes = Vec::new();
    let end;
    loop {
        let mut b = [0; 1024];
        let n = stream.read(&mut b).await.unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&b[..n]);
        if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
            end = i + 4;
            break;
        }
        assert!(bytes.len() < 65536);
    }
    let head = String::from_utf8(bytes[..end].to_vec()).unwrap();
    let path = head
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap();
    let headers: BTreeMap<_, _> = head
        .lines()
        .skip(1)
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.to_ascii_lowercase(), v.trim().to_string()))
        .collect();
    let size = headers
        .get("content-length")
        .map(|n| n.parse::<usize>().unwrap())
        .unwrap_or(0);
    assert!(size < 2 * 1024 * 1024);
    while bytes.len() < end + size {
        let mut b = [0; 4096];
        let n = stream.read(&mut b).await.unwrap();
        assert!(n > 0);
        bytes.extend_from_slice(&b[..n]);
    }
    let body = &bytes[end..end + size];
    let response = {
        let mut s = state.lock().unwrap();
        match path {
            "/.well-known/openid-configuration" => Reply::json(
                200,
                json!({"issuer":ISSUER,"authorization_endpoint":format!("{base}/api/accounts/authorize"),"token_endpoint":format!("{base}/api/accounts/oauth/token"),"jwks_uri":format!("{base}/.well-known/jwks.json"),"revocation_endpoint":format!("{base}/api/accounts/oauth/revoke")}),
            ),
            "/.well-known/jwks.json" => Reply::json(
                200,
                serde_json::from_str(include_str!("../fixtures/chatgpt-jwks.json")).unwrap(),
            ),
            "/api/accounts/oauth/token" => {
                assert_eq!(headers["content-type"], "application/x-www-form-urlencoded");
                assert!(!headers.contains_key("authorization"));
                let u = reqwest::Url::parse(&format!(
                    "http://localhost/?{}",
                    std::str::from_utf8(body).unwrap()
                ))
                .unwrap();
                let p: BTreeMap<String, String> = u
                    .query_pairs()
                    .map(|(k, v)| (k.into_owned(), v.into_owned()))
                    .collect();
                assert_ne!(p["client_id"], "dynamic_agent_client");
                assert_eq!(p["resource"], "https://api.openai.com/v1");
                let refresh = p["grant_type"] == "refresh_token";
                if refresh {
                    s.refreshes += 1;
                    assert!(!p.contains_key("scope"));
                } else {
                    assert_eq!(p["code"], "fixture-code");
                    assert_eq!(p["code_verifier"].len(), 64);
                    use base64::Engine;
                    use sha2::Digest;
                    assert_eq!(
                        base64::engine::general_purpose::URL_SAFE_NO_PAD
                            .encode(sha2::Sha256::digest(p["code_verifier"].as_bytes())),
                        s.challenge
                    );
                    assert!(p["redirect_uri"].starts_with("http://127.0.0.1:"));
                }
                if refresh && s.refresh_reply.is_some() {
                    s.refresh_reply.take().unwrap()
                } else {
                    s.token_generation += 1;
                    let now = jsonwebtoken::get_current_timestamp();
                    let mut claims = json!({"iss":ISSUER,"aud":p["client_id"],"sub":s.subject,"iat":now,"exp":now+3600,"nonce":s.nonce});
                    if let Some(overrides) = s.token_override.take() {
                        for (k, v) in overrides.as_object().unwrap() {
                            claims[k] = v.clone();
                        }
                    }
                    let mut h = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::RS256);
                    h.kid = Some("fixture".into());
                    let mut jwt = jsonwebtoken::encode(
                        &h,
                        &claims,
                        &jsonwebtoken::EncodingKey::from_rsa_pem(include_bytes!(
                            "../fixtures/chatgpt-test-key.pem"
                        ))
                        .unwrap(),
                    )
                    .unwrap();
                    if std::mem::take(&mut s.bad_signature) {
                        let start = jwt.rfind('.').unwrap() + 1;
                        let new = if &jwt[start..start + 1] == "A" {
                            "B"
                        } else {
                            "A"
                        };
                        jwt.replace_range(start..start + 1, new);
                    }
                    Reply::json(
                        200,
                        json!({"access_token":format!("fixture-access-{}",s.token_generation),"refresh_token":format!("fixture-refresh-{}",s.token_generation),"id_token":jwt,"token_type":"Bearer","expires_in":3600,"scope":if s.direct { "openid offline_access chatgpt.tokens.use.direct" } else { "openid offline_access" },"earliest_refresh_at":{"unknown":"preserved"}}),
                    )
                }
            }
            "/api/accounts/oauth/revoke" => {
                s.revoke = true;
                Reply {
                    status: 200,
                    body: String::new(),
                    kind: "text/plain",
                }
            }
            "/v1/models" => {
                assert!(headers["authorization"].starts_with("Bearer fixture-access-"));
                Reply::json(
                    200,
                    json!({"models":[{"slug":"hidden","display_name":"Hidden","visibility":"hide"},{"slug":"fixture-model","display_name":"Fixture model","visibility":"list"}]}),
                )
            }
            "/v1/responses" => {
                assert!(headers["authorization"].starts_with("Bearer fixture-access-"));
                assert!(!headers.contains_key("chatgpt-account-id"));
                let v = serde_json::from_slice(body).unwrap();
                s.requests.push((headers["authorization"].clone(), v));
                s.replies.pop_front().expect("unexpected inference retry")
            }
            _ => panic!("unexpected path {path}"),
        }
    };
    let pause = if path == "/api/accounts/oauth/token" {
        state.lock().unwrap().pause.take()
    } else {
        None
    };
    if let Some((entered, release)) = pause {
        let _ = entered.send(());
        let _ = release.await;
    }
    let head = format!(
        "HTTP/1.1 {} Fixture\r\nContent-Type: {}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        response.status, response.kind
    );
    if stream.write_all(head.as_bytes()).await.is_err() {
        return;
    }
    for chunk in response.body.as_bytes().chunks(13) {
        if stream
            .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
            .await
            .is_err()
            || stream.write_all(chunk).await.is_err()
            || stream.write_all(b"\r\n").await.is_err()
        {
            return;
        }
    }
    let _ = stream.write_all(b"0\r\n\r\n").await;
}
