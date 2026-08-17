//! The `nightlio_token` session cookie — port of
//! `api/utils/auth_cookies.py` (`set_auth_cookie` / `clear_auth_cookie` /
//! `_is_https_request`).
//!
//! Attributes: `HttpOnly`, `SameSite=Lax`, `Path=/`, `Max-Age` mirroring
//! the JWT's 3600 s expiry. `Secure` is computed **per request**, never
//! from static config: `is_secure OR (TRUST_PROXY_HEADERS truthy AND first
//! X-Forwarded-Proto value == "https")`. Callers derive `is_secure` the
//! way Flask does behind `ProxyFix(x_proto=1)`: when the proxy is trusted,
//! from the RIGHTMOST `X-Forwarded-Proto` value (see the `request_is_secure`
//! helpers in `routes::auth` / `auth::oidc`). Getting `Secure` wrong is worse
//! than a logout — browsers refuse to transmit a `Secure` cookie over
//! plain HTTP at all, so a plain-HTTP self-hoster whose cookie is marked
//! `Secure` gets a silent login loop.

use cookie::time::{Duration, OffsetDateTime};
use cookie::{Cookie, SameSite};

use crate::config::JWT_ACCESS_TOKEN_EXPIRES_SECS;

/// Cookie name (`api/utils/auth_cookies.py::COOKIE_NAME`).
pub const COOKIE_NAME: &str = "nightlio_token";

/// Port of `_is_https_request`: best-effort HTTPS detection, proxy-aware
/// only when opted in.
///
/// - `is_secure`: Flask's `request.is_secure` — for this plain-HTTP server
///   that means the ProxyFix-derived value: `TRUST_PROXY_HEADERS` truthy
///   AND the rightmost `X-Forwarded-Proto` value is exactly `https`
///   (werkzeug compares the rewritten scheme case-sensitively).
/// - `trust_proxy_headers`: the already-parsed `TRUST_PROXY_HEADERS` flag
///   (`crate::config::is_truthy` semantics).
/// - `x_forwarded_proto`: the raw `X-Forwarded-Proto` header, `None` when
///   absent. Only its first comma-separated value counts, trimmed and
///   compared lowercase.
pub fn is_https_request(
    is_secure: bool,
    trust_proxy_headers: bool,
    x_forwarded_proto: Option<&str>,
) -> bool {
    if is_secure {
        return true;
    }
    if trust_proxy_headers {
        let first = x_forwarded_proto
            .unwrap_or("")
            .split(',')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        return first == "https";
    }
    false
}

/// Build the session cookie carrying `token`, as `set_auth_cookie` does:
/// `HttpOnly; SameSite=Lax; Path=/; Max-Age=<max_age_seconds>`, plus
/// `Secure` when `secure` (computed per request via [`is_https_request`]).
/// `max_age_seconds` mirrors the JWT expiry —
/// [`auth_cookie`](crate::auth::cookie::auth_cookie) callers pass
/// [`JWT_ACCESS_TOKEN_EXPIRES_SECS`] so the cookie never outlives the
/// token it carries.
pub fn build_auth_cookie(token: &str, max_age_seconds: i64, secure: bool) -> Cookie<'static> {
    Cookie::build((COOKIE_NAME, token.to_owned()))
        .max_age(Duration::seconds(max_age_seconds))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(secure)
        .build()
}

/// [`build_auth_cookie`] with the standard JWT-mirroring Max-Age (3600 s).
pub fn auth_cookie(token: &str, secure: bool) -> Cookie<'static> {
    build_auth_cookie(token, JWT_ACCESS_TOKEN_EXPIRES_SECS as i64, secure)
}

/// The clearing variant (`clear_auth_cookie`, used by logout — which
/// requires no auth and is idempotent): same name/path/flags, empty value,
/// `Max-Age=0` and `Expires` at the Unix epoch, matching Flask's
/// `set_cookie(..., max_age=0, expires=0)`.
pub fn clear_auth_cookie(secure: bool) -> Cookie<'static> {
    Cookie::build((COOKIE_NAME, ""))
        .max_age(Duration::ZERO)
        .expires(OffsetDateTime::UNIX_EPOCH)
        .path("/")
        .http_only(true)
        .same_site(SameSite::Lax)
        .secure(secure)
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    // Direct TLS always wins, regardless of proxy settings.
    #[case::direct_tls(true, false, None, true)]
    #[case::direct_tls_trusting_proxy(true, true, Some("http"), true)]
    // Plain HTTP without proxy trust: headers are ignored.
    #[case::plain_http(false, false, None, false)]
    #[case::spoofed_header_untrusted(false, false, Some("https"), false)]
    // Proxy trust enabled: this pure function only applies the
    // first-value fallback; callers additionally pass the
    // ProxyFix-rightmost result as `is_secure` (see request_is_secure in
    // routes::auth / auth::oidc), so `http,https` IS Secure end-to-end.
    #[case::proxy_https(false, true, Some("https"), true)]
    #[case::proxy_https_mixed_case(false, true, Some("HTTPS"), true)]
    #[case::proxy_https_padded(false, true, Some("  https , http"), true)]
    #[case::proxy_http(false, true, Some("http"), false)]
    #[case::proxy_http_first_fallback_only(false, true, Some("http,https"), false)]
    #[case::proxy_http_first_with_proxyfix_is_secure(true, true, Some("http,https"), true)]
    #[case::proxy_no_header(false, true, None, false)]
    #[case::proxy_empty_header(false, true, Some(""), false)]
    fn https_detection(
        #[case] is_secure: bool,
        #[case] trust_proxy: bool,
        #[case] xfp: Option<&str>,
        #[case] expected: bool,
    ) {
        assert_eq!(is_https_request(is_secure, trust_proxy, xfp), expected);
    }

    #[test]
    fn auth_cookie_attributes() {
        let cookie = auth_cookie("some.jwt.token", false);
        assert_eq!(cookie.name(), "nightlio_token");
        assert_eq!(cookie.value(), "some.jwt.token");
        assert_eq!(cookie.path(), Some("/"));
        assert_eq!(cookie.http_only(), Some(true));
        assert_eq!(cookie.same_site(), Some(SameSite::Lax));
        assert_eq!(cookie.max_age(), Some(Duration::seconds(3600)));
        // Non-HTTPS request: Secure must be absent or login over plain
        // HTTP silently breaks.
        assert_ne!(cookie.secure(), Some(true));
    }

    #[test]
    fn auth_cookie_attribute_string_plain_http() {
        let s = auth_cookie("tok", false).to_string();
        assert!(s.starts_with("nightlio_token=tok;"), "got: {s}");
        assert!(s.contains("HttpOnly"), "got: {s}");
        assert!(s.contains("SameSite=Lax"), "got: {s}");
        assert!(s.contains("Path=/"), "got: {s}");
        assert!(s.contains("Max-Age=3600"), "got: {s}");
        assert!(!s.contains("Secure"), "got: {s}");
        assert!(!s.contains("Domain"), "got: {s}");
    }

    #[test]
    fn auth_cookie_attribute_string_https() {
        let s = auth_cookie("tok", true).to_string();
        assert!(s.contains("Secure"), "got: {s}");
        assert!(s.contains("HttpOnly"), "got: {s}");
        assert!(s.contains("SameSite=Lax"), "got: {s}");
        assert!(s.contains("Max-Age=3600"), "got: {s}");
    }

    #[test]
    fn custom_max_age_is_respected() {
        let cookie = build_auth_cookie("tok", 60, false);
        assert_eq!(cookie.max_age(), Some(Duration::seconds(60)));
    }

    #[test]
    fn clear_cookie_expires_immediately() {
        let cookie = clear_auth_cookie(false);
        assert_eq!(cookie.name(), "nightlio_token");
        assert_eq!(cookie.value(), "");
        assert_eq!(cookie.path(), Some("/"));
        assert_eq!(cookie.http_only(), Some(true));
        assert_eq!(cookie.same_site(), Some(SameSite::Lax));
        assert_eq!(cookie.max_age(), Some(Duration::ZERO));
        assert_eq!(cookie.expires_datetime(), Some(OffsetDateTime::UNIX_EPOCH));

        let s = cookie.to_string();
        assert!(s.starts_with("nightlio_token=;"), "got: {s}");
        assert!(s.contains("Max-Age=0"), "got: {s}");
        assert!(s.contains("Expires="), "got: {s}");
    }

    #[test]
    fn clear_cookie_secure_variant() {
        assert_eq!(clear_auth_cookie(true).secure(), Some(true));
        assert_ne!(clear_auth_cookie(false).secure(), Some(true));
    }
}
