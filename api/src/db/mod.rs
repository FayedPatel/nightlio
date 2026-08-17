//! Data layer — port of `api/database*.py` (the `MoodDatabase` facade and
//! its mixins: users, moods, groups, goals, achievements, activity, stats).
//!
//! Owned by the data-layer agent. Contract for that work:
//! - `rusqlite` with the `bundled` feature only (SQLite version is pinned;
//!   GLOB/printf/instr/julianday/strftime('%w') and ON CONFLICT semantics
//!   must not drift under a system libsqlite3).
//! - All blocking calls run under `tokio::task::spawn_blocking`.
//! - The startup `bootstrap()` must port `api/database_schema.py::
//!   init_database()` verbatim (ordering, backfills, groups rebuild under
//!   `PRAGMA foreign_keys=OFF` before any transaction opens).
//! - Main DB runs in WAL journal mode on pooled connections (owner-approved
//!   post-cutover change), with the `-wal` file bounded: default 1000-page
//!   `wal_autocheckpoint`, `journal_size_limit=4194304`, and a
//!   `wal_checkpoint(TRUNCATE)` after bootstrap and on graceful shutdown.
//!   The bootstrap's dedicated raw connection does not touch journal mode.

use r2d2_sqlite::SqliteConnectionManager;

pub mod achievements;
pub mod activity;
pub mod bootstrap;
pub mod common;
pub mod goals;
pub mod groups;
pub mod moods;
pub mod stats;
pub mod users;

pub use bootstrap::{SelfHostSeed, bootstrap};
pub use common::{DatabaseError, checkpoint_truncate, connect, execute_with_retry, open_pool};

/// Shared connection pool handed to every query method.
pub type DbPool = r2d2::Pool<SqliteConnectionManager>;

/// A checked-out connection.
pub type DbConnection = r2d2::PooledConnection<SqliteConnectionManager>;
