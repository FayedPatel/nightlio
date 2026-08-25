//! Regression test for the v0.6.0 PG default-seed parity fix: a fresh
//! PostgreSQL self-host deploy, booted through the PRODUCTION startup path
//! (`db::connect_from_config` — the exact sequence `serve()` runs), must
//! answer `GET /api/groups` with the same three default groups (and 27
//! options) a fresh SQLite deploy gets from `db::bootstrap`. Before the
//! fix the Postgres branch seeded nothing and this endpoint answered `[]`.
//!
//! Deliberately NO `seed_bootstrap_equivalent` hand-seed here — the whole
//! point is that the server's own startup produces the baseline. Also
//! covers idempotency (a second startup changes nothing, and never
//! refreshes an existing user row) and restart-time backfill (a deploy
//! that lost/never had its default groups gets them back on next boot).
//!
//! Gated on `NIGHTLIO_PG_TEST_URL` (clean skip when unset), like every
//! other PG-tier test.

use std::collections::HashMap;

mod support;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, header};
use serde_json::Value;
use tower::ServiceExt;

use nightlio_api::auth::jwt;
use nightlio_api::config::Config;
use nightlio_api::db;
use nightlio_api::routes;
use nightlio_api::state::AppState;

fn dev_config(dir: &tempfile::TempDir, database_url: &str) -> Config {
    let vars: HashMap<String, String> =
        HashMap::from([("APP_ENV".to_string(), "development".to_string())]);
    let lookup = move |key: &str| vars.get(key).cloned();
    let mut cfg = Config::from_lookup(&lookup);
    cfg.database_path = dir
        .path()
        .join("nightlio.db")
        .to_string_lossy()
        .into_owned();
    cfg.database_url = Some(database_url.to_string());
    cfg
}

/// Boot the app exactly as production does and fetch `GET /api/groups` for
/// the self-host user (id 1).
async fn boot_and_fetch_groups(cfg: &Config) -> Value {
    let db = db::connect_from_config(cfg)
        .await
        .expect("production startup against the PG database");
    let token = jwt::issue_token(&cfg.jwt_secret, 1).expect("token");
    let app: Router = routes::build_router(AppState::new(cfg.clone(), db));
    let request = Request::builder()
        .uri("/api/groups")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .expect("request");
    let response = app.oneshot(request).await.expect("infallible");
    assert_eq!(response.status().as_u16(), 200, "GET /api/groups");
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).expect("json body")
}

/// `[(group name, group id, option count)]` in response order.
fn group_shape(groups: &Value) -> Vec<(String, i64, usize)> {
    groups
        .as_array()
        .expect("groups array")
        .iter()
        .map(|group| {
            (
                group["name"].as_str().expect("name").to_string(),
                group["id"].as_i64().expect("id"),
                group["options"].as_array().expect("options").len(),
            )
        })
        .collect()
}

#[tokio::test]
async fn fresh_pg_startup_seeds_the_selfhost_baseline_idempotently_and_backfills() {
    if support::pg_test_url().is_none() {
        eprintln!("skipping PG bootstrap-seed regression: NIGHTLIO_PG_TEST_URL not set");
        return;
    }

    let url = support::create_empty_pg_db("baseline").await;
    let dir = tempfile::tempdir().expect("tempdir");
    let cfg = dev_config(&dir, &url);

    // --- Fresh deploy: exactly the SQLite-bootstrap baseline ---------------
    let groups = boot_and_fetch_groups(&cfg).await;
    // Response order is by name under COLLATE "C"; ids reflect insert order
    // (Emotions, Sleep, Productivity), matching a fresh SQLite file.
    assert_eq!(
        group_shape(&groups),
        vec![
            ("Emotions".to_string(), 1, 13),
            ("Productivity".to_string(), 3, 8),
            ("Sleep".to_string(), 2, 6),
        ],
        "default groups must match the SQLite bootstrap (was [] before the fix)"
    );
    let mut option_ids: Vec<i64> = groups
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|group| group["options"].as_array().unwrap())
        .map(|option| option["id"].as_i64().unwrap())
        .collect();
    option_ids.sort_unstable();
    assert_eq!(
        option_ids,
        (1..=27).collect::<Vec<i64>>(),
        "default options must occupy ids 1..=27, like SQLite"
    );

    // --- Idempotency: a second startup changes nothing ---------------------
    // Plant sentinels on the user row first — the baseline must never
    // refresh an existing row's email/last_login (login upserts own that).
    let client = support::pg_connect(&url).await;
    client
        .execute(
            "UPDATE users SET email = 'keep@example.com', \
             last_login = '2020-01-01 00:00:00' WHERE id = 1",
            &[],
        )
        .await
        .expect("plant sentinels");
    let groups_again = boot_and_fetch_groups(&cfg).await;
    assert_eq!(
        groups_again, groups,
        "second startup must serve an identical group list"
    );
    let row = client
        .query_one("SELECT email, last_login FROM users WHERE id = 1", &[])
        .await
        .expect("read the user row");
    assert_eq!(row.get::<_, String>(0), "keep@example.com");
    assert_eq!(
        row.get::<_, Option<String>>(1).as_deref(),
        Some("2020-01-01 00:00:00"),
        "baseline must never refresh an existing user's last_login"
    );
    let users: i64 = client
        .query_one("SELECT COUNT(*) FROM users", &[])
        .await
        .expect("count users")
        .get(0);
    assert_eq!(users, 1, "no duplicate self-host user");

    // --- Backfill: a deploy that lost its groups gets them back ------------
    client
        .execute("DELETE FROM group_options", &[])
        .await
        .expect("wipe options");
    client
        .execute("DELETE FROM groups", &[])
        .await
        .expect("wipe groups");
    let restored = boot_and_fetch_groups(&cfg).await;
    // Identity sequences advanced past the deleted rows, so only names and
    // option counts are pinned here — not ids.
    let shape: Vec<(String, usize)> = group_shape(&restored)
        .into_iter()
        .map(|(name, _, options)| (name, options))
        .collect();
    assert_eq!(
        shape,
        vec![
            ("Emotions".to_string(), 13),
            ("Productivity".to_string(), 8),
            ("Sleep".to_string(), 6),
        ],
        "restart must backfill the default groups"
    );
}
