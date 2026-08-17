//! Auth route family — port of `api/routes/auth_routes.py`
//! (`create_auth_routes`) plus the service-layer logic it calls in
//! `api/services/user_service.py`.
//!
//! Rules (all POST-only, registered without trailing slashes — the slashed
//! variants 404 via the shell's JSON fallback):
//! - `POST /auth/verify` — behind `require_auth` ([`AuthUser`]); returns
//!   the caller's user payload.
//! - `POST /auth/logout` — deliberately NOT behind auth; idempotent cookie
//!   clear.
//! - `POST /auth/local/login` — rate limited 30/min; credentialed branch
//!   when the JSON body carries `username` or `password`, credential-free
//!   self-host branch otherwise (only while OIDC is unconfigured).
//! - `POST /auth/local/register` — behind `require_auth` (family-account
//!   semantics), then rate limited 10/min; creates an additional local
//!   user, returns NO token/cookie.
//!
//! Body parsing mirrors `request.get_json(silent=True) or {}`: a missing
//! body, missing/non-JSON Content-Type, or unparseable JSON all become the
//! empty object (the credential-free login path depends on this — the e2e
//! harness posts with no body at all). A parseable but non-object truthy
//! JSON body reproduces the Python `AttributeError` → catch-all 500.

use std::net::SocketAddr;

use std::convert::Infallible;

use axum::body::Bytes;
use axum::extract::{ConnectInfo, FromRequestParts, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Map, Value, json};
use tokio::task::spawn_blocking;

use super::{ForwardedInfo, automatic_options};
use crate::auth::extract::AuthUser;
use crate::auth::rate_limit::{
    self, AUTH_WINDOW_SECONDS, LOGIN_BUCKET_PREFIX, LOGIN_MAX_REQUESTS, RATE_LIMIT_MESSAGE,
    REGISTER_BUCKET_PREFIX, REGISTER_MAX_REQUESTS,
};
use crate::auth::{cookie, jwt, password};
use crate::config::{AppEnv, Config};
use crate::db::users::UserRow;
use crate::db::{DatabaseError, activity, groups, users};
use crate::state::AppState;

/// `MIN_PASSWORD_LENGTH` (`api/routes/auth_routes.py`).
const MIN_PASSWORD_LENGTH: usize = 8;

/// `Allow` for these POST-only rules (fixture `verify_options-204.json`).
const POST_ALLOW: &str = "POST, OPTIONS";

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/auth/verify",
            post(verify_token).merge(automatic_options(POST_ALLOW)),
        )
        .route(
            "/auth/logout",
            post(logout).merge(automatic_options(POST_ALLOW)),
        )
        .route(
            "/auth/local/login",
            post(local_login).merge(automatic_options(POST_ALLOW)),
        )
        .route(
            "/auth/local/register",
            post(local_register).merge(automatic_options(POST_ALLOW)),
        )
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

/// The TCP peer address (`request.remote_addr`), when the server was
/// started with connect-info; `None` under `tower::ServiceExt::oneshot`.
/// Infallible — absence must never fail the request (the limiter then
/// buckets under `"unknown"`, like Flask's `remote_addr or "unknown"`).
struct PeerAddr(Option<SocketAddr>);

impl<S: Send + Sync> FromRequestParts<S> for PeerAddr {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(PeerAddr(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|ConnectInfo(addr)| *addr),
        ))
    }
}

fn error_json(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}

/// The route's catch-all 500 for login (`Authentication failed`).
fn auth_failed() -> Response {
    error_json(StatusCode::INTERNAL_SERVER_ERROR, "Authentication failed")
}

/// The route's catch-all 500 for register (`Registration failed`).
fn register_failed() -> Response {
    error_json(StatusCode::INTERNAL_SERVER_ERROR, "Registration failed")
}

/// `_user_payload`: the four public user fields, nothing more.
fn user_payload(user: &UserRow) -> Value {
    json!({
        "id": user.id,
        "name": user.name,
        "email": user.email,
        "avatar_url": user.avatar_url,
    })
}

/// `_is_https_request` inputs for this server: the Rust binary terminates
/// plain HTTP only, so on a direct connection `is_secure` is always false
/// and HTTPS can only be asserted by a trusted proxy's `X-Forwarded-Proto`.
///
/// ProxyFix parity: with `TRUST_PROXY_HEADERS` set, Flask runs behind
/// werkzeug's `ProxyFix(x_proto=1)` (api/app.py), which rewrites
/// `request.is_secure` from the RIGHTMOST `X-Forwarded-Proto` value (the
/// trusted proxy's own, case-sensitively `https`), and `_is_https_request`
/// consults that BEFORE its first-comma-value fallback. So `http,https`
/// still yields Secure. [`ForwardedInfo::from_headers`] computes that same
/// rightmost value here.
fn request_is_secure(config: &Config, headers: &HeaderMap) -> bool {
    let is_secure = config.trust_proxy_headers
        && ForwardedInfo::from_headers(headers).scheme.as_deref() == Some("https");
    let x_forwarded_proto = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok());
    cookie::is_https_request(is_secure, config.trust_proxy_headers, x_forwarded_proto)
}

fn with_set_cookie(mut response: Response, cookie_string: &str) -> Response {
    if let Ok(value) = HeaderValue::from_str(cookie_string) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

/// Flask `request.get_json(silent=True)`: parse only when the declared
/// mimetype is JSON (`application/json` or `application/*+json`); any
/// parse/mimetype problem yields `None` instead of an error.
fn get_json_silent(headers: &HeaderMap, body: &[u8]) -> Option<Value> {
    let mimetype = headers
        .get(header::CONTENT_TYPE)?
        .to_str()
        .ok()?
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let is_json = mimetype == "application/json"
        || (mimetype.starts_with("application/") && mimetype.ends_with("+json"));
    if !is_json {
        return None;
    }
    serde_json::from_slice(body).ok()
}

/// Python truthiness for a JSON value (`bool(x)`).
fn python_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

/// `data = request.get_json(silent=True) or {}` followed by `data.get(...)`
/// attribute access: falsy/missing bodies become the empty object; a truthy
/// non-object body raises (Python `AttributeError`) → `Err` for the route's
/// catch-all 500.
fn json_object_or_empty(parsed: Option<Value>) -> Result<Map<String, Value>, ()> {
    match parsed {
        Some(Value::Object(map)) => Ok(map),
        Some(other) if python_truthy(&other) => Err(()),
        _ => Ok(Map::new()),
    }
}

/// `str(value)` as the login path applies it to non-string credentials.
fn python_str(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Number(number) => number.to_string(),
        other => other.to_string(),
    }
}

/// `@rate_limit(...)` — runs after `require_auth` (the [`AuthUser`]
/// extractor precedes handler bodies) and before any body validation.
/// `TESTING` bypasses entirely; storage failures fail OPEN inside
/// [`rate_limit::allow_request`].
async fn enforce_rate_limit(
    state: &AppState,
    headers: &HeaderMap,
    remote_addr: Option<SocketAddr>,
    bucket_prefix: &'static str,
    max_requests: i64,
) -> Result<(), Response> {
    if state.config.app_env == AppEnv::Testing {
        return Ok(());
    }
    let ip = rate_limit::client_ip(
        state.config.trust_proxy_headers,
        headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok()),
        remote_addr.map(|addr| addr.ip().to_string()).as_deref(),
    );
    let key = rate_limit::bucket_key(bucket_prefix, &ip);
    let db_path = state.config.rate_limit_db_path.clone();
    let allowed = spawn_blocking(move || {
        rate_limit::allow_request(&db_path, &key, max_requests, AUTH_WINDOW_SECONDS)
    })
    .await
    // A panicked/cancelled blocking task is a limiter failure → fail open.
    .unwrap_or(true);
    if allowed {
        Ok(())
    } else {
        Err(error_json(
            StatusCode::TOO_MANY_REQUESTS,
            RATE_LIMIT_MESSAGE,
        ))
    }
}

/// `_issue_login_response`: JSON `{token, user}` plus the session cookie.
/// The cookie's `Max-Age` mirrors the JWT expiry (3600 s); `Secure` is
/// computed per request.
fn issue_login_response(
    state: &AppState,
    headers: &HeaderMap,
    status: StatusCode,
    user: &UserRow,
) -> Response {
    let Ok(token) = jwt::issue_token(&state.config.jwt_secret, user.id) else {
        return auth_failed();
    };
    let secure = request_is_secure(&state.config, headers);
    let session_cookie = cookie::auth_cookie(&token, secure);
    let response = (
        status,
        Json(json!({ "token": token, "user": user_payload(user) })),
    )
        .into_response();
    with_set_cookie(response, &session_cookie.to_string())
}

// ---------------------------------------------------------------------------
// POST /auth/verify
// ---------------------------------------------------------------------------

/// Verify the caller's session (Bearer or cookie) and return user info.
/// Routed through the [`AuthUser`] extractor so a cookie-only session can
/// be verified too (the SPA's reload path).
async fn verify_token(State(state): State<AppState>, user: AuthUser) -> Response {
    let pool = state.pool.clone();
    let user_id = user.user_id;
    let looked_up = spawn_blocking(move || -> Result<Option<UserRow>, DatabaseError> {
        let conn = pool.get().map_err(DatabaseError::from)?;
        users::get_user_by_id(&conn, user_id)
    })
    .await;
    match looked_up {
        Ok(Ok(Some(row))) => Json(json!({ "user": user_payload(&row) })).into_response(),
        Ok(Ok(None)) => error_json(StatusCode::NOT_FOUND, "User not found"),
        Ok(Err(exc)) => {
            tracing::error!(error = %exc, "Token verification error");
            error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Token verification failed",
            )
        }
        Err(exc) => {
            tracing::error!(error = %exc, "Token verification error");
            error_json(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Token verification failed",
            )
        }
    }
}

// ---------------------------------------------------------------------------
// POST /auth/logout
// ---------------------------------------------------------------------------

/// Clear the session cookie. Deliberately not behind auth (a client must
/// be able to log out with an expired/invalid token) and idempotent.
async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let secure = request_is_secure(&state.config, &headers);
    let clearing = cookie::clear_auth_cookie(secure);
    let response = Json(json!({ "status": "success" })).into_response();
    with_set_cookie(response, &clearing.to_string())
}

// ---------------------------------------------------------------------------
// POST /auth/local/login
// ---------------------------------------------------------------------------

async fn local_login(
    State(state): State<AppState>,
    PeerAddr(remote_addr): PeerAddr,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Decorator order: @rate_limit wraps the view, so the limiter runs
    // before everything else (including the DISABLE_LOCAL_LOGIN refusal).
    if let Err(response) = enforce_rate_limit(
        &state,
        &headers,
        remote_addr,
        LOGIN_BUCKET_PREFIX,
        LOGIN_MAX_REQUESTS,
    )
    .await
    {
        return response;
    }

    // SSO-only deployments have exactly one door: both the credentialed and
    // the credential-free branch are refused.
    if state.config.disable_local_login {
        return error_json(StatusCode::FORBIDDEN, "Local login is disabled");
    }

    let Ok(data) = json_object_or_empty(get_json_silent(&headers, &body)) else {
        return auth_failed();
    };
    // `data.get("username")` — a JSON null value behaves exactly like an
    // absent key (Python `dict.get` returns None for both).
    let username = data.get("username").filter(|value| !value.is_null());
    let password = data.get("password").filter(|value| !value.is_null());

    if username.is_some() || password.is_some() {
        // Credentialed branch: triggered by mere key presence; a falsy or
        // missing partner then 400s (values are only truthiness-checked).
        let (Some(username), Some(password)) = (
            username.filter(|value| python_truthy(value)),
            password.filter(|value| python_truthy(value)),
        ) else {
            return error_json(
                StatusCode::BAD_REQUEST,
                "Username and password are required",
            );
        };
        credentialed_login(&state, &headers, python_str(username), python_str(password)).await
    } else {
        selfhost_login(&state, &headers).await
    }
}

/// The credentialed login path: verify against the stored hash (argon2id
/// or legacy Werkzeug), upgrading legacy hashes to argon2id on success.
async fn credentialed_login(
    state: &AppState,
    headers: &HeaderMap,
    username: String,
    password: String,
) -> Response {
    type Lookup = Result<Option<(UserRow, Option<String>)>, DatabaseError>;
    let pool = state.pool.clone();
    let looked_up = spawn_blocking(move || -> Lookup {
        let conn = pool.get().map_err(DatabaseError::from)?;
        let Some(user) = users::get_user_by_provider(&conn, "local", &username)? else {
            return Ok(None);
        };
        let stored_hash = users::get_user_password_hash(&conn, user.id)?;
        Ok(Some((user, stored_hash)))
    })
    .await;
    let user_and_hash = match looked_up {
        Ok(Ok(found)) => found,
        _ => return auth_failed(),
    };
    // Unknown username, user with no stored hash, and wrong password are
    // deliberately indistinguishable (same 401 body, no Set-Cookie).
    let (user, stored_hash) = match user_and_hash {
        Some((user, Some(hash))) if !hash.is_empty() => (user, hash),
        _ => return error_json(StatusCode::UNAUTHORIZED, "Invalid credentials"),
    };

    // scrypt/pbkdf2/argon2 verification is CPU-bound: blocking pool, never
    // the async executor. The legacy-hash upgrade (rehash-on-login) runs in
    // the same blocking context — argon2id hashing is just as CPU-bound,
    // and it must only ever happen right after the plaintext verified.
    let pool = state.pool.clone();
    let user_id = user.id;
    let verified = spawn_blocking(move || {
        let verdict = password::check_password_hash(&stored_hash, &password)?;
        if verdict && password::needs_rehash(&stored_hash) {
            // Transparent upgrade of legacy Werkzeug rows to argon2id (the
            // Flask rollback constraint is retired — see auth::password).
            // Best-effort: a rehash/persist failure logs a warning and
            // never fails the login; the legacy hash simply stays put
            // until the next successful login.
            let new_hash = password::generate_password_hash(&password);
            let persisted = pool
                .get()
                .map_err(DatabaseError::from)
                .and_then(|conn| users::set_user_password(&conn, user_id, &new_hash));
            if let Err(exc) = persisted {
                tracing::warn!(error = %exc, user_id, "Password rehash failed; keeping legacy hash");
            }
        }
        Ok::<bool, password::PasswordHashError>(verdict)
    })
    .await;
    match verified {
        Ok(Ok(true)) => {}
        Ok(Ok(false)) => return error_json(StatusCode::UNAUTHORIZED, "Invalid credentials"),
        // An unparseable stored hash is a data problem (Python: uncaught
        // ValueError from werkzeug → the catch-all 500), not a wrong
        // password.
        _ => return auth_failed(),
    }

    let pool = state.pool.clone();
    let user_id = user.id;
    let updated = spawn_blocking(move || -> Result<(), DatabaseError> {
        let conn = pool.get().map_err(DatabaseError::from)?;
        users::update_user_last_login(&conn, user_id)?;
        // record_login_activity is best-effort: never breaks the login.
        let _ = activity::add_activity(&conn, user_id, "login", Some(&json!({"method": "local"})));
        Ok(())
    })
    .await;
    if !matches!(updated, Ok(Ok(()))) {
        return auth_failed();
    }

    issue_login_response(state, headers, StatusCode::OK, &user)
}

/// The credential-free self-host branch: valid only while OIDC is
/// unconfigured — anonymous token issuance fails closed once an identity
/// provider exists.
async fn selfhost_login(state: &AppState, headers: &HeaderMap) -> Response {
    if state.config.oidc_enabled() {
        return error_json(StatusCode::FORBIDDEN, "Credentials required");
    }

    let default_user_id = state.config.default_self_host_id.clone();
    // Config already applies `SELFHOST_USER_NAME or "Me"`; email falls back
    // to `<default_user_id>@localhost` exactly like the Python route.
    let name = state.config.selfhost_user_name.clone();
    let email = state
        .config
        .selfhost_user_email
        .clone()
        .unwrap_or_else(|| format!("{default_user_id}@localhost"));

    let pool = state.pool.clone();
    let upserted = spawn_blocking(move || -> Result<Option<UserRow>, DatabaseError> {
        let conn = pool.get().map_err(DatabaseError::from)?;
        // ensure_local_user: upsert keyed by the legacy google_id column;
        // idempotent, refreshes the friendly name/email, burns an
        // autoincrement id on every conflicting insert attempt (known,
        // fixture-documented behavior).
        let user = users::upsert_user_by_google_id(
            &conn,
            &default_user_id,
            Some(&email),
            Some(&name),
            None,
        )?;
        if let Some(user) = &user {
            let _ = activity::add_activity(
                &conn,
                user.id,
                "login",
                Some(&json!({"method": "selfhost"})),
            );
        }
        Ok(user)
    })
    .await;

    match upserted {
        Ok(Ok(Some(user))) => issue_login_response(state, headers, StatusCode::OK, &user),
        _ => auth_failed(),
    }
}

// ---------------------------------------------------------------------------
// POST /auth/local/register
// ---------------------------------------------------------------------------

/// `sqlite3.IntegrityError` equivalent: any constraint violation (here the
/// UNIQUE index on `(auth_provider, external_id)`).
fn is_integrity_error(error: &DatabaseError) -> bool {
    matches!(
        error,
        DatabaseError::Sqlite(rusqlite::Error::SqliteFailure(inner, _))
            if inner.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

async fn local_register(
    State(state): State<AppState>,
    // require_auth runs first (extractor order) — before the rate limiter
    // and any body validation. Any authenticated user may create another
    // local account (family-account semantics; the app has no role model).
    _caller: AuthUser,
    PeerAddr(remote_addr): PeerAddr,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(response) = enforce_rate_limit(
        &state,
        &headers,
        remote_addr,
        REGISTER_BUCKET_PREFIX,
        REGISTER_MAX_REQUESTS,
    )
    .await
    {
        return response;
    }

    let Ok(data) = json_object_or_empty(get_json_silent(&headers, &body)) else {
        return register_failed();
    };

    // Validation order is user-visible: username, then password presence,
    // then password length.
    let username = match data.get("username").and_then(Value::as_str) {
        // `not username or not isinstance(username, str) or not
        // username.strip()` collapse to this one 400.
        Some(raw) if !raw.trim().is_empty() => raw.trim().to_string(),
        _ => return error_json(StatusCode::BAD_REQUEST, "Username is required"),
    };
    let password = match data.get("password").and_then(Value::as_str) {
        // `not password or not isinstance(password, str)`.
        Some(raw) if !raw.is_empty() => raw.to_string(),
        _ => return error_json(StatusCode::BAD_REQUEST, "Password is required"),
    };
    if password.chars().count() < MIN_PASSWORD_LENGTH {
        return error_json(
            StatusCode::BAD_REQUEST,
            "Password must be at least 8 characters",
        );
    }
    let email = data
        .get("email")
        .and_then(Value::as_str)
        .map(str::to_string);
    let name = data.get("name").and_then(Value::as_str).map(str::to_string);

    // Ok(user) → 201; Err(()) → 409 (username taken).
    type Registered = Result<Result<UserRow, ()>, DatabaseError>;
    let pool = state.pool.clone();
    let registered = spawn_blocking(move || -> Registered {
        // argon2id hash (new-hash format since the Flask rollback
        // constraint was retired): CPU-bound, so it stays on the blocking
        // pool with the DB work.
        let password_hash = password::generate_password_hash(&password);
        let conn = pool.get().map_err(DatabaseError::from)?;
        let user_id = match users::create_local_user(
            &conn,
            &username,
            &password_hash,
            email.as_deref(),
            name.as_deref(),
        ) {
            Ok(id) => id,
            Err(exc) if is_integrity_error(&exc) => return Ok(Err(())),
            Err(exc) => return Err(exc),
        };
        groups::ensure_default_groups_for_user(&conn, user_id)
            .map_err(|exc| DatabaseError::Message(exc.to_string()))?;
        // Bug-compatible: register_local_user returning None (for any
        // reason) maps to the 409 branch in the Python route.
        match users::get_user_by_id(&conn, user_id)? {
            Some(user) => Ok(Ok(user)),
            None => Ok(Err(())),
        }
    })
    .await;

    match registered {
        // 201 with the user payload; registering does NOT log the new user
        // in — no token, no Set-Cookie.
        Ok(Ok(Ok(user))) => (
            StatusCode::CREATED,
            Json(json!({ "status": "success", "user": user_payload(&user) })),
        )
            .into_response(),
        Ok(Ok(Err(()))) => error_json(StatusCode::CONFLICT, "Username is already taken"),
        Ok(Err(exc)) => {
            tracing::error!(error = %exc, "Local register error");
            register_failed()
        }
        Err(exc) => {
            tracing::error!(error = %exc, "Local register error");
            register_failed()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case::null(json!(null), false)]
    #[case::false_(json!(false), false)]
    #[case::true_(json!(true), true)]
    #[case::zero(json!(0), false)]
    #[case::zero_float(json!(0.0), false)]
    #[case::one(json!(1), true)]
    #[case::empty_string(json!(""), false)]
    #[case::string(json!("x"), true)]
    #[case::empty_array(json!([]), false)]
    #[case::array(json!([1]), true)]
    #[case::empty_object(json!({}), false)]
    #[case::object(json!({"a": 1}), true)]
    fn truthiness_matches_python(#[case] value: Value, #[case] expected: bool) {
        assert_eq!(python_truthy(&value), expected);
    }

    #[test]
    fn json_object_or_empty_matches_get_json_or_dict() {
        // Missing / falsy bodies → {}.
        assert_eq!(json_object_or_empty(None), Ok(Map::new()));
        for falsy in [json!(null), json!(false), json!(0), json!(""), json!([])] {
            assert_eq!(json_object_or_empty(Some(falsy)), Ok(Map::new()));
        }
        assert_eq!(json_object_or_empty(Some(json!({}))), Ok(Map::new()));
        // A dict passes through.
        let map = json_object_or_empty(Some(json!({"username": "alice"}))).unwrap();
        assert_eq!(map.get("username"), Some(&json!("alice")));
        // Truthy non-dict → AttributeError → catch-all 500.
        for bad in [json!([1]), json!("body"), json!(5), json!(true)] {
            assert_eq!(json_object_or_empty(Some(bad)), Err(()));
        }
    }

    #[test]
    fn get_json_silent_requires_a_json_mimetype() {
        let payload = br#"{"username": "alice"}"#;
        let mut headers = HeaderMap::new();
        // No Content-Type at all → None (the no-body e2e login path).
        assert_eq!(get_json_silent(&headers, payload), None);
        // Non-JSON mimetype → None even though the body parses.
        headers.insert(header::CONTENT_TYPE, "text/plain".parse().unwrap());
        assert_eq!(get_json_silent(&headers, payload), None);
        // JSON mimetype (parameters/casing tolerated) → parsed.
        for value in [
            "application/json",
            "application/json; charset=utf-8",
            "Application/JSON",
            "application/vnd.api+json",
        ] {
            headers.insert(header::CONTENT_TYPE, value.parse().unwrap());
            assert_eq!(
                get_json_silent(&headers, payload),
                Some(json!({"username": "alice"})),
                "{value}"
            );
        }
        // Unparseable body → None (silent).
        headers.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
        assert_eq!(get_json_silent(&headers, b"{not json"), None);
        assert_eq!(get_json_silent(&headers, b""), None);
    }

    #[test]
    fn python_str_renders_scalars_like_python() {
        assert_eq!(python_str(&json!("alice")), "alice");
        assert_eq!(python_str(&json!(123)), "123");
        assert_eq!(python_str(&json!(true)), "True");
        assert_eq!(python_str(&json!(false)), "False");
    }

    /// A `Config` whose only significant knob is `TRUST_PROXY_HEADERS`.
    fn proxy_config(trust: bool) -> Config {
        Config::from_lookup(&|key| match key {
            "TRUST_PROXY_HEADERS" if trust => Some("1".to_string()),
            _ => None,
        })
    }

    #[rstest]
    // No proxy trust: the header is client-spoofable and ignored.
    #[case::untrusted_spoof(false, Some("https"), false)]
    #[case::untrusted_none(false, None, false)]
    // Trusted proxy, single value.
    #[case::trusted_https(true, Some("https"), true)]
    #[case::trusted_http(true, Some("http"), false)]
    #[case::trusted_none(true, None, false)]
    // Regression (parity finding): ProxyFix(x_proto=1) rewrites
    // request.is_secure from the RIGHTMOST value, so an appending proxy
    // chain sending `http,https` must still yield Secure, as Flask does.
    #[case::trusted_rightmost_https(true, Some("http,https"), true)]
    // Flask's first-comma-value fallback in _is_https_request still
    // applies when the rightmost value is not `https`.
    #[case::trusted_first_https(true, Some("https,http"), true)]
    #[case::trusted_neither(true, Some("http,http"), false)]
    // werkzeug compares the ProxyFix-rewritten scheme case-sensitively
    // (only the first-value fallback lowercases), so `http, HTTPS` stays
    // non-Secure in both implementations.
    #[case::trusted_rightmost_uppercase(true, Some("http, HTTPS"), false)]
    fn request_is_secure_matches_flask_proxyfix(
        #[case] trust: bool,
        #[case] xfp: Option<&str>,
        #[case] expected: bool,
    ) {
        let mut headers = HeaderMap::new();
        if let Some(value) = xfp {
            headers.insert("x-forwarded-proto", value.parse().unwrap());
        }
        assert_eq!(request_is_secure(&proxy_config(trust), &headers), expected);
    }

    #[test]
    fn request_is_secure_joins_repeated_header_lines() {
        // Two header lines behave like one comma-joined header (WSGI
        // semantics): the rightmost value of the last line wins.
        let mut headers = HeaderMap::new();
        headers.append("x-forwarded-proto", "http".parse().unwrap());
        headers.append("x-forwarded-proto", "https".parse().unwrap());
        assert!(request_is_secure(&proxy_config(true), &headers));
    }
}
