//! JWT issue/verify — byte-compatible port of `generate_jwt_token` in
//! `api/routes/auth_routes.py` and the decode path in
//! `api/utils/auth_middleware.py`.
//!
//! Contract (live sessions must survive cutover):
//! - HS256, signed with `JWT_SECRET_KEY`.
//! - Claims are exactly `{user_id, exp, iat}` — no `iss`/`aud`/`sub`/`jti`.
//! - Expiry is `JWT_ACCESS_TOKEN_EXPIRES` (3600 s), leeway 0 (python-jose's
//!   default is zero leeway; `jsonwebtoken`'s default of 60 s must be
//!   overridden or Rust would accept tokens Flask rejects).
//! - `required_spec_claims` trimmed to `exp`; aud/iss validation OFF. With
//!   `jsonwebtoken`'s default `validate_aud = true` an `aud`-bearing token
//!   would be rejected even with `aud: None` configured, so it is disabled
//!   explicitly.
//! - Error strings are exact: `Token expired` for an expired signature,
//!   `Invalid token` for everything else (Flask matches "expired" in the
//!   jose error message; every other `JWTError` becomes "Invalid token").

use jsonwebtoken::{
    Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode, errors::ErrorKind,
};
use serde::{Deserialize, Serialize};

use crate::config::JWT_ACCESS_TOKEN_EXPIRES_SECS;

/// Exact 401 body message for an expired token
/// (`api/utils/auth_middleware.py`).
pub const TOKEN_EXPIRED_MESSAGE: &str = "Token expired";

/// Exact 401 body message for any other verification failure.
pub const INVALID_TOKEN_MESSAGE: &str = "Invalid token";

/// The full JWT claim set — nothing more, nothing less.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claims {
    /// Nightlio user id (SQLite rowid of the `users` table).
    pub user_id: i64,
    /// Expiry, seconds since the Unix epoch.
    pub exp: i64,
    /// Issued-at, seconds since the Unix epoch.
    pub iat: i64,
}

/// Verification failure, carrying the exact Flask error message as its
/// `Display` form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum VerifyError {
    /// The signature was valid but `exp` is in the past (leeway 0).
    #[error("Token expired")]
    Expired,
    /// Malformed token, bad signature, missing `exp`, wrong algorithm, ...
    #[error("Invalid token")]
    Invalid,
}

impl VerifyError {
    /// The exact message for the JSON error body (`{"error": "..."}`).
    pub fn message(self) -> &'static str {
        match self {
            VerifyError::Expired => TOKEN_EXPIRED_MESSAGE,
            VerifyError::Invalid => INVALID_TOKEN_MESSAGE,
        }
    }
}

/// Issue a token for `user_id` as of `now_unix` (seconds since the epoch):
/// `iat = now`, `exp = now + JWT_ACCESS_TOKEN_EXPIRES` (3600 s). Pure —
/// callers pass the clock so tests can pin it.
pub fn issue_token_at(secret: &str, user_id: i64, now_unix: i64) -> anyhow::Result<String> {
    let claims = Claims {
        user_id,
        exp: now_unix + JWT_ACCESS_TOKEN_EXPIRES_SECS as i64,
        iat: now_unix,
    };
    // Header::default() is {"typ": "JWT", "alg": "HS256"} — the same header
    // python-jose emits.
    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;
    Ok(token)
}

/// Issue a token for `user_id` using the current wall clock.
pub fn issue_token(secret: &str, user_id: i64) -> anyhow::Result<String> {
    issue_token_at(secret, user_id, chrono::Utc::now().timestamp())
}

/// Verify `token` and return its claims, or the exact Flask-shaped error.
pub fn verify_token(secret: &str, token: &str) -> Result<Claims, VerifyError> {
    let mut validation = Validation::new(Algorithm::HS256);
    // python-jose applies no leeway; jsonwebtoken defaults to 60 s.
    validation.leeway = 0;
    // Only `exp` is required. `Validation::new` already sets exactly this,
    // but the contract is important enough to pin explicitly.
    validation.set_required_spec_claims(&["exp"]);
    // Off, not merely unset: with validate_aud = true (the default) and
    // aud = None, jsonwebtoken rejects any token that carries an `aud`
    // claim. python-jose performs no aud/iss checks unless configured to.
    validation.validate_aud = false;

    decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map(|data| data.claims)
    .map_err(|err| match err.kind() {
        ErrorKind::ExpiredSignature => VerifyError::Expired,
        _ => VerifyError::Invalid,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "test-cross-verification-secret";

    /// Pinned python-jose tokens generated with the Flask venv (the exact
    /// library the production Flask app verified tokens with) while it
    /// still existed. See the `_comment` field for provenance; the
    /// `python_jose_verdict` on each record is what Flask's decode call did
    /// with the token at generation time.
    const JWT_FIXTURE: &str = include_str!("../../tests/fixtures/jwt_python_jose.json");

    /// Rollback-safety records: Rust-generated artifacts that the real
    /// python-jose/werkzeug accepted at generation time.
    const ACCEPTANCE_FIXTURE: &str = include_str!("../../tests/fixtures/acceptance.json");

    /// Look up a pinned python-jose token by fixture name.
    fn fixture_token(name: &str) -> String {
        let fixture: serde_json::Value = serde_json::from_str(JWT_FIXTURE).unwrap();
        let record = fixture["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("no fixture token named {name}"));
        record["token"].as_str().unwrap().to_string()
    }

    #[test]
    fn claims_serialize_to_exactly_user_id_exp_iat() {
        let value = serde_json::to_value(Claims {
            user_id: 7,
            exp: 2,
            iat: 1,
        })
        .unwrap();
        let obj = value.as_object().unwrap();
        let mut keys: Vec<_> = obj.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["exp", "iat", "user_id"]);
    }

    #[test]
    fn round_trip_within_rust() {
        let now = chrono::Utc::now().timestamp();
        let token = issue_token_at(SECRET, 42, now).unwrap();
        let claims = verify_token(SECRET, &token).unwrap();
        assert_eq!(
            claims,
            Claims {
                user_id: 42,
                exp: now + 3600,
                iat: now
            }
        );
    }

    /// Token pinned from python-jose in the Flask venv:
    /// `jwt.encode({"user_id": 42, "exp": 4102444800, "iat": 1700000000},
    /// "test-cross-verification-secret", algorithm="HS256")`. Keeps the
    /// python->rust direction covered even without the venv present.
    #[test]
    fn verifies_pinned_python_jose_token() {
        let token = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.\
                     eyJ1c2VyX2lkIjo0MiwiZXhwIjo0MTAyNDQ0ODAwLCJpYXQiOjE3MDAwMDAwMDB9.\
                     EfFfrZQYAIjUcnqdBss9BNJbDVg8eDrOnMxf8wxbCJ0";
        let claims = verify_token(SECRET, token).unwrap();
        assert_eq!(
            claims,
            Claims {
                user_id: 42,
                exp: 4_102_444_800,
                iat: 1_700_000_000
            }
        );
    }

    #[test]
    fn verifies_fixture_token_issued_by_python_jose() {
        let claims = verify_token(SECRET, &fixture_token("valid")).unwrap();
        assert_eq!(
            claims,
            Claims {
                user_id: 1337,
                exp: 4_102_444_800,
                iat: 1_700_000_000
            }
        );
    }

    /// The rust->python direction, pinned: this token was issued by
    /// `issue_token_at(SECRET, 99, 4102441200)` and decoded by the venv's
    /// python-jose at fixture-generation time (`python_jose_accepted:
    /// true`, claims checked to be exactly `{user_id, exp, iat}`). Here we
    /// re-assert the byte-level facts: the pinned token still verifies and
    /// today's issuer still produces those exact bytes.
    #[test]
    fn python_jose_accepted_rust_issued_token() {
        let fixture: serde_json::Value = serde_json::from_str(ACCEPTANCE_FIXTURE).unwrap();
        let record = &fixture["python_jose_jwt"];
        assert_eq!(record["python_jose_accepted"], serde_json::json!(true));
        assert_eq!(record["secret"].as_str().unwrap(), SECRET);
        let token = record["token"].as_str().unwrap();

        let claims = verify_token(SECRET, token).unwrap();
        assert_eq!(
            claims,
            Claims {
                user_id: 99,
                exp: 4_102_444_800,
                iat: 4_102_441_200
            }
        );
        // HS256 signing is deterministic: the issuer must still emit the
        // exact bytes python-jose accepted.
        assert_eq!(issue_token_at(SECRET, 99, 4_102_441_200).unwrap(), token);
    }

    #[test]
    fn fixture_expired_token_yields_expired() {
        assert_eq!(
            verify_token(SECRET, &fixture_token("expired")).unwrap_err(),
            VerifyError::Expired
        );
    }

    #[test]
    fn fixture_wrong_secret_token_yields_invalid() {
        assert_eq!(
            verify_token(SECRET, &fixture_token("wrong_secret")).unwrap_err(),
            VerifyError::Invalid
        );
    }

    /// Documented divergence, pinned: python-jose accepts float `exp`/`iat`
    /// (it coerces to int), but `Claims` deserializes them as `i64`, so
    /// Rust rejects with `Invalid`. Harmless in practice — Flask's issuer
    /// passes datetimes, which python-jose serializes as ints, so no real
    /// token ever carries a float.
    #[test]
    fn float_exp_iat_token_is_invalid_here_though_jose_accepts_it() {
        assert_eq!(
            verify_token(SECRET, &fixture_token("float_exp_iat")).unwrap_err(),
            VerifyError::Invalid
        );
    }

    #[test]
    fn expired_token_yields_exact_expired_error() {
        // Issued two hours ago: exp is one hour in the past.
        let now = chrono::Utc::now().timestamp();
        let token = issue_token_at(SECRET, 1, now - 7200).unwrap();
        let err = verify_token(SECRET, &token).unwrap_err();
        assert_eq!(err, VerifyError::Expired);
        assert_eq!(err.message(), "Token expired");
        assert_eq!(err.to_string(), "Token expired");
    }

    #[test]
    fn leeway_is_zero_a_barely_expired_token_is_rejected() {
        // exp = now - 2: inside jsonwebtoken's default 60 s leeway, which
        // python-jose does not grant. Must be rejected.
        let now = chrono::Utc::now().timestamp();
        let token = issue_token_at(SECRET, 1, now - 3602).unwrap();
        assert_eq!(
            verify_token(SECRET, &token).unwrap_err(),
            VerifyError::Expired
        );
    }

    #[test]
    fn wrong_secret_yields_exact_invalid_error() {
        let token = issue_token(SECRET, 1).unwrap();
        let err = verify_token("some-other-secret", &token).unwrap_err();
        assert_eq!(err, VerifyError::Invalid);
        assert_eq!(err.message(), "Invalid token");
        assert_eq!(err.to_string(), "Invalid token");
    }

    #[test]
    fn garbage_and_tampered_tokens_are_invalid() {
        assert_eq!(
            verify_token(SECRET, "not-a-jwt").unwrap_err(),
            VerifyError::Invalid
        );
        let mut token = issue_token(SECRET, 1).unwrap();
        // Flip the last signature character.
        let last = if token.ends_with('A') { 'B' } else { 'A' };
        token.pop();
        token.push(last);
        assert_eq!(
            verify_token(SECRET, &token).unwrap_err(),
            VerifyError::Invalid
        );
    }

    #[test]
    fn token_with_aud_claim_still_verifies() {
        // Guards the validate_aud = false requirement: with jsonwebtoken's
        // default `validate_aud = true` (and `aud: None`), an `aud`-bearing
        // token is rejected. (python-jose itself, called the way Flask
        // calls it, rejects this token with JWTClaimsError — real Nightlio
        // tokens never carry `aud`, so the divergence is deliberate; see
        // the fixture note.)
        let token = fixture_token("aud_bearing");
        assert_eq!(verify_token(SECRET, &token).unwrap().user_id, 5);
    }
}
