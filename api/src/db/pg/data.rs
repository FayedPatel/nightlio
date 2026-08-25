//! PostgreSQL twin of [`crate::db::data`] (the export/import queries for
//! the v0.6.0 `data` route family). Same operation set, same deterministic
//! order, same merge-skip semantics, same rows on the wire — translated
//! statement by statement per the wire-quirk policy:
//!
//! - `?` placeholders become `$n`; `last_insert_rowid()` becomes
//!   `RETURNING id`; `CURRENT_TIMESTAMP` writes become `nightlio_now()`
//!   (the 0001 baseline helper emitting SQLite's `YYYY-MM-DD HH:MM:SS` UTC
//!   text); `INSERT OR IGNORE` becomes `ON CONFLICT DO NOTHING`.
//! - Ordering: the text columns are `COLLATE "C"`, so `ORDER BY` is
//!   byte-identical to SQLite BINARY. `created_at ASC` gains `NULLS FIRST`
//!   (SQLite sorts NULLs smallest — first under ASC — while PostgreSQL
//!   defaults to NULLS LAST); `mood_entries.date` and
//!   `goal_completions.date` are NOT NULL, so their sorts need no override.
//! - `mood` is `double precision`: writes bind `f64`, reads fold integral
//!   values back to bare integers via [`util::mood_value_from_f64`].
//! - The weekly-rollover projection reuses the pure helper from the sync
//!   twin ([`data::project_weekly_state`]) so export projects exactly what
//!   the goals GETs project, on both backends.
//!
//! Functions follow the `pg/util.rs` conventions: `&impl GenericClient`
//! (`&mut` for the transactional import) so store ops share one
//! checked-out connection.

use std::collections::{HashMap, HashSet};

use tokio_postgres::GenericClient;

use super::util;
use crate::db::DatabaseError;
use crate::db::common::MoodValue;
use crate::db::data::{self, ExportData, ExportEntry, ExportGoal, ExportSelection, ImportCounts};

// --- SQL (dialect-translated from the `db/data.rs` constants) ----------------

const EXPORT_ENTRIES_SQL: &str = "SELECT id, date, mood, content, created_at, updated_at \
     FROM mood_entries WHERE user_id = $1 ORDER BY date ASC, id ASC";

const EXPORT_SELECTIONS_SQL: &str = "SELECT g.name AS group_name, go.name AS option_name \
     FROM entry_selections es \
     JOIN group_options go ON es.option_id = go.id \
     JOIN groups g ON go.group_id = g.id \
     WHERE es.entry_id = $1 \
     ORDER BY g.name, go.name";

/// `NULLS FIRST`: SQLite sorts NULL `created_at` first under ASC.
const EXPORT_GOALS_SQL: &str = "SELECT id, title, description, frequency_per_week, completed, \
            streak, period_start, last_completed_date, created_at, updated_at \
     FROM goals WHERE user_id = $1 ORDER BY created_at ASC NULLS FIRST, id ASC";

const EXPORT_COMPLETIONS_SQL: &str = "SELECT date FROM goal_completions \
     WHERE user_id = $1 AND goal_id = $2 ORDER BY date ASC";

const PRELOAD_ENTRY_KEYS_SQL: &str = "SELECT date, content FROM mood_entries WHERE user_id = $1";

const PRELOAD_GOAL_TITLES_SQL: &str = "SELECT title FROM goals WHERE user_id = $1";

const PRELOAD_GROUPS_SQL: &str = "SELECT id, name FROM groups WHERE user_id = $1";

const PRELOAD_OPTIONS_SQL: &str = "SELECT go.id, go.group_id, go.name \
     FROM group_options go JOIN groups g ON go.group_id = g.id \
     WHERE g.user_id = $1 ORDER BY go.id ASC";

const IMPORT_ENTRY_SQL: &str = "INSERT INTO mood_entries \
     (user_id, date, mood, content, created_at, updated_at) \
     VALUES ($1, $2, $3, $4, COALESCE($5, nightlio_now()), COALESCE($6, nightlio_now())) \
     RETURNING id";

const IMPORT_GROUP_SQL: &str = "INSERT INTO groups (name, user_id) VALUES ($1, $2) RETURNING id";

const IMPORT_OPTION_SQL: &str =
    "INSERT INTO group_options (group_id, name) VALUES ($1, $2) RETURNING id";

const IMPORT_SELECTION_SQL: &str =
    "INSERT INTO entry_selections (entry_id, option_id) VALUES ($1, $2)";

const IMPORT_GOAL_SQL: &str = "INSERT INTO goals \
     (user_id, title, description, frequency_per_week, completed, streak, \
      period_start, last_completed_date, created_at, updated_at) \
     VALUES ($1, $2, $3, $4, $5, $6, $7, $8, \
             COALESCE($9, nightlio_now()), COALESCE($10, nightlio_now())) \
     RETURNING id";

const IMPORT_COMPLETION_SQL: &str = "INSERT INTO goal_completions (user_id, goal_id, date) \
     VALUES ($1, $2, $3) ON CONFLICT DO NOTHING";

// --- Export ------------------------------------------------------------------

/// Twin of `data::export_data` — a pure read in the deterministic contract
/// order, with the weekly rollover projected onto the response.
pub async fn export_data(
    client: &impl GenericClient,
    user_id: i64,
) -> Result<ExportData, DatabaseError> {
    export_data_on(client, user_id, data::local_today()).await
}

async fn export_data_on(
    client: &impl GenericClient,
    user_id: i64,
    today: chrono::NaiveDate,
) -> Result<ExportData, DatabaseError> {
    let entry_rows = client
        .query(EXPORT_ENTRIES_SQL, &[&user_id])
        .await
        .map_err(util::db_error)?;
    let mut entries = Vec::with_capacity(entry_rows.len());
    for row in &entry_rows {
        let entry_id: i64 = row.try_get("id").map_err(util::db_error)?;
        let selections = client
            .query(EXPORT_SELECTIONS_SQL, &[&entry_id])
            .await
            .map_err(util::db_error)?
            .iter()
            .map(|selection| {
                Ok(ExportSelection {
                    group_name: selection.try_get("group_name")?,
                    option_name: selection.try_get("option_name")?,
                })
            })
            .collect::<Result<Vec<_>, tokio_postgres::Error>>()
            .map_err(util::db_error)?;
        entries.push(
            (|| -> Result<ExportEntry, tokio_postgres::Error> {
                Ok(ExportEntry {
                    date: row.try_get("date")?,
                    mood: util::mood_value_from_f64(row.try_get("mood")?),
                    content: row.try_get("content")?,
                    created_at: row.try_get("created_at")?,
                    updated_at: row.try_get("updated_at")?,
                    selections,
                })
            })()
            .map_err(util::db_error)?,
        );
    }

    let goal_rows = client
        .query(EXPORT_GOALS_SQL, &[&user_id])
        .await
        .map_err(util::db_error)?;
    let mut goals = Vec::with_capacity(goal_rows.len());
    for row in &goal_rows {
        let goal_id: i64 = row.try_get("id").map_err(util::db_error)?;
        let mut goal = (|| -> Result<ExportGoal, tokio_postgres::Error> {
            Ok(ExportGoal {
                title: row.try_get("title")?,
                description: row.try_get("description")?,
                frequency_per_week: row.try_get("frequency_per_week")?,
                completed: row.try_get("completed")?,
                streak: row.try_get("streak")?,
                period_start: row.try_get("period_start")?,
                last_completed_date: row.try_get("last_completed_date")?,
                created_at: row.try_get("created_at")?,
                updated_at: row.try_get("updated_at")?,
                completions: Vec::new(),
            })
        })()
        .map_err(util::db_error)?;
        let (completed, streak, period_start) = data::project_weekly_state(
            goal.completed,
            goal.streak,
            goal.frequency_per_week,
            goal.period_start.as_deref(),
            today,
        );
        goal.completed = completed;
        goal.streak = streak;
        goal.period_start = period_start;
        goal.completions = client
            .query(EXPORT_COMPLETIONS_SQL, &[&user_id, &goal_id])
            .await
            .map_err(util::db_error)?
            .iter()
            .map(|completion| completion.try_get("date"))
            .collect::<Result<Vec<_>, tokio_postgres::Error>>()
            .map_err(util::db_error)?;
        goals.push(goal);
    }

    Ok(ExportData { entries, goals })
}

// --- Import ------------------------------------------------------------------

/// Twin of `data::import_data` — one transaction, all-or-nothing,
/// duplicates skipped, groups/options resolved-or-created through the
/// preloaded name→id cache.
pub async fn import_data(
    client: &mut impl GenericClient,
    user_id: i64,
    data: &ExportData,
) -> Result<ImportCounts, DatabaseError> {
    let tx = client.transaction().await.map_err(util::db_error)?;
    let mut counts = ImportCounts::default();

    let mut entry_keys: HashSet<(String, String)> = tx
        .query(PRELOAD_ENTRY_KEYS_SQL, &[&user_id])
        .await
        .map_err(util::db_error)?
        .iter()
        .map(|row| (row.get(0), row.get(1)))
        .collect();
    let mut goal_titles: HashSet<String> = tx
        .query(PRELOAD_GOAL_TITLES_SQL, &[&user_id])
        .await
        .map_err(util::db_error)?
        .iter()
        .map(|row| row.get(0))
        .collect();
    let mut group_ids: HashMap<String, i64> = tx
        .query(PRELOAD_GROUPS_SQL, &[&user_id])
        .await
        .map_err(util::db_error)?
        .iter()
        .map(|row| (row.get(1), row.get(0)))
        .collect();
    let mut option_ids: HashMap<(i64, String), i64> = HashMap::new();
    for row in tx
        .query(PRELOAD_OPTIONS_SQL, &[&user_id])
        .await
        .map_err(util::db_error)?
    {
        let (option_id, group_id, name): (i64, i64, String) = (row.get(0), row.get(1), row.get(2));
        // First-wins: duplicate names resolve to the oldest option row.
        option_ids.entry((group_id, name)).or_insert(option_id);
    }

    let insert_selection = tx
        .prepare(IMPORT_SELECTION_SQL)
        .await
        .map_err(util::db_error)?;
    for entry in &data.entries {
        let key = (entry.date.clone(), entry.content.clone());
        if entry_keys.contains(&key) {
            counts.entries.skipped += 1;
            continue;
        }
        let mood: f64 = match entry.mood {
            MoodValue::Int(value) => value as f64,
            MoodValue::Float(value) => value,
        };
        let entry_id: i64 = tx
            .query_one(
                IMPORT_ENTRY_SQL,
                &[
                    &user_id,
                    &entry.date,
                    &mood,
                    &entry.content,
                    &entry.created_at,
                    &entry.updated_at,
                ],
            )
            .await
            .map_err(util::db_error)?
            .get(0);
        for selection in &entry.selections {
            let group_id = match group_ids.get(&selection.group_name) {
                Some(id) => *id,
                None => {
                    let id: i64 = tx
                        .query_one(IMPORT_GROUP_SQL, &[&selection.group_name, &user_id])
                        .await
                        .map_err(util::db_error)?
                        .get(0);
                    group_ids.insert(selection.group_name.clone(), id);
                    id
                }
            };
            let option_key = (group_id, selection.option_name.clone());
            let option_id = match option_ids.get(&option_key) {
                Some(id) => *id,
                None => {
                    let id: i64 = tx
                        .query_one(IMPORT_OPTION_SQL, &[&group_id, &selection.option_name])
                        .await
                        .map_err(util::db_error)?
                        .get(0);
                    option_ids.insert(option_key, id);
                    id
                }
            };
            tx.execute(&insert_selection, &[&entry_id, &option_id])
                .await
                .map_err(util::db_error)?;
        }
        entry_keys.insert(key);
        counts.entries.imported += 1;
    }

    let insert_completion = tx
        .prepare(IMPORT_COMPLETION_SQL)
        .await
        .map_err(util::db_error)?;
    for goal in &data.goals {
        if goal_titles.contains(&goal.title) {
            counts.goals.skipped += 1;
            continue;
        }
        let goal_id: i64 = tx
            .query_one(
                IMPORT_GOAL_SQL,
                &[
                    &user_id,
                    &goal.title,
                    &goal.description,
                    &goal.frequency_per_week,
                    &goal.completed.unwrap_or(0),
                    &goal.streak.unwrap_or(0),
                    &goal.period_start,
                    &goal.last_completed_date,
                    &goal.created_at,
                    &goal.updated_at,
                ],
            )
            .await
            .map_err(util::db_error)?
            .get(0);
        for date in &goal.completions {
            tx.execute(&insert_completion, &[&user_id, &goal_id, &date])
                .await
                .map_err(util::db_error)?;
        }
        goal_titles.insert(goal.title.clone());
        counts.goals.imported += 1;
    }

    tx.commit().await.map_err(util::db_error)?;
    Ok(counts)
}

// --- Tests (live PG, gated on NIGHTLIO_PG_TEST_URL) ---------------------------

#[cfg(test)]
mod tests {
    use super::super::util::test_support::{connect_scratch, seed_default_user};
    use super::*;
    use crate::db::data::CategoryCounts;
    use serde_json::json;

    /// Twin of the sync module's merge test: names resolve-or-create, dup
    /// keys skip (including intra-file), weekly state written raw,
    /// `ON CONFLICT DO NOTHING` dedupes completions, re-import idempotent.
    #[tokio::test]
    async fn pg_import_merges_resolves_names_and_is_idempotent() {
        let Some(mut client) = connect_scratch("data_import_merge").await else {
            return;
        };
        let user_id = seed_default_user(&client).await;
        // Explicit ids sit far from the identity sequences' start so the
        // import's own auto-increment inserts ("Custom Tags") cannot collide.
        client
            .batch_execute(
                "INSERT INTO groups (id, user_id, name) VALUES (10, 1, 'Emotions');
                 INSERT INTO group_options (id, group_id, name) VALUES (10, 10, 'happy');",
            )
            .await
            .unwrap();

        let file = ExportData {
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
                completions: vec!["2025-08-05".to_string(), "2025-08-05".to_string()],
            }],
        };

        let counts = import_data(&mut client, user_id, &file).await.unwrap();
        assert_eq!(
            counts.entries,
            CategoryCounts {
                imported: 1,
                skipped: 1
            }
        );
        assert_eq!(
            counts.goals,
            CategoryCounts {
                imported: 1,
                skipped: 0
            }
        );

        let row = client
            .query_one(
                "SELECT (SELECT COUNT(*) FROM groups WHERE user_id = 1), \
                        (SELECT COUNT(*) FROM entry_selections), \
                        (SELECT COUNT(*) FROM goal_completions), \
                        (SELECT period_start FROM goals WHERE title = 'Journal'), \
                        (SELECT created_at FROM mood_entries LIMIT 1)",
                &[],
            )
            .await
            .unwrap();
        let (groups, selections, completions): (i64, i64, i64) =
            (row.get(0), row.get(1), row.get(2));
        assert_eq!(groups, 2, "Custom Tags created, Emotions resolved");
        assert_eq!(selections, 2);
        assert_eq!(completions, 1, "ON CONFLICT DO NOTHING deduped");
        assert_eq!(row.get::<_, String>(3), "2025-08-04", "weekly state raw");
        assert_eq!(row.get::<_, String>(4), "2025-08-01 21:15:00");

        let again = import_data(&mut client, user_id, &file).await.unwrap();
        assert_eq!(
            again.entries,
            CategoryCounts {
                imported: 0,
                skipped: 2
            }
        );
        assert_eq!(
            again.goals,
            CategoryCounts {
                imported: 0,
                skipped: 1
            }
        );
    }

    /// Export determinism + projection twin: entries `date ASC, id ASC`,
    /// selections by group/option name, goal rollover projected without
    /// writing, completions ascending.
    #[tokio::test]
    async fn pg_export_orders_and_projects_like_sqlite() {
        let Some(client) = connect_scratch("data_export_order").await else {
            return;
        };
        let user_id = seed_default_user(&client).await;
        client
            .batch_execute(
                "INSERT INTO groups (id, user_id, name) VALUES (1, 1, 'Emotions');
                 INSERT INTO group_options (id, group_id, name) VALUES (1, 1, 'happy');
                 INSERT INTO group_options (id, group_id, name) VALUES (5, 1, 'content');
                 INSERT INTO mood_entries (id, user_id, date, mood, content, created_at) VALUES
                   (1, 1, '2025-08-03', 5, 'Third day.', '2025-08-03 10:00:00'),
                   (2, 1, '2025-08-01', 4, 'Great workout day.', '2025-08-01 08:00:00'),
                   (3, 1, '2025-08-03', 2, 'Same-day later insert.', '2025-08-03 06:00:00');
                 INSERT INTO entry_selections (entry_id, option_id) VALUES (2, 1), (2, 5);
                 INSERT INTO goals (id, user_id, title, description, frequency_per_week, \
                                    completed, streak, period_start, created_at, updated_at)
                 VALUES (1, 1, 'Meditate', '10 minutes', 3, 3, 5, '2020-01-06', \
                         '2020-01-01 00:00:00', '2020-01-01 00:00:00');
                 INSERT INTO goal_completions (user_id, goal_id, date) VALUES
                   (1, 1, '2020-01-08'), (1, 1, '2020-01-06');",
            )
            .await
            .unwrap();

        let today = chrono::NaiveDate::from_ymd_opt(2026, 8, 15).unwrap();
        let export = export_data_on(&client, user_id, today).await.unwrap();

        let order: Vec<&str> = export
            .entries
            .iter()
            .map(|entry| entry.content.as_str())
            .collect();
        assert_eq!(
            order,
            ["Great workout day.", "Third day.", "Same-day later insert."]
        );
        assert_eq!(
            serde_json::to_value(&export.entries[0].selections).unwrap(),
            json!([
                {"group_name": "Emotions", "option_name": "content"},
                {"group_name": "Emotions", "option_name": "happy"},
            ])
        );
        let goal = &export.goals[0];
        assert_eq!(goal.completed, Some(0));
        assert_eq!(goal.streak, Some(6));
        assert_eq!(goal.period_start.as_deref(), Some("2026-08-10"));
        assert_eq!(goal.completions, ["2020-01-06", "2020-01-08"]);

        // Pure read: raw weekly state and sentinel timestamp survive.
        let row = client
            .query_one(
                "SELECT completed, streak, period_start, updated_at FROM goals WHERE id = 1",
                &[],
            )
            .await
            .unwrap();
        assert_eq!(
            (
                row.get::<_, i64>(0),
                row.get::<_, i64>(1),
                row.get::<_, String>(2),
                row.get::<_, String>(3),
            ),
            (
                3,
                5,
                "2020-01-06".to_string(),
                "2020-01-01 00:00:00".to_string()
            ),
            "export must not write"
        );
    }
}
