//! Subprocess tests for the `seed-demo` subcommand (v0.6.0): usage errors
//! always run; the SQLite tier seeds a tempdir database through the real
//! binary and asserts the row counts, idempotency (already-seeded no-op
//! still exits 0 — the demo compose gates the api on
//! `service_completed_successfully`), and `--force` appending; a PG twin
//! runs the same asserts against an empty scratch database when
//! `NIGHTLIO_PG_TEST_URL` is set.

use std::process::Command;

mod support;

/// The binary under test, built by cargo for this integration test.
const BIN: &str = env!("CARGO_BIN_EXE_nightlio-api");

/// Run the binary from an empty tempdir (so no repo `.env` can leak a
/// `DATABASE_URL` into the defaults) and return (exit code, stdout, stderr).
fn run_bin_in(dir: &tempfile::TempDir, args: &[&str]) -> (Option<i32>, String, String) {
    let output = Command::new(BIN)
        .args(args)
        .current_dir(dir.path())
        .env_remove("DATABASE_URL")
        .env_remove("DATABASE_PATH")
        .output()
        .expect("run nightlio-api");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn run_bin(args: &[&str]) -> (Option<i32>, String, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    run_bin_in(&dir, args)
}

// ---------------------------------------------------------------------------
// Usage errors (no database needed — always run)
// ---------------------------------------------------------------------------

#[test]
fn unknown_argument_is_a_usage_error() {
    let (code, _, stderr) = run_bin(&["seed-demo", "--bogus"]);
    assert_eq!(code, Some(2), "stderr: {stderr}");
    assert!(stderr.contains("unknown argument `--bogus`"), "{stderr}");
    assert!(stderr.contains("usage: nightlio-api seed-demo"), "{stderr}");
}

#[test]
fn flag_without_value_is_a_usage_error() {
    for flag in ["--database-url", "--sqlite"] {
        let (code, _, stderr) = run_bin(&["seed-demo", flag]);
        assert_eq!(code, Some(2), "{flag}: stderr: {stderr}");
        assert!(
            stderr.contains(&format!("{flag} needs a value")),
            "{stderr}"
        );
    }
}

#[test]
fn both_targets_together_are_a_usage_error() {
    let (code, _, stderr) = run_bin(&[
        "seed-demo",
        "--database-url",
        "postgres://u:p@localhost:5432/db",
        "--sqlite",
        "demo.db",
    ]);
    assert_eq!(code, Some(2), "stderr: {stderr}");
    assert!(
        stderr.contains("--database-url and --sqlite are mutually exclusive"),
        "{stderr}"
    );
}

// ---------------------------------------------------------------------------
// Shared count assertions
// ---------------------------------------------------------------------------

/// The seeded shape both backends must produce on a fresh database.
const EXPECTED: &[(&str, i64)] = &[
    ("groups", 3),
    ("group_options", 27),
    ("mood_entries", 14),
    ("goals", 6),
    ("goal_completions", 18),
];

fn sqlite_count(conn: &rusqlite::Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

// ---------------------------------------------------------------------------
// SQLite tier (always runs)
// ---------------------------------------------------------------------------

#[test]
fn sqlite_seed_counts_idempotency_and_force() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("demo.db");
    let path = db_path.to_string_lossy().into_owned();

    // First run seeds and prints the summary.
    let (code, stdout, stderr) = run_bin_in(&dir, &["seed-demo", "--sqlite", &path]);
    assert_eq!(code, Some(0), "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("seed-demo complete"), "{stdout}");

    let conn = rusqlite::Connection::open(&db_path).expect("open the seeded file");
    for (table, expected) in EXPECTED {
        assert_eq!(sqlite_count(&conn, table), *expected, "{table}");
    }
    assert!(sqlite_count(&conn, "entry_selections") > 0, "selections");
    assert!(
        sqlite_count(&conn, "achievements") >= 1,
        "achievements must unlock naturally"
    );

    // Second run: already-seeded no-op, exit 0 (compose contract), no rows
    // added.
    let (code, stdout, stderr) = run_bin_in(&dir, &["seed-demo", "--sqlite", &path]);
    assert_eq!(code, Some(0), "stdout: {stdout}\nstderr: {stderr}");
    assert!(
        stdout.contains("already seeded (14 entries); pass --force to add demo data anyway"),
        "{stdout}"
    );
    for (table, expected) in EXPECTED {
        assert_eq!(sqlite_count(&conn, table), *expected, "{table} after no-op");
    }

    // --force appends another dataset.
    let (code, stdout, stderr) = run_bin_in(&dir, &["seed-demo", "--sqlite", &path, "--force"]);
    assert_eq!(code, Some(0), "stdout: {stdout}\nstderr: {stderr}");
    assert_eq!(sqlite_count(&conn, "mood_entries"), 28, "forced append");
}

// ---------------------------------------------------------------------------
// PostgreSQL tier (gated on NIGHTLIO_PG_TEST_URL)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn postgres_seed_matches_the_sqlite_counts() {
    if support::pg_test_url().is_none() {
        eprintln!("skipping seed-demo PG tier: NIGHTLIO_PG_TEST_URL not set");
        return;
    }
    let url = support::create_empty_pg_db("seed_demo").await;
    let dir = tempfile::tempdir().expect("tempdir");

    let (code, stdout, stderr) = run_bin_in(&dir, &["seed-demo", "--database-url", &url]);
    assert_eq!(code, Some(0), "stdout: {stdout}\nstderr: {stderr}");
    assert!(stdout.contains("seed-demo complete"), "{stdout}");

    let client = support::pg_connect(&url).await;
    for (table, expected) in EXPECTED {
        let count: i64 = client
            .query_one(&format!("SELECT COUNT(*) FROM {table}"), &[])
            .await
            .unwrap()
            .get(0);
        assert_eq!(count, *expected, "{table}");
    }
    let selections: i64 = client
        .query_one("SELECT COUNT(*) FROM entry_selections", &[])
        .await
        .unwrap()
        .get(0);
    assert!(selections > 0, "selections");
    let achievements: i64 = client
        .query_one("SELECT COUNT(*) FROM achievements", &[])
        .await
        .unwrap()
        .get(0);
    assert!(achievements >= 1, "achievements must unlock naturally");

    // Idempotent re-run, exit 0.
    let (code, stdout, stderr) = run_bin_in(&dir, &["seed-demo", "--database-url", &url]);
    assert_eq!(code, Some(0), "stdout: {stdout}\nstderr: {stderr}");
    assert!(
        stdout.contains("already seeded (14 entries); pass --force to add demo data anyway"),
        "{stdout}"
    );
    let entries: i64 = client
        .query_one("SELECT COUNT(*) FROM mood_entries", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(entries, 14, "no-op run must not add rows");
}
