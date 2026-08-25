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

use crate::config::{Config, DatabaseTarget};

pub mod achievements;
pub mod activity;
pub mod bootstrap;
pub mod common;
pub mod data;
pub mod goals;
pub mod groups;
pub mod migrate;
pub mod moods;
pub mod pg;
pub mod stats;
pub mod store;
pub mod users;

pub use bootstrap::{SelfHostSeed, bootstrap};
pub use common::{DatabaseError, checkpoint_truncate, connect, execute_with_retry, open_pool};

/// The shared SQLite connection pool (the only backend the query layer
/// currently speaks; every existing query method still runs on this).
pub type SqlitePool = r2d2::Pool<SqliteConnectionManager>;

/// A checked-out SQLite connection.
pub type DbConnection = r2d2::PooledConnection<SqliteConnectionManager>;

/// Boot the configured database backend exactly the way the server does —
/// the single startup path shared by `serve()`, the `seed-demo` subcommand,
/// and the integration tests (so a seeded database is always the database
/// the server would have produced).
///
/// - SQLite: legacy `bootstrap()` (the idempotent v0→v3 upgrade path for
///   every file in the wild), then the migration runner adopts the file and
///   applies anything beyond the baseline, then the serving pool opens and
///   the WAL is folded once so the `-wal` file starts at zero bytes.
/// - Postgres: build the (lazy) pool, run migrations (`0001_baseline`
///   bootstraps a fresh database; the first real connection, so an
///   unreachable server fails fast with an actionable message), then seed
///   the self-host baseline the SQLite bootstrap seeds — the default user
///   and default tag groups (idempotent; backfills existing deploys).
pub async fn connect_from_config(cfg: &Config) -> anyhow::Result<DbHandle> {
    match cfg.database_target()? {
        DatabaseTarget::Sqlite => {
            bootstrap(&cfg.database_path, &SelfHostSeed::from(cfg))?;
            migrate::run_sqlite(&cfg.database_path)?;
            let db = DbHandle::Sqlite(open_pool(&cfg.database_path)?);
            // The pool just switched the main DB to WAL; fold and truncate
            // the log once before it serves traffic.
            checkpoint_truncate(&db)?;
            Ok(db)
        }
        DatabaseTarget::Postgres(url) => {
            tracing::warn!(
                "PostgreSQL backend selected via DATABASE_URL — experimental in v0.6.0; \
                 SQLite remains the default and the only e2e-graded backend \
                 (see docs/POSTGRES.md)"
            );
            let pool = pg::build_pool(&url)?;
            migrate::run_postgres(&pool).await?;
            pg::bootstrap::ensure_selfhost_baseline(&pool, &SelfHostSeed::from(cfg)).await?;
            Ok(DbHandle::Pg(pool))
        }
    }
}

/// The database handle carried by `AppState` — the single chokepoint where
/// the backend is selected (v0.6.0, opt-in Postgres via `DATABASE_URL`).
///
/// The `Pg` variant is constructible but not bootable yet: the pool builder,
/// the `main.rs` Postgres branch, and the async store facade land in later
/// v0.6.0 workstreams. Until then, every SQLite-only code path that receives
/// a `Pg` handle fails with [`DbHandle::pg_not_wired`] — a clear
/// `DatabaseError`, never a panic or a silent fallback to SQLite.
#[derive(Clone)]
pub enum DbHandle {
    Sqlite(SqlitePool),
    Pg(deadpool_postgres::Pool),
}

impl DbHandle {
    /// Borrow the SQLite pool behind this handle. Fails on `Pg` — callers
    /// are SQLite-only paths that predate the Postgres backend.
    pub fn sqlite_pool(&self) -> Result<&SqlitePool, DatabaseError> {
        match self {
            DbHandle::Sqlite(pool) => Ok(pool),
            DbHandle::Pg(_) => Err(Self::pg_not_wired()),
        }
    }

    /// Check a connection out of the SQLite pool (the direct replacement for
    /// the old `pool.get()` call sites). Fails on `Pg`.
    pub fn get(&self) -> Result<DbConnection, DatabaseError> {
        Ok(self.sqlite_pool()?.get()?)
    }

    /// The error every SQLite-only path returns when handed a `Pg` handle.
    pub fn pg_not_wired() -> DatabaseError {
        DatabaseError::Message(
            "postgres backend not yet wired: this code path only supports the \
             SQLite backend in this build"
                .to_string(),
        )
    }
}
