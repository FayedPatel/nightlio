//! Port of `api/database_moods.py` (`MoodEntriesMixin`): CRUD for mood
//! entries plus the entry-selections read/write the mood routes consume
//! (`get_entry_selections` lives in `api/database_groups.py` on the Python
//! side but is exercised by `mood_service`, so it is ported here with the
//! rest of the entry lifecycle).
//!
//! Owned by the moods data-mixin agent — no other agent edits this file.
//!
//! Every SQL string is kept byte-identical to what the Python builds at
//! runtime (triple-quoted literals and f-string composition included), so
//! query semantics cannot drift. Two quirks are REQUIRED behavior per
//! `contract/DECISIONS.md`:
//!
//! - **#4 mixed date-format semantics** — the date-range list filters with a
//!   raw-string `BETWEEN` on the stored `date` column (no normalisation), so
//!   US-format rows (`8/2/2025`) fall outside ISO ranges even when they are
//!   chronologically inside. Listing *order*, by contrast, normalises both
//!   shapes via `_ISO_DAY_EXPR` (unparseable dates sort last under DESC).
//! - **Two response shapes** — PUT returns the entry WITH a `selections`
//!   array (`MoodEntryWithSelections`, mirroring `MoodService.update_entry`)
//!   while GET by id returns the entry WITHOUT a `selections` key
//!   (`MoodEntryRow`).
//!
//! All functions take `&Connection`; callers run them under
//! `tokio::task::spawn_blocking`.

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::Serialize;

use super::common::{DatabaseError, MoodValue};

// --- SQL (verbatim ports of the Python string literals) ----------------------

/// Verbatim `database_stats._ISO_DAY_EXPR`: normalise a stored
/// `mood_entries.date` to ISO `YYYY-MM-DD`, or NULL if it matches neither
/// the ISO nor the US `M/D/YYYY` shape. Defined as a macro so the query
/// constants below can be composed with `concat!` at compile time exactly
/// the way the Python composes its f-strings at import time.
macro_rules! iso_day_expr {
    () => {
        r"
    CASE
        WHEN date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]' THEN date
        WHEN date GLOB '*/*/[0-9][0-9][0-9][0-9]' THEN
            substr(date, -4)
            || '-'
            || printf('%02d', CAST(substr(date, 1, instr(date, '/') - 1) AS INTEGER))
            || '-'
            || printf(
                   '%02d',
                   CAST(
                       substr(
                           substr(date, instr(date, '/') + 1),
                           1,
                           instr(substr(date, instr(date, '/') + 1), '/') - 1
                       ) AS INTEGER
                   )
               )
        ELSE NULL
    END
"
    };
}

/// Verbatim `database_moods._ENTRY_ORDER`: newest journaled day first
/// (normalised across both stored date shapes), then creation time within
/// the day. Ordering by `created_at` alone floated backdated entries above
/// newer-dated ones. Unparseable dates (NULL day) sort last under DESC.
macro_rules! entry_order {
    () => {
        concat!("ORDER BY ", iso_day_expr!(), " DESC, created_at DESC")
    };
}

const INSERT_MOOD_ENTRY_WITH_TIME_SQL: &str = r"
                    INSERT INTO mood_entries (user_id, date, mood, content, created_at)
                    VALUES (?, ?, ?, ?, ?)
                    ";

const INSERT_MOOD_ENTRY_SQL: &str = r"
                    INSERT INTO mood_entries (user_id, date, mood, content)
                    VALUES (?, ?, ?, ?)
                    ";

const INSERT_ENTRY_SELECTION_SQL: &str =
    "INSERT INTO entry_selections (entry_id, option_id) VALUES (?, ?)";

const GET_ALL_MOOD_ENTRIES_SQL: &str = concat!(
    r"
                SELECT id, date, mood, content, created_at, updated_at
                  FROM mood_entries
                 WHERE user_id = ?
                 ",
    entry_order!(),
    r"
                "
);

/// DECISIONS.md #4: raw-string `BETWEEN` on the stored date column — the
/// range filter and the list ordering deliberately use different date
/// semantics, and preserving that changes-users'-numbers quirk is required.
const GET_MOOD_ENTRIES_BY_DATE_RANGE_SQL: &str = concat!(
    r"
                SELECT id, date, mood, content, created_at, updated_at
                  FROM mood_entries
                 WHERE user_id = ? AND date BETWEEN ? AND ?
                 ",
    entry_order!(),
    r"
                "
);

const GET_MOOD_ENTRY_BY_ID_SQL: &str = r"
                SELECT id, date, mood, content, created_at, updated_at
                  FROM mood_entries
                 WHERE id = ? AND user_id = ?
                ";

const SELECT_ENTRY_EXISTS_SQL: &str = "SELECT id FROM mood_entries WHERE id = ? AND user_id = ?";

const TOUCH_UPDATED_AT_SQL: &str = r"
                    UPDATE mood_entries
                       SET updated_at = CURRENT_TIMESTAMP
                     WHERE id = ? AND user_id = ?
                    ";

const DELETE_ENTRY_SELECTIONS_SQL: &str = "DELETE FROM entry_selections WHERE entry_id = ?";

const DELETE_MOOD_ENTRY_SQL: &str = "DELETE FROM mood_entries WHERE id = ? AND user_id = ?";

/// `database_groups.get_entry_selections`, user-scoped variant (the entry is
/// additionally verified to belong to the user via the `mood_entries` join).
const GET_ENTRY_SELECTIONS_SCOPED_SQL: &str = r"
                    SELECT go.id, go.name, g.name as group_name
                      FROM entry_selections es
                      JOIN mood_entries me ON es.entry_id = me.id
                      JOIN group_options go ON es.option_id = go.id
                      JOIN groups g ON go.group_id = g.id
                     WHERE es.entry_id = ? AND me.user_id = ?
                     ORDER BY g.name, go.name
                    ";

/// `database_groups.get_entry_selections`, unscoped variant — the caller
/// (e.g. the update path) is expected to have verified entry ownership.
const GET_ENTRY_SELECTIONS_SQL: &str = r"
                    SELECT go.id, go.name, g.name as group_name
                      FROM entry_selections es
                      JOIN group_options go ON es.option_id = go.id
                      JOIN groups g ON go.group_id = g.id
                     WHERE es.entry_id = ?
                     ORDER BY g.name, go.name
                    ";

// --- Row shapes --------------------------------------------------------------

/// One mood entry as the Flask API emits it (GET shape: no `selections`
/// key). `created_at`/`updated_at` default to CURRENT_TIMESTAMP at insert
/// but stay `Option` for tolerance of hand-edited rows in the wild.
#[derive(Debug, Clone, Serialize)]
pub struct MoodEntryRow {
    pub id: i64,
    pub date: String,
    /// Read tolerantly ([`MoodValue`]): SQLite's dynamic typing admits a
    /// schema-legal REAL like `4.5` (passes the 1..=5 CHECK), which Flask
    /// serves transparently — a strict `i64` here turned such rows into 500s.
    pub mood: MoodValue,
    pub content: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

/// One selected option: `{id, name, group_name}`.
#[derive(Debug, Clone, Serialize)]
pub struct EntrySelectionRow {
    pub id: i64,
    pub name: String,
    pub group_name: String,
}

/// PUT shape: the entry WITH its `selections` array (mirrors
/// `MoodService.update_entry` setting `entry["selections"]`).
#[derive(Debug, Clone, Serialize)]
pub struct MoodEntryWithSelections {
    #[serde(flatten)]
    pub entry: MoodEntryRow,
    pub selections: Vec<EntrySelectionRow>,
}

/// Field bag for `update_mood_entry` — each `Some` becomes a SET clause,
/// mirroring the Python keyword arguments. `time` writes `created_at`.
/// `selected_options: Some(vec![])` clears the selections (the route maps a
/// JSON `selected_options: null` to `[]` before it reaches the data layer).
#[derive(Debug, Clone, Default)]
pub struct MoodEntryUpdate {
    pub mood: Option<i64>,
    pub content: Option<String>,
    pub date: Option<String>,
    pub time: Option<String>,
    pub selected_options: Option<Vec<i64>>,
}

fn map_entry(row: &Row<'_>) -> rusqlite::Result<MoodEntryRow> {
    Ok(MoodEntryRow {
        id: row.get("id")?,
        date: row.get("date")?,
        mood: row.get("mood")?,
        content: row.get("content")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn map_selection(row: &Row<'_>) -> rusqlite::Result<EntrySelectionRow> {
    Ok(EntrySelectionRow {
        id: row.get("id")?,
        name: row.get("name")?,
        group_name: row.get("group_name")?,
    })
}

// --- Queries -----------------------------------------------------------------

/// Port of `add_mood_entry`. Returns the new entry id. Note the Python
/// checks `if time:` (truthiness), so an empty-string `time` falls through
/// to the no-time INSERT and `created_at` takes its CURRENT_TIMESTAMP
/// default.
pub fn add_mood_entry(
    conn: &Connection,
    user_id: i64,
    date: &str,
    mood: i64,
    content: &str,
    time: Option<&str>,
    selected_options: Option<&[i64]>,
) -> Result<i64, DatabaseError> {
    let tx = conn.unchecked_transaction()?;
    let entry_id = match time {
        Some(time) if !time.is_empty() => {
            tx.execute(
                INSERT_MOOD_ENTRY_WITH_TIME_SQL,
                params![user_id, date, mood, content, time],
            )?;
            tx.last_insert_rowid()
        }
        _ => {
            tx.execute(INSERT_MOOD_ENTRY_SQL, params![user_id, date, mood, content])?;
            tx.last_insert_rowid()
        }
    };

    // Python: `if selected_options:` — an empty list inserts nothing.
    if let Some(options) = selected_options
        && !options.is_empty()
    {
        let mut stmt = tx.prepare(INSERT_ENTRY_SELECTION_SQL)?;
        for option_id in options {
            stmt.execute(params![entry_id, option_id])?;
        }
    }

    tx.commit()?;
    Ok(entry_id)
}

/// Port of `get_all_mood_entries`: every entry for the user, newest
/// normalised day first, then `created_at` DESC within the day.
pub fn get_all_mood_entries(
    conn: &Connection,
    user_id: i64,
) -> Result<Vec<MoodEntryRow>, DatabaseError> {
    let mut stmt = conn.prepare(GET_ALL_MOOD_ENTRIES_SQL)?;
    let rows = stmt
        .query_map(params![user_id], map_entry)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Port of `get_mood_entries_by_date_range` — raw-string `BETWEEN` on the
/// stored date (DECISIONS.md #4), same ordering as the full list.
pub fn get_mood_entries_by_date_range(
    conn: &Connection,
    user_id: i64,
    start_date: &str,
    end_date: &str,
) -> Result<Vec<MoodEntryRow>, DatabaseError> {
    let mut stmt = conn.prepare(GET_MOOD_ENTRIES_BY_DATE_RANGE_SQL)?;
    let rows = stmt
        .query_map(params![user_id, start_date, end_date], map_entry)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Port of `get_mood_entry_by_id` (GET shape: no `selections` key).
pub fn get_mood_entry_by_id(
    conn: &Connection,
    user_id: i64,
    entry_id: i64,
) -> Result<Option<MoodEntryRow>, DatabaseError> {
    Ok(conn
        .query_row(
            GET_MOOD_ENTRY_BY_ID_SQL,
            params![entry_id, user_id],
            map_entry,
        )
        .optional()?)
}

/// Port of `update_mood_entry`. Returns whether an update happened, with
/// the Python's exact semantics:
/// - entry missing (or owned by someone else) → `false`, nothing written;
/// - no field updates and no `selected_options` → `updated_at` is still
///   touched but the return value is `false` (bug-compatible; the route's
///   empty-body 400 normally shields this path);
/// - `selected_options: Some(_)` always counts as an update, including
///   `Some(vec![])` which just clears the selections.
pub fn update_mood_entry(
    conn: &Connection,
    user_id: i64,
    entry_id: i64,
    update: &MoodEntryUpdate,
) -> Result<bool, DatabaseError> {
    let mut updates: Vec<&str> = Vec::new();
    let mut update_params: Vec<&dyn rusqlite::ToSql> = Vec::new();

    if let Some(mood) = &update.mood {
        updates.push("mood = ?");
        update_params.push(mood);
    }
    if let Some(content) = &update.content {
        updates.push("content = ?");
        update_params.push(content);
    }
    if let Some(date) = &update.date {
        updates.push("date = ?");
        update_params.push(date);
    }
    if let Some(time) = &update.time {
        updates.push("created_at = ?");
        update_params.push(time);
    }

    let exists = conn
        .query_row(SELECT_ENTRY_EXISTS_SQL, params![entry_id, user_id], |row| {
            row.get::<_, i64>(0)
        })
        .optional()?;
    if exists.is_none() {
        return Ok(false);
    }

    let tx = conn.unchecked_transaction()?;
    let mut updated = false;
    if !updates.is_empty() {
        updates.push("updated_at = CURRENT_TIMESTAMP");
        let sql = format!(
            "UPDATE mood_entries SET {} WHERE id = ? AND user_id = ?",
            updates.join(", ")
        );
        update_params.push(&entry_id);
        update_params.push(&user_id);
        tx.execute(&sql, update_params.as_slice())?;
        updated = true;
    } else {
        tx.execute(TOUCH_UPDATED_AT_SQL, params![entry_id, user_id])?;
    }

    if let Some(options) = &update.selected_options {
        tx.execute(DELETE_ENTRY_SELECTIONS_SQL, params![entry_id])?;
        if !options.is_empty() {
            let mut stmt = tx.prepare(INSERT_ENTRY_SELECTION_SQL)?;
            for option_id in options {
                stmt.execute(params![entry_id, option_id])?;
            }
        }
        updated = true;
    }

    tx.commit()?;
    Ok(updated || update.selected_options.is_some())
}

/// Port of `delete_mood_entry`. `true` only when a row owned by the user
/// was actually deleted.
pub fn delete_mood_entry(
    conn: &Connection,
    user_id: i64,
    entry_id: i64,
) -> Result<bool, DatabaseError> {
    let rows = conn.execute(DELETE_MOOD_ENTRY_SQL, params![entry_id, user_id])?;
    Ok(rows > 0)
}

/// Port of `database_groups.get_entry_selections`: the selected options for
/// an entry, ordered by group name then option name. With `user_id` the
/// entry is additionally verified to belong to that user (unknown/foreign
/// entries yield an empty list, never an error — DECISIONS.md #5's 200-`[]`
/// rides on this); without it the caller must have verified ownership.
pub fn get_entry_selections(
    conn: &Connection,
    entry_id: i64,
    user_id: Option<i64>,
) -> Result<Vec<EntrySelectionRow>, DatabaseError> {
    let rows = match user_id {
        Some(user_id) => {
            let mut stmt = conn.prepare(GET_ENTRY_SELECTIONS_SCOPED_SQL)?;
            stmt.query_map(params![entry_id, user_id], map_selection)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        }
        None => {
            let mut stmt = conn.prepare(GET_ENTRY_SELECTIONS_SQL)?;
            stmt.query_map(params![entry_id], map_selection)?
                .collect::<rusqlite::Result<Vec<_>>>()?
        }
    };
    Ok(rows)
}

/// The PUT response composition from `MoodService.update_entry`: re-read the
/// entry (user-scoped) and attach its selections (unscoped read — ownership
/// was just verified). `None` when the entry vanished.
pub fn get_mood_entry_with_selections(
    conn: &Connection,
    user_id: i64,
    entry_id: i64,
) -> Result<Option<MoodEntryWithSelections>, DatabaseError> {
    let Some(entry) = get_mood_entry_by_id(conn, user_id, entry_id)? else {
        return Ok(None);
    };
    let selections = get_entry_selections(conn, entry_id, None)?;
    Ok(Some(MoodEntryWithSelections { entry, selections }))
}

// --- Tests -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::bootstrap::{SelfHostSeed, bootstrap};
    use serde_json::json;

    /// Bootstrapped tempfile DB with the fixture rows from
    /// `moods_parity.py` — the same seed the Python side was run against to
    /// pin the expected values below (command in `python_fixture` docs).
    ///
    /// Seed (default self-host user has id 1):
    /// - groups 10 "ZTest Activities" / 11 "ATest Emotions" (user 1) with
    ///   options 100 "Exercise", 101 "Cooking" (group 10), 102 "happy"
    ///   (group 11);
    /// - entries 1..=6 for user 1 (ISO, US-format, duplicate-day and
    ///   unparseable dates) plus user 2 ("other") with entry 7.
    fn seeded_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("moods.db");
        let path = path.to_str().unwrap();
        bootstrap(path, &SelfHostSeed::default()).unwrap();
        let conn = crate::db::common::connect(path).unwrap();

        conn.execute_batch(
            "INSERT INTO groups (id, user_id, name) VALUES (10, 1, 'ZTest Activities');
             INSERT INTO groups (id, user_id, name) VALUES (11, 1, 'ATest Emotions');
             INSERT INTO group_options (id, group_id, name) VALUES (100, 10, 'Exercise');
             INSERT INTO group_options (id, group_id, name) VALUES (101, 10, 'Cooking');
             INSERT INTO group_options (id, group_id, name) VALUES (102, 11, 'happy');
             INSERT INTO users (google_id, email, name)
                  VALUES ('other-google-id', 'other@example.com', 'Other');",
        )
        .unwrap();

        let ids = [
            add_mood_entry(
                &conn,
                1,
                "2025-08-01",
                4,
                "Great workout day.",
                Some("2025-08-01 08:00:00"),
                Some(&[102, 100]),
            )
            .unwrap(),
            add_mood_entry(
                &conn,
                1,
                "8/2/2025",
                2,
                "Rough day, US-format date.",
                Some("2025-08-02 09:00:00"),
                None,
            )
            .unwrap(),
            add_mood_entry(
                &conn,
                1,
                "2025-08-03",
                5,
                "Third day.",
                Some("2025-08-03 10:00:00"),
                Some(&[]),
            )
            .unwrap(),
            add_mood_entry(
                &conn,
                1,
                "not-a-date",
                3,
                "Unparseable.",
                Some("2025-08-04 11:00:00"),
                None,
            )
            .unwrap(),
            add_mood_entry(
                &conn,
                1,
                "2025-08-03",
                1,
                "Backdated later insert.",
                Some("2025-08-03 06:00:00"),
                None,
            )
            .unwrap(),
            add_mood_entry(
                &conn,
                1,
                "12/31/2025",
                3,
                "US New Year's Eve.",
                Some("2025-12-31 23:00:00"),
                None,
            )
            .unwrap(),
        ];
        assert_eq!(ids, [1, 2, 3, 4, 5, 6], "seed must produce ids 1..=6");

        let other_entry = add_mood_entry(
            &conn,
            2,
            "2025-08-02",
            3,
            "Someone else's entry.",
            Some("2025-08-02 12:00:00"),
            None,
        )
        .unwrap();
        assert_eq!(other_entry, 7);

        (dir, conn)
    }

    /// Expected values in the tests below were produced by running the real
    /// Python data layer (`api.database.MoodDatabase`) against an
    /// identically-seeded DB:
    ///
    /// ```text
    /// api/venv/bin/python moods_parity.py   # scratchpad script, 2026-08-15
    /// get_all order: [6, 3, 5, 2, 1, 4]
    /// range 2025-08-01..2025-08-31: [3, 5, 1]
    /// range 0..9: [6, 3, 5, 2, 1]
    /// selections e1: [{"id": 102, ...ATest Emotions}, {"id": 100, ...ZTest Activities}]
    /// update none/none: False; empty selections: True; mood+content: True;
    /// missing entry: False; foreign entry: False; replace selections: True
    /// delete e4: True; again: False; foreign: False
    /// ```
    fn ids(rows: &[MoodEntryRow]) -> Vec<i64> {
        rows.iter().map(|row| row.id).collect()
    }

    #[test]
    fn add_and_get_by_id_roundtrip() {
        let (_dir, conn) = seeded_db();
        let entry = get_mood_entry_by_id(&conn, 1, 1).unwrap().unwrap();
        assert_eq!(entry.id, 1);
        assert_eq!(entry.date, "2025-08-01");
        assert_eq!(entry.mood, MoodValue::Int(4));
        assert_eq!(entry.content, "Great workout day.");
        // `time` was provided, so created_at is the caller's string verbatim
        // while updated_at took its CURRENT_TIMESTAMP default.
        assert_eq!(entry.created_at.as_deref(), Some("2025-08-01 08:00:00"));
        assert!(entry.updated_at.is_some());

        assert!(get_mood_entry_by_id(&conn, 1, 9999).unwrap().is_none());
        // Foreign user's entry is invisible.
        assert!(get_mood_entry_by_id(&conn, 1, 7).unwrap().is_none());
        assert!(get_mood_entry_by_id(&conn, 2, 7).unwrap().is_some());
    }

    #[test]
    fn schema_legal_real_mood_reads_back_like_python() {
        let (_dir, conn) = seeded_db();
        // SQLite dynamic typing: 4.5 passes the 1..=5 CHECK and INTEGER
        // affinity keeps it REAL (only integral reals are converted).
        // Pinned from Python (scratchpad real_mood_xcheck.py, 2026-08-15):
        // sqlite3 returns the float transparently and Flask emits 4.5.
        conn.execute(
            "INSERT INTO mood_entries (user_id, date, mood, content, created_at)
             VALUES (1, '2025-08-06', 4.5, 'Real-typed mood.', '2025-08-06 08:00:00')",
            [],
        )
        .unwrap();
        let id = conn.last_insert_rowid();
        assert_eq!(
            conn.query_row(
                "SELECT typeof(mood) FROM mood_entries WHERE id = ?",
                [id],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "real"
        );

        // Reading must not 500 (previously: InvalidColumnType Real for
        // MoodEntryRow.mood: i64) and must serialize as the JSON float 4.5.
        let entry = get_mood_entry_by_id(&conn, 1, id).unwrap().unwrap();
        assert_eq!(entry.mood, MoodValue::Float(4.5));
        assert_eq!(serde_json::to_value(&entry).unwrap()["mood"], json!(4.5));

        // The list endpoints traverse the same row mapper.
        let all = get_all_mood_entries(&conn, 1).unwrap();
        assert!(all.iter().any(|row| row.id == id));
        // Integer-typed rows still emit bare integers.
        let ints = get_mood_entry_by_id(&conn, 1, 1).unwrap().unwrap();
        assert_eq!(serde_json::to_value(&ints).unwrap()["mood"], json!(4));
    }

    #[test]
    fn empty_time_string_takes_default_created_at_branch() {
        let (_dir, conn) = seeded_db();
        // Python checks `if time:` — "" falls through to the no-time INSERT.
        let id = add_mood_entry(
            &conn,
            1,
            "2025-08-05",
            3,
            "Empty time string.",
            Some(""),
            None,
        )
        .unwrap();
        let entry = get_mood_entry_by_id(&conn, 1, id).unwrap().unwrap();
        let created_at = entry.created_at.expect("CURRENT_TIMESTAMP default");
        assert!(!created_at.is_empty());
        assert_ne!(created_at, "");
    }

    #[test]
    fn get_all_orders_by_normalised_day_then_created_at() {
        let (_dir, conn) = seeded_db();
        // Pinned from Python: [6, 3, 5, 2, 1, 4] — the US-format 12/31/2025
        // first, created_at DESC breaks the 2025-08-03 tie, 8/2/2025 sorts
        // between the ISO days, and the unparseable date lands last.
        assert_eq!(
            ids(&get_all_mood_entries(&conn, 1).unwrap()),
            [6, 3, 5, 2, 1, 4]
        );
        // Per-user isolation.
        assert_eq!(ids(&get_all_mood_entries(&conn, 2).unwrap()), [7]);
    }

    #[test]
    fn date_range_is_raw_string_between() {
        let (_dir, conn) = seeded_db();
        // Pinned from Python: DECISIONS.md #4 — '8/2/2025' is chronologically
        // inside August 2025 but excluded from the ISO range, and so is
        // '12/31/2025'; ordering still normalises.
        assert_eq!(
            ids(&get_mood_entries_by_date_range(&conn, 1, "2025-08-01", "2025-08-31").unwrap()),
            [3, 5, 1]
        );
        // Pinned from Python: a lexicographic "0".."9" range catches both
        // US-format rows but still not 'not-a-date' ('n' > '9').
        assert_eq!(
            ids(&get_mood_entries_by_date_range(&conn, 1, "0", "9").unwrap()),
            [6, 3, 5, 2, 1]
        );
    }

    #[test]
    fn entry_selections_ordering_and_scoping() {
        let (_dir, conn) = seeded_db();
        // Pinned from Python: ordered by group name then option name —
        // 'ATest Emotions' before 'ZTest Activities'.
        let rows = get_entry_selections(&conn, 1, None).unwrap();
        assert_eq!(
            serde_json::to_value(&rows).unwrap(),
            json!([
                {"id": 102, "name": "happy", "group_name": "ATest Emotions"},
                {"id": 100, "name": "Exercise", "group_name": "ZTest Activities"},
            ])
        );
        // Scoped variant returns the same rows for the owner...
        let scoped = get_entry_selections(&conn, 1, Some(1)).unwrap();
        assert_eq!(
            serde_json::to_value(&scoped).unwrap(),
            serde_json::to_value(&rows).unwrap()
        );
        // ...and an empty list (not an error) for a foreign user or an
        // entry with no selections.
        assert!(get_entry_selections(&conn, 1, Some(2)).unwrap().is_empty());
        assert!(get_entry_selections(&conn, 3, None).unwrap().is_empty());
    }

    #[test]
    fn update_semantics_match_python() {
        let (_dir, conn) = seeded_db();

        // Pinned from Python: no fields + no selected_options returns False
        // but still touches updated_at (bug-compatible).
        conn.execute(
            "UPDATE mood_entries SET updated_at = '1999-01-01 00:00:00' WHERE id = 2",
            [],
        )
        .unwrap();
        assert!(!update_mood_entry(&conn, 1, 2, &MoodEntryUpdate::default()).unwrap());
        let touched = get_mood_entry_by_id(&conn, 1, 2).unwrap().unwrap();
        assert_ne!(touched.updated_at.as_deref(), Some("1999-01-01 00:00:00"));

        // Pinned from Python: Some(vec![]) clears selections and returns True.
        assert!(
            update_mood_entry(
                &conn,
                1,
                2,
                &MoodEntryUpdate {
                    selected_options: Some(vec![]),
                    ..Default::default()
                }
            )
            .unwrap()
        );

        // Pinned from Python: partial mood+content update returns True and
        // leaves the stored date untouched.
        assert!(
            update_mood_entry(
                &conn,
                1,
                2,
                &MoodEntryUpdate {
                    mood: Some(3),
                    content: Some("Updated content.".to_string()),
                    ..Default::default()
                }
            )
            .unwrap()
        );
        let entry = get_mood_entry_by_id(&conn, 1, 2).unwrap().unwrap();
        assert_eq!(
            (entry.mood, entry.content.as_str(), entry.date.as_str()),
            (MoodValue::Int(3), "Updated content.", "8/2/2025")
        );

        // Pinned from Python: missing and foreign entries return False.
        let mood_only = MoodEntryUpdate {
            mood: Some(3),
            ..Default::default()
        };
        assert!(!update_mood_entry(&conn, 1, 9999, &mood_only).unwrap());
        assert!(!update_mood_entry(&conn, 2, 2, &mood_only).unwrap());

        // Pinned from Python: replacing selections rewrites the set.
        assert!(
            update_mood_entry(
                &conn,
                1,
                1,
                &MoodEntryUpdate {
                    selected_options: Some(vec![101]),
                    ..Default::default()
                }
            )
            .unwrap()
        );
        assert_eq!(
            serde_json::to_value(get_entry_selections(&conn, 1, None).unwrap()).unwrap(),
            json!([{"id": 101, "name": "Cooking", "group_name": "ZTest Activities"}])
        );

        // Pinned from Python: `time` overwrites created_at verbatim.
        assert!(
            update_mood_entry(
                &conn,
                1,
                2,
                &MoodEntryUpdate {
                    time: Some("2025-08-02 09:30:00".to_string()),
                    ..Default::default()
                }
            )
            .unwrap()
        );
        let entry = get_mood_entry_by_id(&conn, 1, 2).unwrap().unwrap();
        assert_eq!(entry.created_at.as_deref(), Some("2025-08-02 09:30:00"));
    }

    #[test]
    fn delete_semantics_match_python() {
        let (_dir, conn) = seeded_db();
        // Pinned from Python: True / False on repeat / False for a foreign
        // user's attempt (row survives).
        assert!(delete_mood_entry(&conn, 1, 4).unwrap());
        assert!(!delete_mood_entry(&conn, 1, 4).unwrap());
        assert!(!delete_mood_entry(&conn, 2, 1).unwrap());
        assert_eq!(
            ids(&get_all_mood_entries(&conn, 1).unwrap()),
            [6, 3, 5, 2, 1]
        );

        // ON DELETE CASCADE clears the deleted entry's selections (FKs are
        // ON via the connection defaults).
        assert!(delete_mood_entry(&conn, 1, 1).unwrap());
        let orphans: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM entry_selections WHERE entry_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(orphans, 0);
    }

    #[test]
    fn two_response_shapes_with_and_without_selections() {
        let (_dir, conn) = seeded_db();

        // GET shape: exactly the six columns, no `selections` key.
        let entry = get_mood_entry_by_id(&conn, 1, 1).unwrap().unwrap();
        let value = serde_json::to_value(&entry).unwrap();
        let mut keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            ["content", "created_at", "date", "id", "mood", "updated_at"]
        );

        // PUT shape: same entry flattened plus a `selections` array.
        let with = get_mood_entry_with_selections(&conn, 1, 1)
            .unwrap()
            .unwrap();
        let value = serde_json::to_value(&with).unwrap();
        let object = value.as_object().unwrap();
        assert_eq!(object.len(), 7);
        assert_eq!(
            object["selections"],
            json!([
                {"id": 102, "name": "happy", "group_name": "ATest Emotions"},
                {"id": 100, "name": "Exercise", "group_name": "ZTest Activities"},
            ])
        );
        assert_eq!(object["date"], "2025-08-01");

        assert!(
            get_mood_entry_with_selections(&conn, 1, 9999)
                .unwrap()
                .is_none()
        );
        // Foreign entry: ownership check fails before the selections read.
        assert!(
            get_mood_entry_with_selections(&conn, 1, 7)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn sql_text_matches_python_composition() {
        // The Python builds its list queries by f-string interpolation of
        // `_ENTRY_ORDER` (itself built from `_ISO_DAY_EXPR`); the constants
        // here are compile-time `concat!`s of the same fragments. Guard the
        // seams so a reformat cannot silently change the SQL text.
        assert!(GET_ALL_MOOD_ENTRIES_SQL.contains("ORDER BY \n    CASE"));
        assert!(GET_ALL_MOOD_ENTRIES_SQL.contains("    END\n DESC, created_at DESC"));
        assert!(GET_MOOD_ENTRIES_BY_DATE_RANGE_SQL.contains("date BETWEEN ? AND ?"));
        assert!(
            GET_MOOD_ENTRIES_BY_DATE_RANGE_SQL.contains(
                "|| printf('%02d', CAST(substr(date, 1, instr(date, '/') - 1) AS INTEGER))"
            )
        );
    }
}
