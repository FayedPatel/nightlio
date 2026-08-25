//! Hand-rolled schema-migration runner (v0.6.0) with refinery-style
//! semantics over embedded, paired-dialect SQL files.
//!
//! Why hand-rolled: refinery's rusqlite feature pins libsqlite3-sys (a
//! native `links` key — two versions cannot coexist in one build), which
//! would dictate our deliberately pinned rusqlite 0.40.2-bundled. So this
//! module reimplements the useful subset: an embedded manifest, a
//! `schema_migrations(version, name, checksum, applied_at)` ledger on both
//! backends, per-migration transactions, and refuse-to-start verification.
//!
//! Ground rules (see docs/MIGRATIONS.md):
//! - Every migration ships as a **pair** of dialect files under
//!   `api/migrations/{sqlite,postgres}/NNNN_name.sql`, embedded at compile
//!   time via `include_str!`. A unit test asserts both directories list
//!   identical `(version, name)` pairs and match [`MIGRATIONS`].
//! - Shipped migration files are **frozen**: their SHA-256 checksums are
//!   recorded when applied, and any later drift refuses to start.
//! - **Adoption, never destruction** (SQLite): the legacy `bootstrap()`
//!   owns the baseline (it *is* the idempotent v0→v3 path) and always runs
//!   first. This runner then adopts a `user_version >= 3` database by
//!   recording 0001 as an already-applied baseline and applying 0002+ only.
//!   `user_version` is frozen at 3 forever; new schema changes live here.
//! - Postgres has no pre-history: 0001 bootstraps the entire schema.
//!
//! Refuse-to-start conditions (each with an actionable message):
//! - checksum mismatch between a recorded migration and this build's copy;
//! - downgrade (the database records a version newer than this build ships);
//! - numbering gaps, in either the embedded manifest or the recorded ledger;
//! - a SQLite database stuck below `user_version 3` while migrations beyond
//!   the baseline are pending (the lenient serve-anyway behavior survives
//!   only while nothing beyond the baseline exists).

use rusqlite::Connection;
use sha2::{Digest, Sha256};

use super::common::{DatabaseError, connect};

/// One migration: a version, a human-readable name, and the paired dialect
/// bodies. Both bodies always exist — a dialect that needs no work ships a
/// comment-only file (like the SQLite baseline) rather than no file.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub sqlite_sql: &'static str,
    pub postgres_sql: &'static str,
}

/// Which backend a runner is applying against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Sqlite,
    Postgres,
}

impl Migration {
    fn sql(&self, dialect: Dialect) -> &'static str {
        match dialect {
            Dialect::Sqlite => self.sqlite_sql,
            Dialect::Postgres => self.postgres_sql,
        }
    }
}

/// The embedded manifest. Append-only: new migrations are added at the end
/// with the next version number and a fresh pair of dialect files; existing
/// entries (and their files) are frozen forever.
pub const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    name: "baseline",
    sqlite_sql: include_str!("../../migrations/sqlite/0001_baseline.sql"),
    postgres_sql: include_str!("../../migrations/postgres/0001_baseline.sql"),
}];

/// The SQLite `user_version` the legacy bootstrap stamps on completion.
/// Frozen at 3 forever — every later schema change goes through this runner
/// and its `schema_migrations` ledger instead.
pub const BOOTSTRAP_COMPLETE_USER_VERSION: i64 = 3;

const CREATE_SCHEMA_MIGRATIONS_SQLITE: &str = "
    CREATE TABLE IF NOT EXISTS schema_migrations (
        version INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        checksum TEXT NOT NULL,
        applied_at TEXT NOT NULL
    )
    ";

const CREATE_SCHEMA_MIGRATIONS_PG: &str = "
    CREATE TABLE IF NOT EXISTS schema_migrations (
        version bigint PRIMARY KEY,
        name text NOT NULL,
        checksum text NOT NULL,
        applied_at text NOT NULL
    )
    ";

const INSERT_MIGRATION_SQLITE: &str =
    "INSERT INTO schema_migrations (version, name, checksum, applied_at) VALUES (?, ?, ?, ?)";

const INSERT_MIGRATION_PG: &str =
    "INSERT INTO schema_migrations (version, name, checksum, applied_at) VALUES ($1, $2, $3, $4)";

/// SHA-256 of a migration body, lowercase hex — the value stored in
/// `schema_migrations.checksum`.
pub fn checksum(sql: &str) -> String {
    let digest = Sha256::digest(sql.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// UTC wall-clock text for `applied_at`, identical shape on both backends
/// (`YYYY-MM-DD HH:MM:SS`, same as SQLite's CURRENT_TIMESTAMP).
fn now_utc_text() -> String {
    chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// A row read back from `schema_migrations`.
#[derive(Debug, Clone)]
struct Applied {
    version: i64,
    name: String,
    checksum: String,
}

// --- Verification (backend-independent, fully unit-tested) -------------------

/// The embedded manifest must be contiguous 1..=N in order — a gap or
/// duplicate is a packaging bug in the build itself and always refuses.
fn verify_manifest(migrations: &[Migration]) -> Result<(), DatabaseError> {
    for (index, migration) in migrations.iter().enumerate() {
        let expected = index as i64 + 1;
        if migration.version != expected {
            return Err(DatabaseError::Message(format!(
                "Refusing to start: the embedded migration manifest is broken — expected \
                 version {expected} at position {index} but found {:04}_{} (numbering gap or \
                 duplicate). This is a packaging bug in this build of nightlio-api, not a \
                 problem with your data; report it and roll back to the previous image.",
                migration.version, migration.name
            )));
        }
    }
    Ok(())
}

/// Compare the recorded ledger against the embedded manifest and return the
/// migrations still pending. Refuses on downgrade, ledger gaps, and
/// checksum/name drift.
fn verify_and_plan<'m>(
    migrations: &'m [Migration],
    applied: &[Applied],
    dialect: Dialect,
) -> Result<Vec<&'m Migration>, DatabaseError> {
    let max_embedded = migrations.last().map_or(0, |m| m.version);

    // Ledger contiguity: schema_migrations rows must be 1..=max with no
    // holes (SELECT ... ORDER BY version feeds this in order).
    for (index, row) in applied.iter().enumerate() {
        let expected = index as i64 + 1;
        if row.version != expected {
            return Err(DatabaseError::Message(format!(
                "Refusing to start: schema_migrations records version {} but is missing \
                 version {expected} — the migration history is inconsistent (partial restore \
                 or manual edit). Restore the database from a backup, or repair the \
                 schema_migrations table by hand before starting.",
                row.version
            )));
        }
    }

    // Downgrade: the data was last touched by a newer build.
    if let Some(newest) = applied.last()
        && newest.version > max_embedded
    {
        return Err(DatabaseError::Message(format!(
            "Refusing to start: the database records migration {:04}_{} but this build of \
             nightlio-api only ships migrations up to {max_embedded:04} — the data directory \
             was last run by a NEWER nightlio. Downgrades are not supported: run the newer \
             version against this database, or restore a backup taken before the upgrade.",
            newest.version, newest.name
        )));
    }

    // Immutability: every recorded migration must match this build's copy.
    for row in applied {
        // Contiguity above guarantees the index exists.
        let embedded = &migrations[(row.version - 1) as usize];
        if embedded.name != row.name {
            return Err(DatabaseError::Message(format!(
                "Refusing to start: migration version {} is recorded as '{}' but this build \
                 ships it as '{}'. Shipped migrations are immutable — this build's migration \
                 history diverged from the one that produced this database. Use a build from \
                 the same lineage, or restore from a backup.",
                row.version, row.name, embedded.name
            )));
        }
        let current = checksum(embedded.sql(dialect));
        if current != row.checksum {
            return Err(DatabaseError::Message(format!(
                "Refusing to start: migration {:04}_{} does not match the copy recorded in \
                 schema_migrations (recorded checksum {}, this build has {current}). Shipped \
                 migration files are immutable once applied; this usually means a modified \
                 or tampered build. Restore the original migration file (schema changes \
                 belong in a NEW migration), or restore the database from a backup.",
                row.version, row.name, row.checksum
            )));
        }
    }

    let max_applied = applied.last().map_or(0, |row| row.version);
    Ok(migrations
        .iter()
        .filter(|m| m.version > max_applied)
        .collect())
}

// --- SQLite runner -----------------------------------------------------------

/// Run migrations against the SQLite database at `db_path`. Must be called
/// after the legacy `bootstrap()` and before the serving pool opens.
pub fn run_sqlite(db_path: &str) -> Result<(), DatabaseError> {
    run_sqlite_with(db_path, MIGRATIONS)
}

/// Manifest-parameterized body of [`run_sqlite`] (tests inject synthetic
/// manifests to exercise the refusal paths).
fn run_sqlite_with(db_path: &str, migrations: &[Migration]) -> Result<(), DatabaseError> {
    verify_manifest(migrations)?;

    let conn = connect(db_path)?;
    let user_version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let has_ledger = schema_migrations_exists(&conn)?;

    if user_version < BOOTSTRAP_COMPLETE_USER_VERSION {
        // The legacy bootstrap did not provably complete (it swallows
        // "non-critical" errors and skips the stamp, exactly like the Flask
        // build did). Keep the historical lenient serve-anyway behavior —
        // but only while nothing beyond the baseline is pending; a real
        // schema change must never run on a half-migrated file.
        let applied = if has_ledger {
            load_applied_sqlite(&conn)?
        } else {
            Vec::new()
        };
        let max_applied = applied.last().map_or(0, |row| row.version);
        let pending_beyond_baseline = migrations
            .iter()
            .any(|m| m.version > 1 && m.version > max_applied);
        if pending_beyond_baseline {
            return Err(DatabaseError::Message(format!(
                "Refusing to start: this database has not completed the legacy bootstrap \
                 (PRAGMA user_version = {user_version}, expected \
                 {BOOTSTRAP_COMPLETE_USER_VERSION}) and schema migrations beyond the \
                 baseline are pending. The startup log above contains the exact bootstrap \
                 step that failed (look for 'non-critical' warnings — commonly a leftover \
                 groups_migration_new table or a concurrent writer holding the file). Fix \
                 that cause and restart: the bootstrap self-heals, and the pending \
                 migrations then apply. If the file is damaged, restore from a backup."
            )));
        }
        tracing::warn!(
            "schema_migrations adoption skipped: user_version = {user_version} (bootstrap \
             incomplete); serving anyway because nothing beyond the baseline is pending — \
             the next clean bootstrap run will adopt"
        );
        return Ok(());
    }

    // Adoption: a bootstrapped (user_version >= 3) database without a ledger
    // gets one, with 0001 recorded as the already-applied baseline. The
    // baseline body is deliberately never executed on SQLite — bootstrap()
    // owns that schema.
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let adopt = || -> Result<bool, DatabaseError> {
        conn.execute(CREATE_SCHEMA_MIGRATIONS_SQLITE, [])?;
        let rows: i64 = conn.query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
            row.get(0)
        })?;
        if rows > 0 {
            return Ok(false);
        }
        let baseline = &migrations[0];
        conn.execute(
            INSERT_MIGRATION_SQLITE,
            rusqlite::params![
                baseline.version,
                baseline.name,
                checksum(baseline.sql(Dialect::Sqlite)),
                now_utc_text(),
            ],
        )?;
        Ok(true)
    };
    match adopt() {
        Ok(adopted) => {
            conn.execute_batch("COMMIT")?;
            if adopted {
                tracing::info!(
                    "schema_migrations created; 0001_baseline recorded as adopted \
                     (bootstrap-owned) baseline"
                );
            }
        }
        Err(exc) => {
            let _ = conn.execute_batch("ROLLBACK");
            return Err(exc);
        }
    }

    let applied = load_applied_sqlite(&conn)?;
    let pending = verify_and_plan(migrations, &applied, Dialect::Sqlite)?;
    for migration in pending {
        apply_sqlite(&conn, migration)?;
    }
    Ok(())
}

fn schema_migrations_exists(conn: &Connection) -> Result<bool, DatabaseError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'schema_migrations'",
        [],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn load_applied_sqlite(conn: &Connection) -> Result<Vec<Applied>, DatabaseError> {
    let rows = conn
        .prepare("SELECT version, name, checksum FROM schema_migrations ORDER BY version")?
        .query_map([], |row| {
            Ok(Applied {
                version: row.get(0)?,
                name: row.get(1)?,
                checksum: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Apply one migration inside its own `BEGIN IMMEDIATE` transaction: the
/// body and its ledger row commit together or not at all. Migration bodies
/// must therefore not contain their own BEGIN/COMMIT.
fn apply_sqlite(conn: &Connection, migration: &Migration) -> Result<(), DatabaseError> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let run = || -> Result<(), DatabaseError> {
        conn.execute_batch(migration.sqlite_sql)?;
        conn.execute(
            INSERT_MIGRATION_SQLITE,
            rusqlite::params![
                migration.version,
                migration.name,
                checksum(migration.sql(Dialect::Sqlite)),
                now_utc_text(),
            ],
        )?;
        Ok(())
    };
    match run() {
        Ok(()) => {
            conn.execute_batch("COMMIT")?;
            tracing::info!(
                "migration {:04}_{} applied (sqlite)",
                migration.version,
                migration.name
            );
            Ok(())
        }
        Err(exc) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(DatabaseError::Message(format!(
                "migration {:04}_{} failed and was rolled back: {exc}",
                migration.version, migration.name
            )))
        }
    }
}

// --- Postgres runner ---------------------------------------------------------

/// Run migrations against Postgres. On a fresh database, 0001 bootstraps
/// the entire schema. The whole run holds
/// `pg_advisory_lock(hashtext('nightlio_migrations'))` so concurrent
/// replicas serialize instead of racing DDL.
pub async fn run_postgres(pool: &deadpool_postgres::Pool) -> Result<(), DatabaseError> {
    run_postgres_with(pool, MIGRATIONS).await
}

async fn run_postgres_with(
    pool: &deadpool_postgres::Pool,
    migrations: &[Migration],
) -> Result<(), DatabaseError> {
    verify_manifest(migrations)?;

    let object = pool.get().await.map_err(|exc| {
        DatabaseError::Message(format!(
            "Refusing to start: cannot reach PostgreSQL via DATABASE_URL: {exc}. Check that \
             the server is running and the URL's host, port, database name, credentials, and \
             sslmode are correct. Using the bundled docker compose? Start the database profile \
             first: docker compose --profile postgres up -d. For an external server, see \
             docs/POSTGRES.md (Bring your own PostgreSQL)."
        ))
    })?;
    // Detach the session from the pool: advisory locks are session-scoped,
    // and a pooled session outlives an early error return. A detached
    // client's socket closes on drop, so the server releases the lock even
    // when this function bails mid-run.
    let client = deadpool_postgres::Object::take(object);

    client
        .query_one(
            "SELECT pg_advisory_lock(hashtext('nightlio_migrations'))",
            &[],
        )
        .await
        .map_err(|exc| pg_err("acquiring the migration advisory lock", &exc))?;

    let result = run_postgres_locked(&client, migrations).await;

    // Best-effort tidy unlock; dropping the detached session releases it
    // regardless.
    let _ = client
        .query_one(
            "SELECT pg_advisory_unlock(hashtext('nightlio_migrations'))",
            &[],
        )
        .await;
    result
}

async fn run_postgres_locked(
    client: &deadpool_postgres::ClientWrapper,
    migrations: &[Migration],
) -> Result<(), DatabaseError> {
    client
        .batch_execute(CREATE_SCHEMA_MIGRATIONS_PG)
        .await
        .map_err(|exc| pg_err("creating schema_migrations", &exc))?;

    let rows = client
        .query(
            "SELECT version, name, checksum FROM schema_migrations ORDER BY version",
            &[],
        )
        .await
        .map_err(|exc| pg_err("reading schema_migrations", &exc))?;
    let applied: Vec<Applied> = rows
        .iter()
        .map(|row| Applied {
            version: row.get(0),
            name: row.get(1),
            checksum: row.get(2),
        })
        .collect();

    let pending = verify_and_plan(migrations, &applied, Dialect::Postgres)?;
    for migration in pending {
        apply_postgres(client, migration).await?;
    }
    Ok(())
}

/// Apply one migration inside its own transaction (DDL is transactional on
/// Postgres, so body + ledger row commit atomically).
async fn apply_postgres(
    client: &deadpool_postgres::ClientWrapper,
    migration: &Migration,
) -> Result<(), DatabaseError> {
    client
        .batch_execute("BEGIN")
        .await
        .map_err(|exc| pg_err("opening a migration transaction", &exc))?;
    let run = async {
        client.batch_execute(migration.postgres_sql).await?;
        client
            .execute(
                INSERT_MIGRATION_PG,
                &[
                    &migration.version,
                    &migration.name,
                    &checksum(migration.sql(Dialect::Postgres)),
                    &now_utc_text(),
                ],
            )
            .await?;
        Ok::<(), tokio_postgres::Error>(())
    };
    match run.await {
        Ok(()) => {
            client
                .batch_execute("COMMIT")
                .await
                .map_err(|exc| pg_err("committing a migration", &exc))?;
            tracing::info!(
                "migration {:04}_{} applied (postgres)",
                migration.version,
                migration.name
            );
            Ok(())
        }
        Err(exc) => {
            let _ = client.batch_execute("ROLLBACK").await;
            Err(DatabaseError::Message(format!(
                "migration {:04}_{} failed and was rolled back: {exc}",
                migration.version, migration.name
            )))
        }
    }
}

fn pg_err(context: &str, exc: &tokio_postgres::Error) -> DatabaseError {
    DatabaseError::Message(format!("Database error while {context}: {exc}"))
}

// --- Tests -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    use super::*;
    use crate::db::bootstrap::{SelfHostSeed, bootstrap};

    fn migrations_dir(dialect: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("migrations")
            .join(dialect)
    }

    fn corpus_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../contract/corpus")
    }

    /// `(version, name)` pairs parsed from a dialect directory's
    /// `NNNN_name.sql` filenames.
    fn dir_manifest(dialect: &str) -> BTreeSet<(i64, String)> {
        std::fs::read_dir(migrations_dir(dialect))
            .unwrap_or_else(|exc| panic!("missing migrations/{dialect} directory: {exc}"))
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|file| file.ends_with(".sql"))
            .map(|file| {
                let stem = file.strip_suffix(".sql").unwrap();
                let (version, name) = stem
                    .split_once('_')
                    .unwrap_or_else(|| panic!("bad migration filename: {file}"));
                assert_eq!(version.len(), 4, "version prefix must be 4 digits: {file}");
                (version.parse::<i64>().unwrap(), name.to_string())
            })
            .collect()
    }

    fn user_version(path: &Path) -> i64 {
        Connection::open(path)
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap()
    }

    fn ledger(path: &Path) -> Vec<(i64, String, String)> {
        Connection::open(path)
            .unwrap()
            .prepare("SELECT version, name, checksum FROM schema_migrations ORDER BY version")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    fn table_counts(path: &Path) -> Vec<(String, i64)> {
        let conn = Connection::open(path).unwrap();
        let tables: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table' \
                 AND name NOT LIKE 'sqlite_%' AND name <> 'schema_migrations' ORDER BY name",
            )
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        tables
            .into_iter()
            .map(|table| {
                let count: i64 = conn
                    .query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |row| {
                        row.get(0)
                    })
                    .unwrap();
                (table, count)
            })
            .collect()
    }

    /// Synthetic version-2 migration used to exercise the apply path.
    const ADD_WIDGETS: Migration = Migration {
        version: 2,
        name: "add_widgets",
        sqlite_sql: "CREATE TABLE widgets (id INTEGER PRIMARY KEY, name TEXT NOT NULL);",
        postgres_sql: "CREATE TABLE widgets (id bigint GENERATED BY DEFAULT AS IDENTITY PRIMARY KEY, \
             name text COLLATE \"C\" NOT NULL);",
    };

    // -- Dialect manifest parity ---------------------------------------------

    /// Both dialect directories list identical (version, name) pairs, and
    /// the on-disk pairs are exactly the embedded manifest (a file added on
    /// disk but not embedded — or vice versa — fails here).
    #[test]
    fn dialect_dirs_list_identical_version_name_pairs_matching_embedded_manifest() {
        let sqlite = dir_manifest("sqlite");
        let postgres = dir_manifest("postgres");
        assert_eq!(
            sqlite, postgres,
            "migrations/sqlite and migrations/postgres must pair every (version, name)"
        );
        let embedded: BTreeSet<(i64, String)> = MIGRATIONS
            .iter()
            .map(|m| (m.version, m.name.to_string()))
            .collect();
        assert_eq!(
            sqlite, embedded,
            "on-disk migration files must match the MIGRATIONS manifest embedded in the binary"
        );
    }

    #[test]
    fn embedded_manifest_is_contiguous_from_one() {
        verify_manifest(MIGRATIONS).unwrap();
    }

    // -- Adoption -------------------------------------------------------------

    #[test]
    fn fresh_bootstrap_then_migrate_adopts_baseline_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fresh.db");
        let path_str = path.to_str().unwrap();
        bootstrap(path_str, &SelfHostSeed::default()).unwrap();

        run_sqlite(path_str).unwrap();
        let rows = ledger(&path);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, 1);
        assert_eq!(rows[0].1, "baseline");
        assert_eq!(rows[0].2, checksum(MIGRATIONS[0].sqlite_sql));
        assert_eq!(user_version(&path), 3, "user_version stays frozen at 3");

        // Second run: verification passes, nothing re-applies, ledger
        // byte-identical (including applied_at).
        let before: Vec<(i64, String, String, String)> = Connection::open(&path)
            .unwrap()
            .prepare("SELECT version, name, checksum, applied_at FROM schema_migrations")
            .unwrap()
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        run_sqlite(path_str).unwrap();
        let after: Vec<(i64, String, String, String)> = Connection::open(&path)
            .unwrap()
            .prepare("SELECT version, name, checksum, applied_at FROM schema_migrations")
            .unwrap()
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(before, after, "second run must be a no-op");
    }

    /// Corpus databases (real historical production shapes): startup order
    /// bootstrap() -> run_sqlite() adopts every one of them without touching
    /// a single data row.
    #[test]
    fn corpus_databases_are_adopted_without_data_changes() {
        for input in [
            "fresh.db",
            "half-migrated.db",
            "half-migrated.migrated.db",
            "legacy-groups.db",
        ] {
            let src = corpus_dir().join(input);
            assert!(src.exists(), "corpus input missing: {}", src.display());
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("work.db");
            std::fs::copy(&src, &path).unwrap();
            let path_str = path.to_str().unwrap();

            bootstrap(path_str, &SelfHostSeed::default()).unwrap();
            assert_eq!(user_version(&path), 3, "{input}: bootstrap must reach v3");
            let counts_before = table_counts(&path);

            run_sqlite(path_str).unwrap();

            let rows = ledger(&path);
            assert_eq!(rows.len(), 1, "{input}: exactly the adopted baseline");
            assert_eq!(rows[0].0, 1, "{input}");
            assert_eq!(rows[0].1, "baseline", "{input}");
            assert_eq!(
                table_counts(&path),
                counts_before,
                "{input}: adoption must never touch data rows"
            );
            assert_eq!(user_version(&path), 3, "{input}: user_version frozen at 3");
        }
    }

    // -- Applying beyond the baseline ----------------------------------------

    #[test]
    fn pending_migrations_apply_in_per_migration_transactions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("apply.db");
        let path_str = path.to_str().unwrap();
        bootstrap(path_str, &SelfHostSeed::default()).unwrap();

        let manifest = [MIGRATIONS[0], ADD_WIDGETS];
        run_sqlite_with(path_str, &manifest).unwrap();

        let conn = Connection::open(&path).unwrap();
        conn.execute("INSERT INTO widgets (name) VALUES ('w')", [])
            .unwrap();
        let rows = ledger(&path);
        assert_eq!(
            rows.iter().map(|r| r.0).collect::<Vec<_>>(),
            vec![1, 2],
            "baseline adopted, 0002 applied"
        );
        assert_eq!(rows[1].2, checksum(ADD_WIDGETS.sqlite_sql));

        // Idempotent rerun.
        run_sqlite_with(path_str, &manifest).unwrap();
        assert_eq!(ledger(&path).len(), 2);
    }

    #[test]
    fn failed_migration_rolls_back_body_and_ledger_row_together() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rollback.db");
        let path_str = path.to_str().unwrap();
        bootstrap(path_str, &SelfHostSeed::default()).unwrap();

        let broken = Migration {
            version: 2,
            name: "broken",
            sqlite_sql: "CREATE TABLE half_done (id INTEGER PRIMARY KEY); \
                         INSERT INTO no_such_table VALUES (1);",
            postgres_sql: "SELECT 1;",
        };
        let err = run_sqlite_with(path_str, &[MIGRATIONS[0], broken]).unwrap_err();
        assert!(
            err.to_string().contains("0002_broken"),
            "unexpected error: {err}"
        );
        assert!(
            err.to_string().contains("rolled back"),
            "unexpected error: {err}"
        );

        let conn = Connection::open(&path).unwrap();
        let leftovers: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'half_done'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(leftovers, 0, "partial DDL must roll back");
        assert_eq!(ledger(&path).len(), 1, "only the adopted baseline remains");
    }

    // -- Refuse-to-start conditions ------------------------------------------

    #[test]
    fn checksum_mismatch_refuses_to_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tamper.db");
        let path_str = path.to_str().unwrap();
        bootstrap(path_str, &SelfHostSeed::default()).unwrap();
        run_sqlite_with(path_str, &[MIGRATIONS[0], ADD_WIDGETS]).unwrap();

        let tampered = Migration {
            sqlite_sql: "CREATE TABLE widgets (id INTEGER PRIMARY KEY);",
            ..ADD_WIDGETS
        };
        let err = run_sqlite_with(path_str, &[MIGRATIONS[0], tampered]).unwrap_err();
        let text = err.to_string();
        assert!(text.starts_with("Refusing to start"), "{text}");
        assert!(text.contains("0002_add_widgets"), "{text}");
        assert!(text.contains("checksum"), "{text}");
    }

    #[test]
    fn downgrade_refuses_to_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("downgrade.db");
        let path_str = path.to_str().unwrap();
        bootstrap(path_str, &SelfHostSeed::default()).unwrap();
        run_sqlite_with(path_str, &[MIGRATIONS[0], ADD_WIDGETS]).unwrap();

        // An older build that only ships the baseline must refuse.
        let err = run_sqlite_with(path_str, &[MIGRATIONS[0]]).unwrap_err();
        let text = err.to_string();
        assert!(text.starts_with("Refusing to start"), "{text}");
        assert!(text.contains("NEWER"), "{text}");
        assert!(text.contains("Downgrades are not supported"), "{text}");
    }

    #[test]
    fn manifest_numbering_gap_refuses_to_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gap.db");
        let path_str = path.to_str().unwrap();
        bootstrap(path_str, &SelfHostSeed::default()).unwrap();

        let skips_two = Migration {
            version: 3,
            ..ADD_WIDGETS
        };
        let err = run_sqlite_with(path_str, &[MIGRATIONS[0], skips_two]).unwrap_err();
        let text = err.to_string();
        assert!(text.starts_with("Refusing to start"), "{text}");
        assert!(text.contains("numbering gap or duplicate"), "{text}");
    }

    #[test]
    fn ledger_gap_refuses_to_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ledger-gap.db");
        let path_str = path.to_str().unwrap();
        bootstrap(path_str, &SelfHostSeed::default()).unwrap();
        run_sqlite(path_str).unwrap();

        // Simulate a partial restore: a recorded version with a hole below it.
        Connection::open(&path)
            .unwrap()
            .execute(
                "INSERT INTO schema_migrations (version, name, checksum, applied_at) \
                 VALUES (3, 'phantom', 'deadbeef', '2026-01-01 00:00:00')",
                [],
            )
            .unwrap();
        let three = Migration {
            version: 3,
            name: "phantom",
            sqlite_sql: "SELECT 1;",
            postgres_sql: "SELECT 1;",
        };
        let err = run_sqlite_with(path_str, &[MIGRATIONS[0], ADD_WIDGETS, three]).unwrap_err();
        let text = err.to_string();
        assert!(text.starts_with("Refusing to start"), "{text}");
        assert!(text.contains("missing version 2"), "{text}");
    }

    // -- The user_version < 3 lenient gate ------------------------------------

    #[test]
    fn incomplete_bootstrap_stays_lenient_when_only_baseline_exists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lenient.db");
        let path_str = path.to_str().unwrap();
        bootstrap(path_str, &SelfHostSeed::default()).unwrap();
        // Simulate a file whose bootstrap never provably completed.
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", 0)
            .unwrap();

        // Today's lenient behavior survives: Ok, and no ledger is created.
        run_sqlite(path_str).unwrap();
        let conn = Connection::open(&path).unwrap();
        assert!(
            !schema_migrations_exists(&conn).unwrap(),
            "lenient path must not adopt a half-bootstrapped file"
        );
    }

    #[test]
    fn incomplete_bootstrap_fails_closed_when_migrations_beyond_baseline_pend() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("failclosed.db");
        let path_str = path.to_str().unwrap();
        bootstrap(path_str, &SelfHostSeed::default()).unwrap();
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", 0)
            .unwrap();

        let err = run_sqlite_with(path_str, &[MIGRATIONS[0], ADD_WIDGETS]).unwrap_err();
        let text = err.to_string();
        assert!(text.starts_with("Refusing to start"), "{text}");
        assert!(
            text.contains("has not completed the legacy bootstrap"),
            "{text}"
        );
        assert!(text.contains("user_version = 0"), "{text}");
    }
}
