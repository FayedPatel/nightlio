//! Groups route family — port of `api/routes/group_routes.py` plus the
//! service-layer logic from `api/services/group_service.py`, folded into the
//! handlers (the service was a thin validation shim over the DB mixin).
//!
//! Contract notes (all fixture-verified, `contract/fixtures/groups/`):
//! - `GET /groups` returns a BARE array `[{id, name, options: [{id, name}]}]`,
//!   both nesting levels ordered by name (SQLite BINARY collation).
//! - `POST /groups` has two distinct 400 strings: a falsy `name` fails the
//!   route-layer check (`Group name is required`), a whitespace-only `name`
//!   passes it and fails the service-layer strip check
//!   (`Group name cannot be empty`). Same split for option names.
//! - `POST /groups/{id}/options` on an unknown/foreign group: contract
//!   change (owner-approved status-code consistency fix,
//!   supersedes `contract/DECISIONS.md` item 6) — now a 404
//!   `Group not found` (the same resource-not-found convention as
//!   `DELETE /groups/{id}`), instead of Flask's recorded 400
//!   `Group not found for user` `ValueError` mapping. The data layer's
//!   error is untouched; the handler remaps it.
//! - Deletes return 200 with a JSON body, never 204 (the SPA client throws
//!   on any 2xx that is not `application/json`); unknown/foreign ids are
//!   JSON 404 (`Group not found` / `Option not found`).
//!   `DELETE /options/{id}` has no frontend caller but is registered.
//! - `DELETE /groups/{id}` cascades through `group_options` to
//!   `entry_selections` (schema `ON DELETE CASCADE`; `PRAGMA foreign_keys`
//!   is ON per connection).
//! - Path ids use [`FlaskPath`]`<`[`FlaskInt`]`>` and are extracted BEFORE
//!   auth, matching Flask's order (routing 404 precedes the decorator's
//!   401). Trailing-slash variants 404 via the shell fallback
//!   (`strict_slashes` default).
//! - No rule here registers OPTIONS explicitly, so every path gets Flask's
//!   automatic OPTIONS (200, empty body, `Allow`).
//!
//! Drift (not fixture-covered): a request body that is missing, non-JSON,
//! or carries a non-string `name` gets the route's 400 `... is required`
//! JSON error. Flask would 500 leaking `str(exception)` for the non-JSON
//! and truthy-non-string cases (blanket `except`); clean 400 JSON keeps the
//! SPA's error shape and leaks nothing.

use axum::extract::State;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::{Value, json};

use super::{FlaskInt, FlaskPath, automatic_options};
use crate::auth::extract::AuthUser;
use crate::db::DbPool;
use crate::db::groups::{self as db_groups, GroupsError};
use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

/// `Allow` for the `/groups` collection rule (GET + POST).
const GROUPS_ALLOW: &str = "HEAD, GET, OPTIONS, POST";
/// `Allow` for the POST-only option-creation rule.
const POST_ALLOW: &str = "POST, OPTIONS";
/// `Allow` for the DELETE-only rules.
const DELETE_ALLOW: &str = "OPTIONS, DELETE";

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/groups",
            get(get_groups)
                .post(create_group)
                .merge(automatic_options(GROUPS_ALLOW)),
        )
        .route(
            "/groups/{group_id}",
            delete(delete_group).merge(automatic_options(DELETE_ALLOW)),
        )
        .route(
            "/groups/{group_id}/options",
            post(create_group_option).merge(automatic_options(POST_ALLOW)),
        )
        .route(
            "/options/{option_id}",
            delete(delete_option).merge(automatic_options(DELETE_ALLOW)),
        )
}

/// Run a groups data-layer call on a pooled connection under
/// `spawn_blocking` (rusqlite is synchronous).
async fn with_conn<T, F>(pool: DbPool, op: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce(&rusqlite::Connection) -> Result<T, GroupsError> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        op(&conn).map_err(ApiError::from)
    })
    .await
    .map_err(|exc| ApiError::Internal(anyhow::anyhow!(exc)))?
}

/// Port of the route-layer `if not name:` falsy check on `data.get("name")`.
/// Missing/invalid body, missing key, null, or empty string → 400 with the
/// route's exact message (see module docs for the drift on non-string
/// values).
fn extract_name(
    body: &Result<Json<Value>, JsonRejection>,
    missing_message: &'static str,
) -> Result<String, ApiError> {
    let Ok(Json(data)) = body else {
        return Err(ApiError::validation(missing_message));
    };
    match data.get("name") {
        Some(Value::String(name)) if !name.is_empty() => Ok(name.clone()),
        _ => Err(ApiError::validation(missing_message)),
    }
}

/// `GET /groups` — bare array, `group_service.get_all_groups(user_id)`.
async fn get_groups(
    user: AuthUser,
    State(state): State<AppState>,
) -> ApiResult<Json<Vec<db_groups::Group>>> {
    let default_id = state.config.default_self_host_id.clone();
    let groups = with_conn(state.pool.clone(), move |conn| {
        db_groups::get_all_groups(conn, Some(user.user_id), &default_id)
    })
    .await?;
    Ok(Json(groups))
}

/// `POST /groups` — 201 `{status, group_id, message}`. Route-layer falsy
/// check, then the service's strip check, then the insert (with the
/// stripped name, exactly like `GroupService.create_group`).
async fn create_group(
    user: AuthUser,
    State(state): State<AppState>,
    body: Result<Json<Value>, JsonRejection>,
) -> ApiResult<impl IntoResponse> {
    let name = extract_name(&body, "Group name is required")?;
    let trimmed = name.trim().to_string();
    if trimmed.is_empty() {
        return Err(ApiError::validation("Group name cannot be empty"));
    }
    let default_id = state.config.default_self_host_id.clone();
    let group_id = with_conn(state.pool.clone(), move |conn| {
        db_groups::create_group(conn, &trimmed, Some(user.user_id), &default_id)
    })
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "status": "success",
            "group_id": group_id,
            "message": "Group created successfully",
        })),
    ))
}

/// `POST /groups/{id}/options` — 201 `{status, option_id, message}`.
/// contract change (owner-approved): an unknown/foreign group is a
/// 404 `Group not found` (matching `DELETE /groups/{id}`'s not-found
/// body), remapped here from the data layer's untouched
/// `Group not found for user` `ValueError` port (which Flask surfaced as
/// a 400).
async fn create_group_option(
    FlaskPath(FlaskInt(group_id)): FlaskPath<FlaskInt>,
    user: AuthUser,
    State(state): State<AppState>,
    body: Result<Json<Value>, JsonRejection>,
) -> ApiResult<impl IntoResponse> {
    let name = extract_name(&body, "Option name is required")?;
    let trimmed = name.trim().to_string();
    if trimmed.is_empty() {
        return Err(ApiError::validation("Option name cannot be empty"));
    }
    let default_id = state.config.default_self_host_id.clone();
    let option_id = with_conn(state.pool.clone(), move |conn| {
        db_groups::create_group_option(conn, group_id, &trimmed, Some(user.user_id), &default_id)
    })
    .await
    .map_err(|error| match error {
        ApiError::Validation(message) if message == "Group not found for user" => {
            ApiError::NotFound("Group not found".to_string())
        }
        other => other,
    })?;
    Ok((
        StatusCode::CREATED,
        Json(json!({
            "status": "success",
            "option_id": option_id,
            "message": "Option created successfully",
        })),
    ))
}

/// `DELETE /groups/{id}` — 200 JSON on success (never 204); unknown or
/// foreign id → 404 `Group not found`. Cascades to options and selections.
async fn delete_group(
    FlaskPath(FlaskInt(group_id)): FlaskPath<FlaskInt>,
    user: AuthUser,
    State(state): State<AppState>,
) -> ApiResult<Json<Value>> {
    let default_id = state.config.default_self_host_id.clone();
    let deleted = with_conn(state.pool.clone(), move |conn| {
        db_groups::delete_group(conn, group_id, Some(user.user_id), &default_id)
    })
    .await?;
    if deleted {
        Ok(Json(json!({
            "status": "success",
            "message": "Group deleted successfully",
        })))
    } else {
        Err(ApiError::NotFound("Group not found".to_string()))
    }
}

/// `DELETE /options/{id}` — 200 JSON on success; unknown or foreign id →
/// 404 `Option not found`. Registered despite having no frontend caller.
async fn delete_option(
    FlaskPath(FlaskInt(option_id)): FlaskPath<FlaskInt>,
    user: AuthUser,
    State(state): State<AppState>,
) -> ApiResult<Json<Value>> {
    let default_id = state.config.default_self_host_id.clone();
    let deleted = with_conn(state.pool.clone(), move |conn| {
        db_groups::delete_group_option(conn, option_id, Some(user.user_id), &default_id)
    })
    .await?;
    if deleted {
        Ok(Json(json!({
            "status": "success",
            "message": "Option deleted successfully",
        })))
    } else {
        Err(ApiError::NotFound("Option not found".to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_body(value: Value) -> Result<Json<Value>, JsonRejection> {
        Ok(Json(value))
    }

    #[test]
    fn extract_name_passes_whitespace_through_to_the_service_check() {
        // "   " is truthy in Python, so the route check passes and the
        // service strip check produces the OTHER 400 string.
        let name = extract_name(&ok_body(json!({"name": "   "})), "Group name is required")
            .expect("whitespace passes the falsy check");
        assert_eq!(name, "   ");
    }

    #[test]
    fn extract_name_rejects_falsy_values_with_the_route_message() {
        for body in [
            json!({}),
            json!({"name": null}),
            json!({"name": ""}),
            json!({"name": false}),
            json!(null),
            json!([1, 2]),
        ] {
            let err = extract_name(&ok_body(body.clone()), "Group name is required").unwrap_err();
            assert!(
                matches!(&err, ApiError::Validation(msg) if msg == "Group name is required"),
                "body {body} should hit the route-layer 400"
            );
        }
    }
}
