//! PostgreSQL backend plumbing (v0.6.0, opt-in via `DATABASE_URL`).
//!
//! Ships labeled **experimental**: SQLite remains the default and the only
//! e2e-graded backend in 0.6.0. This module owns infrastructure only — the
//! deadpool connection pool and its rustls TLS wiring. The async query
//! twins that make the store facade's `Pg` arms real land in a later
//! workstream; until then a booted `Pg` handle serves the routes that don't
//! touch the store and returns clear errors from the ones that do.

pub mod pool;

pub mod achievements;
pub mod activity;
pub mod bootstrap;
pub mod data;
pub mod goals;
pub mod groups;
pub mod moods;
pub mod stats;
pub mod users;
pub mod util;

pub use pool::build_pool;
