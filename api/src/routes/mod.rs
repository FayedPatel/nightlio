//! HTTP routes — port of `api/routes/*.py` as axum handlers mounted under
//! `/api`, plus the CORS / security-header / proxy-header / tracing
//! middleware from `create_app()` (`api/app.py`).
//!
//! The shell agent owns this file (router assembly + middleware + shared
//! routing helpers); each family module below is owned wholesale by its
//! route-family agent.
//!
//! # Conventions for route-family agents
//!
//! - Family routers ([`auth`], [`mood`], [`goals`], [`groups`],
//!   [`achievements`], [`extras`]) register paths RELATIVE to `/api`
//!   (e.g. `/moods`, `/goal/{id}`): they are nested at `/api` here.
//!   [`misc`] is the one exception — it owns the `/api` ↔ `/api/` slash
//!   pair, which nesting cannot express, so it registers absolute paths
//!   and is merged.
//! - Flask `strict_slashes` semantics come for free: axum matches paths
//!   exactly, so a rule registered without a trailing slash 404s the
//!   slashed variant (via the JSON fallback). Do NOT add any
//!   normalize-path layer. The only alias pairs (goal completions,
//!   achievements/progress) must be registered explicitly by their owners.
//! - Path ids: use [`FlaskPath`]`<`[`FlaskInt`]`>` (or tuples of
//!   [`FlaskInt`]) so negative / non-integer / overflow ids produce the
//!   Flask-equivalent JSON 404 (`contract/DECISIONS.md` #1) instead of
//!   axum's default 400.
//! - Automatic OPTIONS (204, empty body, `Allow` — contract change,
//!   previously Flask's 200) is reproduced per-route with
//!   [`automatic_options`]; the two explicitly-registered OPTIONS rule
//!   families (goal completions, achievements/progress) are registered by
//!   hand by their owners and answer the same 204. CORS preflights DO
//!   traverse the router: the CORS after-request layer never
//!   short-circuits, so preflights ride the same 204 with the CORS
//!   headers appended afterwards.
//! - Auth: take `user: crate::auth::extract::AuthUser` as a handler
//!   argument. It enforces the cookie-auth CSRF checks itself, in Flask's
//!   exact order (403 before any token verification).

pub mod achievements;
pub mod auth;
pub mod extras;
pub mod goals;
pub mod groups;
pub mod misc;
pub mod mood;

use std::sync::Arc;

use axum::extract::{FromRequestParts, Request};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::MethodRouter;
use axum::{Json, Router};
use serde_json::json;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

use crate::config::{AppEnv, Config};
use crate::state::AppState;

/// The CSP string from `api/utils/security_headers.py`, byte-identical.
/// Sent only when the Flask config would have `DEBUG = False`, i.e. only
/// in production (development and testing both set `DEBUG = True`).
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; \
     script-src 'self' 'unsafe-inline'; \
     style-src 'self' 'unsafe-inline'; \
     img-src 'self' data: https:; \
     connect-src 'self'; \
     frame-src 'none';";

// ---------------------------------------------------------------------------
// Router assembly
// ---------------------------------------------------------------------------

/// All `/api` routes, before state and middleware are applied.
fn api_router() -> Router<AppState> {
    Router::new()
        // misc owns the /api ↔ /api/ slash pair → absolute paths, merged.
        .merge(misc::router())
        // Family routers register paths relative to /api.
        .nest("/api", auth::router())
        .nest("/api", mood::router())
        .nest("/api", groups::router())
        .nest("/api", goals::router())
        .nest("/api", achievements::router())
        .nest("/api", extras::router())
}

/// The complete application: routes + state + the `create_app()` middleware
/// stack. This is what `main.rs` serves and what integration tests oneshot.
pub fn build_router(state: AppState) -> Router {
    let config = state.config.clone();
    let mut api = api_router();
    // OIDC routes exist only when configured, mirroring the conditional
    // blueprint registration in `create_app()` (`api/app.py`) — when the
    // issuer is unset the paths fall through to the standard JSON 404.
    if config.oidc_enabled() {
        api = api.nest("/api", crate::auth::oidc::router());
    }
    let router = api
        .method_not_allowed_fallback(method_not_allowed)
        .fallback(fallback_not_found)
        .with_state(state);
    apply_middleware(router, &config)
}

/// Middleware mirroring `create_app()` (`api/app.py`). Later `.layer(...)`
/// calls wrap earlier ones, so the security headers (Flask `after_request`)
/// sit OUTSIDE the CORS layer and stamp every response — including CORS
/// preflights, the 404 fallback, 405s, and the `/api` 308 redirect.
fn apply_middleware(router: Router, config: &Config) -> Router {
    // flask-cors equivalent: a pure after_request decorator, NOT a
    // short-circuiting layer. tower-http's CorsLayer intercepts every
    // OPTIONS request before the router, which would break the OPTIONS
    // contract (all OPTIONS answer 204 with Allow — contract change;
    // JSON 404 for unknown paths). Here the router always answers and
    // CORS headers are appended after, exactly like flask-cors'
    // after_request hook — so preflights ride the router's 204 too.
    let origins = Arc::new(config.cors_origins.clone());
    let cors = axum::middleware::from_fn(move |request: Request, next: Next| {
        let origins = Arc::clone(&origins);
        async move { cors_after_request(&origins, request, next).await }
    });

    let mut router = router
        .layer(cors)
        // Security headers on every response (api/utils/security_headers.py).
        .layer(SetResponseHeaderLayer::overriding(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::X_XSS_PROTECTION,
            HeaderValue::from_static("1; mode=block"),
        ))
        .layer(SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            HeaderValue::from_static("strict-origin-when-cross-origin"),
        ));

    // CSP only when not DEBUG: production is the only non-DEBUG env.
    if config.app_env == AppEnv::Production {
        router = router.layer(SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CONTENT_SECURITY_POLICY),
        ));
    }

    // Access logs.
    let mut router = router.layer(TraceLayer::new_for_http());

    // ProxyFix(x_for=1, x_proto=1, x_proto=1 → consume one value from the
    // right of each X-Forwarded-* header) only when TRUST_PROXY_HEADERS is
    // truthy — the headers are client-spoofable without a stripping proxy.
    if config.trust_proxy_headers {
        router = router.layer(axum::middleware::from_fn(proxy_fix_middleware));
    }
    router
}

// ---------------------------------------------------------------------------
// CORS (flask-cors after_request semantics)
// ---------------------------------------------------------------------------

/// `Access-Control-Allow-Methods` value flask-cors sends on preflights
/// with its default configuration (all methods, sorted).
const CORS_ALLOW_METHODS: &str = "DELETE, GET, HEAD, OPTIONS, PATCH, POST, PUT";

/// Append CORS response headers exactly like flask-cors with
/// `CORS(app, origins=[...], supports_credentials=True)`:
/// - explicit origin list (never `*` — credentials and wildcard are
///   mutually exclusive), exact match against the `Origin` header;
/// - allowed origin → `Access-Control-Allow-Origin` echo,
///   `Access-Control-Allow-Credentials: true`, `Vary: Origin`;
/// - preflights (OPTIONS + `Access-Control-Request-Method`) additionally
///   get `Access-Control-Allow-Methods` and an echo of the requested
///   headers (lowercased, `", "`-joined) — layered ON the router's own
///   response, so a preflight to a GET-only rule still carries Flask's
///   automatic-OPTIONS 200 + `Allow`, matching the recorded fixtures;
/// - disallowed/absent origin → no CORS headers at all.
async fn cors_after_request(origins: &[String], request: Request, next: Next) -> Response {
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let is_preflight = request.method() == Method::OPTIONS
        && request
            .headers()
            .contains_key(header::ACCESS_CONTROL_REQUEST_METHOD);
    let requested_headers = request
        .headers()
        .get(header::ACCESS_CONTROL_REQUEST_HEADERS)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    let mut response = next.run(request).await;

    let Some(origin) = origin.filter(|origin| origins.iter().any(|allowed| allowed == origin))
    else {
        return response;
    };
    let Ok(origin_value) = HeaderValue::from_str(&origin) else {
        return response;
    };
    let headers = response.headers_mut();
    headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin_value);
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
        HeaderValue::from_static("true"),
    );
    if is_preflight {
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static(CORS_ALLOW_METHODS),
        );
        if let Some(requested) = requested_headers {
            let echoed = requested
                .split(',')
                .map(|name| name.trim().to_ascii_lowercase())
                .filter(|name| !name.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            if let Ok(value) = HeaderValue::from_str(&echoed) {
                headers.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, value);
            }
        }
    }
    headers.append(header::VARY, HeaderValue::from_static("Origin"));
    response
}

// ---------------------------------------------------------------------------
// Error envelope
// ---------------------------------------------------------------------------

/// The app-level Flask 404 body: `{"error": "Resource not found"}`.
pub fn resource_not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": "Resource not found" })),
    )
        .into_response()
}

/// Router fallback for unknown paths (including trailing-slash variants of
/// rules registered without one — Werkzeug `strict_slashes`).
pub async fn fallback_not_found() -> Response {
    resource_not_found()
}

/// Method-not-allowed fallback: the standard JSON envelope on every 405
/// (contract change — Werkzeug's HTML body is gone). Axum appends the
/// synthesized `Allow` header to this response after the fact, so the
/// registered-methods contract is untouched.
pub async fn method_not_allowed() -> Response {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        Json(json!({ "error": "Method not allowed" })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Flask `<int:...>` path semantics
// ---------------------------------------------------------------------------

/// A path segment matching Flask's `<int:...>` converter: unsigned digits
/// only, value must fit in `i64`. Negative, non-integer, and overflow
/// (> i64) segments all fail deserialization, which [`FlaskPath`] maps to
/// the JSON 404 — per `contract/DECISIONS.md` #1 the overflow case is
/// accepted drift from Flask's leaked-Python-text 500.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FlaskInt(pub i64);

impl<'de> serde::Deserialize<'de> for FlaskInt {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = FlaskInt;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a non-negative integer path segment")
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<FlaskInt, E> {
                // Flask's int converter regex is `\d+`: no sign, no dot.
                if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(E::custom("not a Flask <int> segment"));
                }
                value
                    .parse::<i64>()
                    .map(FlaskInt)
                    .map_err(|_| E::custom("id overflows i64"))
            }
        }
        deserializer.deserialize_str(Visitor)
    }
}

/// `axum::extract::Path` with Flask routing semantics: any extraction
/// failure means "the rule never matched" → the standard JSON 404, not
/// axum's default 400. Use with [`FlaskInt`] (or tuples of it):
/// `FlaskPath(FlaskInt(id)): FlaskPath<FlaskInt>`.
#[derive(Debug, Clone, Copy)]
pub struct FlaskPath<T>(pub T);

impl<S, T> FromRequestParts<S> for FlaskPath<T>
where
    T: serde::de::DeserializeOwned + Send,
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        match axum::extract::Path::<T>::from_request_parts(parts, state).await {
            Ok(axum::extract::Path(value)) => Ok(FlaskPath(value)),
            Err(_) => Err(resource_not_found()),
        }
    }
}

// ---------------------------------------------------------------------------
// Flask automatic OPTIONS
// ---------------------------------------------------------------------------

/// A `MethodRouter` for a route's automatic OPTIONS response: 204, empty
/// body, `Allow` header, `text/html; charset=utf-8` (contract change:
/// every OPTIONS in the app answers 204 now — the old two-regime split
/// between Flask's automatic 200 and the explicitly-registered 204 rules
/// is gone; the frontend never checks Content-Type on OPTIONS). The wire
/// response carries no `Content-Length` (RFC 9110 §8.6): hyper strips it
/// for 204 at serialization (`can_have_content_length`, h1/role.rs).
/// axum's top-level router stamps `Content-Length: 0` on the in-process
/// response AFTER every layer runs (`set_content_length`,
/// axum/src/routing/route.rs), so it cannot be removed here and oneshot
/// tests must not assert its absence — or its value. Merge into a route:
/// `get(handler).merge(automatic_options("HEAD, GET, OPTIONS"))`. CORS
/// preflights reach this handler too; the after-request CORS layer appends
/// the CORS headers on the way out.
pub fn automatic_options(allow: &'static str) -> MethodRouter<AppState> {
    axum::routing::options(move || async move {
        (
            StatusCode::NO_CONTENT,
            [
                (header::ALLOW, HeaderValue::from_static(allow)),
                (
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("text/html; charset=utf-8"),
                ),
            ],
        )
    })
}

// ---------------------------------------------------------------------------
// ProxyFix
// ---------------------------------------------------------------------------

/// Trusted forwarded-request properties, inserted as a request extension
/// only when `TRUST_PROXY_HEADERS` is truthy (werkzeug
/// `ProxyFix(x_for=1, x_proto=1, x_host=1)`). Each field is the value one
/// from the right of the corresponding `X-Forwarded-*` header, i.e. the
/// value appended by the trusted proxy. The headers themselves are left on
/// the request, exactly as werkzeug leaves them in the WSGI environ.
///
/// Consumers: `Option<axum::Extension<ForwardedInfo>>` — absent means the
/// proxy is not trusted (or sent no headers), so fall back to the direct
/// connection's properties (scheme `http`, the `Host` header).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForwardedInfo {
    /// From `X-Forwarded-For` (werkzeug's `REMOTE_ADDR` rewrite).
    pub client: Option<String>,
    /// From `X-Forwarded-Proto` (werkzeug's `wsgi.url_scheme` rewrite).
    pub scheme: Option<String>,
    /// From `X-Forwarded-Host` (werkzeug's `HTTP_HOST` rewrite).
    pub host: Option<String>,
}

impl ForwardedInfo {
    /// Extract the trusted (rightmost) value of each header. Multiple
    /// header lines are joined with commas first, matching how WSGI
    /// presents repeated headers to werkzeug.
    pub fn from_headers(headers: &HeaderMap) -> Self {
        fn rightmost(headers: &HeaderMap, name: &str) -> Option<String> {
            let joined = headers
                .get_all(name)
                .iter()
                .filter_map(|value| value.to_str().ok())
                .collect::<Vec<_>>()
                .join(",");
            let last = joined.rsplit(',').next().unwrap_or("").trim();
            if last.is_empty() {
                None
            } else {
                Some(last.to_string())
            }
        }
        ForwardedInfo {
            client: rightmost(headers, "x-forwarded-for"),
            scheme: rightmost(headers, "x-forwarded-proto"),
            host: rightmost(headers, "x-forwarded-host"),
        }
    }
}

async fn proxy_fix_middleware(mut request: Request, next: Next) -> Response {
    let info = ForwardedInfo::from_headers(request.headers());
    request.extensions_mut().insert(info);
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwarded_info_consumes_one_from_the_right() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "1.1.1.1, 2.2.2.2".parse().unwrap());
        headers.insert("x-forwarded-proto", "http,https".parse().unwrap());
        headers.insert(
            "x-forwarded-host",
            "outer.example, inner.example".parse().unwrap(),
        );
        let info = ForwardedInfo::from_headers(&headers);
        assert_eq!(info.client.as_deref(), Some("2.2.2.2"));
        assert_eq!(info.scheme.as_deref(), Some("https"));
        assert_eq!(info.host.as_deref(), Some("inner.example"));
    }

    #[test]
    fn forwarded_info_joins_repeated_header_lines() {
        let mut headers = HeaderMap::new();
        headers.append("x-forwarded-proto", "https".parse().unwrap());
        headers.append("x-forwarded-proto", "http".parse().unwrap());
        let info = ForwardedInfo::from_headers(&headers);
        assert_eq!(info.scheme.as_deref(), Some("http"));
    }

    #[test]
    fn forwarded_info_absent_headers_are_none() {
        let info = ForwardedInfo::from_headers(&HeaderMap::new());
        assert_eq!(info, ForwardedInfo::default());
    }

    #[test]
    fn csp_string_matches_python() {
        // The Python source builds the string via adjacent-literal
        // concatenation; pin the exact result.
        assert_eq!(
            CONTENT_SECURITY_POLICY,
            "default-src 'self'; script-src 'self' 'unsafe-inline'; \
             style-src 'self' 'unsafe-inline'; img-src 'self' data: https:; \
             connect-src 'self'; frame-src 'none';"
        );
    }

    #[test]
    fn flask_int_accepts_digits_only() {
        let ok: FlaskInt =
            serde_json::from_value(serde_json::Value::String("42".into())).expect("digits parse");
        assert_eq!(ok, FlaskInt(42));
        for bad in ["-1", "1.5", "abc", "", "99999999999999999999"] {
            let result: Result<FlaskInt, _> =
                serde_json::from_value(serde_json::Value::String(bad.into()));
            assert!(result.is_err(), "{bad} should not match <int>");
        }
    }
}
