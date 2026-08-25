//! Achievements routes — port of `api/routes/achievement_routes.py` plus
//! the service-layer logic from `api/services/achievement_service.py`.
//!
//! Contract (golden fixtures in `contract/fixtures/achievements/`):
//! - `GET /achievements`: bare array of DB rows merged with static
//!   metadata (`nft_minted` integer 0/1, null nft columns), ordered by
//!   `earned_at DESC`. Trailing-slash variant 404s (strict slashes).
//! - `POST /achievements/check`: ignores the request body entirely,
//!   idempotent (UNIQUE constraint), returns metadata objects WITHOUT
//!   `id`/`earned_at` plus `count`. Trailing-slash variant 404s.
//! - `GET /achievements/progress` and `/achievements/progress/`: the ONE
//!   achievements rule family registered with `strict_slashes=False` —
//!   both variants are explicit rules with byte-identical bodies.
//! - `OPTIONS` on every achievements rule (both progress slash variants
//!   included) is the app-wide automatic OPTIONS: unauthenticated,
//!   204-empty with an `Allow` header (contract change — the old
//!   split between the dedicated 204 rule and Flask's automatic 200 is
//!   gone).
//!
//! The backend metadata table is wire truth (`contract/DECISIONS.md`,
//! "Achievement icon swap"): `consistency_king` → `Target`,
//! `mood_master` → `Crown` — the frontend's opposite hardcoding is a
//! pre-existing UI bug, not to be "fixed" here.

use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};

use super::automatic_options;
use crate::auth::extract::AuthUser;
use crate::db::achievements::{AchievementRow, AchievementsProgress};
use crate::db::store;
use crate::error::ApiResult;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/achievements",
            get(get_user_achievements).merge(automatic_options("HEAD, GET, OPTIONS")),
        )
        .route(
            "/achievements/check",
            post(check_achievements).merge(automatic_options("POST, OPTIONS")),
        )
        // strict_slashes=False family: both slash variants are explicit
        // rules, each with the app-wide automatic OPTIONS (contract change).
        .route(
            "/achievements/progress",
            get(achievements_progress).merge(automatic_options("HEAD, GET, OPTIONS")),
        )
        .route(
            "/achievements/progress/",
            get(achievements_progress).merge(automatic_options("HEAD, GET, OPTIONS")),
        )
}

// ---------------------------------------------------------------------------
// Achievement metadata (api/services/achievement_service.py)
// ---------------------------------------------------------------------------

/// Static achievement metadata — the `self.achievements` dict of
/// `AchievementService`, verbatim.
struct AchievementMeta {
    name: &'static str,
    description: &'static str,
    icon: &'static str,
    rarity: &'static str,
}

/// Port of `AchievementService.get_achievement_info`: `None` for unknown
/// types (Python returns `{}`, so callers merge nothing).
fn achievement_meta(achievement_type: &str) -> Option<AchievementMeta> {
    match achievement_type {
        "first_entry" => Some(AchievementMeta {
            name: "First Entry",
            description: "Log your first mood entry",
            icon: "Zap",
            rarity: "common",
        }),
        "week_warrior" => Some(AchievementMeta {
            name: "Week Warrior",
            description: "Maintain a 7-day streak",
            icon: "Flame",
            rarity: "uncommon",
        }),
        "consistency_king" => Some(AchievementMeta {
            name: "Consistency King",
            description: "Maintain a 30-day streak",
            icon: "Target",
            rarity: "rare",
        }),
        "data_lover" => Some(AchievementMeta {
            name: "Data Lover",
            description: "View statistics on 10 different days",
            icon: "BarChart3",
            rarity: "uncommon",
        }),
        "mood_master" => Some(AchievementMeta {
            name: "Mood Master",
            description: "Log 100 total entries",
            icon: "Crown",
            rarity: "legendary",
        }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /achievements` — port of `get_user_achievements` (route) +
/// `AchievementService.get_user_achievements`: bare array of DB rows, each
/// merged (`dict.update`) with the static metadata for its type.
async fn get_user_achievements(
    State(state): State<AppState>,
    user: AuthUser,
) -> ApiResult<Json<Vec<Value>>> {
    let user_id = user.user_id;
    let rows = store::achievements::get_user_achievements(&state.db, user_id).await?;
    let achievements = rows
        .into_iter()
        .map(merge_row_metadata)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(achievements))
}

/// One `GET /achievements` array item: the serialized DB row with the
/// metadata keys merged in (unknown types merge nothing, like Python's
/// `dict.update({})`).
fn merge_row_metadata(row: AchievementRow) -> Result<Value, serde_json::Error> {
    let meta = achievement_meta(&row.achievement_type);
    let mut value = serde_json::to_value(&row)?;
    if let (Value::Object(map), Some(meta)) = (&mut value, meta) {
        map.insert("name".to_string(), json!(meta.name));
        map.insert("description".to_string(), json!(meta.description));
        map.insert("icon".to_string(), json!(meta.icon));
        map.insert("rarity".to_string(), json!(meta.rarity));
    }
    Ok(value)
}

/// `POST /achievements/check` — port of `check_achievements` (route) +
/// `AchievementService.check_and_award_achievements`. The request body is
/// ignored entirely (no body extractor). Awarding is idempotent via the
/// UNIQUE(user_id, achievement_type) constraint; each newly-awarded type
/// gets a best-effort `achievement_unlocked` activity write that must never
/// break the award itself.
async fn check_achievements(
    State(state): State<AppState>,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    let user_id = user.user_id;
    let new_types = store::achievements::check_achievements(&state.db, user_id).await?;

    let new_achievements = new_achievement_objects(&new_types);

    Ok(Json(json!({
        "new_achievements": new_achievements,
        "count": new_achievements.len(),
    })))
}

/// Unified `new_achievements` item builder (contract change): the
/// metadata-object shape shared by `POST /achievements/check` and
/// `POST /api/mood`. Objects carry no `id`/`earned_at`; `achievement_type`
/// is always set, even for unknown types (Python mutates the `{}` fallback
/// too).
pub(crate) fn new_achievement_objects(types: &[String]) -> Vec<serde_json::Value> {
    types
        .iter()
        .map(|achievement_type| {
            let mut object = match achievement_meta(achievement_type) {
                Some(meta) => json!({
                    "name": meta.name,
                    "description": meta.description,
                    "icon": meta.icon,
                    "rarity": meta.rarity,
                }),
                None => json!({}),
            };
            object["achievement_type"] = json!(achievement_type);
            object
        })
        .collect()
}

/// `GET /achievements/progress[/]` — port of `achievements_progress`:
/// straight passthrough of `get_achievements_progress` (five fixed keys,
/// `current` clamped to `[0, max]`).
async fn achievements_progress(
    State(state): State<AppState>,
    user: AuthUser,
) -> ApiResult<Json<AchievementsProgress>> {
    let user_id = user.user_id;
    let progress = store::achievements::get_achievements_progress(&state.db, user_id).await?;
    Ok(Json(progress))
}

// ---------------------------------------------------------------------------
// Tests — insta snapshots vs contract/fixtures/achievements/
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use axum::response::Response;
    use serde_json::{Value, json};
    use tower::ServiceExt;

    use crate::auth::jwt;
    use crate::config::Config;
    use crate::db::{self, DbHandle, SelfHostSeed, SqlitePool};
    use crate::state::AppState;

    // --- Harness -------------------------------------------------------------

    struct TestApp {
        app: Router,
        pool: SqlitePool,
        token: String,
        _dir: tempfile::TempDir,
    }

    /// Full router + middleware on a fresh tempfile DB. The bootstrap seeds
    /// the default self-host user (id 1); `token` authenticates as it.
    fn test_app() -> TestApp {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("nightlio.db");
        let lookup = |key: &str| match key {
            "APP_ENV" => Some("development".to_string()),
            _ => None,
        };
        let mut cfg = Config::from_lookup(&lookup);
        cfg.database_path = db_path.to_string_lossy().into_owned();
        db::bootstrap(&cfg.database_path, &SelfHostSeed::from(&cfg)).expect("bootstrap");
        let pool = db::open_pool(&cfg.database_path).expect("pool");
        let token = jwt::issue_token(&cfg.jwt_secret, 1).expect("token");
        let state = AppState::new(cfg, DbHandle::Sqlite(pool.clone()));
        TestApp {
            app: crate::routes::build_router(state),
            pool,
            token,
            _dir: dir,
        }
    }

    async fn send(app: &TestApp, method: &str, path: &str, token: Option<&str>) -> Response {
        let mut builder = Request::builder().method(method).uri(path);
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        app.app
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .expect("infallible")
    }

    async fn body_string(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("body");
        String::from_utf8(bytes.to_vec()).expect("utf8 body")
    }

    async fn body_json(response: Response) -> Value {
        serde_json::from_str(&body_string(response).await).expect("json body")
    }

    fn fixture(name: &str) -> Value {
        let path = format!(
            "{}/../contract/fixtures/achievements/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|exc| panic!("missing fixture {path}: {exc}"));
        serde_json::from_str(&raw).expect("fixture json")
    }

    /// Assert status + JSON body against the fixture (ids/timestamps must
    /// already be normalized by the caller), then insta-snapshot the
    /// normalized `{status, body}` pair under the fixture's name.
    fn assert_fixture_and_snapshot(name: &str, status: StatusCode, body: Value) {
        let fx = fixture(name);
        assert_eq!(
            u64::from(status.as_u16()),
            fx["response"]["status"].as_u64().expect("fixture status"),
            "{name}: status"
        );
        assert_eq!(body, fx["response"]["body"], "{name}: body");
        insta::assert_json_snapshot!(name, json!({ "status": status.as_u16(), "body": body }));
    }

    fn assert_json_content_type(response: &Response) {
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        assert!(
            content_type.starts_with("application/json"),
            "expected JSON content type, got {content_type:?}"
        );
    }

    // --- Seed helpers --------------------------------------------------------

    fn seed_achievement(app: &TestApp, achievement_type: &str, earned_at: &str) {
        let conn = app.pool.get().unwrap();
        conn.execute(
            "INSERT INTO achievements (user_id, achievement_type, earned_at) VALUES (1, ?, ?)",
            [achievement_type, earned_at],
        )
        .unwrap();
    }

    fn seed_mood_entry(app: &TestApp, date: &str) {
        let conn = app.pool.get().unwrap();
        conn.execute(
            "INSERT INTO mood_entries (user_id, date, mood, content) VALUES (1, ?, 3, 'seed')",
            [date],
        )
        .unwrap();
    }

    /// Seed `views` statistics views on `views` DISTINCT calendar days
    /// (today going backwards) — the per-day semantics of the
    /// `record_stats_view_on` counter.
    fn seed_stats_views(app: &TestApp, views: usize) {
        let conn = app.pool.get().unwrap();
        let today = chrono::Local::now().date_naive();
        for offset in 0..views {
            let day = today - chrono::Days::new(offset as u64);
            assert!(db::achievements::record_stats_view_on(&conn, 1, day).unwrap());
        }
    }

    /// 7 consecutive daily ISO entries ending today → streak 7, 7 entries.
    fn seed_seven_day_streak(app: &TestApp) {
        let today = chrono::Local::now().date_naive();
        for offset in 0..7 {
            let date = today - chrono::Days::new(offset);
            seed_mood_entry(app, &date.format("%Y-%m-%d").to_string());
        }
    }

    // --- GET /achievements ---------------------------------------------------

    #[tokio::test]
    async fn get_achievements_empty() {
        let app = test_app();
        let response = send(&app, "GET", "/api/achievements", Some(&app.token)).await;
        let status = response.status();
        assert_json_content_type(&response);
        let body = body_json(response).await;
        assert_fixture_and_snapshot("get_achievements_empty", status, body);
    }

    #[tokio::test]
    async fn get_achievements_unlocked() {
        let app = test_app();
        // Insert order fixes the autoincrement ids (1, 2, 3); earned_at
        // values fix the ORDER BY earned_at DESC output order recorded in
        // the fixture: data_lover(3), first_entry(1), week_warrior(2).
        seed_achievement(&app, "first_entry", "2026-08-13 10:00:00");
        seed_achievement(&app, "week_warrior", "2026-08-12 10:00:00");
        seed_achievement(&app, "data_lover", "2026-08-14 10:00:00");

        let response = send(&app, "GET", "/api/achievements", Some(&app.token)).await;
        let status = response.status();
        assert_json_content_type(&response);
        let mut body = body_json(response).await;
        // Normalize timestamps exactly like the recorded fixture.
        for item in body.as_array_mut().expect("bare array") {
            assert!(item["earned_at"].is_string(), "earned_at present");
            item["earned_at"] = json!("<TS>");
        }
        assert_fixture_and_snapshot("get_achievements_unlocked", status, body);
    }

    #[tokio::test]
    async fn get_achievements_unauth() {
        let app = test_app();
        let response = send(&app, "GET", "/api/achievements", None).await;
        let status = response.status();
        assert_json_content_type(&response);
        let body = body_json(response).await;
        assert_fixture_and_snapshot("get_achievements_unauth", status, body);
    }

    #[tokio::test]
    async fn get_achievements_invalid_token() {
        let app = test_app();
        let response = send(&app, "GET", "/api/achievements", Some("not-a-jwt")).await;
        let status = response.status();
        assert_json_content_type(&response);
        let body = body_json(response).await;
        assert_fixture_and_snapshot("get_achievements_invalid_token", status, body);
    }

    #[tokio::test]
    async fn get_achievements_trailing_slash_404() {
        // Rule registered WITHOUT a trailing slash → strict-slash JSON 404.
        let app = test_app();
        let response = send(&app, "GET", "/api/achievements/", Some(&app.token)).await;
        let status = response.status();
        assert_json_content_type(&response);
        let body = body_json(response).await;
        assert_fixture_and_snapshot("get_achievements_trailing_slash_404", status, body);
    }

    // --- POST /achievements/check --------------------------------------------

    #[tokio::test]
    async fn post_achievements_check_empty_then_unlocks_then_nothing_new() {
        let app = test_app();

        // Empty account: no condition met.
        let response = send(&app, "POST", "/api/achievements/check", Some(&app.token)).await;
        let status = response.status();
        assert_json_content_type(&response);
        let body = body_json(response).await;
        assert_fixture_and_snapshot("post_achievements_check_empty", status, body);

        // data_lover condition met (10 stats views) but not yet awarded.
        seed_stats_views(&app, 10);
        let response = send(&app, "POST", "/api/achievements/check", Some(&app.token)).await;
        let status = response.status();
        let body = body_json(response).await;
        assert_fixture_and_snapshot("post_achievements_check_unlocks", status, body);

        // Idempotent: immediately after, every met condition is already
        // awarded. The body is ignored entirely — send garbage with no
        // Content-Type to prove it.
        let request = Request::builder()
            .method("POST")
            .uri("/api/achievements/check")
            .header(header::AUTHORIZATION, format!("Bearer {}", app.token))
            .body(Body::from("this is not json"))
            .unwrap();
        let response = app.app.clone().oneshot(request).await.expect("infallible");
        let status = response.status();
        let body = body_json(response).await;
        assert_fixture_and_snapshot("post_achievements_check_nothing_new", status, body);

        // The award wrote exactly one best-effort activity row.
        let conn = app.pool.get().unwrap();
        let activity = db::activity::get_activity(&conn, 1, None, 50).unwrap();
        let unlocked: Vec<_> = activity
            .iter()
            .filter(|row| row.event_type == "achievement_unlocked")
            .collect();
        assert_eq!(unlocked.len(), 1);
        assert_eq!(
            unlocked[0].metadata,
            Some(json!({ "achievement_type": "data_lover" }))
        );
    }

    #[tokio::test]
    async fn post_achievements_check_unauth() {
        let app = test_app();
        let response = send(&app, "POST", "/api/achievements/check", None).await;
        let status = response.status();
        assert_json_content_type(&response);
        let body = body_json(response).await;
        assert_fixture_and_snapshot("post_achievements_check_unauth", status, body);
    }

    #[tokio::test]
    async fn post_achievements_check_trailing_slash_404() {
        let app = test_app();
        let response = send(&app, "POST", "/api/achievements/check/", Some(&app.token)).await;
        let status = response.status();
        assert_json_content_type(&response);
        let body = body_json(response).await;
        assert_fixture_and_snapshot("post_achievements_check_trailing_slash_404", status, body);
    }

    // --- GET /achievements/progress (both slash variants) ---------------------

    #[tokio::test]
    async fn get_achievements_progress_empty() {
        let app = test_app();
        let response = send(&app, "GET", "/api/achievements/progress", Some(&app.token)).await;
        let status = response.status();
        assert_json_content_type(&response);
        let body = body_json(response).await;
        assert_fixture_and_snapshot("get_achievements_progress_empty", status, body);
    }

    #[tokio::test]
    async fn get_achievements_progress_seeded_both_variants_byte_identical() {
        let app = test_app();
        // Fixture scenario: 7 entries, 7-day streak, 10 stats views.
        seed_seven_day_streak(&app);
        seed_stats_views(&app, 10);

        let response = send(&app, "GET", "/api/achievements/progress", Some(&app.token)).await;
        let status = response.status();
        assert_json_content_type(&response);
        let raw = body_string(response).await;
        let body: Value = serde_json::from_str(&raw).expect("json body");
        assert_fixture_and_snapshot("get_achievements_progress_seeded", status, body);

        // Trailing-slash alias: a distinct registered rule with a
        // byte-identical body.
        let response = send(&app, "GET", "/api/achievements/progress/", Some(&app.token)).await;
        let status = response.status();
        assert_json_content_type(&response);
        let raw_slash = body_string(response).await;
        assert_eq!(raw, raw_slash, "slash variants must be byte-identical");
        let body: Value = serde_json::from_str(&raw_slash).expect("json body");
        assert_fixture_and_snapshot("get_achievements_progress_slash_seeded", status, body);
    }

    #[tokio::test]
    async fn get_achievements_progress_unauth_both_variants() {
        let app = test_app();
        for (path, name) in [
            (
                "/api/achievements/progress",
                "get_achievements_progress_unauth",
            ),
            (
                "/api/achievements/progress/",
                "get_achievements_progress_slash_unauth",
            ),
        ] {
            let response = send(&app, "GET", path, None).await;
            let status = response.status();
            assert_json_content_type(&response);
            let body = body_json(response).await;
            assert_fixture_and_snapshot(name, status, body);
        }
    }

    // --- OPTIONS regimes -----------------------------------------------------

    #[tokio::test]
    async fn options_achievements_progress_both_variants_204_empty() {
        // App-wide automatic OPTIONS (contract change): 204, empty body, no auth.
        let app = test_app();
        for (path, name) in [
            (
                "/api/achievements/progress",
                "options_achievements_progress",
            ),
            (
                "/api/achievements/progress/",
                "options_achievements_progress_slash",
            ),
        ] {
            let response = send(&app, "OPTIONS", path, None).await;
            let status = response.status();
            assert_eq!(status, StatusCode::NO_CONTENT, "{path}");
            assert_eq!(
                response
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok()),
                Some("text/html; charset=utf-8"),
                "{path}"
            );
            let body = body_string(response).await;
            assert_fixture_and_snapshot(name, status, json!(body));
        }
    }

    #[tokio::test]
    async fn automatic_options_on_other_achievements_rules() {
        // App-wide automatic OPTIONS (contract change, not
        // fixture-recorded for this family): 204, empty body, Allow header.
        let app = test_app();
        for (path, allow) in [
            ("/api/achievements", "HEAD, GET, OPTIONS"),
            ("/api/achievements/check", "POST, OPTIONS"),
        ] {
            let response = send(&app, "OPTIONS", path, None).await;
            assert_eq!(response.status(), StatusCode::NO_CONTENT, "{path}");
            assert_eq!(
                response
                    .headers()
                    .get(header::ALLOW)
                    .and_then(|value| value.to_str().ok()),
                Some(allow),
                "{path}"
            );
            assert_eq!(body_string(response).await, "", "{path}");
        }
    }
}
