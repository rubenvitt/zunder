//! OIDC login against a local mock provider (discovery, JWKS, token
//! endpoint) with RSA keys generated when the tests run.

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use axum::extract::State;
use axum::http::{header, Request, StatusCode};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::{json, Value};
use zunder_server::auth::oidc::{verify_id_token, OidcError};
use zunder_server::config::OidcConfig;

use common::{test_config, Auth, TestResponse, TestServer};

/// An RSA signing key of the mock provider with its public JWK.
struct TestKey {
    kid: &'static str,
    pem: String,
    jwk: Value,
}

impl TestKey {
    fn generate(kid: &'static str) -> Self {
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use base64::Engine;
        use rsa::pkcs8::{EncodePrivateKey, LineEnding};
        use rsa::traits::PublicKeyParts;

        let key = rsa::RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048).unwrap();
        let pem = key
            .to_pkcs8_pem(LineEnding::LF)
            .unwrap()
            .as_str()
            .to_owned();
        let b64 = |bytes: Vec<u8>| URL_SAFE_NO_PAD.encode(bytes);
        let jwk = json!({
            "kty": "RSA",
            "use": "sig",
            "alg": "RS256",
            "kid": kid,
            "n": b64(key.n().to_bytes_be()),
            "e": b64(key.e().to_bytes_be()),
        });
        Self { kid, pem, jwk }
    }
}

/// The provider's two keys, generated once per test binary (no private
/// key material lives in the repository).
fn keys() -> &'static [TestKey; 2] {
    static KEYS: OnceLock<[TestKey; 2]> = OnceLock::new();
    KEYS.get_or_init(|| {
        [
            TestKey::generate("oidc-test-key"),
            TestKey::generate("oidc-other-key"),
        ]
    })
}

/// The key the provider publishes first.
fn test_key() -> &'static TestKey {
    &keys()[0]
}

/// A second key: for forged signatures and key rotation.
fn other_key() -> &'static TestKey {
    &keys()[1]
}
const CLIENT_ID: &str = "zunder";
const CLIENT_SECRET: &str = "geheim-fuer-tests";
const SUBJECT: &str = "person-1";

/// What the mock provider puts into the next ID token.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Good,
    WrongNonce,
    WrongAudience,
    WrongIssuer,
    Expired,
    /// Signed with another key but claiming the trusted key ID.
    ForgedSignature,
    OtherSubject,
    EmailUnverified,
    /// Signed with the second key under its own key ID.
    RotatedKey,
}

struct Control {
    base: String,
    nonce: String,
    challenge: String,
    mode: Mode,
    jwks: Vec<Value>,
    token_requests: usize,
}

#[derive(Clone)]
struct Idp {
    control: Arc<Mutex<Control>>,
}

impl Idp {
    async fn start() -> (Idp, String) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let idp = Idp {
            control: Arc::new(Mutex::new(Control {
                base: base.clone(),
                nonce: String::new(),
                challenge: String::new(),
                mode: Mode::Good,
                jwks: vec![serde_json::from_value(test_key().jwk.clone()).unwrap()],
                token_requests: 0,
            })),
        };
        let router = Router::new()
            .route("/.well-known/openid-configuration", get(discovery))
            .route("/jwks", get(jwks))
            .route("/token", post(token))
            .with_state(idp.clone());
        tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        (idp, base)
    }

    fn set_mode(&self, mode: Mode) {
        self.control.lock().unwrap().mode = mode;
    }
}

async fn discovery(State(idp): State<Idp>) -> Json<Value> {
    let base = idp.control.lock().unwrap().base.clone();
    Json(json!({
        "issuer": base,
        "authorization_endpoint": format!("{base}/authorize"),
        "token_endpoint": format!("{base}/token"),
        "jwks_uri": format!("{base}/jwks"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"],
    }))
}

async fn jwks(State(idp): State<Idp>) -> Json<Value> {
    Json(json!({"keys": idp.control.lock().unwrap().jwks}))
}

fn now_secs() -> i64 {
    (zunder_core::hlc::system_time_ms() / 1000) as i64
}

fn sign(claims: &Value, pem: &str, kid: &str) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(kid.to_owned());
    jsonwebtoken::encode(
        &header,
        claims,
        &EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap(),
    )
    .unwrap()
}

async fn token(
    State(idp): State<Idp>,
    Form(form): Form<HashMap<String, String>>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut c = idp.control.lock().unwrap();
    c.token_requests += 1;
    let bad = |what: &str| {
        (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "invalid_grant", "error_description": what})),
        )
    };
    if form.get("grant_type").map(String::as_str) != Some("authorization_code") {
        return Err(bad("grant_type"));
    }
    if form.get("code").map(String::as_str) != Some("good-code") {
        return Err(bad("code"));
    }
    if form.get("client_id").map(String::as_str) != Some(CLIENT_ID)
        || form.get("client_secret").map(String::as_str) != Some(CLIENT_SECRET)
    {
        return Err(bad("client"));
    }
    if form.get("redirect_uri").map(String::as_str) != Some("http://localhost:8080/auth/callback") {
        return Err(bad("redirect_uri"));
    }
    let verifier = form.get("code_verifier").ok_or_else(|| bad("verifier"))?;
    if zunder_server::auth::pkce_challenge(verifier) != c.challenge {
        return Err(bad("pkce"));
    }

    let now = now_secs();
    let mut claims = json!({
        "iss": c.base,
        "sub": SUBJECT,
        "aud": CLIENT_ID,
        "exp": now + 300,
        "iat": now,
        "nonce": c.nonce,
        "email": "person@example.org",
        "email_verified": true,
        "name": "Person Beispiel",
    });
    let (mut pem, mut kid) = (test_key().pem.as_str(), test_key().kid);
    match c.mode {
        Mode::Good => {}
        Mode::WrongNonce => claims["nonce"] = json!("other-nonce"),
        Mode::WrongAudience => claims["aud"] = json!("someone-else"),
        Mode::WrongIssuer => claims["iss"] = json!("https://evil.example"),
        Mode::Expired => {
            claims["exp"] = json!(now - 3600);
            claims["iat"] = json!(now - 7200);
        }
        Mode::ForgedSignature => pem = &other_key().pem,
        Mode::OtherSubject => claims["sub"] = json!("person-2"),
        Mode::EmailUnverified => claims["email_verified"] = json!(false),
        Mode::RotatedKey => (pem, kid) = (&other_key().pem, other_key().kid),
    }
    Ok(Json(json!({
        "access_token": "access",
        "token_type": "Bearer",
        "expires_in": 300,
        "id_token": sign(&claims, pem, kid),
    })))
}

async fn server_for(base: &str, adjust: impl FnOnce(&mut OidcConfig)) -> TestServer {
    let dir = tempfile::tempdir().unwrap();
    let mut config = test_config(&dir);
    config.session_secret = "0123456789abcdef0123456789abcdef-test".into();
    let mut oidc = OidcConfig {
        issuer: url::Url::parse(base).unwrap(),
        client_id: CLIENT_ID.into(),
        client_secret: Some(CLIENT_SECRET.into()),
        allowed_subject: Some(SUBJECT.into()),
        allowed_email: None,
    };
    adjust(&mut oidc);
    config.oidc = Some(oidc);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let (app, router) = zunder_server::build_with_http_client(config, http)
        .await
        .unwrap();
    TestServer { app, router, dir }
}

/// Starts a login at the server, lets the mock provider "authorize" it and
/// returns the callback response.
async fn login(s: &TestServer, idp: &Idp) -> TestResponse {
    login_from(s, idp, "/auth/login?return_to=/heute").await
}

/// Like [`login`], starting at `start_path` (an `/auth/login` URL).
async fn login_from(s: &TestServer, idp: &Idp, start_path: &str) -> TestResponse {
    let start = s.get(start_path, Auth::None).await;
    assert_eq!(start.status, StatusCode::SEE_OTHER, "{}", start.text());
    let location = url::Url::parse(start.header("location").unwrap()).unwrap();
    assert_eq!(location.path(), "/authorize");
    let q: HashMap<String, String> = location.query_pairs().into_owned().collect();
    assert_eq!(q["response_type"], "code");
    assert_eq!(q["client_id"], CLIENT_ID);
    assert_eq!(q["redirect_uri"], "http://localhost:8080/auth/callback");
    assert_eq!(q["code_challenge_method"], "S256");
    assert!(q["scope"].split(' ').any(|s| s == "openid"));
    let state = q["state"].clone();
    let cookie = start.cookie("zunder_login").expect("login cookie");
    for secret in [&state, &q["nonce"]] {
        assert!(
            !cookie.contains(secret.as_str()),
            "the login cookie is sealed"
        );
    }
    {
        let mut c = idp.control.lock().unwrap();
        c.nonce = q["nonce"].clone();
        c.challenge = q["code_challenge"].clone();
    }
    callback(s, &state, Some(&cookie)).await
}

async fn callback(s: &TestServer, state: &str, cookie: Option<&str>) -> TestResponse {
    let mut req = Request::get(format!("/auth/callback?code=good-code&state={state}"));
    if let Some(cookie) = cookie {
        req = req.header(header::COOKIE, format!("zunder_login={cookie}"));
    }
    s.send(req.body(axum::body::Body::empty()).unwrap()).await
}

#[tokio::test]
async fn full_login_flow() {
    let (idp, base) = Idp::start().await;
    let s = server_for(&base, |_| {}).await;

    let r = login(&s, &idp).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    assert_eq!(r.header("location"), Some("/heute"));
    let session = r.cookie("zunder_session").expect("session cookie");
    assert_eq!(
        r.cookie("zunder_login").as_deref(),
        Some(""),
        "login cookie cleared"
    );

    let me = s.get("/api/me", Auth::Cookie(&session)).await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.json()["email"], "person@example.org");
    assert_eq!(me.json()["name"], "Person Beispiel");
    assert_eq!(me.json()["user_id"], s.app.owner_id().to_string());
    let owner = s.app.owner().await.unwrap();
    assert_eq!(owner.subject.as_deref(), Some(SUBJECT));

    // a second login maps to the same user
    let r = login(&s, &idp).await;
    let second = r.cookie("zunder_session").unwrap();
    assert_ne!(second, session);
    let me2 = s.get("/api/me", Auth::Cookie(&second)).await.json();
    assert_eq!(me2["user_id"], me.json()["user_id"]);
}

#[tokio::test]
async fn oauth_authorization_survives_the_login_round_trip() {
    // claude.ai sends the owner to /oauth/authorize; without a session the
    // server detours via the OIDC login and lands on the consent page.
    let (idp, base) = Idp::start().await;
    let s = server_for(&base, |_| {}).await;
    let r = s
        .post(
            "/oauth/register",
            Auth::None,
            json!({"client_name": "Claude", "redirect_uris": ["https://claude.ai/api/mcp/auth_callback"]}),
        )
        .await;
    let client_id = r.json()["client_id"].as_str().unwrap().to_owned();
    let authorize = format!(
        "/oauth/authorize?response_type=code&client_id={client_id}\
         &redirect_uri=https%3A%2F%2Fclaude.ai%2Fapi%2Fmcp%2Fauth_callback\
         &code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM&code_challenge_method=S256\
         &state=abc&resource=http%3A%2F%2Flocalhost%3A8080%2Fmcp"
    );
    let r = s.get(&authorize, Auth::None).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let to_login = r.header("location").unwrap().to_owned();
    assert!(to_login.starts_with("/auth/login?return_to="), "{to_login}");

    let r = login_from(&s, &idp, &to_login).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    assert_eq!(r.header("location"), Some(authorize.as_str()));
    let session = r.cookie("zunder_session").expect("session cookie");

    let r = s.get(&authorize, Auth::Cookie(&session)).await;
    assert_eq!(r.status, StatusCode::OK);
    let page = r.text();
    assert!(page.contains("Zugriff erlauben?"), "{page}");
    assert!(page.contains("person@example.org"), "{page}");
}

#[tokio::test]
async fn invalid_id_tokens_are_rejected() {
    let (idp, base) = Idp::start().await;
    let s = server_for(&base, |_| {}).await;
    for (mode, status) in [
        (Mode::WrongNonce, StatusCode::UNAUTHORIZED),
        (Mode::WrongAudience, StatusCode::UNAUTHORIZED),
        (Mode::WrongIssuer, StatusCode::UNAUTHORIZED),
        (Mode::Expired, StatusCode::UNAUTHORIZED),
        (Mode::ForgedSignature, StatusCode::UNAUTHORIZED),
        (Mode::RotatedKey, StatusCode::UNAUTHORIZED),
        (Mode::OtherSubject, StatusCode::FORBIDDEN),
    ] {
        idp.set_mode(mode);
        let r = login(&s, &idp).await;
        assert_eq!(r.status, status, "{mode:?}: {}", r.text());
        assert!(r.cookie("zunder_session").is_none(), "{mode:?}");
        assert!(r.text().contains("Anmeldung fehlgeschlagen"));
        let expected = if mode == Mode::OtherSubject {
            "darf sich bei Zunder nicht anmelden"
        } else {
            "ID-Token ist ungültig"
        };
        assert!(r.text().contains(expected), "{mode:?}: {}", r.text());
    }
}

#[tokio::test]
async fn rotated_keys_are_fetched_again() {
    let (idp, base) = Idp::start().await;
    let s = server_for(&base, |_| {}).await;
    assert_eq!(login(&s, &idp).await.status, StatusCode::SEE_OTHER);
    // The provider rotates to its second key; the cached JWKS lacks it.
    {
        let mut c = idp.control.lock().unwrap();
        c.jwks = vec![serde_json::from_value(other_key().jwk.clone()).unwrap()];
        c.mode = Mode::RotatedKey;
    }
    let r = login(&s, &idp).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    assert!(r.cookie("zunder_session").is_some());
}

#[tokio::test]
async fn allowed_email_must_be_verified() {
    let (idp, base) = Idp::start().await;
    let s = server_for(&base, |c| {
        c.allowed_subject = None;
        c.allowed_email = Some("person@example.org".into());
    })
    .await;
    let r = login(&s, &idp).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    idp.set_mode(Mode::EmailUnverified);
    let r = login(&s, &idp).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn anonymous_logins_do_not_evict_a_pending_login() {
    let (idp, base) = Idp::start().await;
    let s = server_for(&base, |_| {}).await;
    let start = s.get("/auth/login?return_to=/heute", Auth::None).await;
    let location = url::Url::parse(start.header("location").unwrap()).unwrap();
    let q: HashMap<String, String> = location.query_pairs().into_owned().collect();
    {
        let mut c = idp.control.lock().unwrap();
        c.nonce = q["nonce"].clone();
        c.challenge = q["code_challenge"].clone();
    }
    // A flood of anonymous logins while the owner is at the provider.
    for _ in 0..300 {
        let r = s.get("/auth/login", Auth::None).await;
        assert_eq!(r.status, StatusCode::SEE_OTHER);
    }
    let r = callback(&s, &q["state"], start.cookie("zunder_login").as_deref()).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    assert_eq!(r.header("location"), Some("/heute"));
    assert!(r.cookie("zunder_session").is_some());
}

#[tokio::test]
async fn state_is_bound_to_the_browser_and_single_use() {
    let (idp, base) = Idp::start().await;
    let s = server_for(&base, |_| {}).await;

    let start = s.get("/auth/login", Auth::None).await;
    let location = url::Url::parse(start.header("location").unwrap()).unwrap();
    let q: HashMap<String, String> = location.query_pairs().into_owned().collect();
    {
        let mut c = idp.control.lock().unwrap();
        c.nonce = q["nonce"].clone();
        c.challenge = q["code_challenge"].clone();
    }
    let state = &q["state"];
    let cookie = start.cookie("zunder_login").unwrap();

    // without the login cookie (another browser): refused before any token request
    let r = callback(&s, state, None).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // the cookie of another login in this browser does not fit the state
    let other = s.get("/auth/login", Auth::None).await;
    let r = callback(&s, state, other.cookie("zunder_login").as_deref()).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // a cookie the server did not seal
    let r = callback(&s, state, Some("erfunden")).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(idp.control.lock().unwrap().token_requests, 0);

    let r = callback(&s, state, Some(&cookie)).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(r.header("location"), Some("/"));

    // replay
    let r = callback(&s, state, Some(&cookie)).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(r.cookie("zunder_session").is_none());

    // unknown state
    let r = callback(&s, "erfunden", Some("erfunden")).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);

    // provider-side error
    let req = Request::get("/auth/callback?error=access_denied&error_description=abgebrochen")
        .body(axum::body::Body::empty())
        .unwrap();
    let r = s.send(req).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(r.text().contains("access_denied"));
}

#[tokio::test]
async fn unreachable_provider_is_reported() {
    let s = server_for("http://127.0.0.1:9", |_| {}).await;
    let r = s.get("/auth/login", Auth::None).await;
    assert_eq!(r.status, StatusCode::BAD_GATEWAY);
    assert!(r.text().contains("nicht erreichbar"));
}

#[test]
fn symmetric_tokens_are_refused() {
    // alg confusion: an HS256 token "signed" with the public key material
    let jwks: jsonwebtoken::jwk::JwkSet =
        serde_json::from_value(json!({"keys": [test_key().jwk.clone()]})).unwrap();
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some("oidc-test-key".into());
    let now = now_secs();
    let token = jsonwebtoken::encode(
        &header,
        &json!({"iss": "i", "sub": "s", "aud": CLIENT_ID, "exp": now + 60, "iat": now, "nonce": "n"}),
        &EncodingKey::from_secret(test_key().jwk.to_string().as_bytes()),
    )
    .unwrap();
    let err = verify_id_token(&token, &jwks, "i", CLIENT_ID, "n").unwrap_err();
    assert!(matches!(err, OidcError::InvalidToken(_)));

    // the same claims signed properly verify
    let good = sign(
        &json!({"iss": "i", "sub": "s", "aud": CLIENT_ID, "exp": now + 60, "iat": now, "nonce": "n"}),
        &test_key().pem,
        test_key().kid,
    );
    let claims = verify_id_token(&good, &jwks, "i", CLIENT_ID, "n").unwrap();
    assert_eq!(claims.sub, "s");
    // several audiences require azp
    let multi = sign(
        &json!({"iss": "i", "sub": "s", "aud": [CLIENT_ID, "x"], "exp": now + 60, "iat": now, "nonce": "n"}),
        &test_key().pem,
        test_key().kid,
    );
    assert!(verify_id_token(&multi, &jwks, "i", CLIENT_ID, "n").is_err());
}
