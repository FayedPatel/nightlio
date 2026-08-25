//! Data route family (v0.6.0): versioned JSON export/import.
//!
//! - `GET /export/data` (auth) returns the portable v1 envelope
//!   `{schema_version, exported_at, app_version, data: {entries, goals}}`
//!   as the JSON response body — the frontend builds the downloadable file.
//!   A pure read; empty accounts still get the full envelope with both
//!   arrays present and empty.
//! - `POST /import/data` (auth + cookie-CSRF via [`AuthUser`]) accepts the
//!   same envelope as plain `application/json` (no multipart). The body is
//!   parsed to a generic [`Value`] and dispatched on `schema_version`
//!   FIRST: missing/non-integer → 400, anything but 1 → 400
//!   `Unsupported schema_version: N (supported: 1)`. Unknown extra keys
//!   inside a supported version are ignored (future additive fields import
//!   fine). Any invalid row rejects the WHOLE file with a row-indexed 400
//!   (`entries[1]: mood must be between 1 and 5`) before the transaction
//!   opens — nothing is imported. Bodies over the 8 MiB in-handler cap →
//!   413 (the `MAX_PDF_CONTENT_SIZE` pattern); the transport-level
//!   `DefaultBodyLimit` is 16 MiB.
//!
//! Paths are relative to `/api` (this router is nested there). This family
//! is Rust-native — no Flask recording exists; `contract/fixtures/data/`
//! is hand-authored spec (DECISIONS.md 2026-08-24).

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};

use super::automatic_options;
use crate::auth::extract::AuthUser;
use crate::db;
use crate::db::common::MoodValue;
use crate::db::data::{ExportData, ExportEntry, ExportGoal, ExportSelection};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// `Allow` for the GET-only export rule (fixture
/// `export_data_options_204.json`).
const EXPORT_DATA_ALLOW: &str = "HEAD, GET, OPTIONS";

/// `Allow` for the POST-only import rule (fixture
/// `import_data_options_204.json` — same value as the other POST-only
/// mutations).
const IMPORT_DATA_ALLOW: &str = "OPTIONS, POST";

/// In-handler import cap, measured on raw body bytes: 8 MiB → 413
/// (code-derived, no fixture; `contract/openapi-parts/data.yaml`).
pub const MAX_IMPORT_SIZE: usize = 8 * 1024 * 1024;

/// Transport-level body limit — comfortably above the in-handler cap so
/// the cap's own 413 body is always the one on the wire.
const TRANSPORT_BODY_LIMIT: usize = 16 * 1024 * 1024;

/// The only export schema version this server reads and writes. The
/// importer accepts every version ≤ current, so v1 files import forever.
pub const SUPPORTED_SCHEMA_VERSION: i64 = 1;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/export/data",
            get(export_data).merge(automatic_options(EXPORT_DATA_ALLOW)),
        )
        .route(
            "/import/data",
            post(import_data)
                .merge(automatic_options(IMPORT_DATA_ALLOW))
                .layer(DefaultBodyLimit::max(TRANSPORT_BODY_LIMIT)),
        )
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

/// `GET /api/export/data` — the v1 envelope. `exported_at` is SQLite-style
/// UTC text; `app_version` is the live crate version (the
/// `config_get_200.json` substitution convention).
async fn export_data(State(state): State<AppState>, user: AuthUser) -> ApiResult<Json<Value>> {
    let data = db::store::data::export_data(&state.db, user.user_id).await?;
    Ok(Json(json!({
        "schema_version": SUPPORTED_SCHEMA_VERSION,
        "exported_at": chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        "app_version": env!("CARGO_PKG_VERSION"),
        "data": data,
    })))
}

// ---------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------

/// `POST /api/import/data`. Auth (and the cookie-auth CSRF predicate) is
/// enforced by the [`AuthUser`] extractor before this body runs — 401/403
/// precede any body validation.
async fn import_data(
    State(state): State<AppState>,
    user: AuthUser,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Cap first: the size check needs no parse.
    if body.len() > MAX_IMPORT_SIZE {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({
                "error": format!("Import file is too large (max {MAX_IMPORT_SIZE} bytes)")
            })),
        )
            .into_response();
    }

    // Strict JSON (never get_json(silent=True)-lenient): a non-JSON
    // Content-Type or an unparsable body takes the app-level 400 handler.
    if !is_json_content_type(&headers) {
        return bad_request();
    }
    let Ok(value) = serde_json::from_slice::<Value>(&body) else {
        return bad_request();
    };

    // Version dispatch before anything else.
    let file = match value.get("schema_version").and_then(Value::as_i64) {
        None => return ApiError::validation("schema_version is required").into_response(),
        Some(SUPPORTED_SCHEMA_VERSION) => match parse_v1(&value) {
            Ok(file) => file,
            Err(message) => return ApiError::Validation(message).into_response(),
        },
        Some(other) => {
            return ApiError::validation(format!(
                "Unsupported schema_version: {other} (supported: 1)"
            ))
            .into_response();
        }
    };

    match db::store::data::import_data(&state.db, user.user_id, file).await {
        Ok(counts) => Json(json!({
            "status": "success",
            "entries": counts.entries,
            "goals": counts.goals,
        }))
        .into_response(),
        Err(exc) => exc.into_response(),
    }
}

// ---------------------------------------------------------------------------
// v1 parsing/validation
// ---------------------------------------------------------------------------

/// Parse a v1 envelope out of the generic JSON value. Unknown keys are
/// ignored everywhere (additive evolution); any invalid row fails the whole
/// file with a row-indexed message (the transaction never opens).
fn parse_v1(value: &Value) -> Result<ExportData, String> {
    let data = value
        .get("data")
        .and_then(Value::as_object)
        .ok_or_else(|| "data must be an object".to_string())?;
    let entries_raw = data
        .get("entries")
        .and_then(Value::as_array)
        .ok_or_else(|| "data.entries must be an array".to_string())?;
    let goals_raw = data
        .get("goals")
        .and_then(Value::as_array)
        .ok_or_else(|| "data.goals must be an array".to_string())?;

    let entries = entries_raw
        .iter()
        .enumerate()
        .map(|(index, entry)| parse_entry(index, entry))
        .collect::<Result<Vec<_>, String>>()?;
    let goals = goals_raw
        .iter()
        .enumerate()
        .map(|(index, goal)| parse_goal(index, goal))
        .collect::<Result<Vec<_>, String>>()?;
    Ok(ExportData { entries, goals })
}

/// A key that is absent or `null` degrades to `None` (the data layer's
/// `COALESCE` fills timestamps in); present-but-not-a-string is an error.
fn optional_string(
    object: &serde_json::Map<String, Value>,
    key: &str,
) -> Result<Option<String>, ()> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(()),
    }
}

/// Absent/`null` integers degrade to `None`; anything else non-integer is
/// an error.
fn optional_int(object: &serde_json::Map<String, Value>, key: &str) -> Result<Option<i64>, ()> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(other) => other.as_i64().map(Some).ok_or(()),
    }
}

fn parse_entry(index: usize, value: &Value) -> Result<ExportEntry, String> {
    let fail = |message: &str| format!("entries[{index}]: {message}");
    let object = value.as_object().ok_or_else(|| fail("must be an object"))?;

    let date = object
        .get("date")
        .and_then(Value::as_str)
        .filter(|date| !date.trim().is_empty())
        .ok_or_else(|| fail("date must be a non-empty string"))?
        .to_string();
    let mood = object
        .get("mood")
        .and_then(Value::as_i64)
        .filter(|mood| (1..=5).contains(mood))
        .ok_or_else(|| fail("mood must be between 1 and 5"))?;
    let content = object
        .get("content")
        .and_then(Value::as_str)
        .filter(|content| !content.trim().is_empty())
        .ok_or_else(|| fail("content must be a non-empty string"))?
        .to_string();
    let created_at =
        optional_string(object, "created_at").map_err(|()| fail("created_at must be a string"))?;
    let updated_at =
        optional_string(object, "updated_at").map_err(|()| fail("updated_at must be a string"))?;

    let selections = match object.get("selections") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .map(|(sel_index, item)| parse_selection(index, sel_index, item))
            .collect::<Result<Vec<_>, String>>()?,
        Some(_) => return Err(fail("selections must be an array")),
    };

    Ok(ExportEntry {
        date,
        mood: MoodValue::Int(mood),
        content,
        created_at,
        updated_at,
        selections,
    })
}

fn parse_selection(
    entry_index: usize,
    sel_index: usize,
    value: &Value,
) -> Result<ExportSelection, String> {
    let fail = |message: &str| format!("entries[{entry_index}]: selections[{sel_index}] {message}");
    let object = value.as_object().ok_or_else(|| fail("must be an object"))?;
    let group_name = object
        .get("group_name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| fail("group_name must be a non-empty string"))?
        .to_string();
    let option_name = object
        .get("option_name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| fail("option_name must be a non-empty string"))?
        .to_string();
    Ok(ExportSelection {
        group_name,
        option_name,
    })
}

fn parse_goal(index: usize, value: &Value) -> Result<ExportGoal, String> {
    let fail = |message: &str| format!("goals[{index}]: {message}");
    let object = value.as_object().ok_or_else(|| fail("must be an object"))?;

    let title = object
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| !title.trim().is_empty())
        .ok_or_else(|| fail("title must be a non-empty string"))?
        .to_string();
    let description = optional_string(object, "description")
        .map_err(|()| fail("description must be a string"))?;
    let frequency_per_week = object
        .get("frequency_per_week")
        .and_then(Value::as_i64)
        .filter(|freq| (1..=7).contains(freq))
        .ok_or_else(|| fail("frequency_per_week must be between 1 and 7"))?;
    let completed = optional_int(object, "completed")
        .map_err(|()| fail("completed must be an integer"))?
        .unwrap_or(0);
    let streak = optional_int(object, "streak")
        .map_err(|()| fail("streak must be an integer"))?
        .unwrap_or(0);
    let period_start = optional_string(object, "period_start")
        .map_err(|()| fail("period_start must be a string"))?;
    let last_completed_date = optional_string(object, "last_completed_date")
        .map_err(|()| fail("last_completed_date must be a string"))?;
    let created_at =
        optional_string(object, "created_at").map_err(|()| fail("created_at must be a string"))?;
    let updated_at =
        optional_string(object, "updated_at").map_err(|()| fail("updated_at must be a string"))?;

    let completions = match object.get("completions") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| fail("completions must be an array of date strings"))
            })
            .collect::<Result<Vec<_>, String>>()?,
        Some(_) => return Err(fail("completions must be an array of date strings")),
    };

    Ok(ExportGoal {
        title,
        description,
        frequency_per_week: Some(frequency_per_week),
        completed: Some(completed),
        streak: Some(streak),
        period_start,
        last_completed_date,
        created_at,
        updated_at,
        completions,
    })
}

// ---------------------------------------------------------------------------
// Shared helpers (the extras.rs conventions)
// ---------------------------------------------------------------------------

/// Flask `request.is_json`: mimetype `application/json` or an
/// `application/*+json` suffix type.
fn is_json_content_type(headers: &HeaderMap) -> bool {
    let Some(content_type) = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let mimetype = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    mimetype == "application/json"
        || (mimetype.starts_with("application/") && mimetype.ends_with("+json"))
}

/// App-level Flask 400 handler body (`{"error": "Bad request"}`).
fn bad_request() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": "Bad request" })),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Tests — pure parser semantics (the wire contract is graded against the
// fixtures in api/tests/data_routes.rs)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_envelope() -> Value {
        json!({
            "schema_version": 1,
            "data": { "entries": [], "goals": [] }
        })
    }

    #[test]
    fn parse_v1_accepts_the_empty_envelope_and_ignores_unknown_keys() {
        let mut envelope = minimal_envelope();
        envelope["future_field"] = json!({"anything": true});
        envelope["data"]["future_array"] = json!([1, 2, 3]);
        let parsed = parse_v1(&envelope).expect("unknown keys must be ignored");
        assert!(parsed.entries.is_empty());
        assert!(parsed.goals.is_empty());
    }

    #[test]
    fn parse_v1_requires_the_data_arrays() {
        assert_eq!(
            parse_v1(&json!({"schema_version": 1})).unwrap_err(),
            "data must be an object"
        );
        assert_eq!(
            parse_v1(&json!({"schema_version": 1, "data": {"goals": []}})).unwrap_err(),
            "data.entries must be an array"
        );
        assert_eq!(
            parse_v1(&json!({"schema_version": 1, "data": {"entries": []}})).unwrap_err(),
            "data.goals must be an array"
        );
    }

    #[test]
    fn entry_rows_validate_with_row_indexed_messages() {
        let entry = |patch: Value| {
            let mut base = json!({
                "date": "2025-08-01",
                "mood": 4,
                "content": "ok",
                "selections": []
            });
            base.as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            base
        };
        // The second row fails → index 1 in the message.
        let envelope = json!({
            "schema_version": 1,
            "data": { "entries": [entry(json!({})), entry(json!({"mood": 9}))], "goals": [] }
        });
        assert_eq!(
            parse_v1(&envelope).unwrap_err(),
            "entries[1]: mood must be between 1 and 5"
        );

        for (patch, message) in [
            (
                json!({"mood": 0}),
                "entries[0]: mood must be between 1 and 5",
            ),
            (
                json!({"mood": 4.5}),
                "entries[0]: mood must be between 1 and 5",
            ),
            (
                json!({"mood": null}),
                "entries[0]: mood must be between 1 and 5",
            ),
            (
                json!({"date": 7}),
                "entries[0]: date must be a non-empty string",
            ),
            (
                json!({"content": "  "}),
                "entries[0]: content must be a non-empty string",
            ),
            (
                json!({"created_at": 12}),
                "entries[0]: created_at must be a string",
            ),
            (
                json!({"selections": "nope"}),
                "entries[0]: selections must be an array",
            ),
            (
                json!({"selections": [{"group_name": "G"}]}),
                "entries[0]: selections[0] option_name must be a non-empty string",
            ),
        ] {
            let envelope = json!({
                "schema_version": 1,
                "data": { "entries": [entry(patch)], "goals": [] }
            });
            assert_eq!(parse_v1(&envelope).unwrap_err(), message);
        }
    }

    #[test]
    fn goal_rows_validate_and_tolerate_nullable_fields() {
        let goal = json!({
            "title": "Read",
            "description": null,
            "frequency_per_week": 2,
            "completed": 0,
            "streak": 0,
            "period_start": "2025-08-04",
            "last_completed_date": null,
            "completions": ["2025-08-05"]
        });
        let envelope = json!({
            "schema_version": 1,
            "data": { "entries": [], "goals": [goal] }
        });
        let parsed = parse_v1(&envelope).expect("nullable fields import");
        assert_eq!(parsed.goals[0].description, None);
        assert_eq!(parsed.goals[0].frequency_per_week, Some(2));
        assert_eq!(parsed.goals[0].completions, ["2025-08-05"]);

        for (patch_key, patch_value, message) in [
            (
                "title",
                json!("  "),
                "goals[0]: title must be a non-empty string",
            ),
            (
                "frequency_per_week",
                json!(8),
                "goals[0]: frequency_per_week must be between 1 and 7",
            ),
            (
                "completed",
                json!("x"),
                "goals[0]: completed must be an integer",
            ),
            (
                "completions",
                json!([7]),
                "goals[0]: completions must be an array of date strings",
            ),
        ] {
            let mut bad = envelope.clone();
            bad["data"]["goals"][0][patch_key] = patch_value;
            assert_eq!(parse_v1(&bad).unwrap_err(), message);
        }
    }

    #[test]
    fn missing_timestamps_and_selections_degrade_to_defaults() {
        // A minimal hand-authored file: only the required user data.
        let envelope = json!({
            "schema_version": 1,
            "data": {
                "entries": [{"date": "2025-08-01", "mood": 3, "content": "hi"}],
                "goals": [{"title": "Walk", "frequency_per_week": 3}]
            }
        });
        let parsed = parse_v1(&envelope).expect("minimal file parses");
        assert_eq!(parsed.entries[0].created_at, None);
        assert!(parsed.entries[0].selections.is_empty());
        assert_eq!(parsed.goals[0].completed, Some(0));
        assert_eq!(parsed.goals[0].streak, Some(0));
        assert_eq!(parsed.goals[0].period_start, None);
        assert!(parsed.goals[0].completions.is_empty());
    }

    #[test]
    fn json_content_type_matches_flask_is_json() {
        let mut headers = HeaderMap::new();
        assert!(!is_json_content_type(&headers));
        headers.insert(header::CONTENT_TYPE, "text/plain".parse().unwrap());
        assert!(!is_json_content_type(&headers));
        headers.insert(
            header::CONTENT_TYPE,
            "application/json; charset=utf-8".parse().unwrap(),
        );
        assert!(is_json_content_type(&headers));
        headers.insert(
            header::CONTENT_TYPE,
            "application/problem+json".parse().unwrap(),
        );
        assert!(is_json_content_type(&headers));
    }
}
