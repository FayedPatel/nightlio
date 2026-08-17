//! Port of `api/database_common.py`: the `SQLQueries` string constants and
//! the `DatabaseConnectionMixin` connection helpers, adapted from
//! one-connection-per-call `sqlite3` to an `r2d2` pool.
//!
//! Semantics preserved from the Python side:
//! - every connection sets `PRAGMA foreign_keys=ON` (`_connect`);
//! - a 5s busy timeout, matching `sqlite3.connect`'s default `timeout=5.0`;
//! - `_execute_with_retry`: 3 attempts, retrying only on
//!   "database is locked" with a 0.1s * attempt backoff.
//!
//! Deliberate departure from the Python side (owner-approved, post-cutover):
//! pooled connections switch the main DB to WAL journal mode, with the
//! `-wal` file size bounded two ways:
//! - `wal_autocheckpoint` stays at its 1000-page default (~4 MB), so the
//!   log folds back into the main DB automatically during normal writes;
//! - `PRAGMA journal_size_limit=4194304` truncates the `-wal` file back to
//!   at most 4 MB after checkpoints, so it can never grow without bound
//!   on disk.
//!
//! [`checkpoint_truncate`] additionally folds and truncates the log to zero
//! bytes; `main.rs` runs it once after bootstrap and again on graceful
//! shutdown. The bootstrap's own raw connection is untouched — only pooled
//! connections opt into WAL.

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ValueRef};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

use super::DbPool;

/// Port of `database_common.DatabaseError`. Message prefixes mirror the
/// Python wrapper strings so log-grepping deployments keep working.
#[derive(Debug, thiserror::Error)]
pub enum DatabaseError {
    #[error("Database error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("Database connection failed: {0}")]
    Pool(#[from] r2d2::Error),

    /// Pre-formatted message (retry exhaustion, migration invariants, ...).
    #[error("{0}")]
    Message(String),
}

/// A stored `mood_entries.mood` value, read tolerantly.
///
/// The column is INTEGER-declared with `CHECK (mood >= 1 AND mood <= 5)`,
/// but SQLite's dynamic typing lets a REAL like `4.5` satisfy the CHECK and
/// stay REAL (INTEGER affinity only converts reals that are losslessly
/// integral, so any stored REAL is fractional). Python's `sqlite3` hands
/// such rows to Flask transparently and every endpoint serves 200; reading
/// strictly as `i64` made the Rust port 500 instead. This untagged enum
/// mirrors the dynamic typing: `Int(4)` serializes as `4`, `Float(4.5)` as
/// `4.5` — exactly the JSON Flask emits.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(untagged)]
pub enum MoodValue {
    Int(i64),
    Float(f64),
}

impl MoodValue {
    /// The string Python's `json.dumps` produces for this value as a dict
    /// *key* (`4` -> `"4"`, `4.5` -> `"4.5"`). Whole floats keep their
    /// `.0` (`str(4.0) == "4.0"`) for completeness, although INTEGER
    /// affinity means such values are stored as integers and can't occur.
    pub fn python_key_string(self) -> String {
        match self {
            Self::Int(v) => v.to_string(),
            Self::Float(v) if v == v.trunc() && v.is_finite() => format!("{v:.1}"),
            Self::Float(v) => format!("{v}"),
        }
    }
}

impl FromSql for MoodValue {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value {
            ValueRef::Integer(v) => Ok(Self::Int(v)),
            ValueRef::Real(v) => Ok(Self::Float(v)),
            _ => Err(FromSqlError::InvalidType),
        }
    }
}

/// Verbatim port of `database_common.SQLQueries`. Each constant is built
/// with `concat!` from the exact same string fragments as the Python
/// adjacent-literal concatenation, so the resulting SQL text is
/// byte-identical (including the double-space runs the Python joins
/// produce).
pub mod sql_queries {
    // User queries
    pub const CREATE_USER: &str =
        "INSERT INTO users (google_id, email, name, avatar_url) VALUES (?, ?, ?, ?)";

    pub const GET_USER_BY_GOOGLE_ID: &str = concat!(
        "SELECT id, google_id, email, name, avatar_url, auth_provider, external_id, ",
        "created_at, last_login FROM users WHERE google_id = ?"
    );

    pub const GET_USER_BY_ID: &str = concat!(
        "SELECT id, google_id, email, name, avatar_url, auth_provider, external_id, ",
        "created_at, last_login FROM users WHERE id = ?"
    );

    pub const GET_USER_BY_PROVIDER: &str = concat!(
        "SELECT id, google_id, email, name, avatar_url, auth_provider, external_id, ",
        "created_at, last_login FROM users WHERE auth_provider = ? AND external_id = ?"
    );

    pub const CREATE_PROVIDER_USER: &str = concat!(
        "INSERT INTO users (google_id, email, name, avatar_url, auth_provider, ",
        "external_id, password_hash) VALUES (?, ?, ?, ?, ?, ?, ?)"
    );

    pub const UPSERT_USER: &str = concat!(
        "INSERT INTO users (google_id, email, name, avatar_url) ",
        "VALUES (?, ?, ?, ?) ",
        "ON CONFLICT(google_id) DO UPDATE SET ",
        "  email=COALESCE(excluded.email, users.email), ",
        "  name=COALESCE(excluded.name, users.name), ",
        "  avatar_url=COALESCE(excluded.avatar_url, users.avatar_url), ",
        "  last_login=CURRENT_TIMESTAMP"
    );

    // Goals queries
    // `id ASC` tiebreaker is a deliberate (contract change) addition over the ported
    // Python SQL: rows created within the same second had SQL-unspecified
    // order (flaky under test and across SQLite versions). `id ASC` pins
    // the insertion order SQLite's scan happened to produce when the
    // goals-list golden fixture was recorded, so recorded behavior is now
    // guaranteed instead of accidental.
    pub const GET_GOALS_BY_USER: &str = concat!(
        "SELECT id, user_id, title, description, frequency_per_week, completed, ",
        "       streak, period_start, last_completed_date, created_at, updated_at ",
        "FROM goals WHERE user_id = ? ORDER BY created_at DESC, id ASC"
    );

    pub const GET_GOAL_BY_ID: &str = concat!(
        "SELECT id, user_id, title, description, frequency_per_week, completed, ",
        "       streak, period_start, last_completed_date, created_at, updated_at ",
        "FROM goals WHERE id = ? AND user_id = ?"
    );

    // Mood entries queries
    pub const GET_USER_ENTRY_DATES: &str =
        "SELECT DISTINCT date FROM mood_entries WHERE user_id = ? ORDER BY date DESC";

    pub const GET_MOOD_STATISTICS: &str = concat!(
        "SELECT ",
        "  COUNT(*) as total_entries, ",
        "  AVG(mood) as average_mood, ",
        "  MIN(mood) as lowest_mood, ",
        "  MAX(mood) as highest_mood, ",
        "  MIN(date) as first_entry_date, ",
        "  MAX(date) as last_entry_date ",
        "FROM mood_entries WHERE user_id = ?"
    );
}

/// Apply the per-connection defaults from `DatabaseConnectionMixin._connect`
/// (foreign keys ON, python-sqlite3-equivalent 5s busy timeout).
fn apply_connection_defaults(conn: &Connection) -> rusqlite::Result<()> {
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.execute_batch("PRAGMA foreign_keys=ON")
}

/// Pool-connection setup: the `_connect` defaults plus WAL with a bounded
/// log. `journal_mode=WAL` is persistent (a DB-header field), but is set on
/// every pooled connection so a freshly created file gets it too;
/// `journal_size_limit` is per-connection and must be set on each one.
/// `wal_autocheckpoint` is deliberately left at its 1000-page default
/// (~4 MB): normal write traffic folds the log automatically, and the size
/// limit truncates the `-wal` file back to <=4 MB afterwards.
fn apply_pool_connection_defaults(conn: &Connection) -> rusqlite::Result<()> {
    apply_connection_defaults(conn)?;
    // Both pragmas return a result row; query it rather than execute.
    conn.query_row("PRAGMA journal_mode=WAL", [], |_row| Ok(()))?;
    conn.query_row("PRAGMA journal_size_limit=4194304", [], |_row| Ok(()))?;
    Ok(())
}

/// Fold the entire WAL back into the main DB file and truncate the `-wal`
/// file to zero bytes. Run once after bootstrap (before serving traffic)
/// and on graceful shutdown, so the on-disk log starts and ends empty.
pub fn checkpoint_truncate(pool: &DbPool) -> Result<(), DatabaseError> {
    let conn = pool.get()?;
    // Returns a (busy, log, checkpointed) row.
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_row| Ok(()))?;
    Ok(())
}

/// Ensure the directory the DB file lives in exists — the equivalent of
/// `MoodDatabase.__init__`'s `resolved_path.parent.mkdir(parents=True,
/// exist_ok=True)`.
pub fn ensure_parent_dir(db_path: &str) -> Result<(), DatabaseError> {
    if let Some(parent) = Path::new(db_path).parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|exc| DatabaseError::Message(format!("Database connection failed: {exc}")))?;
    }
    Ok(())
}

/// Build the shared r2d2 pool for the main DB. Every pooled connection gets
/// the `_connect` defaults plus the bounded-WAL setup (see module docs).
pub fn open_pool(db_path: &str) -> Result<DbPool, DatabaseError> {
    ensure_parent_dir(db_path)?;
    let manager = SqliteConnectionManager::file(db_path)
        .with_init(|conn| apply_pool_connection_defaults(conn));
    Ok(r2d2::Pool::builder().build(manager)?)
}

/// One-off connection with the same defaults as the pool — the direct
/// equivalent of `DatabaseConnectionMixin._connect` for code that is not
/// handed a pool (bootstrap seed helpers, tests).
pub fn connect(db_path: &str) -> Result<Connection, DatabaseError> {
    let conn = Connection::open(db_path).map_err(|exc| {
        tracing::error!("Failed to connect to database: {exc}");
        DatabaseError::Message(format!("Database connection failed: {exc}"))
    })?;
    apply_connection_defaults(&conn)?;
    Ok(conn)
}

/// Result of a write statement — stands in for the bits of
/// `sqlite3.Cursor` the Python callers actually use.
#[derive(Debug, Clone, Copy)]
pub struct ExecuteResult {
    pub rows_affected: usize,
    pub last_insert_rowid: i64,
}

/// Port of `_execute_with_retry`: run a single statement, retrying up to 3
/// times when SQLite reports "database is locked", with the same
/// `0.1 * (attempt + 1)` seconds backoff. Each attempt checks a connection
/// out of the pool (the Python opens a fresh connection per attempt).
pub fn execute_with_retry(
    pool: &DbPool,
    query: &str,
    params: &[&dyn rusqlite::ToSql],
) -> Result<ExecuteResult, DatabaseError> {
    const RETRIES: usize = 3;
    for attempt in 0..RETRIES {
        let conn = pool.get()?;
        match conn.execute(query, params) {
            Ok(rows_affected) => {
                return Ok(ExecuteResult {
                    rows_affected,
                    last_insert_rowid: conn.last_insert_rowid(),
                });
            }
            Err(exc) => {
                // Python: `"database is locked" in str(exc)` on
                // OperationalError. rusqlite surfaces the same message text
                // for SQLITE_BUSY, so the substring check carries over.
                if exc.to_string().contains("database is locked") && attempt < RETRIES - 1 {
                    let delay = Duration::from_millis(100 * (attempt as u64 + 1));
                    tracing::warn!(
                        "Database locked (attempt {}). Retrying in {:.2}s...",
                        attempt + 1,
                        delay.as_secs_f64()
                    );
                    std::thread::sleep(delay);
                    continue;
                }
                tracing::error!("Database operation failed: {exc}");
                return Err(DatabaseError::Message(format!(
                    "Database operation failed: {exc}"
                )));
            }
        }
    }
    Err(DatabaseError::Message(
        "Database operation failed after all retries".to_string(),
    ))
}

/// Port of `_get_default_user_id`: id of the seeded self-host user, or None.
pub fn get_default_user_id(
    conn: &Connection,
    default_self_host_id: &str,
) -> Result<Option<i64>, DatabaseError> {
    Ok(conn
        .query_row(
            "SELECT id FROM users WHERE google_id = ?",
            [default_self_host_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?)
}

/// Port of `_resolve_user_id`: `None` means the default self-host user
/// (backward-compat shim for callers that predate per-user scoping).
pub fn resolve_user_id(
    conn: &Connection,
    user_id: Option<i64>,
    default_self_host_id: &str,
) -> Result<Option<i64>, DatabaseError> {
    match user_id {
        Some(id) => Ok(Some(id)),
        None => get_default_user_id(conn, default_self_host_id),
    }
}

/// Column names of a table via `PRAGMA table_info` (empty set = no table).
pub(crate) fn table_columns(conn: &Connection, table: &str) -> rusqlite::Result<HashSet<String>> {
    let safe = table.replace('"', "\"\"");
    let mut stmt = conn.prepare(&format!("PRAGMA table_info(\"{safe}\")"))?;
    let cols = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<HashSet<_>>>()?;
    Ok(cols)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn pool_connections_have_foreign_keys_on_and_wal_journal_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pool.db");
        let pool = open_pool(path.to_str().unwrap()).unwrap();
        let conn = pool.get().unwrap();
        let fk: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(fk, 1, "pool connections must run with foreign_keys=ON");
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal", "pooled main-DB connections must run in WAL");
        let limit: i64 = conn
            .query_row("PRAGMA journal_size_limit", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            limit, 4_194_304,
            "journal_size_limit must bound the -wal file to 4 MB"
        );
        let autockpt: i64 = conn
            .query_row("PRAGMA wal_autocheckpoint", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            autockpt, 1000,
            "wal_autocheckpoint must stay at its 1000-page default"
        );
    }

    /// The `-wal` file is bounded on disk: grow it past the 4 MB
    /// `journal_size_limit` inside one big transaction (auto-checkpoint
    /// can't run mid-transaction), then `checkpoint_truncate` and assert
    /// the on-disk `-wal` file is back within the limit.
    #[test]
    fn checkpoint_truncate_bounds_wal_file_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("walsize.db");
        let pool = open_pool(path.to_str().unwrap()).unwrap();
        let wal_path = dir.path().join("walsize.db-wal");

        {
            let mut conn = pool.get().unwrap();
            conn.execute_batch("CREATE TABLE blobs (data BLOB)")
                .unwrap();
            let tx = conn.transaction().unwrap();
            // 8 MB of pages in one transaction: well past the 4 MB limit.
            let chunk = vec![0xABu8; 64 * 1024];
            for _ in 0..128 {
                tx.execute("INSERT INTO blobs (data) VALUES (?)", [&chunk])
                    .unwrap();
            }
            // Measure before commit: WAL frames are appended as the
            // transaction writes, and the commit itself fires the
            // autocheckpoint (whose truncation is part of what's under
            // test — asserting growth after commit would race it).
            let grown = std::fs::metadata(&wal_path).unwrap().len();
            assert!(
                grown > 4_194_304,
                "test must actually grow the -wal past journal_size_limit, got {grown} bytes"
            );
            tx.commit().unwrap();
        }

        checkpoint_truncate(&pool).unwrap();

        let truncated = std::fs::metadata(&wal_path).unwrap().len();
        assert!(
            truncated <= 4_194_304,
            "-wal file must be <= journal_size_limit after checkpoint, got {truncated} bytes"
        );
    }

    #[test]
    fn open_pool_creates_missing_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/data/nightlio.db");
        let pool = open_pool(path.to_str().unwrap()).unwrap();
        drop(pool);
        assert!(path.exists());
    }

    #[test]
    fn execute_with_retry_tries_three_times_on_database_locked() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("locked.db");

        // Set up a table, then hold a RESERVED (write) lock from a side
        // connection via BEGIN IMMEDIATE. Deliberately not EXCLUSIVE:
        // r2d2_sqlite's default `is-valid` feature runs "SELECT 1;" at
        // checkout, and an EXCLUSIVE lock blocks even that read, wedging
        // `pool.get()` instead of exercising the statement-level retry
        // this test is about. A RESERVED lock lets reads through while
        // any write still fails with "database is locked".
        let setup = Connection::open(&path).unwrap();
        setup.execute_batch("CREATE TABLE t (x INTEGER)").unwrap();
        setup
            .execute_batch("BEGIN IMMEDIATE; INSERT INTO t (x) VALUES (0)")
            .unwrap();

        // Pool with a tiny busy timeout so each locked attempt fails fast
        // (the retry loop under test is the application-level one).
        let manager = SqliteConnectionManager::file(&path)
            .with_init(|conn| conn.busy_timeout(Duration::from_millis(5)));
        let pool = r2d2::Pool::builder().build(manager).unwrap();

        let started = Instant::now();
        let err = execute_with_retry(&pool, "INSERT INTO t (x) VALUES (1)", &[]).unwrap_err();
        let elapsed = started.elapsed();

        assert!(
            err.to_string().starts_with("Database operation failed:"),
            "unexpected error: {err}"
        );
        assert!(
            err.to_string().contains("database is locked"),
            "unexpected error: {err}"
        );
        // Two backoffs happened before the final attempt: 0.1s + 0.2s.
        assert!(
            elapsed >= Duration::from_millis(300),
            "expected 3 attempts with 0.1s/0.2s backoff, finished in {elapsed:?}"
        );

        setup.execute_batch("ROLLBACK").unwrap();
        // With the lock released the same call succeeds.
        let ok = execute_with_retry(&pool, "INSERT INTO t (x) VALUES (1)", &[]).unwrap();
        assert_eq!(ok.rows_affected, 1);
        assert_eq!(ok.last_insert_rowid, 1);
    }
}
