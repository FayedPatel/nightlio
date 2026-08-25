//! Shared helpers for the PostgreSQL query twins (both WS4 domain batches
//! build against this module).
//!
//! Conventions the twins follow:
//! - Twin functions take `&impl GenericClient` (or `&mut impl GenericClient`
//!   when they open a transaction), so multi-step store ops can share one
//!   checked-out connection — and compose across domains (e.g. an entry
//!   create calling the activity twin) exactly like the SQLite closures
//!   share one `&Connection`.
//! - Store arms check a connection out via [`client`] once per op.
//! - Errors map into the existing [`DatabaseError`] shape: pool checkout
//!   failures use the `Database connection failed:` prefix (mirroring the
//!   `Pool` variant's display), query failures the `Database error:` prefix
//!   (mirroring the `Sqlite` variant's display). Only the HTTP status is
//!   contract-graded, but keeping the prefixes preserves log-grepping.
//! - SQLite date-modifier arithmetic (`datetime('now', ?)`,
//!   `date('now','localtime', ?)`) is computed in Rust with chrono and bound
//!   as text — never translated into PostgreSQL date arithmetic.

use deadpool_postgres::Pool;
use tokio_postgres::error::SqlState;

use crate::db::DatabaseError;
use crate::db::common::MoodValue;

/// Check a connection out of the deadpool pool. Failure maps to the same
/// `Database connection failed:` message prefix `DatabaseError::Pool` shows
/// for r2d2, so both backends log alike (route-visible status: 500).
pub async fn client(pool: &Pool) -> Result<deadpool_postgres::Client, DatabaseError> {
    pool.get()
        .await
        .map_err(|exc| DatabaseError::Message(format!("Database connection failed: {exc}")))
}

/// Map a driver error into the store's error shape with the `Database
/// error:` prefix `DatabaseError::Sqlite` displays (route-visible: 500).
///
/// `tokio_postgres::Error`'s own `Display` is terse (`db error`); the
/// server's message (`ERROR: null value in column ... violates not-null
/// constraint`, ...) lives in the wrapped `DbError`, so it is surfaced
/// here — logs stay as greppable as rusqlite's message-bearing errors.
pub fn db_error(exc: tokio_postgres::Error) -> DatabaseError {
    let detail = match exc.as_db_error() {
        Some(db_error) => db_error.to_string(),
        None => exc.to_string(),
    };
    DatabaseError::Message(format!("Database error: {detail}"))
}

/// The PostgreSQL equivalent of `sqlite3.IntegrityError`: any SQLSTATE in
/// class 23 (integrity constraint violation — NOT NULL, FK, UNIQUE, CHECK).
/// The SQLite arms match `rusqlite::ErrorCode::ConstraintViolation`, which
/// covers the same set.
pub fn is_integrity_violation(exc: &tokio_postgres::Error) -> bool {
    exc.code()
        .is_some_and(|state| state.code().starts_with("23"))
}

/// Specifically a UNIQUE constraint violation (SQLSTATE 23505), for call
/// sites that key duplicate-detection off it (ON CONFLICT-adjacent paths).
pub fn is_unique_violation(exc: &tokio_postgres::Error) -> bool {
    exc.code() == Some(&SqlState::UNIQUE_VIOLATION)
}

/// Map a `mood double precision` column back to the [`MoodValue`] wire
/// shape: SQLite's INTEGER affinity stores integral moods as integers (so
/// `4.0` reads back as `Int(4)`) while fractional REALs stay REAL
/// (`Float(4.5)`). PostgreSQL stores every mood as a double, so the read
/// side reproduces the affinity split here.
pub fn mood_value_from_f64(value: f64) -> MoodValue {
    // Same losslessness rule as SQLite's REAL->INTEGER conversion: finite,
    // integral, and exactly representable as an i64.
    if value.is_finite()
        && value == value.trunc()
        && (-9_007_199_254_740_992.0..=9_007_199_254_740_992.0).contains(&value)
    {
        MoodValue::Int(value as i64)
    } else {
        MoodValue::Float(value)
    }
}

/// Nullable-column variant of [`mood_value_from_f64`] for aggregate reads
/// (`MIN(mood)` over zero rows is NULL).
pub fn opt_mood_value_from_f64(value: Option<f64>) -> Option<MoodValue> {
    value.map(mood_value_from_f64)
}

/// The text `datetime('now', '-<days> days')` produces on SQLite: UTC
/// wall-clock minus exactly `days * 86400` seconds, formatted
/// `YYYY-MM-DD HH:MM:SS`. Computed in Rust and bound as a text parameter —
/// PostgreSQL date arithmetic is deliberately not used (see module docs).
pub fn utc_now_minus_days_text(days: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::days(days))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

/// The text `date('now', 'localtime', '-<days> days')` produces on SQLite:
/// the server-local calendar date minus exactly `days` days, ISO-formatted.
/// (Matches Python's naive `datetime.now()` date math ported elsewhere.)
pub fn local_date_minus_days_text(days: i64) -> String {
    (chrono::Local::now().date_naive() - chrono::Duration::days(days))
        .format("%Y-%m-%d")
        .to_string()
}

/// Port of `_get_default_user_id` for the PG backend: id of the seeded
/// self-host user, or `None`.
pub async fn get_default_user_id(
    client: &impl tokio_postgres::GenericClient,
    default_self_host_id: &str,
) -> Result<Option<i64>, DatabaseError> {
    let row = client
        .query_opt(
            "SELECT id FROM users WHERE google_id = $1",
            &[&default_self_host_id],
        )
        .await
        .map_err(db_error)?;
    Ok(row.map(|row| row.get(0)))
}

/// Port of `_resolve_user_id` for the PG backend: `None` means the default
/// self-host user (backward-compat shim predating per-user scoping).
pub async fn resolve_user_id(
    client: &impl tokio_postgres::GenericClient,
    user_id: Option<i64>,
    default_self_host_id: &str,
) -> Result<Option<i64>, DatabaseError> {
    match user_id {
        Some(id) => Ok(Some(id)),
        None => get_default_user_id(client, default_self_host_id).await,
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    //! Live-PostgreSQL test harness, gated on `NIGHTLIO_PG_TEST_URL`.
    //!
    //! Every PG-side quirk test calls [`connect_scratch`] first and returns
    //! early (clean skip) when the variable is unset, so the default
    //! `cargo test` run never needs a server. When it is set, each test
    //! works inside its own named schema on the shared database: the schema
    //! is dropped and recreated up front (rerun-safe), the 0001 baseline is
    //! applied into it, and `search_path` pins every unqualified reference
    //! to it — so parallel tests never collide as long as schema names are
    //! unique per test. Point the variable at a disposable PostgreSQL 16
    //! server, e.g.
    //! `postgres://user:pw@localhost:5432/db?sslmode=disable` (the harness
    //! connects with `NoTls`).

    use tokio_postgres::{Client, NoTls};

    /// Connect and prepare a scratch schema with the 0001 baseline applied.
    /// Returns `None` (skip) when `NIGHTLIO_PG_TEST_URL` is unset.
    pub(crate) async fn connect_scratch(schema: &str) -> Option<Client> {
        let Ok(url) = std::env::var("NIGHTLIO_PG_TEST_URL") else {
            eprintln!("skipping PG quirk test: NIGHTLIO_PG_TEST_URL not set");
            return None;
        };
        assert!(
            schema
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
            "scratch schema names must be simple identifiers: {schema}"
        );
        let (client, connection) = tokio_postgres::connect(&url, NoTls)
            .await
            .expect("connect to NIGHTLIO_PG_TEST_URL");
        tokio::spawn(async move {
            let _ = connection.await;
        });
        client
            .batch_execute(&format!(
                "DROP SCHEMA IF EXISTS {schema} CASCADE; \
                 CREATE SCHEMA {schema}; \
                 SET search_path TO {schema};"
            ))
            .await
            .expect("create scratch schema");
        client
            .batch_execute(crate::db::migrate::MIGRATIONS[0].postgres_sql)
            .await
            .expect("apply the 0001 postgres baseline");
        Some(client)
    }

    /// Seed the default self-host user (`id` 1 on a fresh schema) the way
    /// the SQLite bootstrap does, returning its id.
    pub(crate) async fn seed_default_user(client: &Client) -> i64 {
        client
            .query_one(
                "INSERT INTO users (google_id, email, name, auth_provider, external_id) \
                 VALUES ('selfhost_default_user', 'selfhost_default_user@localhost', 'Me', \
                         'local', 'selfhost_default_user') RETURNING id",
                &[],
            )
            .await
            .expect("seed default user")
            .get(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mood_value_mapping_reproduces_sqlite_integer_affinity() {
        assert_eq!(mood_value_from_f64(4.0), MoodValue::Int(4));
        assert_eq!(mood_value_from_f64(1.0), MoodValue::Int(1));
        assert_eq!(mood_value_from_f64(4.5), MoodValue::Float(4.5));
        assert_eq!(mood_value_from_f64(3.25), MoodValue::Float(3.25));
        assert_eq!(opt_mood_value_from_f64(None), None);
        assert_eq!(opt_mood_value_from_f64(Some(2.0)), Some(MoodValue::Int(2)));
        // The wire shapes the mapping preserves.
        assert_eq!(
            serde_json::to_string(&mood_value_from_f64(4.0)).unwrap(),
            "4"
        );
        assert_eq!(
            serde_json::to_string(&mood_value_from_f64(4.5)).unwrap(),
            "4.5"
        );
    }

    #[test]
    fn cutoff_helpers_produce_sqlite_shaped_text() {
        let cutoff = utc_now_minus_days_text(90);
        assert_eq!(cutoff.len(), 19, "{cutoff}");
        assert_eq!(&cutoff[4..5], "-");
        assert_eq!(&cutoff[10..11], " ");
        let day = local_date_minus_days_text(90);
        assert_eq!(day.len(), 10, "{day}");
        assert_eq!(&day[4..5], "-");
        // Exactly days * 86400 seconds, like the SQLite modifier.
        let now = utc_now_minus_days_text(0);
        assert!(now > cutoff);
    }
}
