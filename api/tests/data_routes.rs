//! Data route family (v0.6.0 export/import) vs the hand-authored spec
//! fixtures in `contract/fixtures/data/` (this family never existed in
//! Flask — the fixtures ARE the contract, DECISIONS.md 2026-08-24): every
//! scenario replays the recorded request against the full assembled router
//! on a bootstrapped database, asserts status + content type + body with
//! the fixture conventions' volatile-value normalization
//! (`created_at`/`updated_at`/`exported_at` → `<TS>`, live crate version →
//! `<VERSION>`, bare ISO dates → `<DATE>`), then pins the normalized
//! `{status, body}` pair as an insta snapshot.
//!
//! Beyond one parity test per fixture: idempotent re-import, group/option
//! resolve-or-create, an A→B export/import round trip, whole-file
//! atomicity, the 8 MiB 413 cap, and the rollover-on-read self-heal of
//! raw imported weekly state.
//!
//! Every test loops over the available backends (`support::backends()`):
//! SQLite always, plus a PostgreSQL twin replaying the same fixtures
//! byte-identically when `NIGHTLIO_PG_TEST_URL` is set.

use std::collections::HashMap;

mod support;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
use chrono::Datelike;
use serde_json::{Value, json};
use tower::ServiceExt;

use nightlio_api::auth::jwt;
use nightlio_api::config::Config;
use nightlio_api::db::{self, SelfHostSeed};
use nightlio_api::routes;
use nightlio_api::state::AppState;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct TestApp {
    app: Router,
    /// Bearer token for the bootstrapped self-host user (id 1).
    token: String,
    db: support::TestDb,
}

/// Fresh bootstrapped state on each available backend: self-host user id 1,
/// default groups 1-3 (Emotions, Sleep, Productivity), options 1-27 — the
/// exact baseline the fixture notes seed on top of.
async fn make_apps() -> Vec<TestApp> {
    let mut apps = Vec::new();
    for backend in support::backends() {
        apps.push(make_app_on(backend).await);
    }
    apps
}

async fn make_app_on(backend: support::Backend) -> TestApp {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("nightlio.db");
    let vars: HashMap<String, String> =
        HashMap::from([("APP_ENV".to_string(), "development".to_string())]);
    let lookup = move |key: &str| vars.get(key).cloned();
    let mut cfg = Config::from_lookup(&lookup);
    cfg.database_path = db_path.to_string_lossy().into_owned();
    let token = jwt::issue_token(&cfg.jwt_secret, 1).expect("token");
    let (handle, test_db) = match backend {
        support::Backend::Sqlite => {
            db::bootstrap(&cfg.database_path, &SelfHostSeed::from(&cfg)).expect("bootstrap");
            let pool = db::open_pool(&cfg.database_path).expect("pool");
            (
                db::DbHandle::Sqlite(pool),
                support::TestDb::Sqlite {
                    path: cfg.database_path.clone(),
                    _dir: dir,
                },
            )
        }
        support::Backend::Pg => {
            let url = support::create_pg_db(&SelfHostSeed::from(&cfg)).await;
            let pool = db::pg::build_pool(&url).expect("pg pool");
            (
                db::DbHandle::Pg(pool),
                support::TestDb::Pg { url, _dir: dir },
            )
        }
    };
    let state = AppState::new(cfg, handle);
    TestApp {
        app: routes::build_router(state),
        token,
        db: test_db,
    }
}

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/../contract/fixtures/data/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|exc| panic!("missing fixture {path}: {exc}"));
    serde_json::from_str(&raw).expect("fixture json")
}

async fn body_json(response: Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), 32 << 20)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("json body")
}

async fn body_bytes(response: Response) -> Vec<u8> {
    axum::body::to_bytes(response.into_body(), 32 << 20)
        .await
        .expect("body")
        .to_vec()
}

/// ISO Monday of the current local week — what the rollover projection
/// repoints stale `period_start`s to.
fn current_monday() -> String {
    let today = chrono::Local::now().date_naive();
    (today - chrono::Duration::days(i64::from(today.weekday().num_days_from_monday())))
        .format("%Y-%m-%d")
        .to_string()
}

/// Rebuild the recorded request: method + path verbatim; `Authorization` /
/// `Cookie` replaced with a real JWT for the seeded user when the fixture
/// carried one; `Content-Type` verbatim; JSON bodies serialized, string
/// bodies (the not-JSON fixture) sent as raw bytes.
async fn replay(app: &TestApp, fix: &Value) -> Response {
    let method = fix["method"].as_str().expect("method");
    let path = fix["path"].as_str().expect("path");
    let mut builder = Request::builder().method(method).uri(path);
    let headers = &fix["request"]["headers_that_matter"];
    if headers.get("Authorization").is_some() {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {}", app.token));
    }
    if headers.get("Cookie").is_some() {
        builder = builder.header(header::COOKIE, format!("nightlio_token={}", app.token));
    }
    if let Some(content_type) = headers.get("Content-Type").and_then(Value::as_str) {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    let body = match &fix["request"]["body"] {
        Value::Null => Body::empty(),
        Value::String(raw) => Body::from(raw.clone().into_bytes()),
        other => Body::from(serde_json::to_vec(other).expect("body json")),
    };
    app.app
        .clone()
        .oneshot(builder.body(body).expect("request"))
        .await
        .expect("infallible")
}

/// Fixture-style normalization of volatile values, recursively:
/// - `created_at`/`updated_at`/`exported_at` strings → `<TS>`;
/// - `app_version` → `<VERSION>` (asserted to be the live crate version,
///   the `config_get_200.json` convention);
/// - bare ISO dates (`date`, `period_start`, `last_completed_date`, each
///   `completions` element) → `<DATE>`; JSON nulls stay null.
fn normalize(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, item) in map.iter_mut() {
                match key.as_str() {
                    "created_at" | "updated_at" | "exported_at" if item.is_string() => {
                        assert!(
                            item.as_str().is_some_and(|ts| !ts.is_empty()),
                            "{key} must be populated"
                        );
                        *item = Value::String("<TS>".to_string());
                    }
                    "app_version" if item.is_string() => {
                        assert_eq!(
                            item.as_str(),
                            Some(env!("CARGO_PKG_VERSION")),
                            "app_version must be the live crate version"
                        );
                        *item = Value::String("<VERSION>".to_string());
                    }
                    "date" | "period_start" | "last_completed_date" if item.is_string() => {
                        *item = Value::String("<DATE>".to_string());
                    }
                    "completions" if item.is_array() => {
                        for completion in item.as_array_mut().expect("array") {
                            if completion.is_string() {
                                *completion = Value::String("<DATE>".to_string());
                            }
                        }
                    }
                    _ => normalize(item),
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

/// Replay a fixture, assert full parity (status, JSON content type,
/// normalized body), and pin the result as an insta snapshot named after
/// the fixture.
async fn assert_fixture_parity(app: &TestApp, name: &str) {
    let fix = fixture(name);
    let response = replay(app, &fix).await;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let mut body = body_json(response).await;
    normalize(&mut body);

    let expected = &fix["response"];
    assert_eq!(
        u64::from(status),
        expected["status"].as_u64().expect("status"),
        "{name}: status"
    );
    assert!(
        content_type.starts_with("application/json"),
        "{name}: content type was {content_type:?}"
    );
    assert_eq!(body, expected["body"], "{name}: body");

    let snapshot_name = name.trim_end_matches(".json");
    insta::assert_json_snapshot!(snapshot_name, json!({ "status": status, "body": body }));
}

/// Drive a mutation through the real routes during scenario setup.
async fn setup_request(app: &TestApp, method: &str, path: &str, body: Value) -> Value {
    let response = app
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {}", app.token))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .expect("infallible");
    assert!(
        response.status().is_success(),
        "setup {method} {path} failed: {}",
        response.status()
    );
    body_json(response).await
}

/// A plain authenticated JSON request whose response the test inspects.
async fn bearer_json(app: &TestApp, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {}", app.token));
    let body = if body.is_null() {
        Body::empty()
    } else {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        Body::from(serde_json::to_vec(&body).unwrap())
    };
    let response = app
        .app
        .clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .expect("infallible");
    let status = response.status();
    (status, body_json(response).await)
}

/// Seed the account state `export_data_get_200.json` was authored against
/// (order from the fixture note): two entries (one with default-option
/// selections 1=happy, 5=content), the Meditate goal with today's progress
/// logged, and the Read goal.
async fn seed_export_fixture_state(app: &TestApp) {
    setup_request(
        app,
        "POST",
        "/api/mood",
        json!({
            "mood": 4,
            "date": "2025-08-01",
            "content": "Great workout day.",
            "selected_options": [1, 5]
        }),
    )
    .await;
    setup_request(
        app,
        "POST",
        "/api/mood",
        json!({ "mood": 2, "date": "2025-08-02", "content": "Rough night." }),
    )
    .await;
    setup_request(
        app,
        "POST",
        "/api/goals",
        json!({ "title": "Meditate", "description": "10 minutes", "frequency_per_week": 3 }),
    )
    .await;
    setup_request(app, "POST", "/api/goals/1/progress", json!({})).await;
    setup_request(
        app,
        "POST",
        "/api/goals",
        json!({ "title": "Read", "frequency_per_week": 2 }),
    )
    .await;
}

// ---------------------------------------------------------------------------
// GET /api/export/data
// ---------------------------------------------------------------------------

#[tokio::test]
async fn export_data_get_200() {
    for app in make_apps().await {
        seed_export_fixture_state(&app).await;
        assert_fixture_parity(&app, "export_data_get_200.json").await;
    }
}

#[tokio::test]
async fn export_data_get_200_empty() {
    for app in make_apps().await {
        assert_fixture_parity(&app, "export_data_get_200_empty.json").await;
    }
}

#[tokio::test]
async fn export_data_get_401() {
    for app in make_apps().await {
        assert_fixture_parity(&app, "export_data_get_401.json").await;
    }
}

#[tokio::test]
async fn export_data_get_trailing_slash_404() {
    for app in make_apps().await {
        assert_fixture_parity(&app, "export_data_get_trailing_slash_404.json").await;
    }
}

#[tokio::test]
async fn export_data_options_204() {
    for app in make_apps().await {
        let fix = fixture("export_data_options_204.json");
        let response = replay(&app, &fix).await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            response
                .headers()
                .get(header::ALLOW)
                .and_then(|value| value.to_str().ok()),
            fix["response"]["headers_that_matter"]["Allow"].as_str()
        );
        assert_eq!(body_bytes(response).await, b"");
    }
}

// ---------------------------------------------------------------------------
// POST /api/import/data — success paths
// ---------------------------------------------------------------------------

#[tokio::test]
async fn import_data_post_200_then_idempotent_reimport() {
    for app in make_apps().await {
        // Fixture note: pre-seed one duplicate per category BEFORE the
        // import — the file's first entry and first goal must skip.
        setup_request(
            &app,
            "POST",
            "/api/mood",
            json!({
                "mood": 4,
                "date": "2025-08-01",
                "content": "Great workout day.",
                "selected_options": [1, 5]
            }),
        )
        .await;
        setup_request(
            &app,
            "POST",
            "/api/goals",
            json!({ "title": "Meditate", "description": "10 minutes", "frequency_per_week": 3 }),
        )
        .await;
        assert_fixture_parity(&app, "import_data_post_200.json").await;

        // Re-POSTing the same body is idempotent: everything skips, counts
        // stay per file row.
        let fix = fixture("import_data_post_200.json");
        let (status, body) = bearer_json(
            &app,
            "POST",
            "/api/import/data",
            fix["request"]["body"].clone(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            json!({
                "status": "success",
                "entries": { "imported": 0, "skipped": 2 },
                "goals": { "imported": 0, "skipped": 2 },
            }),
            "re-import must skip every row"
        );
    }
}

#[tokio::test]
async fn import_data_post_200_all_new_empty_db_then_rollover_self_heals_on_read() {
    for app in make_apps().await {
        assert_fixture_parity(&app, "import_data_post_200_all_new_empty_db.json").await;

        // The stale 2025-08-04 period_start was written raw…
        let raw_period: String = match &app.db {
            support::TestDb::Sqlite { path, .. } => {
                let conn = db::connect(path).expect("connect");
                conn.query_row(
                    "SELECT period_start FROM goals WHERE title = 'Read'",
                    [],
                    |row| row.get(0),
                )
                .unwrap()
            }
            support::TestDb::Pg { .. } => {
                let client = app.db.pg_client().await;
                client
                    .query_one("SELECT period_start FROM goals WHERE title = 'Read'", &[])
                    .await
                    .unwrap()
                    .get(0)
            }
        };
        assert_eq!(raw_period, "2025-08-04", "import writes weekly state raw");

        // …and the goals read projects the rollover onto the response
        // (period repointed to the current Monday, counter reset) without
        // the import ever recomputing anything.
        let (status, goals) = bearer_json(&app, "GET", "/api/goals", Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        let goals = goals.as_array().expect("array");
        assert_eq!(goals.len(), 1);
        let goal = &goals[0];
        assert_eq!(goal["title"], "Read");
        assert_eq!(goal["period_start"], json!(current_monday()));
        assert_eq!(goal["completed"], json!(0));
        assert_eq!(goal["streak"], json!(0));
        assert_eq!(goal["last_completed_date"], Value::Null);
        assert_eq!(goal["already_completed_today"], json!(false));
    }
}

/// Selections resolve-or-create by exact name: an existing group/option
/// pair resolves to the seeded rows, a new pair creates the group and
/// option exactly once — including across entries within one file (the
/// preloaded cache is the dedupe; `group_options` has no unique index).
#[tokio::test]
async fn import_resolves_existing_names_and_creates_new_ones_once() {
    for app in make_apps().await {
        let file = json!({
            "schema_version": 1,
            "exported_at": "2025-08-04 09:00:00",
            "app_version": "0.6.0",
            "data": {
                "entries": [
                    {
                        "date": "2025-08-01", "mood": 3, "content": "One",
                        "created_at": "2025-08-01 08:00:00",
                        "updated_at": "2025-08-01 08:00:00",
                        "selections": [
                            { "group_name": "Emotions", "option_name": "happy" },
                            { "group_name": "Trips", "option_name": "beach" }
                        ]
                    },
                    {
                        "date": "2025-08-02", "mood": 4, "content": "Two",
                        "created_at": "2025-08-02 08:00:00",
                        "updated_at": "2025-08-02 08:00:00",
                        "selections": [
                            { "group_name": "Trips", "option_name": "beach" },
                            { "group_name": "Trips", "option_name": "hike" }
                        ]
                    }
                ],
                "goals": []
            }
        });
        let (status, body) = bearer_json(&app, "POST", "/api/import/data", file).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["entries"], json!({ "imported": 2, "skipped": 0 }));

        let (status, groups) = bearer_json(&app, "GET", "/api/groups", Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        let groups = groups.as_array().expect("array");
        assert_eq!(groups.len(), 4, "exactly one Trips group created");
        let trips: Vec<&Value> = groups
            .iter()
            .filter(|group| group["name"] == "Trips")
            .collect();
        assert_eq!(trips.len(), 1);
        let trip_options: Vec<&str> = trips[0]["options"]
            .as_array()
            .expect("options")
            .iter()
            .map(|option| option["name"].as_str().unwrap())
            .collect();
        assert_eq!(trip_options, ["beach", "hike"], "beach created once");
        let emotions = groups
            .iter()
            .find(|group| group["name"] == "Emotions")
            .expect("Emotions");
        assert_eq!(
            emotions["options"].as_array().unwrap().len(),
            13,
            "existing group untouched"
        );

        // The first entry's selections: happy resolved to the SEEDED
        // option (id 1), beach to the newly created one.
        let (status, selections) =
            bearer_json(&app, "GET", "/api/mood/1/selections", Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            selections[0],
            json!({ "id": 1, "name": "happy", "group_name": "Emotions" }),
            "existing option must resolve, not duplicate"
        );
        assert_eq!(selections[1]["name"], "beach");
        assert_eq!(selections[1]["group_name"], "Trips");
    }
}

/// A→B round trip: everything a real account exports imports into a fresh
/// instance, and the fresh instance's export is byte-identical (timestamps
/// included — they are preserved) apart from `exported_at`.
#[tokio::test]
async fn export_import_roundtrip_a_to_b() {
    for backend in support::backends() {
        let app_a = make_app_on(backend).await;
        let app_b = make_app_on(backend).await;
        seed_export_fixture_state(&app_a).await;

        let (status, mut export_a) =
            bearer_json(&app_a, "GET", "/api/export/data", Value::Null).await;
        assert_eq!(status, StatusCode::OK);

        let (status, result) =
            bearer_json(&app_b, "POST", "/api/import/data", export_a.clone()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            result,
            json!({
                "status": "success",
                "entries": { "imported": 2, "skipped": 0 },
                "goals": { "imported": 2, "skipped": 0 },
            })
        );

        let (status, mut export_b) =
            bearer_json(&app_b, "GET", "/api/export/data", Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        export_a["exported_at"] = json!("<TS>");
        export_b["exported_at"] = json!("<TS>");
        assert_eq!(
            export_b, export_a,
            "roundtripped export must be identical (timestamps preserved)"
        );
    }
}

// ---------------------------------------------------------------------------
// POST /api/import/data — failure paths
// ---------------------------------------------------------------------------

#[tokio::test]
async fn import_data_post_400_invalid_row_is_atomic() {
    for app in make_apps().await {
        assert_fixture_parity(&app, "import_data_post_400_invalid_row.json").await;

        // Whole-file atomicity: the VALID entries[0] must not have been
        // imported — the single transaction never opened.
        let (status, moods) = bearer_json(&app, "GET", "/api/moods", Value::Null).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(moods, json!([]), "nothing imports on a validation failure");
    }
}

/// A failure in the goals category also rolls back the entries category —
/// the file is one unit.
#[tokio::test]
async fn import_invalid_goal_rejects_valid_entries_too() {
    for app in make_apps().await {
        let file = json!({
            "schema_version": 1,
            "data": {
                "entries": [
                    { "date": "2025-08-01", "mood": 4, "content": "Valid.", "selections": [] }
                ],
                "goals": [
                    { "title": "Bad", "frequency_per_week": 9 }
                ]
            }
        });
        let (status, body) = bearer_json(&app, "POST", "/api/import/data", file).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            body,
            json!({ "error": "goals[0]: frequency_per_week must be between 1 and 7" })
        );
        let (_, moods) = bearer_json(&app, "GET", "/api/moods", Value::Null).await;
        assert_eq!(moods, json!([]));
        let (_, goals) = bearer_json(&app, "GET", "/api/goals", Value::Null).await;
        assert_eq!(goals, json!([]));
    }
}

#[tokio::test]
async fn import_data_post_400_missing_schema_version() {
    for app in make_apps().await {
        assert_fixture_parity(&app, "import_data_post_400_missing_schema_version.json").await;
    }
}

#[tokio::test]
async fn import_data_post_400_unsupported_schema_version() {
    for app in make_apps().await {
        assert_fixture_parity(&app, "import_data_post_400_unsupported_schema_version.json").await;
    }
}

#[tokio::test]
async fn import_data_post_400_not_json() {
    for app in make_apps().await {
        assert_fixture_parity(&app, "import_data_post_400_not_json.json").await;
    }
}

#[tokio::test]
async fn import_data_post_401() {
    for app in make_apps().await {
        assert_fixture_parity(&app, "import_data_post_401.json").await;
    }
}

#[tokio::test]
async fn import_data_post_403_cookie_missing_csrf() {
    for app in make_apps().await {
        assert_fixture_parity(&app, "import_data_post_403_cookie_missing_csrf.json").await;
    }
}

/// In-handler cap: a body over 8 MiB (8388608 bytes) is a 413 with the
/// JSON error envelope — code-derived, no fixture (the `MAX_PDF_CONTENT_SIZE`
/// pattern; the transport `DefaultBodyLimit` sits higher, at 16 MiB).
#[tokio::test]
async fn import_data_post_413_over_cap() {
    for app in make_apps().await {
        let oversized = "A".repeat(8 * 1024 * 1024 + 1);
        let response = app
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/import/data")
                    .header(header::AUTHORIZATION, format!("Bearer {}", app.token))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(oversized))
                    .unwrap(),
            )
            .await
            .expect("infallible");
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let body = body_json(response).await;
        assert!(
            body["error"].as_str().is_some_and(|msg| !msg.is_empty()),
            "413 keeps the JSON error envelope: {body}"
        );

        // Nothing was imported.
        let (_, moods) = bearer_json(&app, "GET", "/api/moods", Value::Null).await;
        assert_eq!(moods, json!([]));
    }
}

// ---------------------------------------------------------------------------
// Routing regimes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn import_data_get_405() {
    for app in make_apps().await {
        let fix = fixture("import_data_get_405.json");
        let response = replay(&app, &fix).await;
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        let allow = response
            .headers()
            .get(header::ALLOW)
            .and_then(|value| value.to_str().ok())
            .expect("Allow on 405")
            .to_string();
        // Method order in the synthesized Allow is not graded.
        assert!(allow.contains("POST"), "Allow was {allow:?}");
        assert!(allow.contains("OPTIONS"), "Allow was {allow:?}");
        let body = body_json(response).await;
        assert_eq!(body, fix["response"]["body"]);
        insta::assert_json_snapshot!(
            "import_data_get_405",
            json!({ "status": 405, "body": body })
        );
    }
}

#[tokio::test]
async fn import_data_options_204() {
    for app in make_apps().await {
        let fix = fixture("import_data_options_204.json");
        let response = replay(&app, &fix).await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            response
                .headers()
                .get(header::ALLOW)
                .and_then(|value| value.to_str().ok()),
            fix["response"]["headers_that_matter"]["Allow"].as_str()
        );
        assert_eq!(body_bytes(response).await, b"");
    }
}

/// The import slash variant 404s exactly like the export one (fixture note:
/// "POST /api/import/data/ 404s identically").
#[tokio::test]
async fn import_data_post_trailing_slash_404() {
    for app in make_apps().await {
        let (status, body) = bearer_json(
            &app,
            "POST",
            "/api/import/data/",
            json!({ "schema_version": 1, "data": { "entries": [], "goals": [] } }),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body, json!({ "error": "Resource not found" }));
    }
}
