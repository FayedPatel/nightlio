//! Goals store ops — the former inline closure bodies of
//! `routes/goals.rs`, moved here verbatim (including the best-effort
//! `goal_completed` activity write and the completions existence probe).
//! The Pg arms dispatch to the async twins in `db/pg/goals.rs` and reuse
//! the same `GoalsError` → `ApiError` mapping, so route-visible behavior
//! is backend-independent.

use rusqlite::Connection;
use serde_json::json;

use crate::db::activity;
use crate::db::goals::{self as db_goals, Goal, GoalCompletion, GoalProgress, GoalsError};
use crate::db::pg::{activity as pg_activity, goals as pg_goals, util as pg_util};
use crate::db::{DbHandle, SqlitePool};
use crate::error::{ApiError, ApiResult};

/// Run a goals db call on a pooled connection under `spawn_blocking`.
async fn with_conn<T, F>(pool: &SqlitePool, f: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce(&Connection) -> Result<T, GoalsError> + Send + 'static,
{
    let pool = pool.clone();
    tokio::task::spawn_blocking(move || -> ApiResult<T> {
        let conn = pool.get()?;
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

/// Check a Pg connection out, mapping pool failure like the Sqlite arm's
/// `pool.get()?` (an internal 500).
async fn pg_client(pool: &deadpool_postgres::Pool) -> Result<deadpool_postgres::Client, ApiError> {
    pg_util::client(pool)
        .await
        .map_err(|exc| ApiError::Internal(anyhow::Error::new(exc)))
}

/// `GET /goals` — bare array, rollover projected per row (pure read).
pub async fn get_goals(db: &DbHandle, user_id: i64) -> ApiResult<Vec<Goal>> {
    match db {
        DbHandle::Sqlite(pool) => {
            with_conn(pool, move |conn| db_goals::get_goals(conn, user_id)).await
        }
        DbHandle::Pg(pool) => {
            let client = pg_client(pool).await?;
            pg_goals::get_goals(&**client, user_id)
                .await
                .map_err(goals_error)
        }
    }
}

/// `POST /goals` — insert, returns the new goal id.
pub async fn create_goal(
    db: &DbHandle,
    user_id: i64,
    title: String,
    description: String,
    frequency: i64,
) -> ApiResult<i64> {
    match db {
        DbHandle::Sqlite(pool) => {
            with_conn(pool, move |conn| {
                db_goals::create_goal(conn, user_id, &title, &description, frequency)
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let client = pg_client(pool).await?;
            pg_goals::create_goal(&**client, user_id, &title, &description, frequency)
                .await
                .map_err(goals_error)
        }
    }
}

/// `GET /goals/{id}` — single goal (rollover projected, pure read).
pub async fn get_goal_by_id(db: &DbHandle, user_id: i64, goal_id: i64) -> ApiResult<Option<Goal>> {
    match db {
        DbHandle::Sqlite(pool) => {
            with_conn(pool, move |conn| {
                db_goals::get_goal_by_id(conn, user_id, goal_id)
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let client = pg_client(pool).await?;
            pg_goals::get_goal_by_id(&**client, user_id, goal_id)
                .await
                .map_err(goals_error)
        }
    }
}

/// `PUT`/`PATCH /goals/{id}` — partial update; `false` only for a
/// missing/foreign row.
pub async fn update_goal(
    db: &DbHandle,
    user_id: i64,
    goal_id: i64,
    title: Option<String>,
    description: Option<String>,
    frequency: Option<i64>,
) -> ApiResult<bool> {
    match db {
        DbHandle::Sqlite(pool) => {
            with_conn(pool, move |conn| {
                db_goals::update_goal(
                    conn,
                    user_id,
                    goal_id,
                    title.as_deref(),
                    description.as_deref(),
                    frequency,
                )
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let client = pg_client(pool).await?;
            pg_goals::update_goal(
                &**client,
                user_id,
                goal_id,
                title.as_deref(),
                description.as_deref(),
                frequency,
            )
            .await
            .map_err(goals_error)
        }
    }
}

/// `DELETE /goals/{id}` — `false` for unknown/foreign ids.
pub async fn delete_goal(db: &DbHandle, user_id: i64, goal_id: i64) -> ApiResult<bool> {
    match db {
        DbHandle::Sqlite(pool) => {
            with_conn(pool, move |conn| {
                db_goals::delete_goal(conn, user_id, goal_id)
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let client = pg_client(pool).await?;
            pg_goals::delete_goal(&**client, user_id, goal_id)
                .await
                .map_err(goals_error)
        }
    }
}

/// `POST /goals/{id}/progress` — log a completion (with the service layer's
/// best-effort `goal_completed` activity write, only when a NEW completion
/// day was recorded); `None` for a missing/foreign goal.
pub async fn increment_progress(
    db: &DbHandle,
    user_id: i64,
    goal_id: i64,
    date_str: Option<String>,
) -> ApiResult<Option<GoalProgress>> {
    match db {
        DbHandle::Sqlite(pool) => {
            with_conn(pool, move |conn| {
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
                    let _ =
                        activity::add_activity(conn, user_id, "goal_completed", Some(&metadata));
                }
                Ok(result)
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let client = pg_client(pool).await?;
            let result =
                pg_goals::increment_goal_progress(&**client, user_id, goal_id, date_str.as_deref())
                    .await
                    .map_err(goals_error)?;
            if let Some(progress) = &result
                && !progress.already_logged
            {
                let metadata = json!({
                    "goal_id": goal_id,
                    "title": progress.goal.title,
                    "date": progress.logged_date,
                });
                // Best-effort, like the Python try/except pass.
                let _ = pg_activity::add_activity(
                    &**client,
                    user_id,
                    "goal_completed",
                    Some(&metadata),
                )
                .await;
            }
            Ok(result)
        }
    }
}

/// `GET .../completions` — existence probe (pure read), then the rows;
/// `None` when the goal is missing/foreign (the route's 404).
pub async fn get_completions(
    db: &DbHandle,
    user_id: i64,
    goal_id: i64,
    start: Option<String>,
    end: Option<String>,
) -> ApiResult<Option<Vec<GoalCompletion>>> {
    match db {
        DbHandle::Sqlite(pool) => {
            with_conn(pool, move |conn| {
                if db_goals::get_goal_by_id(conn, user_id, goal_id)?.is_none() {
                    return Ok(None);
                }
                db_goals::get_goal_completions(
                    conn,
                    user_id,
                    goal_id,
                    start.as_deref(),
                    end.as_deref(),
                )
                .map(Some)
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let client = pg_client(pool).await?;
            if pg_goals::get_goal_by_id(&**client, user_id, goal_id)
                .await
                .map_err(goals_error)?
                .is_none()
            {
                return Ok(None);
            }
            pg_goals::get_goal_completions(
                &**client,
                user_id,
                goal_id,
                start.as_deref(),
                end.as_deref(),
            )
            .await
            .map(Some)
            .map_err(goals_error)
        }
    }
}
