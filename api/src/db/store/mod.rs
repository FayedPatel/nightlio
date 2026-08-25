//! Route-level data-store facade — the single dispatch point between the
//! SQLite backend (the existing synchronous `db/*.rs` modules, run under
//! `spawn_blocking`) and the opt-in Postgres backend (v0.6.0).
//!
//! Every operation is a `pub async fn` taking a [`crate::db::DbHandle`] plus the
//! route's parameters and matching on the backend:
//! - `Sqlite`: the exact closure body the route handler used to run inline
//!   under `spawn_blocking`, moved here verbatim — same statements, same
//!   error mapping, same ordering — so fixtures and insta snapshots stay
//!   byte-identical.
//! - `Pg`: natively async calls into the `db/pg` query twins, which
//!   translate each statement per the wire-quirk policy in
//!   docs/plans/v0.6.0.md so the JSON the routes emit is byte-identical
//!   across backends.
//!
//! Modules mirror the `db/*.rs` domains; each op lives with the domain that
//! owns its primary table. Multi-step compositions (entry create + activity
//! log + achievement check, ...) stay intact inside a single op.

pub mod achievements;
pub mod activity;
pub mod data;
pub mod goals;
pub mod groups;
pub mod moods;
pub mod stats;
pub mod users;

use rusqlite::Connection;

use crate::db::{DatabaseError, SqlitePool};
use crate::error::{ApiError, ApiResult};

/// Run a blocking data-layer closure on the pool via `spawn_blocking`.
pub(crate) async fn with_db<T, F>(pool: &SqlitePool, f: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce(&Connection) -> ApiResult<T> + Send + 'static,
{
    let pool = pool.clone();
    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        f(&conn)
    })
    .await
    .map_err(|exc| ApiError::Internal(anyhow::anyhow!("blocking task failed: {exc}")))?
}

/// Run a data-layer call on the blocking pool (`spawn_blocking`), checking a
/// connection out of the shared pool inside the task.
pub(crate) async fn run_db<T, F>(pool: &SqlitePool, func: F) -> Result<T, DatabaseError>
where
    T: Send + 'static,
    F: FnOnce(&Connection) -> Result<T, DatabaseError> + Send + 'static,
{
    let pool = pool.clone();
    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        func(&conn)
    })
    .await
    .map_err(|exc| DatabaseError::Message(format!("Database error: blocking task failed: {exc}")))?
}

/// Data-layer failure → 500 (the Flask generic `except Exception` branch;
/// only the status is parity-relevant, the Python message text is not).
pub(crate) fn db_err(error: DatabaseError) -> ApiError {
    match error {
        DatabaseError::Sqlite(inner) => ApiError::Database(inner),
        DatabaseError::Pool(inner) => ApiError::Pool(inner),
        DatabaseError::Message(message) => ApiError::Internal(anyhow::anyhow!(message)),
    }
}

/// Data-layer failure where `Message` is a ported Python `ValueError`
/// (e.g. `month must be between 1 and 12` from the digest) → 400.
pub(crate) fn value_err(error: DatabaseError) -> ApiError {
    match error {
        DatabaseError::Message(message) => ApiError::Validation(message),
        other => db_err(other),
    }
}
