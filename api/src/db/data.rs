//! SQLite queries for the versioned JSON export/import family (v0.6.0,
//! `GET /api/export/data` / `POST /api/import/data`). Rust-native — this
//! family never existed in Flask; the hand-authored fixtures in
//! `contract/fixtures/data/` are the contract (DECISIONS.md 2026-08-24).
//!
//! Semantics pinned by the contract:
//! - The v1 envelope carries NO ids and NO user_id (autoincrement ids are
//!   not portable across instances). Entry selections are denormalized by
//!   exact name as `{group_name, option_name}` pairs.
//! - Export is a pure read with deterministic order: entries `date ASC,
//!   id ASC`, goals `created_at ASC, id ASC`, selections in the
//!   `GET /api/mood/{id}/selections` order (group name, then option name),
//!   completions ascending. Goal weekly state is exported with the weekly
//!   rollover *projected* exactly as the goals GETs project it — nothing is
//!   persisted.
//! - Import merges inside ONE transaction, rows in file order, and skips
//!   duplicates: entry dup key `(date, content)`, goal dup key `title` (a
//!   duplicate goal is skipped whole — its completions never touch existing
//!   streak state). The dup sets update after each insert, so intra-file
//!   duplicates skip too. Groups/options resolve-or-create by exact name
//!   through a preloaded name→id cache (`idx_groups_user_name` is unique;
//!   `group_options` has no unique index — the cache IS the dedupe).
//!   Timestamps are preserved via `COALESCE(?, CURRENT_TIMESTAMP)`; goal
//!   weekly state is written raw (the rollover projection self-heals stale
//!   periods on the next goals read). NO achievement checks and NO
//!   activity_log writes — import is a pure restore.
//!
//! All functions take `&Connection`; callers run them under
//! `tokio::task::spawn_blocking`.

use std::collections::{HashMap, HashSet};

use chrono::{Datelike, NaiveDate};
use rusqlite::{Connection, params};
use serde::Serialize;

use super::common::{DatabaseError, MoodValue};

// --- SQL ---------------------------------------------------------------------

/// Export order is contract: ascending by the stored date string
/// (byte-lexicographic, like every other raw-date comparison in the app),
/// `id ASC` pinning insertion order within a day.
const EXPORT_ENTRIES_SQL: &str = "SELECT id, date, mood, content, created_at, updated_at \
     FROM mood_entries WHERE user_id = ? ORDER BY date ASC, id ASC";

/// Denormalized selection names for one entry, in the
/// `GET /api/mood/{id}/selections` order (group name, then option name).
const EXPORT_SELECTIONS_SQL: &str = "SELECT g.name AS group_name, go.name AS option_name \
     FROM entry_selections es \
     JOIN group_options go ON es.option_id = go.id \
     JOIN groups g ON go.group_id = g.id \
     WHERE es.entry_id = ? \
     ORDER BY g.name, go.name";

/// Creation order (`created_at ASC`), `id ASC` pinning same-second rows —
/// the mirror of `sql_queries::GET_GOALS_BY_USER`'s DESC list.
const EXPORT_GOALS_SQL: &str = "SELECT id, title, description, frequency_per_week, completed, \
            streak, period_start, last_completed_date, created_at, updated_at \
     FROM goals WHERE user_id = ? ORDER BY created_at ASC, id ASC";

const EXPORT_COMPLETIONS_SQL: &str = "SELECT date FROM goal_completions \
     WHERE user_id = ? AND goal_id = ? ORDER BY date ASC";

const PRELOAD_ENTRY_KEYS_SQL: &str = "SELECT date, content FROM mood_entries WHERE user_id = ?";

const PRELOAD_GOAL_TITLES_SQL: &str = "SELECT title FROM goals WHERE user_id = ?";

const PRELOAD_GROUPS_SQL: &str = "SELECT id, name FROM groups WHERE user_id = ?";

/// `ORDER BY go.id ASC` + first-wins cache insertion make duplicate option
/// names (legal — `group_options` has no unique index) resolve
/// deterministically to the oldest row.
const PRELOAD_OPTIONS_SQL: &str = "SELECT go.id, go.group_id, go.name \
     FROM group_options go JOIN groups g ON go.group_id = g.id \
     WHERE g.user_id = ? ORDER BY go.id ASC";

/// File timestamps preserved; absent ones take `CURRENT_TIMESTAMP`.
const IMPORT_ENTRY_SQL: &str = "INSERT INTO mood_entries \
     (user_id, date, mood, content, created_at, updated_at) \
     VALUES (?, ?, ?, ?, COALESCE(?, CURRENT_TIMESTAMP), COALESCE(?, CURRENT_TIMESTAMP))";

const IMPORT_GROUP_SQL: &str = "INSERT INTO groups (name, user_id) VALUES (?, ?)";

const IMPORT_OPTION_SQL: &str = "INSERT INTO group_options (group_id, name) VALUES (?, ?)";

const IMPORT_SELECTION_SQL: &str =
    "INSERT INTO entry_selections (entry_id, option_id) VALUES (?, ?)";

/// Weekly state (`completed`/`streak`/`period_start`) is written raw from
/// the file — never recomputed at import; `project_rollover` self-heals
/// stale periods on the next goals read.
const IMPORT_GOAL_SQL: &str = "INSERT INTO goals \
     (user_id, title, description, frequency_per_week, completed, streak, \
      period_start, last_completed_date, created_at, updated_at) \
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, \
             COALESCE(?, CURRENT_TIMESTAMP), COALESCE(?, CURRENT_TIMESTAMP))";

const IMPORT_COMPLETION_SQL: &str =
    "INSERT OR IGNORE INTO goal_completions (user_id, goal_id, date) VALUES (?, ?, ?)";

// --- Wire shapes -------------------------------------------------------------

/// One denormalized selection: names, never ids (contract schema
/// `DataExportSelection`).
#[derive(Debug, Clone, Serialize)]
pub struct ExportSelection {
    pub group_name: String,
    pub option_name: String,
}

/// One exported entry (contract schema `DataExportEntry`). `mood` is a
/// [`MoodValue`] so schema-legal REAL rows export as `4.5` instead of
/// erroring; the import parser only ever produces `Int` (the schema pins an
/// integer 1..=5).
#[derive(Debug, Clone, Serialize)]
pub struct ExportEntry {
    pub date: String,
    pub mood: MoodValue,
    pub content: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub selections: Vec<ExportSelection>,
}

/// One exported goal (contract schema `DataExportGoal`). The numeric weekly
/// fields stay `Option` for read tolerance of hand-edited rows; the import
/// parser guarantees `Some` for everything the schema requires.
#[derive(Debug, Clone, Serialize)]
pub struct ExportGoal {
    pub title: String,
    pub description: Option<String>,
    pub frequency_per_week: Option<i64>,
    pub completed: Option<i64>,
    pub streak: Option<i64>,
    pub period_start: Option<String>,
    pub last_completed_date: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub completions: Vec<String>,
}

/// The `data` member of the v1 envelope (the route adds `schema_version`,
/// `exported_at`, `app_version` around it).
#[derive(Debug, Clone, Serialize)]
pub struct ExportData {
    pub entries: Vec<ExportEntry>,
    pub goals: Vec<ExportGoal>,
}

/// Per-category import counters (`imported + skipped` = rows in the file; a
/// skipped goal counts once, whole).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct CategoryCounts {
    pub imported: i64,
    pub skipped: i64,
}

/// The `POST /api/import/data` result (contract schema `DataImportResult`
/// minus the constant `status` key, which the route adds).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ImportCounts {
    pub entries: CategoryCounts,
    pub goals: CategoryCounts,
}

// --- Weekly-rollover projection (pure; shared with the PG twin) --------------

/// ISO date of the Monday of `date_obj`'s week (Python `weekday()` is
/// Monday=0) — duplicated from the frozen `db/goals.rs` private helper,
/// like `db/pg/goals.rs` already does.
pub(crate) fn week_start_iso(date_obj: NaiveDate) -> String {
    let start =
        date_obj - chrono::Duration::days(i64::from(date_obj.weekday().num_days_from_monday()));
    start.format("%Y-%m-%d").to_string()
}

/// Local "today", matching Python's naive `datetime.now()`.
pub(crate) fn local_today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

/// The pure half of the goals rollover, applied to a goal's raw weekly
/// state at export time: an in-period row passes through unchanged; a stale
/// row projects `completed` 0, `streak` recomputed (incremented when the
/// closed week met `frequency_per_week`, else reset) and `period_start`
/// repointed to the current Monday. Identical logic to
/// `db/goals.rs::project_rollover` — export must serve exactly what the
/// goals GETs serve.
pub(crate) fn project_weekly_state(
    completed: Option<i64>,
    streak: Option<i64>,
    frequency_per_week: Option<i64>,
    period_start: Option<&str>,
    today: NaiveDate,
) -> (Option<i64>, Option<i64>, Option<String>) {
    let today_start = week_start_iso(today);
    if period_start.unwrap_or("") == today_start {
        return (completed, streak, period_start.map(str::to_string));
    }
    let freq = frequency_per_week.unwrap_or(0);
    let current_completed = completed.unwrap_or(0);
    let projected_streak = if freq > 0 && current_completed >= freq {
        streak.unwrap_or(0) + 1
    } else {
        0
    };
    (Some(0), Some(projected_streak), Some(today_start))
}

// --- Export ------------------------------------------------------------------

/// Everything the user owns, in the deterministic contract order. A pure
/// read: the weekly rollover is projected onto the response, never written.
pub fn export_data(conn: &Connection, user_id: i64) -> Result<ExportData, DatabaseError> {
    export_data_on(conn, user_id, local_today())
}

fn export_data_on(
    conn: &Connection,
    user_id: i64,
    today: NaiveDate,
) -> Result<ExportData, DatabaseError> {
    let raw_entries = {
        let mut stmt = conn.prepare(EXPORT_ENTRIES_SQL)?;
        let rows = stmt.query_map(params![user_id], |row| {
            Ok((
                row.get::<_, i64>("id")?,
                ExportEntry {
                    date: row.get("date")?,
                    mood: row.get("mood")?,
                    content: row.get("content")?,
                    created_at: row.get("created_at")?,
                    updated_at: row.get("updated_at")?,
                    selections: Vec::new(),
                },
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    let mut entries = Vec::with_capacity(raw_entries.len());
    {
        let mut stmt = conn.prepare(EXPORT_SELECTIONS_SQL)?;
        for (entry_id, mut entry) in raw_entries {
            entry.selections = stmt
                .query_map(params![entry_id], |row| {
                    Ok(ExportSelection {
                        group_name: row.get("group_name")?,
                        option_name: row.get("option_name")?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            entries.push(entry);
        }
    }

    let raw_goals = {
        let mut stmt = conn.prepare(EXPORT_GOALS_SQL)?;
        let rows = stmt.query_map(params![user_id], |row| {
            Ok((
                row.get::<_, i64>("id")?,
                ExportGoal {
                    title: row.get("title")?,
                    description: row.get("description")?,
                    frequency_per_week: row.get("frequency_per_week")?,
                    completed: row.get("completed")?,
                    streak: row.get("streak")?,
                    period_start: row.get("period_start")?,
                    last_completed_date: row.get("last_completed_date")?,
                    created_at: row.get("created_at")?,
                    updated_at: row.get("updated_at")?,
                    completions: Vec::new(),
                },
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    let mut goals = Vec::with_capacity(raw_goals.len());
    {
        let mut stmt = conn.prepare(EXPORT_COMPLETIONS_SQL)?;
        for (goal_id, mut goal) in raw_goals {
            let (completed, streak, period_start) = project_weekly_state(
                goal.completed,
                goal.streak,
                goal.frequency_per_week,
                goal.period_start.as_deref(),
                today,
            );
            goal.completed = completed;
            goal.streak = streak;
            goal.period_start = period_start;
            goal.completions = stmt
                .query_map(params![user_id, goal_id], |row| row.get("date"))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            goals.push(goal);
        }
    }

    Ok(ExportData { entries, goals })
}

// --- Import ------------------------------------------------------------------

/// Merge a validated v1 file into the user's account: one transaction,
/// all-or-nothing, duplicates skipped (see module docs). The caller (route
/// layer) has already validated every row.
pub fn import_data(
    conn: &Connection,
    user_id: i64,
    data: &ExportData,
) -> Result<ImportCounts, DatabaseError> {
    let tx = conn.unchecked_transaction()?;
    let mut counts = ImportCounts::default();

    // Preload the duplicate keys and the group/option name→id cache.
    let mut entry_keys: HashSet<(String, String)> = {
        let mut stmt = tx.prepare(PRELOAD_ENTRY_KEYS_SQL)?;
        let rows = stmt.query_map(params![user_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        rows.collect::<rusqlite::Result<HashSet<_>>>()?
    };
    let mut goal_titles: HashSet<String> = {
        let mut stmt = tx.prepare(PRELOAD_GOAL_TITLES_SQL)?;
        let rows = stmt.query_map(params![user_id], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<HashSet<_>>>()?
    };
    let mut group_ids: HashMap<String, i64> = {
        let mut stmt = tx.prepare(PRELOAD_GROUPS_SQL)?;
        let rows = stmt.query_map(params![user_id], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, i64>(0)?))
        })?;
        rows.collect::<rusqlite::Result<HashMap<_, _>>>()?
    };
    let mut option_ids: HashMap<(i64, String), i64> = HashMap::new();
    {
        let mut stmt = tx.prepare(PRELOAD_OPTIONS_SQL)?;
        let rows = stmt.query_map(params![user_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        for row in rows {
            let (option_id, group_id, name) = row?;
            // First-wins: duplicate names resolve to the oldest option row.
            option_ids.entry((group_id, name)).or_insert(option_id);
        }
    }

    // Entries, in file order.
    for entry in &data.entries {
        let key = (entry.date.clone(), entry.content.clone());
        if entry_keys.contains(&key) {
            counts.entries.skipped += 1;
            continue;
        }
        let mood: rusqlite::types::Value = match entry.mood {
            MoodValue::Int(value) => rusqlite::types::Value::Integer(value),
            MoodValue::Float(value) => rusqlite::types::Value::Real(value),
        };
        tx.execute(
            IMPORT_ENTRY_SQL,
            params![
                user_id,
                entry.date,
                mood,
                entry.content,
                entry.created_at,
                entry.updated_at
            ],
        )?;
        let entry_id = tx.last_insert_rowid();
        for selection in &entry.selections {
            let group_id = match group_ids.get(&selection.group_name) {
                Some(id) => *id,
                None => {
                    tx.execute(IMPORT_GROUP_SQL, params![selection.group_name, user_id])?;
                    let id = tx.last_insert_rowid();
                    group_ids.insert(selection.group_name.clone(), id);
                    id
                }
            };
            let option_key = (group_id, selection.option_name.clone());
            let option_id = match option_ids.get(&option_key) {
                Some(id) => *id,
                None => {
                    tx.execute(IMPORT_OPTION_SQL, params![group_id, selection.option_name])?;
                    let id = tx.last_insert_rowid();
                    option_ids.insert(option_key, id);
                    id
                }
            };
            tx.execute(IMPORT_SELECTION_SQL, params![entry_id, option_id])?;
        }
        entry_keys.insert(key);
        counts.entries.imported += 1;
    }

    // Goals, in file order. A duplicate title skips the WHOLE goal — its
    // completions are ignored, existing streak state is never mutated.
    for goal in &data.goals {
        if goal_titles.contains(&goal.title) {
            counts.goals.skipped += 1;
            continue;
        }
        tx.execute(
            IMPORT_GOAL_SQL,
            params![
                user_id,
                goal.title,
                goal.description,
                goal.frequency_per_week,
                goal.completed.unwrap_or(0),
                goal.streak.unwrap_or(0),
                goal.period_start,
                goal.last_completed_date,
                goal.created_at,
                goal.updated_at
            ],
        )?;
        let goal_id = tx.last_insert_rowid();
        for date in &goal.completions {
            tx.execute(IMPORT_COMPLETION_SQL, params![user_id, goal_id, date])?;
        }
        goal_titles.insert(goal.title.clone());
        counts.goals.imported += 1;
    }

    tx.commit()?;
    Ok(counts)
}

// --- Tests -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::bootstrap::SelfHostSeed;
    use crate::db::moods;
    use serde_json::json;

    /// Fresh bootstrapped DB (default user 1, default groups 1..=3 with
    /// options 1..=27).
    fn test_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("data.db");
        let path = path.to_str().unwrap();
        crate::db::bootstrap(path, &SelfHostSeed::default()).unwrap();
        let conn = crate::db::connect(path).unwrap();
        (dir, conn)
    }

    fn count(conn: &Connection, sql: &str) -> i64 {
        conn.query_row(sql, [], |row| row.get(0)).unwrap()
    }

    #[test]
    fn week_projection_matches_goals_semantics() {
        let today = NaiveDate::from_ymd_opt(2026, 8, 15).unwrap(); // Saturday
        // In-period rows pass through untouched.
        assert_eq!(
            project_weekly_state(Some(2), Some(4), Some(3), Some("2026-08-10"), today),
            (Some(2), Some(4), Some("2026-08-10".to_string()))
        );
        // Stale + met → streak increments, counter resets.
        assert_eq!(
            project_weekly_state(Some(3), Some(5), Some(3), Some("2020-01-06"), today),
            (Some(0), Some(6), Some("2026-08-10".to_string()))
        );
        // Stale + missed → streak resets.
        assert_eq!(
            project_weekly_state(Some(2), Some(5), Some(3), Some("2020-01-06"), today),
            (Some(0), Some(0), Some("2026-08-10".to_string()))
        );
        // NULL period is stale (Python `or ""`), NULL numerics coerce to 0.
        assert_eq!(
            project_weekly_state(None, Some(9), Some(7), None, today),
            (Some(0), Some(0), Some("2026-08-10".to_string()))
        );
    }

    #[test]
    fn export_orders_deterministically_and_projects_rollover_without_writing() {
        let (_dir, conn) = test_db();
        // Entries inserted out of date order; options 1 = Emotions/happy,
        // 5 = Emotions/content.
        moods::add_mood_entry(
            &conn,
            1,
            "2025-08-03",
            5,
            "Third day.",
            Some("2025-08-03 10:00:00"),
            None,
        )
        .unwrap();
        moods::add_mood_entry(
            &conn,
            1,
            "2025-08-01",
            4,
            "Great workout day.",
            Some("2025-08-01 08:00:00"),
            Some(&[1, 5]),
        )
        .unwrap();
        moods::add_mood_entry(
            &conn,
            1,
            "2025-08-03",
            2,
            "Same-day later insert.",
            Some("2025-08-03 06:00:00"),
            None,
        )
        .unwrap();
        // Stale goal: met week (freq 3, completed 3, streak 5), sentinel
        // timestamps, completions inserted out of ascending order.
        conn.execute(
            "INSERT INTO goals (user_id, title, description, frequency_per_week, completed, \
             streak, period_start, created_at, updated_at) \
             VALUES (1, 'Meditate', '10 minutes', 3, 3, 5, '2020-01-06', \
                     '2020-01-01 00:00:00', '2020-01-01 00:00:00')",
            [],
        )
        .unwrap();
        for date in ["2020-01-08", "2020-01-06"] {
            conn.execute(
                "INSERT INTO goal_completions (user_id, goal_id, date) VALUES (1, 1, ?)",
                [date],
            )
            .unwrap();
        }

        let today = NaiveDate::from_ymd_opt(2026, 8, 15).unwrap();
        let export = export_data_on(&conn, 1, today).unwrap();

        // Entries: date ASC, id ASC — same-day rows keep insertion order.
        let dates: Vec<(&str, &str)> = export
            .entries
            .iter()
            .map(|entry| (entry.date.as_str(), entry.content.as_str()))
            .collect();
        assert_eq!(
            dates,
            [
                ("2025-08-01", "Great workout day."),
                ("2025-08-03", "Third day."),
                ("2025-08-03", "Same-day later insert."),
            ]
        );
        // Selections denormalized in group-name/option-name order:
        // content before happy.
        assert_eq!(
            serde_json::to_value(&export.entries[0].selections).unwrap(),
            json!([
                {"group_name": "Emotions", "option_name": "content"},
                {"group_name": "Emotions", "option_name": "happy"},
            ])
        );
        assert!(export.entries[1].selections.is_empty());

        // Goal: rollover projected (met week → streak 6, counter 0, current
        // Monday), completions ascending, timestamps exported verbatim.
        let goal = &export.goals[0];
        assert_eq!(goal.title, "Meditate");
        assert_eq!(goal.completed, Some(0));
        assert_eq!(goal.streak, Some(6));
        assert_eq!(goal.period_start.as_deref(), Some("2026-08-10"));
        assert_eq!(goal.completions, ["2020-01-06", "2020-01-08"]);
        assert_eq!(goal.created_at.as_deref(), Some("2020-01-01 00:00:00"));

        // Pure read: the raw row keeps its stale week and sentinel
        // updated_at.
        let (completed, streak, period_start, updated_at): (i64, i64, String, String) = conn
            .query_row(
                "SELECT completed, streak, period_start, updated_at FROM goals WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            (
                completed,
                streak,
                period_start.as_str(),
                updated_at.as_str()
            ),
            (3, 5, "2020-01-06", "2020-01-01 00:00:00"),
            "export must not write"
        );
    }

    fn sample_file() -> ExportData {
        ExportData {
            entries: vec![
                ExportEntry {
                    date: "2025-08-01".to_string(),
                    mood: MoodValue::Int(4),
                    content: "Great workout day.".to_string(),
                    created_at: Some("2025-08-01 21:15:00".to_string()),
                    updated_at: Some("2025-08-01 21:15:00".to_string()),
                    selections: vec![
                        ExportSelection {
                            group_name: "Emotions".to_string(),
                            option_name: "happy".to_string(),
                        },
                        ExportSelection {
                            group_name: "Custom Tags".to_string(),
                            option_name: "beach".to_string(),
                        },
                    ],
                },
                // Intra-file duplicate of the first entry.
                ExportEntry {
                    date: "2025-08-01".to_string(),
                    mood: MoodValue::Int(4),
                    content: "Great workout day.".to_string(),
                    created_at: None,
                    updated_at: None,
                    selections: Vec::new(),
                },
            ],
            goals: vec![ExportGoal {
                title: "Journal".to_string(),
                description: Some("Evening pages".to_string()),
                frequency_per_week: Some(5),
                completed: Some(1),
                streak: Some(0),
                period_start: Some("2025-08-04".to_string()),
                last_completed_date: Some("2025-08-05".to_string()),
                created_at: Some("2025-08-02 08:00:00".to_string()),
                updated_at: Some("2025-08-05 08:00:00".to_string()),
                // Duplicate date: INSERT OR IGNORE keeps one row.
                completions: vec!["2025-08-05".to_string(), "2025-08-05".to_string()],
            }],
        }
    }

    #[test]
    fn import_merges_resolves_names_and_skips_duplicates() {
        let (_dir, conn) = test_db();
        let file = sample_file();

        let counts = import_data(&conn, 1, &file).unwrap();
        assert_eq!(
            counts,
            ImportCounts {
                entries: CategoryCounts {
                    imported: 1,
                    skipped: 1
                },
                goals: CategoryCounts {
                    imported: 1,
                    skipped: 0
                },
            },
            "intra-file duplicate entry must skip"
        );

        // Emotions/happy resolved to the seeded option (id 1, group 1);
        // Custom Tags/beach created group 4 + option 28.
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM groups WHERE user_id = 1"),
            4
        );
        let (beach_group, beach_option): (i64, i64) = conn
            .query_row(
                "SELECT g.id, go.id FROM group_options go \
                 JOIN groups g ON go.group_id = g.id \
                 WHERE g.name = 'Custom Tags' AND go.name = 'beach'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((beach_group, beach_option), (4, 28));
        let selection_option_ids: Vec<i64> = {
            let mut stmt = conn
                .prepare(
                    "SELECT option_id FROM entry_selections WHERE entry_id = 1 ORDER BY option_id",
                )
                .unwrap();
            stmt.query_map([], |row| row.get(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        assert_eq!(
            selection_option_ids,
            [1, 28],
            "happy resolves, beach creates"
        );

        // Timestamps preserved verbatim; weekly state written raw.
        let created_at: String = conn
            .query_row(
                "SELECT created_at FROM mood_entries WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(created_at, "2025-08-01 21:15:00");
        let (period_start, completions): (String, i64) = conn
            .query_row(
                "SELECT period_start, \
                        (SELECT COUNT(*) FROM goal_completions WHERE goal_id = 1) \
                 FROM goals WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(period_start, "2025-08-04", "written raw, never recomputed");
        assert_eq!(completions, 1, "INSERT OR IGNORE deduped the completion");

        // Re-import is idempotent: everything skips, nothing mutates.
        let again = import_data(&conn, 1, &file).unwrap();
        assert_eq!(
            again,
            ImportCounts {
                entries: CategoryCounts {
                    imported: 0,
                    skipped: 2
                },
                goals: CategoryCounts {
                    imported: 0,
                    skipped: 1
                },
            }
        );
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM mood_entries"), 1);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM goals"), 1);
        assert_eq!(
            count(&conn, "SELECT COUNT(*) FROM groups WHERE user_id = 1"),
            4
        );
    }

    #[test]
    fn import_missing_timestamps_take_current_timestamp() {
        let (_dir, conn) = test_db();
        let file = ExportData {
            entries: vec![ExportEntry {
                date: "2025-08-02".to_string(),
                mood: MoodValue::Int(2),
                content: "Rough night.".to_string(),
                created_at: None,
                updated_at: None,
                selections: Vec::new(),
            }],
            goals: Vec::new(),
        };
        import_data(&conn, 1, &file).unwrap();
        let (created_at, updated_at): (String, String) = conn
            .query_row(
                "SELECT created_at, updated_at FROM mood_entries WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            created_at.len(),
            19,
            "CURRENT_TIMESTAMP default: {created_at}"
        );
        assert_eq!(
            updated_at.len(),
            19,
            "CURRENT_TIMESTAMP default: {updated_at}"
        );
    }

    #[test]
    fn import_writes_no_activity_and_no_achievements() {
        let (_dir, conn) = test_db();
        import_data(&conn, 1, &sample_file()).unwrap();
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM activity_log"), 0);
        assert_eq!(count(&conn, "SELECT COUNT(*) FROM achievements"), 0);
    }
}
