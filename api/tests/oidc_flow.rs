//! OIDC SSO end-to-end against a wiremock fake provider (discovery, JWKS,
//! and token endpoints over real HTTP), mirroring `api/auth/oauth.py`:
//! happy path, state mismatch, nonce mismatch, expired ID token, wrong
//! audience, wrong issuer, and the `_safe_frontend_base` fallback for an
//! off-host `FRONTEND_URL`. Also the mount contract: routes absent (JSON
//! 404) when OIDC is unconfigured, 404 `OIDC is not configured` when the
//! issuer is set without a client id.
//!
//! Since v0.6.0 every test loops over the available backends
//! (`support::backends()`): SQLite always, exactly as before, plus a
//! PostgreSQL twin (exercising the PG user-upsert path) when
//! `NIGHTLIO_PG_TEST_URL` is set (see `tests/support/mod.rs`).

use std::collections::HashMap;

mod support;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use chrono::{Duration, Utc};
use openidconnect::core::{
    CoreIdToken, CoreIdTokenClaims, CoreJsonWebKeySet, CoreJwsSigningAlgorithm,
    CoreRsaPrivateSigningKey,
};
use openidconnect::{
    Audience, EmptyAdditionalClaims, EndUserEmail, EndUserName, IssuerUrl, JsonWebKeyId,
    LocalizedClaim, Nonce, PrivateSigningKey, StandardClaims, SubjectIdentifier,
};
use serde_json::{Value, json};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use nightlio_api::auth::jwt;
use nightlio_api::config::Config;
use nightlio_api::db::{self, SelfHostSeed};
use nightlio_api::routes;
use nightlio_api::state::AppState;

/// The Host header every request carries (Flask `request.host`).
const HOST: &str = "app.example.test";
const CLIENT_ID: &str = "test-client-id";

/// Throwaway 2048-bit RSA key, generated for these tests only.
const RSA_TEST_PEM: &str = r#"-----BEGIN RSA PRIVATE KEY-----
MIIEowIBAAKCAQEApdM7MpCaw1IQCW/xvgatoEJiE/hykAXjn04j/VaSYgQXKcRr
Fwf7rqAMR/RrXU8eF1h07OzXVZoVGQBj7ou7R0QD2kvLZqQgQojnZTKlENLMfwXA
BoDR5Vdli7/MOoYjRpom9t7yXy2jTP40aKg3Bw0MiNVhUDI+oa/+RxtGi6cbxx2m
lyVsWFCGON+Uvh5jUY9I2clFwnsmP9iBuAjo9CDNJwKriCVw/eD2vUsb/bSWPSqH
HwNxUJIgy5qRTPGjqvWJoDW8jT1X8cgyFtiEPsmJh7qusbrUN1G2HxMSPPBo5sys
6bDHpnL87cvGHR7F28/YwfH1fjW4dH6fPowyaQIDAQABAoIBAAyRZqHFthvFIbbc
D5NNMBt0GeO1F2u6goli/lUiWqxdQ9XUJPYFJ8pmUh/2/VcrBVcUICKKgySMrc+B
Xnw922xtPNSD8kR909MZy13kZCsoVZGarDaMohE9hJJTr6Uk4FIRd2VhDuIZ/vqO
zrE670FpzVrRP1Ojxz2bwuF9RqVpXj/wZU6HFtmdIMhrLmkkshOmrZU6an44r9L6
S26uvUFbsgTPXZ9oVUV7kvujAnq40UaK4eY7JrtsTDTRDKtyxJaE2zDfDaT2F87T
68HPXkVmg9bhTIqdcN2bZVevK/8iIYWl8hLEOtg6tgVuIDoE4YVA0XdC+htGPlAQ
RKJs2f0CgYEA2OiKuA4X0jdyDwPNE1qsonPyEc7sCHPX1GTSbPlYFUolWYGVYPAt
BAh2L29kHbQJpKG+pUYcfZc15jWmo9jiQTLVZ7WsE9Nxq2jPTKkzOWz8h9cTjHuc
rO0MSHjLABLwMeZ6ei7Tbsrf5bSkkns4rCPPpmfYwm0Bz8VkUlYl3z0CgYEAw7Xf
M5oa6OicVmQ8pK9KtajUO+nXKMmWrWkzoRVRx1Y3WveZcmTG6JozVAwZkrnnsQo1
uvR8Z7Ads2I5aW1Nlh9i5ScJXTPn3ohBjtWs80Qkp2yjvqM6NpysYXO6tN3Szy6z
FKQ7WnUg9hFPWe+SVHMP+CSNUwaaHHTiirggEp0CgYAeujKMiFKPkRMzVVKD32B9
UveD1lBRkjeM+wtkLJ5xxaMs3tKOfPejjp9PcPQ50Ptcux0KxLfcgsM77XXB2EOV
AOKCYpYR6O49Xgef0IhVJj9P7wPx7sDvLlWDHrmDNSuZphDLpj6Ff2/gVorJxXLt
z9TmuedXA6IyEMB5eYK78QKBgQCUKQV2fT3OAPsJ9Axs6D940v0I9nhqamJlmXT6
h7dHXx+9ACDslxp2UPZ2tEpP5+lc/8u5YwkjPhLeEIhCJftMoSovLKRMKNVqhGCN
D3pFF9tf3EECO3QAkA94HzLDZgMH0eTExaghTPbNEkGuZk2zHQCD7LgImMDmth4i
wk2ViQKBgHeq8nHPeEjCmchS0SlpGVixW4PvcrksbEIuL7fZpRJtTN6/fG/hzyE9
GXOoU0AJ2f0b56IAaElZdJkF3T/9/d4VLifLS63P5tziN2Yh2bIiHsJx0kEWUOr1
72Vhl0CS66TzKZlc02jW3mDaRVqYyfyzw646g8Md32VtqLNDWG6m
-----END RSA PRIVATE KEY-----"#;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct TestApp {
    app: Router,
    jwt_secret: String,
    _db: support::TestDb,
}

/// Development-env app on a tempfile DB (or a private PostgreSQL database
/// when the backend is `Pg`), like the auth-family harness.
async fn make_app_on(backend: support::Backend, extra: &[(&str, &str)]) -> TestApp {
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
    let jwt_secret = cfg.jwt_secret.clone();
    let (handle, test_db) = match backend {
        support::Backend::Sqlite => {
            db::bootstrap(&cfg.database_path, &SelfHostSeed::from(&cfg)).expect("bootstrap");
            let pool = db::open_pool(&cfg.database_path).expect("pool");
            (
                db::DbHandle::Sqlite(pool),
                support::TestDb::Sqlite {
                    path: cfg.database_path.clone(),
                    _dir: dir,
                },
            )
        }
        support::Backend::Pg => {
            let url = support::create_pg_db(&SelfHostSeed::from(&cfg)).await;
            let pool = db::pg::build_pool(&url).expect("pg pool");
            (
                db::DbHandle::Pg(pool),
                support::TestDb::Pg { url, _dir: dir },
            )
        }
    };
    TestApp {
        app: routes::build_router(AppState::new(cfg, handle)),
        jwt_secret,
        _db: test_db,
    }
}

impl TestApp {
    async fn get(&self, uri: &str, cookie: Option<&str>) -> Response {
        let mut builder = Request::builder()
            .method("GET")
            .uri(uri)
            .header("host", HOST);
        if let Some(value) = cookie {
            builder = builder.header(header::COOKIE, value);
        }
        self.app
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .expect("infallible")
    }
}

async fn body_json(response: Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("json body")
}

fn location(response: &Response) -> String {
    response
        .headers()
        .get(header::LOCATION)
        .expect("Location header")
        .to_str()
        .unwrap()
        .to_string()
}

fn set_cookie_starting_with(response: &Response, prefix: &str) -> Option<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with(prefix))
        .map(str::to_string)
}

/// Parse an URL's query string (form-encoding: '+' is a space; state/nonce
/// are base64url so this is lossless for them).
fn query_params(url: &str) -> HashMap<String, String> {
    let query = url.split_once('?').map(|(_, q)| q).unwrap_or("");
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (
                urlencoding::decode(&key.replace('+', " "))
                    .unwrap()
                    .into_owned(),
                urlencoding::decode(&value.replace('+', " "))
                    .unwrap()
                    .into_owned(),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Fake provider
// ---------------------------------------------------------------------------

struct Provider {
    server: MockServer,
    signing_key: CoreRsaPrivateSigningKey,
}

impl Provider {
    /// Discovery document + JWKS mounted; token endpoint mounted per test
    /// once the nonce is known.
    async fn start() -> Provider {
        let server = MockServer::start().await;
        let signing_key = CoreRsaPrivateSigningKey::from_pem(
            RSA_TEST_PEM,
            Some(JsonWebKeyId::new("test-key".to_string())),
        )
        .expect("test RSA key parses");
        let issuer = server.uri();
        let discovery = json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/authorize"),
            "token_endpoint": format!("{issuer}/token"),
            "jwks_uri": format!("{issuer}/jwks"),
            "response_types_supported": ["code"],
            "subject_types_supported": ["public"],
            "id_token_signing_alg_values_supported": ["RS256"],
            "scopes_supported": ["openid", "email", "profile"],
        });
        Mock::given(method("GET"))
            .and(path("/.well-known/openid-configuration"))
            .respond_with(ResponseTemplate::new(200).set_body_json(discovery))
            .mount(&server)
            .await;
        let jwks = CoreJsonWebKeySet::new(vec![signing_key.as_verification_key()]);
        Mock::given(method("GET"))
            .and(path("/jwks"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::to_value(&jwks).unwrap()),
            )
            .mount(&server)
            .await;
        Provider {
            server,
            signing_key,
        }
    }

    fn uri(&self) -> String {
        self.server.uri()
    }

    /// Sign an ID token; `issuer`/`audience`/`nonce`/expiry are parameters
    /// so each failure mode can bend exactly one of them.
    fn id_token(
        &self,
        issuer: &str,
        audience: &str,
        nonce: &str,
        expires_in_secs: i64,
    ) -> CoreIdToken {
        let now = Utc::now();
        let mut name = LocalizedClaim::new();
        name.insert(None, EndUserName::new("Pat".to_string()));
        let claims = CoreIdTokenClaims::new(
            IssuerUrl::new(issuer.to_string()).unwrap(),
            vec![Audience::new(audience.to_string())],
            now + Duration::seconds(expires_in_secs),
            now - Duration::seconds(10),
            StandardClaims::new(SubjectIdentifier::new("subject-1".to_string())),
            EmptyAdditionalClaims {},
        )
        .set_nonce(Some(Nonce::new(nonce.to_string())))
        .set_email(Some(EndUserEmail::new("pat@example.com".to_string())))
        .set_name(Some(name));
        CoreIdToken::new(
            claims,
            &self.signing_key,
            CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
            None,
            None,
        )
        .expect("id token signs")
    }

    async fn mount_token_endpoint(&self, id_token: &CoreIdToken) {
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "mock-access-token",
                "token_type": "Bearer",
                "expires_in": 3600,
                "id_token": id_token,
            })))
            .mount(&self.server)
            .await;
    }
}

fn oidc_env(provider: &Provider) -> Vec<(String, String)> {
    vec![
        // Trailing slash on purpose: the route must rstrip it before
        // deriving the discovery URL.
        (
            "OIDC_ISSUER_URL".to_string(),
            format!("{}/", provider.uri()),
        ),
        ("OIDC_CLIENT_ID".to_string(), CLIENT_ID.to_string()),
        (
            "OIDC_CLIENT_SECRET".to_string(),
            "test-client-secret".to_string(),
        ),
    ]
}

fn as_str_pairs(pairs: &[(String, String)]) -> Vec<(&str, &str)> {
    pairs
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect()
}

/// Drive the login leg; returns `(state, nonce, state_cookie_pair)`.
async fn login(app: &TestApp, provider: &Provider) -> (String, String, String) {
    let response = app.get("/api/auth/login/oidc", None).await;
    assert_eq!(response.status(), StatusCode::FOUND);
    let auth_url = location(&response);
    assert!(
        auth_url.starts_with(&format!("{}/authorize?", provider.uri())),
        "unexpected authorize URL: {auth_url}"
    );
    let params = query_params(&auth_url);
    assert_eq!(
        params.get("response_type").map(String::as_str),
        Some("code")
    );
    assert_eq!(params.get("client_id").map(String::as_str), Some(CLIENT_ID));
    // Scope exactly "openid email profile".
    assert_eq!(
        params.get("scope").map(String::as_str),
        Some("openid email profile")
    );
    // url_for(_external=True) fallback: plain-HTTP host from the request.
    assert_eq!(
        params.get("redirect_uri").map(String::as_str),
        Some(&*format!("http://{HOST}/api/auth/callback/oidc"))
    );
    let state = params.get("state").expect("state param").clone();
    let nonce = params.get("nonce").expect("nonce param").clone();
    assert!(!state.is_empty() && !nonce.is_empty());

    let set_cookie =
        set_cookie_starting_with(&response, "nightlio_oidc=").expect("state cookie set");
    assert!(set_cookie.contains("HttpOnly"), "{set_cookie}");
    assert!(set_cookie.contains("SameSite=Lax"), "{set_cookie}");
    assert!(set_cookie.contains("Path=/"), "{set_cookie}");
    let cookie_pair = set_cookie.split(';').next().unwrap().to_string();
    (state, nonce, cookie_pair)
}

async fn callback(app: &TestApp, state: &str, cookie_pair: &str) -> Response {
    app.get(
        &format!("/api/auth/callback/oidc?code=mock-code&state={state}"),
        Some(cookie_pair),
    )
    .await
}

fn assert_sso_error(response: &Response, fragment: &str) {
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(location(response), format!("/login#sso_error={fragment}"));
    // No session cookie on failure.
    assert!(set_cookie_starting_with(response, "nightlio_token=").is_none());
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn happy_path_end_to_end() {
    for backend in support::backends() {
        let provider = Provider::start().await;
        let env = oidc_env(&provider);
        let app = make_app_on(backend, &as_str_pairs(&env)).await;

        let (state, nonce, cookie_pair) = login(&app, &provider).await;
        let id_token = provider.id_token(&provider.uri(), CLIENT_ID, &nonce, 3600);
        provider.mount_token_endpoint(&id_token).await;

        let response = callback(&app, &state, &cookie_pair).await;
        assert_eq!(response.status(), StatusCode::FOUND);
        let target = location(&response);
        // FRONTEND_URL unset → same-origin redirect, token in the fragment.
        let token = target
            .strip_prefix("/login#sso_token=")
            .unwrap_or_else(|| panic!("unexpected redirect target: {target}"));
        let claims = jwt::verify_token(&app.jwt_secret, token).expect("app JWT verifies");
        assert_eq!(claims.exp - claims.iat, 3600);

        // The same token is also the httpOnly session cookie.
        let session_cookie =
            set_cookie_starting_with(&response, "nightlio_token=").expect("session cookie");
        assert!(session_cookie.starts_with(&format!("nightlio_token={token};")));
        assert!(session_cookie.contains("HttpOnly"), "{session_cookie}");
        assert!(session_cookie.contains("Max-Age=3600"), "{session_cookie}");
        // Handshake state is single-use: the state cookie is cleared.
        let cleared =
            set_cookie_starting_with(&response, "nightlio_oidc=").expect("state cookie cleared");
        assert!(cleared.contains("Max-Age=0"), "{cleared}");

        // The upserted user is live: verify resolves it with the new token.
        let verify = app
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/verify")
                    .header("host", HOST)
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(verify.status(), StatusCode::OK);
        let body = body_json(verify).await;
        assert_eq!(body["user"]["email"], json!("pat@example.com"));
        assert_eq!(body["user"]["name"], json!("Pat"));
    }
}

#[tokio::test]
async fn state_mismatch_redirects_with_callback_failed() {
    for backend in support::backends() {
        let provider = Provider::start().await;
        let env = oidc_env(&provider);
        let app = make_app_on(backend, &as_str_pairs(&env)).await;

        let (_state, _nonce, cookie_pair) = login(&app, &provider).await;
        // Token endpoint is never reached; no mock needed.
        let response = callback(&app, "attacker-forged-state", &cookie_pair).await;
        assert_sso_error(&response, "callback_failed");
    }
}

#[tokio::test]
async fn missing_state_cookie_redirects_with_callback_failed() {
    for backend in support::backends() {
        let provider = Provider::start().await;
        let env = oidc_env(&provider);
        let app = make_app_on(backend, &as_str_pairs(&env)).await;

        let (state, _nonce, _cookie_pair) = login(&app, &provider).await;
        let response = app
            .get(
                &format!("/api/auth/callback/oidc?code=mock-code&state={state}"),
                None,
            )
            .await;
        assert_sso_error(&response, "callback_failed");
    }
}

#[tokio::test]
async fn nonce_mismatch_redirects_with_callback_failed() {
    for backend in support::backends() {
        let provider = Provider::start().await;
        let env = oidc_env(&provider);
        let app = make_app_on(backend, &as_str_pairs(&env)).await;

        let (state, _nonce, cookie_pair) = login(&app, &provider).await;
        let id_token = provider.id_token(&provider.uri(), CLIENT_ID, "some-other-nonce", 3600);
        provider.mount_token_endpoint(&id_token).await;

        let response = callback(&app, &state, &cookie_pair).await;
        assert_sso_error(&response, "callback_failed");
    }
}

#[tokio::test]
async fn expired_id_token_redirects_with_callback_failed() {
    for backend in support::backends() {
        let provider = Provider::start().await;
        let env = oidc_env(&provider);
        let app = make_app_on(backend, &as_str_pairs(&env)).await;

        let (state, nonce, cookie_pair) = login(&app, &provider).await;
        // exp one hour in the past.
        let id_token = provider.id_token(&provider.uri(), CLIENT_ID, &nonce, -3600);
        provider.mount_token_endpoint(&id_token).await;

        let response = callback(&app, &state, &cookie_pair).await;
        assert_sso_error(&response, "callback_failed");
    }
}

#[tokio::test]
async fn wrong_audience_redirects_with_callback_failed() {
    for backend in support::backends() {
        let provider = Provider::start().await;
        let env = oidc_env(&provider);
        let app = make_app_on(backend, &as_str_pairs(&env)).await;

        let (state, nonce, cookie_pair) = login(&app, &provider).await;
        let id_token = provider.id_token(&provider.uri(), "another-client", &nonce, 3600);
        provider.mount_token_endpoint(&id_token).await;

        let response = callback(&app, &state, &cookie_pair).await;
        assert_sso_error(&response, "callback_failed");
    }
}

#[tokio::test]
async fn wrong_issuer_redirects_with_callback_failed() {
    for backend in support::backends() {
        let provider = Provider::start().await;
        let env = oidc_env(&provider);
        let app = make_app_on(backend, &as_str_pairs(&env)).await;

        let (state, nonce, cookie_pair) = login(&app, &provider).await;
        let id_token = provider.id_token("https://evil.example", CLIENT_ID, &nonce, 3600);
        provider.mount_token_endpoint(&id_token).await;

        let response = callback(&app, &state, &cookie_pair).await;
        assert_sso_error(&response, "callback_failed");
    }
}

#[tokio::test]
async fn off_host_frontend_url_falls_back_to_same_origin() {
    for backend in support::backends() {
        let provider = Provider::start().await;
        let mut env = oidc_env(&provider);
        // The redirect would hand the token fragment to this origin — it must
        // be ignored because its host is not the request host.
        env.push((
            "FRONTEND_URL".to_string(),
            "https://evil.example".to_string(),
        ));
        let app = make_app_on(backend, &as_str_pairs(&env)).await;

        let (state, nonce, cookie_pair) = login(&app, &provider).await;
        let id_token = provider.id_token(&provider.uri(), CLIENT_ID, &nonce, 3600);
        provider.mount_token_endpoint(&id_token).await;

        let response = callback(&app, &state, &cookie_pair).await;
        assert_eq!(response.status(), StatusCode::FOUND);
        let target = location(&response);
        assert!(
            target.starts_with("/login#sso_token="),
            "token fragment leaked off-host: {target}"
        );
    }
}

#[tokio::test]
async fn same_origin_frontend_url_is_honored() {
    for backend in support::backends() {
        let provider = Provider::start().await;
        let mut env = oidc_env(&provider);
        env.push(("FRONTEND_URL".to_string(), format!("http://{HOST}")));
        let app = make_app_on(backend, &as_str_pairs(&env)).await;

        let (state, nonce, cookie_pair) = login(&app, &provider).await;
        let id_token = provider.id_token(&provider.uri(), CLIENT_ID, &nonce, 3600);
        provider.mount_token_endpoint(&id_token).await;

        let response = callback(&app, &state, &cookie_pair).await;
        assert_eq!(response.status(), StatusCode::FOUND);
        let target = location(&response);
        assert!(
            target.starts_with(&format!("http://{HOST}/login#sso_token=")),
            "same-origin FRONTEND_URL not honored: {target}"
        );
    }
}

#[tokio::test]
async fn routes_absent_when_oidc_unconfigured() {
    for backend in support::backends() {
        let app = make_app_on(backend, &[]).await;
        for path in ["/api/auth/login/oidc", "/api/auth/callback/oidc"] {
            let response = app.get(path, None).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            let body = body_json(response).await;
            assert_eq!(body, json!({ "error": "Resource not found" }), "{path}");
        }
    }
}

#[tokio::test]
async fn issuer_without_client_id_is_not_configured() {
    for backend in support::backends() {
        // Blueprint mounts (issuer set) but `_get_oidc_settings` returns None.
        let app = make_app_on(backend, &[("OIDC_ISSUER_URL", "http://127.0.0.1:9/")]).await;
        for path in ["/api/auth/login/oidc", "/api/auth/callback/oidc"] {
            let response = app.get(path, None).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            let body = body_json(response).await;
            assert_eq!(body, json!({ "error": "OIDC is not configured" }), "{path}");
        }
    }
}
