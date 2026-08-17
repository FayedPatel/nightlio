//! The CSRF predicate — a pure port of `_csrf_check_failure` and the
//! surrounding gating in `api/utils/auth_middleware.py`.
//!
//! It applies only to cookie-authenticated requests using a state-changing
//! method ({POST, PUT, PATCH, DELETE}). Bearer-token requests bypass it
//! entirely: a token living in another origin's localStorage cannot be
//! attached by a cross-site page, so there is nothing for CSRF to exploit.
//! Check order matters and is user-visible (the 403 body differs):
//! Content-Type first, then the `X-Requested-With` header.

/// Header name a cookie-authed mutation must carry
/// (`api/utils/auth_cookies.py::CSRF_HEADER_NAME`).
pub const CSRF_HEADER_NAME: &str = "X-Requested-With";

/// Required literal value of that header (case-sensitive).
pub const CSRF_HEADER_VALUE: &str = "nightlio";

/// Exact 403 body message when the content type is not JSON.
pub const CONTENT_TYPE_MESSAGE: &str = "Content-Type must be application/json";

/// Exact 403 body message when the `X-Requested-With` header is wrong.
pub const MISSING_HEADER_MESSAGE: &str = "Missing required request header";

/// Methods that mutate state (`_STATE_CHANGING_METHODS`).
pub const STATE_CHANGING_METHODS: [&str; 4] = ["POST", "PUT", "PATCH", "DELETE"];

/// How the request authenticated. Bearer always beats cookie — the
/// extractor never downgrades a `Authorization: Bearer` request to cookie
/// auth, so a Bearer request is [`AuthSource::Bearer`] even if a cookie is
/// also present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthSource {
    /// `Authorization: Bearer <token>` header.
    Bearer,
    /// The httpOnly `nightlio_token` cookie.
    Cookie,
}

/// A failed CSRF check; `Display` is the exact Flask 403 body message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CsrfRejection {
    /// Content-Type's first `;`-segment, trimmed and lowercased, was not
    /// `application/json`.
    #[error("Content-Type must be application/json")]
    ContentType,
    /// `X-Requested-With` was absent or not exactly `nightlio`.
    #[error("Missing required request header")]
    MissingHeader,
}

impl CsrfRejection {
    /// The exact message for the JSON error body (`{"error": "..."}`).
    pub fn message(self) -> &'static str {
        match self {
            CsrfRejection::ContentType => CONTENT_TYPE_MESSAGE,
            CsrfRejection::MissingHeader => MISSING_HEADER_MESSAGE,
        }
    }
}

/// The predicate. `method` is the request method's canonical uppercase
/// name (as `http::Method::as_str` yields); `content_type` and
/// `x_requested_with` are the raw header values, `None` when absent.
///
/// Returns `Ok(())` when the request may proceed, `Err` carrying the exact
/// 403 message otherwise.
pub fn csrf_check(
    method: &str,
    auth_source: AuthSource,
    content_type: Option<&str>,
    x_requested_with: Option<&str>,
) -> Result<(), CsrfRejection> {
    if auth_source == AuthSource::Bearer {
        return Ok(());
    }
    if !STATE_CHANGING_METHODS.contains(&method) {
        return Ok(());
    }
    // (request.content_type or "").split(";")[0].strip().lower()
    let first_segment = content_type
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if first_segment != "application/json" {
        return Err(CsrfRejection::ContentType);
    }
    // Exact, case-sensitive comparison against the literal "nightlio".
    if x_requested_with != Some(CSRF_HEADER_VALUE) {
        return Err(CsrfRejection::MissingHeader);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::AuthSource::{Bearer, Cookie};
    use super::*;
    use rstest::rstest;

    const JSON: Option<&str> = Some("application/json");
    const HDR: Option<&str> = Some("nightlio");

    #[rstest]
    // Bearer bypasses everything, even the worst-case request shape.
    #[case::bearer_post_bare("POST", Bearer, None, None, Ok(()))]
    #[case::bearer_delete_form("DELETE", Bearer, Some("application/x-www-form-urlencoded"), None, Ok(()))]
    #[case::bearer_put_wrong_header("PUT", Bearer, Some("text/plain"), Some("Nightlio"), Ok(()))]
    // Non-state-changing methods are exempt regardless of auth source.
    #[case::cookie_get("GET", Cookie, None, None, Ok(()))]
    #[case::cookie_head("HEAD", Cookie, None, None, Ok(()))]
    #[case::cookie_options("OPTIONS", Cookie, None, None, Ok(()))]
    // Cookie + mutation + both checks passing.
    #[case::cookie_post_ok("POST", Cookie, JSON, HDR, Ok(()))]
    #[case::cookie_put_ok("PUT", Cookie, JSON, HDR, Ok(()))]
    #[case::cookie_patch_ok("PATCH", Cookie, JSON, HDR, Ok(()))]
    #[case::cookie_delete_ok("DELETE", Cookie, JSON, HDR, Ok(()))]
    // Content-Type normalization: first ;-segment, trimmed, lowercased.
    #[case::charset_suffix("POST", Cookie, Some("application/json; charset=utf-8"), HDR, Ok(()))]
    #[case::mixed_case("POST", Cookie, Some("Application/JSON"), HDR, Ok(()))]
    #[case::padded("POST", Cookie, Some("  application/json  ; x=y"), HDR, Ok(()))]
    // Content-Type failures — checked FIRST, so the header being right
    // (or wrong) is irrelevant to which 403 you get.
    #[case::no_content_type("POST", Cookie, None, HDR, Err(CsrfRejection::ContentType))]
    #[case::empty_content_type("POST", Cookie, Some(""), HDR, Err(CsrfRejection::ContentType))]
    #[case::form(
        "POST",
        Cookie,
        Some("application/x-www-form-urlencoded"),
        HDR,
        Err(CsrfRejection::ContentType)
    )]
    #[case::text_plain(
        "DELETE",
        Cookie,
        Some("text/plain"),
        None,
        Err(CsrfRejection::ContentType)
    )]
    #[case::json_suffix_mime(
        "PUT",
        Cookie,
        Some("application/vnd.api+json"),
        HDR,
        Err(CsrfRejection::ContentType)
    )]
    // Header failures — only reachable once Content-Type passes.
    #[case::missing_header("POST", Cookie, JSON, None, Err(CsrfRejection::MissingHeader))]
    #[case::empty_header("POST", Cookie, JSON, Some(""), Err(CsrfRejection::MissingHeader))]
    #[case::wrong_value(
        "PATCH",
        Cookie,
        JSON,
        Some("XMLHttpRequest"),
        Err(CsrfRejection::MissingHeader)
    )]
    #[case::value_is_case_sensitive(
        "DELETE",
        Cookie,
        JSON,
        Some("Nightlio"),
        Err(CsrfRejection::MissingHeader)
    )]
    #[case::padded_value(
        "POST",
        Cookie,
        JSON,
        Some(" nightlio "),
        Err(CsrfRejection::MissingHeader)
    )]
    // Method names are canonical uppercase; a hypothetical lowercase
    // string is not in the set (axum/http always hands us uppercase).
    #[case::lowercase_method_not_matched("post", Cookie, None, None, Ok(()))]
    fn table(
        #[case] method: &str,
        #[case] auth: AuthSource,
        #[case] content_type: Option<&str>,
        #[case] header: Option<&str>,
        #[case] expected: Result<(), CsrfRejection>,
    ) {
        assert_eq!(csrf_check(method, auth, content_type, header), expected);
    }

    #[test]
    fn rejection_messages_are_exact() {
        assert_eq!(
            CsrfRejection::ContentType.message(),
            "Content-Type must be application/json"
        );
        assert_eq!(
            CsrfRejection::MissingHeader.message(),
            "Missing required request header"
        );
        // Display mirrors message() so error plumbing can't drift.
        assert_eq!(CsrfRejection::ContentType.to_string(), CONTENT_TYPE_MESSAGE);
        assert_eq!(
            CsrfRejection::MissingHeader.to_string(),
            MISSING_HEADER_MESSAGE
        );
    }
}
