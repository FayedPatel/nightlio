//! Shell integration tests: the assembled router + middleware stack vs the
//! golden fixtures in `contract/fixtures/misc/`, plus the auth extractor's
//! exact 401/403 bodies, the security-header stack, CORS preflight, the
//! `/api` 308 redirect, and Flask `<int:...>` path semantics.

use std::collections::HashMap;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};
use tower::ServiceExt;

use nightlio_api::auth::extract::AuthUser;
use nightlio_api::auth::jwt;
use nightlio_api::config::Config;
use nightlio_api::db::{self, SelfHostSeed};
use nightlio_api::routes::{self, CONTENT_SECURITY_POLICY, FlaskInt, FlaskPath};
use nightlio_api::state::AppState;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn make_state(extra: &[(&str, &str)]) -> (AppState, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("nightlio.db");
    let mut vars: HashMap<String, String> = HashMap::new();
    vars.insert("APP_ENV".to_string(), "development".to_string());
    for (key, value) in extra {
        vars.insert((*key).to_string(), (*value).to_string());
    }
    let lookup = move |key: &str| vars.get(key).cloned();
    let mut cfg = Config::from_lookup(&lookup);
    cfg.database_path = db_path.to_string_lossy().into_owned();
    db::bootstrap(&cfg.database_path, &SelfHostSeed::from(&cfg)).expect("bootstrap");
    let pool = db::open_pool(&cfg.database_path).expect("pool");
    (AppState::new(cfg, pool), dir)
}

fn make_app(extra: &[(&str, &str)]) -> (Router, tempfile::TempDir) {
    let (state, dir) = make_state(extra);
    (routes::build_router(state), dir)
}

async fn send(app: &Router, request: Request<Body>) -> Response {
    app.clone().oneshot(request).await.expect("infallible")
}

async fn get_path(app: &Router, path: &str) -> Response {
    send(
        app,
        Request::builder().uri(path).body(Body::empty()).unwrap(),
    )
    .await
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
        "{}/../contract/fixtures/misc/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|exc| panic!("missing fixture {path}: {exc}"));
    serde_json::from_str(&raw).expect("fixture json")
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

// ---------------------------------------------------------------------------
// misc family vs fixtures
// ---------------------------------------------------------------------------

#[tokio::test]
async fn health_matches_fixture() {
    let (app, _dir) = make_app(&[]);
    let response = get_path(&app, "/api/").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_json_content_type(&response);
    let mut body = body_json(response).await;
    let ts = body["timestamp"].as_f64().expect("timestamp is a float");
    assert!(ts > 1_500_000_000.0, "epoch seconds, got {ts}");
    body["timestamp"] = Value::String("<TS>".to_string());
    assert_eq!(body, fixture("health_get_200.json")["response"]["body"]);
}

#[tokio::test]
async fn time_matches_fixture() {
    let (app, _dir) = make_app(&[]);
    let response = get_path(&app, "/api/time").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_json_content_type(&response);
    let mut body = body_json(response).await;
    assert!(body["time"].as_f64().is_some(), "time is a float");
    body["time"] = Value::String("<TS>".to_string());
    assert_eq!(body, fixture("time_get_200.json")["response"]["body"]);
}

#[tokio::test]
async fn config_matches_fixture() {
    let (app, _dir) = make_app(&[]);
    let response = get_path(&app, "/api/config").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_json_content_type(&response);
    assert_eq!(
        body_json(response).await,
        fixture("config_get_200.json")["response"]["body"]
    );
}

#[tokio::test]
async fn config_signup_url_null_unless_oidc_enabled() {
    // Env var set but OIDC off → forced null.
    let (app, _dir) = make_app(&[("OIDC_SIGNUP_URL", "https://id.example.com/signup")]);
    let body = body_json(get_path(&app, "/api/config").await).await;
    assert_eq!(body["signup_url"], Value::Null);
    assert_eq!(body["enable_oidc"], json!(false));

    // OIDC on → the (scheme-validated) URL flows through.
    let (app, _dir) = make_app(&[
        ("OIDC_ISSUER_URL", "https://id.example.com"),
        ("OIDC_SIGNUP_URL", "https://id.example.com/signup"),
    ]);
    let body = body_json(get_path(&app, "/api/config").await).await;
    assert_eq!(body["enable_oidc"], json!(true));
    assert_eq!(body["signup_url"], json!("https://id.example.com/signup"));
}

#[tokio::test]
async fn config_local_login_defaults_off_when_oidc_configured() {
    // Owner-approved behavior change: with OIDC configured and
    // DISABLE_LOCAL_LOGIN unset, local login defaults OFF.
    let (app, _dir) = make_app(&[("OIDC_ISSUER_URL", "https://id.example.com")]);
    let body = body_json(get_path(&app, "/api/config").await).await;
    assert_eq!(body["enable_oidc"], json!(true));
    assert_eq!(body["enable_local_login"], json!(false));

    // Explicit DISABLE_LOCAL_LOGIN=0 re-enables local login even with SSO.
    let (app, _dir) = make_app(&[
        ("OIDC_ISSUER_URL", "https://id.example.com"),
        ("DISABLE_LOCAL_LOGIN", "0"),
    ]);
    let body = body_json(get_path(&app, "/api/config").await).await;
    assert_eq!(body["enable_oidc"], json!(true));
    assert_eq!(body["enable_local_login"], json!(true));

    // No OIDC, flag unset: unchanged — local login stays on (this is also
    // what config_matches_fixture pins via the OIDC-blanked fixture).
    let (app, _dir) = make_app(&[]);
    let body = body_json(get_path(&app, "/api/config").await).await;
    assert_eq!(body["enable_oidc"], json!(false));
    assert_eq!(body["enable_local_login"], json!(true));

    // Explicit =1 without SSO still disables (unchanged).
    let (app, _dir) = make_app(&[("DISABLE_LOCAL_LOGIN", "1")]);
    let body = body_json(get_path(&app, "/api/config").await).await;
    assert_eq!(body["enable_local_login"], json!(false));
}

#[tokio::test]
async fn api_without_slash_redirects_308() {
    let (app, _dir) = make_app(&[]);
    let response = send(
        &app,
        Request::builder()
            .uri("/api")
            .header(header::HOST, "127.0.0.1:5306")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
    let expected =
        &fixture("health_get_no_trailing_slash_308.json")["response"]["headers"]["Location"];
    assert_eq!(
        response
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        expected.as_str()
    );
    // contract change: empty body, Location is the contract.
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(body.is_empty(), "308 body should be empty, was {body:?}");
}

#[tokio::test]
async fn unknown_path_is_json_404() {
    let (app, _dir) = make_app(&[]);
    let response = get_path(&app, "/api/nonexistent").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_json_content_type(&response);
    assert_eq!(
        body_json(response).await,
        fixture("nonexistent_get_404.json")["response"]["body"]
    );
}

#[tokio::test]
async fn trailing_slash_variants_404() {
    let (app, _dir) = make_app(&[]);
    for path in ["/api/time/", "/api/config/"] {
        let response = get_path(&app, path).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert_eq!(
            body_json(response).await,
            json!({ "error": "Resource not found" }),
            "{path}"
        );
    }
}

#[tokio::test]
async fn post_time_is_405_with_allow() {
    let (app, _dir) = make_app(&[]);
    let response = send(
        &app,
        Request::builder()
            .method("POST")
            .uri("/api/time")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    let allow = response
        .headers()
        .get(header::ALLOW)
        .and_then(|value| value.to_str().ok())
        .expect("Allow header on 405");
    assert!(allow.contains("GET"), "Allow was {allow:?}");
    // contract change: 405 carries the standard JSON envelope.
    assert_json_content_type(&response);
    assert_eq!(
        body_json(response).await,
        fixture("time_post_405.json")["response"]["body"]
    );
}

#[tokio::test]
async fn plain_options_returns_automatic_204() {
    // contract change: all OPTIONS answer 204-empty with Allow.
    let (app, _dir) = make_app(&[]);
    let response = send(
        &app,
        Request::builder()
            .method("OPTIONS")
            .uri("/api/time")
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
        Some("HEAD, GET, OPTIONS")
    );
    // No Content-Length assertion: the wire response carries none (hyper
    // strips it from 204s), but axum stamps it on the in-process response.
    assert_eq!(body_string(response).await, "");
}

// ---------------------------------------------------------------------------
// Middleware stack
// ---------------------------------------------------------------------------

#[tokio::test]
async fn security_headers_on_every_response_no_csp_in_dev() {
    let (app, _dir) = make_app(&[]);
    for path in ["/api/", "/api/nonexistent"] {
        let response = get_path(&app, path).await;
        let headers = response.headers();
        assert_eq!(
            headers.get(header::X_FRAME_OPTIONS).unwrap(),
            "DENY",
            "{path}"
        );
        assert_eq!(
            headers.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(),
            "nosniff",
            "{path}"
        );
        assert_eq!(
            headers.get(header::X_XSS_PROTECTION).unwrap(),
            "1; mode=block",
            "{path}"
        );
        assert_eq!(
            headers.get(header::REFERRER_POLICY).unwrap(),
            "strict-origin-when-cross-origin",
            "{path}"
        );
        // DEBUG env (development): no CSP, matching security_headers.py.
        assert!(
            headers.get(header::CONTENT_SECURITY_POLICY).is_none(),
            "{path}"
        );
    }
}

#[tokio::test]
async fn csp_present_in_production_only() {
    let (app, _dir) = make_app(&[("APP_ENV", "production")]);
    let response = get_path(&app, "/api/").await;
    assert_eq!(
        response
            .headers()
            .get(header::CONTENT_SECURITY_POLICY)
            .and_then(|value| value.to_str().ok()),
        Some(CONTENT_SECURITY_POLICY)
    );
}

#[tokio::test]
async fn cors_preflight_is_answered_for_allowed_origin() {
    let (app, _dir) = make_app(&[]);
    let response = send(
        &app,
        Request::builder()
            .method("OPTIONS")
            .uri("/api/time")
            .header(header::ORIGIN, "http://localhost:5173")
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "PUT")
            .header(
                header::ACCESS_CONTROL_REQUEST_HEADERS,
                "authorization,content-type",
            )
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    // contract change: preflights ride the automatic-OPTIONS 204;
    // the after-request CORS layer appends the CORS headers.
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let headers = response.headers();
    assert_eq!(
        headers
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|value| value.to_str().ok()),
        Some("http://localhost:5173")
    );
    assert_eq!(
        headers
            .get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS)
            .and_then(|value| value.to_str().ok()),
        Some("true")
    );
    // flask-cors echoes the requested headers lowercased, ", "-joined —
    // matches the recorded preflight fixture.
    assert_eq!(
        headers
            .get(header::ACCESS_CONTROL_ALLOW_HEADERS)
            .and_then(|value| value.to_str().ok()),
        Some("authorization, content-type")
    );
    assert_eq!(
        headers
            .get(header::ACCESS_CONTROL_ALLOW_METHODS)
            .and_then(|value| value.to_str().ok()),
        Some("DELETE, GET, HEAD, OPTIONS, PATCH, POST, PUT")
    );
    // The preflight is layered on Flask's automatic OPTIONS response, so
    // the Allow header is present too (fixture-verified).
    assert_eq!(
        headers
            .get(header::ALLOW)
            .and_then(|value| value.to_str().ok()),
        Some("HEAD, GET, OPTIONS")
    );
    assert_eq!(
        headers
            .get(header::VARY)
            .and_then(|value| value.to_str().ok()),
        Some("Origin")
    );
    // Preflight responses carry the security headers too (Flask
    // after_request stamps every response).
    assert_eq!(headers.get(header::X_FRAME_OPTIONS).unwrap(), "DENY");
}

#[tokio::test]
async fn options_on_unknown_path_is_json_404() {
    // flask-cors never short-circuits routing: OPTIONS on an unregistered
    // path falls through to the app-level JSON 404.
    let (app, _dir) = make_app(&[]);
    let response = send(
        &app,
        Request::builder()
            .method("OPTIONS")
            .uri("/api/nonexistent")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        body_json(response).await,
        json!({ "error": "Resource not found" })
    );
}

#[tokio::test]
async fn cors_simple_request_gets_allow_origin() {
    let (app, _dir) = make_app(&[]);
    let response = send(
        &app,
        Request::builder()
            .uri("/api/time")
            .header(header::ORIGIN, "http://localhost:5173")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|value| value.to_str().ok()),
        Some("http://localhost:5173")
    );
    // Unlisted origins get no CORS headers.
    let response = send(
        &app,
        Request::builder()
            .uri("/api/time")
            .header(header::ORIGIN, "http://evil.example")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert!(
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none()
    );
}

#[tokio::test]
async fn proxy_fix_only_when_trusted() {
    // Trusted: rightmost X-Forwarded-Proto/Host drive the redirect target.
    let (app, _dir) = make_app(&[("TRUST_PROXY_HEADERS", "1")]);
    let response = send(
        &app,
        Request::builder()
            .uri("/api")
            .header(header::HOST, "internal:5000")
            .header("X-Forwarded-Proto", "https")
            .header("X-Forwarded-Host", "public.example")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
    assert_eq!(
        response
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some("https://public.example/api/")
    );

    // Untrusted (default): the spoofable headers are ignored.
    let (app, _dir) = make_app(&[]);
    let response = send(
        &app,
        Request::builder()
            .uri("/api")
            .header(header::HOST, "internal:5000")
            .header("X-Forwarded-Proto", "https")
            .header("X-Forwarded-Host", "public.example")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        response
            .headers()
            .get(header::LOCATION)
            .and_then(|value| value.to_str().ok()),
        Some("http://internal:5000/api/")
    );
}

// ---------------------------------------------------------------------------
// AuthUser extractor
// ---------------------------------------------------------------------------

async fn whoami(user: AuthUser) -> Json<Value> {
    Json(json!({ "user_id": user.user_id }))
}

fn auth_app() -> (Router, String, tempfile::TempDir) {
    let (state, dir) = make_state(&[]);
    let secret = state.config.jwt_secret.clone();
    let app = Router::new()
        .route("/api/whoami", get(whoami).put(whoami))
        .with_state(state);
    (app, secret, dir)
}

fn valid_token(secret: &str) -> String {
    jwt::issue_token(secret, 42).expect("token")
}

fn expired_token(secret: &str) -> String {
    let now = chrono::Utc::now().timestamp();
    jwt::issue_token_at(secret, 42, now - 7200).expect("token")
}

#[tokio::test]
async fn no_credentials_is_exact_401() {
    let (app, _secret, _dir) = auth_app();
    let response = get_path(&app, "/api/whoami").await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_json_content_type(&response);
    // Same body whether the header, the cookie, or both are missing —
    // matches the recorded preferences_get_401 fixture.
    assert_eq!(
        body_json(response).await,
        fixture("preferences_get_401.json")["response"]["body"]
    );
}

#[tokio::test]
async fn bearer_token_authenticates() {
    let (app, secret, _dir) = auth_app();
    let response = send(
        &app,
        Request::builder()
            .uri("/api/whoami")
            .header(
                header::AUTHORIZATION,
                format!("Bearer {}", valid_token(&secret)),
            )
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await, json!({ "user_id": 42 }));
}

#[tokio::test]
async fn cookie_token_authenticates_gets() {
    let (app, secret, _dir) = auth_app();
    let response = send(
        &app,
        Request::builder()
            .uri("/api/whoami")
            .header(
                header::COOKIE,
                format!("nightlio_token={}", valid_token(&secret)),
            )
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await, json!({ "user_id": 42 }));
}

#[tokio::test]
async fn expired_and_invalid_tokens_have_exact_bodies() {
    let (app, secret, _dir) = auth_app();
    let response = send(
        &app,
        Request::builder()
            .uri("/api/whoami")
            .header(
                header::AUTHORIZATION,
                format!("Bearer {}", expired_token(&secret)),
            )
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        body_json(response).await,
        json!({ "error": "Token expired" })
    );

    let response = send(
        &app,
        Request::builder()
            .uri("/api/whoami")
            .header(header::AUTHORIZATION, "Bearer not-a-jwt")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        body_json(response).await,
        json!({ "error": "Invalid token" })
    );
}

#[tokio::test]
async fn cookie_mutation_without_csrf_header_is_403() {
    let (app, secret, _dir) = auth_app();
    let response = send(
        &app,
        Request::builder()
            .method("PUT")
            .uri("/api/whoami")
            .header(
                header::COOKIE,
                format!("nightlio_token={}", valid_token(&secret)),
            )
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        body_json(response).await,
        fixture("preferences_put_403_cookie_missing_csrf_header.json")["response"]["body"]
    );
}

#[tokio::test]
async fn cookie_mutation_with_wrong_content_type_is_403_content_type_first() {
    let (app, secret, _dir) = auth_app();
    let response = send(
        &app,
        Request::builder()
            .method("PUT")
            .uri("/api/whoami")
            .header(
                header::COOKIE,
                format!("nightlio_token={}", valid_token(&secret)),
            )
            .header(header::CONTENT_TYPE, "text/plain")
            .header("X-Requested-With", "nightlio")
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        body_json(response).await,
        json!({ "error": "Content-Type must be application/json" })
    );
}

#[tokio::test]
async fn csrf_runs_before_token_verification() {
    // Expired cookie token + missing CSRF header: Flask 403s (CSRF is
    // checked before jwt.decode), never 401.
    let (app, secret, _dir) = auth_app();
    let response = send(
        &app,
        Request::builder()
            .method("PUT")
            .uri("/api/whoami")
            .header(
                header::COOKIE,
                format!("nightlio_token={}", expired_token(&secret)),
            )
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        body_json(response).await,
        json!({ "error": "Missing required request header" })
    );
}

#[tokio::test]
async fn cookie_mutation_with_full_csrf_passes() {
    let (app, secret, _dir) = auth_app();
    let response = send(
        &app,
        Request::builder()
            .method("PUT")
            .uri("/api/whoami")
            .header(
                header::COOKIE,
                format!("nightlio_token={}", valid_token(&secret)),
            )
            .header(header::CONTENT_TYPE, "application/json")
            .header("X-Requested-With", "nightlio")
            .body(Body::from("{}"))
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await, json!({ "user_id": 42 }));
}

#[tokio::test]
async fn bearer_mutation_bypasses_csrf() {
    let (app, secret, _dir) = auth_app();
    let response = send(
        &app,
        Request::builder()
            .method("PUT")
            .uri("/api/whoami")
            .header(
                header::AUTHORIZATION,
                format!("Bearer {}", valid_token(&secret)),
            )
            // No Content-Type, no X-Requested-With.
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

// ---------------------------------------------------------------------------
// Flask <int:...> path semantics
// ---------------------------------------------------------------------------

async fn item(FlaskPath(FlaskInt(id)): FlaskPath<FlaskInt>) -> Json<Value> {
    Json(json!({ "id": id }))
}

#[tokio::test]
async fn flask_path_404s_like_the_int_converter() {
    let (state, _dir) = make_state(&[]);
    let app = Router::new()
        .route("/api/item/{id}", get(item))
        .fallback(routes::fallback_not_found)
        .with_state(state);

    let response = get_path(&app, "/api/item/7").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body_json(response).await, json!({ "id": 7 }));

    // Negative, non-integer, and overflow (> i64, contract/DECISIONS.md #1)
    // ids: the rule never matches → clean JSON 404, never 400/500.
    for bad in ["-5", "abc", "1.5", "184467440737095516151234"] {
        let response = get_path(&app, &format!("/api/item/{bad}")).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{bad}");
        assert_eq!(
            body_json(response).await,
            json!({ "error": "Resource not found" }),
            "{bad}"
        );
    }
}

// ---------------------------------------------------------------------------
// Contract encoding facts (DECISIONS.md "Contract encoding facts")
// ---------------------------------------------------------------------------

#[tokio::test]
async fn json_bodies_are_compact_without_trailing_newline() {
    // Bodies are compact serde_json output: no pretty-printing, no
    // trailing newline. Pinned so a serializer change cannot silently
    // alter the wire encoding. The float-bearing `/api/time` body only
    // round-trips byte-exactly because the crate enables serde_json's
    // `float_roundtrip` feature (default f64 parsing is 1-ulp imprecise).
    // Regression pin: this exact epoch value round-tripped 1 ulp off
    // ("…23") under the default imprecise parser.
    let pinned = r#"{"time":1786933306.9170625}"#;
    let parsed: Value = serde_json::from_str(pinned).expect("pinned json");
    assert_eq!(pinned, serde_json::to_string(&parsed).expect("compact"));

    let (app, _dir) = make_app(&[]);
    for path in ["/api/time", "/api/nonexistent"] {
        let response = get_path(&app, path).await;
        let raw = body_string(response).await;
        let parsed: Value = serde_json::from_str(&raw).expect("json body");
        assert_eq!(
            raw,
            serde_json::to_string(&parsed).expect("compact"),
            "{path}"
        );
    }
}

#[tokio::test]
async fn originless_request_gets_no_cors_headers() {
    // Requests without an Origin header are not CORS requests: the
    // after-request layer must not emit any Access-Control-* header.
    let (app, _dir) = make_app(&[]);
    let response = get_path(&app, "/api/time").await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers();
    for name in [
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
        header::ACCESS_CONTROL_ALLOW_METHODS,
        header::ACCESS_CONTROL_ALLOW_HEADERS,
    ] {
        assert!(headers.get(&name).is_none(), "{name} must be absent");
    }
}
