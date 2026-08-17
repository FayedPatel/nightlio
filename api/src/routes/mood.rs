//! Mood + statistics route family — port of `api/routes/mood_routes.py`
//! with the service-layer logic from `api/services/mood_service.py` inlined
//! into the handlers (the Rust port has no separate service layer).
//!
//! Golden fixtures: `contract/fixtures/mood/*.json`. Behavioral quirks that
//! are REQUIRED, not accidental:
//!
//! - `POST /mood` checks `not all([mood, date, content])` with Python
//!   truthiness, so a falsy mood (`0`) or empty content string hits
//!   `Missing required fields` BEFORE the 1..=5 range validator.
//! - The `time` field is written verbatim into `created_at` (create and
//!   update both).
//! - `GET /moods?start_date=&end_date=` filters with a raw-string SQL
//!   `BETWEEN` on the stored date column (`contract/DECISIONS.md` #4).
//! - contract change (owner-approved, DECISIONS.md "Post-cutover
//!   cleanup candidates" item #4): POST/PUT normalize a `%m/%d/%Y` date to
//!   ISO before storage (ISO passes through verbatim), so the raw-string
//!   BETWEEN and the lexicographic MIN/MAX in statistics are
//!   chronologically correct for API-written rows. The SQL itself stays
//!   verbatim.
//! - PUT returns the entry WITH a `selections` array; GET by id returns it
//!   WITHOUT one. DELETE returns 200 with a JSON body, never 204.
//! - contract change (owner-approved status-code consistency fix,
//!   supersedes DECISIONS.md #5): `GET /mood/{id}/selections` 404s with
//!   `GET /mood/{id}`'s exact body (`Entry not found`) when the entry does
//!   not exist or belongs to another user, instead of Flask's recorded
//!   200 `[]`. An entry that EXISTS but whose selections were cascaded
//!   away by a group delete still returns 200 `[]`
//!   (fixture `groups/get-mood-selections__after-group-delete.json`).
//! - contract change (owner-approved, supersedes DECISIONS.md #9):
//!   `GET /statistics` is a pure read — it no longer increments the
//!   server-side `stats_views` metric. The `data_lover` achievement is fed
//!   by the explicit `POST /statistics/view`, which counts at most one
//!   view per calendar day and returns `{"counted": bool}`.
//! - Path ids use [`FlaskPath`], so negative/non-integer/overflow ids get
//!   the routing-level JSON 404 before auth runs (overflow is the accepted
//!   drift from Flask's 500, DECISIONS.md #1).
//!
//! Ungraded drift (no fixture exists): Flask turns a malformed JSON body on
//! PUT into a 500 (`request.json` raises inside the generic handler); this
//! port treats an unparseable/missing body as `{}`, which yields the clean
//! 400 (`No update fields provided`). Error-message text on 500s is never
//! matched, per the contract.

use std::collections::HashMap;

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Datelike;
use serde_json::{Map, Value, json};

use super::{FlaskInt, FlaskPath, automatic_options};
use crate::auth::extract::AuthUser;
use crate::db::common::DatabaseError;
use crate::db::{achievements, activity, moods, stats};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// Sanity bounds for statistics query params (`mood_routes.py`).
const MIN_STATS_YEAR: i64 = 1970;
const MAX_STATS_YEAR: i64 = 2100;

/// `Allow` value for the GET-only rules (matches the recorded automatic-
/// OPTIONS fixtures' ordering for GET rules).
const GET_ALLOW: &str = "HEAD, GET, OPTIONS";

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/mood",
            post(create_mood_entry).merge(automatic_options("OPTIONS, POST")),
        )
        .route(
            "/moods",
            get(get_mood_entries).merge(automatic_options(GET_ALLOW)),
        )
        .route(
            "/mood/{entry_id}",
            get(get_mood_entry)
                .put(update_mood_entry)
                .delete(delete_mood_entry)
                .merge(automatic_options("HEAD, GET, OPTIONS, PUT, DELETE")),
        )
        .route(
            "/mood/{entry_id}/selections",
            get(get_entry_selections).merge(automatic_options(GET_ALLOW)),
        )
        .route(
            "/statistics",
            get(get_mood_statistics).merge(automatic_options(GET_ALLOW)),
        )
        .route(
            "/statistics/view",
            post(record_statistics_view).merge(automatic_options("OPTIONS, POST")),
        )
        .route(
            "/statistics/extended",
            get(get_extended_statistics).merge(automatic_options(GET_ALLOW)),
        )
        .route(
            "/statistics/heatmap",
            get(get_statistics_heatmap).merge(automatic_options(GET_ALLOW)),
        )
        .route(
            "/statistics/digest",
            get(get_statistics_digest).merge(automatic_options(GET_ALLOW)),
        )
        .route(
            "/streak",
            get(get_current_streak).merge(automatic_options(GET_ALLOW)),
        )
}

// ---------------------------------------------------------------------------
// Shared plumbing
// ---------------------------------------------------------------------------

/// Run a blocking data-layer closure on the pool via `spawn_blocking`.
async fn with_db<T, F>(state: &AppState, f: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce(&rusqlite::Connection) -> ApiResult<T> + Send + 'static,
{
    let pool = state.pool.clone();
    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        f(&conn)
    })
    .await
    .map_err(|exc| ApiError::Internal(anyhow::anyhow!("blocking task failed: {exc}")))?
}

/// Data-layer failure → 500 (the Flask generic `except Exception` branch;
/// only the status is parity-relevant, the Python message text is not).
fn db_err(error: DatabaseError) -> ApiError {
    match error {
        DatabaseError::Sqlite(inner) => ApiError::Database(inner),
        DatabaseError::Pool(inner) => ApiError::Pool(inner),
        DatabaseError::Message(message) => ApiError::Internal(anyhow::anyhow!(message)),
    }
}

/// Data-layer failure where `Message` is a ported Python `ValueError`
/// (e.g. `month must be between 1 and 12` from the digest) → 400.
fn value_err(error: DatabaseError) -> ApiError {
    match error {
        DatabaseError::Message(message) => ApiError::Validation(message),
        other => db_err(other),
    }
}

/// `request.get_json(silent=True) or {}`: only parse when the Content-Type
/// is JSON (Flask's mimetype check); any failure or non-object collapses to
/// the empty dict.
fn parse_json_body(headers: &HeaderMap, body: &Bytes) -> Map<String, Value> {
    let is_json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(|raw| {
            let mime = raw
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            mime == "application/json" || mime.ends_with("+json")
        })
        .unwrap_or(false);
    if !is_json {
        return Map::new();
    }
    match serde_json::from_slice::<Value>(body) {
        Ok(Value::Object(map)) => map,
        _ => Map::new(),
    }
}

/// Python truthiness for JSON values (`not all([...])`).
fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_none_or(|float| float != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(map) => !map.is_empty(),
    }
}

/// Python `int(x)` over a JSON value: bools coerce, floats truncate toward
/// zero, strings parse (with surrounding whitespace stripped); anything
/// else fails.
fn python_int(value: &Value) -> Option<i64> {
    match value {
        Value::Bool(flag) => Some(i64::from(*flag)),
        Value::Number(number) => number
            .as_i64()
            .or_else(|| number.as_f64().map(|float| float.trunc() as i64)),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// Python `str(x)` — only the string/number cases matter on the wire; the
/// rest exist so odd payloads degrade the same way (into validation 400s).
fn python_str(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Bool(true) => "True".to_string(),
        Value::Bool(false) => "False".to_string(),
        Value::Null => "None".to_string(),
        other => other.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Validators (api/utils/validators.py + mood_routes helpers)
// ---------------------------------------------------------------------------

/// CPython `datetime.strptime` equivalent for one entry-date format
/// (`validate_entry_date` tries them in `ENTRY_DATE_FORMATS` order:
/// `%Y-%m-%d` first, then `%m/%d/%Y`): `%Y` is exactly four digits,
/// `%m`/`%d` are 1-2 digits, and impossible calendar dates fail.
fn parse_date_parts(value: &str, sep: char, year_first: bool) -> Option<chrono::NaiveDate> {
    let mut parts = value.split(sep);
    let (a, b, c) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    let (year_str, month_str, day_str) = if year_first { (a, b, c) } else { (c, a, b) };
    if year_str.len() != 4 || !year_str.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let field = |text: &str| -> Option<u32> {
        if text.is_empty() || text.len() > 2 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        text.parse().ok()
    };
    let (month, day) = (field(month_str)?, field(day_str)?);
    let year: i32 = year_str.parse().ok()?;
    chrono::NaiveDate::from_ymd_opt(year, month, day)
}

/// Port of `validate_entry_date`, plus the contract change: a
/// `%m/%d/%Y` date is normalized to ISO `YYYY-MM-DD` before storage, while
/// an ISO input passes through verbatim (including unpadded `2025-8-1`).
/// Anything else still 400s with the exact Flask message. One day of
/// future slack.
fn validate_entry_date(raw: &Value) -> ApiResult<String> {
    let value = python_str(raw);
    let (parsed, canonical) = if let Some(parsed) = parse_date_parts(&value, '-', true) {
        (parsed, value.clone())
    } else if let Some(parsed) = parse_date_parts(&value, '/', false) {
        (parsed, parsed.format("%Y-%m-%d").to_string())
    } else {
        return Err(ApiError::validation("date must be YYYY-MM-DD or M/D/YYYY"));
    };
    let tomorrow = chrono::Local::now().date_naive() + chrono::Days::new(1);
    if parsed > tomorrow {
        return Err(ApiError::validation("date cannot be in the future"));
    }
    Ok(canonical)
}

/// Port of `_normalise_selected_options`.
fn normalise_selected_options(raw: &Value, allow_none: bool) -> ApiResult<Option<Vec<i64>>> {
    if raw.is_null() {
        return Ok(if allow_none { None } else { Some(Vec::new()) });
    }
    let Value::Array(items) = raw else {
        return Err(ApiError::validation("selected_options must be an array"));
    };
    items
        .iter()
        .map(|item| {
            python_int(item)
                .ok_or_else(|| ApiError::validation("selected_options must contain integers"))
        })
        .collect::<ApiResult<Vec<i64>>>()
        .map(Some)
}

/// Port of `_parse_int_param`: optional integer query param with range
/// validation and the exact Flask error strings.
fn parse_int_param(
    name: &str,
    raw: Option<&String>,
    default: i64,
    low: i64,
    high: i64,
) -> ApiResult<i64> {
    let Some(raw) = raw else { return Ok(default) };
    let value: i64 = raw
        .trim()
        .parse()
        .map_err(|_| ApiError::validation(format!("{name} must be an integer")))?;
    if !(low..=high).contains(&value) {
        return Err(ApiError::validation(format!(
            "{name} must be between {low} and {high}"
        )));
    }
    Ok(value)
}

// ---------------------------------------------------------------------------
// Entry CRUD
// ---------------------------------------------------------------------------

/// `POST /mood` — create an entry, log activity, check achievements.
async fn create_mood_entry(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    let data = parse_json_body(&headers, &body);
    let mood = data.get("mood").cloned().unwrap_or(Value::Null);
    let date = data.get("date").cloned().unwrap_or(Value::Null);
    let content = data.get("content").cloned().unwrap_or(Value::Null);
    let time = data.get("time").cloned().unwrap_or(Value::Null);
    // Python default: `data.get("selected_options", [])`.
    let selected_raw = data
        .get("selected_options")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));

    // Truthiness check FIRST: mood 0 / empty content land here, never in
    // the range validator (fixture mood_post_400_missing_fields).
    if !(is_truthy(&mood) && is_truthy(&date) && is_truthy(&content)) {
        return Err(ApiError::validation("Missing required fields"));
    }
    let mood_value =
        python_int(&mood).ok_or_else(|| ApiError::validation("Mood must be an integer"))?;
    let date_value = validate_entry_date(&date)?;
    let content_value = python_str(&content);
    // `str(time) if time else None` — truthiness, so "" becomes None.
    let time_value = if is_truthy(&time) {
        Some(python_str(&time))
    } else {
        None
    };
    let selected_options = normalise_selected_options(&selected_raw, false)?.unwrap_or_default();

    // MoodService.create_mood_entry validation.
    if !(1..=5).contains(&mood_value) {
        return Err(ApiError::validation("Mood must be between 1 and 5"));
    }
    if content_value.trim().is_empty() {
        return Err(ApiError::validation("Content cannot be empty"));
    }

    let user_id = user.user_id;
    let (entry_id, new_achievements) = with_db(&state, move |conn| {
        let entry_id = moods::add_mood_entry(
            conn,
            user_id,
            &date_value,
            mood_value,
            &content_value,
            time_value.as_deref(),
            Some(&selected_options),
        )
        .map_err(db_err)?;
        // Best-effort activity writes must never break the mutation.
        let _ = activity::add_activity(
            conn,
            user_id,
            "entry_created",
            Some(&json!({ "entry_id": entry_id, "date": date_value })),
        );
        let new_achievements = achievements::check_achievements(conn, user_id).map_err(db_err)?;
        for achievement_type in &new_achievements {
            let _ = activity::add_activity(
                conn,
                user_id,
                "achievement_unlocked",
                Some(&json!({ "achievement_type": achievement_type })),
            );
        }
        Ok((entry_id, new_achievements))
    })
    .await?;

    // contract change: `new_achievements` carries full metadata objects
    // (the same shape as `POST /achievements/check`) instead of bare
    // achievement_type strings. Activity logging above keeps the raw strings.
    let new_achievements = crate::routes::achievements::new_achievement_objects(&new_achievements);

    Ok((
        StatusCode::CREATED,
        Json(json!({
            "status": "success",
            "entry_id": entry_id,
            "new_achievements": new_achievements,
            "message": "Mood entry created successfully",
        })),
    )
        .into_response())
}

/// `GET /moods` — bare array; optional raw-string date-range filter.
async fn get_mood_entries(
    State(state): State<AppState>,
    user: AuthUser,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<Vec<moods::MoodEntryRow>>> {
    let start_date = params.get("start_date").cloned();
    let end_date = params.get("end_date").cloned();
    let user_id = user.user_id;
    let entries = with_db(&state, move |conn| {
        // Python: `if start_date and end_date` — truthiness, so empty
        // strings fall through to the full list.
        match (start_date.as_deref(), end_date.as_deref()) {
            (Some(start), Some(end)) if !start.is_empty() && !end.is_empty() => {
                moods::get_mood_entries_by_date_range(conn, user_id, start, end).map_err(db_err)
            }
            _ => moods::get_all_mood_entries(conn, user_id).map_err(db_err),
        }
    })
    .await?;
    Ok(Json(entries))
}

/// `GET /mood/{id}` — plain entry object, no `selections` key.
async fn get_mood_entry(
    FlaskPath(FlaskInt(entry_id)): FlaskPath<FlaskInt>,
    State(state): State<AppState>,
    user: AuthUser,
) -> ApiResult<Json<moods::MoodEntryRow>> {
    let user_id = user.user_id;
    let entry = with_db(&state, move |conn| {
        moods::get_mood_entry_by_id(conn, user_id, entry_id).map_err(db_err)
    })
    .await?
    .ok_or_else(|| ApiError::NotFound("Entry not found".to_string()))?;
    Ok(Json(entry))
}

/// `PUT /mood/{id}` — partial update; response embeds the updated entry
/// WITH its `selections` array.
async fn update_mood_entry(
    FlaskPath(FlaskInt(entry_id)): FlaskPath<FlaskInt>,
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<Value>> {
    let data = parse_json_body(&headers, &body);
    let mood = data.get("mood").cloned().unwrap_or(Value::Null);
    let content = data.get("content").cloned().unwrap_or(Value::Null);
    let date = data.get("date").cloned().unwrap_or(Value::Null);
    let time = data.get("time").cloned().unwrap_or(Value::Null);

    // `selected_options` is special-cased on key PRESENCE: an explicit
    // JSON null means "clear the selections" ([]), absence means "leave
    // them alone" (None).
    let has_selected_key = data.contains_key("selected_options");
    let selected_options = if has_selected_key {
        let raw = data.get("selected_options").cloned().unwrap_or(Value::Null);
        Some(normalise_selected_options(&raw, true)?.unwrap_or_default())
    } else {
        None
    };

    if mood.is_null() && content.is_null() && date.is_null() && time.is_null() && !has_selected_key
    {
        return Err(ApiError::validation("No update fields provided"));
    }

    let mood_value = if mood.is_null() {
        None
    } else {
        Some(python_int(&mood).ok_or_else(|| ApiError::validation("Mood must be an integer"))?)
    };
    let content_value = if content.is_null() {
        None
    } else {
        Some(python_str(&content))
    };
    let date_value = if date.is_null() {
        None
    } else {
        Some(validate_entry_date(&date)?)
    };
    let time_value = if is_truthy(&time) {
        Some(python_str(&time))
    } else {
        None
    };

    // MoodService.update_entry validation.
    if let Some(mood_value) = mood_value
        && !(1..=5).contains(&mood_value)
    {
        return Err(ApiError::validation("Mood must be between 1 and 5"));
    }
    if let Some(content_value) = &content_value
        && content_value.trim().is_empty()
    {
        return Err(ApiError::validation("Content cannot be empty"));
    }

    let update = moods::MoodEntryUpdate {
        mood: mood_value,
        content: content_value,
        date: date_value,
        time: time_value,
        selected_options,
    };
    let user_id = user.user_id;
    let entry = with_db(&state, move |conn| {
        if !moods::update_mood_entry(conn, user_id, entry_id, &update).map_err(db_err)? {
            return Ok(None);
        }
        let Some(entry) =
            moods::get_mood_entry_with_selections(conn, user_id, entry_id).map_err(db_err)?
        else {
            return Ok(None);
        };
        let _ = activity::add_activity(
            conn,
            user_id,
            "entry_edited",
            Some(&json!({ "entry_id": entry_id })),
        );
        Ok(Some(entry))
    })
    .await?
    .ok_or_else(|| ApiError::NotFound("Entry not found or no changes made".to_string()))?;

    Ok(Json(json!({
        "status": "success",
        "message": "Mood entry updated successfully",
        "entry": entry,
    })))
}

/// `DELETE /mood/{id}` — 200 with a JSON body, never 204/empty.
async fn delete_mood_entry(
    FlaskPath(FlaskInt(entry_id)): FlaskPath<FlaskInt>,
    State(state): State<AppState>,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    let user_id = user.user_id;
    let deleted = with_db(&state, move |conn| {
        let deleted = moods::delete_mood_entry(conn, user_id, entry_id).map_err(db_err)?;
        if deleted {
            let _ = activity::add_activity(
                conn,
                user_id,
                "entry_deleted",
                Some(&json!({ "entry_id": entry_id })),
            );
        }
        Ok(deleted)
    })
    .await?;
    if !deleted {
        return Err(ApiError::NotFound("Entry not found".to_string()));
    }
    Ok(Json(json!({
        "status": "success",
        "message": "Mood entry deleted successfully",
    })))
}

/// `GET /mood/{id}/selections` — bare array. contract change
/// (owner-approved): a nonexistent/foreign entry is a 404 with the same
/// body as `GET /mood/{id}` (`Entry not found`) instead of Flask's
/// recorded 200 `[]`. The existence check is user-scoped, so the
/// after-group-delete case (entry exists, selections cascaded away) still
/// returns 200 `[]`.
async fn get_entry_selections(
    FlaskPath(FlaskInt(entry_id)): FlaskPath<FlaskInt>,
    State(state): State<AppState>,
    user: AuthUser,
) -> ApiResult<Json<Vec<moods::EntrySelectionRow>>> {
    let user_id = user.user_id;
    let selections = with_db(&state, move |conn| {
        if moods::get_mood_entry_by_id(conn, user_id, entry_id)
            .map_err(db_err)?
            .is_none()
        {
            return Err(ApiError::NotFound("Entry not found".to_string()));
        }
        moods::get_entry_selections(conn, entry_id, Some(user_id)).map_err(db_err)
    })
    .await?;
    Ok(Json(selections))
}

// ---------------------------------------------------------------------------
// Statistics + streak
// ---------------------------------------------------------------------------

/// `GET /statistics` — bundles statistics, mood distribution (string keys,
/// only logged moods), and the current streak. contract change: a
/// pure read — the old `stats_views` increment side effect moved to
/// `POST /statistics/view`.
async fn get_mood_statistics(
    State(state): State<AppState>,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    let user_id = user.user_id;
    let body = with_db(&state, move |conn| {
        let statistics = achievements::get_mood_statistics(conn, user_id).map_err(db_err)?;
        let mood_distribution = achievements::get_mood_counts(conn, user_id).map_err(db_err)?;
        let current_streak = achievements::get_current_streak(conn, user_id);
        Ok(json!({
            "statistics": statistics,
            "mood_distribution": mood_distribution,
            "current_streak": current_streak,
        }))
    })
    .await?;
    Ok(Json(body))
}

/// `POST /statistics/view` — contract change (owner-approved): record
/// a statistics view for the `data_lover` achievement. Per-day idempotent:
/// `counted` is true only when this call incremented `stats_views` (first
/// view of the server-local day). Auth (and, for cookie callers, CSRF) is
/// enforced by the [`AuthUser`] extractor like every mutation; the rule is
/// strict-slash, so `/statistics/view/` 404s.
async fn record_statistics_view(
    State(state): State<AppState>,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    let user_id = user.user_id;
    let counted = with_db(&state, move |conn| {
        achievements::record_stats_view(conn, user_id).map_err(db_err)
    })
    .await?;
    Ok(Json(json!({ "counted": counted })))
}

/// `GET /statistics/extended` — the six extended aggregations; the digest
/// is always for the CURRENT server-local year/month.
async fn get_extended_statistics(
    State(state): State<AppState>,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    let today = chrono::Local::now().date_naive();
    let (year, month) = (i64::from(today.year()), i64::from(today.month()));
    let user_id = user.user_id;
    let body = with_db(&state, move |conn| {
        Ok(json!({
            "rolling_averages": stats::rolling_averages(conn, user_id).map_err(db_err)?,
            "weekday_averages": stats::weekday_averages(conn, user_id).map_err(db_err)?,
            "mood_volatility":
                stats::mood_volatility(conn, user_id, stats::DEFAULT_VOLATILITY_WINDOW_DAYS)
                    .map_err(db_err)?,
            "tag_correlations": stats::tag_correlations(conn, user_id).map_err(db_err)?,
            "goal_correlations": stats::goal_correlations(conn, user_id).map_err(db_err)?,
            "monthly_digest": stats::monthly_digest(conn, user_id, year, month)
                .map_err(db_err)?,
        }))
    })
    .await?;
    Ok(Json(body))
}

/// `GET /statistics/heatmap?year=` — defaults to the current local year;
/// non-integer or out-of-range years 400.
async fn get_statistics_heatmap(
    State(state): State<AppState>,
    user: AuthUser,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<stats::Heatmap>> {
    let year = parse_int_param(
        "year",
        params.get("year"),
        i64::from(chrono::Local::now().date_naive().year()),
        MIN_STATS_YEAR,
        MAX_STATS_YEAR,
    )?;
    let user_id = user.user_id;
    let heatmap = with_db(&state, move |conn| {
        stats::heatmap(conn, user_id, year).map_err(db_err)
    })
    .await?;
    Ok(Json(heatmap))
}

/// `GET /statistics/digest?year=&month=` — defaults to the current local
/// year/month; validation errors 400 with the exact Flask messages.
async fn get_statistics_digest(
    State(state): State<AppState>,
    user: AuthUser,
    Query(params): Query<HashMap<String, String>>,
) -> ApiResult<Json<stats::MonthlyDigest>> {
    let today = chrono::Local::now().date_naive();
    let year = parse_int_param(
        "year",
        params.get("year"),
        i64::from(today.year()),
        MIN_STATS_YEAR,
        MAX_STATS_YEAR,
    )?;
    let month = parse_int_param(
        "month",
        params.get("month"),
        i64::from(today.month()),
        1,
        12,
    )?;
    let user_id = user.user_id;
    let digest = with_db(&state, move |conn| {
        // The mixin's own ValueError port maps to 400, like Flask's
        // `except ValueError` in this route.
        stats::monthly_digest(conn, user_id, year, month).map_err(value_err)
    })
    .await?;
    Ok(Json(digest))
}

/// `GET /streak` — current streak plus the pluralized message.
async fn get_current_streak(
    State(state): State<AppState>,
    user: AuthUser,
) -> ApiResult<Json<Value>> {
    let user_id = user.user_id;
    let streak = with_db(&state, move |conn| {
        Ok(achievements::get_current_streak(conn, user_id))
    })
    .await?;
    let plural = if streak == 1 { "" } else { "s" };
    Ok(Json(json!({
        "current_streak": streak,
        "message": format!("Current streak: {streak} day{plural}"),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truthiness_matches_python() {
        for falsy in [
            json!(null),
            json!(false),
            json!(0),
            json!(0.0),
            json!(""),
            json!([]),
            json!({}),
        ] {
            assert!(!is_truthy(&falsy), "{falsy} should be falsy");
        }
        for truthy in [json!(true), json!(1), json!(-1), json!("x"), json!([0])] {
            assert!(is_truthy(&truthy), "{truthy} should be truthy");
        }
    }

    #[test]
    fn python_int_conversions() {
        assert_eq!(python_int(&json!(4)), Some(4));
        assert_eq!(python_int(&json!(4.7)), Some(4)); // int() truncates
        assert_eq!(python_int(&json!(-4.7)), Some(-4));
        assert_eq!(python_int(&json!(true)), Some(1));
        assert_eq!(python_int(&json!(" 5 ")), Some(5));
        assert_eq!(python_int(&json!("+5")), Some(5));
        assert_eq!(python_int(&json!("4.0")), None);
        assert_eq!(python_int(&json!("abc")), None);
        assert_eq!(python_int(&json!([1])), None);
        assert_eq!(python_int(&json!(null)), None);
    }

    #[test]
    fn entry_date_validation_matches_python_strptime() {
        // Both formats, including 1-2 digit month/day like strptime.
        for ok in ["2025-08-01", "2025-8-1", "8/2/2025", "08/02/2025"] {
            assert!(validate_entry_date(&json!(ok)).is_ok(), "{ok}");
        }
        // contract change: US-format input normalizes to ISO before
        // storage; ISO input (even unpadded) passes through verbatim.
        assert_eq!(
            validate_entry_date(&json!("8/2/2025")).unwrap(),
            "2025-08-02"
        );
        assert_eq!(
            validate_entry_date(&json!("08/02/2025")).unwrap(),
            "2025-08-02"
        );
        assert_eq!(
            validate_entry_date(&json!("12/31/2024")).unwrap(),
            "2024-12-31"
        );
        assert_eq!(
            validate_entry_date(&json!("2025-08-01")).unwrap(),
            "2025-08-01"
        );
        assert_eq!(validate_entry_date(&json!("2025-8-1")).unwrap(), "2025-8-1");
        for bad in ["01-08-2025", "2025/08/01", "not-a-date", "8-2-25", ""] {
            let err = validate_entry_date(&json!(bad)).unwrap_err();
            assert_eq!(
                err.to_string(),
                "date must be YYYY-MM-DD or M/D/YYYY",
                "{bad}"
            );
        }
        // Future beyond today+1 → the other message.
        let future = chrono::Local::now().date_naive() + chrono::Days::new(30);
        let err = validate_entry_date(&json!(future.format("%Y-%m-%d").to_string())).unwrap_err();
        assert_eq!(err.to_string(), "date cannot be in the future");
        // Tomorrow is inside the one-day slack.
        let tomorrow = chrono::Local::now().date_naive() + chrono::Days::new(1);
        assert!(validate_entry_date(&json!(tomorrow.format("%Y-%m-%d").to_string())).is_ok());
    }

    #[test]
    fn selected_options_normalisation() {
        assert_eq!(
            normalise_selected_options(&json!([1, "2", 3.9]), false).unwrap(),
            Some(vec![1, 2, 3])
        );
        assert_eq!(
            normalise_selected_options(&json!(null), false).unwrap(),
            Some(vec![])
        );
        assert_eq!(
            normalise_selected_options(&json!(null), true).unwrap(),
            None
        );
        assert_eq!(
            normalise_selected_options(&json!("nope"), false)
                .unwrap_err()
                .to_string(),
            "selected_options must be an array"
        );
        assert_eq!(
            normalise_selected_options(&json!(["x"]), false)
                .unwrap_err()
                .to_string(),
            "selected_options must contain integers"
        );
    }

    #[test]
    fn int_param_parsing() {
        let raw = |s: &str| Some(s.to_string());
        assert_eq!(
            parse_int_param("year", None, 2025, 1970, 2100).unwrap(),
            2025
        );
        assert_eq!(
            parse_int_param("year", raw("1999").as_ref(), 2025, 1970, 2100).unwrap(),
            1999
        );
        assert_eq!(
            parse_int_param("year", raw("abc").as_ref(), 2025, 1970, 2100)
                .unwrap_err()
                .to_string(),
            "year must be an integer"
        );
        assert_eq!(
            parse_int_param("year", raw("1969").as_ref(), 2025, 1970, 2100)
                .unwrap_err()
                .to_string(),
            "year must be between 1970 and 2100"
        );
        assert_eq!(
            parse_int_param("month", raw("13").as_ref(), 8, 1, 12)
                .unwrap_err()
                .to_string(),
            "month must be between 1 and 12"
        );
    }
}
