//! Tier-4 round trip for the `migrate-to-postgres` subcommand (v0.6.0):
//! seed a real SQLite database through the actual routes (the recorder's
//! populated mood scenario) plus direct inserts of the historical quirks
//! the tool must preserve verbatim (a US-format garbage date and a REAL
//! mood value), run the compiled binary against a scratch PostgreSQL
//! database, then verify row counts, id/sequence integrity, the quirky
//! rows, the refuse-unless-empty guard — and finally boot a PG-backed app
//! on the imported data and replay the same recorded fixtures the SQLite
//! harness is graded against.
//!
//! The import test is gated on `NIGHTLIO_PG_TEST_URL` (clean skip when
//! unset); the argument-parsing tests always run.

use std::collections::HashMap;
use std::process::Command;

mod support;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, header};
use axum::response::Response;
use chrono::Datelike;
use serde_json::{Value, json};
use tower::ServiceExt;

use nightlio_api::auth::jwt;
use nightlio_api::config::Config;
use nightlio_api::db::{self, SelfHostSeed};
use nightlio_api::routes;
use nightlio_api::state::AppState;

/// The binary under test, built by cargo for this integration test.
const BIN: &str = env!("CARGO_BIN_EXE_nightlio-api");

// ---------------------------------------------------------------------------
// Argument parsing (no PostgreSQL needed — runs in the default suite)
// ---------------------------------------------------------------------------

/// Run the binary from an empty tempdir (so no repo `.env` can leak a
/// `DATABASE_URL` into the defaults) and return (exit code, stderr).
fn run_bin(args: &[&str]) -> (Option<i32>, String) {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = Command::new(BIN)
        .args(args)
        .current_dir(dir.path())
        .env_remove("DATABASE_URL")
        .env_remove("DATABASE_PATH")
        .output()
        .expect("run nightlio-api");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn unknown_argument_is_a_usage_error() {
    let (code, stderr) = run_bin(&["migrate-to-postgres", "--bogus"]);
    assert_eq!(code, Some(2), "stderr: {stderr}");
    assert!(stderr.contains("unknown argument `--bogus`"), "{stderr}");
    assert!(
        stderr.contains("usage: nightlio-api migrate-to-postgres"),
        "{stderr}"
    );
}

#[test]
fn flag_without_value_is_a_usage_error() {
    for flag in ["--sqlite", "--postgres-url"] {
        let (code, stderr) = run_bin(&["migrate-to-postgres", flag]);
        assert_eq!(code, Some(2), "{flag}: stderr: {stderr}");
        assert!(
            stderr.contains(&format!("{flag} needs a value")),
            "{stderr}"
        );
    }
}

#[test]
fn missing_postgres_target_is_a_usage_error() {
    let (code, stderr) = run_bin(&["migrate-to-postgres"]);
    assert_eq!(code, Some(2), "stderr: {stderr}");
    assert!(
        stderr.contains("no PostgreSQL target: set DATABASE_URL or pass --postgres-url"),
        "{stderr}"
    );
}

// ---------------------------------------------------------------------------
// Fixture-replay helpers (the mood_routes conventions, minus insta — the
// snapshots stay owned by that harness)
// ---------------------------------------------------------------------------

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/../contract/fixtures/mood/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|exc| panic!("missing fixture {path}: {exc}"));
    serde_json::from_str(&raw).expect("fixture json")
}

/// Same volatile-value normalization as the mood harness: timestamps →
/// `<TS>`, and the extended digest's current year/month placeholders.
fn normalize(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, item) in map.iter_mut() {
                if (key == "created_at" || key == "updated_at") && item.is_string() {
                    *item = Value::String("<TS>".to_string());
                } else {
                    if key == "monthly_digest" && item.is_object() {
                        let today = chrono::Local::now().date_naive();
                        if item["year"] == json!(today.year()) {
                            item["year"] = json!("<CURRENT_YEAR>");
                        }
                        if item["month"] == json!(today.month()) {
                            item["month"] = json!("<CURRENT_MONTH>");
                        }
                    }
                    normalize(item);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                normalize(item);
            }
        }
        _ => {}
    }
}

async fn body_json(response: Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("json body")
}

fn build_request(fx: &Value, token: &str) -> Request<Body> {
    let method = fx["method"].as_str().expect("fixture method");
    let path = fx["path"].as_str().expect("fixture path");
    let mut builder = Request::builder().method(method).uri(path);
    if fx["request"]["headers_that_matter"]
        .get("Authorization")
        .is_some()
    {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let body = &fx["request"]["body"];
    if body.is_null() {
        builder.body(Body::empty()).expect("request")
    } else {
        builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(body).expect("body json")))
            .expect("request")
    }
}

/// Replay a recorded fixture and assert status + normalized body equality.
async fn check_fixture(app: &Router, token: &str, name: &str) {
    let fx = fixture(name);
    let response = app
        .clone()
        .oneshot(build_request(&fx, token))
        .await
        .expect("infallible");
    assert_eq!(
        u64::from(response.status().as_u16()),
        fx["response"]["status"].as_u64().expect("fixture status"),
        "{name}: status"
    );
    let mut actual = body_json(response).await;
    normalize(&mut actual);
    assert_eq!(actual, fx["response"]["body"], "{name}: body");
}

async fn get_authed(app: &Router, token: &str, path: &str) -> Value {
    let request = Request::builder()
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .expect("request");
    let response = app.clone().oneshot(request).await.expect("infallible");
    assert!(response.status().is_success(), "GET {path}");
    body_json(response).await
}

fn dev_config(dir: &tempfile::TempDir) -> Config {
    let vars: HashMap<String, String> =
        HashMap::from([("APP_ENV".to_string(), "development".to_string())]);
    let lookup = move |key: &str| vars.get(key).cloned();
    let mut cfg = Config::from_lookup(&lookup);
    cfg.database_path = dir
        .path()
        .join("nightlio.db")
        .to_string_lossy()
        .into_owned();
    cfg
}

/// The exact per-table counts of a SQLite database, in the tool's order.
const TABLES: [&str; 10] = [
    "users",
    "groups",
    "group_options",
    "mood_entries",
    "entry_selections",
    "achievements",
    "goals",
    "goal_completions",
    "user_metrics",
    "activity_log",
];

// ---------------------------------------------------------------------------
// The round trip (gated on NIGHTLIO_PG_TEST_URL)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn round_trip_preserves_rows_sequences_and_the_wire_contract() {
    if support::pg_test_url().is_none() {
        eprintln!("skipping migrate-to-postgres round trip: NIGHTLIO_PG_TEST_URL not set");
        return;
    }

    // --- 1. Build the SQLite source through the real app -------------------
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = dev_config(&dir);
    let db_path = cfg.database_path.clone();
    let secret = cfg.jwt_secret.clone();
    db::bootstrap(&db_path, &SelfHostSeed::from(&cfg)).expect("bootstrap");
    let sqlite_pool = db::open_pool(&db_path).expect("pool");
    let sqlite_app = routes::build_router(AppState::new(
        cfg,
        db::DbHandle::Sqlite(sqlite_pool.clone()),
    ));
    let token = jwt::issue_token(&secret, 1).expect("token");

    // The recorder's pre-scenario seed: group 4 `Activities`, option 28
    // `Exercise` (defaults occupy 1..=3 / 1..=27).
    {
        let conn = db::connect(&db_path).expect("connect");
        conn.execute(
            "INSERT INTO groups (user_id, name) VALUES (1, 'Activities')",
            [],
        )
        .unwrap();
        assert_eq!(conn.last_insert_rowid(), 4);
        conn.execute(
            "INSERT INTO group_options (group_id, name) VALUES (4, 'Exercise')",
            [],
        )
        .unwrap();
        assert_eq!(conn.last_insert_rowid(), 28);
    }

    // The three recorded entries, through the real routes (ids 1..=3).
    check_fixture(&sqlite_app, &token, "mood_post_created_first_entry").await;
    check_fixture(&sqlite_app, &token, "mood_post_created_us_date").await;
    let request = Request::builder()
        .method("POST")
        .uri("/api/mood")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({ "mood": 5, "date": "2025-08-03", "content": "Third day.", "selected_options": [] })
                .to_string(),
        ))
        .unwrap();
    let response = sqlite_app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status().as_u16(), 201);

    // Historical quirks, inserted directly (the routes normalize them away
    // today, but real pre-v0.4 databases contain them): a second user with
    // a US-format garbage date and a schema-legal REAL mood.
    {
        let conn = db::connect(&db_path).expect("connect");
        conn.execute(
            "INSERT INTO users (google_id, email, name) VALUES ('quirky', 'quirky@example.com', 'Quirk')",
            [],
        )
        .unwrap();
        assert_eq!(conn.last_insert_rowid(), 2, "quirky user must get id 2");
        conn.execute(
            "INSERT INTO mood_entries (user_id, date, mood, content) VALUES (2, '8/2/2025', 4.5, 'Garbage date, REAL mood.')",
            [],
        )
        .unwrap();
        assert_eq!(conn.last_insert_rowid(), 4, "quirky entry must get id 4");
    }
    // Fold the WAL so the binary's read-only open sees a clean file.
    db::checkpoint_truncate(&db::DbHandle::Sqlite(sqlite_pool.clone())).expect("checkpoint");

    let source_counts: Vec<(String, i64)> = {
        let conn = db::connect(&db_path).expect("connect");
        TABLES
            .iter()
            .map(|table| {
                let count: i64 = conn
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                        row.get(0)
                    })
                    .unwrap();
                ((*table).to_string(), count)
            })
            .collect()
    };
    assert!(
        source_counts
            .iter()
            .any(|(table, count)| table == "achievements" && *count > 0),
        "the scenario must earn at least one achievement (nft_minted copy coverage)"
    );

    // --- 2. Import into a scratch (empty, unmigrated) PostgreSQL DB --------
    let pg_url = support::create_empty_pg_db("roundtrip").await;
    let output = Command::new(BIN)
        .args([
            "migrate-to-postgres",
            "--sqlite",
            &db_path,
            "--postgres-url",
            &pg_url,
        ])
        .current_dir(dir.path())
        .output()
        .expect("run nightlio-api migrate-to-postgres");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "import failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("Import complete"), "{stdout}");
    assert!(!stdout.contains("MISMATCH"), "{stdout}");

    // --- 3. Row counts, quirky rows, sequence integrity --------------------
    let client = support::pg_connect(&pg_url).await;
    for (table, expected) in &source_counts {
        let count: i64 = client
            .query_one(&format!("SELECT COUNT(*) FROM {table}"), &[])
            .await
            .unwrap()
            .get(0);
        assert_eq!(count, *expected, "{table}: row count after import");
    }

    // Ids preserved verbatim, including the garbage date and the REAL mood.
    let row = client
        .query_one(
            "SELECT id, date, mood, content FROM mood_entries WHERE user_id = 2",
            &[],
        )
        .await
        .unwrap();
    assert_eq!(row.get::<_, i64>(0), 4);
    assert_eq!(row.get::<_, String>(1), "8/2/2025");
    assert_eq!(row.get::<_, f64>(2), 4.5);
    assert_eq!(row.get::<_, String>(3), "Garbage date, REAL mood.");
    let mood_of_first: f64 = client
        .query_one("SELECT mood FROM mood_entries WHERE id = 1", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(mood_of_first, 4.0);

    // setval: the next generated ids continue after the imported maxima
    // (probe rows are removed again so the wire replay below sees exactly
    // the imported state; sequences do not rewind, which is fine — no
    // fixture pins the NEXT id of these tables).
    let probe_user: i64 = client
        .query_one(
            "INSERT INTO users (google_id, email, name) VALUES ('seq-probe', 'p@example.com', 'P') RETURNING id",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        probe_user, 3,
        "users sequence must continue after max(id)=2"
    );
    let probe_entry: i64 = client
        .query_one(
            "INSERT INTO mood_entries (user_id, date, mood, content) VALUES (2, '2025-08-09', 3, 'probe') RETURNING id",
            &[],
        )
        .await
        .unwrap()
        .get(0);
    assert_eq!(
        probe_entry, 5,
        "mood_entries sequence must continue after max(id)=4"
    );
    client
        .execute("DELETE FROM mood_entries WHERE id = $1", &[&probe_entry])
        .await
        .unwrap();
    client
        .execute("DELETE FROM users WHERE id = $1", &[&probe_user])
        .await
        .unwrap();

    // --- 4. Refuse-unless-empty: a second run must abort -------------------
    let output = Command::new(BIN)
        .args([
            "migrate-to-postgres",
            "--sqlite",
            &db_path,
            "--postgres-url",
            &pg_url,
        ])
        .current_dir(dir.path())
        .output()
        .expect("re-run nightlio-api migrate-to-postgres");
    assert_eq!(output.status.code(), Some(1), "second run must refuse");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Refusing to import"), "{stderr}");
    let users_after: i64 = client
        .query_one("SELECT COUNT(*) FROM users", &[])
        .await
        .unwrap()
        .get(0);
    assert_eq!(users_after, 2, "the refused run must not write anything");

    // --- 5. Boot a PG-backed app on the import and replay the same recorded
    //        fixtures the SQLite harness is graded against ------------------
    let pg_dir = tempfile::tempdir().expect("tempdir");
    let mut pg_cfg = dev_config(&pg_dir);
    pg_cfg.jwt_secret = secret.clone();
    let pg_pool = db::pg::build_pool(&pg_url).expect("pg pool");
    let pg_app = routes::build_router(AppState::new(pg_cfg, db::DbHandle::Pg(pg_pool)));
    for name in [
        "moods_get_populated",
        "moods_get_date_range",
        "mood_get_by_id",
        "mood_selections_populated",
        "statistics_populated",
        "statistics_heatmap_populated",
        "statistics_digest_populated",
        "statistics_extended_populated",
    ] {
        check_fixture(&pg_app, &token, name).await;
    }

    // The quirky rows on the wire: served verbatim by the PG-backed app
    // (float mood stays a float, the garbage date is untouched).
    let quirky_token = jwt::issue_token(&secret, 2).expect("token");
    let body = get_authed(&pg_app, &quirky_token, "/api/moods").await;
    let entries = body.as_array().expect("moods array");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["id"], json!(4));
    assert_eq!(entries[0]["date"], json!("8/2/2025"));
    assert_eq!(entries[0]["mood"], json!(4.5));
}
