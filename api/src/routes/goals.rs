//! Goals route family — port of `api/routes/goal_routes.py` with the
//! service-layer logic from `api/services/goal_service.py` folded into the
//! handlers. Data access goes through [`crate::db::goals`] (and the
//! best-effort activity log through [`crate::db::activity`]) under
//! `spawn_blocking`.
//!
//! Routing notes (fixture-verified, `contract/fixtures/goals/`):
//! - `/goals` and `/goals/{id}` are strict-slash rules: the trailing-slash
//!   variants fall through to the app-level JSON 404.
//! - The completions rule family was registered with `strict_slashes=False`
//!   in Flask, so all four alias paths
//!   (`/goals/{id}/completions`[`/`], `/goal/{id}/completions`[`/`]) are
//!   registered here explicitly. contract change (owner-approved):
//!   every OPTIONS in the family — automatic and completions alike — now
//!   answers 204 empty with an `Allow` header and no auth required; the
//!   old two-regime split (automatic 200 vs. explicit 204) is gone.
//! - Path ids use [`FlaskPath`]`<`[`FlaskInt`]`>`, extracted BEFORE
//!   [`AuthUser`]: like Flask, a request whose id the `<int:...>` converter
//!   would refuse 404s even without credentials.
//!
//! Behavior notes:
//! - contract change (owner-approved): goal GETs are pure reads. The
//!   weekly rollover is projected into responses by `db::goals::get_goals`
//!   / `get_goal_by_id` without writing; only the write paths (progress,
//!   update) persist it.
//! - `PUT`/`PATCH` share one handler (same Werkzeug rule). Contract
//!   change (owner-approved, `contract/DECISIONS.md` item 7): an update
//!   with no recognized fields is a 400 `No fields to update`, and a
//!   blank/whitespace title is rejected with create's 400
//!   `Title is required` — only a genuinely missing/foreign row is the
//!   404 `No changes or goal not found`. (Flask collapsed empty-body and
//!   missing-row into one 404 and silently wrote blank titles.)
//! - `POST /goals/{id}/progress` parses its body with
//!   `get_json(silent=True)` tolerance: a missing or malformed body simply
//!   logs today. A truthy `date` value is validated like
//!   `validate_completion_date` (ISO or M/D/YYYY, one day of future slack)
//!   and 400s with the validator's message when unparseable.
//! - `GET .../completions` passes explicit `start`/`end` bounds to SQL
//!   verbatim. contract change (owner-approved status-code
//!   consistency fix): a nonexistent/foreign goal now 404s with
//!   `GET /goals/{id}`'s exact body (`Not found`) on all four alias
//!   paths, instead of Flask's recorded 200-`[]` quirk. An existing goal
//!   with no completions still returns 200 `[]`.

use axum::body::Bytes;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodRouter, get, post};
use axum::{Json, Router};
use chrono::NaiveDate;
use serde_json::{Value, json};

use super::{FlaskInt, FlaskPath, automatic_options};
use crate::auth::extract::AuthUser;
use crate::db::activity;
use crate::db::goals::{self as db_goals, GoalsError};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// `Allow` for `/goals` (two Werkzeug rules on one path: GET and POST).
const GOALS_ALLOW: &str = "HEAD, GET, OPTIONS, POST";
/// `Allow` for `/goals/{id}` (three rules: GET; PUT+PATCH; DELETE).
const GOAL_ITEM_ALLOW: &str = "HEAD, GET, OPTIONS, PUT, PATCH, DELETE";
/// `Allow` for the POST-only progress rule.
const PROGRESS_ALLOW: &str = "POST, OPTIONS";
/// `Allow` for the GET-only completions alias rules.
const COMPLETIONS_ALLOW: &str = "HEAD, GET, OPTIONS";

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/goals",
            get(list_goals)
                .post(create_goal)
                .merge(automatic_options(GOALS_ALLOW)),
        )
        .route(
            "/goals/{goal_id}",
            get(get_goal)
                .put(update_goal)
                .patch(update_goal)
                .delete(delete_goal)
                .merge(automatic_options_with_id(GOAL_ITEM_ALLOW)),
        )
        .route(
            "/goals/{goal_id}/progress",
            post(increment_progress).merge(automatic_options_with_id(PROGRESS_ALLOW)),
        )
        // The strict_slashes=False completions rule family: all four alias
        // paths registered explicitly, sharing the unified OPTIONS 204.
        .route(
            "/goals/{goal_id}/completions",
            get(get_completions).merge(automatic_options_with_id(COMPLETIONS_ALLOW)),
        )
        .route(
            "/goals/{goal_id}/completions/",
            get(get_completions).merge(automatic_options_with_id(COMPLETIONS_ALLOW)),
        )
        .route(
            "/goal/{goal_id}/completions",
            get(get_completions).merge(automatic_options_with_id(COMPLETIONS_ALLOW)),
        )
        .route(
            "/goal/{goal_id}/completions/",
            get(get_completions).merge(automatic_options_with_id(COMPLETIONS_ALLOW)),
        )
}

// ---------------------------------------------------------------------------
// OPTIONS variants
// ---------------------------------------------------------------------------

/// Unified OPTIONS (contract change) for rules with a `<int:...>`
/// segment: 204, empty body, `Allow` header and Flask's
/// `text/html; charset=utf-8` label kept — same shape as the shared
/// [`automatic_options`] helper, which takes no path and therefore cannot
/// enforce that the id still satisfies the converter (a non-integer id
/// never matched the rule → JSON 404). No auth required; the completions
/// aliases (fixture `routing-completions-options-204.json`) route through
/// this too.
fn automatic_options_with_id(allow: &'static str) -> MethodRouter<AppState> {
    axum::routing::options(move |_: FlaskPath<FlaskInt>| async move {
        (
            StatusCode::NO_CONTENT,
            [
                (header::ALLOW, HeaderValue::from_static(allow)),
                (
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("text/html; charset=utf-8"),
                ),
            ],
        )
    })
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /goals` — bare array, weekly rollover projected per row by the db
/// layer (pure read), ordered by `created_at DESC`.
async fn list_goals(State(state): State<AppState>, user: AuthUser) -> ApiResult<Response> {
    let user_id = user.user_id;
    let goals = with_conn(state, move |conn| db_goals::get_goals(conn, user_id)).await?;
    Ok(Json(goals).into_response())
}

/// `POST /goals` — 201 `{"id": ...}`. Python computes
/// `int(freq_per_week or freq or 0)` BEFORE the title check, so a
/// non-integer frequency 400s first; the route-level bounds message is
/// `frequency_per_week must be 1..7` (distinct from the db layer's).
async fn create_goal(
    State(state): State<AppState>,
    user: AuthUser,
    body: Bytes,
) -> ApiResult<Response> {
    let data = parse_json(&body);
    // Python: `int(data.get("frequency_per_week") or data.get("frequency") or 0)`
    // — falsy values (0, "", null, false) fall through the `or` chain.
    let frequency = match [data.get("frequency_per_week"), data.get("frequency")]
        .into_iter()
        .flatten()
        .find(|value| is_py_truthy(value))
    {
        Some(value) => python_int(value)?,
        None => 0,
    };
    let title = data
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let description = data
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if title.is_empty() {
        return Err(ApiError::validation("Title is required"));
    }
    if !(1..=7).contains(&frequency) {
        return Err(ApiError::validation("frequency_per_week must be 1..7"));
    }
    let user_id = user.user_id;
    let goal_id = with_conn(state, move |conn| {
        db_goals::create_goal(conn, user_id, &title, &description, frequency)
    })
    .await?;
    Ok((StatusCode::CREATED, Json(json!({ "id": goal_id }))).into_response())
}

/// `GET /goals/{id}` — single goal (rollover projected, pure read) or 404
/// `Not found`.
async fn get_goal(
    FlaskPath(FlaskInt(goal_id)): FlaskPath<FlaskInt>,
    State(state): State<AppState>,
    user: AuthUser,
) -> ApiResult<Response> {
    let user_id = user.user_id;
    let goal = with_conn(state, move |conn| {
        db_goals::get_goal_by_id(conn, user_id, goal_id)
    })
    .await?;
    match goal {
        Some(goal) => Ok(Json(goal).into_response()),
        None => Err(ApiError::NotFound("Not found".to_string())),
    }
}

/// `PUT`/`PATCH /goals/{id}` — partial update; success body is always
/// `{"status": "ok"}`. `frequency` is a legacy alias checked with `is None`
/// (unlike create's falsy chain), so an explicit `frequency_per_week: 0`
/// reaches the db layer and 400s with its
/// `frequency_per_week must be between 1 and 7` message.
///
/// contract change (owner-approved): a blank/whitespace title 400s
/// with create's `Title is required`, an update carrying no updatable
/// fields 400s with `No fields to update`, and the 404 is reserved for a
/// missing/foreign goal id. Field validation runs before the row lookup,
/// so an empty body 400s even when the id does not exist.
async fn update_goal(
    FlaskPath(FlaskInt(goal_id)): FlaskPath<FlaskInt>,
    State(state): State<AppState>,
    user: AuthUser,
    body: Bytes,
) -> ApiResult<Response> {
    let data = parse_json(&body);
    let title = data
        .get("title")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let description = data
        .get("description")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let frequency = data
        .get("frequency_per_week")
        .filter(|value| !value.is_null())
        .or_else(|| data.get("frequency").filter(|value| !value.is_null()))
        .map(python_int)
        .transpose()?;
    if let Some(title) = title.as_deref()
        && title.trim().is_empty()
    {
        // Same message the create path uses for a blank title.
        return Err(ApiError::validation("Title is required"));
    }
    if title.is_none() && description.is_none() && frequency.is_none() {
        return Err(ApiError::validation("No fields to update"));
    }
    let user_id = user.user_id;
    let success = with_conn(state, move |conn| {
        db_goals::update_goal(
            conn,
            user_id,
            goal_id,
            title.as_deref(),
            description.as_deref(),
            frequency,
        )
    })
    .await?;
    if success {
        Ok(Json(json!({ "status": "ok" })).into_response())
    } else {
        // Only reachable for a missing/foreign row now — the field checks
        // above already 400ed the "no changes" half of the legacy message
        // (fixture goal-update-404-not-found.json keeps the recorded text).
        Err(ApiError::NotFound(
            "No changes or goal not found".to_string(),
        ))
    }
}

/// `DELETE /goals/{id}` — JSON body on success (the SPA client throws on
/// any 2xx that is not `application/json`; never 204 here).
async fn delete_goal(
    FlaskPath(FlaskInt(goal_id)): FlaskPath<FlaskInt>,
    State(state): State<AppState>,
    user: AuthUser,
) -> ApiResult<Response> {
    let user_id = user.user_id;
    let success = with_conn(state, move |conn| {
        db_goals::delete_goal(conn, user_id, goal_id)
    })
    .await?;
    if success {
        Ok(Json(json!({ "status": "ok" })).into_response())
    } else {
        Err(ApiError::NotFound("Not found".to_string()))
    }
}

/// `POST /goals/{id}/progress` — logs a completion (today by default, a
/// validated backdate with a `{date}` body) and returns the full updated
/// goal row plus `already_logged` / `logged_date`. The service layer's
/// best-effort `goal_completed` activity write is preserved: only when a
/// NEW completion day was recorded, and never allowed to fail the request.
async fn increment_progress(
    FlaskPath(FlaskInt(goal_id)): FlaskPath<FlaskInt>,
    State(state): State<AppState>,
    user: AuthUser,
    body: Bytes,
) -> ApiResult<Response> {
    // request.get_json(silent=True) or {} — malformed/missing body is {}.
    let data = parse_json(&body);
    let date_str = match data.get("date") {
        Some(value) if is_py_truthy(value) => {
            Some(validate_completion_date(value).map_err(ApiError::Validation)?)
        }
        _ => None,
    };
    let user_id = user.user_id;
    let updated = with_conn(state, move |conn| {
        let result =
            db_goals::increment_goal_progress(conn, user_id, goal_id, date_str.as_deref())?;
        if let Some(progress) = &result
            && !progress.already_logged
        {
            let metadata = json!({
                "goal_id": goal_id,
                "title": progress.goal.title,
                "date": progress.logged_date,
            });
            // Best-effort, like the Python try/except pass.
            let _ = activity::add_activity(conn, user_id, "goal_completed", Some(&metadata));
        }
        Ok(result)
    })
    .await?;
    match updated {
        Some(progress) => Ok(Json(progress).into_response()),
        None => Err(ApiError::NotFound("Not found".to_string())),
    }
}

/// `GET .../completions` — bare array of `{date}` rows, ascending; both
/// bounds default to the last 90 days when either is absent/empty (the db
/// layer mirrors the Python falsy check).
///
/// contract change (owner-approved): a nonexistent/foreign goal is a
/// 404 with `GET /goals/{id}`'s body (`Not found`) instead of 200 `[]`.
/// The existence probe reuses `get_goal_by_id`, which since the rewrite is a
/// pure read — probing never writes anything.
async fn get_completions(
    FlaskPath(FlaskInt(goal_id)): FlaskPath<FlaskInt>,
    State(state): State<AppState>,
    user: AuthUser,
    RawQuery(query): RawQuery,
) -> ApiResult<Response> {
    let start = query_param(query.as_deref(), "start");
    let end = query_param(query.as_deref(), "end");
    let user_id = user.user_id;
    let rows = with_conn(state, move |conn| {
        if db_goals::get_goal_by_id(conn, user_id, goal_id)?.is_none() {
            return Ok(None);
        }
        db_goals::get_goal_completions(conn, user_id, goal_id, start.as_deref(), end.as_deref())
            .map(Some)
    })
    .await?;
    match rows {
        Some(rows) => Ok(Json(rows).into_response()),
        None => Err(ApiError::NotFound("Not found".to_string())),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Run a goals db call on a pooled connection under `spawn_blocking`.
async fn with_conn<T, F>(state: AppState, f: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce(&rusqlite::Connection) -> Result<T, GoalsError> + Send + 'static,
{
    tokio::task::spawn_blocking(move || -> ApiResult<T> {
        let conn = state.pool.get()?;
        f(&conn).map_err(goals_error)
    })
    .await
    .map_err(|exc| ApiError::Internal(anyhow::anyhow!("blocking task failed: {exc}")))?
}

/// `GoalsError` → `ApiError`: validation messages reach the client as 400
/// (Flask's `except ValueError`), everything else is an internal 500.
fn goals_error(err: GoalsError) -> ApiError {
    match err {
        GoalsError::Validation(message) => ApiError::Validation(message),
        GoalsError::Database(inner) => ApiError::Internal(anyhow::anyhow!(inner)),
        GoalsError::Sqlite(inner) => ApiError::Database(inner),
    }
}

/// Flask `request.args.get(key)`: first occurrence wins, `+` and
/// percent-escapes decode, absent key is `None`. Never rejects the request
/// (unlike `axum::extract::Query`'s 400 rejection, which Flask has no
/// equivalent for).
fn query_param(query: Option<&str>, key: &str) -> Option<String> {
    let query = query?;
    for pair in query.split('&') {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        if name == key {
            let plus_decoded = value.replace('+', " ");
            return Some(
                urlencoding::decode(&plus_decoded)
                    .map(|decoded| decoded.into_owned())
                    .unwrap_or(plus_decoded),
            );
        }
    }
    None
}

/// Lenient body parse: any unreadable/missing body behaves like `{}`
/// (`data.get(...)` on a non-object returns nothing).
fn parse_json(body: &[u8]) -> Value {
    serde_json::from_slice(body).unwrap_or(Value::Null)
}

/// Python truthiness for JSON values.
fn is_py_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|float| float != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

/// Python `int(x)` over JSON values: bools count, floats truncate toward
/// zero, strings parse after trimming. Failures map to the Flask
/// `ValueError` handler's 400 (message text is not part of the contract).
fn python_int(value: &Value) -> Result<i64, ApiError> {
    match value {
        Value::Bool(flag) => Ok(i64::from(*flag)),
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|float| float.trunc() as i64))
            .ok_or_else(|| {
                ApiError::validation(format!(
                    "invalid literal for int() with base 10: '{number}'"
                ))
            }),
        Value::String(text) => {
            let trimmed = text.trim();
            trimmed.parse::<i64>().map_err(|_| {
                ApiError::validation(format!(
                    "invalid literal for int() with base 10: '{trimmed}'"
                ))
            })
        }
        other => Err(ApiError::validation(format!(
            "int() argument must be a string or a number, not {other}"
        ))),
    }
}

/// Python `str(x)` for the date value handed to the validator.
fn python_str(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Null => "None".to_string(),
        other => other.to_string(),
    }
}

/// Port of `api/utils/validators.py::validate_completion_date`: accepts
/// ISO or legacy M/D/YYYY, allows one day of future slack, returns the
/// date normalized to ISO. Error strings are the Python messages verbatim.
fn validate_completion_date(value: &Value) -> Result<String, String> {
    let text = python_str(value);
    let parsed = NaiveDate::parse_from_str(&text, "%Y-%m-%d")
        .or_else(|_| NaiveDate::parse_from_str(&text, "%m/%d/%Y"))
        .map_err(|_| "date must be YYYY-MM-DD or M/D/YYYY".to_string())?;
    if parsed > chrono::Local::now().date_naive() + chrono::Duration::days(1) {
        return Err("date cannot be in the future".to_string());
    }
    Ok(parsed.format("%Y-%m-%d").to_string())
}

// ---------------------------------------------------------------------------
// Tests — golden fixtures from contract/fixtures/goals/
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use rstest::rstest;
    use tower::ServiceExt;

    use crate::auth::jwt;
    use crate::config::Config;
    use crate::db::{self, SelfHostSeed};

    // -- harness ----------------------------------------------------------

    /// Full app (router + middleware) on a fresh tempfile database, plus a
    /// Bearer token for the seeded self-host user (id 1).
    fn make_app() -> (Router, String, tempfile::TempDir) {
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
        let app = crate::routes::build_router(AppState::new(cfg, pool));
        (app, token, dir)
    }

    fn request(
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<&Value>,
    ) -> Request<Body> {
        let mut builder = Request::builder().method(method).uri(path);
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        match body {
            Some(value) => builder
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(value.to_string()))
                .unwrap(),
            None => builder.body(Body::empty()).unwrap(),
        }
    }

    async fn send(app: &Router, req: Request<Body>) -> Response {
        app.clone().oneshot(req).await.expect("infallible")
    }

    async fn body_value(response: Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("body");
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("json body")
        }
    }

    fn fixture(name: &str) -> Value {
        let path = format!(
            "{}/../contract/fixtures/goals/{name}.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|exc| panic!("missing fixture {path}: {exc}"));
        serde_json::from_str(&raw).expect("fixture json")
    }

    // -- normalization (mirrors the recorder's rules) ---------------------

    fn matches_pattern(text: &str, pattern: &str) -> bool {
        // 'd' = ASCII digit, any other pattern byte matches itself.
        text.len() == pattern.len()
            && text.bytes().zip(pattern.bytes()).all(|(byte, spec)| {
                if spec == b'd' {
                    byte.is_ascii_digit()
                } else {
                    byte == spec
                }
            })
    }

    fn normalize(value: &mut Value) {
        match value {
            Value::String(text) => {
                if matches_pattern(text, "dddd-dd-dd dd:dd:dd") {
                    *value = Value::String("<TS>".to_string());
                } else if matches_pattern(text, "dddd-dd-dd") {
                    *value = Value::String("<DATE>".to_string());
                }
            }
            Value::Array(items) => items.iter_mut().for_each(normalize),
            Value::Object(map) => map.values_mut().for_each(normalize),
            _ => {}
        }
    }

    /// Assert status + content-type + normalized body against a fixture,
    /// then pin the result as an insta snapshot.
    async fn expect_fixture(name: &str, response: Response) {
        let fx = fixture(name);
        let expected = fx["response"]["body"].clone();
        expect_fixture_body(name, response, expected).await;
    }

    async fn expect_fixture_body(name: &str, response: Response, expected: Value) {
        let fx = fixture(name);
        let expected_status =
            u16::try_from(fx["response"]["status"].as_u64().expect("status")).unwrap();
        assert_eq!(
            response.status().as_u16(),
            expected_status,
            "{name}: status"
        );
        if let Some(content_type) = fx["response"]["content_type"].as_str() {
            let actual_ct = response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("");
            assert!(
                actual_ct.starts_with(content_type),
                "{name}: content type {actual_ct:?} !~ {content_type:?}"
            );
        }
        let mut actual = body_value(response).await;
        normalize(&mut actual);
        assert_eq!(actual, expected, "{name}: body");
        insta::assert_json_snapshot!(name, json!({ "status": expected_status, "body": actual }));
    }

    fn today() -> NaiveDate {
        chrono::Local::now().date_naive()
    }

    fn iso(date: NaiveDate) -> String {
        date.format("%Y-%m-%d").to_string()
    }

    fn this_monday() -> NaiveDate {
        use chrono::Datelike;
        today() - chrono::Duration::days(i64::from(today().weekday().num_days_from_monday()))
    }

    async fn create(app: &Router, token: &str, body: Value) -> Response {
        send(app, request("POST", "/api/goals", Some(token), Some(&body))).await
    }

    // -- list + create ----------------------------------------------------

    #[tokio::test]
    async fn list_empty_then_seeded_matches_fixtures() {
        let (app, token, _dir) = make_app();
        let response = send(&app, request("GET", "/api/goals", Some(&token), None)).await;
        expect_fixture("goals-list-empty", response).await;

        create(
            &app,
            &token,
            json!({ "title": "Meditate", "description": "10 minutes", "frequency_per_week": 3 }),
        )
        .await;
        create(
            &app,
            &token,
            json!({ "title": "Read", "frequency_per_week": 2 }),
        )
        .await;

        let response = send(&app, request("GET", "/api/goals", Some(&token), None)).await;
        expect_fixture("goals-list-seeded", response).await;
    }

    #[tokio::test]
    async fn create_matches_fixtures() {
        let (app, token, _dir) = make_app();
        let response = create(
            &app,
            &token,
            json!({ "title": "Meditate", "description": "10 minutes", "frequency_per_week": 3 }),
        )
        .await;
        expect_fixture("goals-create-201", response).await;

        let response = create(
            &app,
            &token,
            json!({ "title": "Bad freq", "frequency_per_week": 9 }),
        )
        .await;
        expect_fixture("goals-create-400-bad-frequency", response).await;

        let response = create(
            &app,
            &token,
            json!({ "description": "no title", "frequency_per_week": 3 }),
        )
        .await;
        expect_fixture("goals-create-400-missing-title", response).await;
    }

    // -- get + routing -----------------------------------------------------

    #[tokio::test]
    async fn get_goal_matches_fixtures() {
        let (app, token, _dir) = make_app();
        create(
            &app,
            &token,
            json!({ "title": "Meditate", "description": "10 minutes", "frequency_per_week": 3 }),
        )
        .await;

        let response = send(&app, request("GET", "/api/goals/1", Some(&token), None)).await;
        expect_fixture("goal-get-200", response).await;

        let response = send(
            &app,
            request("GET", "/api/goals/999999", Some(&token), None),
        )
        .await;
        expect_fixture("goal-get-404", response).await;

        let response = send(&app, request("GET", "/api/goals/-1", Some(&token), None)).await;
        expect_fixture("routing-goals-negative-id-404", response).await;

        let response = send(&app, request("GET", "/api/goals/", Some(&token), None)).await;
        expect_fixture("routing-goals-trailing-slash-404", response).await;
    }

    // -- update ------------------------------------------------------------

    #[tokio::test]
    async fn update_matches_fixtures() {
        let (app, token, _dir) = make_app();
        create(
            &app,
            &token,
            json!({ "title": "Meditate", "description": "10 minutes", "frequency_per_week": 3 }),
        )
        .await;

        let response = send(
            &app,
            request(
                "PUT",
                "/api/goals/1",
                Some(&token),
                Some(&json!({ "title": "Meditate more", "frequency_per_week": 4 })),
            ),
        )
        .await;
        expect_fixture("goal-put-200", response).await;

        let response = send(
            &app,
            request(
                "PATCH",
                "/api/goals/1",
                Some(&token),
                Some(&json!({ "description": "15 minutes" })),
            ),
        )
        .await;
        expect_fixture("goal-patch-200", response).await;

        let response = send(
            &app,
            request(
                "PUT",
                "/api/goals/1",
                Some(&token),
                Some(&json!({ "frequency_per_week": 0 })),
            ),
        )
        .await;
        expect_fixture("goal-update-400-bad-frequency", response).await;

        let response = send(
            &app,
            request("PUT", "/api/goals/1", Some(&token), Some(&json!({}))),
        )
        .await;
        expect_fixture("goal-update-404-no-fields", response).await;

        let response = send(
            &app,
            request(
                "PUT",
                "/api/goals/999999",
                Some(&token),
                Some(&json!({ "title": "nope" })),
            ),
        )
        .await;
        expect_fixture("goal-update-404-not-found", response).await;
    }

    /// contract change (`contract/DECISIONS.md` item 7) matrix:
    /// empty body → 400 `No fields to update` (distinguished from the
    /// missing-row 404), blank/whitespace title → create's 400
    /// `Title is required`, missing/foreign row with real fields → 404,
    /// valid update → 200. PUT and PATCH share the handler, so every case
    /// runs under both methods.
    #[rstest]
    #[case::empty_body_400("/api/goals/1", json!({}), 400, json!({ "error": "No fields to update" }))]
    #[case::no_updatable_fields_400("/api/goals/1", json!({ "bogus": 1 }), 400, json!({ "error": "No fields to update" }))]
    #[case::blank_title_400("/api/goals/1", json!({ "title": "" }), 400, json!({ "error": "Title is required" }))]
    #[case::whitespace_title_400("/api/goals/1", json!({ "title": "   " }), 400, json!({ "error": "Title is required" }))]
    #[case::blank_title_wins_over_other_fields("/api/goals/1", json!({ "title": " ", "description": "kept" }), 400, json!({ "error": "Title is required" }))]
    #[case::empty_body_on_missing_row_still_400("/api/goals/999999", json!({}), 400, json!({ "error": "No fields to update" }))]
    #[case::blank_title_on_missing_row_still_400("/api/goals/999999", json!({ "title": "" }), 400, json!({ "error": "Title is required" }))]
    #[case::missing_row_404("/api/goals/999999", json!({ "title": "nope" }), 404, json!({ "error": "No changes or goal not found" }))]
    #[case::valid_update_200("/api/goals/1", json!({ "title": "Renamed" }), 200, json!({ "status": "ok" }))]
    #[tokio::test]
    async fn update_asymmetries_v05(
        #[case] path: &str,
        #[case] body: Value,
        #[case] status: u16,
        #[case] expected: Value,
    ) {
        let (app, token, _dir) = make_app();
        create(
            &app,
            &token,
            json!({ "title": "Meditate", "description": "10 minutes", "frequency_per_week": 3 }),
        )
        .await;
        for method in ["PUT", "PATCH"] {
            let response = send(&app, request(method, path, Some(&token), Some(&body))).await;
            assert_eq!(response.status().as_u16(), status, "{method} {path} {body}");
            assert_eq!(
                body_value(response).await,
                expected,
                "{method} {path} {body}"
            );
        }
    }

    #[tokio::test]
    async fn rejected_blank_title_update_leaves_row_untouched() {
        // contract change: the blank-title 400 must not write '' the
        // way Flask silently did. No recorded fixture exists for this case;
        // the insta snapshot below pins the new contract shape.
        let (app, token, _dir) = make_app();
        create(
            &app,
            &token,
            json!({ "title": "Meditate", "description": "10 minutes", "frequency_per_week": 3 }),
        )
        .await;

        let response = send(
            &app,
            request(
                "PUT",
                "/api/goals/1",
                Some(&token),
                Some(&json!({ "title": "   " })),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = body_value(response).await;
        assert_eq!(body, json!({ "error": "Title is required" }));
        insta::assert_json_snapshot!(
            "goal-update-400-blank-title",
            json!({ "status": 400, "body": body })
        );

        let response = send(&app, request("GET", "/api/goals/1", Some(&token), None)).await;
        let body = body_value(response).await;
        assert_eq!(body["title"], json!("Meditate"), "title must be untouched");
        assert_eq!(body["description"], json!("10 minutes"));
    }

    // -- delete ------------------------------------------------------------

    #[tokio::test]
    async fn delete_matches_fixtures() {
        let (app, token, _dir) = make_app();
        create(
            &app,
            &token,
            json!({ "title": "Meditate", "frequency_per_week": 3 }),
        )
        .await;
        create(
            &app,
            &token,
            json!({ "title": "Read", "frequency_per_week": 2 }),
        )
        .await;

        let response = send(&app, request("DELETE", "/api/goals/2", Some(&token), None)).await;
        expect_fixture("goal-delete-200", response).await;

        let response = send(&app, request("DELETE", "/api/goals/2", Some(&token), None)).await;
        expect_fixture("goal-delete-404", response).await;
    }

    // -- progress ----------------------------------------------------------

    #[tokio::test]
    async fn progress_flow_matches_fixtures() {
        // Mirrors the recorder's session: create → PUT → PATCH → progress.
        let (app, token, _dir) = make_app();
        create(
            &app,
            &token,
            json!({ "title": "Meditate", "description": "10 minutes", "frequency_per_week": 3 }),
        )
        .await;
        send(
            &app,
            request(
                "PUT",
                "/api/goals/1",
                Some(&token),
                Some(&json!({ "title": "Meditate more", "frequency_per_week": 4 })),
            ),
        )
        .await;
        send(
            &app,
            request(
                "PATCH",
                "/api/goals/1",
                Some(&token),
                Some(&json!({ "description": "15 minutes" })),
            ),
        )
        .await;

        let response = send(
            &app,
            request("POST", "/api/goals/1/progress", Some(&token), None),
        )
        .await;
        expect_fixture("goal-progress-200-today", response).await;

        let response = send(
            &app,
            request("POST", "/api/goals/1/progress", Some(&token), None),
        )
        .await;
        expect_fixture("goal-progress-200-already-logged", response).await;

        // Backdate to yesterday (the fixture's date was 2 days before its
        // run date, same week). When today is Monday, yesterday falls in
        // the PREVIOUS week: the completion is logged but the closed
        // week's counter does not move, so `completed` stays 1 — patch the
        // expectation accordingly and keep the snapshot pinned at the
        // fixture's recorded value.
        let backdate = today() - chrono::Duration::days(1);
        let response = send(
            &app,
            request(
                "POST",
                "/api/goals/1/progress",
                Some(&token),
                Some(&json!({ "date": iso(backdate) })),
            ),
        )
        .await;
        let name = "goal-progress-200-backdate";
        let mut expected = fixture(name)["response"]["body"].clone();
        let in_period = backdate >= this_monday();
        if !in_period {
            expected["completed"] = json!(1);
        }
        assert_eq!(response.status(), StatusCode::OK, "{name}: status");
        let mut actual = body_value(response).await;
        normalize(&mut actual);
        assert_eq!(actual, expected, "{name}: body");
        // Deterministic snapshot: pin the fixture's recorded counter.
        actual["completed"] = json!(2);
        insta::assert_json_snapshot!(name, json!({ "status": 200, "body": actual }));

        let response = send(
            &app,
            request(
                "POST",
                "/api/goals/1/progress",
                Some(&token),
                Some(&json!({ "date": "not-a-date" })),
            ),
        )
        .await;
        expect_fixture("goal-progress-400-bad-date", response).await;

        let response = send(
            &app,
            request("POST", "/api/goals/999999/progress", Some(&token), None),
        )
        .await;
        expect_fixture("goal-progress-404", response).await;
    }

    #[tokio::test]
    async fn progress_rejects_far_future_date() {
        // validate_completion_date: > today + 1 day → 400 (message body is
        // the Python string; only status + shape are contract).
        let (app, token, _dir) = make_app();
        create(
            &app,
            &token,
            json!({ "title": "g", "frequency_per_week": 1 }),
        )
        .await;
        let response = send(
            &app,
            request(
                "POST",
                "/api/goals/1/progress",
                Some(&token),
                Some(&json!({ "date": iso(today() + chrono::Duration::days(2)) })),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            body_value(response).await,
            json!({ "error": "date cannot be in the future" })
        );
    }

    // -- completions --------------------------------------------------------

    #[tokio::test]
    async fn completions_match_fixtures() {
        let (app, token, _dir) = make_app();
        create(
            &app,
            &token,
            json!({ "title": "Meditate", "description": "10 minutes", "frequency_per_week": 3 }),
        )
        .await;
        create(
            &app,
            &token,
            json!({ "title": "Read", "frequency_per_week": 2 }),
        )
        .await;
        // Two completion days on goal 1: today and yesterday (both inside
        // the default 90-day window regardless of weekday).
        send(
            &app,
            request("POST", "/api/goals/1/progress", Some(&token), None),
        )
        .await;
        send(
            &app,
            request(
                "POST",
                "/api/goals/1/progress",
                Some(&token),
                Some(&json!({ "date": iso(today() - chrono::Duration::days(1)) })),
            ),
        )
        .await;

        let response = send(
            &app,
            request("GET", "/api/goals/1/completions", Some(&token), None),
        )
        .await;
        expect_fixture("goal-completions-200", response).await;

        let response = send(
            &app,
            request(
                "GET",
                "/api/goals/1/completions?start=2000-01-01&end=2100-01-01",
                Some(&token),
                None,
            ),
        )
        .await;
        expect_fixture("goal-completions-200-range", response).await;

        let response = send(
            &app,
            request("GET", "/api/goals/2/completions", Some(&token), None),
        )
        .await;
        expect_fixture("goal-completions-200-empty", response).await;

        let response = send(
            &app,
            request("GET", "/api/goals/999999/completions", Some(&token), None),
        )
        .await;
        expect_fixture("goal-completions-404-nonexistent-goal", response).await;

        // Aliases: singular and trailing-slash rules, same handler/body.
        let response = send(
            &app,
            request("GET", "/api/goal/1/completions", Some(&token), None),
        )
        .await;
        expect_fixture("routing-completions-alias-singular", response).await;

        let response = send(
            &app,
            request("GET", "/api/goals/1/completions/", Some(&token), None),
        )
        .await;
        expect_fixture("routing-completions-alias-trailing-slash", response).await;

        // strict_slashes=False also covers the singular + slash variant
        // (no fixture recorded; Flask matches it without a redirect).
        let response = send(
            &app,
            request("GET", "/api/goal/1/completions/", Some(&token), None),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = body_value(response).await;
        normalize(&mut body);
        assert_eq!(body, json!([{ "date": "<DATE>" }, { "date": "<DATE>" }]));
    }

    /// contract change matrix, per alias path: goal with completions
    /// → 200 rows; existing goal without completions → 200 []; missing and
    /// foreign-user goals → the same 404 body `GET /goals/{id}` uses.
    #[rstest]
    #[case::plural("/api/goals")]
    #[case::singular("/api/goal")]
    #[tokio::test]
    async fn completions_present_missing_and_foreign_matrix_v05(
        #[case] prefix: &str,
        #[values("", "/")] suffix: &str,
    ) {
        let (app, token, dir) = make_app();
        // Goal 1 (user 1) with one completion; goal 2 (user 1) with none.
        create(
            &app,
            &token,
            json!({ "title": "Meditate", "frequency_per_week": 3 }),
        )
        .await;
        create(
            &app,
            &token,
            json!({ "title": "Read", "frequency_per_week": 2 }),
        )
        .await;
        send(
            &app,
            request("POST", "/api/goals/1/progress", Some(&token), None),
        )
        .await;
        // Goal 3 belongs to a second user, inserted directly.
        {
            let db_path = dir.path().join("nightlio.db");
            let conn = db::connect(db_path.to_str().expect("utf-8 path")).expect("connect");
            conn.execute(
                "INSERT INTO users (google_id, email, name) VALUES ('other-user', 'other@example.com', 'Other')",
                [],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO goals (user_id, title, description, frequency_per_week) VALUES (2, 'Theirs', '', 3)",
                [],
            )
            .unwrap();
        }

        let url = |goal_id: &str| format!("{prefix}/{goal_id}/completions{suffix}");

        let response = send(&app, request("GET", &url("1"), Some(&token), None)).await;
        assert_eq!(response.status(), StatusCode::OK, "{}", url("1"));
        let mut body = body_value(response).await;
        normalize(&mut body);
        assert_eq!(body, json!([{ "date": "<DATE>" }]), "{}", url("1"));

        let response = send(&app, request("GET", &url("2"), Some(&token), None)).await;
        assert_eq!(response.status(), StatusCode::OK, "{}", url("2"));
        assert_eq!(body_value(response).await, json!([]), "{}", url("2"));

        for goal_id in ["999999", "3"] {
            let path = url(goal_id);
            let response = send(&app, request("GET", &path, Some(&token), None)).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            assert_eq!(
                body_value(response).await,
                json!({ "error": "Not found" }),
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn completions_options_is_204_without_auth() {
        let (app, _token, _dir) = make_app();
        let response = send(
            &app,
            request("OPTIONS", "/api/goals/1/completions", None, None),
        )
        .await;
        expect_fixture("routing-completions-options-204", response).await;

        for path in [
            "/api/goals/1/completions/",
            "/api/goal/1/completions",
            "/api/goal/1/completions/",
        ] {
            let response = send(&app, request("OPTIONS", path, None, None)).await;
            assert_eq!(response.status(), StatusCode::NO_CONTENT, "{path}");
            assert_eq!(body_value(response).await, Value::Null, "{path}");
        }
    }

    // -- auth ----------------------------------------------------------------

    #[tokio::test]
    async fn unauthenticated_requests_match_401_fixtures() {
        let (app, _token, _dir) = make_app();
        let cases: [(&str, &str, &str, Option<Value>); 7] = [
            ("goals-list-401", "GET", "/api/goals", None),
            (
                "goals-create-401",
                "POST",
                "/api/goals",
                Some(json!({ "title": "X", "frequency_per_week": 1 })),
            ),
            ("goal-get-401", "GET", "/api/goals/1", None),
            (
                "goal-update-401",
                "PUT",
                "/api/goals/1",
                Some(json!({ "title": "X" })),
            ),
            ("goal-delete-401", "DELETE", "/api/goals/1", None),
            ("goal-progress-401", "POST", "/api/goals/1/progress", None),
            (
                "goal-completions-401",
                "GET",
                "/api/goals/1/completions",
                None,
            ),
        ];
        for (name, method, path, body) in cases {
            let response = send(&app, request(method, path, None, body.as_ref())).await;
            expect_fixture(name, response).await;
        }
    }

    // -- unified OPTIONS + 405 (contract change: every OPTIONS answers 204 + Allow) ------

    #[tokio::test]
    async fn automatic_options_and_405_semantics() {
        let (app, _token, _dir) = make_app();

        for (path, allow) in [
            ("/api/goals", GOALS_ALLOW),
            ("/api/goals/1", GOAL_ITEM_ALLOW),
            ("/api/goals/1/progress", PROGRESS_ALLOW),
            ("/api/goals/1/completions", COMPLETIONS_ALLOW),
        ] {
            let response = send(&app, request("OPTIONS", path, None, None)).await;
            assert_eq!(response.status(), StatusCode::NO_CONTENT, "{path}");
            assert_eq!(
                response
                    .headers()
                    .get(header::ALLOW)
                    .and_then(|value| value.to_str().ok()),
                Some(allow),
                "{path}"
            );
            assert_eq!(body_value(response).await, Value::Null, "{path}");
        }

        // A non-<int> id never matches the rule, even for OPTIONS.
        let response = send(&app, request("OPTIONS", "/api/goals/abc", None, None)).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            body_value(response).await,
            json!({ "error": "Resource not found" })
        );

        // Unregistered methods on registered rules → 405 with Allow and the
        // JSON envelope (contract change).
        for (method, path) in [
            ("GET", "/api/goals/1/progress"),
            ("POST", "/api/goals/1/completions"),
            ("PUT", "/api/goals"),
        ] {
            let response = send(&app, request(method, path, None, None)).await;
            assert_eq!(
                response.status(),
                StatusCode::METHOD_NOT_ALLOWED,
                "{method} {path}"
            );
            assert!(
                response.headers().contains_key(header::ALLOW),
                "{method} {path}: Allow header"
            );
            assert_eq!(
                body_value(response).await,
                json!({ "error": "Method not allowed" }),
                "{method} {path}: body"
            );
        }
    }

    // -- helper unit checks ----------------------------------------------------

    #[test]
    fn validate_completion_date_ports_python_shapes() {
        assert_eq!(
            validate_completion_date(&json!("2020-01-05")).unwrap(),
            "2020-01-05"
        );
        // Legacy M/D/YYYY normalizes to ISO.
        assert_eq!(
            validate_completion_date(&json!("1/5/2020")).unwrap(),
            "2020-01-05"
        );
        // Tomorrow is inside the one-day slack.
        let tomorrow = today() + chrono::Duration::days(1);
        assert!(validate_completion_date(&json!(iso(tomorrow))).is_ok());
        for bad in [json!("not-a-date"), json!("2020-13-40"), json!(20200105)] {
            assert_eq!(
                validate_completion_date(&bad).unwrap_err(),
                "date must be YYYY-MM-DD or M/D/YYYY",
                "{bad}"
            );
        }
        let far = today() + chrono::Duration::days(2);
        assert_eq!(
            validate_completion_date(&json!(iso(far))).unwrap_err(),
            "date cannot be in the future"
        );
    }

    #[test]
    fn python_int_coercions() {
        assert_eq!(python_int(&json!(3)).unwrap(), 3);
        assert_eq!(python_int(&json!(3.9)).unwrap(), 3);
        assert_eq!(python_int(&json!(true)).unwrap(), 1);
        assert_eq!(python_int(&json!(" 4 ")).unwrap(), 4);
        assert!(python_int(&json!("abc")).is_err());
        assert!(python_int(&json!("3.5")).is_err());
        assert!(python_int(&json!([1])).is_err());
    }

    #[test]
    fn py_truthiness() {
        for falsy in [
            json!(null),
            json!(false),
            json!(0),
            json!(0.0),
            json!(""),
            json!([]),
            json!({}),
        ] {
            assert!(!is_py_truthy(&falsy), "{falsy}");
        }
        for truthy in [
            json!(true),
            json!(1),
            json!("x"),
            json!([0]),
            json!({"a": 1}),
        ] {
            assert!(is_py_truthy(&truthy), "{truthy}");
        }
    }
}
