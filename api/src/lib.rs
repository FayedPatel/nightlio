//! Nightlio API — Rust port of the Flask backend under `api/`.
//!
//! Library crate so integration tests and the thin `main.rs` binary share
//! the same modules. Module ownership:
//! - `config` / `error` / `state`: scaffold (this crate's foundation).
//! - `db`: data-layer agent (bootstrap/migrations, per-mixin query ports).
//! - `auth`: auth agent (JWT, local login, OIDC).
//! - `routes`: routes agent (axum handlers mirroring `api/routes/*.py`).

pub mod auth;
pub mod config;
pub mod db;
pub mod error;
pub mod routes;
pub mod state;
