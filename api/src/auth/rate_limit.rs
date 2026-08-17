//! SQLite-backed sliding-window rate limiter — port of
//! `api/utils/rate_limiter.py`.
//!
//! Storage is a SEPARATE SQLite file (`Config::rate_limit_db_path`, never
//! the main app DB), matching the Python module's isolation: rate-limit
//! bookkeeping churn never contends with app writes. (Since the
//! owner-approved WAL change in `db::common`, the main DB also runs in WAL
//! on pooled connections — the isolation here predates that and stands on
//! its own.)
//!
//! Semantics preserved from the Python module:
//! - one short-lived connection per check (as `_get_connection` does), WAL,
//!   5000 ms busy timeout, table + index created on demand;
//! - the delete-old / count / insert sequence runs inside a single
//!   `BEGIN IMMEDIATE` transaction so check-then-act is atomic across
//!   processes sharing the file (the Flask app may still be running against
//!   the same `rate_limit.db` during cutover);
//! - buckets are keyed `"{module}.{qualname}:{ip}"` with the PYTHON module
//!   and qualname strings kept verbatim ([`LOGIN_BUCKET_PREFIX`] /
//!   [`REGISTER_BUCKET_PREFIX`]) so in-flight Flask counters survive the
//!   cutover and both servers share one budget;
//! - `X-Forwarded-For` is honored only when `TRUST_PROXY_HEADERS` is truthy
//!   (first comma-separated value), else the TCP peer address, else
//!   `"unknown"` ([`client_ip`]);
//! - FAIL OPEN on any storage error ([`allow_request`]): a broken rate-limit
//!   store must not take down login/registration for every user. This is a
//!   deliberate tradeoff — document it if you touch this file, don't
//!   silently change it to fail closed;
//! - the `TESTING` bypass (Flask `current_app.config.get("TESTING")`) is
//!   enforced by the calling route (`AppEnv::Testing`), matching where the
//!   Python decorator checks it.
//!
//! These functions are synchronous and do blocking file I/O — HTTP callers
//! must run them under `tokio::task::spawn_blocking`.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::Connection;

/// `f"{f.__module__}.{f.__qualname__}"` for the login view. Verbatim from
/// the Python decoration site (`api/routes/auth_routes.py::local_login`,
/// defined inside `create_auth_routes`) — do NOT "clean up" the
/// `<locals>` segment or live Flask buckets stop matching.
pub const LOGIN_BUCKET_PREFIX: &str =
    "api.routes.auth_routes.create_auth_routes.<locals>.local_login";

/// `f"{f.__module__}.{f.__qualname__}"` for the register view. Verbatim —
/// see [`LOGIN_BUCKET_PREFIX`].
pub const REGISTER_BUCKET_PREFIX: &str =
    "api.routes.auth_routes.create_auth_routes.<locals>.local_register";

/// Exact 429 body message (`api/utils/rate_limiter.py`).
pub const RATE_LIMIT_MESSAGE: &str = "Rate limit exceeded. Please try again later.";

/// `@rate_limit(max_requests=30, window_minutes=1)` on `local_login`.
pub const LOGIN_MAX_REQUESTS: i64 = 30;

/// `@rate_limit(max_requests=10, window_minutes=1)` on `local_register`.
pub const REGISTER_MAX_REQUESTS: i64 = 10;

/// Both auth endpoints use `window_minutes=1`.
pub const AUTH_WINDOW_SECONDS: f64 = 60.0;

/// `_BUSY_TIMEOUT_MS`: how long a connection waits on another process's
/// lock before erroring (and thus failing open).
const BUSY_TIMEOUT_MS: u64 = 5000;

/// Port of `_client_ip`: resolve the caller's IP for bucketing.
///
/// `X-Forwarded-For` is trivially spoofable by a direct client, so it is
/// honored only when the operator opted in via `TRUST_PROXY_HEADERS`
/// (first comma-separated value, stripped — the client end of the chain,
/// matching the Python's `forwarded.split(",")[0].strip()`). Otherwise the
/// actual TCP peer address (`request.remote_addr`), or `"unknown"` when
/// even that is unavailable.
pub fn client_ip(
    trust_proxy_headers: bool,
    x_forwarded_for: Option<&str>,
    remote_addr: Option<&str>,
) -> String {
    if trust_proxy_headers
        && let Some(forwarded) = x_forwarded_for
        && !forwarded.is_empty()
    {
        return forwarded.split(',').next().unwrap_or("").trim().to_string();
    }
    match remote_addr.filter(|addr| !addr.is_empty()) {
        Some(addr) => addr.to_string(),
        None => "unknown".to_string(),
    }
}

/// The full storage key for one caller of one endpoint:
/// `"{bucket_prefix}:{ip}"`.
pub fn bucket_key(bucket_prefix: &str, ip: &str) -> String {
    format!("{bucket_prefix}:{ip}")
}

/// Port of `_get_connection`: open (creating parent directories and the
/// schema on demand), set the busy timeout, switch to WAL.
fn open_connection(db_path: &str) -> rusqlite::Result<Connection> {
    // os.makedirs(directory, exist_ok=True). A failure here (e.g. an
    // unwritable parent) surfaces as the open error below and rides the
    // same fail-open path.
    if let Some(parent) = Path::new(db_path).parent()
        && !parent.as_os_str().is_empty()
    {
        let _ = std::fs::create_dir_all(parent);
    }

    let conn = Connection::open(db_path)?;
    // PRAGMA busy_timeout = 5000 (also covers the Python connect timeout).
    conn.busy_timeout(Duration::from_millis(BUSY_TIMEOUT_MS))?;
    // `PRAGMA journal_mode = WAL` returns a result row; query it like the
    // Python's cursor-based execute does.
    conn.query_row("PRAGMA journal_mode = WAL", [], |row| {
        row.get::<_, String>(0)
    })?;
    conn.execute(
        "CREATE TABLE IF NOT EXISTS rate_limit_events (key TEXT NOT NULL, ts REAL NOT NULL)",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_rate_limit_events_key_ts \
         ON rate_limit_events (key, ts)",
        [],
    )?;
    Ok(conn)
}

/// `time.time()` — float epoch seconds (the `ts REAL` column).
fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs_f64())
        .unwrap_or(0.0)
}

/// Port of `_check_and_record`: atomically check-and-record one request
/// against the sliding window.
///
/// Returns `Ok(true)` if the request is allowed (and has been recorded),
/// `Ok(false)` if the caller is at/over the cap (nothing recorded). Storage
/// failures are `Err`; callers decide the failure mode ([`allow_request`]
/// fails open).
pub fn check_and_record(
    db_path: &str,
    key: &str,
    max_requests: i64,
    window_seconds: f64,
) -> rusqlite::Result<bool> {
    let now = now_epoch_seconds();
    let window_start = now - window_seconds;

    let conn = open_connection(db_path)?;
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| -> rusqlite::Result<bool> {
        // Opportunistic prune: drop this key's aged-out events so no
        // separate cleanup job is needed.
        conn.execute(
            "DELETE FROM rate_limit_events WHERE key = ? AND ts < ?",
            rusqlite::params![key, window_start],
        )?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM rate_limit_events WHERE key = ? AND ts >= ?",
            rusqlite::params![key, window_start],
            |row| row.get(0),
        )?;
        if count >= max_requests {
            conn.execute_batch("COMMIT")?;
            return Ok(false);
        }
        conn.execute(
            "INSERT INTO rate_limit_events (key, ts) VALUES (?, ?)",
            rusqlite::params![key, now],
        )?;
        conn.execute_batch("COMMIT")?;
        Ok(true)
    })();
    if result.is_err() {
        let _ = conn.execute_batch("ROLLBACK");
    }
    result
}

/// [`check_and_record`] with the FAIL-OPEN policy applied: any storage
/// error allows the request through with a warning logged, exactly like the
/// Python decorator's `except sqlite3.Error` branch. See the module doc
/// before changing this.
pub fn allow_request(db_path: &str, key: &str, max_requests: i64, window_seconds: f64) -> bool {
    match check_and_record(db_path, key, max_requests, window_seconds) {
        Ok(allowed) => allowed,
        Err(exc) => {
            tracing::warn!(
                error = %exc,
                "Rate limiter storage error; allowing request through (fail-open)"
            );
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    fn temp_db() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir
            .path()
            .join("rate_limit.db")
            .to_string_lossy()
            .into_owned();
        (dir, path)
    }

    #[test]
    fn allows_up_to_cap_then_rejects() {
        let (_dir, db) = temp_db();
        let key = bucket_key(LOGIN_BUCKET_PREFIX, "1.2.3.4");
        for i in 0..5 {
            assert!(check_and_record(&db, &key, 5, 60.0).unwrap(), "req {i}");
        }
        assert!(!check_and_record(&db, &key, 5, 60.0).unwrap());
        // Rejected requests are not recorded: still exactly 5 rows.
        let conn = Connection::open(&db).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM rate_limit_events WHERE key = ?",
                [&key],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 5);
    }

    #[test]
    fn buckets_are_isolated_per_endpoint_and_ip() {
        let (_dir, db) = temp_db();
        let login_a = bucket_key(LOGIN_BUCKET_PREFIX, "1.1.1.1");
        let register_a = bucket_key(REGISTER_BUCKET_PREFIX, "1.1.1.1");
        let login_b = bucket_key(LOGIN_BUCKET_PREFIX, "2.2.2.2");
        for _ in 0..3 {
            assert!(check_and_record(&db, &login_a, 3, 60.0).unwrap());
        }
        assert!(!check_and_record(&db, &login_a, 3, 60.0).unwrap());
        // Same IP, different endpoint: fresh budget.
        assert!(check_and_record(&db, &register_a, 3, 60.0).unwrap());
        // Same endpoint, different IP: fresh budget.
        assert!(check_and_record(&db, &login_b, 3, 60.0).unwrap());
    }

    #[test]
    fn window_slides_old_events_are_pruned() {
        let (_dir, db) = temp_db();
        let key = "k";
        for _ in 0..3 {
            assert!(check_and_record(&db, key, 3, 60.0).unwrap());
        }
        assert!(!check_and_record(&db, key, 3, 60.0).unwrap());
        // Age every event out of the window; the next check prunes them
        // and allows again.
        let conn = Connection::open(&db).unwrap();
        conn.execute("UPDATE rate_limit_events SET ts = ts - 61.0", [])
            .unwrap();
        drop(conn);
        assert!(check_and_record(&db, key, 3, 60.0).unwrap());
        let conn = Connection::open(&db).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM rate_limit_events", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1, "aged-out events must be pruned");
    }

    #[test]
    fn fails_open_when_storage_is_unusable() {
        // A directory path cannot be opened as a SQLite database: the
        // check errors, and allow_request lets the request through.
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().to_string_lossy().into_owned();
        assert!(check_and_record(&db, "k", 1, 60.0).is_err());
        assert!(allow_request(&db, "k", 1, 60.0));
        // A working store still enforces the cap through allow_request.
        let (_dir2, ok_db) = temp_db();
        assert!(allow_request(&ok_db, "k", 1, 60.0));
        assert!(!allow_request(&ok_db, "k", 1, 60.0));
    }

    #[test]
    fn store_is_wal_and_survives_concurrent_connections() {
        let (_dir, db) = temp_db();
        assert!(check_and_record(&db, "k", 5, 60.0).unwrap());
        let conn = Connection::open(&db).unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
    }

    #[test]
    fn bucket_prefixes_are_verbatim_python_qualnames() {
        // Live Flask buckets must keep matching across the cutover.
        assert_eq!(
            LOGIN_BUCKET_PREFIX,
            "api.routes.auth_routes.create_auth_routes.<locals>.local_login"
        );
        assert_eq!(
            REGISTER_BUCKET_PREFIX,
            "api.routes.auth_routes.create_auth_routes.<locals>.local_register"
        );
        assert_eq!(
            bucket_key(LOGIN_BUCKET_PREFIX, "10.0.0.1"),
            "api.routes.auth_routes.create_auth_routes.<locals>.local_login:10.0.0.1"
        );
    }

    #[rstest]
    // Untrusted: the header is ignored even when present.
    #[case::untrusted_header_ignored(false, Some("9.9.9.9"), Some("1.2.3.4"), "1.2.3.4")]
    #[case::untrusted_no_peer(false, Some("9.9.9.9"), None, "unknown")]
    // Trusted: first comma-separated value, stripped.
    #[case::trusted_single(true, Some("9.9.9.9"), Some("1.2.3.4"), "9.9.9.9")]
    #[case::trusted_chain(true, Some(" 9.9.9.9 , 8.8.8.8"), Some("1.2.3.4"), "9.9.9.9")]
    // Trusted but header absent/empty: fall back to the peer address.
    #[case::trusted_absent(true, None, Some("1.2.3.4"), "1.2.3.4")]
    #[case::trusted_empty(true, Some(""), Some("1.2.3.4"), "1.2.3.4")]
    #[case::trusted_absent_no_peer(true, None, None, "unknown")]
    #[case::empty_peer_is_unknown(false, None, Some(""), "unknown")]
    fn client_ip_resolution(
        #[case] trust: bool,
        #[case] xff: Option<&str>,
        #[case] peer: Option<&str>,
        #[case] expected: &str,
    ) {
        assert_eq!(client_ip(trust, xff, peer), expected);
    }
}
