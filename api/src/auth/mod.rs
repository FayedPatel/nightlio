//! Authentication — port of `api/routes/auth_routes.py`,
//! `api/utils/auth_middleware.py`, `api/utils/auth_cookies.py`, and the
//! werkzeug-compatible password hashing (scrypt / pbkdf2:sha256).
//!
//! Owned by the auth agent. Headless core only for now — pure modules with
//! parity tests against the Flask implementation; nothing here is wired
//! into `main.rs` or the routes yet.

pub mod cookie;
pub mod csrf;
pub mod extract;
pub mod jwt;
pub mod oidc;
pub mod password;
pub mod rate_limit;
