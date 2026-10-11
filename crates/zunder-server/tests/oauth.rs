//! The built-in OAuth 2.1 authorization server for `/mcp` (MCP
//! authorization, claude.ai connectors): discovery, dynamic client
//! registration, consent with a browser session, authorization code with
//! PKCE, token rotation with reuse detection, revocation and the negative
//! cases.

mod common;

use std::collections::HashMap;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use serde_json::{json, Value};
use url::Url;
use zunder_server::auth::pkce_challenge;

use common::{api_token, mcp_post, Auth, TestResponse, TestServer};

const CLAUDE_CALLBACK: &str = "https://claude.ai/api/mcp/auth_callback";
const PUBLIC: &str = "http://localhost:8080";
const RESOURCE: &str = "http://localhost:8080/mcp";
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk-zunder-test";

fn rpc(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn encode(pairs: &[(&str, &str)]) -> String {
    url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs)
        .finish()
}

async fn form_post(
    s: &TestServer,
    path: &str,
    cookie: Option<&str>,
    origin: Option<&str>,
    pairs: &[(&str, &str)],
) -> TestResponse {
    let mut b = Request::builder()
        .method(Method::POST)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(c) = cookie {
        b = b.header(header::COOKIE, format!("zunder_session={c}"));
    }
    if let Some(o) = origin {
        b = b.header(header::ORIGIN, o);
    }
    s.send(b.body(Body::from(encode(pairs))).unwrap()).await
}

async fn get_with_cookie(s: &TestServer, path: &str, cookie: Option<&str>) -> TestResponse {
    let auth = match cookie {
        Some(c) => Auth::Cookie(c),
        None => Auth::None,
    };
    s.get(path, auth).await
}

async fn register(s: &TestServer, body: Value) -> TestResponse {
    s.post("/oauth/register", Auth::None, body).await
}

/// Registers a client like claude.ai does and returns its `client_id`.
async fn register_claude(s: &TestServer) -> String {
    let r = register(
        s,
        json!({
            "client_name": "Claude",
            "redirect_uris": [CLAUDE_CALLBACK],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        }),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    let body = r.json();
    assert_eq!(body["client_name"], "Claude");
    assert_eq!(body["token_endpoint_auth_method"], "none");
    assert_eq!(body["redirect_uris"], json!([CLAUDE_CALLBACK]));
    assert!(body.get("client_secret").is_none());
    body["client_id"].as_str().unwrap().to_owned()
}

fn authorize_path(client_id: &str, extra: &[(&str, &str)]) -> String {
    let challenge = pkce_challenge(VERIFIER);
    let mut pairs = vec![
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", CLAUDE_CALLBACK),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("state", "zustand-123"),
        ("resource", RESOURCE),
    ];
    for (k, v) in extra {
        pairs.retain(|(key, _)| key != k);
        if !v.is_empty() {
            pairs.push((k, v));
        }
    }
    format!("/oauth/authorize?{}", encode(&pairs))
}

fn location(r: &TestResponse) -> Url {
    let raw = r.header("location").expect("redirect has a Location");
    Url::parse(raw)
        .or_else(|_| Url::parse(PUBLIC).unwrap().join(raw))
        .unwrap()
}

fn query(url: &Url) -> HashMap<String, String> {
    url.query_pairs().into_owned().collect()
}

/// The `request_id` of a consent page.
fn request_id(page: &str) -> String {
    let marker = "name=\"request_id\" value=\"";
    let start = page.find(marker).expect("consent page has a request_id") + marker.len();
    let end = start + page[start..].find('"').unwrap();
    page[start..end].to_owned()
}

/// Opens the consent page with a session and allows; returns the code.
async fn authorize_code(s: &TestServer, cookie: &str, client_id: &str) -> String {
    let r = get_with_cookie(s, &authorize_path(client_id, &[]), Some(cookie)).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let id = request_id(&r.text());
    let r = form_post(
        s,
        "/oauth/authorize",
        Some(cookie),
        Some(PUBLIC),
        &[("request_id", &id), ("decision", "allow")],
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.text());
    let to = location(&r);
    assert!(to.as_str().starts_with(CLAUDE_CALLBACK), "{to}");
    let q = query(&to);
    assert_eq!(q["state"], "zustand-123");
    assert_eq!(q["iss"], PUBLIC);
    assert!(q["code"].starts_with("zoc_"));
    q["code"].clone()
}

async fn token(s: &TestServer, pairs: &[(&str, &str)]) -> TestResponse {
    form_post(s, "/oauth/token", None, None, pairs).await
}

async fn exchange(s: &TestServer, client_id: &str, code: &str) -> TestResponse {
    token(
        s,
        &[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", CLAUDE_CALLBACK),
            ("client_id", client_id),
            ("code_verifier", VERIFIER),
            ("resource", RESOURCE),
        ],
    )
    .await
}

async fn refresh(s: &TestServer, client_id: &str, refresh_token: &str) -> TestResponse {
    token(
        s,
        &[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
            ("resource", RESOURCE),
        ],
    )
    .await
}

fn oauth_error(r: &TestResponse) -> String {
    r.json()["error"].as_str().unwrap().to_owned()
}

/// `(access_token, refresh_token)` of a successful token response.
fn tokens(r: &TestResponse) -> (String, String) {
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert_eq!(r.header("cache-control"), Some("no-store"));
    let body = r.json();
    assert_eq!(body["token_type"], "Bearer");
    assert_eq!(body["expires_in"], 3600);
    let access = body["access_token"].as_str().unwrap().to_owned();
    let refresh = body["refresh_token"].as_str().unwrap().to_owned();
    assert!(access.starts_with("zoa_"));
    assert!(refresh.starts_with("zor_"));
    (access, refresh)
}

async fn mcp_status(s: &TestServer, token: &str) -> StatusCode {
    mcp_post(s, token, rpc(1, "ping", json!({}))).await.status
}

// ----- discovery ------------------------------------------------------------------------

#[tokio::test]
async fn discovery_metadata_and_challenge() {
    let s = TestServer::new().await;
    for path in [
        "/.well-known/oauth-protected-resource",
        "/.well-known/oauth-protected-resource/mcp",
    ] {
        let r = s.get(path, Auth::None).await;
        assert_eq!(r.status, StatusCode::OK, "{path}");
        assert_eq!(r.header("access-control-allow-origin"), Some("*"));
        let body = r.json();
        assert_eq!(body["resource"], RESOURCE);
        assert_eq!(body["authorization_servers"], json!([PUBLIC]));
        assert_eq!(body["bearer_methods_supported"], json!(["header"]));
    }

    let r = s
        .get("/.well-known/oauth-authorization-server", Auth::None)
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let m = r.json();
    assert_eq!(m["issuer"], PUBLIC);
    assert_eq!(
        m["authorization_endpoint"],
        format!("{PUBLIC}/oauth/authorize")
    );
    assert_eq!(m["token_endpoint"], format!("{PUBLIC}/oauth/token"));
    assert_eq!(
        m["registration_endpoint"],
        format!("{PUBLIC}/oauth/register")
    );
    assert_eq!(m["revocation_endpoint"], format!("{PUBLIC}/oauth/revoke"));
    assert_eq!(m["code_challenge_methods_supported"], json!(["S256"]));
    assert_eq!(
        m["grant_types_supported"],
        json!(["authorization_code", "refresh_token"])
    );
    assert_eq!(m["response_types_supported"], json!(["code"]));
    assert_eq!(m["token_endpoint_auth_methods_supported"], json!(["none"]));

    // A 401 from /mcp points at the protected resource metadata.
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(rpc(1, "ping", json!({})).to_string()))
        .unwrap();
    let r = s.send(req).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        r.header("www-authenticate"),
        Some("Bearer resource_metadata=\"http://localhost:8080/.well-known/oauth-protected-resource\"")
    );
    let r = mcp_post(&s, "zoa_unbekannt", rpc(1, "ping", json!({}))).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert!(r
        .header("www-authenticate")
        .unwrap()
        .ends_with(", error=\"invalid_token\""));
}

#[tokio::test]
async fn metadata_follows_the_public_url() {
    let s = TestServer::with_config(|c| {
        c.public_url = Url::parse("https://zunder.example.org").unwrap();
    })
    .await;
    let r = s
        .get("/.well-known/oauth-protected-resource", Auth::None)
        .await;
    assert_eq!(r.json()["resource"], "https://zunder.example.org/mcp");
    assert_eq!(
        r.json()["authorization_servers"],
        json!(["https://zunder.example.org"])
    );
    let r = s
        .get("/.well-known/oauth-authorization-server", Auth::None)
        .await;
    assert_eq!(r.json()["issuer"], "https://zunder.example.org");
}

// ----- registration ---------------------------------------------------------------------

#[tokio::test]
async fn registration_validates_metadata() {
    let s = TestServer::new().await;
    // Local tools may use loopback http; the client is public regardless of
    // the requested auth method.
    let r = register(
        &s,
        json!({
            "client_name": "Lokales Werkzeug",
            "redirect_uris": ["http://localhost:3118/callback", "http://127.0.0.1/callback"],
            "token_endpoint_auth_method": "client_secret_post",
        }),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.text());
    assert_eq!(r.json()["token_endpoint_auth_method"], "none");
    assert_eq!(
        r.json()["grant_types"],
        json!(["authorization_code", "refresh_token"])
    );

    // Without a name the client gets a placeholder.
    let r = register(&s, json!({"redirect_uris": [CLAUDE_CALLBACK]})).await;
    assert_eq!(r.status, StatusCode::CREATED);
    assert_eq!(r.json()["client_name"], "Unbenannter Client");

    for (body, error) in [
        (json!({}), "invalid_redirect_uri"),
        (json!({"redirect_uris": []}), "invalid_redirect_uri"),
        (
            json!({"redirect_uris": ["http://evil.example/cb"]}),
            "invalid_redirect_uri",
        ),
        (
            json!({"redirect_uris": ["https://ok.example/cb#frag"]}),
            "invalid_redirect_uri",
        ),
        (
            json!({"redirect_uris": ["myapp://callback"]}),
            "invalid_redirect_uri",
        ),
        (
            json!({"redirect_uris": "https://ok.example/cb"}),
            "invalid_client_metadata",
        ),
        (
            json!({"redirect_uris": [CLAUDE_CALLBACK], "grant_types": ["client_credentials"]}),
            "invalid_client_metadata",
        ),
        (
            json!({"redirect_uris": [CLAUDE_CALLBACK], "response_types": ["token"]}),
            "invalid_client_metadata",
        ),
        (
            json!({"redirect_uris": [CLAUDE_CALLBACK], "client_name": "x".repeat(101)}),
            "invalid_client_metadata",
        ),
        (
            json!({"redirect_uris": [CLAUDE_CALLBACK], "client_name": "a\u{7}b"}),
            "invalid_client_metadata",
        ),
        (json!([1, 2]), "invalid_client_metadata"),
    ] {
        let r = register(&s, body.clone()).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(oauth_error(&r), error, "{body}");
    }
    let uris: Vec<String> = (0..11).map(|i| format!("https://a.example/{i}")).collect();
    let r = register(&s, json!({"redirect_uris": uris})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // Not JSON at all.
    let req = Request::builder()
        .method(Method::POST)
        .uri("/oauth/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from("kein json"))
        .unwrap();
    let r = s.send(req).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(oauth_error(&r), "invalid_client_metadata");

    // Body limit.
    let req = Request::builder()
        .method(Method::POST)
        .uri("/oauth/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"redirect_uris": [CLAUDE_CALLBACK], "pad": "x".repeat(20_000)}).to_string(),
        ))
        .unwrap();
    assert_eq!(s.send(req).await.status, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn registration_is_rate_limited() {
    let s = TestServer::new().await;
    for i in 0..30 {
        let r = register(&s, json!({"redirect_uris": [CLAUDE_CALLBACK]})).await;
        assert_eq!(r.status, StatusCode::CREATED, "registration {i}");
    }
    let r = register(&s, json!({"redirect_uris": [CLAUDE_CALLBACK]})).await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(oauth_error(&r), "temporarily_unavailable");
}

// ----- full flow ------------------------------------------------------------------------

#[tokio::test]
async fn full_flow_register_consent_token_mcp_refresh_revoke() {
    let s = TestServer::new().await;
    let client_id = register_claude(&s).await;

    // Without a session: off to the login, and back here afterwards.
    let path = authorize_path(&client_id, &[]);
    let r = get_with_cookie(&s, &path, None).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let to = location(&r);
    assert_eq!(to.path(), "/auth/login");
    assert_eq!(query(&to)["return_to"], path);

    // With a session: consent page naming client and redirect host.
    let cookie = s.session().await;
    let r = get_with_cookie(&s, &path, Some(&cookie)).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.header("cache-control"), Some("no-store"));
    assert!(r
        .header("content-security-policy")
        .unwrap()
        .contains("frame-ancestors 'none'"));
    let page = r.text();
    assert!(page.contains("<strong>Claude</strong>"), "{page}");
    assert!(page.contains("<code>claude.ai</code>"), "{page}");
    assert!(page.contains("Erlauben") && page.contains("Ablehnen"));
    assert!(
        !page.contains("localhost)"),
        "no loopback warning for claude.ai"
    );
    let id = request_id(&page);

    let r = form_post(
        &s,
        "/oauth/authorize",
        Some(&cookie),
        Some(PUBLIC),
        &[("request_id", &id), ("decision", "allow")],
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let q = query(&location(&r));
    let code = q["code"].clone();
    assert_eq!(q["state"], "zustand-123");

    // The consent cannot be replayed.
    let r = form_post(
        &s,
        "/oauth/authorize",
        Some(&cookie),
        Some(PUBLIC),
        &[("request_id", &id), ("decision", "allow")],
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // Code → tokens (PKCE).
    let (access, refresh_token) = tokens(&exchange(&s, &client_id, &code).await);

    // The access token works for /mcp …
    let r = mcp_post(&s, &access, rpc(1, "tools/list", json!({}))).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert!(!r.json()["result"]["tools"].as_array().unwrap().is_empty());
    let created = common::mcp_ok(&s, &access, "create_task", json!({"title": "Per OAuth"})).await;
    let task_id = created["task"]["id"].as_str().unwrap().to_owned();
    let history =
        common::mcp_ok(&s, &access, "get_task_history", json!({"task_id": task_id})).await;
    assert_eq!(
        history["entries"][0]["source"], "mcp:oauth:Claude",
        "{history}"
    );

    // … but not for the sync API.
    let r = s.get("/api/snapshot", Auth::Bearer(&access)).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);

    // The token page lists the connected client.
    let session = s.session().await;
    let r = s.get("/api/oauth/clients", Auth::Cookie(&session)).await;
    assert_eq!(r.status, StatusCode::OK);
    let clients = r.json();
    assert_eq!(clients.as_array().unwrap().len(), 1);
    assert_eq!(clients[0]["id"], client_id.as_str());
    assert_eq!(clients[0]["name"], "Claude");
    assert_eq!(clients[0]["redirect_uris"], json!([CLAUDE_CALLBACK]));
    assert!(clients[0]["last_used_at"].is_string());
    // Only with a browser session.
    let r = s
        .get("/api/oauth/clients", Auth::Bearer(common::BOOTSTRAP))
        .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    // Refresh rotates: a new pair, the old refresh token is spent.
    let (access2, refresh2) = tokens(&refresh(&s, &client_id, &refresh_token).await);
    assert_ne!(access2, access);
    assert_ne!(refresh2, refresh_token);
    assert_eq!(mcp_status(&s, &access2).await, StatusCode::OK);
    let (access3, refresh3) = tokens(&refresh(&s, &client_id, &refresh2).await);
    assert_eq!(mcp_status(&s, &access3).await, StatusCode::OK);

    // Reusing a rotated refresh token revokes the whole family.
    let r = refresh(&s, &client_id, &refresh_token).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(oauth_error(&r), "invalid_grant");
    for t in [&access, &access2, &access3] {
        assert_eq!(mcp_status(&s, t).await, StatusCode::UNAUTHORIZED);
    }
    let r = refresh(&s, &client_id, &refresh3).await;
    assert_eq!(oauth_error(&r), "invalid_grant");

    // A fresh authorization of the same client works again.
    let code = authorize_code(&s, &cookie, &client_id).await;
    let (access4, refresh4) = tokens(&exchange(&s, &client_id, &code).await);
    assert_eq!(mcp_status(&s, &access4).await, StatusCode::OK);

    // Revoking on the token page cuts the client off.
    let r = s
        .request(
            Method::DELETE,
            &format!("/api/oauth/clients/{client_id}"),
            Auth::Web(&session),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(mcp_status(&s, &access4).await, StatusCode::UNAUTHORIZED);
    let r = refresh(&s, &client_id, &refresh4).await;
    assert_eq!(oauth_error(&r), "invalid_grant");
    let r = s.get("/api/oauth/clients", Auth::Cookie(&session)).await;
    assert_eq!(r.json(), json!([]));
    let r = s
        .request(
            Method::DELETE,
            &format!("/api/oauth/clients/{client_id}"),
            Auth::Web(&session),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    // The revoked client cannot start a new authorization.
    let r = get_with_cookie(&s, &authorize_path(&client_id, &[]), Some(&cookie)).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(r.header("location").is_none());

    // API tokens keep working unchanged.
    let api = api_token(&s, "claude-code").await;
    assert_eq!(mcp_status(&s, &api).await, StatusCode::OK);
}

#[tokio::test]
async fn revocation_endpoint() {
    let s = TestServer::new().await;
    let client_id = register_claude(&s).await;
    let cookie = s.session().await;
    let code = authorize_code(&s, &cookie, &client_id).await;
    let (access, refresh_token) = tokens(&exchange(&s, &client_id, &code).await);

    // Revoking an access token affects only it.
    let r = form_post(
        &s,
        "/oauth/revoke",
        None,
        None,
        &[("token", &access), ("client_id", &client_id)],
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(mcp_status(&s, &access).await, StatusCode::UNAUTHORIZED);
    let (access2, refresh2) = tokens(&refresh(&s, &client_id, &refresh_token).await);
    assert_eq!(mcp_status(&s, &access2).await, StatusCode::OK);

    // Revoking a refresh token ends the grant.
    let r = form_post(&s, "/oauth/revoke", None, None, &[("token", &refresh2)]).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(mcp_status(&s, &access2).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        oauth_error(&refresh(&s, &client_id, &refresh2).await),
        "invalid_grant"
    );

    // Unknown tokens: still 200.
    let r = form_post(
        &s,
        "/oauth/revoke",
        None,
        None,
        &[("token", "zor_unbekannt")],
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
}

// ----- negative cases -------------------------------------------------------------------

#[tokio::test]
async fn token_endpoint_rejects_wrong_verifier_redirect_and_reuse() {
    let s = TestServer::new().await;
    let client_id = register_claude(&s).await;
    let cookie = s.session().await;

    // Wrong PKCE verifier: invalid_grant, and the code is spent.
    let code = authorize_code(&s, &cookie, &client_id).await;
    let wrong_verifier = "x".repeat(50);
    let r = token(
        &s,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", CLAUDE_CALLBACK),
            ("client_id", &client_id),
            ("code_verifier", &wrong_verifier),
        ],
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(oauth_error(&r), "invalid_grant");
    assert_eq!(
        oauth_error(&exchange(&s, &client_id, &code).await),
        "invalid_grant"
    );

    // Missing verifier.
    let code = authorize_code(&s, &cookie, &client_id).await;
    let r = token(
        &s,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", CLAUDE_CALLBACK),
            ("client_id", &client_id),
        ],
    )
    .await;
    assert_eq!(oauth_error(&r), "invalid_request");

    // Wrong redirect URI.
    let code = authorize_code(&s, &cookie, &client_id).await;
    let r = token(
        &s,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", "https://claude.ai/anders"),
            ("client_id", &client_id),
            ("code_verifier", VERIFIER),
        ],
    )
    .await;
    assert_eq!(oauth_error(&r), "invalid_grant");

    // Another client cannot redeem the code.
    let other = register_claude(&s).await;
    let code = authorize_code(&s, &cookie, &client_id).await;
    assert_eq!(
        oauth_error(&exchange(&s, &other, &code).await),
        "invalid_grant"
    );

    // Foreign resource at the token endpoint.
    let code = authorize_code(&s, &cookie, &client_id).await;
    let r = token(
        &s,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", CLAUDE_CALLBACK),
            ("client_id", &client_id),
            ("code_verifier", VERIFIER),
            ("resource", "https://fremd.example/mcp"),
        ],
    )
    .await;
    assert_eq!(oauth_error(&r), "invalid_target");

    // Reused code: rejected, and the tokens of the first redemption die.
    let code = authorize_code(&s, &cookie, &client_id).await;
    let (access, refresh_token) = tokens(&exchange(&s, &client_id, &code).await);
    assert_eq!(mcp_status(&s, &access).await, StatusCode::OK);
    assert_eq!(
        oauth_error(&exchange(&s, &client_id, &code).await),
        "invalid_grant"
    );
    assert_eq!(mcp_status(&s, &access).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        oauth_error(&refresh(&s, &client_id, &refresh_token).await),
        "invalid_grant"
    );

    // Unknown code, unknown client, unknown grant type, wrong content type.
    assert_eq!(
        oauth_error(&exchange(&s, &client_id, "zoc_unbekannt").await),
        "invalid_grant"
    );
    let r = exchange(&s, "unbekannt", "zoc_x").await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(oauth_error(&r), "invalid_client");
    let r = token(&s, &[("grant_type", "client_credentials")]).await;
    assert_eq!(oauth_error(&r), "unsupported_grant_type");
    let r = token(&s, &[]).await;
    assert_eq!(oauth_error(&r), "invalid_request");
    let r = s
        .post(
            "/oauth/token",
            Auth::None,
            json!({"grant_type": "refresh_token"}),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(oauth_error(&r), "invalid_request");
    // Refresh tokens are bound to their client.
    let code = authorize_code(&s, &cookie, &client_id).await;
    let (_, refresh_token) = tokens(&exchange(&s, &client_id, &code).await);
    assert_eq!(
        oauth_error(&refresh(&s, &other, &refresh_token).await),
        "invalid_grant"
    );
    // An access token is no refresh token.
    let code = authorize_code(&s, &cookie, &client_id).await;
    let (access, _) = tokens(&exchange(&s, &client_id, &code).await);
    assert_eq!(
        oauth_error(&refresh(&s, &client_id, &access).await),
        "invalid_grant"
    );
}

#[tokio::test]
async fn expired_code_and_tokens_are_rejected() {
    let s = TestServer::new().await;
    let client_id = register_claude(&s).await;
    let cookie = s.session().await;
    let db = rusqlite::Connection::open(s.app.config().db_path()).unwrap();

    let code = authorize_code(&s, &cookie, &client_id).await;
    db.execute("UPDATE oauth_codes SET expires_at = 1", [])
        .unwrap();
    assert_eq!(
        oauth_error(&exchange(&s, &client_id, &code).await),
        "invalid_grant"
    );

    let code = authorize_code(&s, &cookie, &client_id).await;
    let (access, refresh_token) = tokens(&exchange(&s, &client_id, &code).await);
    db.execute("UPDATE oauth_tokens SET expires_at = 1", [])
        .unwrap();
    assert_eq!(mcp_status(&s, &access).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        oauth_error(&refresh(&s, &client_id, &refresh_token).await),
        "invalid_grant"
    );
    // Without a live grant the client is no longer listed.
    let r = s.get("/api/oauth/clients", Auth::Cookie(&cookie)).await;
    assert_eq!(r.json(), json!([]));
}

#[tokio::test]
async fn authorize_rejects_bad_requests() {
    let s = TestServer::new().await;
    let client_id = register_claude(&s).await;
    let cookie = s.session().await;

    // Unknown client or unregistered redirect URI: page, never a redirect.
    for path in [
        authorize_path("unbekannt", &[]),
        authorize_path(&client_id, &[("redirect_uri", "https://evil.example/cb")]),
        authorize_path(
            &client_id,
            &[("redirect_uri", "https://claude.ai/api/mcp/auth_callback/x")],
        ),
    ] {
        let r = get_with_cookie(&s, &path, Some(&cookie)).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{path}");
        assert!(r.header("location").is_none(), "{path}");
        assert!(r.text().contains("Verbindung nicht möglich"));
    }

    // Everything else goes back to the client with an error.
    for (extra, error) in [
        (
            vec![("response_type", "token")],
            "unsupported_response_type",
        ),
        (vec![("code_challenge", "")], "invalid_request"),
        (vec![("code_challenge_method", "plain")], "invalid_request"),
        (vec![("code_challenge_method", "")], "invalid_request"),
        (vec![("code_challenge", "zu-kurz")], "invalid_request"),
        (
            vec![("resource", "https://fremd.example/mcp")],
            "invalid_target",
        ),
        (
            vec![("resource", "http://localhost:8080/api")],
            "invalid_target",
        ),
    ] {
        let r = get_with_cookie(&s, &authorize_path(&client_id, &extra), Some(&cookie)).await;
        assert_eq!(r.status, StatusCode::SEE_OTHER, "{extra:?}");
        let to = location(&r);
        assert!(to.as_str().starts_with(CLAUDE_CALLBACK));
        let q = query(&to);
        assert_eq!(q["error"], error, "{extra:?}");
        assert_eq!(q["state"], "zustand-123");
        assert!(!q.contains_key("code"));
    }

    // Resource variants that denote this server are fine; so is none.
    for resource in ["http://LOCALHOST:8080/mcp/", ""] {
        let r = get_with_cookie(
            &s,
            &authorize_path(&client_id, &[("resource", resource)]),
            Some(&cookie),
        )
        .await;
        assert_eq!(r.status, StatusCode::OK, "{resource}");
    }
}

#[tokio::test]
async fn consent_requires_the_same_session_and_origin() {
    let s = TestServer::new().await;
    let client_id = register_claude(&s).await;
    let cookie = s.session().await;
    let page = || async {
        let r = get_with_cookie(&s, &authorize_path(&client_id, &[]), Some(&cookie)).await;
        request_id(&r.text())
    };

    // Another session cannot answer the consent.
    let id = page().await;
    let other = s.session().await;
    let r = form_post(
        &s,
        "/oauth/authorize",
        Some(&other),
        Some(PUBLIC),
        &[("request_id", &id), ("decision", "allow")],
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert!(r.header("location").is_none());

    // No session at all.
    let id = page().await;
    let r = form_post(
        &s,
        "/oauth/authorize",
        None,
        None,
        &[("request_id", &id), ("decision", "allow")],
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    // Foreign origin.
    let id = page().await;
    let r = form_post(
        &s,
        "/oauth/authorize",
        Some(&cookie),
        Some("https://evil.example"),
        &[("request_id", &id), ("decision", "allow")],
    )
    .await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);

    // Unknown request ID.
    let r = form_post(
        &s,
        "/oauth/authorize",
        Some(&cookie),
        Some(PUBLIC),
        &[("request_id", "geraten"), ("decision", "allow")],
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // Deny → access_denied back to the client.
    let id = page().await;
    let r = form_post(
        &s,
        "/oauth/authorize",
        Some(&cookie),
        None,
        &[("request_id", &id), ("decision", "deny")],
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let q = query(&location(&r));
    assert_eq!(q["error"], "access_denied");
    assert_eq!(q["state"], "zustand-123");
    assert!(!q.contains_key("code"));
}

#[tokio::test]
async fn loopback_clients_match_any_port_and_get_a_warning() {
    let s = TestServer::new().await;
    let r = register(
        &s,
        json!({"client_name": "Werkzeug", "redirect_uris": ["http://localhost/callback"]}),
    )
    .await;
    let client_id = r.json()["client_id"].as_str().unwrap().to_owned();
    let cookie = s.session().await;
    let path = authorize_path(
        &client_id,
        &[("redirect_uri", "http://localhost:3118/callback")],
    );
    let r = get_with_cookie(&s, &path, Some(&cookie)).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    let page = r.text();
    assert!(page.contains("(localhost)"), "loopback warning: {page}");
    let id = request_id(&page);
    let r = form_post(
        &s,
        "/oauth/authorize",
        Some(&cookie),
        Some(PUBLIC),
        &[("request_id", &id), ("decision", "allow")],
    )
    .await;
    let to = location(&r);
    assert_eq!(to.port(), Some(3118));
    let code = query(&to)["code"].clone();
    // The token request must name the redirect URI actually used.
    let r = token(
        &s,
        &[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("redirect_uri", "http://localhost:3118/callback"),
            ("client_id", &client_id),
            ("code_verifier", VERIFIER),
        ],
    )
    .await;
    tokens(&r);
}

#[tokio::test]
async fn client_names_are_escaped_on_the_consent_page() {
    let s = TestServer::new().await;
    let r = register(
        &s,
        json!({"client_name": "<script>alert(1)</script>", "redirect_uris": [CLAUDE_CALLBACK]}),
    )
    .await;
    let client_id = r.json()["client_id"].as_str().unwrap().to_owned();
    let cookie = s.session().await;
    let r = get_with_cookie(&s, &authorize_path(&client_id, &[]), Some(&cookie)).await;
    let page = r.text();
    assert!(!page.contains("<script>"));
    assert!(page.contains("&lt;script&gt;"));
}

#[tokio::test]
async fn tokens_are_bound_to_this_server_as_audience() {
    let s = TestServer::new().await;
    let client_id = register_claude(&s).await;
    let cookie = s.session().await;
    let code = authorize_code(&s, &cookie, &client_id).await;
    let (access, _) = tokens(&exchange(&s, &client_id, &code).await);
    assert_eq!(mcp_status(&s, &access).await, StatusCode::OK);

    // The same database behind another public URL is another resource.
    let mut config = s.app.config().clone();
    config.public_url = Url::parse("https://anders.example.org").unwrap();
    let TestServer { app, router, dir } = s;
    drop(router);
    drop(app);
    let (app, router) = zunder_server::build(config).await.unwrap();
    let other = TestServer { app, router, dir };
    let req = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header(header::AUTHORIZATION, format!("Bearer {access}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(rpc(1, "ping", json!({})).to_string()))
        .unwrap();
    let r = other.send(req).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn dev_auth_consents_without_cookie() {
    let s = TestServer::with_config(|c| c.dev_auth = true).await;
    let client_id = register_claude(&s).await;
    let r = get_with_cookie(&s, &authorize_path(&client_id, &[]), None).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.text());
    assert!(r.text().contains("dev@localhost"));
    let id = request_id(&r.text());
    let r = form_post(
        &s,
        "/oauth/authorize",
        None,
        Some(PUBLIC),
        &[("request_id", &id), ("decision", "allow")],
    )
    .await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let code = query(&location(&r))["code"].clone();
    let (access, _) = tokens(&exchange(&s, &client_id, &code).await);
    assert_eq!(mcp_status(&s, &access).await, StatusCode::OK);
}

#[tokio::test]
async fn login_round_trip_with_dev_auth_returns_to_authorize() {
    // /auth/login with dev auth creates a session and redirects back to the
    // local return path, here the authorization request.
    let s = TestServer::with_config(|c| c.dev_auth = true).await;
    let client_id = register_claude(&s).await;
    let path = authorize_path(&client_id, &[]);
    let login = format!("/auth/login?{}", encode(&[("return_to", path.as_str())]));
    let r = s.get(&login, Auth::None).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(r.header("location"), Some(path.as_str()));
    assert!(r.cookie("zunder_session").is_some());
}
