//! The `AuthUser` axum extractor — port of `require_auth` in
//! `api/utils/auth_middleware.py`, including its exact check ORDER, which
//! is user-visible:
//!
//! 1. token extraction (`Authorization: Bearer` first, `nightlio_token`
//!    cookie fallback) — no credential at all → 401
//!    `{"error": "Authorization header required"}` (yes, even when only
//!    the cookie is missing);
//! 2. CSRF checks for cookie-authenticated state-changing requests —
//!    BEFORE token verification, so a cookie mutation with a missing CSRF
//!    header and an expired token gets the 403, not the 401;
//! 3. JWT verification — 401 `{"error": "Token expired"}` /
//!    `{"error": "Invalid token"}` per `auth::jwt`.
//!
//! Because the CSRF enforcement lives inside the extractor, route-family
//! agents get Flask-equivalent behavior by simply taking `user: AuthUser`
//! as a handler argument — no extra middleware needed. The standalone
//! [`enforce_csrf`] helper is exported for any handler that manages auth
//! manually.
//!
//! Flask's `require_auth` also short-circuits OPTIONS to 204, but that
//! branch is unreachable dead code (Flask's automatic OPTIONS answers at
//! the routing layer first — fixture-verified), so it is not ported here;
//! OPTIONS behavior is reproduced route-by-route in the routers.

use axum::Json;
use axum::extract::{FromRef, FromRequestParts};
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::auth::cookie::COOKIE_NAME;
use crate::auth::csrf::{self, AuthSource, CsrfRejection};
use crate::auth::jwt::{self, VerifyError};
use crate::state::AppState;

/// Exact 401 body message when no credential is presented at all
/// (`api/utils/auth_middleware.py`).
pub const AUTH_REQUIRED_MESSAGE: &str = "Authorization header required";

/// The authenticated caller. Handlers take this as an argument; extraction
/// failure produces the exact Flask-shaped 401/403 JSON bodies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthUser {
    /// `g.user_id` — the `user_id` claim of the verified JWT.
    pub user_id: i64,
    /// How the request authenticated; Bearer always beats cookie.
    pub source: AuthSource,
}

/// Extraction failure; `IntoResponse` yields the exact Flask JSON bodies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthRejection {
    /// No `Authorization: Bearer` header and no non-empty `nightlio_token`
    /// cookie → 401 `{"error": "Authorization header required"}`.
    MissingCredentials,
    /// Cookie-authenticated mutation failed a CSRF check → 403 with the
    /// exact message from `auth::csrf`.
    Csrf(CsrfRejection),
    /// JWT verification failed → 401 `Token expired` / `Invalid token`.
    Token(VerifyError),
}

impl IntoResponse for AuthRejection {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            AuthRejection::MissingCredentials => (StatusCode::UNAUTHORIZED, AUTH_REQUIRED_MESSAGE),
            AuthRejection::Csrf(rejection) => (StatusCode::FORBIDDEN, rejection.message()),
            AuthRejection::Token(error) => (StatusCode::UNAUTHORIZED, error.message()),
        };
        (status, Json(json!({ "error": message }))).into_response()
    }
}

/// Port of `_extract_token`: `Authorization: Bearer` wins when present (a
/// header is never "downgraded" to cookie auth); otherwise the first
/// non-empty `nightlio_token` cookie. Flask's `if cookie_token:` treats an
/// empty cookie value as absent, so we do too.
pub fn extract_token(parts: &Parts) -> Option<(String, AuthSource)> {
    if let Some(value) = parts.headers.get(header::AUTHORIZATION)
        && let Ok(raw) = value.to_str()
        && let Some(token) = raw.strip_prefix("Bearer ")
    {
        return Some((token.to_string(), AuthSource::Bearer));
    }
    for value in parts.headers.get_all(header::COOKIE) {
        let Ok(raw) = value.to_str() else { continue };
        for parsed in cookie::Cookie::split_parse(raw.to_owned()) {
            let Ok(parsed) = parsed else { continue };
            if parsed.name() == COOKIE_NAME && !parsed.value().is_empty() {
                return Some((parsed.value().to_string(), AuthSource::Cookie));
            }
        }
    }
    None
}

/// Shared CSRF enforcement: run `auth::csrf::csrf_check` against the
/// request parts. Safe to call unconditionally — Bearer requests and
/// non-state-changing methods pass through inside the predicate.
pub fn enforce_csrf(parts: &Parts, source: AuthSource) -> Result<(), CsrfRejection> {
    let content_type = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok());
    let x_requested_with = parts
        .headers
        .get(csrf::CSRF_HEADER_NAME)
        .and_then(|value| value.to_str().ok());
    csrf::csrf_check(
        parts.method.as_str(),
        source,
        content_type,
        x_requested_with,
    )
}

impl<S> FromRequestParts<S> for AuthUser
where
    AppState: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = AuthRejection;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app = AppState::from_ref(state);
        let (token, source) = extract_token(parts).ok_or(AuthRejection::MissingCredentials)?;
        // CSRF before verification — matches require_auth's order.
        enforce_csrf(parts, source).map_err(AuthRejection::Csrf)?;
        let claims =
            jwt::verify_token(&app.config.jwt_secret, &token).map_err(AuthRejection::Token)?;
        Ok(AuthUser {
            user_id: claims.user_id,
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

    fn parts_for(builder: axum::http::request::Builder) -> Parts {
        let (parts, ()) = builder.body(()).unwrap().into_parts();
        parts
    }

    #[test]
    fn bearer_wins_over_cookie() {
        let parts = parts_for(
            Request::builder()
                .header("Authorization", "Bearer header-token")
                .header("Cookie", "nightlio_token=cookie-token"),
        );
        assert_eq!(
            extract_token(&parts),
            Some(("header-token".to_string(), AuthSource::Bearer))
        );
    }

    #[test]
    fn cookie_fallback_when_no_bearer() {
        let parts = parts_for(
            Request::builder().header("Cookie", "other=1; nightlio_token=cookie-token; a=b"),
        );
        assert_eq!(
            extract_token(&parts),
            Some(("cookie-token".to_string(), AuthSource::Cookie))
        );
    }

    #[test]
    fn empty_cookie_value_is_no_credential() {
        let parts = parts_for(Request::builder().header("Cookie", "nightlio_token="));
        assert_eq!(extract_token(&parts), None);
    }

    #[test]
    fn malformed_authorization_header_falls_back_to_cookie() {
        // "Bearer" without the trailing space does not match startswith("Bearer ").
        let parts = parts_for(
            Request::builder()
                .header("Authorization", "Bearer")
                .header("Cookie", "nightlio_token=tok"),
        );
        assert_eq!(
            extract_token(&parts),
            Some(("tok".to_string(), AuthSource::Cookie))
        );
    }

    #[test]
    fn no_headers_yields_none() {
        let parts = parts_for(Request::builder());
        assert_eq!(extract_token(&parts), None);
    }
}
