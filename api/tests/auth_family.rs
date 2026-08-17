//! Auth route family vs the golden fixtures in `contract/fixtures/auth/`
//! (status + shape; ids/tokens/timestamps normalized like the fixtures),
//! plus the rate-limiter behavior the fixtures only describe in notes
//! (30/min login, 10/min register, TESTING bypass, fail-open, proxy-aware
//! client IP bucketing).

use std::collections::{BTreeMap, HashMap};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use serde_json::{Value, json};
use tower::ServiceExt;

use nightlio_api::auth::jwt;
use nightlio_api::config::Config;
use nightlio_api::db::{self, SelfHostSeed};
use nightlio_api::routes;
use nightlio_api::state::AppState;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct TestApp {
    app: Router,
    jwt_secret: String,
    db_path: String,
    _dir: tempfile::TempDir,
}

/// Development-env app on a tempfile DB. `RATE_LIMIT_DB_PATH` is always
/// pointed into the tempdir so parallel tests never share a limiter store
/// (the default would be the repo-relative `data/rate_limit.db`).
fn make_app(extra: &[(&str, &str)]) -> TestApp {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("nightlio.db");
    let rate_limit_path = dir.path().join("rate_limit.db");
    let mut vars: HashMap<String, String> = HashMap::new();
    vars.insert("APP_ENV".to_string(), "development".to_string());
    vars.insert(
        "RATE_LIMIT_DB_PATH".to_string(),
        rate_limit_path.to_string_lossy().into_owned(),
    );
    for (key, value) in extra {
        vars.insert((*key).to_string(), (*value).to_string());
    }
    let lookup = move |key: &str| vars.get(key).cloned();
    let mut cfg = Config::from_lookup(&lookup);
    cfg.database_path = db_path.to_string_lossy().into_owned();
    db::bootstrap(&cfg.database_path, &SelfHostSeed::from(&cfg)).expect("bootstrap");
    let pool = db::open_pool(&cfg.database_path).expect("pool");
    let jwt_secret = cfg.jwt_secret.clone();
    let db_path = cfg.database_path.clone();
    TestApp {
        app: routes::build_router(AppState::new(cfg, pool)),
        jwt_secret,
        db_path,
        _dir: dir,
    }
}

impl TestApp {
    fn bearer(&self, user_id: i64) -> String {
        format!(
            "Bearer {}",
            jwt::issue_token(&self.jwt_secret, user_id).expect("token")
        )
    }

    async fn send(&self, request: Request<Body>) -> Response {
        self.app.clone().oneshot(request).await.expect("infallible")
    }

    /// POST with an arbitrary header set; `body: None` sends no body and no
    /// Content-Type at all (the e2e credential-free login shape).
    async fn post(&self, path: &str, headers: &[(&str, &str)], body: Option<Value>) -> Response {
        let mut builder = Request::builder().method("POST").uri(path);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = match body {
            Some(value) => builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(value.to_string()))
                .unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        };
        self.send(request).await
    }

    /// Register a local user through the API using the seeded user's token.
    async fn register(&self, body: Value) -> Response {
        let auth = self.bearer(1);
        self.post(
            "/api/auth/local/register",
            &[("Authorization", auth.as_str())],
            Some(body),
        )
        .await
    }
}

async fn body_json(response: Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("json body")
}

async fn body_string(response: Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("body");
    String::from_utf8(bytes.to_vec()).expect("utf8 body")
}

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/../contract/fixtures/auth/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|exc| panic!("missing fixture {path}: {exc}"));
    serde_json::from_str(&raw).expect("fixture json")
}

fn fixture_status(fx: &Value) -> u16 {
    fx["response"]["status"].as_u64().expect("fixture status") as u16
}

/// Normalize the volatile fields exactly like the recorded fixtures do:
/// `token` → `"<JWT>"`, and (opt-in, for autoincrement-burn cases the
/// fixtures say not to grade) `user.id` → `"<ID>"`.
fn normalized(mut body: Value, normalize_user_id: bool) -> Value {
    if let Some(token) = body.get_mut("token") {
        // Applied to live responses AND fixture bodies (already "<JWT>").
        assert!(
            token
                .as_str()
                .is_some_and(|t| t == "<JWT>" || t.split('.').count() == 3),
            "token must be a JWT, got {token}"
        );
        *token = json!("<JWT>");
    }
    if normalize_user_id {
        body["user"]["id"] = json!("<ID>");
    }
    body
}

fn assert_json_content_type(response: &Response) {
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    assert!(
        content_type.starts_with("application/json"),
        "expected JSON content type, got {content_type:?}"
    );
}

/// Split a Set-Cookie header into (name, value, attributes). Attribute
/// names are lowercased; valueless attributes (HttpOnly, Secure) map to
/// `None`. Ordering is deliberately NOT graded — RFC 6265 recipients
/// ignore it — only the attribute set and values are.
fn parse_set_cookie(raw: &str) -> (String, String, BTreeMap<String, Option<String>>) {
    let mut segments = raw.split(';').map(str::trim);
    let first = segments.next().expect("cookie pair");
    let (name, value) = first.split_once('=').expect("name=value");
    let mut attributes = BTreeMap::new();
    for segment in segments {
        match segment.split_once('=') {
            Some((key, val)) => attributes.insert(
                key.trim().to_ascii_lowercase(),
                Some(val.trim().to_string()),
            ),
            None => attributes.insert(segment.to_ascii_lowercase(), None),
        };
    }
    (name.to_string(), value.to_string(), attributes)
}

fn set_cookie_header(response: &Response) -> String {
    response
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .expect("Set-Cookie header")
        .to_string()
}

/// The session-cookie attribute contract shared by both login branches:
/// `Max-Age=3600; HttpOnly; Path=/; SameSite=Lax`, `Secure` absent over
/// plain HTTP, value = the body token.
fn assert_session_cookie(response: &Response, expected_token: &str) {
    let (name, value, attrs) = parse_set_cookie(&set_cookie_header(response));
    assert_eq!(name, "nightlio_token");
    assert_eq!(value, expected_token);
    assert_eq!(attrs.get("max-age"), Some(&Some("3600".to_string())));
    assert_eq!(attrs.get("path"), Some(&Some("/".to_string())));
    assert!(
        attrs.contains_key("httponly"),
        "HttpOnly missing: {attrs:?}"
    );
    assert_eq!(attrs.get("samesite"), Some(&Some("Lax".to_string())));
    assert!(
        !attrs.contains_key("secure"),
        "Secure must be absent over plain HTTP"
    );
    assert!(
        !attrs.contains_key("expires"),
        "Expires must be absent — Max-Age is authoritative: {attrs:?}"
    );
}

// ---------------------------------------------------------------------------
// POST /auth/verify
// ---------------------------------------------------------------------------

#[tokio::test]
async fn verify_bearer_matches_fixture() {
    let fx = fixture("verify_bearer-200.json");
    let test = make_app(&[]);
    let auth = test.bearer(1);
    let response = test
        .post(
            "/api/auth/verify",
            &[("Authorization", auth.as_str())],
            None,
        )
        .await;
    assert_eq!(response.status().as_u16(), fixture_status(&fx));
    assert_json_content_type(&response);
    assert!(
        response.headers().get(header::SET_COOKIE).is_none(),
        "verify never refreshes the cookie"
    );
    let body = body_json(response).await;
    assert_eq!(body, fx["response"]["body"]);
    insta::assert_json_snapshot!("verify_bearer_200", json!({"status": 200, "body": body}));
}

#[tokio::test]
async fn verify_cookie_with_csrf_headers_matches_fixture() {
    let fx = fixture("verify_cookie-with-csrf-headers-200.json");
    let test = make_app(&[]);
    let cookie = format!(
        "nightlio_token={}",
        jwt::issue_token(&test.jwt_secret, 1).unwrap()
    );
    let response = test
        .post(
            "/api/auth/verify",
            &[
                ("Cookie", cookie.as_str()),
                ("Content-Type", "application/json"),
                ("X-Requested-With", "nightlio"),
            ],
            None,
        )
        .await;
    assert_eq!(response.status().as_u16(), fixture_status(&fx));
    let body = body_json(response).await;
    assert_eq!(body, fx["response"]["body"]);
    insta::assert_json_snapshot!(
        "verify_cookie_with_csrf_headers_200",
        json!({"status": 200, "body": body})
    );
}

#[tokio::test]
async fn verify_cookie_without_csrf_headers_matches_fixture() {
    let fx = fixture("verify_cookie-without-csrf-headers-403.json");
    let test = make_app(&[]);
    let cookie = format!(
        "nightlio_token={}",
        jwt::issue_token(&test.jwt_secret, 1).unwrap()
    );
    // Cookie only, no Content-Type: the Content-Type check fires first.
    let response = test
        .post("/api/auth/verify", &[("Cookie", cookie.as_str())], None)
        .await;
    assert_eq!(response.status().as_u16(), fixture_status(&fx));
    assert_json_content_type(&response);
    let body = body_json(response).await;
    assert_eq!(body, fx["response"]["body"]);
    insta::assert_json_snapshot!(
        "verify_cookie_without_csrf_headers_403",
        json!({"status": 403, "body": body})
    );

    // With JSON Content-Type but no X-Requested-With, the second check
    // fires instead (fixture notes: checks run in that order).
    let response = test
        .post(
            "/api/auth/verify",
            &[
                ("Cookie", cookie.as_str()),
                ("Content-Type", "application/json"),
            ],
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        body_json(response).await,
        json!({ "error": "Missing required request header" })
    );
}

#[tokio::test]
async fn verify_invalid_token_matches_fixture() {
    let fx = fixture("verify_invalid-token-401.json");
    let test = make_app(&[]);
    let response = test
        .post(
            "/api/auth/verify",
            &[("Authorization", "Bearer not.a.jwt")],
            None,
        )
        .await;
    assert_eq!(response.status().as_u16(), fixture_status(&fx));
    let body = body_json(response).await;
    assert_eq!(body, fx["response"]["body"]);
    insta::assert_json_snapshot!(
        "verify_invalid_token_401",
        json!({"status": 401, "body": body})
    );
}

#[tokio::test]
async fn verify_no_auth_matches_fixture() {
    let fx = fixture("verify_no-auth-401.json");
    let test = make_app(&[]);
    let response = test.post("/api/auth/verify", &[], None).await;
    assert_eq!(response.status().as_u16(), fixture_status(&fx));
    let body = body_json(response).await;
    assert_eq!(body, fx["response"]["body"]);
    insta::assert_json_snapshot!("verify_no_auth_401", json!({"status": 401, "body": body}));
}

#[tokio::test]
async fn verify_unknown_user_is_404() {
    // Valid token for a user id with no row: the route's own 404 branch.
    let test = make_app(&[]);
    let auth = test.bearer(9999);
    let response = test
        .post(
            "/api/auth/verify",
            &[("Authorization", auth.as_str())],
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        body_json(response).await,
        json!({ "error": "User not found" })
    );
}

#[tokio::test]
async fn verify_options_matches_fixture() {
    let fx = fixture("verify_options-204.json");
    // The fixture was recorded with http://localhost:5000 in CORS_ORIGINS.
    let test = make_app(&[(
        "CORS_ORIGINS",
        "http://localhost:5173,http://localhost:5000",
    )]);
    let response = test
        .send(
            Request::builder()
                .method("OPTIONS")
                .uri("/api/auth/verify")
                .header(header::ORIGIN, "http://localhost:5000")
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(response.status().as_u16(), fixture_status(&fx));
    let expected_headers = &fx["response"]["headers_that_matter"];
    for name in [
        "Allow",
        "Access-Control-Allow-Origin",
        "Access-Control-Allow-Credentials",
        "Access-Control-Allow-Methods",
    ] {
        assert_eq!(
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok()),
            expected_headers[name].as_str(),
            "{name}"
        );
    }
    // contract change: the fixture no longer records Content-Length
    // (the wire 204 carries none — hyper strips it at serialization).
    assert_eq!(body_string(response).await, "");
}

#[tokio::test]
async fn verify_plain_options_is_automatic_204() {
    // Without preflight headers: automatic OPTIONS, same 204-empty (contract
    // change).
    let test = make_app(&[]);
    let response = test
        .send(
            Request::builder()
                .method("OPTIONS")
                .uri("/api/auth/verify")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        response
            .headers()
            .get(header::ALLOW)
            .and_then(|value| value.to_str().ok()),
        Some("POST, OPTIONS")
    );
    assert_eq!(body_string(response).await, "");
}

#[tokio::test]
async fn verify_trailing_slash_matches_fixture() {
    let fx = fixture("verify_trailing-slash-404.json");
    let test = make_app(&[]);
    let auth = test.bearer(1);
    let response = test
        .post(
            "/api/auth/verify/",
            &[("Authorization", auth.as_str())],
            None,
        )
        .await;
    assert_eq!(response.status().as_u16(), fixture_status(&fx));
    let body = body_json(response).await;
    assert_eq!(body, fx["response"]["body"]);
    insta::assert_json_snapshot!(
        "verify_trailing_slash_404",
        json!({"status": 404, "body": body})
    );
}

// ---------------------------------------------------------------------------
// POST /auth/logout
// ---------------------------------------------------------------------------

#[tokio::test]
async fn logout_matches_fixture_and_is_idempotent() {
    let fx = fixture("logout_200.json");
    let test = make_app(&[]);
    for round in 0..2 {
        let response = test.post("/api/auth/logout", &[], None).await;
        assert_eq!(
            response.status().as_u16(),
            fixture_status(&fx),
            "round {round}"
        );
        assert_json_content_type(&response);

        // Clearing cookie, attribute-exact vs the recorded header:
        // empty value, Expires at the epoch, Max-Age=0, HttpOnly, Path=/,
        // SameSite=Lax, Secure absent over plain HTTP.
        let (name, value, attrs) = parse_set_cookie(&set_cookie_header(&response));
        let (fx_name, fx_value, fx_attrs) = parse_set_cookie(
            fx["response"]["headers_that_matter"]["Set-Cookie"]
                .as_str()
                .unwrap(),
        );
        assert_eq!(name, fx_name);
        assert_eq!(value, fx_value);
        assert_eq!(
            attrs, fx_attrs,
            "clearing cookie attribute set must match verbatim"
        );

        let body = body_json(response).await;
        assert_eq!(body, fx["response"]["body"]);
        if round == 0 {
            insta::assert_json_snapshot!("logout_200", json!({"status": 200, "body": body}));
        }
    }
}

// ---------------------------------------------------------------------------
// POST /auth/local/login
// ---------------------------------------------------------------------------

#[tokio::test]
async fn login_selfhost_credential_free_matches_fixture() {
    let fx = fixture("local-login_selfhost-credential-free.json");
    let test = make_app(&[]);
    let response = test
        .post("/api/auth/local/login", &[], Some(json!({})))
        .await;
    assert_eq!(response.status().as_u16(), fixture_status(&fx));
    assert_json_content_type(&response);
    let cookie_header = set_cookie_header(&response);
    let body = body_json(response).await;
    // The cookie carries the same token as the body.
    let token = body["token"].as_str().expect("token").to_string();
    assert_eq!(
        jwt::verify_token(&test.jwt_secret, &token).unwrap().user_id,
        1
    );
    {
        let (name, value, _) = parse_set_cookie(&cookie_header);
        assert_eq!(
            (name.as_str(), value.as_str()),
            ("nightlio_token", token.as_str())
        );
    }

    let body = normalized(body, false);
    assert_eq!(body, normalized(fx["response"]["body"].clone(), false));
    insta::assert_json_snapshot!(
        "local_login_selfhost_credential_free_200",
        json!({"status": 200, "body": body})
    );
}

#[tokio::test]
async fn login_selfhost_no_body_matches_fixture() {
    let fx = fixture("local-login_selfhost-no-body.json");
    let test = make_app(&[]);
    // No Content-Type header and no body at all: get_json(silent=True)
    // yields None, treated as {}.
    let response = test.post("/api/auth/local/login", &[], None).await;
    assert_eq!(response.status().as_u16(), fixture_status(&fx));
    let token = {
        let (_, value, _) = parse_set_cookie(&set_cookie_header(&response));
        value
    };
    assert_session_cookie(&response, &token);
    let body = normalized(body_json(response).await, false);
    assert_eq!(body, normalized(fx["response"]["body"].clone(), false));
    insta::assert_json_snapshot!(
        "local_login_selfhost_no_body_200",
        json!({"status": 200, "body": body})
    );
}

#[tokio::test]
async fn login_non_json_content_type_is_credential_free() {
    // get_json(silent=True) semantics: a text/plain body containing valid
    // credential JSON is NOT parsed — the credential-free branch answers.
    let test = make_app(&[]);
    let response = test
        .post(
            "/api/auth/local/login",
            &[("Content-Type", "text/plain")],
            None,
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await["user"]["id"], json!(1));
}

#[tokio::test]
async fn login_credentialed_matches_fixture() {
    let fx = fixture("local-login_credentialed.json");
    let test = make_app(&[]);
    let created = test
        .register(json!({
            "username": "alice",
            "password": "hunter2secret",
            "email": "alice@example.com",
            "name": "Alice"
        }))
        .await;
    assert_eq!(created.status(), StatusCode::CREATED);

    let response = test
        .post(
            "/api/auth/local/login",
            &[],
            Some(json!({"username": "alice", "password": "hunter2secret"})),
        )
        .await;
    assert_eq!(response.status().as_u16(), fixture_status(&fx));
    let cookie_header = set_cookie_header(&response);
    let body = body_json(response).await;
    let token = body["token"].as_str().expect("token").to_string();
    let (_, cookie_value, _) = parse_set_cookie(&cookie_header);
    assert_eq!(cookie_value, token);
    // user.id is autoincrement — fixture notes say not to grade it.
    let body = normalized(body, true);
    assert_eq!(body, normalized(fx["response"]["body"].clone(), true));
    insta::assert_json_snapshot!(
        "local_login_credentialed_200",
        json!({"status": 200, "body": body})
    );
}

#[tokio::test]
async fn login_missing_password_matches_fixture() {
    let fx = fixture("local-login_missing-password-400.json");
    let test = make_app(&[]);
    let response = test
        .post(
            "/api/auth/local/login",
            &[],
            Some(json!({"username": "alice"})),
        )
        .await;
    assert_eq!(response.status().as_u16(), fixture_status(&fx));
    let body = body_json(response).await;
    assert_eq!(body, fx["response"]["body"]);
    insta::assert_json_snapshot!(
        "local_login_missing_password_400",
        json!({"status": 400, "body": body})
    );

    // Empty-string values are only truthiness-checked: same 400. And key
    // presence alone (password only) also routes into the credentialed
    // branch instead of self-host mode.
    for bad in [
        json!({"username": "", "password": ""}),
        json!({"username": "alice", "password": ""}),
        json!({"password": "hunter2secret"}),
    ] {
        let response = test.post("/api/auth/local/login", &[], Some(bad)).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            body_json(response).await,
            json!({ "error": "Username and password are required" })
        );
    }
}

#[tokio::test]
async fn login_wrong_password_matches_fixture() {
    let fx = fixture("local-login_wrong-password-401.json");
    let test = make_app(&[]);
    let created = test
        .register(json!({"username": "alice", "password": "hunter2secret"}))
        .await;
    assert_eq!(created.status(), StatusCode::CREATED);

    let response = test
        .post(
            "/api/auth/local/login",
            &[],
            Some(json!({"username": "alice", "password": "wrongpassword"})),
        )
        .await;
    assert_eq!(response.status().as_u16(), fixture_status(&fx));
    assert!(
        response.headers().get(header::SET_COOKIE).is_none(),
        "no Set-Cookie on failure"
    );
    let body = body_json(response).await;
    assert_eq!(body, fx["response"]["body"]);
    insta::assert_json_snapshot!(
        "local_login_wrong_password_401",
        json!({"status": 401, "body": body})
    );

    // Unknown username: deliberately the identical body.
    let response = test
        .post(
            "/api/auth/local/login",
            &[],
            Some(json!({"username": "nobody", "password": "hunter2secret"})),
        )
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        body_json(response).await,
        json!({ "error": "Invalid credentials" })
    );
}

#[tokio::test]
async fn login_trailing_slash_matches_fixture() {
    let fx = fixture("local-login_trailing-slash-404.json");
    let test = make_app(&[]);
    let response = test
        .post("/api/auth/local/login/", &[], Some(json!({})))
        .await;
    assert_eq!(response.status().as_u16(), fixture_status(&fx));
    let body = body_json(response).await;
    assert_eq!(body, fx["response"]["body"]);
    insta::assert_json_snapshot!(
        "local_login_trailing_slash_404",
        json!({"status": 404, "body": body})
    );
}

#[tokio::test]
async fn login_credential_free_403_when_oidc_configured() {
    // Fixture notes: with OIDC configured the same request is 403.
    // DISABLE_LOCAL_LOGIN=0 is explicit here: with OIDC configured the flag
    // now defaults to true (owner-approved behavior change), which would
    // trip the earlier "Local login is disabled" guard; the explicit 0
    // keeps local login enabled so the credential-free branch's own
    // fail-closed OIDC check is what refuses.
    let test = make_app(&[
        ("OIDC_ISSUER_URL", "https://id.example.com"),
        ("DISABLE_LOCAL_LOGIN", "0"),
    ]);
    let response = test
        .post("/api/auth/local/login", &[], Some(json!({})))
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        body_json(response).await,
        json!({ "error": "Credentials required" })
    );
}

#[tokio::test]
async fn login_defaults_disabled_when_oidc_configured() {
    // Owner-approved behavior change: OIDC configured + DISABLE_LOCAL_LOGIN
    // unset -> local login defaults OFF, so both branches hit the
    // "Local login is disabled" guard.
    let test = make_app(&[("OIDC_ISSUER_URL", "https://id.example.com")]);
    for body in [
        json!({}),
        json!({"username": "alice", "password": "hunter2secret"}),
    ] {
        let response = test.post("/api/auth/local/login", &[], Some(body)).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            body_json(response).await,
            json!({ "error": "Local login is disabled" })
        );
    }
}

#[tokio::test]
async fn login_disabled_403_refuses_both_branches() {
    let test = make_app(&[("DISABLE_LOCAL_LOGIN", "1")]);
    for body in [
        json!({}),
        json!({"username": "alice", "password": "hunter2secret"}),
    ] {
        let response = test.post("/api/auth/local/login", &[], Some(body)).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(
            body_json(response).await,
            json!({ "error": "Local login is disabled" })
        );
    }
}

#[tokio::test]
async fn login_selfhost_honors_configured_identity() {
    let test = make_app(&[
        ("SELFHOST_USER_NAME", "Night Owl"),
        ("SELFHOST_USER_EMAIL", "me@example.com"),
    ]);
    let response = test
        .post("/api/auth/local/login", &[], Some(json!({})))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = body_json(response).await;
    assert_eq!(body["user"]["name"], json!("Night Owl"));
    assert_eq!(body["user"]["email"], json!("me@example.com"));
}

#[tokio::test]
async fn login_cookie_gains_secure_behind_trusted_https_proxy() {
    let test = make_app(&[("TRUST_PROXY_HEADERS", "1")]);
    let response = test
        .post(
            "/api/auth/local/login",
            &[("X-Forwarded-Proto", "https")],
            Some(json!({})),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let (_, _, attrs) = parse_set_cookie(&set_cookie_header(&response));
    assert!(attrs.contains_key("secure"), "Secure expected: {attrs:?}");

    // Same header WITHOUT proxy trust: spoofable, ignored.
    let test = make_app(&[]);
    let response = test
        .post(
            "/api/auth/local/login",
            &[("X-Forwarded-Proto", "https")],
            Some(json!({})),
        )
        .await;
    let (_, _, attrs) = parse_set_cookie(&set_cookie_header(&response));
    assert!(
        !attrs.contains_key("secure"),
        "Secure must be absent: {attrs:?}"
    );
}

#[tokio::test]
async fn login_with_legacy_werkzeug_hash_rehashes_to_argon2id() {
    let test = make_app(&[]);
    // Pinned werkzeug hash for "hunter2" (cheap pbkdf2 params keep the test
    // fast; same pinned fixture family as the password unit tests). Seeded
    // directly — the register route now writes argon2id, so a legacy row
    // can only exist via direct insertion.
    let legacy_hash = "pbkdf2:sha256:1000$9mGk3jaNAJBbWfu4$a956000d41c9a2a08ee917585fe66311acd661744951aac17d3aa9a1e89aca0f";
    let pool = db::open_pool(&test.db_path).expect("pool");
    let user_id = {
        let conn = pool.get().expect("conn");
        db::users::create_local_user(&conn, "legacy", legacy_hash, None, None).expect("seed user")
    };

    // First login verifies against the werkzeug hash and transparently
    // upgrades the stored value to argon2id.
    let creds = json!({"username": "legacy", "password": "hunter2"});
    let response = test
        .post("/api/auth/local/login", &[], Some(creds.clone()))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let stored = {
        let conn = pool.get().expect("conn");
        db::users::get_user_password_hash(&conn, user_id)
            .expect("query")
            .expect("stored hash")
    };
    assert!(
        stored.starts_with("$argon2id$"),
        "expected argon2id after rehash-on-login, got {stored}"
    );

    // The same password logs in again against the upgraded hash…
    let response = test.post("/api/auth/local/login", &[], Some(creds)).await;
    assert_eq!(response.status(), StatusCode::OK);
    // …and an argon2id hash is left alone (no needless rewrite).
    let stored_again = {
        let conn = pool.get().expect("conn");
        db::users::get_user_password_hash(&conn, user_id)
            .expect("query")
            .expect("stored hash")
    };
    assert_eq!(stored, stored_again);

    // Wrong password is still rejected after the upgrade.
    let response = test
        .post(
            "/api/auth/local/login",
            &[],
            Some(json!({"username": "legacy", "password": "hunter3"})),
        )
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        body_json(response).await,
        json!({ "error": "Invalid credentials" })
    );
}

// ---------------------------------------------------------------------------
// POST /auth/local/register
// ---------------------------------------------------------------------------

#[tokio::test]
async fn register_bearer_matches_fixture() {
    let fx = fixture("local-register_bearer-201.json");
    let test = make_app(&[]);
    let response = test
        .register(json!({
            "username": "alice",
            "password": "hunter2secret",
            "email": "alice@example.com",
            "name": "Alice"
        }))
        .await;
    assert_eq!(response.status().as_u16(), fixture_status(&fx));
    assert_json_content_type(&response);
    assert!(
        response.headers().get(header::SET_COOKIE).is_none(),
        "registering does not log the new user in"
    );
    let body = normalized(body_json(response).await, true);
    assert_eq!(body, normalized(fx["response"]["body"].clone(), true));
    insta::assert_json_snapshot!(
        "local_register_bearer_201",
        json!({"status": 201, "body": body})
    );
}

#[tokio::test]
async fn register_defaults_email_and_name() {
    // DB-layer defaults: email -> "<username>@localhost", name -> username;
    // never null in the response.
    let test = make_app(&[]);
    let response = test
        .register(json!({"username": "carol", "password": "hunter2secret"}))
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = body_json(response).await;
    assert_eq!(body["user"]["email"], json!("carol@localhost"));
    assert_eq!(body["user"]["name"], json!("carol"));
    assert_eq!(body["user"]["avatar_url"], Value::Null);

    // Whitespace around the username is stripped before use.
    let response = test
        .register(json!({"username": "  dave  ", "password": "hunter2secret"}))
        .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = body_json(response).await;
    assert_eq!(body["user"]["name"], json!("dave"));

    // The new user can immediately log in.
    let response = test
        .post(
            "/api/auth/local/login",
            &[],
            Some(json!({"username": "carol", "password": "hunter2secret"})),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn register_duplicate_matches_fixture() {
    let fx = fixture("local-register_duplicate-409.json");
    let test = make_app(&[]);
    let first = test
        .register(json!({"username": "alice", "password": "hunter2secret"}))
        .await;
    assert_eq!(first.status(), StatusCode::CREATED);
    let response = test
        .register(json!({"username": "alice", "password": "hunter2secret"}))
        .await;
    assert_eq!(response.status().as_u16(), fixture_status(&fx));
    let body = body_json(response).await;
    assert_eq!(body, fx["response"]["body"]);
    insta::assert_json_snapshot!(
        "local_register_duplicate_409",
        json!({"status": 409, "body": body})
    );
}

#[tokio::test]
async fn register_no_auth_matches_fixture() {
    let fx = fixture("local-register_no-auth-401.json");
    let test = make_app(&[]);
    let response = test
        .post(
            "/api/auth/local/register",
            &[],
            Some(json!({"username": "bob", "password": "hunter2secret"})),
        )
        .await;
    assert_eq!(response.status().as_u16(), fixture_status(&fx));
    let body = body_json(response).await;
    assert_eq!(body, fx["response"]["body"]);
    insta::assert_json_snapshot!(
        "local_register_no_auth_401",
        json!({"status": 401, "body": body})
    );
}

#[tokio::test]
async fn register_short_password_matches_fixture() {
    let fx = fixture("local-register_short-password-400.json");
    let test = make_app(&[]);
    let response = test
        .register(json!({"username": "carol", "password": "short"}))
        .await;
    assert_eq!(response.status().as_u16(), fixture_status(&fx));
    let body = body_json(response).await;
    assert_eq!(body, fx["response"]["body"]);
    insta::assert_json_snapshot!(
        "local_register_short_password_400",
        json!({"status": 400, "body": body})
    );
}

#[tokio::test]
async fn register_validation_order_username_then_password() {
    let test = make_app(&[]);
    // Missing / blank / non-string username → "Username is required".
    for body in [
        json!({}),
        json!({"username": "", "password": "hunter2secret"}),
        json!({"username": "   ", "password": "hunter2secret"}),
        json!({"username": 123, "password": "hunter2secret"}),
        json!({"username": null, "password": "hunter2secret"}),
    ] {
        let response = test.register(body.clone()).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(
            body_json(response).await,
            json!({ "error": "Username is required" }),
            "{body}"
        );
    }
    // Missing / empty / non-string password → "Password is required".
    for body in [
        json!({"username": "bob"}),
        json!({"username": "bob", "password": ""}),
        json!({"username": "bob", "password": 12345678}),
    ] {
        let response = test.register(body.clone()).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(
            body_json(response).await,
            json!({ "error": "Password is required" }),
            "{body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rate limiting (fixture notes: 30/min login, 10/min register)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn login_rate_limited_after_30_per_minute() {
    let test = make_app(&[]);
    for i in 0..30 {
        let response = test
            .post("/api/auth/local/login", &[], Some(json!({})))
            .await;
        assert_eq!(response.status(), StatusCode::OK, "request {i}");
    }
    let response = test
        .post("/api/auth/local/login", &[], Some(json!({})))
        .await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        body_json(response).await,
        json!({ "error": "Rate limit exceeded. Please try again later." })
    );
}

#[tokio::test]
async fn register_rate_limited_after_10_per_minute_but_not_before_auth() {
    let test = make_app(&[]);
    // Even failing (400) requests consume budget: the limiter wraps the
    // view, running before body validation.
    for i in 0..10 {
        let response = test
            .register(json!({"username": "carol", "password": "short"}))
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "request {i}");
    }
    let response = test
        .register(json!({"username": "carol", "password": "hunter2secret"}))
        .await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        body_json(response).await,
        json!({ "error": "Rate limit exceeded. Please try again later." })
    );
    // require_auth still runs BEFORE the limiter: an unauthenticated
    // request gets 401, never 429.
    let response = test
        .post(
            "/api/auth/local/register",
            &[],
            Some(json!({"username": "x", "password": "hunter2secret"})),
        )
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    // And the login endpoint's budget is untouched (separate bucket).
    let response = test
        .post("/api/auth/local/login", &[], Some(json!({})))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn rate_limiter_bypassed_in_testing_env() {
    let test = make_app(&[("APP_ENV", "testing")]);
    for i in 0..35 {
        let response = test
            .post("/api/auth/local/login", &[], Some(json!({})))
            .await;
        assert_eq!(response.status(), StatusCode::OK, "request {i}");
    }
}

#[tokio::test]
async fn rate_limiter_fails_open_when_store_is_unwritable() {
    // Point the limiter store at a directory: every check errors, and the
    // deliberate FAIL-OPEN policy lets logins through instead of 500/429.
    let dir = tempfile::tempdir().unwrap();
    let test = make_app(&[("RATE_LIMIT_DB_PATH", dir.path().to_string_lossy().as_ref())]);
    for i in 0..3 {
        let response = test
            .post("/api/auth/local/login", &[], Some(json!({})))
            .await;
        assert_eq!(response.status(), StatusCode::OK, "request {i}");
    }
}

#[tokio::test]
async fn forwarded_for_buckets_only_when_proxy_is_trusted() {
    // Trusted proxy: X-Forwarded-For (first value) is the bucket key, so a
    // different client behind the same proxy gets a fresh budget.
    let test = make_app(&[("TRUST_PROXY_HEADERS", "1")]);
    for i in 0..30 {
        let response = test
            .post(
                "/api/auth/local/login",
                &[("X-Forwarded-For", "9.9.9.9, 10.0.0.1")],
                Some(json!({})),
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK, "request {i}");
    }
    let response = test
        .post(
            "/api/auth/local/login",
            &[("X-Forwarded-For", "9.9.9.9, 10.0.0.1")],
            Some(json!({})),
        )
        .await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let response = test
        .post(
            "/api/auth/local/login",
            &[("X-Forwarded-For", "8.8.8.8")],
            Some(json!({})),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);

    // Untrusted (default): the forged header is ignored — every caller
    // shares the fallback bucket, so rotating the header cannot bypass the
    // cap.
    let test = make_app(&[]);
    for i in 0..30 {
        let response = test
            .post(
                "/api/auth/local/login",
                &[("X-Forwarded-For", &format!("1.2.3.{i}"))],
                Some(json!({})),
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK, "request {i}");
    }
    let response = test
        .post(
            "/api/auth/local/login",
            &[("X-Forwarded-For", "7.7.7.7")],
            Some(json!({})),
        )
        .await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
}
