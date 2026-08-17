//! Groups route family vs the golden fixtures in
//! `contract/fixtures/groups/`: every scenario replays the recorded request
//! against the full assembled router (state + middleware) on a
//! bootstrapped tempfile DB and asserts status + content type + body, then
//! pins the `{status, body}` pair as an insta snapshot.
//!
//! Fixtures NOT replayed here (owned by the mood family, recorded in the
//! groups directory only for the selections interplay):
//! `get-mood-selections__*.json`, `post-mood__with-selected-options.json`,
//! `routing__get-selections-negative-id.json`. The cascade interplay behind
//! `get-mood-selections__after-group-delete.json` is covered at the DB
//! level by [`delete_group_cascades_to_entry_selections`].

use std::collections::HashMap;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::Response;
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
    pool: db::DbPool,
    /// Bearer token for the bootstrapped self-host user (id 1).
    token: String,
    _dir: tempfile::TempDir,
}

/// Fresh bootstrapped state: self-host user id 1, default groups 1-3
/// (Emotions, Sleep, Productivity), options 1-27 — the exact baseline the
/// fixtures were recorded against.
fn make_app() -> TestApp {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("nightlio.db");
    let vars: HashMap<String, String> =
        HashMap::from([("APP_ENV".to_string(), "development".to_string())]);
    let lookup = move |key: &str| vars.get(key).cloned();
    let mut cfg = Config::from_lookup(&lookup);
    cfg.database_path = db_path.to_string_lossy().into_owned();
    db::bootstrap(&cfg.database_path, &SelfHostSeed::from(&cfg)).expect("bootstrap");
    let pool = db::open_pool(&cfg.database_path).expect("pool");
    let token = jwt::issue_token(&cfg.jwt_secret, 1).expect("token");
    let state = AppState::new(cfg, pool.clone());
    TestApp {
        app: routes::build_router(state),
        pool,
        token,
        _dir: dir,
    }
}

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/../contract/fixtures/groups/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|exc| panic!("missing fixture {path}: {exc}"));
    serde_json::from_str(&raw).expect("fixture json")
}

async fn body_json(response: Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("json body")
}

/// Rebuild the recorded request: method + path verbatim; `Authorization`
/// replaced with a real JWT for the seeded user when the fixture carried
/// one; `Content-Type` and the JSON body verbatim.
async fn replay(app: &TestApp, fix: &Value) -> Response {
    let method = fix["method"].as_str().expect("method");
    let path = fix["path"].as_str().expect("path");
    let mut builder = Request::builder().method(method).uri(path);
    let headers = &fix["request"]["headers_that_matter"];
    if headers.get("Authorization").is_some() {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {}", app.token));
    }
    if let Some(content_type) = headers.get("Content-Type").and_then(Value::as_str) {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    let body = match &fix["request"]["body"] {
        Value::Null => Body::empty(),
        other => Body::from(serde_json::to_vec(other).expect("body json")),
    };
    app.app
        .clone()
        .oneshot(builder.body(body).expect("request"))
        .await
        .expect("infallible")
}

/// Replay a fixture, assert full parity (status, JSON content type, body),
/// and pin the result as an insta snapshot named after the fixture.
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
    let body = body_json(response).await;

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

    let snapshot_name = name.trim_end_matches(".json").replace(['-', '.'], "_");
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

// ---------------------------------------------------------------------------
// GET /api/groups
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_groups_fresh_account_defaults() {
    let app = make_app();
    assert_fixture_parity(&app, "get-groups__fresh-account-defaults.json").await;
}

#[tokio::test]
async fn get_groups_happy() {
    let app = make_app();
    // Recorded state: groups 4 (Activities) and 5 (Weather) with no
    // options; options 28 (Exercise) and 29 (Reading) under Emotions (1).
    setup_request(&app, "POST", "/api/groups", json!({"name": "Activities"})).await;
    setup_request(&app, "POST", "/api/groups", json!({"name": "Weather"})).await;
    setup_request(
        &app,
        "POST",
        "/api/groups/1/options",
        json!({"name": "Exercise"}),
    )
    .await;
    setup_request(
        &app,
        "POST",
        "/api/groups/1/options",
        json!({"name": "Reading"}),
    )
    .await;
    assert_fixture_parity(&app, "get-groups__happy.json").await;
}

#[tokio::test]
async fn get_groups_empty_after_deleting_the_seeded_defaults() {
    let app = make_app();
    for group_id in 1..=3 {
        setup_request(
            &app,
            "DELETE",
            &format!("/api/groups/{group_id}"),
            Value::Null,
        )
        .await;
    }
    assert_fixture_parity(&app, "get-groups__empty.json").await;
}

#[tokio::test]
async fn get_groups_401_no_auth() {
    let app = make_app();
    assert_fixture_parity(&app, "get-groups__401-no-auth.json").await;
}

// ---------------------------------------------------------------------------
// POST /api/groups
// ---------------------------------------------------------------------------

#[tokio::test]
async fn post_groups_happy() {
    let app = make_app();
    assert_fixture_parity(&app, "post-groups__happy.json").await;
}

#[tokio::test]
async fn post_groups_400_empty_name() {
    let app = make_app();
    assert_fixture_parity(&app, "post-groups__400-empty-name.json").await;
}

#[tokio::test]
async fn post_groups_400_whitespace_name() {
    // Whitespace passes the route-layer falsy check and fails the service
    // strip check: a DIFFERENT 400 string than the empty-name case.
    let app = make_app();
    assert_fixture_parity(&app, "post-groups__400-whitespace-name.json").await;
}

#[tokio::test]
async fn post_groups_401_no_auth() {
    let app = make_app();
    assert_fixture_parity(&app, "post-groups__401-no-auth.json").await;
}

// ---------------------------------------------------------------------------
// POST /api/groups/{id}/options
// ---------------------------------------------------------------------------

#[tokio::test]
async fn post_group_options_happy() {
    let app = make_app();
    assert_fixture_parity(&app, "post-group-options__happy.json").await;
}

#[tokio::test]
async fn post_group_options_400_empty_name() {
    let app = make_app();
    assert_fixture_parity(&app, "post-group-options__400-empty-name.json").await;
}

#[tokio::test]
async fn post_group_options_404_unknown_group() {
    // contract change (owner-approved, supersedes DECISIONS.md item
    // 6): an unknown group id is now a 404 "Group not found" (the same
    // resource-not-found convention as DELETE /groups/{id}) instead of
    // Flask's recorded 400 "Group not found for user".
    let app = make_app();
    assert_fixture_parity(&app, "post-group-options__404-unknown-group.json").await;
}

#[tokio::test]
async fn post_group_options_404_foreign_group() {
    // contract change: a group owned by ANOTHER user 404s exactly
    // like a nonexistent one.
    let app = make_app();
    {
        let conn = app.pool.get().expect("conn");
        conn.execute(
            "INSERT INTO users (google_id, email, name) VALUES ('other-user', 'other@example.com', 'Other')",
            [],
        )
        .unwrap();
        // Defaults occupy group ids 1..=3, so the foreign group gets id 4.
        conn.execute(
            "INSERT INTO groups (user_id, name) VALUES (2, 'Theirs')",
            [],
        )
        .unwrap();
    }
    let response = app
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/groups/4/options")
                .header(header::AUTHORIZATION, format!("Bearer {}", app.token))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({"name": "X"})).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .expect("infallible");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        body_json(response).await,
        json!({ "error": "Group not found" })
    );
}

#[tokio::test]
async fn post_group_options_401_no_auth() {
    let app = make_app();
    assert_fixture_parity(&app, "post-group-options__401-no-auth.json").await;
}

// ---------------------------------------------------------------------------
// DELETE /api/groups/{id}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_group_happy() {
    let app = make_app();
    // The fixture deletes group 5: create groups 4 and 5 first.
    setup_request(&app, "POST", "/api/groups", json!({"name": "Activities"})).await;
    setup_request(&app, "POST", "/api/groups", json!({"name": "Weather"})).await;
    assert_fixture_parity(&app, "delete-group__happy.json").await;
}

#[tokio::test]
async fn delete_group_404_not_found() {
    let app = make_app();
    assert_fixture_parity(&app, "delete-group__404-not-found.json").await;
}

#[tokio::test]
async fn delete_group_401_no_auth() {
    let app = make_app();
    assert_fixture_parity(&app, "delete-group__401-no-auth.json").await;
}

/// Interplay behind `get-mood-selections__after-group-delete.json` (the
/// selections ROUTE belongs to the mood family): deleting a group through
/// the real route cascades `group_options` → `entry_selections`.
#[tokio::test]
async fn delete_group_cascades_to_entry_selections() {
    let app = make_app();
    {
        let conn = app.pool.get().expect("conn");
        conn.execute(
            "INSERT INTO mood_entries (user_id, date, mood, content) VALUES (1, '2026-08-15', 4, 'x')",
            [],
        )
        .unwrap();
        // Options 1 (happy) and 2 (excited) belong to group 1 (Emotions).
        conn.execute(
            "INSERT INTO entry_selections (entry_id, option_id) VALUES (1, 1), (1, 2)",
            [],
        )
        .unwrap();
    }
    setup_request(&app, "DELETE", "/api/groups/1", Value::Null).await;
    let conn = app.pool.get().expect("conn");
    let selections: i64 = conn
        .query_row("SELECT COUNT(*) FROM entry_selections", [], |row| {
            row.get(0)
        })
        .unwrap();
    let options: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM group_options WHERE group_id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        (selections, options),
        (0, 0),
        "cascade must prune both tables"
    );
}

// ---------------------------------------------------------------------------
// DELETE /api/options/{id}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_option_happy() {
    let app = make_app();
    assert_fixture_parity(&app, "delete-option__happy.json").await;
}

#[tokio::test]
async fn delete_option_404_not_found() {
    let app = make_app();
    assert_fixture_parity(&app, "delete-option__404-not-found.json").await;
}

#[tokio::test]
async fn delete_option_401_no_auth() {
    let app = make_app();
    assert_fixture_parity(&app, "delete-option__401-no-auth.json").await;
}

// ---------------------------------------------------------------------------
// Routing semantics
// ---------------------------------------------------------------------------

#[tokio::test]
async fn routing_bad_ids_and_trailing_slashes_are_json_404() {
    let app = make_app();
    for name in [
        "routing__delete-group-negative-id.json",
        "routing__delete-group-noninteger-id.json",
        "routing__delete-group-trailing-slash.json",
        "routing__delete-option-negative-id.json",
        "routing__get-groups-trailing-slash.json",
        "routing__post-options-trailing-slash.json",
    ] {
        assert_fixture_parity(&app, name).await;
    }
}

#[tokio::test]
async fn routing_overflow_id_is_clean_404_documented_drift() {
    // contract/DECISIONS.md item 1: the recorded Flask response is a 500
    // leaking "Python int too large to convert to SQLite INTEGER"; the
    // Rust port returns the clean JSON 404 instead. Assert the Rust
    // expectation, not the recorded fixture body.
    let app = make_app();
    let fix = fixture("routing__delete-group-overflow-id.json");
    let response = replay(&app, &fix).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body = body_json(response).await;
    assert_eq!(body, json!({ "error": "Resource not found" }));
    insta::assert_json_snapshot!(
        "routing__delete_group_overflow_id_rust_drift",
        json!({ "status": 404, "body": body })
    );
}

/// Routing 404s fire BEFORE auth, exactly like Flask (URL matching happens
/// before the `require_auth` decorator): a bad id with NO credentials is
/// still 404, never 401.
#[tokio::test]
async fn routing_404_beats_missing_auth() {
    let app = make_app();
    for path in ["/api/groups/abc", "/api/groups/-1", "/api/options/-1"] {
        let response = app
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("infallible");
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert_eq!(
            body_json(response).await,
            json!({ "error": "Resource not found" }),
            "{path}"
        );
    }
}

/// No groups rule registers OPTIONS explicitly, so all of them carry the
/// automatic OPTIONS response: 204, empty body, `Allow` header (contract
/// change).
#[tokio::test]
async fn automatic_options_on_every_groups_rule() {
    let app = make_app();
    for (path, allow) in [
        ("/api/groups", "HEAD, GET, OPTIONS, POST"),
        ("/api/groups/1", "OPTIONS, DELETE"),
        ("/api/groups/1/options", "POST, OPTIONS"),
        ("/api/options/1", "OPTIONS, DELETE"),
    ] {
        let response = app
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("infallible");
        assert_eq!(response.status(), StatusCode::NO_CONTENT, "{path}");
        assert_eq!(
            response
                .headers()
                .get(header::ALLOW)
                .and_then(|value| value.to_str().ok()),
            Some(allow),
            "{path}"
        );
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("body");
        assert!(bytes.is_empty(), "{path}: automatic OPTIONS body is empty");
    }
}
