//! PostgreSQL twin of [`crate::db::moods`] (WS4; docs/plans/v0.6.0.md
//! Feature 4, "PG wire-quirk policy").
//!
//! Every function here mirrors one sync operation statement-by-statement;
//! the graded contract is the **wire** shape the fixtures pin, so SQLite's
//! observable quirks are reproduced on purpose:
//!
//! - `mood` is `double precision` on Postgres; integral reads map to
//!   `MoodValue::Int` via [`util::mood_value_from_f64`] so an inserted `4`
//!   serializes as `4` (SQLite's INTEGER affinity) while a schema-legal
//!   `4.5` stays a float.
//! - [`PG_ISO_DAY_EXPR`] rebuilds the sync `iso_day_expr!` normalisation
//!   with a regex `CASE`, reproducing even the garbage-date quirks
//!   (`13/45/2025` → `2025-13-45`, `a/b/2025` → `2025-00-00`, SQLite
//!   `CAST`'s numeric-prefix parsing, and `printf('%02d', …)`'s
//!   no-truncation padding). The stats twin composes the same fragment.
//! - List ordering appends `NULLS LAST`: SQLite sorts NULLs first ASC /
//!   last DESC, Postgres the opposite — without it, unparseable dates
//!   would float to the top instead of sorting last under `DESC`.
//! - The date-range filter stays a raw-string `BETWEEN` on the stored
//!   `date` column (DECISIONS.md #4); `COLLATE "C"` on the column keeps
//!   the comparison byte-lexicographic like SQLite's BINARY collation.
//! - `last_insert_rowid()` sites become `RETURNING id`;
//!   `CURRENT_TIMESTAMP` writes become `nightlio_now()` (the 0001 baseline
//!   helper emitting SQLite's `YYYY-MM-DD HH:MM:SS` UTC text).
//!
//! Functions follow the `pg/util.rs` conventions: they take
//! `&impl GenericClient` (`&mut` when they open a transaction) so store
//! ops can share one checked-out connection across domains, exactly like
//! the SQLite closures share one `&Connection`.

use std::sync::LazyLock;

use tokio_postgres::types::ToSql;
use tokio_postgres::{GenericClient, Row};

use super::util;
use crate::db::common::DatabaseError;
use crate::db::moods::{EntrySelectionRow, MoodEntryRow, MoodEntryUpdate, MoodEntryWithSelections};

// --- The normalisation fragment (shared with the stats twin) -----------------

/// One padded `M`/`D` segment of the US-format rebuild: the exact output of
/// SQLite's `printf('%02d', CAST(segment AS INTEGER))`.
///
/// - `CAST(x AS INTEGER)` parses an optional-whitespace/sign numeric
///   *prefix* and yields 0 when there is none (`'a'` → 0, `'12x'` → 12,
///   `' 12'` → 12) — reproduced with `substring(... from
///   '^\s*[+-]?[0-9]+')` (Postgres' bigint cast tolerates the leading
///   whitespace the pattern keeps) and `COALESCE(…, 0)`.
/// - `printf('%02d', n)` left-pads to *at least* two characters but never
///   truncates (`5` → `05`, `45` → `45`, `123` → `123`, `-5` → `-5`) —
///   reproduced with `lpad(n::text, GREATEST(length(n::text), 2), '0')`
///   (a plain `lpad(…, 2, '0')` would truncate three-digit garbage, and
///   `to_char(n, 'FM00')` would emit `##`).
fn padded_us_segment(index: u8) -> String {
    let number = format!(
        "COALESCE(NULLIF(substring(split_part(date, '/', {index}) from '^\\s*[+-]?[0-9]+'), '')::bigint, 0)"
    );
    format!("lpad(({number})::text, GREATEST(length(({number})::text), 2), '0')")
}

/// PostgreSQL rebuild of the sync `iso_day_expr!` / `ISO_DAY_EXPR`
/// fragment: normalise a stored `mood_entries.date` to ISO `YYYY-MM-DD`,
/// or NULL when it matches neither the ISO nor the US `M/D/YYYY` shape.
///
/// The GLOB patterns become regexes with the same accept set for
/// slash-count ≤ 2 (SQLite's `*/*/[0-9][0-9][0-9][0-9]` also admits
/// strings with three or more slashes, e.g. `a/b/c/2025` → `2025-00-00`;
/// the regex rejects those — an accepted, unreachable-through-the-UI
/// divergence). Garbage quirks are preserved: `13/45/2025` → `2025-13-45`.
pub(crate) static PG_ISO_DAY_EXPR: LazyLock<String> = LazyLock::new(|| {
    format!(
        r"CASE
        WHEN date ~ '^\d{{4}}-\d{{2}}-\d{{2}}$' THEN date
        WHEN date ~ '^[^/]*/[^/]*/\d{{4}}$' THEN
            right(date, 4)
            || '-'
            || {month}
            || '-'
            || {day}
        ELSE NULL
    END",
        month = padded_us_segment(1),
        day = padded_us_segment(2),
    )
});

// --- SQL ---------------------------------------------------------------------

/// Twin of the sync `entry_order!`: newest normalised journaled day first,
/// then creation time within the day. `NULLS LAST` reproduces SQLite's
/// NULL ordering (last under DESC) so unparseable dates sort last.
static PG_ENTRY_ORDER: LazyLock<String> = LazyLock::new(|| {
    format!(
        "ORDER BY {expr} DESC NULLS LAST, created_at DESC NULLS LAST",
        expr = PG_ISO_DAY_EXPR.as_str()
    )
});

const INSERT_MOOD_ENTRY_WITH_TIME_SQL: &str = "INSERT INTO mood_entries \
    (user_id, date, mood, content, created_at) VALUES ($1, $2, $3, $4, $5) RETURNING id";

const INSERT_MOOD_ENTRY_SQL: &str = "INSERT INTO mood_entries \
    (user_id, date, mood, content) VALUES ($1, $2, $3, $4) RETURNING id";

const INSERT_ENTRY_SELECTION_SQL: &str =
    "INSERT INTO entry_selections (entry_id, option_id) VALUES ($1, $2)";

static GET_ALL_MOOD_ENTRIES_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT id, date, mood, content, created_at, updated_at \
         FROM mood_entries WHERE user_id = $1 {order}",
        order = PG_ENTRY_ORDER.as_str()
    )
});

/// DECISIONS.md #4: raw-string `BETWEEN` on the stored date column (the
/// range filter and the list ordering deliberately use different date
/// semantics). `COLLATE "C"` on the column makes the comparison
/// byte-lexicographic, matching SQLite BINARY.
static GET_MOOD_ENTRIES_BY_DATE_RANGE_SQL: LazyLock<String> = LazyLock::new(|| {
    format!(
        "SELECT id, date, mood, content, created_at, updated_at \
         FROM mood_entries WHERE user_id = $1 AND date BETWEEN $2 AND $3 {order}",
        order = PG_ENTRY_ORDER.as_str()
    )
});

const GET_MOOD_ENTRY_BY_ID_SQL: &str = "SELECT id, date, mood, content, created_at, updated_at \
    FROM mood_entries WHERE id = $1 AND user_id = $2";

const SELECT_ENTRY_EXISTS_SQL: &str = "SELECT id FROM mood_entries WHERE id = $1 AND user_id = $2";

const TOUCH_UPDATED_AT_SQL: &str =
    "UPDATE mood_entries SET updated_at = nightlio_now() WHERE id = $1 AND user_id = $2";

const DELETE_ENTRY_SELECTIONS_SQL: &str = "DELETE FROM entry_selections WHERE entry_id = $1";

const DELETE_MOOD_ENTRY_SQL: &str = "DELETE FROM mood_entries WHERE id = $1 AND user_id = $2";

/// User-scoped selections read (ownership verified via the join); ordering
/// by group then option name is byte-order via the columns' `COLLATE "C"`.
const GET_ENTRY_SELECTIONS_SCOPED_SQL: &str = "SELECT go.id, go.name, g.name as group_name \
    FROM entry_selections es \
    JOIN mood_entries me ON es.entry_id = me.id \
    JOIN group_options go ON es.option_id = go.id \
    JOIN groups g ON go.group_id = g.id \
    WHERE es.entry_id = $1 AND me.user_id = $2 \
    ORDER BY g.name, go.name";

/// Unscoped variant — the caller must have verified entry ownership.
const GET_ENTRY_SELECTIONS_SQL: &str = "SELECT go.id, go.name, g.name as group_name \
    FROM entry_selections es \
    JOIN group_options go ON es.option_id = go.id \
    JOIN groups g ON go.group_id = g.id \
    WHERE es.entry_id = $1 \
    ORDER BY g.name, go.name";

// --- Row mapping -------------------------------------------------------------

fn map_entry(row: &Row) -> Result<MoodEntryRow, tokio_postgres::Error> {
    Ok(MoodEntryRow {
        id: row.try_get("id")?,
        date: row.try_get("date")?,
        mood: util::mood_value_from_f64(row.try_get("mood")?),
        content: row.try_get("content")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn map_selection(row: &Row) -> Result<EntrySelectionRow, tokio_postgres::Error> {
    Ok(EntrySelectionRow {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        group_name: row.try_get("group_name")?,
    })
}

fn collect_entries(rows: Vec<Row>) -> Result<Vec<MoodEntryRow>, DatabaseError> {
    rows.iter()
        .map(|row| map_entry(row).map_err(util::db_error))
        .collect()
}

// --- Queries -----------------------------------------------------------------

/// Twin of `moods::add_mood_entry`. Returns the new entry id (`RETURNING
/// id` replaces `last_insert_rowid()`). An empty-string `time` falls
/// through to the no-time INSERT (Python truthiness), letting `created_at`
/// take its `nightlio_now()` default.
pub async fn add_mood_entry(
    client: &mut impl GenericClient,
    user_id: i64,
    date: &str,
    mood: i64,
    content: &str,
    time: Option<&str>,
    selected_options: Option<&[i64]>,
) -> Result<i64, DatabaseError> {
    let tx = client.transaction().await.map_err(util::db_error)?;
    // The column is double precision; SQLite's INTEGER affinity makes the
    // read side fold integral values back to bare integers (`map_entry`).
    let mood = mood as f64;
    let row = match time {
        Some(time) if !time.is_empty() => {
            tx.query_one(
                INSERT_MOOD_ENTRY_WITH_TIME_SQL,
                &[&user_id, &date, &mood, &content, &time],
            )
            .await
        }
        _ => {
            tx.query_one(INSERT_MOOD_ENTRY_SQL, &[&user_id, &date, &mood, &content])
                .await
        }
    }
    .map_err(util::db_error)?;
    let entry_id: i64 = row.try_get(0).map_err(util::db_error)?;

    // Python: `if selected_options:` — an empty list inserts nothing.
    if let Some(options) = selected_options
        && !options.is_empty()
    {
        let stmt = tx
            .prepare(INSERT_ENTRY_SELECTION_SQL)
            .await
            .map_err(util::db_error)?;
        for option_id in options {
            tx.execute(&stmt, &[&entry_id, option_id])
                .await
                .map_err(util::db_error)?;
        }
    }

    tx.commit().await.map_err(util::db_error)?;
    Ok(entry_id)
}

/// Twin of `moods::get_all_mood_entries`.
pub async fn get_all_mood_entries(
    client: &impl GenericClient,
    user_id: i64,
) -> Result<Vec<MoodEntryRow>, DatabaseError> {
    let rows = client
        .query(GET_ALL_MOOD_ENTRIES_SQL.as_str(), &[&user_id])
        .await
        .map_err(util::db_error)?;
    collect_entries(rows)
}

/// Twin of `moods::get_mood_entries_by_date_range` (raw-string `BETWEEN`,
/// DECISIONS.md #4).
pub async fn get_mood_entries_by_date_range(
    client: &impl GenericClient,
    user_id: i64,
    start_date: &str,
    end_date: &str,
) -> Result<Vec<MoodEntryRow>, DatabaseError> {
    let rows = client
        .query(
            GET_MOOD_ENTRIES_BY_DATE_RANGE_SQL.as_str(),
            &[&user_id, &start_date, &end_date],
        )
        .await
        .map_err(util::db_error)?;
    collect_entries(rows)
}

/// Twin of `moods::get_mood_entry_by_id` (GET shape: no `selections` key).
pub async fn get_mood_entry_by_id(
    client: &impl GenericClient,
    user_id: i64,
    entry_id: i64,
) -> Result<Option<MoodEntryRow>, DatabaseError> {
    let row = client
        .query_opt(GET_MOOD_ENTRY_BY_ID_SQL, &[&entry_id, &user_id])
        .await
        .map_err(util::db_error)?;
    row.map(|row| map_entry(&row).map_err(util::db_error))
        .transpose()
}

/// Twin of `moods::update_mood_entry` — same return semantics, including
/// the bug-compatible "no fields still touches `updated_at` but returns
/// `false`" branch.
pub async fn update_mood_entry(
    client: &mut impl GenericClient,
    user_id: i64,
    entry_id: i64,
    update: &MoodEntryUpdate,
) -> Result<bool, DatabaseError> {
    let mood = update.mood.map(|mood| mood as f64);
    let mut sets: Vec<String> = Vec::new();
    let mut params: Vec<&(dyn ToSql + Sync)> = Vec::new();
    if let Some(mood) = mood.as_ref() {
        params.push(mood);
        sets.push(format!("mood = ${}", params.len()));
    }
    if let Some(content) = update.content.as_ref() {
        params.push(content);
        sets.push(format!("content = ${}", params.len()));
    }
    if let Some(date) = update.date.as_ref() {
        params.push(date);
        sets.push(format!("date = ${}", params.len()));
    }
    if let Some(time) = update.time.as_ref() {
        params.push(time);
        sets.push(format!("created_at = ${}", params.len()));
    }

    let exists = client
        .query_opt(SELECT_ENTRY_EXISTS_SQL, &[&entry_id, &user_id])
        .await
        .map_err(util::db_error)?;
    if exists.is_none() {
        return Ok(false);
    }

    let tx = client.transaction().await.map_err(util::db_error)?;
    let mut updated = false;
    if !sets.is_empty() {
        sets.push("updated_at = nightlio_now()".to_string());
        let sql = format!(
            "UPDATE mood_entries SET {} WHERE id = ${} AND user_id = ${}",
            sets.join(", "),
            params.len() + 1,
            params.len() + 2
        );
        params.push(&entry_id);
        params.push(&user_id);
        tx.execute(&sql, &params).await.map_err(util::db_error)?;
        updated = true;
    } else {
        tx.execute(TOUCH_UPDATED_AT_SQL, &[&entry_id, &user_id])
            .await
            .map_err(util::db_error)?;
    }

    if let Some(options) = update.selected_options.as_ref() {
        tx.execute(DELETE_ENTRY_SELECTIONS_SQL, &[&entry_id])
            .await
            .map_err(util::db_error)?;
        if !options.is_empty() {
            let stmt = tx
                .prepare(INSERT_ENTRY_SELECTION_SQL)
                .await
                .map_err(util::db_error)?;
            for option_id in options {
                tx.execute(&stmt, &[&entry_id, option_id])
                    .await
                    .map_err(util::db_error)?;
            }
        }
        updated = true;
    }

    tx.commit().await.map_err(util::db_error)?;
    Ok(updated || update.selected_options.is_some())
}

/// Twin of `moods::delete_mood_entry` — `true` only when a row owned by
/// the user was actually deleted.
pub async fn delete_mood_entry(
    client: &impl GenericClient,
    user_id: i64,
    entry_id: i64,
) -> Result<bool, DatabaseError> {
    let rows = client
        .execute(DELETE_MOOD_ENTRY_SQL, &[&entry_id, &user_id])
        .await
        .map_err(util::db_error)?;
    Ok(rows > 0)
}

/// Twin of `moods::get_entry_selections` (scoped/unscoped variants).
pub async fn get_entry_selections(
    client: &impl GenericClient,
    entry_id: i64,
    user_id: Option<i64>,
) -> Result<Vec<EntrySelectionRow>, DatabaseError> {
    let rows = match user_id {
        Some(user_id) => client
            .query(GET_ENTRY_SELECTIONS_SCOPED_SQL, &[&entry_id, &user_id])
            .await
            .map_err(util::db_error)?,
        None => client
            .query(GET_ENTRY_SELECTIONS_SQL, &[&entry_id])
            .await
            .map_err(util::db_error)?,
    };
    rows.iter()
        .map(|row| map_selection(row).map_err(util::db_error))
        .collect()
}

/// Twin of `moods::get_mood_entry_with_selections` (the PUT response
/// composition: user-scoped entry re-read, then the unscoped selections).
pub async fn get_mood_entry_with_selections(
    client: &impl GenericClient,
    user_id: i64,
    entry_id: i64,
) -> Result<Option<MoodEntryWithSelections>, DatabaseError> {
    let Some(entry) = get_mood_entry_by_id(client, user_id, entry_id).await? else {
        return Ok(None);
    };
    let selections = get_entry_selections(client, entry_id, None).await?;
    Ok(Some(MoodEntryWithSelections { entry, selections }))
}

#[cfg(test)]
mod tests {
    use super::super::util::test_support::connect_scratch;
    use super::*;
    use crate::db::common::MoodValue;
    use serde_json::json;

    /// Seed mirroring the sync `moods::tests::seeded_db` fixture (whose
    /// expected values were pinned from the real Python data layer), plus
    /// group 12 `'apple'` to make the C-collation ordering discriminating
    /// (locale collations would sort `'apple'` before `'ATest …'`).
    async fn seed(client: &mut tokio_postgres::Client) {
        client
            .batch_execute(
                "INSERT INTO users (id, google_id, email, name) VALUES (1, 'u1', 'u1@x', 'One');
                 INSERT INTO users (id, google_id, email, name) VALUES (2, 'u2', 'u2@x', 'Two');
                 INSERT INTO groups (id, user_id, name) VALUES (10, 1, 'ZTest Activities');
                 INSERT INTO groups (id, user_id, name) VALUES (11, 1, 'ATest Emotions');
                 INSERT INTO groups (id, user_id, name) VALUES (12, 1, 'apple');
                 INSERT INTO group_options (id, group_id, name) VALUES (100, 10, 'Exercise');
                 INSERT INTO group_options (id, group_id, name) VALUES (101, 10, 'Cooking');
                 INSERT INTO group_options (id, group_id, name) VALUES (102, 11, 'happy');
                 INSERT INTO group_options (id, group_id, name) VALUES (120, 12, 'banana');",
            )
            .await
            .expect("seed users/groups");

        type SeedRow<'a> = (&'a str, i64, &'a str, &'a str, Option<&'a [i64]>);
        let seeds: [SeedRow<'_>; 6] = [
            (
                "2025-08-01",
                4,
                "Great workout day.",
                "2025-08-01 08:00:00",
                Some(&[102, 100, 120]),
            ),
            (
                "8/2/2025",
                2,
                "Rough day, US-format date.",
                "2025-08-02 09:00:00",
                None,
            ),
            ("2025-08-03", 5, "Third day.", "2025-08-03 10:00:00", None),
            ("not-a-date", 3, "Unparseable.", "2025-08-04 11:00:00", None),
            (
                "2025-08-03",
                1,
                "Backdated later insert.",
                "2025-08-03 06:00:00",
                None,
            ),
            (
                "12/31/2025",
                3,
                "US New Year's Eve.",
                "2025-12-31 23:00:00",
                None,
            ),
        ];
        for (index, (date, mood, content, time, options)) in seeds.into_iter().enumerate() {
            let id = add_mood_entry(client, 1, date, mood, content, Some(time), options)
                .await
                .unwrap();
            assert_eq!(id, index as i64 + 1, "seed must produce ids 1..=6");
        }
        let other = add_mood_entry(
            client,
            2,
            "2025-08-02",
            3,
            "Someone else's entry.",
            Some("2025-08-02 12:00:00"),
            None,
        )
        .await
        .unwrap();
        assert_eq!(other, 7);
    }

    fn ids(rows: &[MoodEntryRow]) -> Vec<i64> {
        rows.iter().map(|row| row.id).collect()
    }

    /// Quirk rows exercised: ISO-day normalisation ordering with NULLS
    /// LAST (unparseable dates sort last under DESC), raw-string lexico
    /// BETWEEN (DECISIONS.md #4), and COLLATE "C" name ordering
    /// (uppercase before lowercase, like SQLite BINARY).
    #[tokio::test]
    async fn pg_entry_order_range_and_collation_match_sqlite() {
        let Some(mut client) = connect_scratch("ws4b_moods_order").await else {
            return;
        };
        seed(&mut client).await;

        // Pinned from Python/SQLite: [6, 3, 5, 2, 1, 4] — US 12/31/2025
        // first, created_at DESC breaks the 2025-08-03 tie, 8/2/2025 sorts
        // between the ISO days, the unparseable date lands last.
        assert_eq!(
            ids(&get_all_mood_entries(&client, 1).await.unwrap()),
            [6, 3, 5, 2, 1, 4]
        );
        assert_eq!(ids(&get_all_mood_entries(&client, 2).await.unwrap()), [7]);

        // Pinned from Python/SQLite: the ISO range excludes the US-format
        // rows even though they are chronologically inside it...
        assert_eq!(
            ids(
                &get_mood_entries_by_date_range(&client, 1, "2025-08-01", "2025-08-31")
                    .await
                    .unwrap()
            ),
            [3, 5, 1]
        );
        // ...and a lexicographic "0".."9" range catches both US-format
        // rows but still not 'not-a-date' ('n' > '9').
        assert_eq!(
            ids(&get_mood_entries_by_date_range(&client, 1, "0", "9")
                .await
                .unwrap()),
            [6, 3, 5, 2, 1]
        );

        // Pinned from SQLite BINARY collation (python3 sqlite3,
        // 2026-08-22): 'ATest Emotions' < 'ZTest Activities' < 'apple'
        // (byte order — a locale collation would sort 'apple' first).
        let rows = get_entry_selections(&client, 1, None).await.unwrap();
        assert_eq!(
            serde_json::to_value(&rows).unwrap(),
            json!([
                {"id": 102, "name": "happy", "group_name": "ATest Emotions"},
                {"id": 100, "name": "Exercise", "group_name": "ZTest Activities"},
                {"id": 120, "name": "banana", "group_name": "apple"},
            ])
        );
        // Scoped variant: same rows for the owner, empty (never an error)
        // for a foreign user.
        assert_eq!(
            serde_json::to_value(get_entry_selections(&client, 1, Some(1)).await.unwrap()).unwrap(),
            serde_json::to_value(&rows).unwrap()
        );
        assert!(
            get_entry_selections(&client, 1, Some(2))
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// Quirk row exercised: `mood double precision` with the integral-f64 →
    /// `Int` wire mapping — an inserted integer serializes as `4`, a
    /// schema-legal `4.5` as the float `4.5`, exactly the MoodValue shape.
    #[tokio::test]
    async fn pg_mood_value_wire_shape_matches_sqlite() {
        let Some(mut client) = connect_scratch("ws4b_moods_moodvalue").await else {
            return;
        };
        seed(&mut client).await;

        let entry = get_mood_entry_by_id(&client, 1, 1).await.unwrap().unwrap();
        assert_eq!(entry.mood, MoodValue::Int(4));
        assert_eq!(serde_json::to_value(&entry).unwrap()["mood"], json!(4));
        assert_eq!(entry.date, "2025-08-01");
        assert_eq!(entry.created_at.as_deref(), Some("2025-08-01 08:00:00"));
        assert!(entry.updated_at.is_some());

        // Schema-legal REAL mood (passes the 1..5 CHECK) serves as 4.5.
        client
            .batch_execute(
                "INSERT INTO mood_entries (user_id, date, mood, content, created_at)
                 VALUES (1, '2025-08-06', 4.5, 'Real-typed mood.', '2025-08-06 08:00:00');",
            )
            .await
            .unwrap();
        let all = get_all_mood_entries(&client, 1).await.unwrap();
        let real = all.iter().find(|row| row.date == "2025-08-06").unwrap();
        assert_eq!(real.mood, MoodValue::Float(4.5));
        assert_eq!(serde_json::to_value(real).unwrap()["mood"], json!(4.5));

        // GET shape has exactly the six columns; the PUT composition adds
        // the flattened `selections` array.
        let value = serde_json::to_value(&entry).unwrap();
        let mut keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(
            keys,
            ["content", "created_at", "date", "id", "mood", "updated_at"]
        );
        let with = get_mood_entry_with_selections(&client, 1, 1)
            .await
            .unwrap()
            .unwrap();
        let value = serde_json::to_value(&with).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 7);
        assert_eq!(value["date"], "2025-08-01");
    }

    /// Update/delete semantics parity: the bug-compatible false-but-touch
    /// branch, `Some(vec![])` counting as an update, `time` overwriting
    /// `created_at` verbatim, foreign/missing rows untouched, and the
    /// empty-string `time` insert falling back to the `nightlio_now()`
    /// default.
    #[tokio::test]
    async fn pg_update_delete_semantics_match_sqlite() {
        let Some(mut client) = connect_scratch("ws4b_moods_update").await else {
            return;
        };
        seed(&mut client).await;

        // Empty-string time takes the default created_at branch.
        let id = add_mood_entry(
            &mut client,
            1,
            "2025-08-05",
            3,
            "Empty time string.",
            Some(""),
            None,
        )
        .await
        .unwrap();
        let entry = get_mood_entry_by_id(&client, 1, id).await.unwrap().unwrap();
        let created_at = entry.created_at.expect("nightlio_now() default");
        assert_eq!(
            created_at.len(),
            19,
            "YYYY-MM-DD HH:MM:SS text: {created_at}"
        );

        // No fields + no selected_options: false, but updated_at touched.
        client
            .batch_execute(
                "UPDATE mood_entries SET updated_at = '1999-01-01 00:00:00' WHERE id = 2;",
            )
            .await
            .unwrap();
        assert!(
            !update_mood_entry(&mut client, 1, 2, &MoodEntryUpdate::default())
                .await
                .unwrap()
        );
        let touched = get_mood_entry_by_id(&client, 1, 2).await.unwrap().unwrap();
        assert_ne!(touched.updated_at.as_deref(), Some("1999-01-01 00:00:00"));

        // Some(vec![]) always counts as an update, even with nothing to
        // clear.
        assert!(
            update_mood_entry(
                &mut client,
                1,
                3,
                &MoodEntryUpdate {
                    selected_options: Some(vec![]),
                    ..Default::default()
                }
            )
            .await
            .unwrap()
        );

        // Replacing selections rewrites the set and returns true.
        assert!(
            update_mood_entry(
                &mut client,
                1,
                1,
                &MoodEntryUpdate {
                    selected_options: Some(vec![101]),
                    ..Default::default()
                }
            )
            .await
            .unwrap()
        );
        assert_eq!(
            serde_json::to_value(get_entry_selections(&client, 1, None).await.unwrap()).unwrap(),
            json!([{"id": 101, "name": "Cooking", "group_name": "ZTest Activities"}])
        );

        // Partial mood+content update leaves the stored date untouched.
        assert!(
            update_mood_entry(
                &mut client,
                1,
                2,
                &MoodEntryUpdate {
                    mood: Some(3),
                    content: Some("Updated content.".to_string()),
                    ..Default::default()
                }
            )
            .await
            .unwrap()
        );
        let entry = get_mood_entry_by_id(&client, 1, 2).await.unwrap().unwrap();
        assert_eq!(
            (entry.mood, entry.content.as_str(), entry.date.as_str()),
            (MoodValue::Int(3), "Updated content.", "8/2/2025")
        );

        // Missing and foreign entries return false.
        let mood_only = MoodEntryUpdate {
            mood: Some(3),
            ..Default::default()
        };
        assert!(
            !update_mood_entry(&mut client, 1, 9999, &mood_only)
                .await
                .unwrap()
        );
        assert!(
            !update_mood_entry(&mut client, 2, 2, &mood_only)
                .await
                .unwrap()
        );

        // `time` overwrites created_at verbatim.
        assert!(
            update_mood_entry(
                &mut client,
                1,
                2,
                &MoodEntryUpdate {
                    time: Some("2025-08-02 09:30:00".to_string()),
                    ..Default::default()
                }
            )
            .await
            .unwrap()
        );
        let entry = get_mood_entry_by_id(&client, 1, 2).await.unwrap().unwrap();
        assert_eq!(entry.created_at.as_deref(), Some("2025-08-02 09:30:00"));

        // Delete: true / false on repeat / false for a foreign user; the
        // FK cascade clears the deleted entry's selections.
        assert!(delete_mood_entry(&client, 1, 4).await.unwrap());
        assert!(!delete_mood_entry(&client, 1, 4).await.unwrap());
        assert!(!delete_mood_entry(&client, 2, 1).await.unwrap());
        assert!(delete_mood_entry(&client, 1, 1).await.unwrap());
        assert!(
            get_entry_selections(&client, 1, None)
                .await
                .unwrap()
                .is_empty()
        );
    }

    // --- Pure helpers (no live PG needed) ------------------------------------

    #[test]
    fn iso_day_expr_contains_the_quirk_preserving_fragments() {
        let expr = PG_ISO_DAY_EXPR.as_str();
        assert!(expr.contains(r"date ~ '^\d{4}-\d{2}-\d{2}$'"));
        assert!(expr.contains(r"date ~ '^[^/]*/[^/]*/\d{4}$'"));
        // printf('%02d', …) parity: pad to at least 2, never truncate.
        assert!(expr.contains("GREATEST(length("));
        // SQLite CAST parity: numeric prefix with optional sign/space.
        assert!(expr.contains(r"'^\s*[+-]?[0-9]+'"));
    }
}
