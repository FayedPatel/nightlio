//! Shared backend parameterization for the integration harnesses (v0.6.0).
//!
//! Every harness's `make_app` family is parameterized over [`Backend`]:
//! the SQLite case runs exactly as it always has (tempfile database,
//! legacy bootstrap), and a PostgreSQL twin activates only when
//! `NIGHTLIO_PG_TEST_URL` is set — unset, [`backends`] prints a visible
//! skip notice and local runs stay green with no PostgreSQL installed.
//! The payoff: the same recorded fixtures replay byte-identically against
//! both backends. A PG mismatch is a query-twin bug (`api/src/db/pg/`),
//! never a reason to touch a fixture.
//!
//! Isolation model (template-database cloning):
//! - Once per test binary, a template database
//!   (`nightlio_tpl_<crate>`) is dropped, recreated, and migrated through
//!   the real runner (`db::migrate::run_postgres`, i.e. the 0001 postgres
//!   baseline) — so the schema under test is exactly the schema the app
//!   boots with.
//! - Each PG-backed app then gets its own private database via
//!   `CREATE DATABASE … TEMPLATE …` (paying the migration cost once, not
//!   per test), seeded with the bootstrap-equivalent baseline the SQLite
//!   harnesses inherit from `db::bootstrap`: the default self-host user
//!   (id 1) and the default groups/options (ids 1..=3 / 1..=27), pinned by
//!   assertion so fixture ids line up on both backends.
//! - Names are deterministic (`nightlio_t_<crate>_<n>`), and creation
//!   always starts with `DROP DATABASE IF EXISTS … WITH (FORCE)`, so
//!   re-runs are self-cleaning. Point `NIGHTLIO_PG_TEST_URL` at a
//!   disposable PostgreSQL 16+ server whose user may create databases
//!   (e.g. `postgres://user:pw@localhost:5432/db?sslmode=disable`; the
//!   harness connects with `NoTls`, so use `sslmode=disable`).
//!
//! This module is included by several test binaries that each use a
//! different subset of it, hence the file-wide `dead_code` allowance.
#![allow(dead_code)]

use std::sync::atomic::{AtomicU32, Ordering};

use nightlio_api::db::bootstrap::{DEFAULT_GROUPS, SelfHostSeed};
use tokio_postgres::{Client, NoTls};

/// Which database backend a harness instance runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    Sqlite,
    Pg,
}

/// The backends available in this environment: SQLite always; PostgreSQL
/// only when `NIGHTLIO_PG_TEST_URL` is set (with a visible skip notice
/// otherwise, so a green local run says what it did not cover).
pub fn backends() -> Vec<Backend> {
    if pg_test_url().is_some() {
        vec![Backend::Sqlite, Backend::Pg]
    } else {
        eprintln!("skipping PostgreSQL backend cases: NIGHTLIO_PG_TEST_URL not set");
        vec![Backend::Sqlite]
    }
}

/// `NIGHTLIO_PG_TEST_URL`, or `None` (skip) when unset/empty.
pub fn pg_test_url() -> Option<String> {
    std::env::var("NIGHTLIO_PG_TEST_URL")
        .ok()
        .filter(|url| !url.is_empty())
}

/// The database an app under test runs on, keeping whatever must stay
/// alive for the test's duration (the tempdir also hosts the rate-limiter
/// store and the i18n disk cache on both backends).
pub enum TestDb {
    Sqlite {
        path: String,
        _dir: tempfile::TempDir,
    },
    Pg {
        url: String,
        _dir: tempfile::TempDir,
    },
}

impl TestDb {
    pub fn backend(&self) -> Backend {
        match self {
            TestDb::Sqlite { .. } => Backend::Sqlite,
            TestDb::Pg { .. } => Backend::Pg,
        }
    }

    /// Human label for assertion messages.
    pub fn label(&self) -> &'static str {
        match self {
            TestDb::Sqlite { .. } => "sqlite",
            TestDb::Pg { .. } => "postgres",
        }
    }

    /// Path of the SQLite file (panics on the PG variant — callers match
    /// on the enum first).
    pub fn sqlite_path(&self) -> &str {
        match self {
            TestDb::Sqlite { path, .. } => path,
            TestDb::Pg { .. } => panic!("sqlite_path() called on a PostgreSQL TestDb"),
        }
    }

    /// Direct client to this test's private PostgreSQL database (panics on
    /// the SQLite variant — callers match on the enum first).
    pub async fn pg_client(&self) -> Client {
        match self {
            TestDb::Pg { url, .. } => pg_connect(url).await,
            TestDb::Sqlite { .. } => panic!("pg_client() called on a SQLite TestDb"),
        }
    }
}

/// Connect to a PostgreSQL URL with `NoTls`, driving the connection on a
/// background task (the standard tokio-postgres shape).
pub async fn pg_connect(url: &str) -> Client {
    let (client, connection) = tokio_postgres::connect(url, NoTls)
        .await
        .expect("connect to the test PostgreSQL server");
    tokio::spawn(async move {
        let _ = connection.await;
    });
    client
}

/// Swap the database name in a `postgres://user:pw@host:port/db[?query]`
/// URL, preserving credentials, host, port, and query parameters.
pub fn with_dbname(url: &str, dbname: &str) -> String {
    let (base, query) = match url.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (url, None),
    };
    let scheme_end = base.find("://").map(|idx| idx + 3).unwrap_or(0);
    let after_scheme = &base[scheme_end..];
    let authority = match after_scheme.find('/') {
        Some(idx) => &after_scheme[..idx],
        None => after_scheme,
    };
    let mut out = format!("{}{}/{}", &base[..scheme_end], authority, dbname);
    if let Some(query) = query {
        out.push('?');
        out.push_str(query);
    }
    out
}

/// Assert a generated database name is a simple identifier (they are all
/// interpolated into DDL, which takes no bound parameters).
fn assert_simple_identifier(name: &str) {
    assert!(
        !name.is_empty()
            && name.len() <= 63
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'),
        "test database names must be simple identifiers: {name}"
    );
}

/// All test-harness DDL (DROP/CREATE DATABASE) serializes on this advisory
/// lock so parallel tests never race PostgreSQL's template bookkeeping.
async fn with_harness_lock<T>(admin: &Client, work: impl std::future::Future<Output = T>) -> T {
    admin
        .batch_execute("SELECT pg_advisory_lock(hashtext('nightlio_test_harness'))")
        .await
        .expect("acquire the test-harness advisory lock");
    let out = work.await;
    // Best-effort unlock; dropping the session releases it regardless.
    let _ = admin
        .batch_execute("SELECT pg_advisory_unlock(hashtext('nightlio_test_harness'))")
        .await;
    out
}

/// Per-process template bootstrap (name memoized in a `OnceCell` so the
/// migration cost is paid once per test binary).
static TEMPLATE: tokio::sync::OnceCell<String> = tokio::sync::OnceCell::const_new();

async fn template_name(admin_url: &str) -> &'static str {
    TEMPLATE
        .get_or_init(|| async { build_template(admin_url).await })
        .await
}

async fn build_template(admin_url: &str) -> String {
    let tpl = format!("nightlio_tpl_{}", env!("CARGO_CRATE_NAME"));
    assert_simple_identifier(&tpl);
    let admin = pg_connect(admin_url).await;
    with_harness_lock(&admin, async {
        admin
            .batch_execute(&format!("DROP DATABASE IF EXISTS {tpl} WITH (FORCE)"))
            .await
            .expect("drop a stale template database");
        admin
            .batch_execute(&format!("CREATE DATABASE {tpl}"))
            .await
            .expect("create the template database");
    })
    .await;

    // Apply the real migrations (the 0001 postgres baseline) through the
    // production runner, then make sure no connection lingers on the
    // template — `CREATE DATABASE … TEMPLATE …` requires zero sessions.
    let tpl_url = with_dbname(admin_url, &tpl);
    {
        let pool = nightlio_api::db::pg::build_pool(&tpl_url).expect("template pool");
        nightlio_api::db::migrate::run_postgres(&pool)
            .await
            .expect("apply migrations to the template database");
    }
    for attempt in 0..100u32 {
        let _ = admin
            .query(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
                 WHERE datname = $1 AND pid <> pg_backend_pid()",
                &[&tpl],
            )
            .await;
        let remaining: i64 = admin
            .query_one(
                "SELECT count(*) FROM pg_stat_activity WHERE datname = $1",
                &[&tpl],
            )
            .await
            .expect("count template sessions")
            .get(0);
        if remaining == 0 {
            return tpl;
        }
        assert!(attempt < 99, "template database sessions never drained");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    tpl
}

/// Monotonic per-process suffix so parallel tests in one binary never share
/// a database while names stay deterministic across re-runs.
static DB_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Create this test's private PostgreSQL database as a clone of the
/// migrated template and seed the SQLite-bootstrap-equivalent baseline for
/// the given self-host identity. Returns the database URL.
///
/// Callers gate on [`backends`] / [`pg_test_url`] first; this panics when
/// `NIGHTLIO_PG_TEST_URL` is unset.
pub async fn create_pg_db(seed: &SelfHostSeed) -> String {
    let admin_url = pg_test_url()
        .expect("create_pg_db requires NIGHTLIO_PG_TEST_URL (gate on support::backends())");
    let tpl = template_name(&admin_url).await;
    let name = format!(
        "nightlio_t_{}_{}",
        env!("CARGO_CRATE_NAME"),
        DB_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    assert_simple_identifier(&name);
    {
        let admin = pg_connect(&admin_url).await;
        with_harness_lock(&admin, async {
            admin
                .batch_execute(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"))
                .await
                .expect("drop a stale per-test database");
            admin
                .batch_execute(&format!("CREATE DATABASE {name} TEMPLATE {tpl}"))
                .await
                .expect("clone the template database");
        })
        .await;
    }
    let url = with_dbname(&admin_url, &name);
    seed_bootstrap_equivalent(&url, seed).await;
    url
}

/// Create an EMPTY per-test database (no template, no migrations, no seed)
/// — for tools that must run against a fresh target, like the
/// `migrate-to-postgres` round-trip. Returns the database URL.
pub async fn create_empty_pg_db(name_hint: &str) -> String {
    let admin_url = pg_test_url()
        .expect("create_empty_pg_db requires NIGHTLIO_PG_TEST_URL (gate on support::backends())");
    let name = format!("nightlio_t_{}_{name_hint}", env!("CARGO_CRATE_NAME"));
    assert_simple_identifier(&name);
    {
        let admin = pg_connect(&admin_url).await;
        with_harness_lock(&admin, async {
            admin
                .batch_execute(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"))
                .await
                .expect("drop a stale per-test database");
            admin
                .batch_execute(&format!("CREATE DATABASE {name}"))
                .await
                .expect("create an empty per-test database");
        })
        .await;
    }
    with_dbname(&admin_url, &name)
}

/// Seed exactly what the SQLite `db::bootstrap` seeds on a fresh file: the
/// default self-host user and the default groups/options, with the ids the
/// recorded fixtures were graded against pinned by assertion.
async fn seed_bootstrap_equivalent(url: &str, seed: &SelfHostSeed) {
    let client = pg_connect(url).await;
    let email = seed
        .email
        .clone()
        .unwrap_or_else(|| format!("{}@localhost", seed.external_id));
    let user_id: i64 = client
        .query_one(
            "INSERT INTO users (google_id, email, name, auth_provider, external_id) \
             VALUES ($1, $2, $3, 'local', $1) RETURNING id",
            &[&seed.external_id, &email, &seed.name],
        )
        .await
        .expect("seed the default self-host user")
        .get(0);
    assert_eq!(user_id, 1, "bootstrap seed: default user must get id 1");
    for (group_name, options) in DEFAULT_GROUPS {
        let group_name: &str = group_name;
        let group_id: i64 = client
            .query_one(
                "INSERT INTO groups (name, user_id) VALUES ($1, $2) RETURNING id",
                &[&group_name, &user_id],
            )
            .await
            .expect("seed a default group")
            .get(0);
        for option in *options {
            let option: &str = option;
            client
                .execute(
                    "INSERT INTO group_options (group_id, name) VALUES ($1, $2)",
                    &[&group_id, &option],
                )
                .await
                .expect("seed a default group option");
        }
    }
    let row = client
        .query_one(
            "SELECT (SELECT max(id) FROM groups), (SELECT max(id) FROM group_options)",
            &[],
        )
        .await
        .expect("verify seeded ids");
    let (max_group, max_option): (i64, i64) = (row.get(0), row.get(1));
    assert_eq!(
        (max_group, max_option),
        (3, 27),
        "bootstrap seed: default groups must occupy ids 1..=3 and options 1..=27"
    );
}
