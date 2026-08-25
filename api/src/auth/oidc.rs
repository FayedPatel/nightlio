//! OIDC single sign-on — port of `api/auth/oauth.py` (`oauth_bp`).
//!
//! Authlib did the protocol work invisibly; here every step is explicit and
//! must stay equivalent:
//! - discovery from `<issuer rstrip '/'>/.well-known/openid-configuration`,
//! - scope exactly `openid email profile`,
//! - `state` + `nonce` generated at login and carried in a signed cookie
//!   keyed on `SECRET_KEY` (the equivalent of Flask's itsdangerous-signed
//!   session — `SECRET_KEY` is used for nothing else in this app),
//! - callback validates state, nonce, the ID-token signature via JWKS,
//!   issuer, audience, and expiry (plus `at_hash` when present, as Authlib
//!   does),
//! - claims consumed: `sub` / `email` / `name`-or-`preferred_username` /
//!   `picture`,
//! - ANY validation failure redirects with `#sso_error=callback_failed`;
//!   provisioning/JWT failures redirect with `#sso_error=auth_failed`.
//!
//! The router is mounted by `routes::build_router` ONLY when
//! `Config::oidc_enabled()` — mirroring the conditional blueprint
//! registration in `create_app()`; when absent the paths fall through to
//! the standard JSON 404. Never log tokens, authorization codes, or
//! identity-provider responses here.

use std::collections::HashMap;

use axum::Router;
use axum::body::Body;
use axum::extract::{Extension, Query, State};
use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use cookie::time::{Duration, OffsetDateTime};
use cookie::{Cookie, SameSite};
use hmac::{Hmac, KeyInit, Mac};
use openidconnect::core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata};
use openidconnect::{
    AccessTokenHash, AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce,
    OAuth2TokenResponse, RedirectUrl, Scope, TokenResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;
use subtle::ConstantTimeEq;

use crate::auth::{cookie as auth_cookie, jwt};
use crate::config::Config;
use crate::db::store;
use crate::routes::{ForwardedInfo, automatic_options};
use crate::state::AppState;

/// The signed state/nonce cookie. Deliberately NOT named `session`: a stale
/// Flask session cookie surviving cutover must not collide with (or be
/// misread as) this cookie — an in-flight OIDC login across cutover simply
/// restarts.
pub const STATE_COOKIE_NAME: &str = "nightlio_oidc";

/// State cookie lifetime, seconds. The Flask session cookie was
/// browser-session-scoped; bounding the login handshake to ten minutes is
/// strictly tighter and internal to this server (both ends are ours).
const STATE_COOKIE_MAX_AGE_SECS: i64 = 600;

/// `Allow` for these GET-only rules (Werkzeug sorts methods).
const GET_ALLOW: &str = "HEAD, GET, OPTIONS";

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/auth/login/oidc",
            get(login_oidc).merge(automatic_options(GET_ALLOW)),
        )
        .route(
            "/auth/callback/oidc",
            get(oidc_callback).merge(automatic_options(GET_ALLOW)),
        )
}

// ---------------------------------------------------------------------------
// Settings (`_get_oidc_settings`)
// ---------------------------------------------------------------------------

struct OidcSettings {
    /// `OIDC_ISSUER_URL.rstrip("/")`.
    issuer: String,
    client_id: String,
    /// Authlib tolerates a missing secret (public client); empty string here.
    client_secret: String,
    /// `OIDC_CALLBACK_URL`; `None` falls back to
    /// `url_for("oauth.oidc_callback", _external=True)`.
    callback_url: Option<String>,
}

/// Returns `None` when OIDC is unconfigured (no issuer or no client id) —
/// the handlers then answer 404 `{"error": "OIDC is not configured"}`,
/// exactly like the Flask routes (reachable when the issuer is set but the
/// client id is not: the blueprint mounts, the settings check still fails).
fn oidc_settings(config: &Config) -> Option<OidcSettings> {
    if !config.oidc_enabled() {
        return None;
    }
    let issuer = config.oidc_issuer_url.as_deref()?;
    let client_id = config.oidc_client_id.as_deref()?;
    Some(OidcSettings {
        issuer: issuer.trim_end_matches('/').to_string(),
        client_id: client_id.to_string(),
        client_secret: config.oidc_client_secret.clone().unwrap_or_default(),
        callback_url: config.oidc_callback_url.clone(),
    })
}

fn not_configured() -> Response {
    (
        StatusCode::NOT_FOUND,
        axum::Json(json!({ "error": "OIDC is not configured" })),
    )
        .into_response()
}

/// Flask's app-level 500 (`api/utils/error_handlers.py`) — what an
/// exception escaping `login_oidc` produced.
fn internal_server_error() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        axum::Json(json!({ "error": "Internal server error" })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// `_safe_frontend_base` (verbatim port — gates the token-carrying redirect)
// ---------------------------------------------------------------------------

/// Validate `FRONTEND_URL` before using it as a redirect base. Only two
/// shapes are trusted: a relative path (single leading slash), or an
/// absolute http(s) URL whose host equals this request's host
/// (case-insensitively; `request_host` is the ProxyFix-adjusted host).
/// Backslashes are rejected outright — some browsers treat them as slashes
/// (`/\evil.com`). Anything untrusted becomes `""` (same-origin redirect).
pub fn safe_frontend_base(base: &str, request_host: &str) -> String {
    let base = base.trim_end_matches('/');
    if base.is_empty() {
        return String::new();
    }
    if !base.contains('\\') {
        if base.starts_with('/') && !base.starts_with("//") {
            return base.to_string();
        }
        if let Some((scheme, rest)) = base.split_once("://") {
            // urlsplit lowercases the scheme; netloc runs to the first
            // path/query/fragment delimiter.
            let scheme = scheme.to_ascii_lowercase();
            let netloc = rest.split(['/', '?', '#']).next().unwrap_or("");
            if (scheme == "http" || scheme == "https")
                && !netloc.is_empty()
                && netloc.eq_ignore_ascii_case(request_host)
            {
                return base.to_string();
            }
        }
    }
    tracing::warn!("Ignoring FRONTEND_URL: not a relative path or same-origin URL");
    String::new()
}

// ---------------------------------------------------------------------------
// Werkzeug redirect responses
// ---------------------------------------------------------------------------

/// markupsafe/`html.escape` as werkzeug applies it to the location.
fn html_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&#34;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// `werkzeug.utils.redirect(location)`: 302, `text/html; charset=utf-8`,
/// the exact interstitial body (verified against the Flask venv).
fn flask_redirect(location: &str) -> Response {
    let escaped = html_escape(location);
    let body = format!(
        "<!doctype html>\n<html lang=en>\n<title>Redirecting...</title>\n\
         <h1>Redirecting...</h1>\n<p>You should be redirected automatically \
         to the target URL: <a href=\"{escaped}\">{escaped}</a>. If not, \
         click the link.\n"
    );
    let mut builder = Response::builder()
        .status(StatusCode::FOUND)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8");
    // The URLs built here (IdP authorize URL, `<base>/login#<fragment>`)
    // are ASCII; a location that somehow is not simply omits the header.
    if let Ok(value) = header::HeaderValue::from_str(location) {
        builder = builder.header(header::LOCATION, value);
    }
    builder
        .body(Body::from(body))
        .unwrap_or_else(|_| internal_server_error())
}

/// `_frontend_redirect(fragment)`: back to the SPA login page, token or
/// error flag in the URL fragment (fragments never reach servers or logs).
fn frontend_redirect(config: &Config, request_host: &str, fragment: &str) -> Response {
    let base = safe_frontend_base(
        config.frontend_url.as_deref().unwrap_or("").trim(),
        request_host,
    );
    flask_redirect(&format!("{base}/login#{fragment}"))
}

// ---------------------------------------------------------------------------
// Signed state cookie (itsdangerous-session equivalent, keyed on SECRET_KEY)
// ---------------------------------------------------------------------------

/// What Authlib stored in the Flask session between login and callback.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StatePayload {
    state: String,
    nonce: String,
    /// The exact redirect_uri sent in the authorization request — the token
    /// exchange must repeat it byte-for-byte (Authlib stores it too).
    redirect_uri: String,
}

type HmacSha256 = Hmac<Sha256>;

fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

fn mac_bytes(secret: &str, payload: &[u8]) -> Vec<u8> {
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key length");
    mac.update(payload);
    mac.finalize().into_bytes().to_vec()
}

/// `hex(json_payload) + "." + hex(hmac_sha256(SECRET_KEY, json_payload))`.
fn sign_state_payload(secret: &str, payload: &StatePayload) -> String {
    let encoded = serde_json::to_vec(payload).expect("payload serializes");
    format!(
        "{}.{}",
        hex_encode(&encoded),
        hex_encode(&mac_bytes(secret, &encoded))
    )
}

/// Verify + decode; any malformation or MAC mismatch is `None` (→ the
/// generic callback failure, like an itsdangerous `BadSignature`).
fn verify_state_payload(secret: &str, value: &str) -> Option<StatePayload> {
    let (payload_hex, mac_hex) = value.split_once('.')?;
    let payload = hex_decode(payload_hex)?;
    let presented_mac = hex_decode(mac_hex)?;
    let expected_mac = mac_bytes(secret, &payload);
    if !bool::from(expected_mac.as_slice().ct_eq(presented_mac.as_slice())) {
        return None;
    }
    serde_json::from_slice(&payload).ok()
}

fn state_cookie(value: String, secure: bool) -> Cookie<'static> {
    Cookie::build((STATE_COOKIE_NAME, value))
        .max_age(Duration::seconds(STATE_COOKIE_MAX_AGE_SECS))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(secure)
        .build()
}

fn clear_state_cookie(secure: bool) -> Cookie<'static> {
    Cookie::build((STATE_COOKIE_NAME, ""))
        .max_age(Duration::ZERO)
        .expires(OffsetDateTime::UNIX_EPOCH)
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(secure)
        .build()
}

fn append_set_cookie(response: &mut Response, cookie: &Cookie<'_>) {
    if let Ok(value) = header::HeaderValue::from_str(&cookie.to_string()) {
        response.headers_mut().append(header::SET_COOKIE, value);
    }
}

/// First `name` cookie across all `Cookie` request headers.
fn request_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    for header_value in headers.get_all(header::COOKIE) {
        let Ok(raw) = header_value.to_str() else {
            continue;
        };
        for parsed in Cookie::split_parse(raw.to_string()).flatten() {
            if parsed.name() == name {
                return Some(parsed.value().to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Request context (ProxyFix-adjusted, as Flask saw it)
// ---------------------------------------------------------------------------

/// `request.host`: the trusted `X-Forwarded-Host` when ProxyFix consumed
/// one, else the direct `Host` header.
fn effective_host(headers: &HeaderMap, forwarded: Option<&ForwardedInfo>) -> String {
    forwarded
        .and_then(|info| info.host.clone())
        .or_else(|| {
            headers
                .get(header::HOST)
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        })
        .unwrap_or_default()
}

/// `request.scheme` for `url_for(_external=True)`: the Rust binary
/// terminates plain HTTP only, so the scheme is `http` unless a trusted
/// proxy asserted otherwise.
fn effective_scheme(forwarded: Option<&ForwardedInfo>) -> String {
    forwarded
        .and_then(|info| info.scheme.clone())
        .unwrap_or_else(|| "http".to_string())
}

/// `_is_https_request` for the cookie `Secure` flag — same computation as
/// the auth-route family (per request, never static config).
///
/// ProxyFix parity: with `TRUST_PROXY_HEADERS` set, werkzeug's
/// `ProxyFix(x_proto=1)` rewrites `request.is_secure` from the RIGHTMOST
/// `X-Forwarded-Proto` value, which Flask's `_is_https_request` checks
/// before its first-comma-value fallback — so `http,https` still yields
/// Secure.
fn request_is_secure(config: &Config, headers: &HeaderMap) -> bool {
    let is_secure = config.trust_proxy_headers
        && ForwardedInfo::from_headers(headers).scheme.as_deref() == Some("https");
    let x_forwarded_proto = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok());
    auth_cookie::is_https_request(is_secure, config.trust_proxy_headers, x_forwarded_proto)
}

// ---------------------------------------------------------------------------
// OIDC client plumbing
// ---------------------------------------------------------------------------

/// Redirect-following must be disabled: the token/discovery requests carry
/// credentials and an IdP-controlled redirect is an SSRF vector
/// (openidconnect's own guidance).
fn http_client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::ClientBuilder::new()
        .redirect(reqwest::redirect::Policy::none())
        .build()?)
}

/// Discovery from `<issuer>/.well-known/openid-configuration` (the crate
/// derives that path from the issuer URL, exactly as Authlib's
/// `server_metadata_url` was built), fetching the JWKS alongside.
async fn discover(
    settings: &OidcSettings,
    http: &reqwest::Client,
) -> anyhow::Result<CoreProviderMetadata> {
    let issuer = IssuerUrl::new(settings.issuer.clone())?;
    Ok(CoreProviderMetadata::discover_async(issuer, http).await?)
}

// ---------------------------------------------------------------------------
// GET /auth/login/oidc
// ---------------------------------------------------------------------------

async fn login_oidc(
    State(state): State<AppState>,
    forwarded: Option<Extension<ForwardedInfo>>,
    headers: HeaderMap,
) -> Response {
    let Some(settings) = oidc_settings(&state.config) else {
        return not_configured();
    };
    let forwarded = forwarded.as_ref().map(|Extension(info)| info);
    let redirect_uri = settings.callback_url.clone().unwrap_or_else(|| {
        // url_for("oauth.oidc_callback", _external=True).
        format!(
            "{}://{}/api/auth/callback/oidc",
            effective_scheme(forwarded),
            effective_host(&headers, forwarded)
        )
    });
    match build_authorize_redirect(&settings, redirect_uri).await {
        Ok((location, payload)) => {
            let secure = request_is_secure(&state.config, &headers);
            let signed = sign_state_payload(&state.config.secret_key, &payload);
            let mut response = flask_redirect(&location);
            append_set_cookie(&mut response, &state_cookie(signed, secure));
            response
        }
        // An exception out of authorize_redirect hit Flask's 500 handler.
        Err(exc) => {
            tracing::error!(error = %exc, "OIDC login redirect failed");
            internal_server_error()
        }
    }
}

async fn build_authorize_redirect(
    settings: &OidcSettings,
    redirect_uri: String,
) -> anyhow::Result<(String, StatePayload)> {
    let http = http_client()?;
    let metadata = discover(settings, &http).await?;
    let client = CoreClient::from_provider_metadata(
        metadata,
        ClientId::new(settings.client_id.clone()),
        Some(ClientSecret::new(settings.client_secret.clone())),
    )
    .set_redirect_uri(RedirectUrl::new(redirect_uri.clone())?);
    // Scope exactly "openid email profile" ("openid" is implicit-first).
    let (auth_url, csrf_state, nonce) = client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .add_scope(Scope::new("email".to_string()))
        .add_scope(Scope::new("profile".to_string()))
        .url();
    let payload = StatePayload {
        state: csrf_state.secret().clone(),
        nonce: nonce.secret().clone(),
        redirect_uri,
    };
    Ok((auth_url.to_string(), payload))
}

// ---------------------------------------------------------------------------
// GET /auth/callback/oidc
// ---------------------------------------------------------------------------

/// Which generic fragment a failure maps to — mirroring which `except`
/// block caught it in Python.
enum CallbackError {
    /// Anything in the validation half (`authorize_access_token`): missing/
    /// bad state cookie, state mismatch, provider `error` param, exchange
    /// failure, ID-token signature/iss/aud/exp/nonce/at_hash failure,
    /// missing/empty `sub`.
    CallbackFailed,
    /// Provisioning or JWT issuance failed.
    AuthFailed,
}

impl CallbackError {
    fn fragment(&self) -> &'static str {
        match self {
            CallbackError::CallbackFailed => "sso_error=callback_failed",
            CallbackError::AuthFailed => "sso_error=auth_failed",
        }
    }
}

async fn oidc_callback(
    State(state): State<AppState>,
    forwarded: Option<Extension<ForwardedInfo>>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    if oidc_settings(&state.config).is_none() {
        return not_configured();
    }
    let forwarded = forwarded.as_ref().map(|Extension(info)| info);
    let host = effective_host(&headers, forwarded);
    let secure = request_is_secure(&state.config, &headers);
    let params = Query::<HashMap<String, String>>::try_from_uri(&uri)
        .map(|Query(map)| map)
        .unwrap_or_default();

    let mut response = match run_callback(&state, &headers, &params).await {
        Ok((token, session_cookie_value)) => {
            // Token handed to the SPA in the URL fragment (immediate-state
            // path) AND set as the httpOnly session cookie (the persistent
            // session).
            let mut response =
                frontend_redirect(&state.config, &host, &format!("sso_token={token}"));
            append_set_cookie(
                &mut response,
                &auth_cookie::auth_cookie(&session_cookie_value, secure),
            );
            response
        }
        Err(error) => {
            // Deliberately generic: no codes, tokens, or IdP payloads.
            tracing::warn!("OIDC callback failed validation");
            frontend_redirect(&state.config, &host, error.fragment())
        }
    };
    // The handshake state is single-use either way (Flask popped it from
    // the session).
    append_set_cookie(&mut response, &clear_state_cookie(secure));
    response
}

/// The whole callback pipeline; returns the app JWT (used both in the
/// fragment and the cookie).
async fn run_callback(
    state: &AppState,
    headers: &HeaderMap,
    params: &HashMap<String, String>,
) -> Result<(String, String), CallbackError> {
    let settings = oidc_settings(&state.config).ok_or(CallbackError::CallbackFailed)?;

    // --- authorize_access_token equivalent (validation half) ---
    let payload = request_cookie(headers, STATE_COOKIE_NAME)
        .and_then(|raw| verify_state_payload(&state.config.secret_key, &raw))
        .ok_or(CallbackError::CallbackFailed)?;
    let presented_state = params.get("state").ok_or(CallbackError::CallbackFailed)?;
    if !bool::from(presented_state.as_bytes().ct_eq(payload.state.as_bytes())) {
        return Err(CallbackError::CallbackFailed);
    }
    // An IdP error response (`?error=access_denied&...`) raised in Authlib.
    if params.contains_key("error") {
        return Err(CallbackError::CallbackFailed);
    }
    let code = params.get("code").ok_or(CallbackError::CallbackFailed)?;

    let (subject, email, name, avatar) = exchange_and_validate(&settings, &payload, code)
        .await
        .map_err(|_| CallbackError::CallbackFailed)?;
    if subject.is_empty() {
        return Err(CallbackError::CallbackFailed);
    }

    // --- provisioning (handle_oidc_login → upsert_oidc_user) ---
    let upserted = store::users::upsert_oidc_user(&state.db, subject, email, name, avatar).await;
    let user = match upserted {
        Ok(Some(user)) => user,
        _ => return Err(CallbackError::AuthFailed),
    };

    // --- app JWT (HS256, {user_id, exp, iat}) ---
    let token = jwt::issue_token(&state.config.jwt_secret, user.id)
        .map_err(|_| CallbackError::AuthFailed)?;
    Ok((token.clone(), token))
}

/// Code exchange + full ID-token validation, returning
/// `(sub, email, name-or-preferred_username, picture)`.
async fn exchange_and_validate(
    settings: &OidcSettings,
    payload: &StatePayload,
    code: &str,
) -> anyhow::Result<(String, Option<String>, Option<String>, Option<String>)> {
    let http = http_client()?;
    let metadata = discover(settings, &http).await?;
    let client = CoreClient::from_provider_metadata(
        metadata,
        ClientId::new(settings.client_id.clone()),
        Some(ClientSecret::new(settings.client_secret.clone())),
    )
    .set_redirect_uri(RedirectUrl::new(payload.redirect_uri.clone())?);

    let token_response = client
        .exchange_code(AuthorizationCode::new(code.to_string()))?
        .request_async(&http)
        .await?;

    // Signature (via the JWKS fetched from discovery), issuer, audience,
    // expiry, and nonce are all verified here.
    let id_token = token_response
        .id_token()
        .ok_or_else(|| anyhow::anyhow!("no ID token in token response"))?;
    let verifier = client.id_token_verifier();
    let nonce = Nonce::new(payload.nonce.clone());
    let claims = id_token.claims(&verifier, &nonce)?;

    // Authlib also checked at_hash whenever the IdP included one.
    if let Some(expected_hash) = claims.access_token_hash() {
        let actual_hash = AccessTokenHash::from_token(
            token_response.access_token(),
            id_token.signing_alg()?,
            id_token.signing_key(&verifier)?,
        )?;
        if actual_hash != *expected_hash {
            anyhow::bail!("access token hash mismatch");
        }
    }

    let subject = claims.subject().as_str().to_string();
    let email = claims.email().map(|value| value.as_str().to_string());
    // `userinfo.get("name") or userinfo.get("preferred_username")`:
    // a missing or empty name falls back.
    let name = claims
        .name()
        .and_then(|localized| localized.get(None))
        .map(|value| value.as_str().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            claims
                .preferred_username()
                .map(|value| value.as_str().to_string())
        });
    let avatar = claims
        .picture()
        .and_then(|localized| localized.get(None))
        .map(|value| value.as_str().to_string());
    Ok((subject, email, name, avatar))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    // --- _safe_frontend_base (table-driven, mirroring the Python cases) ---

    #[rstest]
    // Empty / slash-only bases collapse to same-origin.
    #[case::empty("", "app.example.com", "")]
    #[case::single_slash("/", "app.example.com", "")]
    #[case::double_slash("//", "app.example.com", "")]
    // Relative path with a single leading slash is trusted (rstrip'd).
    #[case::relative("/nightlio", "app.example.com", "/nightlio")]
    #[case::relative_trailing("/nightlio/", "app.example.com", "/nightlio")]
    // Protocol-relative is NOT a relative path.
    #[case::protocol_relative("//evil.example", "app.example.com", "")]
    // Backslashes are never allowed through.
    #[case::backslash_path("/\\evil.example", "app.example.com", "")]
    #[case::backslash_absolute("http://app.example.com/\\x", "app.example.com", "")]
    // Same-origin absolute URLs are trusted, case-insensitively, with port.
    #[case::same_origin("http://app.example.com", "app.example.com", "http://app.example.com")]
    #[case::same_origin_https(
        "https://app.example.com",
        "app.example.com",
        "https://app.example.com"
    )]
    #[case::same_origin_case(
        "https://APP.Example.COM",
        "app.example.com",
        "https://APP.Example.COM"
    )]
    #[case::same_origin_port(
        "http://app.example.com:5000",
        "app.example.com:5000",
        "http://app.example.com:5000"
    )]
    #[case::same_origin_path(
        "https://app.example.com/spa",
        "app.example.com",
        "https://app.example.com/spa"
    )]
    // Off-host / wrong scheme / hostless are rejected.
    #[case::off_host("https://evil.example", "app.example.com", "")]
    #[case::port_mismatch("http://app.example.com:8080", "app.example.com", "")]
    #[case::ftp("ftp://app.example.com", "app.example.com", "")]
    #[case::javascript("javascript:alert(1)", "app.example.com", "")]
    #[case::no_netloc("http://", "app.example.com", "")]
    #[case::bare_word("nightlio", "app.example.com", "")]
    fn safe_frontend_base_cases(#[case] base: &str, #[case] host: &str, #[case] expected: &str) {
        assert_eq!(safe_frontend_base(base, host), expected);
    }

    // --- signed state cookie ---

    const SECRET: &str = "state-cookie-test-secret";

    fn sample_payload() -> StatePayload {
        StatePayload {
            state: "abc123".to_string(),
            nonce: "n0nce".to_string(),
            redirect_uri: "http://app.example.com/api/auth/callback/oidc".to_string(),
        }
    }

    #[test]
    fn state_payload_round_trips() {
        let signed = sign_state_payload(SECRET, &sample_payload());
        assert_eq!(
            verify_state_payload(SECRET, &signed),
            Some(sample_payload())
        );
    }

    #[test]
    fn state_payload_rejects_wrong_key_and_tampering() {
        let signed = sign_state_payload(SECRET, &sample_payload());
        assert_eq!(verify_state_payload("another-secret", &signed), None);

        // Flip one hex nibble of the payload half.
        let mut tampered = signed.clone().into_bytes();
        tampered[0] = if tampered[0] == b'a' { b'b' } else { b'a' };
        let tampered = String::from_utf8(tampered).unwrap();
        assert_eq!(verify_state_payload(SECRET, &tampered), None);

        // Structural garbage.
        for bad in ["", ".", "zz.zz", "deadbeef", "deadbeef.od d"] {
            assert_eq!(verify_state_payload(SECRET, bad), None, "{bad}");
        }
    }

    #[test]
    fn state_cookie_attributes() {
        let cookie = state_cookie("value".to_string(), false);
        assert_eq!(cookie.name(), STATE_COOKIE_NAME);
        assert_eq!(cookie.path(), Some("/"));
        assert_eq!(cookie.http_only(), Some(true));
        assert_eq!(cookie.same_site(), Some(SameSite::Lax));
        assert_eq!(cookie.max_age(), Some(Duration::seconds(600)));
        assert_ne!(cookie.secure(), Some(true));
        assert_eq!(state_cookie("v".to_string(), true).secure(), Some(true));

        let clearing = clear_state_cookie(false);
        assert_eq!(clearing.value(), "");
        assert_eq!(clearing.max_age(), Some(Duration::ZERO));
    }

    // --- werkzeug redirect body ---

    #[test]
    fn flask_redirect_matches_werkzeug_bytes() {
        // Captured from the Flask venv:
        // werkzeug.utils.redirect('/login#sso_error=callback_failed').
        let expected = "<!doctype html>\n<html lang=en>\n<title>Redirecting...\
                        </title>\n<h1>Redirecting...</h1>\n<p>You should be \
                        redirected automatically to the target URL: \
                        <a href=\"/login#sso_error=callback_failed\">\
                        /login#sso_error=callback_failed</a>. If not, click \
                        the link.\n";
        let response = flask_redirect("/login#sso_error=callback_failed");
        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(
            response.headers().get(header::LOCATION).unwrap(),
            "/login#sso_error=callback_failed"
        );
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/html; charset=utf-8"
        );
        let body = futures_body(response);
        assert_eq!(body, expected);
    }

    #[test]
    fn flask_redirect_escapes_like_markupsafe() {
        let response = flask_redirect("http://h/x?a=1&b=<\"x\">");
        let body = futures_body(response);
        assert!(
            body.contains("<a href=\"http://h/x?a=1&amp;b=&lt;&#34;x&#34;&gt;\">"),
            "{body}"
        );
    }

    /// Synchronously drain a small test-response body.
    fn futures_body(response: Response) -> String {
        let bytes = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(axum::body::to_bytes(response.into_body(), 1 << 20))
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    // --- misc helpers ---

    #[test]
    fn hex_round_trip() {
        assert_eq!(hex_encode(&[0x00, 0xff, 0x1a]), "00ff1a");
        assert_eq!(hex_decode("00ff1a"), Some(vec![0x00, 0xff, 0x1a]));
        assert_eq!(hex_decode("0"), None);
        assert_eq!(hex_decode("zz"), None);
    }

    #[test]
    fn request_cookie_finds_named_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            "other=1; nightlio_oidc=abc.def; nightlio_token=t"
                .parse()
                .unwrap(),
        );
        assert_eq!(
            request_cookie(&headers, STATE_COOKIE_NAME).as_deref(),
            Some("abc.def")
        );
        assert_eq!(request_cookie(&headers, "missing"), None);
    }

    #[test]
    fn issuer_is_rstripped_and_settings_require_client_id() {
        let vars = |issuer: Option<&str>, client: Option<&str>| {
            let issuer = issuer.map(str::to_string);
            let client = client.map(str::to_string);
            Config::from_lookup(&move |key: &str| match key {
                "OIDC_ISSUER_URL" => issuer.clone(),
                "OIDC_CLIENT_ID" => client.clone(),
                _ => None,
            })
        };
        let settings = oidc_settings(&vars(Some("https://id.example.com///"), Some("cid")))
            .expect("configured");
        assert_eq!(settings.issuer, "https://id.example.com");
        assert!(oidc_settings(&vars(Some("https://id.example.com"), None)).is_none());
        assert!(oidc_settings(&vars(None, Some("cid"))).is_none());
    }

    /// Regression (parity finding): with `TRUST_PROXY_HEADERS=1` the
    /// Secure flag must follow ProxyFix's rightmost `X-Forwarded-Proto`
    /// value (Flask's `request.is_secure`), not only the first value —
    /// `http,https` from an appending proxy chain still means HTTPS.
    #[test]
    fn request_is_secure_honors_proxyfix_rightmost_value() {
        let config = |trust: bool| {
            Config::from_lookup(&move |key: &str| match key {
                "TRUST_PROXY_HEADERS" if trust => Some("1".to_string()),
                _ => None,
            })
        };
        let headers = |xfp: &str| {
            let mut headers = HeaderMap::new();
            headers.insert("x-forwarded-proto", xfp.parse().unwrap());
            headers
        };
        assert!(request_is_secure(&config(true), &headers("http,https")));
        assert!(request_is_secure(&config(true), &headers("https,http")));
        assert!(!request_is_secure(&config(true), &headers("http")));
        assert!(!request_is_secure(&config(false), &headers("http,https")));
    }
}
