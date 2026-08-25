//! PostgreSQL twin of `db/goals.rs` (the `GoalsMixin` port). Same operation
//! set, same validation messages, same projected-rollover semantics, same
//! rows on the wire — translated statement by statement per the wire-quirk
//! policy in docs/plans/v0.6.0.md:
//!
//! - `?` placeholders become `$n`; `last_insert_rowid()` becomes
//!   `RETURNING id`; `CURRENT_TIMESTAMP` becomes `nightlio_now()`.
//! - `INSERT OR IGNORE` becomes `ON CONFLICT DO NOTHING`; the rows-affected
//!   count (1 = newly logged, 0 = duplicate day) carries over.
//! - `MIN(completed, ?)` (SQLite's scalar two-argument MIN) becomes
//!   `LEAST(completed, $n)`.
//! - The `SELECT *` in `increment_goal_progress` becomes the explicit
//!   11-column list every other goal read uses (physical column order is an
//!   implementation detail; the mapper is positional).
//! - Ordering: `created_at` is `text COLLATE "C"`, so `ORDER BY created_at
//!   DESC, id ASC` is byte-identical to SQLite's BINARY collation — except
//!   NULL placement, where PostgreSQL defaults to NULLS FIRST under DESC
//!   while SQLite sorts NULLs smallest (last under DESC); `NULLS LAST`
//!   restores parity for pre-migration rows with NULL timestamps.
//! - All date logic (`_week_start_iso`, the lexicographic
//!   `target_date >= period_start` compare, the 90-day default window) is
//!   Rust chrono code, duplicated from the sync twin's private helpers
//!   (that module is frozen); `date BETWEEN $3 AND $4` on the
//!   `COLLATE "C"` column keeps SQLite's lexicographic BETWEEN.

use chrono::{Datelike, NaiveDate};
use tokio_postgres::GenericClient;
use tokio_postgres::types::ToSql;

use crate::db::DatabaseError;
use crate::db::goals::{Goal, GoalCompletion, GoalProgress, GoalsError};
use crate::db::pg::util;

// --- SQL (dialect-translated from the `db/goals.rs` constants) ---------------

/// `sql_queries::GET_GOALS_BY_USER` (`NULLS LAST`: see module docs).
const GET_GOALS_BY_USER_SQL: &str = "SELECT id, user_id, title, description, frequency_per_week, completed, \
     streak, period_start, last_completed_date, created_at, updated_at \
     FROM goals WHERE user_id = $1 ORDER BY created_at DESC NULLS LAST, id ASC";

/// `sql_queries::GET_GOAL_BY_ID` — also serves as the rollover/progress
/// refresh SELECT and the `SELECT *` twin (identical column set).
const GET_GOAL_BY_ID_SQL: &str = "SELECT id, user_id, title, description, frequency_per_week, completed, \
     streak, period_start, last_completed_date, created_at, updated_at \
     FROM goals WHERE id = $1 AND user_id = $2";

const ROLLOVER_UPDATE_SQL: &str = "UPDATE goals \
        SET completed = $1, \
            streak = $2, \
            period_start = $3, \
            updated_at = nightlio_now() \
      WHERE id = $4 AND user_id = $5";

const CREATE_GOAL_SQL: &str = "INSERT INTO goals (user_id, title, description, frequency_per_week, \
                         completed, streak, period_start) \
     VALUES ($1, $2, $3, $4, 0, 0, $5) RETURNING id";

const DELETE_GOAL_SQL: &str = "DELETE FROM goals WHERE id = $1 AND user_id = $2";

const INSERT_COMPLETION_SQL: &str = "INSERT INTO goal_completions (user_id, goal_id, date) \
     VALUES ($1, $2, $3) ON CONFLICT DO NOTHING";

const PROGRESS_UPDATE_SQL: &str = "UPDATE goals \
        SET completed = $1, \
            streak = $2, \
            period_start = $3, \
            last_completed_date = COALESCE($4, last_completed_date), \
            updated_at = nightlio_now() \
      WHERE id = $5 AND user_id = $6";

const SELECT_COMPLETIONS_SQL: &str = "SELECT date \
       FROM goal_completions \
      WHERE user_id = $1 AND goal_id = $2 AND date BETWEEN $3 AND $4 \
      ORDER BY date ASC";

fn db_err(exc: tokio_postgres::Error) -> GoalsError {
    GoalsError::Database(util::db_error(exc))
}

// --- Row shape ---------------------------------------------------------------

/// The 11 raw table columns, before `already_completed_today` is computed —
/// the twin of the sync module's private `RawGoal`.
#[derive(Debug, Clone)]
struct RawGoal {
    id: i64,
    user_id: i64,
    title: String,
    description: Option<String>,
    frequency_per_week: Option<i64>,
    completed: Option<i64>,
    streak: Option<i64>,
    period_start: Option<String>,
    last_completed_date: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

impl RawGoal {
    /// Positional extraction from any 11-column goal SELECT (all reads in
    /// this module share the same explicit column list, in this order).
    fn from_row(row: &tokio_postgres::Row) -> RawGoal {
        RawGoal {
            id: row.get(0),
            user_id: row.get(1),
            title: row.get(2),
            description: row.get(3),
            frequency_per_week: row.get(4),
            completed: row.get(5),
            streak: row.get(6),
            period_start: row.get(7),
            last_completed_date: row.get(8),
            created_at: row.get(9),
            updated_at: row.get(10),
        }
    }

    /// Attach the computed key: `already_completed_today` is
    /// `last_completed_date == today` (NULL compares false).
    fn into_goal(self, today_str: &str) -> Goal {
        let already_completed_today = self.last_completed_date.as_deref() == Some(today_str);
        Goal {
            id: self.id,
            user_id: self.user_id,
            title: self.title,
            description: self.description,
            frequency_per_week: self.frequency_per_week,
            completed: self.completed,
            streak: self.streak,
            period_start: self.period_start,
            last_completed_date: self.last_completed_date,
            created_at: self.created_at,
            updated_at: self.updated_at,
            already_completed_today,
        }
    }
}

// --- Date helpers (duplicated from the frozen sync twin's private fns) -------

/// ISO date of the Monday of `date_obj`'s week (Python `weekday()` is
/// Monday=0). Chrono math, never SQL date arithmetic.
fn week_start_iso(date_obj: NaiveDate) -> String {
    let start =
        date_obj - chrono::Duration::days(i64::from(date_obj.weekday().num_days_from_monday()));
    start.format("%Y-%m-%d").to_string()
}

/// Local "today", matching Python's naive `datetime.now()`.
fn local_today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

fn iso(date: NaiveDate) -> String {
    date.format("%Y-%m-%d").to_string()
}

/// Pure rollover projection — identical logic to the sync twin's
/// `project_rollover` (reads never write).
fn project_rollover(raw: &RawGoal, today: NaiveDate) -> Goal {
    let today_start = week_start_iso(today);
    let today_str = iso(today);

    if raw.period_start.as_deref().unwrap_or("") == today_start {
        return raw.clone().into_goal(&today_str);
    }

    let current_completed = raw.completed.unwrap_or(0);
    let freq = raw.frequency_per_week.unwrap_or(0);
    let streak = if freq > 0 && current_completed >= freq {
        raw.streak.unwrap_or(0) + 1
    } else {
        0
    };

    let mut projected = raw.clone();
    projected.completed = Some(0);
    projected.streak = Some(streak);
    projected.period_start = Some(today_start);
    projected.into_goal(&today_str)
}

/// Persisting rollover for write paths — identical semantics to the sync
/// twin's `persist_rollover_if_changed` (stale row: write the projected
/// reset, re-read, fall back to the original when the re-read finds
/// nothing).
async fn persist_rollover_if_changed(
    client: &impl GenericClient,
    raw: RawGoal,
    today: NaiveDate,
) -> Result<Goal, GoalsError> {
    let today_start = week_start_iso(today);
    let today_str = iso(today);

    if raw.period_start.as_deref().unwrap_or("") == today_start {
        return Ok(raw.into_goal(&today_str));
    }

    let projected = project_rollover(&raw, today);
    client
        .execute(
            ROLLOVER_UPDATE_SQL,
            &[
                &projected.completed,
                &projected.streak,
                &today_start,
                &raw.id,
                &raw.user_id,
            ],
        )
        .await
        .map_err(db_err)?;

    let refreshed = client
        .query_opt(GET_GOAL_BY_ID_SQL, &[&raw.id, &raw.user_id])
        .await
        .map_err(db_err)?
        .as_ref()
        .map(RawGoal::from_row);

    Ok(refreshed.unwrap_or(raw).into_goal(&today_str))
}

// --- CRUD operations ---------------------------------------------------------

/// Twin of `goals::create_goal`. Returns the new goal id.
pub async fn create_goal(
    client: &impl GenericClient,
    user_id: i64,
    title: &str,
    description: &str,
    frequency_per_week: i64,
) -> Result<i64, GoalsError> {
    create_goal_on(
        client,
        user_id,
        title,
        description,
        frequency_per_week,
        local_today(),
    )
    .await
}

async fn create_goal_on(
    client: &impl GenericClient,
    user_id: i64,
    title: &str,
    description: &str,
    frequency_per_week: i64,
    today: NaiveDate,
) -> Result<i64, GoalsError> {
    if user_id <= 0 {
        return Err(GoalsError::Validation(
            "user_id must be a positive integer".to_string(),
        ));
    }
    if title.trim().is_empty() {
        return Err(GoalsError::Validation(
            "Title is required and cannot be empty".to_string(),
        ));
    }
    if !(1..=7).contains(&frequency_per_week) {
        return Err(GoalsError::Validation(
            "frequency_per_week must be between 1 and 7".to_string(),
        ));
    }

    let period_start = week_start_iso(today);

    match client
        .query_one(
            CREATE_GOAL_SQL,
            &[
                &user_id,
                &title.trim(),
                &description.trim(),
                &frequency_per_week,
                &period_start,
            ],
        )
        .await
    {
        Ok(row) => Ok(row.get(0)),
        Err(exc) => {
            tracing::error!("Failed to create goal for user {user_id}: {exc}");
            Err(GoalsError::Database(DatabaseError::Message(format!(
                "Failed to create goal: {exc}"
            ))))
        }
    }
}

/// Twin of `goals::get_goals` — list ordered by `created_at DESC, id ASC`,
/// rollover projected per row (pure read).
pub async fn get_goals(client: &impl GenericClient, user_id: i64) -> Result<Vec<Goal>, GoalsError> {
    get_goals_on(client, user_id, local_today()).await
}

async fn get_goals_on(
    client: &impl GenericClient,
    user_id: i64,
    today: NaiveDate,
) -> Result<Vec<Goal>, GoalsError> {
    let rows = client
        .query(GET_GOALS_BY_USER_SQL, &[&user_id])
        .await
        .map_err(db_err)?;
    Ok(rows
        .iter()
        .map(|row| project_rollover(&RawGoal::from_row(row), today))
        .collect())
}

/// Twin of `goals::get_goal_by_id` — single goal, rollover projected,
/// `None` when missing or foreign.
pub async fn get_goal_by_id(
    client: &impl GenericClient,
    user_id: i64,
    goal_id: i64,
) -> Result<Option<Goal>, GoalsError> {
    get_goal_by_id_on(client, user_id, goal_id, local_today()).await
}

async fn get_goal_by_id_on(
    client: &impl GenericClient,
    user_id: i64,
    goal_id: i64,
    today: NaiveDate,
) -> Result<Option<Goal>, GoalsError> {
    let raw = client
        .query_opt(GET_GOAL_BY_ID_SQL, &[&goal_id, &user_id])
        .await
        .map_err(db_err)?
        .as_ref()
        .map(RawGoal::from_row);
    Ok(raw.map(|raw| project_rollover(&raw, today)))
}

/// Twin of `goals::update_goal` — same validation-before-existence
/// ordering, same rollover-before-clamp write, same `false` returns.
pub async fn update_goal(
    client: &impl GenericClient,
    user_id: i64,
    goal_id: i64,
    title: Option<&str>,
    description: Option<&str>,
    frequency_per_week: Option<i64>,
) -> Result<bool, GoalsError> {
    update_goal_on(
        client,
        user_id,
        goal_id,
        title,
        description,
        frequency_per_week,
        local_today(),
    )
    .await
}

#[allow(clippy::too_many_arguments)] // internal `_on` variant adds the pinned date
async fn update_goal_on(
    client: &impl GenericClient,
    user_id: i64,
    goal_id: i64,
    title: Option<&str>,
    description: Option<&str>,
    frequency_per_week: Option<i64>,
    today: NaiveDate,
) -> Result<bool, GoalsError> {
    let mut updates: Vec<String> = Vec::new();
    let mut sql_params: Vec<Box<dyn ToSql + Send + Sync>> = Vec::new();

    if let Some(title) = title {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return Err(GoalsError::Validation(
                "Title is required and cannot be empty".to_string(),
            ));
        }
        sql_params.push(Box::new(trimmed.to_string()));
        updates.push(format!("title = ${}", sql_params.len()));
    }
    if let Some(description) = description {
        sql_params.push(Box::new(description.trim().to_string()));
        updates.push(format!("description = ${}", sql_params.len()));
    }
    if let Some(freq) = frequency_per_week {
        if !(1..=7).contains(&freq) {
            return Err(GoalsError::Validation(
                "frequency_per_week must be between 1 and 7".to_string(),
            ));
        }
        sql_params.push(Box::new(freq));
        updates.push(format!("frequency_per_week = ${}", sql_params.len()));
        sql_params.push(Box::new(freq));
        // SQLite's scalar two-argument MIN — LEAST on PostgreSQL.
        updates.push(format!(
            "completed = LEAST(completed, ${})",
            sql_params.len()
        ));
    }

    if updates.is_empty() {
        return Ok(false);
    }

    // Persist the weekly rollover before the UPDATE so the LEAST clamp sees
    // the rolled-over counter (same contract change as the SQLite twin).
    // Missing/foreign rows still yield `Ok(false)` (the route's 404).
    let raw = client
        .query_opt(GET_GOAL_BY_ID_SQL, &[&goal_id, &user_id])
        .await
        .map_err(db_err)?
        .as_ref()
        .map(RawGoal::from_row);
    let Some(raw) = raw else {
        return Ok(false);
    };
    persist_rollover_if_changed(client, raw, today).await?;

    updates.push("updated_at = nightlio_now()".to_string());
    sql_params.push(Box::new(goal_id));
    let goal_placeholder = sql_params.len();
    sql_params.push(Box::new(user_id));
    let user_placeholder = sql_params.len();

    let sql = format!(
        "UPDATE goals SET {} WHERE id = ${goal_placeholder} AND user_id = ${user_placeholder}",
        updates.join(", ")
    );
    let params: Vec<&(dyn ToSql + Sync)> = sql_params
        .iter()
        .map(|param| param.as_ref() as &(dyn ToSql + Sync))
        .collect();
    let changed = client.execute(&sql, &params).await.map_err(db_err)?;
    Ok(changed > 0)
}

/// Twin of `goals::delete_goal` — `goal_completions` rows go with the goal
/// via `ON DELETE CASCADE`.
pub async fn delete_goal(
    client: &impl GenericClient,
    user_id: i64,
    goal_id: i64,
) -> Result<bool, GoalsError> {
    let changed = client
        .execute(DELETE_GOAL_SQL, &[&goal_id, &user_id])
        .await
        .map_err(db_err)?;
    Ok(changed > 0)
}

/// Twin of `goals::increment_goal_progress`: log a completion for
/// `date_str` (ISO, default today). Duplicate days are detected by the
/// `ON CONFLICT DO NOTHING` rows-affected count; the weekly counter only
/// moves for dates inside the current period.
pub async fn increment_goal_progress(
    client: &impl GenericClient,
    user_id: i64,
    goal_id: i64,
    date_str: Option<&str>,
) -> Result<Option<GoalProgress>, GoalsError> {
    increment_goal_progress_on(client, user_id, goal_id, date_str, local_today()).await
}

async fn increment_goal_progress_on(
    client: &impl GenericClient,
    user_id: i64,
    goal_id: i64,
    date_str: Option<&str>,
    today: NaiveDate,
) -> Result<Option<GoalProgress>, GoalsError> {
    let today_str = iso(today);
    let target_date = date_str.unwrap_or(&today_str);

    let raw = client
        .query_opt(GET_GOAL_BY_ID_SQL, &[&goal_id, &user_id])
        .await
        .map_err(db_err)?
        .as_ref()
        .map(RawGoal::from_row);
    let Some(raw) = raw else {
        return Ok(None);
    };

    let goal = persist_rollover_if_changed(client, raw, today).await?;
    let mut current_completed = goal.completed.unwrap_or(0);
    let freq = goal.frequency_per_week.unwrap_or(0);
    let streak = goal.streak.unwrap_or(0);
    let period_start = goal.period_start.clone();

    let newly_logged = client
        .execute(INSERT_COMPLETION_SQL, &[&user_id, &goal_id, &target_date])
        .await
        .map_err(db_err)?
        == 1;

    // Lexicographic compare on ISO dates; empty/NULL period_start is falsy
    // (Python: `bool(period_start) and target_date >= period_start`).
    let in_current_period = period_start
        .as_deref()
        .is_some_and(|start| !start.is_empty() && target_date >= start);
    if newly_logged && in_current_period && current_completed < freq {
        current_completed += 1;
    }

    // max() keeps today's "Completed" button lock intact when a backdated
    // day is logged after today was already marked done.
    let last_completed_date = std::cmp::max(
        goal.last_completed_date.clone().unwrap_or_default(),
        target_date.to_string(),
    );

    client
        .execute(
            PROGRESS_UPDATE_SQL,
            &[
                &current_completed,
                &streak,
                &period_start,
                &last_completed_date,
                &goal_id,
                &user_id,
            ],
        )
        .await
        .map_err(db_err)?;

    let updated = client
        .query_opt(GET_GOAL_BY_ID_SQL, &[&goal_id, &user_id])
        .await
        .map_err(db_err)?
        .as_ref()
        .map(RawGoal::from_row);
    let Some(updated) = updated else {
        return Ok(None);
    };

    Ok(Some(GoalProgress {
        goal: updated.into_goal(&today_str),
        already_logged: !newly_logged,
        logged_date: target_date.to_string(),
    }))
}

/// Twin of `goals::get_goal_completions`. When either bound is absent or
/// empty, BOTH default to the last 90 days ending today; `[]` for
/// nonexistent/foreign goals (the 200-`[]` quirk).
pub async fn get_goal_completions(
    client: &impl GenericClient,
    user_id: i64,
    goal_id: i64,
    start_date: Option<&str>,
    end_date: Option<&str>,
) -> Result<Vec<GoalCompletion>, GoalsError> {
    get_goal_completions_on(
        client,
        user_id,
        goal_id,
        start_date,
        end_date,
        local_today(),
    )
    .await
}

async fn get_goal_completions_on(
    client: &impl GenericClient,
    user_id: i64,
    goal_id: i64,
    start_date: Option<&str>,
    end_date: Option<&str>,
    today: NaiveDate,
) -> Result<Vec<GoalCompletion>, GoalsError> {
    let (start, end) = match (start_date, end_date) {
        (Some(start), Some(end)) if !start.is_empty() && !end.is_empty() => {
            (start.to_string(), end.to_string())
        }
        _ => (iso(today - chrono::Duration::days(90)), iso(today)),
    };

    let rows = client
        .query(SELECT_COMPLETIONS_SQL, &[&user_id, &goal_id, &start, &end])
        .await
        .map_err(db_err)?;
    Ok(rows
        .into_iter()
        .map(|row| GoalCompletion { date: row.get(0) })
        .collect())
}

#[cfg(test)]
mod tests {
    //! Live-PG quirk tests (skipped unless `NIGHTLIO_PG_TEST_URL` is set).
    //! Pinned values mirror the SQLite twin's Python-pinned tests; the
    //! date-sensitive cases run through the `_on` internals with today
    //! pinned to 2026-08-15 (Saturday; Monday = 2026-08-10).

    use serde_json::json;

    use super::*;
    use crate::db::pg::util::test_support::{connect_scratch, seed_default_user};

    /// Seed a goal row with explicit counters/dates (sentinel updated_at).
    #[allow(clippy::too_many_arguments)] // mirrors the seeded column list
    async fn seed_goal(
        client: &tokio_postgres::Client,
        user_id: i64,
        title: &str,
        freq: i64,
        completed: i64,
        streak: i64,
        period_start: Option<&str>,
        created_at: Option<&str>,
    ) {
        client
            .execute(
                "INSERT INTO goals (user_id, title, description, frequency_per_week, \
                 completed, streak, period_start, created_at, updated_at) \
                 VALUES ($1, $2, '', $3, $4, $5, $6, $7, '2020-01-01 00:00:00')",
                &[
                    &user_id,
                    &title,
                    &freq,
                    &completed,
                    &streak,
                    &period_start,
                    &created_at,
                ],
            )
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn list_order_and_projected_rollover_match_the_sqlite_pins() {
        let Some(client) = connect_scratch("ws4a_goals_rollover").await else {
            return;
        };
        let uid = seed_default_user(&client).await;
        let pinned = NaiveDate::from_ymd_opt(2026, 8, 15).unwrap();

        // The five rollover fixtures (met/missed/overmet/null-period/current)
        // plus a NULL created_at row for the ordering quirk.
        seed_goal(
            &client,
            uid,
            "met",
            3,
            3,
            5,
            Some("2020-01-06"),
            Some("2020-01-01 00:00:00"),
        )
        .await;
        seed_goal(
            &client,
            uid,
            "missed",
            3,
            2,
            5,
            Some("2020-01-06"),
            Some("2020-01-02 00:00:00"),
        )
        .await;
        seed_goal(
            &client,
            uid,
            "overmet",
            1,
            5,
            0,
            Some("2020-01-06"),
            Some("2020-01-03 00:00:00"),
        )
        .await;
        seed_goal(
            &client,
            uid,
            "null-period",
            7,
            0,
            9,
            None,
            Some("2020-01-04 00:00:00"),
        )
        .await;
        seed_goal(
            &client,
            uid,
            "current",
            2,
            2,
            4,
            Some("2026-08-10"),
            Some("2020-01-05 00:00:00"),
        )
        .await;
        // Ordering-quirk rows: a created_at tie broken by id ASC, and a NULL
        // created_at that must sort LAST under DESC (SQLite: NULL smallest).
        seed_goal(
            &client,
            uid,
            "tie-a",
            1,
            0,
            0,
            Some("2026-08-10"),
            Some("2020-01-05 00:00:00"),
        )
        .await;
        seed_goal(
            &client,
            uid,
            "null-created",
            1,
            0,
            0,
            Some("2026-08-10"),
            None,
        )
        .await;

        let goals = get_goals_on(&client, uid, pinned).await.unwrap();
        let titles: Vec<&str> = goals.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "current",
                "tie-a",
                "null-period",
                "overmet",
                "missed",
                "met",
                "null-created"
            ],
            "created_at DESC (NULLS LAST), id ASC — byte order + SQLite NULL placement"
        );

        // Projected rollover values, pinned from the Python cross-check.
        let expect = [
            ("current", 2, 4),
            ("tie-a", 0, 0),
            ("null-period", 0, 0),
            ("overmet", 0, 1),
            ("missed", 0, 0),
            ("met", 0, 6),
            ("null-created", 0, 0),
        ];
        for ((title, completed, streak), goal) in expect.iter().zip(&goals) {
            assert_eq!(goal.title, *title);
            assert_eq!(goal.completed, Some(*completed), "{title} completed");
            assert_eq!(goal.streak, Some(*streak), "{title} streak");
            assert_eq!(goal.period_start.as_deref(), Some("2026-08-10"), "{title}");
        }

        // Reads are pure: the raw rows keep their seeded values and the
        // sentinel updated_at.
        let (met_completed, met_streak, met_updated): (i64, i64, String) = {
            let row = client
                .query_one(
                    "SELECT completed, streak, updated_at FROM goals WHERE title = 'met'",
                    &[],
                )
                .await
                .unwrap();
            (row.get(0), row.get(1), row.get(2))
        };
        assert_eq!((met_completed, met_streak), (3, 5), "GET must not write");
        assert_eq!(
            met_updated, "2020-01-01 00:00:00",
            "GET must not bump updated_at"
        );

        // get_goal_by_id projects the same rollover; missing/foreign → None.
        let met_id: i64 = client
            .query_one("SELECT id FROM goals WHERE title = 'met'", &[])
            .await
            .unwrap()
            .get(0);
        let goal = get_goal_by_id_on(&client, uid, met_id, pinned)
            .await
            .unwrap()
            .unwrap();
        assert_eq!((goal.completed, goal.streak), (Some(0), Some(6)));
        assert!(
            get_goal_by_id_on(&client, uid, 999, pinned)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            get_goal_by_id_on(&client, uid + 1, met_id, pinned)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn update_rolls_over_before_the_least_clamp_and_validates() {
        let Some(client) = connect_scratch("ws4a_goals_update").await else {
            return;
        };
        let uid = seed_default_user(&client).await;
        let pinned = NaiveDate::from_ymd_opt(2026, 8, 15).unwrap();

        // Validation messages, verbatim.
        let err = create_goal_on(&client, 0, "x", "", 3, pinned)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "user_id must be a positive integer");
        let err = create_goal_on(&client, uid, "   ", "", 3, pinned)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "Title is required and cannot be empty");
        let err = create_goal_on(&client, uid, "x", "", 8, pinned)
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "frequency_per_week must be between 1 and 7"
        );

        // create trims and seeds a zeroed week (RETURNING id).
        let id = create_goal_on(&client, uid, "  progress  ", "  desc  ", 4, pinned)
            .await
            .unwrap();
        let goal = get_goal_by_id_on(&client, uid, id, pinned)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(goal.title, "progress");
        assert_eq!(goal.description.as_deref(), Some("desc"));
        assert_eq!(goal.period_start.as_deref(), Some("2026-08-10"));

        // No fields / missing row / foreign row → false.
        assert!(
            !update_goal_on(&client, uid, id, None, None, None, pinned)
                .await
                .unwrap()
        );
        assert!(
            !update_goal_on(&client, uid, 999, Some("x"), None, None, pinned)
                .await
                .unwrap()
        );
        assert!(
            !update_goal_on(&client, uid + 1, id, Some("x"), None, None, pinned)
                .await
                .unwrap()
        );

        // Blank title rejected, row untouched.
        let err = update_goal_on(&client, uid, id, Some("   "), None, None, pinned)
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "Title is required and cannot be empty");

        // In-period clamp: completed=3, freq lowered to 1 → LEAST clamps.
        client
            .execute("UPDATE goals SET completed = 3 WHERE id = $1", &[&id])
            .await
            .unwrap();
        assert!(
            update_goal_on(&client, uid, id, None, None, Some(1), pinned)
                .await
                .unwrap()
        );
        let row = client
            .query_one(
                "SELECT completed, frequency_per_week FROM goals WHERE id = $1",
                &[&id],
            )
            .await
            .unwrap();
        assert_eq!((row.get::<_, i64>(0), row.get::<_, i64>(1)), (1, 1));

        // Stale-week clamp: the rollover persists BEFORE the UPDATE, so
        // LEAST sees the fresh week's 0, not the stale 5 — and the met week
        // increments the streak.
        seed_goal(
            &client,
            uid,
            "stale-overmet",
            5,
            5,
            3,
            Some("2020-01-06"),
            Some("2020-01-02 00:00:00"),
        )
        .await;
        let stale_id: i64 = client
            .query_one("SELECT id FROM goals WHERE title = 'stale-overmet'", &[])
            .await
            .unwrap()
            .get(0);
        assert!(
            update_goal_on(&client, uid, stale_id, None, None, Some(2), pinned)
                .await
                .unwrap()
        );
        let row = client
            .query_one(
                "SELECT completed, frequency_per_week, streak, period_start \
                 FROM goals WHERE id = $1",
                &[&stale_id],
            )
            .await
            .unwrap();
        assert_eq!(row.get::<_, i64>(0), 0, "clamp must see the rolled-over 0");
        assert_eq!(row.get::<_, i64>(1), 2);
        assert_eq!(row.get::<_, i64>(2), 4, "met week increments the streak");
        assert_eq!(row.get::<_, String>(3), "2026-08-10");
    }

    #[tokio::test]
    async fn progress_and_completions_keep_lexicographic_date_semantics() {
        let Some(client) = connect_scratch("ws4a_goals_progress").await else {
            return;
        };
        let uid = seed_default_user(&client).await;
        let pinned = NaiveDate::from_ymd_opt(2026, 8, 15).unwrap();
        let id = create_goal_on(&client, uid, "progress", "desc", 4, pinned)
            .await
            .unwrap();

        // today#1: completed=1, newly logged, lock set.
        let first = increment_goal_progress_on(&client, uid, id, None, pinned)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.goal.completed, Some(1));
        assert!(!first.already_logged);
        assert_eq!(first.logged_date, "2026-08-15");
        assert!(first.goal.already_completed_today);
        assert_eq!(
            first.goal.last_completed_date.as_deref(),
            Some("2026-08-15")
        );

        // today#2: duplicate day must not count (ON CONFLICT DO NOTHING).
        let second = increment_goal_progress_on(&client, uid, id, None, pinned)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second.goal.completed, Some(1));
        assert!(second.already_logged);

        // Backdate within the week counts; max() keeps today's lock.
        let back = increment_goal_progress_on(&client, uid, id, Some("2026-08-10"), pinned)
            .await
            .unwrap()
            .unwrap();
        assert!(!back.already_logged);
        assert_eq!(back.goal.completed, Some(2));
        assert_eq!(back.logged_date, "2026-08-10");
        assert_eq!(back.goal.last_completed_date.as_deref(), Some("2026-08-15"));

        // Out-of-period backdate lands in the log without counting.
        let old = increment_goal_progress_on(&client, uid, id, Some("2020-01-01"), pinned)
            .await
            .unwrap()
            .unwrap();
        assert!(!old.already_logged);
        assert_eq!(old.goal.completed, Some(2), "closed week's counter frozen");
        assert_eq!(old.goal.last_completed_date.as_deref(), Some("2026-08-15"));

        // Missing goal → None.
        assert!(
            increment_goal_progress_on(&client, uid, 999, None, pinned)
                .await
                .unwrap()
                .is_none()
        );

        // Completions: BETWEEN on the COLLATE "C" text column is
        // lexicographic; ORDER BY date ASC; default 90-day window drops the
        // out-of-window row; nonexistent goal → [] (200-[] quirk).
        let got = get_goal_completions_on(
            &client,
            uid,
            id,
            Some("2020-01-01"),
            Some("2099-12-31"),
            pinned,
        )
        .await
        .unwrap();
        assert_eq!(
            serde_json::to_value(&got).unwrap(),
            json!([
                {"date": "2020-01-01"},
                {"date": "2026-08-10"},
                {"date": "2026-08-15"}
            ])
        );
        let got = get_goal_completions_on(&client, uid, id, None, None, pinned)
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(&got).unwrap(),
            json!([{"date": "2026-08-10"}, {"date": "2026-08-15"}])
        );
        // One empty bound resets both to the default window.
        let got = get_goal_completions_on(&client, uid, id, Some(""), Some(""), pinned)
            .await
            .unwrap();
        assert_eq!(got.len(), 2);
        assert!(
            get_goal_completions_on(&client, uid, 999, None, None, pinned)
                .await
                .unwrap()
                .is_empty()
        );

        // delete cascades the completion log; second delete → false.
        assert!(delete_goal(&client, uid, id).await.unwrap());
        assert!(!delete_goal(&client, uid, id).await.unwrap());
        let left: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM goal_completions WHERE goal_id = $1",
                &[&id],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(left, 0, "ON DELETE CASCADE must clear the completion log");
    }
}
