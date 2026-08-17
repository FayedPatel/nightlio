//! Corpus parity tests: the Rust `bootstrap()` must produce byte-identical
//! schema dumps (and matching row counts / sqlite_master) to what the
//! Python `init_database()` produced on the same inputs.
//!
//! Inputs and expected outputs live in `contract/corpus/` (see its README):
//! - `fresh.*`                  — bootstrap on an empty path;
//! - `legacy-groups.db`         — pre-Phase-2a shape, migrated →
//!   `legacy-groups.migrated.*`;
//! - `half-migrated.db`         — partially migrated production shape,
//!   migrated → `half-migrated.migrated.*`.
//!
//! The dump formats replicate `dump_pragmas` / `dump_counts` in
//! `contract/corpus/build_corpus.py` exactly (Python tuple/list reprs
//! included) so the checked-in `.pragmas.txt` / `.counts.txt` files diff
//! cleanly against what we generate here.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use nightlio_api::db::bootstrap::{SelfHostSeed, bootstrap};
use rusqlite::Connection;

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../contract/corpus")
}

fn read_expected(name: &str) -> String {
    let path = corpus_dir().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|exc| {
        panic!(
            "corpus file missing: {} ({exc}) — regenerate with \
             `api/venv/bin/python contract/corpus/build_corpus.py`",
            path.display()
        )
    })
}

/// Python `repr` of a str for the identifier-ish strings in these dumps
/// (no quotes/backslashes ever appear in this schema's names).
fn py_str(value: &str) -> String {
    format!("'{value}'")
}

fn py_opt(value: &Option<String>) -> String {
    match value {
        None => "None".to_string(),
        Some(text) => py_str(text),
    }
}

fn user_tables(conn: &Connection) -> Vec<String> {
    conn.prepare(
        "SELECT name FROM sqlite_master \
         WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
    )
    .unwrap()
    .query_map([], |row| row.get::<_, String>(0))
    .unwrap()
    .collect::<rusqlite::Result<_>>()
    .unwrap()
}

/// Replica of `build_corpus.py::dump_pragmas` (same text, same ordering).
fn dump_pragmas(conn: &Connection) -> String {
    let mut lines: Vec<String> = Vec::new();
    for table in user_tables(conn) {
        lines.push(format!("== table: {table}"));
        lines.push("-- PRAGMA table_info (cid, name, type, notnull, dflt_value, pk)".to_string());
        let table_info: Vec<(i64, String, String, i64, Option<String>, i64)> = conn
            .prepare(&format!("PRAGMA table_info(\"{table}\")"))
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        for (cid, name, col_type, notnull, dflt_value, pk) in &table_info {
            lines.push(format!(
                "   ({cid}, {}, {}, {notnull}, {}, {pk})",
                py_str(name),
                py_str(col_type),
                py_opt(dflt_value),
            ));
        }
        lines.push("-- PRAGMA index_list (seq, name, unique, origin, partial)".to_string());
        let index_rows: Vec<(i64, String, i64, String, i64)> = conn
            .prepare(&format!("PRAGMA index_list(\"{table}\")"))
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        for (seq, name, unique, origin, partial) in &index_rows {
            lines.push(format!(
                "   ({seq}, {}, {unique}, {}, {partial})",
                py_str(name),
                py_str(origin),
            ));
        }
        for (_, name, _, _, _) in &index_rows {
            let safe = name.replace('"', "\"\"");
            let cols: Vec<String> = conn
                .prepare(&format!("PRAGMA index_info(\"{safe}\")"))
                .unwrap()
                .query_map([], |row| row.get::<_, String>(2))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            let cols_repr = format!(
                "[{}]",
                cols.iter()
                    .map(|c| py_str(c))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            lines.push(format!("-- PRAGMA index_info({name}) columns: {cols_repr}"));
        }
        lines.push(String::new());
    }
    lines.join("\n")
}

/// Replica of `build_corpus.py::dump_counts`.
fn dump_counts(conn: &Connection) -> String {
    let mut lines: Vec<String> = Vec::new();
    for table in user_tables(conn) {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |row| {
                row.get(0)
            })
            .unwrap();
        lines.push(format!("{table}\t{count}"));
    }
    lines.join("\n") + "\n"
}

/// `sqlite_master` rows (type, name, tbl_name, sql) for non-sqlite objects,
/// ordered. The `sql` column carries the DDL text verbatim, so this asserts
/// the Rust CREATE statements are byte-identical to the Python ones.
fn schema_rows(conn: &Connection) -> Vec<(String, String, String, Option<String>)> {
    conn.prepare(
        "SELECT type, name, tbl_name, sql FROM sqlite_master \
         WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name",
    )
    .unwrap()
    .query_map([], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    })
    .unwrap()
    .collect::<rusqlite::Result<_>>()
    .unwrap()
}

fn parse_counts(text: &str) -> HashMap<String, i64> {
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let (table, count) = line.split_once('\t').expect("counts line");
            (table.to_string(), count.parse().expect("count"))
        })
        .collect()
}

/// Run the full parity assertion set for one corpus case, and return the
/// migrated DB path for extra per-case assertions.
///
/// `input`: corpus DB copied to a temp path first (None = empty path), then
/// bootstrapped with the default self-host seed (the corpus was built with
/// no config overrides). `expected`: basename of the `.pragmas.txt` /
/// `.counts.txt` / `.db` triple to compare against.
fn assert_bootstrap_parity(tmp: &Path, input: Option<&str>, expected: &str) -> PathBuf {
    let work_db = tmp.join("work.db");
    if let Some(name) = input {
        let src = corpus_dir().join(name);
        assert!(
            src.exists(),
            "corpus input missing: {} — regenerate with \
             `api/venv/bin/python contract/corpus/build_corpus.py`",
            src.display()
        );
        std::fs::copy(&src, &work_db).unwrap();
    }

    bootstrap(work_db.to_str().unwrap(), &SelfHostSeed::default()).unwrap();

    let conn = Connection::open(&work_db).unwrap();
    let pragmas = dump_pragmas(&conn);
    let counts = dump_counts(&conn);

    assert_eq!(
        pragmas,
        read_expected(&format!("{expected}.pragmas.txt")),
        "PRAGMA dump mismatch vs {expected}.pragmas.txt"
    );
    assert_eq!(
        counts,
        read_expected(&format!("{expected}.counts.txt")),
        "row-count dump mismatch vs {expected}.counts.txt"
    );

    // sqlite_master parity against the Python-migrated DB itself: catches
    // any DDL-text drift the pragma dumps cannot see.
    let expected_conn = Connection::open(corpus_dir().join(format!("{expected}.db"))).unwrap();
    assert_eq!(
        schema_rows(&conn),
        schema_rows(&expected_conn),
        "sqlite_master mismatch vs {expected}.db"
    );

    // Second run must be a no-op (idempotency): identical dumps.
    drop(conn);
    bootstrap(work_db.to_str().unwrap(), &SelfHostSeed::default()).unwrap();
    let conn = Connection::open(&work_db).unwrap();
    assert_eq!(
        dump_pragmas(&conn),
        pragmas,
        "second bootstrap changed schema"
    );
    assert_eq!(
        dump_counts(&conn),
        counts,
        "second bootstrap changed row counts"
    );

    work_db
}

#[test]
fn fresh_bootstrap_matches_python_corpus() {
    let tmp = tempfile::tempdir().unwrap();
    assert_bootstrap_parity(tmp.path(), None, "fresh");
}

#[test]
fn legacy_groups_migration_matches_python_corpus() {
    let tmp = tempfile::tempdir().unwrap();
    let db = assert_bootstrap_parity(
        tmp.path(),
        Some("legacy-groups.db"),
        "legacy-groups.migrated",
    );
    let conn = Connection::open(&db).unwrap();

    // Ownerless groups backfilled to the seeded self-host user (id 1).
    let owners: Vec<Option<i64>> = conn
        .prepare("SELECT DISTINCT user_id FROM groups ORDER BY user_id")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(owners, vec![Some(1)]);

    // Identity backfill: external_id := google_id everywhere; auth_provider
    // 'local' only for the DEFAULT_SELF_HOST_ID row, else 'legacy-google'.
    let users: Vec<(String, Option<String>, Option<String>)> = conn
        .prepare("SELECT google_id, auth_provider, external_id FROM users ORDER BY id")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(users.len(), 2);
    for (google_id, auth_provider, external_id) in &users {
        assert_eq!(external_id.as_deref(), Some(google_id.as_str()));
        let expected_provider = if google_id == "selfhost_default_user" {
            "local"
        } else {
            "legacy-google"
        };
        assert_eq!(auth_provider.as_deref(), Some(expected_provider));
    }

    // No data lost or invented: user-data row counts unchanged from the
    // pre-migration input dump.
    let before = parse_counts(&read_expected("legacy-groups.counts.txt"));
    for table in [
        "users",
        "groups",
        "group_options",
        "mood_entries",
        "entry_selections",
        "goals",
        "goal_completions",
        "achievements",
        "user_metrics",
    ] {
        let after: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM \"{table}\""), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(after, before[table], "row count changed for {table}");
    }
}

#[test]
fn half_migrated_bootstrap_matches_python_corpus() {
    let tmp = tempfile::tempdir().unwrap();
    let db = assert_bootstrap_parity(
        tmp.path(),
        Some("half-migrated.db"),
        "half-migrated.migrated",
    );
    let conn = Connection::open(&db).unwrap();

    // The legacy Google row (identity columns NULL in the input) got the
    // backfill on re-run.
    let (auth_provider, external_id): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT auth_provider, external_id FROM users WHERE google_id <> 'selfhost_default_user'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(auth_provider.as_deref(), Some("legacy-google"));
    assert_eq!(external_id.as_deref(), Some("108234567890123456789"));
}
