//! Port of `api/database_activity.py`. Owned by the activity data-mixin agent —
//! no other agent edits this file.
//!
//! Stores an append-only per-user event feed (logins, entry mutations,
//! achievements, goal completions). Reads are keyset-paginated on the row
//! id — never OFFSET, which degrades as the table grows — and old rows can
//! be pruned so a self-hosted database file stays small and portable.
//!
//! Known event types: `login`, `entry_created`, `entry_edited`,
//! `entry_deleted`, `achievement_unlocked`, `goal_completed`.
//!
//! Every function takes a `&Connection`; callers run them under
//! `tokio::task::spawn_blocking`.

use rusqlite::{Connection, params};
use serde::Serialize;

use super::common::DatabaseError;

/// Hard ceiling for a single page of activity rows
/// (`MAX_ACTIVITY_PAGE_SIZE` in `api/database_activity.py`).
pub const MAX_ACTIVITY_PAGE_SIZE: i64 = 200;

// --- SQL (byte-identical to the Python string literals) ----------------------
//
// These are activity-specific; `database_common.SQLQueries` has no activity
// constants, so they live here, verbatim including the triple-quoted
// indentation, mirroring how `common::sql_queries` preserves its strings.

/// `ActivityLogMixin.add_activity` INSERT.
const INSERT_ACTIVITY_SQL: &str = "
                INSERT INTO activity_log (user_id, event_type, metadata)
                VALUES (?, ?, ?)
                ";

/// `ActivityLogMixin.get_activity` SELECT when `before` is provided.
const SELECT_ACTIVITY_BEFORE_SQL: &str = "
                    SELECT id, user_id, event_type, metadata, created_at
                      FROM activity_log
                     WHERE user_id = ? AND id < ?
                     ORDER BY id DESC
                     LIMIT ?
                    ";

/// `ActivityLogMixin.get_activity` SELECT for the first (newest) page.
const SELECT_ACTIVITY_FIRST_PAGE_SQL: &str = "
                    SELECT id, user_id, event_type, metadata, created_at
                      FROM activity_log
                     WHERE user_id = ?
                     ORDER BY id DESC
                     LIMIT ?
                    ";

/// `ActivityLogMixin.prune_activity` DELETE.
const PRUNE_ACTIVITY_SQL: &str = "DELETE FROM activity_log WHERE created_at < datetime('now', ?)";

// --- Row / page shapes -------------------------------------------------------

/// One activity_log row as the Flask API emits it (`dict(row)` with
/// `metadata` replaced by its parsed JSON, or `null`). Field names match the
/// JSON keys in `contract/fixtures/misc/activity_get_*.json` exactly.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ActivityRow {
    pub id: i64,
    pub user_id: i64,
    pub event_type: String,
    /// Parsed JSON metadata. `None` both when the column is NULL and when
    /// the stored blob fails to parse (the "Malformed activity metadata"
    /// tolerance — the row is still returned, never an error).
    pub metadata: Option<serde_json::Value>,
    /// SQLite `CURRENT_TIMESTAMP` text (`YYYY-MM-DD HH:MM:SS`, UTC).
    /// `Option` defensively, matching the Python's tolerance of odd rows.
    pub created_at: Option<String>,
}

/// The `{activities, next_cursor}` page body
/// `api/routes/activity_routes.py::get_activity` builds around the mixin.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ActivityPage {
    pub activities: Vec<ActivityRow>,
    pub next_cursor: Option<i64>,
}

// --- Queries -----------------------------------------------------------------

/// Port of `ActivityLogMixin.add_activity`: record an event for a user;
/// returns the new row id.
///
/// `metadata` is stored as a JSON blob. Callers must never put tokens or
/// IDP responses in it.
pub fn add_activity(
    conn: &Connection,
    user_id: i64,
    event_type: &str,
    metadata: Option<&serde_json::Value>,
) -> Result<i64, DatabaseError> {
    // Python: `json.dumps(metadata) if metadata is not None else None`.
    // (serde_json omits json.dumps' ", "/": " separators; the stored blob
    // parses identically, and only the parsed form is ever emitted.)
    let payload = match metadata {
        Some(value) => Some(
            serde_json::to_string(value)
                .map_err(|exc| DatabaseError::Message(format!("Database error: {exc}")))?,
        ),
        None => None,
    };
    conn.execute(INSERT_ACTIVITY_SQL, params![user_id, event_type, payload])?;
    Ok(conn.last_insert_rowid())
}

/// Port of `ActivityLogMixin.get_activity`: a page of the user's activity,
/// newest first.
///
/// Keyset pagination: `before` is the `id` of the last row from the
/// previous page; pass it back to fetch the next (older) page. Row ids are
/// monotonically increasing, so id order matches insertion order and breaks
/// created_at ties deterministically. `limit` is clamped to
/// 1..=`MAX_ACTIVITY_PAGE_SIZE`.
pub fn get_activity(
    conn: &Connection,
    user_id: i64,
    before: Option<i64>,
    limit: i64,
) -> Result<Vec<ActivityRow>, DatabaseError> {
    // Python: `limit = max(1, min(int(limit), MAX_ACTIVITY_PAGE_SIZE))`.
    let limit = limit.clamp(1, MAX_ACTIVITY_PAGE_SIZE);
    fetch_rows(conn, user_id, before, limit)
}

/// Shared row fetch for [`get_activity`] (which clamps) and
/// [`get_activity_page`] (which peeks one row past the clamped limit, so
/// `fetch_limit` may be `MAX_ACTIVITY_PAGE_SIZE + 1`). The SQL is unchanged;
/// only the bound LIMIT parameter differs.
fn fetch_rows(
    conn: &Connection,
    user_id: i64,
    before: Option<i64>,
    limit: i64,
) -> Result<Vec<ActivityRow>, DatabaseError> {
    let mut stmt = match before {
        Some(_) => conn.prepare(SELECT_ACTIVITY_BEFORE_SQL)?,
        None => conn.prepare(SELECT_ACTIVITY_FIRST_PAGE_SQL)?,
    };
    // Row before the metadata blob is parsed (`sqlite3.Row` equivalent).
    struct RawRow {
        id: i64,
        user_id: i64,
        event_type: String,
        metadata: Option<String>,
        created_at: Option<String>,
    }
    let map_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<RawRow> {
        Ok(RawRow {
            id: row.get(0)?,
            user_id: row.get(1)?,
            event_type: row.get(2)?,
            metadata: row.get(3)?,
            created_at: row.get(4)?,
        })
    };
    let raw_rows = match before {
        Some(before) => stmt
            .query_map(params![user_id, before, limit], map_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?,
        None => stmt
            .query_map(params![user_id, limit], map_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?,
    };

    let mut rows = Vec::with_capacity(raw_rows.len());
    for raw in raw_rows {
        // Defensive JSON parse, mirroring the Python's try/except around
        // `json.loads`: a malformed blob is logged and nulled, never an
        // error surfaced to the caller.
        let metadata = raw
            .metadata
            .and_then(|blob| match serde_json::from_str(&blob) {
                Ok(value) => Some(value),
                Err(_) => {
                    tracing::warn!("Malformed activity metadata for row {}", raw.id);
                    None
                }
            });
        rows.push(ActivityRow {
            id: raw.id,
            user_id: raw.user_id,
            event_type: raw.event_type,
            metadata,
            created_at: raw.created_at,
        });
    }
    Ok(rows)
}

/// `get_activity` plus the cursor computation for `GET /api/activity`:
/// fetch one row past the clamped limit and hand back the last *page*
/// row's id as `next_cursor` only when that extra (older) row exists.
///
/// contract change (`contract/DECISIONS.md`, post-cutover item #10):
/// the Flask route emitted a non-null `next_cursor` on a full *final*
/// page (forcing a wasted follow-up request that returned
/// `{activities: [], next_cursor: null}`). `next_cursor` is now `null`
/// whenever no older rows exist, even when the page is exactly full.
/// Everything else — the `{activities, next_cursor}` shape, the 1..=200
/// clamp, and `before` keyset semantics — is unchanged.
pub fn get_activity_page(
    conn: &Connection,
    user_id: i64,
    before: Option<i64>,
    limit: i64,
) -> Result<ActivityPage, DatabaseError> {
    // Route side clamps independently of the DB layer:
    // `clamped_limit = max(1, min(limit, 200))`.
    let clamped_limit = limit.clamp(1, MAX_ACTIVITY_PAGE_SIZE);
    // Peek: fetch limit + 1 rows (deliberately not re-clamped, so the peek
    // still works at limit == MAX_ACTIVITY_PAGE_SIZE).
    let mut activities = fetch_rows(conn, user_id, before, clamped_limit + 1)?;
    let next_cursor = if activities.len() as i64 > clamped_limit {
        activities.truncate(clamped_limit as usize);
        activities.last().map(|row| row.id)
    } else {
        None
    };
    Ok(ActivityPage {
        activities,
        next_cursor,
    })
}

/// Port of `ActivityLogMixin.prune_activity`: delete activity rows older
/// than `days` days; returns the count deleted.
///
/// Maintenance operation across all users (age-based retention), not a
/// per-user query. Not called automatically; wire it to a periodic task or
/// run it at startup (the Flask app calls `db.prune_activity(90)`).
pub fn prune_activity(conn: &Connection, days: i64) -> Result<usize, DatabaseError> {
    let deleted = conn.execute(PRUNE_ACTIVITY_SQL, params![format!("-{days} days")])?;
    if deleted > 0 {
        tracing::info!("Pruned {deleted} activity rows older than {days} days");
    }
    Ok(deleted)
}

// --- Tests -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    //! Seed data and expected values below are pinned from the Python side:
    //! `api/database_activity.py` run (via `api/venv/bin/python`) against an
    //! identically-seeded `MoodDatabase` tempfile, with `next_cursor`
    //! computed exactly as `api/routes/activity_routes.py` does. Timestamps
    //! are excluded from comparison (normalized to `<TS>` in the contract
    //! fixtures as well).

    use rusqlite::params;
    use serde_json::{Value, json};

    use super::*;
    use crate::db::bootstrap::SelfHostSeed;
    use crate::db::{bootstrap, common};

    /// Bootstrap a fresh DB and seed the exact rows the Python pinning run
    /// used: user 1 (default self-host) gets rows 1-7 and a malformed row 9;
    /// a second user owns row 8, so scoping is observable.
    fn seeded_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("activity.db");
        let path = path.to_str().unwrap();
        bootstrap(path, &SelfHostSeed::default()).unwrap();
        let conn = common::connect(path).unwrap();

        let uid = common::get_default_user_id(&conn, "selfhost_default_user")
            .unwrap()
            .expect("bootstrap seeds the default self-host user");
        assert_eq!(uid, 1);

        conn.execute(
            "INSERT INTO users (google_id, email, name) VALUES (?, ?, ?)",
            params!["other_user", "other@example.com", "Other"],
        )
        .unwrap();

        let ids = vec![
            add_activity(&conn, 1, "login", Some(&json!({"method": "selfhost"}))).unwrap(),
            add_activity(
                &conn,
                1,
                "entry_created",
                Some(&json!({"entry_id": 1, "date": "2026-08-10"})),
            )
            .unwrap(),
            add_activity(
                &conn,
                1,
                "achievement_unlocked",
                Some(&json!({"achievement_type": "first_entry"})),
            )
            .unwrap(),
            add_activity(
                &conn,
                1,
                "entry_created",
                Some(&json!({"entry_id": 2, "date": "2026-08-11"})),
            )
            .unwrap(),
            add_activity(
                &conn,
                1,
                "entry_created",
                Some(&json!({"entry_id": 3, "date": "2026-08-12"})),
            )
            .unwrap(),
            add_activity(
                &conn,
                1,
                "entry_created",
                Some(&json!({"entry_id": 4, "date": "2026-08-13"})),
            )
            .unwrap(),
            add_activity(
                &conn,
                1,
                "entry_created",
                Some(&json!({"entry_id": 5, "date": "2026-08-14"})),
            )
            .unwrap(),
            // Second user's row — must never leak into user 1's pages.
            add_activity(&conn, 2, "login", Some(&json!({"method": "selfhost"}))).unwrap(),
        ];
        assert_eq!(ids, vec![1, 2, 3, 4, 5, 6, 7, 8], "add_activity row ids");

        // Malformed metadata inserted raw (bypasses the JSON encoder), the
        // shape the defensive parse exists for.
        conn.execute(
            "INSERT INTO activity_log (user_id, event_type, metadata) VALUES (?, ?, ?)",
            params![1, "entry_deleted", "{not-json"],
        )
        .unwrap();
        assert_eq!(conn.last_insert_rowid(), 9);

        (dir, conn)
    }

    /// Serialize a row and drop `created_at` (nondeterministic, `<TS>` in
    /// the contract fixtures too) so pages compare against pinned JSON.
    fn rows_without_ts(rows: &[ActivityRow]) -> Vec<Value> {
        rows.iter()
            .map(|row| {
                assert!(
                    row.created_at.is_some(),
                    "CURRENT_TIMESTAMP default must populate created_at (row {})",
                    row.id
                );
                let mut value = serde_json::to_value(row).unwrap();
                value.as_object_mut().unwrap().remove("created_at");
                value
            })
            .collect()
    }

    #[test]
    fn sql_constants_prepare_against_bootstrapped_schema() {
        let (_dir, conn) = seeded_db();
        for sql in [
            INSERT_ACTIVITY_SQL,
            SELECT_ACTIVITY_BEFORE_SQL,
            SELECT_ACTIVITY_FIRST_PAGE_SQL,
            PRUNE_ACTIVITY_SQL,
        ] {
            conn.prepare(sql)
                .unwrap_or_else(|exc| panic!("constant failed to prepare: {exc}\n{sql}"));
        }
    }

    #[test]
    fn first_page_limit_3_newest_first_with_cursor() {
        let (_dir, conn) = seeded_db();
        let page = get_activity_page(&conn, 1, None, 3).unwrap();
        // Python-pinned: rows [9, 7, 6] (8 belongs to user 2), next_cursor 6.
        assert_eq!(
            rows_without_ts(&page.activities),
            vec![
                json!({"id": 9, "user_id": 1, "event_type": "entry_deleted", "metadata": null}),
                json!({"id": 7, "user_id": 1, "event_type": "entry_created",
                       "metadata": {"entry_id": 5, "date": "2026-08-14"}}),
                json!({"id": 6, "user_id": 1, "event_type": "entry_created",
                       "metadata": {"entry_id": 4, "date": "2026-08-13"}}),
            ]
        );
        assert_eq!(page.next_cursor, Some(6));
    }

    #[test]
    fn keyset_page_before_6_returns_strictly_older_rows() {
        let (_dir, conn) = seeded_db();
        let page = get_activity_page(&conn, 1, Some(6), 3).unwrap();
        // Python-pinned: rows [5, 4, 3], next_cursor 3.
        assert_eq!(
            page.activities.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![5, 4, 3]
        );
        assert_eq!(
            page.activities[2].metadata,
            Some(json!({"achievement_type": "first_entry"}))
        );
        assert_eq!(page.next_cursor, Some(3));
    }

    #[test]
    fn limit_clamps_low_zero_and_negative_to_one() {
        let (_dir, conn) = seeded_db();
        // limit=0 and limit=-5 behave identically: one row [9], and —
        // because older rows (7..1) exist past the page — next_cursor is
        // set even at limit=0. (Same page/cursor the Python produced here;
        // the peek-ahead fix only changes full *final* pages.)
        for limit in [0, -5] {
            let page = get_activity_page(&conn, 1, None, limit).unwrap();
            assert_eq!(
                page.activities.iter().map(|r| r.id).collect::<Vec<_>>(),
                vec![9],
                "limit={limit}"
            );
            assert_eq!(page.next_cursor, Some(9), "limit={limit}");
        }
    }

    #[test]
    fn limit_clamps_high_to_200_and_short_page_has_no_cursor() {
        let (_dir, conn) = seeded_db();
        let page = get_activity_page(&conn, 1, None, 500).unwrap();
        // Python-pinned: all 8 of user 1's rows, newest first, no cursor.
        assert_eq!(
            page.activities.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![9, 7, 6, 5, 4, 3, 2, 1]
        );
        assert_eq!(page.next_cursor, None);

        // Default route limit (50) yields the same short page; pinned rows
        // include the malformed row 9 with metadata null and row 1 parsed.
        let default_page = get_activity_page(&conn, 1, None, 50).unwrap();
        assert_eq!(
            rows_without_ts(&default_page.activities)[..2],
            [
                json!({"id": 9, "user_id": 1, "event_type": "entry_deleted", "metadata": null}),
                json!({"id": 7, "user_id": 1, "event_type": "entry_created",
                       "metadata": {"entry_id": 5, "date": "2026-08-14"}}),
            ]
        );
        assert_eq!(
            rows_without_ts(&default_page.activities).last().unwrap(),
            &json!({"id": 1, "user_id": 1, "event_type": "login",
                    "metadata": {"method": "selfhost"}})
        );
        assert_eq!(default_page.next_cursor, None);
    }

    /// contract change (DECISIONS.md post-cutover item #10): a full
    /// *final* page now emits `next_cursor: null` — the peek at limit+1
    /// found no older row. (The Flask route returned the last row's id
    /// here, forcing a wasted `{activities: [], next_cursor: null}`
    /// follow-up request.)
    #[test]
    fn full_final_page_has_null_cursor() {
        let (_dir, conn) = seeded_db();
        // Walk at limit=4: first page [9,7,6,5]; rows 4..1 remain older, so
        // the cursor is present and unchanged from the Python walk.
        let first = get_activity_page(&conn, 1, None, 4).unwrap();
        assert_eq!(
            first.activities.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![9, 7, 6, 5]
        );
        assert_eq!(first.next_cursor, Some(5));

        // Second page [4,3,2,1] is exactly full AND final: the peek finds
        // nothing older than id 1, so the cursor is null (the change under
        // test — Flask emitted Some(1) here).
        let full_final = get_activity_page(&conn, 1, Some(5), 4).unwrap();
        assert_eq!(
            full_final
                .activities
                .iter()
                .map(|r| r.id)
                .collect::<Vec<_>>(),
            vec![4, 3, 2, 1]
        );
        assert_eq!(full_final.next_cursor, None);

        // One-more-row-exists: before=6 limit=4 -> [5,4,3,2] with row 1
        // still older, so the cursor is present.
        let penultimate = get_activity_page(&conn, 1, Some(6), 4).unwrap();
        assert_eq!(
            penultimate
                .activities
                .iter()
                .map(|r| r.id)
                .collect::<Vec<_>>(),
            vec![5, 4, 3, 2]
        );
        assert_eq!(penultimate.next_cursor, Some(2));

        // A directly-requested page past the oldest row is still empty with
        // a null cursor (shape unchanged; clients just no longer get sent
        // here by a cursor).
        let empty = get_activity_page(&conn, 1, Some(1), 4).unwrap();
        assert!(empty.activities.is_empty());
        assert_eq!(empty.next_cursor, None);
        assert_eq!(
            serde_json::to_value(&empty).unwrap(),
            json!({"activities": [], "next_cursor": null})
        );
    }

    /// The peek must not be re-clamped: at limit == MAX_ACTIVITY_PAGE_SIZE
    /// (200) it fetches 201 rows, so a full final page of exactly 200 rows
    /// yields a null cursor while 201 rows yield a full page plus a cursor.
    #[test]
    fn peek_works_at_the_200_clamp_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("boundary.db");
        let path = path.to_str().unwrap();
        bootstrap(path, &SelfHostSeed::default()).unwrap();
        let conn = common::connect(path).unwrap();

        for _ in 0..MAX_ACTIVITY_PAGE_SIZE {
            add_activity(&conn, 1, "login", None).unwrap();
        }

        // Exactly 200 rows: full final page, null cursor (limit=200 and the
        // over-clamp limit=9999 behave identically).
        for limit in [MAX_ACTIVITY_PAGE_SIZE, 9999] {
            let page = get_activity_page(&conn, 1, None, limit).unwrap();
            assert_eq!(page.activities.len(), 200, "limit={limit}");
            assert_eq!(page.activities[0].id, 200, "limit={limit}");
            assert_eq!(page.activities[199].id, 1, "limit={limit}");
            assert_eq!(page.next_cursor, None, "limit={limit}");
        }

        // 201 rows: page still 200 rows (201..=2), and the peeked row 1
        // makes the cursor non-null.
        add_activity(&conn, 1, "login", None).unwrap();
        let page = get_activity_page(&conn, 1, None, MAX_ACTIVITY_PAGE_SIZE).unwrap();
        assert_eq!(page.activities.len(), 200);
        assert_eq!(page.activities[0].id, 201);
        assert_eq!(page.activities[199].id, 2);
        assert_eq!(page.next_cursor, Some(2));

        // The raw mixin port keeps its own clamp: get_activity never
        // returns more than 200 rows.
        assert_eq!(get_activity(&conn, 1, None, 9999).unwrap().len(), 200);
    }

    #[test]
    fn before_with_large_limit_reaches_oldest_rows() {
        let (_dir, conn) = seeded_db();
        // Python-pinned: before=3, limit=200 -> [2, 1], next_cursor null.
        let page = get_activity_page(&conn, 1, Some(3), 200).unwrap();
        assert_eq!(
            rows_without_ts(&page.activities),
            vec![
                json!({"id": 2, "user_id": 1, "event_type": "entry_created",
                       "metadata": {"entry_id": 1, "date": "2026-08-10"}}),
                json!({"id": 1, "user_id": 1, "event_type": "login",
                       "metadata": {"method": "selfhost"}}),
            ]
        );
        assert_eq!(page.next_cursor, None);
    }

    #[test]
    fn malformed_metadata_is_returned_as_null_not_an_error() {
        let (_dir, conn) = seeded_db();
        let rows = get_activity(&conn, 1, None, 1).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, 9);
        assert_eq!(rows[0].event_type, "entry_deleted");
        assert_eq!(rows[0].metadata, None, "malformed blob must null out");
    }

    #[test]
    fn pages_are_scoped_per_user() {
        let (_dir, conn) = seeded_db();
        let rows = get_activity(&conn, 2, None, 50).unwrap();
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![8]);
        assert_eq!(rows[0].user_id, 2);
        // And user 1 never sees id 8 (asserted implicitly above; explicit
        // here for the record).
        let user1_ids: Vec<i64> = get_activity(&conn, 1, None, 200)
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect();
        assert!(!user1_ids.contains(&8));
    }

    #[test]
    fn add_activity_without_metadata_stores_null() {
        let (_dir, conn) = seeded_db();
        let id = add_activity(&conn, 1, "login", None).unwrap();
        assert_eq!(id, 10);
        let stored: Option<String> = conn
            .query_row(
                "SELECT metadata FROM activity_log WHERE id = ?",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored, None);
        let rows = get_activity(&conn, 1, None, 1).unwrap();
        assert_eq!(rows[0].metadata, None);
    }

    #[test]
    fn prune_deletes_only_rows_older_than_cutoff() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prune.db");
        let path = path.to_str().unwrap();
        bootstrap(path, &SelfHostSeed::default()).unwrap();
        let conn = common::connect(path).unwrap();

        conn.execute_batch(
            "INSERT INTO activity_log (user_id, event_type, metadata, created_at) \
             VALUES (1, 'login', NULL, datetime('now', '-100 days'));\
             INSERT INTO activity_log (user_id, event_type, metadata, created_at) \
             VALUES (1, 'login', NULL, datetime('now', '-89 days'));\
             INSERT INTO activity_log (user_id, event_type, metadata) \
             VALUES (1, 'login', NULL);",
        )
        .unwrap();

        // Python-pinned: prune_activity(90) deletes 1 row; ids [3, 2] remain.
        assert_eq!(prune_activity(&conn, 90).unwrap(), 1);
        let remaining: Vec<i64> = get_activity(&conn, 1, None, 50)
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(remaining, vec![3, 2]);

        // Idempotent: nothing else is old enough.
        assert_eq!(prune_activity(&conn, 90).unwrap(), 0);
    }
}
