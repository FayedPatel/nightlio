//! Port of `api/database_goals.py` (`GoalsMixin`). Owned by the goals
//! data-mixin agent — no other agent edits this file.
//!
//! Semantics carried over from the Python mixin, with owner-approved
//! contract changes noted inline:
//! - **Weekly rollover is projected on reads, persisted only by writes**
//!   (contract change, owner-approved; supersedes the reads-that-wrote
//!   behavior formerly frozen as `contract/DECISIONS.md` item 11): when a
//!   goal's `period_start` is not the current week's Monday, responses
//!   carry `completed` reset to 0, `streak` recomputed (incremented when
//!   the closed week met `frequency_per_week`, else reset to 0) and
//!   `period_start` repointed to the current Monday. Read paths
//!   (`get_goals`, `get_goal_by_id`) compute this with the pure
//!   [`project_rollover`] and never touch the database; write paths
//!   (`increment_goal_progress`, `update_goal`) persist the same rollover
//!   via [`persist_rollover_if_changed`] before applying their own writes.
//!   The projected `updated_at` is the stored value, not a fresh timestamp
//!   (wire-safe: fixtures normalize timestamps).
//! - `update_goal` returns `false` when no fields were provided (the route
//!   400s that case before calling; the guard remains for direct callers)
//!   and when the row does not exist (the route's 404). Contract
//!   change (owner-approved, `contract/DECISIONS.md` item 7): a blank
//!   title is now rejected with the same `ValueError` message
//!   `create_goal` uses instead of being silently written as `''`. Contract change:
//!   the rollover is persisted before the UPDATE runs, so the
//!   `completed = MIN(completed, ?)` clamp operates on the rolled-over
//!   counter instead of a stale week's value.
//! - Frequency outside 1..=7 raises the Python `ValueError` message
//!   `frequency_per_week must be between 1 and 7` (a 400 at the route).
//! - Progress increments only move the weekly counter for dates inside the
//!   current period; backdated completions from an already-rolled-over week
//!   land in `goal_completions` (calendar, stats) without touching the
//!   closed week's counter or streak.
//!
//! All functions take `&Connection` and are blocking — callers run them
//! under `tokio::task::spawn_blocking`.

use chrono::{Datelike, NaiveDate};
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};
use serde::Serialize;

use super::common::{DatabaseError, sql_queries};

// --- Goals-specific SQL (byte-identical to the Python literals) -------------

/// `_rollover_goal_if_needed` UPDATE.
const ROLLOVER_UPDATE_SQL: &str = "
            UPDATE goals
               SET completed = ?,
                   streak = ?,
                   period_start = ?,
                   updated_at = CURRENT_TIMESTAMP
             WHERE id = ? AND user_id = ?
            ";

/// `_rollover_goal_if_needed` refresh SELECT.
const ROLLOVER_REFRESH_SQL: &str = "
            SELECT id, user_id, title, description, frequency_per_week, completed,
                   streak, period_start, last_completed_date, created_at, updated_at
              FROM goals WHERE id = ? AND user_id = ?
            ";

/// `create_goal` INSERT.
const CREATE_GOAL_SQL: &str = "
                    INSERT INTO goals (user_id, title, description, frequency_per_week,
                                        completed, streak, period_start)
                    VALUES (?, ?, ?, ?, 0, 0, ?)
                    ";

/// `delete_goal` DELETE.
const DELETE_GOAL_SQL: &str = "DELETE FROM goals WHERE id = ? AND user_id = ?";

/// `increment_goal_progress` initial row fetch.
const SELECT_GOAL_STAR_SQL: &str = "SELECT * FROM goals WHERE id = ? AND user_id = ?";

/// `increment_goal_progress` per-day completion log INSERT.
const INSERT_COMPLETION_SQL: &str = "
                INSERT OR IGNORE INTO goal_completions (user_id, goal_id, date)
                VALUES (?, ?, ?)
                ";

/// `increment_goal_progress` UPDATE.
const PROGRESS_UPDATE_SQL: &str = "
                UPDATE goals
                   SET completed = ?,
                       streak = ?,
                       period_start = ?,
                       last_completed_date = COALESCE(?, last_completed_date),
                       updated_at = CURRENT_TIMESTAMP
                 WHERE id = ? AND user_id = ?
                ";

/// `increment_goal_progress` refresh SELECT.
const PROGRESS_REFRESH_SQL: &str = "
                SELECT id, user_id, title, description, frequency_per_week, completed,
                       streak, period_start, last_completed_date, created_at, updated_at
                  FROM goals WHERE id = ? AND user_id = ?
                ";

/// `get_goal_completions` SELECT.
const SELECT_COMPLETIONS_SQL: &str = "
                SELECT date
                  FROM goal_completions
                 WHERE user_id = ? AND goal_id = ? AND date BETWEEN ? AND ?
                 ORDER BY date ASC
                ";

// --- Errors -----------------------------------------------------------------

/// Failure modes of the goals mixin. `Validation` is the Python
/// `ValueError` — the message reaches the client verbatim as a 400.
#[derive(Debug, thiserror::Error)]
pub enum GoalsError {
    /// Port of the `ValueError`s raised by `create_goal` / `update_goal`.
    #[error("{0}")]
    Validation(String),

    /// Port of `database_common.DatabaseError` (`create_goal` wraps SQLite
    /// failures as `Failed to create goal: ...`).
    #[error(transparent)]
    Database(#[from] DatabaseError),

    /// Raw SQLite failure from statements the Python leaves unwrapped.
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

// --- Row shapes -------------------------------------------------------------

/// A `goals` row as the Flask API emits it: the 11 table columns plus the
/// computed `already_completed_today`. Field names match the JSON contract
/// (`contract/fixtures/goals/goal-get-200.json` et al.) exactly; serde's
/// derive keeps them verbatim.
///
/// Columns that predate NOT NULL guarantees (or are nullable by schema)
/// stay `Option` — half-migrated databases exist in the wild and the dict
/// passthrough in Python tolerates NULLs everywhere the schema allows them.
#[derive(Debug, Clone, Serialize)]
pub struct Goal {
    pub id: i64,
    pub user_id: i64,
    pub title: String,
    pub description: Option<String>,
    pub frequency_per_week: Option<i64>,
    pub completed: Option<i64>,
    pub streak: Option<i64>,
    pub period_start: Option<String>,
    pub last_completed_date: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub already_completed_today: bool,
}

/// `increment_goal_progress` response: the updated goal row plus the two
/// extra computed keys the Python adds
/// (`contract/fixtures/goals/goal-progress-200-today.json`).
#[derive(Debug, Clone, Serialize)]
pub struct GoalProgress {
    #[serde(flatten)]
    pub goal: Goal,
    pub already_logged: bool,
    pub logged_date: String,
}

/// One `get_goal_completions` row: `{"date": "YYYY-MM-DD"}`.
#[derive(Debug, Clone, Serialize)]
pub struct GoalCompletion {
    pub date: String,
}

/// The 11 raw table columns, before `already_completed_today` is computed.
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
    /// Named-column extraction so both the explicit 11-column SELECTs and
    /// the `SELECT *` in `increment_goal_progress` map identically,
    /// regardless of physical column order (ALTER TABLE puts
    /// `last_completed_date` last on migrated databases).
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawGoal> {
        Ok(RawGoal {
            id: row.get("id")?,
            user_id: row.get("user_id")?,
            title: row.get("title")?,
            description: row.get("description")?,
            frequency_per_week: row.get("frequency_per_week")?,
            completed: row.get("completed")?,
            streak: row.get("streak")?,
            period_start: row.get("period_start")?,
            last_completed_date: row.get("last_completed_date")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
        })
    }

    /// Attach the computed key: `already_completed_today` is
    /// `last_completed_date == today` (NULL compares false, like Python's
    /// `None == "..."`).
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

// --- Helpers ----------------------------------------------------------------

/// Port of `GoalsMixin._week_start_iso`: ISO date of the Monday of
/// `date_obj`'s week (Python `weekday()` is Monday=0).
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

/// Pure half of the old `_rollover_goal_if_needed` (contract change:
/// reads project the rollover instead of writing it). An in-period row
/// passes through unchanged; a stale row is returned with `completed` 0,
/// `streak` recomputed (incremented when `freq > 0 && completed >= freq`,
/// else reset to 0) and `period_start` repointed to the current Monday.
/// The projected `updated_at` is the stored value, not a fresh timestamp —
/// wire-safe because fixtures normalize timestamps.
fn project_rollover(raw: &RawGoal, today: NaiveDate) -> Goal {
    let today_start = week_start_iso(today);
    let today_str = iso(today);

    // Python: `(goal_dict.get("period_start") or "") == today_start`.
    if raw.period_start.as_deref().unwrap_or("") == today_start {
        return raw.clone().into_goal(&today_str);
    }

    // Python coerces NULLs with `int(x or 0)`.
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

/// Persisting half of the old `_rollover_goal_if_needed`, used only by
/// write paths (`increment_goal_progress`, `update_goal`). An in-period
/// row is a pure read; a stale row gets the projected reset WRITTEN via
/// `ROLLOVER_UPDATE_SQL` (which also bumps `updated_at`), is re-read, and
/// falls back to the original (pre-update) values when the re-read finds
/// nothing — exactly like the Python `dict(refreshed) if refreshed else
/// goal_dict`.
fn persist_rollover_if_changed(
    conn: &Connection,
    raw: RawGoal,
    today: NaiveDate,
) -> Result<Goal, GoalsError> {
    let today_start = week_start_iso(today);
    let today_str = iso(today);

    if raw.period_start.as_deref().unwrap_or("") == today_start {
        return Ok(raw.into_goal(&today_str));
    }

    let projected = project_rollover(&raw, today);
    conn.execute(
        ROLLOVER_UPDATE_SQL,
        params![
            projected.completed,
            projected.streak,
            today_start,
            raw.id,
            raw.user_id
        ],
    )?;

    let refreshed = conn
        .query_row(
            ROLLOVER_REFRESH_SQL,
            params![raw.id, raw.user_id],
            RawGoal::from_row,
        )
        .optional()?;

    Ok(refreshed.unwrap_or(raw).into_goal(&today_str))
}

// --- CRUD operations --------------------------------------------------------

/// Port of `GoalsMixin.create_goal`. Returns the new goal id.
pub fn create_goal(
    conn: &Connection,
    user_id: i64,
    title: &str,
    description: &str,
    frequency_per_week: i64,
) -> Result<i64, GoalsError> {
    create_goal_on(
        conn,
        user_id,
        title,
        description,
        frequency_per_week,
        local_today(),
    )
}

fn create_goal_on(
    conn: &Connection,
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

    match conn.execute(
        CREATE_GOAL_SQL,
        params![
            user_id,
            title.trim(),
            description.trim(),
            frequency_per_week,
            period_start
        ],
    ) {
        Ok(_) => Ok(conn.last_insert_rowid()),
        Err(exc) => {
            tracing::error!("Failed to create goal for user {user_id}: {exc}");
            Err(GoalsError::Database(DatabaseError::Message(format!(
                "Failed to create goal: {exc}"
            ))))
        }
    }
}

/// Port of `GoalsMixin.get_goals` — list ordered by `created_at DESC`,
/// with the weekly rollover projected per row (contract change: a
/// pure read; nothing is written).
pub fn get_goals(conn: &Connection, user_id: i64) -> Result<Vec<Goal>, GoalsError> {
    get_goals_on(conn, user_id, local_today())
}

fn get_goals_on(
    conn: &Connection,
    user_id: i64,
    today: NaiveDate,
) -> Result<Vec<Goal>, GoalsError> {
    let raws = {
        let mut stmt = conn.prepare(sql_queries::GET_GOALS_BY_USER)?;
        let rows = stmt.query_map(params![user_id], RawGoal::from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    Ok(raws
        .into_iter()
        .map(|raw| project_rollover(&raw, today))
        .collect())
}

/// Port of `GoalsMixin.get_goal_by_id` — single goal with the weekly
/// rollover projected (contract change: a pure read), `None` when
/// missing or foreign.
pub fn get_goal_by_id(
    conn: &Connection,
    user_id: i64,
    goal_id: i64,
) -> Result<Option<Goal>, GoalsError> {
    get_goal_by_id_on(conn, user_id, goal_id, local_today())
}

fn get_goal_by_id_on(
    conn: &Connection,
    user_id: i64,
    goal_id: i64,
    today: NaiveDate,
) -> Result<Option<Goal>, GoalsError> {
    let raw = conn
        .query_row(
            sql_queries::GET_GOAL_BY_ID,
            params![goal_id, user_id],
            RawGoal::from_row,
        )
        .optional()?;
    Ok(raw.map(|raw| project_rollover(&raw, today)))
}

/// Port of `GoalsMixin.update_goal`. Returns `false` for "no fields
/// provided" (the route 400s that before calling; kept for direct callers)
/// and for "row not found" (the route's 404). contract change
/// (owner-approved, `contract/DECISIONS.md` item 7): a blank `title` now
/// raises the same validation error `create_goal` uses instead of being
/// silently stored as `""`. A frequency update clamps `completed` via
/// `MIN(completed, ?)`.
///
/// contract change (and latent-bug fix): now that reads no longer
/// persist the rollover, this path persists it itself — after field
/// validation (preserving the 400-before-existence ordering) the row is
/// read and rolled over BEFORE the UPDATE runs, so the
/// `MIN(completed, ?)` clamp operates on the current week's counter, not
/// a stale week's value.
pub fn update_goal(
    conn: &Connection,
    user_id: i64,
    goal_id: i64,
    title: Option<&str>,
    description: Option<&str>,
    frequency_per_week: Option<i64>,
) -> Result<bool, GoalsError> {
    update_goal_on(
        conn,
        user_id,
        goal_id,
        title,
        description,
        frequency_per_week,
        local_today(),
    )
}

#[allow(clippy::too_many_arguments)] // internal `_on` variant adds the pinned date
fn update_goal_on(
    conn: &Connection,
    user_id: i64,
    goal_id: i64,
    title: Option<&str>,
    description: Option<&str>,
    frequency_per_week: Option<i64>,
    today: NaiveDate,
) -> Result<bool, GoalsError> {
    let mut updates: Vec<&str> = Vec::new();
    let mut sql_params: Vec<Value> = Vec::new();

    if let Some(title) = title {
        let trimmed = title.trim();
        if trimmed.is_empty() {
            return Err(GoalsError::Validation(
                "Title is required and cannot be empty".to_string(),
            ));
        }
        updates.push("title = ?");
        sql_params.push(Value::from(trimmed.to_string()));
    }
    if let Some(description) = description {
        updates.push("description = ?");
        sql_params.push(Value::from(description.trim().to_string()));
    }
    if let Some(freq) = frequency_per_week {
        if !(1..=7).contains(&freq) {
            return Err(GoalsError::Validation(
                "frequency_per_week must be between 1 and 7".to_string(),
            ));
        }
        updates.push("frequency_per_week = ?");
        sql_params.push(Value::from(freq));
        updates.push("completed = MIN(completed, ?)");
        sql_params.push(Value::from(freq));
    }

    if updates.is_empty() {
        return Ok(false);
    }

    // contract change: persist the weekly rollover before the UPDATE so the
    // `MIN(completed, ?)` clamp sees the rolled-over counter. A missing
    // or foreign row still yields `Ok(false)` (the route's 404).
    let raw = conn
        .query_row(
            sql_queries::GET_GOAL_BY_ID,
            params![goal_id, user_id],
            RawGoal::from_row,
        )
        .optional()?;
    let Some(raw) = raw else {
        return Ok(false);
    };
    persist_rollover_if_changed(conn, raw, today)?;

    updates.push("updated_at = CURRENT_TIMESTAMP");
    sql_params.push(Value::from(goal_id));
    sql_params.push(Value::from(user_id));

    let sql = format!(
        "UPDATE goals SET {} WHERE id = ? AND user_id = ?",
        updates.join(", ")
    );
    let changed = conn.execute(&sql, params_from_iter(sql_params))?;
    Ok(changed > 0)
}

/// Port of `GoalsMixin.delete_goal`. `goal_completions` rows go with the
/// goal via `ON DELETE CASCADE` (connections run `PRAGMA foreign_keys=ON`).
pub fn delete_goal(conn: &Connection, user_id: i64, goal_id: i64) -> Result<bool, GoalsError> {
    let changed = conn.execute(DELETE_GOAL_SQL, params![goal_id, user_id])?;
    Ok(changed > 0)
}

/// Port of `GoalsMixin.increment_goal_progress`: log a completion for
/// `date_str` (ISO, default today).
///
/// The per-day `goal_completions` log is the source of truth for
/// duplicates (UNIQUE per goal+day). The weekly `completed` counter only
/// moves for dates inside the current period: a date from an
/// already-rolled-over week lands in the log (calendar, stats) but the
/// closed week's counter and streak are not recomputed.
pub fn increment_goal_progress(
    conn: &Connection,
    user_id: i64,
    goal_id: i64,
    date_str: Option<&str>,
) -> Result<Option<GoalProgress>, GoalsError> {
    increment_goal_progress_on(conn, user_id, goal_id, date_str, local_today())
}

fn increment_goal_progress_on(
    conn: &Connection,
    user_id: i64,
    goal_id: i64,
    date_str: Option<&str>,
    today: NaiveDate,
) -> Result<Option<GoalProgress>, GoalsError> {
    let today_str = iso(today);
    let target_date = date_str.unwrap_or(&today_str);

    let raw = conn
        .query_row(
            SELECT_GOAL_STAR_SQL,
            params![goal_id, user_id],
            RawGoal::from_row,
        )
        .optional()?;
    let Some(raw) = raw else {
        return Ok(None);
    };

    let goal = persist_rollover_if_changed(conn, raw, today)?;
    let mut current_completed = goal.completed.unwrap_or(0);
    let freq = goal.frequency_per_week.unwrap_or(0);
    let streak = goal.streak.unwrap_or(0);
    let period_start = goal.period_start.clone();

    let newly_logged = conn.execute(
        INSERT_COMPLETION_SQL,
        params![user_id, goal_id, target_date],
    )? == 1;

    // Python: `bool(period_start) and target_date >= period_start`
    // (lexicographic compare on ISO dates; empty/NULL period_start is
    // falsy).
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

    conn.execute(
        PROGRESS_UPDATE_SQL,
        params![
            current_completed,
            streak,
            period_start,
            last_completed_date,
            goal_id,
            user_id
        ],
    )?;

    let updated = conn
        .query_row(
            PROGRESS_REFRESH_SQL,
            params![goal_id, user_id],
            RawGoal::from_row,
        )
        .optional()?;
    let Some(updated) = updated else {
        return Ok(None);
    };

    Ok(Some(GoalProgress {
        goal: updated.into_goal(&today_str),
        already_logged: !newly_logged,
        logged_date: target_date.to_string(),
    }))
}

/// Port of `GoalsMixin.get_goal_completions`. When either bound is absent
/// — or empty, matching the route's Python falsy check — BOTH default to
/// the last 90 days ending today. Returns `[]` for nonexistent/foreign
/// goals (the 200-`[]` quirk, `contract/DECISIONS.md` item 5).
pub fn get_goal_completions(
    conn: &Connection,
    user_id: i64,
    goal_id: i64,
    start_date: Option<&str>,
    end_date: Option<&str>,
) -> Result<Vec<GoalCompletion>, GoalsError> {
    get_goal_completions_on(conn, user_id, goal_id, start_date, end_date, local_today())
}

fn get_goal_completions_on(
    conn: &Connection,
    user_id: i64,
    goal_id: i64,
    start_date: Option<&str>,
    end_date: Option<&str>,
    today: NaiveDate,
) -> Result<Vec<GoalCompletion>, GoalsError> {
    // Python: `if not start_date or not end_date` — one missing (or empty)
    // bound resets both to the default window.
    let (start, end) = match (start_date, end_date) {
        (Some(start), Some(end)) if !start.is_empty() && !end.is_empty() => {
            (start.to_string(), end.to_string())
        }
        _ => (iso(today - chrono::Duration::days(90)), iso(today)),
    };

    let mut stmt = conn.prepare(SELECT_COMPLETIONS_SQL)?;
    let rows = stmt.query_map(params![user_id, goal_id, start, end], |row| {
        Ok(GoalCompletion {
            date: row.get("date")?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

// --- Tests ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::bootstrap::SelfHostSeed;
    use serde_json::json;

    /// Fresh bootstrapped DB (full legacy `init_database` port) plus a
    /// connection with the `_connect` defaults (foreign_keys=ON).
    fn test_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("goals.db");
        let path = path.to_str().unwrap();
        crate::db::bootstrap(path, &SelfHostSeed::default()).unwrap();
        let conn = crate::db::connect(path).unwrap();
        (dir, conn)
    }

    /// Id of the seeded self-host user (always 1 on a fresh DB).
    fn default_user(conn: &Connection) -> i64 {
        conn.query_row(
            "SELECT id FROM users WHERE google_id = 'selfhost_default_user'",
            [],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[allow(clippy::too_many_arguments)] // test seed helper mirrors the column list
    fn seed_goal(
        conn: &Connection,
        user_id: i64,
        title: &str,
        freq: i64,
        completed: i64,
        streak: i64,
        period_start: Option<&str>,
        created_at: &str,
    ) {
        conn.execute(
            "INSERT INTO goals (user_id, title, description, frequency_per_week, \
             completed, streak, period_start, created_at, updated_at) \
             VALUES (?, ?, '', ?, ?, ?, ?, ?, '2020-01-01 00:00:00')",
            params![
                user_id,
                title,
                freq,
                completed,
                streak,
                period_start,
                created_at
            ],
        )
        .unwrap();
    }

    fn db_column<T: rusqlite::types::FromSql>(conn: &Connection, goal_id: i64, col: &str) -> T {
        conn.query_row(
            &format!("SELECT {col} FROM goals WHERE id = ?"),
            [goal_id],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn today() -> NaiveDate {
        local_today()
    }

    fn monday() -> String {
        week_start_iso(today())
    }

    // -- week_start_iso -------------------------------------------------

    #[test]
    fn week_start_iso_matches_python_weekday_semantics() {
        // Python: date - timedelta(days=date.weekday()), Monday=0.
        let cases = [
            ("2026-08-10", "2026-08-10"), // Monday maps to itself
            ("2026-08-15", "2026-08-10"), // Saturday
            ("2026-08-16", "2026-08-10"), // Sunday
            ("2026-08-17", "2026-08-17"), // next Monday
            ("2020-01-01", "2019-12-30"), // year boundary
        ];
        for (input, expected) in cases {
            let date = NaiveDate::parse_from_str(input, "%Y-%m-%d").unwrap();
            assert_eq!(week_start_iso(date), expected, "week start of {input}");
        }
    }

    // -- create_goal ----------------------------------------------------

    #[test]
    fn create_goal_trims_and_seeds_zeroed_week() {
        let (_dir, conn) = test_db();
        let user_id = default_user(&conn);
        let id = create_goal(&conn, user_id, "  progress  ", "  desc  ", 4).unwrap();
        let goal = get_goal_by_id(&conn, user_id, id).unwrap().unwrap();
        assert_eq!(goal.title, "progress");
        assert_eq!(goal.description.as_deref(), Some("desc"));
        assert_eq!(goal.frequency_per_week, Some(4));
        assert_eq!(goal.completed, Some(0));
        assert_eq!(goal.streak, Some(0));
        assert_eq!(goal.period_start.as_deref(), Some(monday().as_str()));
        assert_eq!(goal.last_completed_date, None);
        assert!(!goal.already_completed_today);
    }

    #[test]
    fn create_goal_validation_messages_match_python_valueerrors() {
        // Messages cross-checked against api/database_goals.py (see the
        // module cross-check script run recorded in the test comments).
        let (_dir, conn) = test_db();
        let user_id = default_user(&conn);
        let err = create_goal(&conn, 0, "x", "", 3).unwrap_err();
        assert_eq!(err.to_string(), "user_id must be a positive integer");
        let err = create_goal(&conn, user_id, "   ", "", 3).unwrap_err();
        assert_eq!(err.to_string(), "Title is required and cannot be empty");
        for bad_freq in [0, 8, -1] {
            let err = create_goal(&conn, user_id, "x", "", bad_freq).unwrap_err();
            assert_eq!(
                err.to_string(),
                "frequency_per_week must be between 1 and 7"
            );
        }
    }

    // -- rollover (projected on read; contract change) -------------------------------
    //
    // Expected values pinned from running api/database_goals.py
    // (api/venv/bin/python) against an identically-seeded database:
    //   met        freq=3 completed=3 streak=5 → streak 6, completed 0
    //   missed     freq=3 completed=2 streak=5 → streak 0, completed 0
    //   overmet    freq=1 completed=5 streak=0 → streak 1, completed 0
    //   null-period freq=7 completed=0 streak=9, period_start NULL
    //                                          → streak 0, completed 0
    //   current    freq=2 completed=2 streak=4, period_start=this Monday
    //                                          → untouched
    // and get_goals order (created_at DESC):
    //   ['current', 'null-period', 'overmet', 'missed', 'met']

    fn seed_rollover_fixture(conn: &Connection, user_id: i64) {
        seed_goal(
            conn,
            user_id,
            "met",
            3,
            3,
            5,
            Some("2020-01-06"),
            "2020-01-01 00:00:00",
        );
        seed_goal(
            conn,
            user_id,
            "missed",
            3,
            2,
            5,
            Some("2020-01-06"),
            "2020-01-02 00:00:00",
        );
        seed_goal(
            conn,
            user_id,
            "overmet",
            1,
            5,
            0,
            Some("2020-01-06"),
            "2020-01-03 00:00:00",
        );
        seed_goal(
            conn,
            user_id,
            "null-period",
            7,
            0,
            9,
            None,
            "2020-01-04 00:00:00",
        );
        let this_monday = monday();
        seed_goal(
            conn,
            user_id,
            "current",
            2,
            2,
            4,
            Some(this_monday.as_str()),
            "2020-01-05 00:00:00",
        );
    }

    #[test]
    fn get_goals_projects_weekly_rollover_without_writing() {
        let (_dir, conn) = test_db();
        let user_id = default_user(&conn);
        seed_rollover_fixture(&conn, user_id);

        let goals = get_goals(&conn, user_id).unwrap();
        let titles: Vec<&str> = goals.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(
            titles,
            ["current", "null-period", "overmet", "missed", "met"],
            "created_at DESC ordering"
        );

        let this_monday = monday();
        let expect = [
            // (title, completed, streak, period_start)
            ("current", 2, 4),
            ("null-period", 0, 0),
            ("overmet", 0, 1),
            ("missed", 0, 0),
            ("met", 0, 6),
        ];
        for ((title, completed, streak), goal) in expect.iter().zip(&goals) {
            assert_eq!(goal.title, *title);
            assert_eq!(goal.completed, Some(*completed), "{title} completed");
            assert_eq!(goal.streak, Some(*streak), "{title} streak");
            assert_eq!(
                goal.period_start.as_deref(),
                Some(this_monday.as_str()),
                "{title} period_start"
            );
            assert!(!goal.already_completed_today, "{title}");
        }

        // contract change: GET must NOT write. The raw rows keep
        // their seeded completed/streak values and every updated_at stays
        // on the seeded sentinel — only the response carries the projected
        // rollover values.
        let raw_expect = [
            // (title, seeded completed, seeded streak)
            ("current", 2i64, 4i64),
            ("null-period", 0, 9),
            ("overmet", 5, 0),
            ("missed", 2, 5),
            ("met", 3, 5),
        ];
        for ((title, completed, streak), goal) in raw_expect.iter().zip(&goals) {
            assert_eq!(goal.title, *title);
            let db_completed: i64 = db_column(&conn, goal.id, "completed");
            let db_streak: i64 = db_column(&conn, goal.id, "streak");
            let db_updated: String = db_column(&conn, goal.id, "updated_at");
            assert_eq!(db_completed, *completed, "{title}: raw completed intact");
            assert_eq!(db_streak, *streak, "{title}: raw streak intact");
            assert_eq!(
                db_updated, "2020-01-01 00:00:00",
                "{title}: GET must not bump updated_at"
            );
        }
    }

    #[test]
    fn get_goal_by_id_rolls_over_and_reports_already_completed_today() {
        let (_dir, conn) = test_db();
        let user_id = default_user(&conn);
        seed_goal(
            &conn,
            user_id,
            "old",
            3,
            3,
            1,
            Some("2020-01-06"),
            "2020-01-01 00:00:00",
        );
        let goal = get_goal_by_id(&conn, user_id, 1).unwrap().unwrap();
        assert_eq!(goal.completed, Some(0));
        assert_eq!(goal.streak, Some(2));
        assert_eq!(goal.period_start.as_deref(), Some(monday().as_str()));

        // Missing / foreign rows are None.
        assert!(get_goal_by_id(&conn, user_id, 999).unwrap().is_none());
        assert!(get_goal_by_id(&conn, user_id + 1, 1).unwrap().is_none());

        // last_completed_date == today → already_completed_today.
        conn.execute(
            "UPDATE goals SET last_completed_date = ? WHERE id = 1",
            [iso(today())],
        )
        .unwrap();
        let goal = get_goal_by_id(&conn, user_id, 1).unwrap().unwrap();
        assert!(goal.already_completed_today);
    }

    // -- update_goal ------------------------------------------------------

    #[test]
    fn update_goal_returns_false_for_no_fields_and_missing_row() {
        let (_dir, conn) = test_db();
        let user_id = default_user(&conn);
        let id = create_goal(&conn, user_id, "g", "", 3).unwrap();
        // No fields → false (the route 400s "No fields to update" before
        // calling; the guard stays for direct callers).
        assert!(!update_goal(&conn, user_id, id, None, None, None).unwrap());
        // Missing row → the same false.
        assert!(!update_goal(&conn, user_id, 999, Some("x"), None, None).unwrap());
        // Foreign row → the same false.
        assert!(!update_goal(&conn, user_id + 1, id, Some("x"), None, None).unwrap());
    }

    #[test]
    fn update_goal_frequency_validation_and_completed_clamp() {
        let (_dir, conn) = test_db();
        let user_id = default_user(&conn);
        let id = create_goal(&conn, user_id, "g", "", 4).unwrap();
        for bad_freq in [0, 8] {
            let err = update_goal(&conn, user_id, id, None, None, Some(bad_freq)).unwrap_err();
            assert_eq!(
                err.to_string(),
                "frequency_per_week must be between 1 and 7"
            );
        }
        // completed=3 then freq lowered to 1 → MIN(completed, 1) clamps.
        conn.execute("UPDATE goals SET completed = 3 WHERE id = ?", [id])
            .unwrap();
        assert!(update_goal(&conn, user_id, id, None, None, Some(1)).unwrap());
        assert_eq!(db_column::<i64>(&conn, id, "completed"), 1);
        assert_eq!(db_column::<i64>(&conn, id, "frequency_per_week"), 1);
    }

    #[test]
    fn update_goal_rolls_over_stale_week_before_clamping() {
        // Regression test for the latent stale-week clamp bug: an
        // over-met goal from a closed week (freq=5, completed=5, stale
        // period) must roll over BEFORE a frequency decrease clamps
        // `completed` — MIN runs against the fresh week's 0, not the stale
        // counter (the old rollover-during-reads code masked this only because the
        // UI happened to GET first).
        let (_dir, conn) = test_db();
        let user_id = default_user(&conn);
        seed_goal(
            &conn,
            user_id,
            "stale-overmet",
            5,
            5,
            3,
            Some("2020-01-06"),
            "2020-01-02 00:00:00",
        );
        let pinned = NaiveDate::from_ymd_opt(2026, 8, 15).unwrap();
        assert!(update_goal_on(&conn, user_id, 1, None, None, Some(2), pinned).unwrap());
        assert_eq!(
            db_column::<i64>(&conn, 1, "completed"),
            0,
            "clamp must see the rolled-over counter, not the stale 5"
        );
        assert_eq!(db_column::<i64>(&conn, 1, "frequency_per_week"), 2);
        assert_eq!(
            db_column::<i64>(&conn, 1, "streak"),
            4,
            "closed week met freq=5, so the rollover increments the streak"
        );
        assert_eq!(
            db_column::<String>(&conn, 1, "period_start"),
            "2026-08-10",
            "rollover repointed period_start to the pinned week's Monday"
        );
    }

    #[test]
    fn update_goal_rejects_blank_title_like_create() {
        // contract change (contract/DECISIONS.md item 7): blank
        // titles no longer write '' — same ValueError as create_goal.
        let (_dir, conn) = test_db();
        let user_id = default_user(&conn);
        let id = create_goal(&conn, user_id, "g", "", 3).unwrap();
        for blank in ["", "   ", "\t\n"] {
            let err = update_goal(&conn, user_id, id, Some(blank), None, None).unwrap_err();
            assert_eq!(
                err.to_string(),
                "Title is required and cannot be empty",
                "title {blank:?}"
            );
        }
        // The rejected update must not have touched the row.
        assert_eq!(db_column::<String>(&conn, id, "title"), "g");
        // Non-blank titles and descriptions still trim.
        assert!(update_goal(&conn, user_id, id, Some("  t  "), None, None).unwrap());
        assert_eq!(db_column::<String>(&conn, id, "title"), "t");
        assert!(update_goal(&conn, user_id, id, None, Some("  d  "), None).unwrap());
        assert_eq!(db_column::<String>(&conn, id, "description"), "d");
    }

    // -- delete_goal ------------------------------------------------------

    #[test]
    fn delete_goal_cascades_completions_and_reports_missing() {
        let (_dir, conn) = test_db();
        let user_id = default_user(&conn);
        let id = create_goal(&conn, user_id, "g", "", 3).unwrap();
        increment_goal_progress(&conn, user_id, id, None)
            .unwrap()
            .unwrap();
        assert!(delete_goal(&conn, user_id, id).unwrap());
        assert!(!delete_goal(&conn, user_id, id).unwrap());
        let left: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM goal_completions WHERE goal_id = ?",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(left, 0, "ON DELETE CASCADE must clear the completion log");
    }

    // -- increment_goal_progress ------------------------------------------
    //
    // Pinned from the Python cross-check on an identically-seeded DB
    // (freq=4 goal, run date 2026-08-15, Monday 2026-08-10):
    //   today#1: completed=1 already_logged=False logged_date=today
    //            already_completed_today=True last_completed_date=today
    //   today#2: completed=1 already_logged=True
    //   monday : completed=2 already_logged=False logged_date=<monday>
    //            already_completed_today=True last_completed_date=today
    //   old    : completed=2 already_logged=False logged_date=2020-01-01
    //            already_completed_today=True last_completed_date=today
    //   missing: None

    #[test]
    fn increment_progress_today_then_duplicate() {
        let (_dir, conn) = test_db();
        let user_id = default_user(&conn);
        let id = create_goal(&conn, user_id, "progress", "desc", 4).unwrap();
        let today_str = iso(today());

        let first = increment_goal_progress(&conn, user_id, id, None)
            .unwrap()
            .unwrap();
        assert_eq!(first.goal.completed, Some(1));
        assert!(!first.already_logged);
        assert_eq!(first.logged_date, today_str);
        assert!(first.goal.already_completed_today);
        assert_eq!(
            first.goal.last_completed_date.as_deref(),
            Some(today_str.as_str())
        );

        let second = increment_goal_progress(&conn, user_id, id, None)
            .unwrap()
            .unwrap();
        assert_eq!(
            second.goal.completed,
            Some(1),
            "duplicate day must not count"
        );
        assert!(second.already_logged);

        assert!(
            increment_goal_progress(&conn, user_id, 999, None)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn increment_progress_backdate_within_week_counts_and_keeps_today_lock() {
        let (_dir, conn) = test_db();
        let user_id = default_user(&conn);
        let id = create_goal(&conn, user_id, "progress", "desc", 4).unwrap();
        let today_str = iso(today());
        let this_monday = monday();

        increment_goal_progress(&conn, user_id, id, None)
            .unwrap()
            .unwrap();
        let back = increment_goal_progress(&conn, user_id, id, Some(this_monday.as_str()))
            .unwrap()
            .unwrap();
        if this_monday == today_str {
            // Today IS Monday: the backdate collides with the earlier log.
            assert!(back.already_logged);
            assert_eq!(back.goal.completed, Some(1));
        } else {
            assert!(!back.already_logged);
            assert_eq!(back.goal.completed, Some(2));
        }
        assert_eq!(back.logged_date, this_monday);
        // max() keeps today's completion lock.
        assert_eq!(
            back.goal.last_completed_date.as_deref(),
            Some(today_str.as_str())
        );
        assert!(back.goal.already_completed_today);
    }

    #[test]
    fn increment_progress_out_of_period_logs_without_counting() {
        let (_dir, conn) = test_db();
        let user_id = default_user(&conn);
        let id = create_goal(&conn, user_id, "progress", "desc", 4).unwrap();
        increment_goal_progress(&conn, user_id, id, None)
            .unwrap()
            .unwrap();

        let old = increment_goal_progress(&conn, user_id, id, Some("2020-01-01"))
            .unwrap()
            .unwrap();
        assert!(!old.already_logged, "old date still lands in the log");
        assert_eq!(
            old.goal.completed,
            Some(1),
            "closed week's counter must not move"
        );
        assert_eq!(old.logged_date, "2020-01-01");
        // max("2020-01-01", today) — today's lock survives.
        assert_eq!(
            old.goal.last_completed_date.as_deref(),
            Some(iso(today()).as_str())
        );
        assert!(old.goal.already_completed_today);

        let logged: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM goal_completions WHERE goal_id = ? AND date = '2020-01-01'",
                [id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(logged, 1);
    }

    #[test]
    fn increment_progress_never_exceeds_frequency() {
        let (_dir, conn) = test_db();
        let user_id = default_user(&conn);
        let id = create_goal(&conn, user_id, "cap", "", 1).unwrap();
        increment_goal_progress(&conn, user_id, id, Some(monday().as_str()))
            .unwrap()
            .unwrap();
        let capped = increment_goal_progress(&conn, user_id, id, None)
            .unwrap()
            .unwrap();
        assert_eq!(
            capped.goal.completed,
            Some(1),
            "counter caps at frequency_per_week"
        );
    }

    #[test]
    fn increment_progress_out_of_period_only_sets_last_completed_when_latest() {
        // Fresh goal, ONLY a backdated out-of-period log: Python's
        // max("" , "2020-01-01") stores the old date and
        // already_completed_today stays false.
        let (_dir, conn) = test_db();
        let user_id = default_user(&conn);
        let id = create_goal(&conn, user_id, "only-old", "", 2).unwrap();
        let old = increment_goal_progress(&conn, user_id, id, Some("2020-01-01"))
            .unwrap()
            .unwrap();
        assert_eq!(old.goal.completed, Some(0));
        assert_eq!(old.goal.last_completed_date.as_deref(), Some("2020-01-01"));
        assert!(!old.goal.already_completed_today);
        assert!(!old.already_logged);
    }

    #[test]
    fn increment_progress_rolls_over_stale_goal_first() {
        // Pinned from Python: "missed"-shaped goal (freq=3, completed=2,
        // streak=5, stale period) → rollover resets to 0/0, then today's
        // log makes completed=1, streak stays 0.
        let (_dir, conn) = test_db();
        let user_id = default_user(&conn);
        seed_goal(
            &conn,
            user_id,
            "missed",
            3,
            2,
            5,
            Some("2020-01-06"),
            "2020-01-02 00:00:00",
        );
        let out = increment_goal_progress(&conn, user_id, 1, None)
            .unwrap()
            .unwrap();
        assert_eq!(out.goal.completed, Some(1));
        assert_eq!(out.goal.streak, Some(0));
        assert_eq!(out.goal.period_start.as_deref(), Some(monday().as_str()));
    }

    // -- get_goal_completions ---------------------------------------------

    #[test]
    fn completions_default_window_and_explicit_range() {
        let (_dir, conn) = test_db();
        let user_id = default_user(&conn);
        let id = create_goal(&conn, user_id, "g", "", 3).unwrap();
        let d10 = iso(today() - chrono::Duration::days(10));
        let d100 = iso(today() - chrono::Duration::days(100));
        let today_str = iso(today());
        for d in [&d100, &d10, &today_str] {
            conn.execute(
                "INSERT OR IGNORE INTO goal_completions (user_id, goal_id, date) VALUES (?, ?, ?)",
                params![user_id, id, d],
            )
            .unwrap();
        }

        // Default 90-day window: 100-days-ago excluded, ascending order.
        let dates = |rows: Vec<GoalCompletion>| -> Vec<String> {
            rows.into_iter().map(|row| row.date).collect()
        };
        let got = get_goal_completions(&conn, user_id, id, None, None).unwrap();
        assert_eq!(dates(got), vec![d10.clone(), today_str.clone()]);

        // One bound missing or empty → both reset to the default window
        // (Python's `if not start_date or not end_date`).
        let got = get_goal_completions(&conn, user_id, id, Some(d100.as_str()), None).unwrap();
        assert_eq!(dates(got), vec![d10.clone(), today_str.clone()]);
        let got = get_goal_completions(&conn, user_id, id, Some(""), Some("")).unwrap();
        assert_eq!(dates(got), vec![d10.clone(), today_str.clone()]);

        // Explicit range includes everything.
        let got = get_goal_completions(&conn, user_id, id, Some("2020-01-01"), Some("2099-12-31"))
            .unwrap();
        assert_eq!(dates(got), vec![d100, d10, today_str]);

        // Nonexistent goal → empty list, not an error (200-[] quirk).
        let got = get_goal_completions(&conn, user_id, 999, None, None).unwrap();
        assert!(got.is_empty());
    }

    // -- JSON shape -------------------------------------------------------

    #[test]
    fn serialized_shapes_match_flask_contract_fixtures() {
        let (_dir, conn) = test_db();
        let user_id = default_user(&conn);
        let id = create_goal(&conn, user_id, "Meditate", "10 minutes", 3).unwrap();

        // Goal shape: contract/fixtures/goals/goal-get-200.json.
        let goal = get_goal_by_id(&conn, user_id, id).unwrap().unwrap();
        let value = serde_json::to_value(&goal).unwrap();
        let keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let mut expected = vec![
            "already_completed_today",
            "completed",
            "created_at",
            "description",
            "frequency_per_week",
            "id",
            "last_completed_date",
            "period_start",
            "streak",
            "title",
            "updated_at",
            "user_id",
        ];
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        expected.sort_unstable();
        assert_eq!(sorted, expected);
        assert_eq!(value["title"], json!("Meditate"));
        assert_eq!(value["description"], json!("10 minutes"));
        assert_eq!(value["last_completed_date"], json!(null));
        assert_eq!(value["already_completed_today"], json!(false));

        // Progress shape: goal-progress-200-today.json adds exactly
        // already_logged and logged_date (flattened alongside the row).
        let progress = increment_goal_progress(&conn, user_id, id, None)
            .unwrap()
            .unwrap();
        let value = serde_json::to_value(&progress).unwrap();
        let object = value.as_object().unwrap();
        assert_eq!(object.len(), 14);
        assert_eq!(value["already_logged"], json!(false));
        assert_eq!(value["logged_date"], json!(iso(today())));
        assert_eq!(value["already_completed_today"], json!(true));
        assert_eq!(value["completed"], json!(1));

        // Completion shape: bare {"date": ...} objects.
        let completions = get_goal_completions(&conn, user_id, id, None, None).unwrap();
        let value = serde_json::to_value(&completions).unwrap();
        assert_eq!(value, json!([{ "date": iso(today()) }]));
    }

    // -- date-pinned rollover determinism ---------------------------------

    #[test]
    fn rollover_is_deterministic_for_a_pinned_today() {
        // Same seed as the Python cross-check, but driven through the
        // `_on` internals with today pinned to 2026-08-15 (Saturday,
        // Monday = 2026-08-10) so the assertions are date-independent.
        let (_dir, conn) = test_db();
        let user_id = default_user(&conn);
        seed_goal(
            &conn,
            user_id,
            "met",
            3,
            3,
            5,
            Some("2020-01-06"),
            "2020-01-01 00:00:00",
        );
        let pinned = NaiveDate::from_ymd_opt(2026, 8, 15).unwrap();
        let goals = get_goals_on(&conn, user_id, pinned).unwrap();
        assert_eq!(goals.len(), 1);
        assert_eq!(goals[0].streak, Some(6));
        assert_eq!(goals[0].completed, Some(0));
        assert_eq!(goals[0].period_start.as_deref(), Some("2026-08-10"));

        // contract change: every read is pure — the stale row is projected the same
        // way on each read and the seeded sentinel timestamp survives.
        let updated_at: String = db_column(&conn, goals[0].id, "updated_at");
        assert_eq!(updated_at, "2020-01-01 00:00:00", "read must not write");
        let again = get_goals_on(&conn, user_id, pinned).unwrap();
        assert_eq!(again[0].streak, Some(6));
        assert_eq!(again[0].completed, Some(0));
        let updated_at_2: String = db_column(&conn, goals[0].id, "updated_at");
        assert_eq!(updated_at, updated_at_2, "repeat reads stay pure");
    }
}
