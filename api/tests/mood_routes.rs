//! Mood + statistics route family vs the golden fixtures in
//! `contract/fixtures/mood/`.
//!
//! Each `check_fixture` call replays the recorded request against the full
//! assembled router (state + middleware) via `tower::ServiceExt::oneshot`,
//! normalizes volatile values exactly like the fixtures do
//! (`created_at`/`updated_at` → `<TS>`, the extended digest's current
//! year/month → `<CURRENT_YEAR>`/`<CURRENT_MONTH>`), asserts status + body
//! equality against the fixture, and pins the normalized response as an
//! `insta` snapshot.
//!
//! The populated scenario reproduces the recorder's seed: the bootstrap
//! self-host user (id 1) plus a group `Activities` (id 4) with option 28
//! `Exercise`, then the three recorded POSTs — so ids and aggregates are
//! deterministic and identical to what the Python app produced.
//!
//! Since v0.6.0 every test loops over the available backends
//! (`support::backends()`): SQLite always, exactly as before, plus a
//! PostgreSQL twin replaying the same fixtures byte-identically when
//! `NIGHTLIO_PG_TEST_URL` is set (see `tests/support/mod.rs`).

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
    token: String,
    db: support::TestDb,
}

/// One app per available backend: SQLite always (the pre-0.6.0 harness,
/// unchanged), plus a PostgreSQL twin when `NIGHTLIO_PG_TEST_URL` is set.
async fn make_apps() -> Vec<TestApp> {
    let mut apps = Vec::new();
    for backend in support::backends() {
        apps.push(make_app_on(backend).await);
    }
    apps
}

async fn make_app_on(backend: support::Backend) -> TestApp {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir
        .path()
        .join("nightlio.db")
        .to_string_lossy()
        .into_owned();
    let vars: HashMap<String, String> =
        HashMap::from([("APP_ENV".to_string(), "development".to_string())]);
    let lookup = move |key: &str| vars.get(key).cloned();
    let mut cfg = Config::from_lookup(&lookup);
    cfg.database_path = db_path.clone();
    let secret = cfg.jwt_secret.clone();
    let (handle, test_db) = match backend {
        support::Backend::Sqlite => {
            db::bootstrap(&cfg.database_path, &SelfHostSeed::from(&cfg)).expect("bootstrap");
            let pool = db::open_pool(&cfg.database_path).expect("pool");
            (
                db::DbHandle::Sqlite(pool),
                support::TestDb::Sqlite {
                    path: db_path,
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
        // Bootstrap (and its PG-side equivalent seed) gives the default
        // self-host user id 1.
        token: jwt::issue_token(&secret, 1).expect("token"),
        db: test_db,
    }
}

/// Recreate the recorder's pre-scenario state: group 4 `Activities` with
/// option 28 `Exercise` (the bootstrap default groups occupy ids 1..=3 and
/// options 1..=27).
async fn seed_activities_group(db: &support::TestDb) {
    match db {
        support::TestDb::Sqlite { path, .. } => {
            let conn = db::connect(path).expect("connect");
            conn.execute(
                "INSERT INTO groups (user_id, name) VALUES (1, 'Activities')",
                [],
            )
            .expect("insert group");
            assert_eq!(
                conn.last_insert_rowid(),
                4,
                "Activities group must get id 4"
            );
            conn.execute(
                "INSERT INTO group_options (group_id, name) VALUES (4, 'Exercise')",
                [],
            )
            .expect("insert option");
            assert_eq!(
                conn.last_insert_rowid(),
                28,
                "Exercise option must get id 28"
            );
        }
        support::TestDb::Pg { .. } => {
            let client = db.pg_client().await;
            let group_id: i64 = client
                .query_one(
                    "INSERT INTO groups (user_id, name) VALUES (1, 'Activities') RETURNING id",
                    &[],
                )
                .await
                .expect("insert group")
                .get(0);
            assert_eq!(group_id, 4, "Activities group must get id 4");
            let option_id: i64 = client
                .query_one(
                    "INSERT INTO group_options (group_id, name) VALUES (4, 'Exercise') RETURNING id",
                    &[],
                )
                .await
                .expect("insert option")
                .get(0);
            assert_eq!(option_id, 28, "Exercise option must get id 28");
        }
    }
}

async fn send(app: &Router, request: Request<Body>) -> Response {
    app.clone().oneshot(request).await.expect("infallible")
}

async fn body_json(response: Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("json body")
}

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/../contract/fixtures/mood/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|exc| panic!("missing fixture {path}: {exc}"));
    serde_json::from_str(&raw).expect("fixture json")
}

/// Fixture-style normalization: timestamps → `<TS>`; inside a
/// `monthly_digest` object, the CURRENT local year/month →
/// `<CURRENT_YEAR>`/`<CURRENT_MONTH>` (only the extended endpoint embeds
/// one keyed like that; explicit digest responses keep their literal
/// year/month).
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

/// Replay a recorded fixture, assert status + normalized body equality, and
/// pin the normalized `{status, body}` pair as an insta snapshot.
async fn check_fixture(app: &Router, token: &str, name: &str) {
    let fx = fixture(name);
    let response = send(app, build_request(&fx, token)).await;
    let status = response.status().as_u16();
    assert_eq!(
        u64::from(status),
        fx["response"]["status"].as_u64().expect("fixture status"),
        "{name}: status"
    );
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(
        content_type.starts_with("application/json"),
        "{name}: expected JSON content type, got {content_type:?}"
    );
    let mut actual = body_json(response).await;
    normalize(&mut actual);
    assert_eq!(actual, fx["response"]["body"], "{name}: body");
    insta::assert_json_snapshot!(name, json!({ "status": status, "body": actual }));
}

/// A plain authenticated JSON request outside the recorded fixtures (used
/// to reproduce the recorder's intermediate seeding steps).
async fn send_json(
    app: &Router,
    token: &str,
    method: &str,
    path: &str,
    body: Value,
) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).expect("body")))
        .expect("request");
    let response = send(app, request).await;
    let status = response.status();
    (status, body_json(response).await)
}

async fn get_authed(app: &Router, token: &str, path: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .expect("request");
    let response = send(app, request).await;
    let status = response.status();
    (status, body_json(response).await)
}

// ---------------------------------------------------------------------------
// 401s (no credential at all)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unauthenticated_requests_match_fixtures() {
    for test in make_apps().await {
        let (app, token) = (test.app.clone(), test.token.clone());
        for name in [
            "mood_post_401",
            "moods_get_401",
            "mood_get_401",
            "mood_put_401",
            "mood_delete_401",
            "mood_selections_401",
            "statistics_401",
            "statistics_view_401",
            "statistics_extended_401",
            "statistics_heatmap_401",
            "statistics_digest_401",
            "streak_401",
        ] {
            check_fixture(&app, &token, name).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Empty account
// ---------------------------------------------------------------------------

#[tokio::test]
async fn empty_account_matches_fixtures() {
    for test in make_apps().await {
        let (app, token) = (test.app.clone(), test.token.clone());
        for name in [
            "moods_get_empty",
            "statistics_empty",
            "statistics_extended_empty",
            "statistics_heatmap_empty",
            "statistics_digest_empty",
            "streak_empty",
        ] {
            check_fixture(&app, &token, name).await;
        }
    }
}

// ---------------------------------------------------------------------------
// POST /mood validation + query-param validation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn validation_errors_match_fixtures() {
    for test in make_apps().await {
        let (app, token) = (test.app.clone(), test.token.clone());
        for name in [
            "mood_post_400_missing_fields",
            "mood_post_400_mood_out_of_range",
            "mood_post_400_bad_date",
            "mood_post_400_bad_selected_options",
            "statistics_heatmap_400_bad_year",
            "statistics_digest_400_bad_month",
        ] {
            check_fixture(&app, &token, name).await;
        }

        // Falsy mood (0) and empty content land in "Missing required fields"
        // BEFORE the range validator (fixture note on missing_fields).
        for body in [
            json!({ "mood": 0, "date": "2025-08-01", "content": "x" }),
            json!({ "mood": 3, "date": "2025-08-01", "content": "" }),
        ] {
            let (status, body) = send_json(&app, &token, "POST", "/api/mood", body).await;
            assert_eq!(status, StatusCode::BAD_REQUEST);
            assert_eq!(body, json!({ "error": "Missing required fields" }));
        }

        // Whitespace-only content passes truthiness but fails the service's
        // strip() check.
        let (status, body) = send_json(
            &app,
            &token,
            "POST",
            "/api/mood",
            json!({ "mood": 3, "date": "2025-08-01", "content": "   " }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body, json!({ "error": "Content cannot be empty" }));

        // Non-integer mood → its own message (after truthiness passes).
        let (status, body) = send_json(
            &app,
            &token,
            "POST",
            "/api/mood",
            json!({ "mood": "high", "date": "2025-08-01", "content": "x" }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body, json!({ "error": "Mood must be an integer" }));
    }
}

// ---------------------------------------------------------------------------
// Full CRUD + statistics scenario (the recorder's populated DB)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn crud_and_statistics_scenario_matches_fixtures() {
    for test in make_apps().await {
        let (app, token) = (test.app.clone(), test.token.clone());
        seed_activities_group(&test.db).await;

        // The two recorded creations (ids 1 and 2)...
        check_fixture(&app, &token, "mood_post_created_first_entry").await;
        check_fixture(&app, &token, "mood_post_created_us_date").await;
        // ...plus the recorder's third entry (no fixture for the POST itself).
        let (status, body) = send_json(
        &app,
        &token,
        "POST",
        "/api/mood",
        json!({ "mood": 5, "date": "2025-08-03", "content": "Third day.", "selected_options": [] }),
    )
    .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body["entry_id"], json!(3));
        assert_eq!(body["new_achievements"], json!([]));

        // Listing: full list and the raw-string BETWEEN range. Contract
        // change: the entry POSTed as 8/2/2025 is stored as 2025-08-02, so it
        // now falls INSIDE the ISO range instead of being excluded.
        check_fixture(&app, &token, "moods_get_populated").await;
        check_fixture(&app, &token, "moods_get_date_range").await;

        // GET by id: plain object without selections; 404 for unknown ids.
        check_fixture(&app, &token, "mood_get_by_id").await;
        check_fixture(&app, &token, "mood_get_404").await;

        // Selections: bare arrays; contract change: nonexistent entry →
        // 404 with GET /mood/{id}'s body (was Flask's 200 []).
        check_fixture(&app, &token, "mood_selections_populated").await;
        check_fixture(&app, &token, "mood_selections_empty").await;
        check_fixture(&app, &token, "mood_selections_nonexistent_entry").await;

        // Statistics family against the recorded pre-PUT state.
        check_fixture(&app, &token, "statistics_populated").await;
        check_fixture(&app, &token, "statistics_heatmap_populated").await;
        check_fixture(&app, &token, "statistics_digest_populated").await;
        check_fixture(&app, &token, "statistics_extended_populated").await;

        // Routing semantics: negative/non-integer ids and the trailing-slash
        // variant 404 with the app-level JSON body.
        check_fixture(&app, &token, "routing_mood_negative_id").await;
        check_fixture(&app, &token, "routing_mood_non_integer_id").await;
        check_fixture(&app, &token, "routing_moods_trailing_slash").await;
        // Overflow (> i64) id: Flask leaks a 500 (routing_mood_overflow_id),
        // but per contract/DECISIONS.md #1 the Rust port returns the clean
        // routing 404 — accepted, documented drift, so only the status and the
        // standard body are asserted here (deliberately NOT via the fixture).
        let (status, body) = get_authed(&app, &token, "/api/mood/99999999999999999999").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body, json!({ "error": "Resource not found" }));

        // Update: embedded entry WITH selections; empty body → 400; unknown
        // id → 404 with its distinct message.
        check_fixture(&app, &token, "mood_put_updated").await;
        check_fixture(&app, &token, "mood_put_400_no_fields").await;
        check_fixture(&app, &token, "mood_put_404").await;

        // PUT shares the selected_options validators with POST.
        let (status, body) = send_json(
            &app,
            &token,
            "PUT",
            "/api/mood/1",
            json!({ "selected_options": "nope" }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            body,
            json!({ "error": "selected_options must be an array" })
        );

        // Delete: the recorder created a fourth entry and deleted it.
        let (status, body) = send_json(
            &app,
            &token,
            "POST",
            "/api/mood",
            json!({ "mood": 3, "date": "2025-08-03", "content": "Fourth entry." }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(body["entry_id"], json!(4));
        check_fixture(&app, &token, "mood_delete_ok").await;
        check_fixture(&app, &token, "mood_delete_404").await;
    }
}

// ---------------------------------------------------------------------------
// contract change: selections 404 on missing/foreign entries, but an
// EXISTING entry keeps 200 [] even when a group delete cascaded its
// selections away
// ---------------------------------------------------------------------------

#[tokio::test]
async fn selections_present_missing_and_foreign_entry_matrix() {
    for test in make_apps().await {
        let (app, token) = (test.app.clone(), test.token.clone());
        seed_activities_group(&test.db).await;
        // Entry 1 belongs to user 1 and selects option 28 (Exercise).
        let (status, _) = send_json(
        &app,
        &token,
        "POST",
        "/api/mood",
        json!({ "mood": 4, "date": "2025-08-01", "content": "Mine.", "selected_options": [28] }),
    )
    .await;
        assert_eq!(status, StatusCode::CREATED);
        // Entry 2 belongs to a second user, inserted directly.
        match &test.db {
            support::TestDb::Sqlite { path, .. } => {
                let conn = db::connect(path).expect("connect");
                conn.execute(
                "INSERT INTO users (google_id, email, name) VALUES ('other-user', 'other@example.com', 'Other')",
                [],
            )
            .unwrap();
                assert_eq!(conn.last_insert_rowid(), 2, "second user must get id 2");
                conn.execute(
                "INSERT INTO mood_entries (user_id, date, mood, content) VALUES (2, '2025-08-01', 3, 'Not yours.')",
                [],
            )
            .unwrap();
            }
            support::TestDb::Pg { .. } => {
                let client = test.db.pg_client().await;
                let user_id: i64 = client
                .query_one(
                    "INSERT INTO users (google_id, email, name) VALUES ('other-user', 'other@example.com', 'Other') RETURNING id",
                    &[],
                )
                .await
                .unwrap()
                .get(0);
                assert_eq!(user_id, 2, "second user must get id 2");
                client
                .execute(
                    "INSERT INTO mood_entries (user_id, date, mood, content) VALUES (2, '2025-08-01', 3, 'Not yours.')",
                    &[],
                )
                .await
                .unwrap();
            }
        }

        // Present → 200 with the selection rows.
        let (status, body) = get_authed(&app, &token, "/api/mood/1/selections").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            body,
            json!([{ "id": 28, "name": "Exercise", "group_name": "Activities" }])
        );

        // Missing and foreign entries → the same 404 body GET /mood/{id} uses.
        for path in ["/api/mood/999/selections", "/api/mood/2/selections"] {
            let (status, body) = get_authed(&app, &token, path).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
            assert_eq!(body, json!({ "error": "Entry not found" }), "{path}");
        }
    }
}

#[tokio::test]
async fn selections_stay_200_empty_after_group_delete_cascade() {
    // Route-level counterpart of the fixture
    // groups/get-mood-selections__after-group-delete.json: the entry still
    // EXISTS, only its selections were cascaded away, so the contract-change 404 must
    // NOT fire here.
    for test in make_apps().await {
        let (app, token) = (test.app.clone(), test.token.clone());
        seed_activities_group(&test.db).await;
        let (status, _) = send_json(
        &app,
        &token,
        "POST",
        "/api/mood",
        json!({ "mood": 4, "date": "2025-08-01", "content": "Cascade me.", "selected_options": [28] }),
    )
    .await;
        assert_eq!(status, StatusCode::CREATED);

        // Delete the Activities group (id 4) through the real route.
        let (status, _) = send_json(&app, &token, "DELETE", "/api/groups/4", json!(null)).await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = get_authed(&app, &token, "/api/mood/1/selections").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, json!([]));
    }
}

// ---------------------------------------------------------------------------
// contract change: POST /statistics/view records per-day views;
// GET /statistics is a pure read
// ---------------------------------------------------------------------------

#[tokio::test]
async fn statistics_view_endpoint_matches_fixtures() {
    for test in make_apps().await {
        let (app, token) = (test.app.clone(), test.token.clone());
        // First view of the day counts; the same-day repeat does not; the
        // strict-slash variant 404s before auth.
        check_fixture(&app, &token, "statistics_view_post_200_counted").await;
        check_fixture(&app, &token, "statistics_view_post_200_already_counted").await;
        check_fixture(&app, &token, "statistics_view_trailing_slash_404").await;
    }
}

#[tokio::test]
async fn statistics_get_is_pure_and_only_recorded_days_feed_data_lover() {
    for test in make_apps().await {
        let (app, token) = (test.app.clone(), test.token.clone());

        // Repeated GET /statistics must NOT advance data_lover (pre-rewrite each
        // GET incremented stats_views).
        for _ in 0..12 {
            let (status, _) = get_authed(&app, &token, "/api/statistics").await;
            assert_eq!(status, StatusCode::OK);
        }
        let (status, progress) = get_authed(&app, &token, "/api/achievements/progress").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            progress["data_lover"],
            json!({ "current": 0, "max": 10 }),
            "GET /statistics must be a pure read"
        );

        // 10 recorded views on 10 distinct days DO reach the threshold.
        {
            let start = chrono::Local::now().date_naive() - chrono::Days::new(30);
            match &test.db {
                support::TestDb::Sqlite { path, .. } => {
                    let conn = db::connect(path).expect("connect");
                    for offset in 0..10 {
                        let counted = nightlio_api::db::achievements::record_stats_view_on(
                            &conn,
                            1,
                            start + chrono::Days::new(offset),
                        )
                        .expect("record view");
                        assert!(counted, "each distinct day must count");
                    }
                }
                support::TestDb::Pg { .. } => {
                    let client = test.db.pg_client().await;
                    for offset in 0..10 {
                        let counted = nightlio_api::db::pg::achievements::record_stats_view_on(
                            &client,
                            1,
                            start + chrono::Days::new(offset),
                        )
                        .await
                        .expect("record view");
                        assert!(counted, "each distinct day must count");
                    }
                }
            }
        }
        let (status, progress) = get_authed(&app, &token, "/api/achievements/progress").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(progress["data_lover"], json!({ "current": 10, "max": 10 }));

        // The check endpoint now unlocks data_lover from those recorded days.
        let (status, body) =
            send_json(&app, &token, "POST", "/api/achievements/check", json!(null)).await;
        assert_eq!(status, StatusCode::OK);
        let unlocked: Vec<&str> = body["new_achievements"]
            .as_array()
            .expect("new_achievements array")
            .iter()
            .map(|item| item["achievement_type"].as_str().expect("achievement_type"))
            .collect();
        assert!(
            unlocked.contains(&"data_lover"),
            "10 recorded view days must unlock data_lover, got {unlocked:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Streak with entries today + yesterday
// ---------------------------------------------------------------------------

#[tokio::test]
async fn streak_counts_consecutive_days_matching_fixture() {
    for test in make_apps().await {
        let (app, token) = (test.app.clone(), test.token.clone());
        let today = chrono::Local::now().date_naive();
        let yesterday = today - chrono::Days::new(1);
        for (date, content) in [(today, "Today."), (yesterday, "Yesterday.")] {
            let (status, _) = send_json(
                &app,
                &token,
                "POST",
                "/api/mood",
                json!({
                    "mood": 4,
                    "date": date.format("%Y-%m-%d").to_string(),
                    "content": content,
                }),
            )
            .await;
            assert_eq!(status, StatusCode::CREATED);
        }
        check_fixture(&app, &token, "streak_populated").await;
    }
}

// ---------------------------------------------------------------------------
// contract change: US-format dates normalize to ISO on write
// ---------------------------------------------------------------------------

#[tokio::test]
async fn us_format_date_is_normalized_to_iso_on_write() {
    for test in make_apps().await {
        let (app, token) = (test.app.clone(), test.token.clone());

        // POST with a US-format date stores (and echoes back) the ISO form.
        let (status, body) = send_json(
            &app,
            &token,
            "POST",
            "/api/mood",
            json!({ "mood": 2, "date": "8/2/2025", "content": "US-format date." }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let entry_id = body["entry_id"].as_i64().expect("entry id");
        let (status, entry) = get_authed(&app, &token, &format!("/api/mood/{entry_id}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(entry["date"], json!("2025-08-02"));

        // PUT date fields normalize the same way.
        let (status, body) = send_json(
            &app,
            &token,
            "PUT",
            &format!("/api/mood/{entry_id}"),
            json!({ "date": "12/31/2024" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["entry"]["date"], json!("2024-12-31"));

        // ISO input still passes through verbatim.
        let (status, body) = send_json(
            &app,
            &token,
            "PUT",
            &format!("/api/mood/{entry_id}"),
            json!({ "date": "2025-08-05" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["entry"]["date"], json!("2025-08-05"));
    }
}

// ---------------------------------------------------------------------------
// The `time` field is written verbatim into created_at
// ---------------------------------------------------------------------------

#[tokio::test]
async fn time_field_is_written_verbatim_into_created_at() {
    for test in make_apps().await {
        let (app, token) = (test.app.clone(), test.token.clone());
        let (status, body) = send_json(
            &app,
            &token,
            "POST",
            "/api/mood",
            json!({
                "mood": 3,
                "date": "2025-08-01",
                "content": "Backdated.",
                "time": "2025-08-01 08:00:00",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let entry_id = body["entry_id"].as_i64().expect("entry id");

        let (status, entry) = get_authed(&app, &token, &format!("/api/mood/{entry_id}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(entry["created_at"], json!("2025-08-01 08:00:00"));

        // PUT `time` overwrites created_at verbatim too.
        let (status, body) = send_json(
            &app,
            &token,
            "PUT",
            &format!("/api/mood/{entry_id}"),
            json!({ "time": "2025-08-01 09:30:00" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["entry"]["created_at"], json!("2025-08-01 09:30:00"));
    }
}

// ---------------------------------------------------------------------------
// Contract encoding facts (DECISIONS.md "Contract encoding facts")
// ---------------------------------------------------------------------------

#[tokio::test]
async fn non_ascii_content_is_raw_utf8_not_escaped() {
    // JSON bodies carry non-ASCII as raw UTF-8, never \uXXXX escapes.
    for test in make_apps().await {
        let (status, _) = send_json(
            &test.app,
            &test.token,
            "POST",
            "/api/mood",
            json!({ "mood": 4, "date": "2026-08-16", "content": "café 🌙" }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);

        let request = Request::builder()
            .uri("/api/moods")
            .header(header::AUTHORIZATION, format!("Bearer {}", test.token))
            .body(Body::empty())
            .expect("request");
        let response = send(&test.app, request).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("body");
        let raw = String::from_utf8(bytes.to_vec()).expect("utf8");
        assert!(raw.contains("café 🌙"), "raw UTF-8 expected in {raw}");
        assert!(!raw.contains("\\u00e9"), "\\uXXXX escaping found in {raw}");
    }
}
