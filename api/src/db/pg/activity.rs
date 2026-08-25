//! PostgreSQL twin of `db/activity.rs` (the `ActivityLogMixin` port).
//!
//! Wire-quirk translation notes (docs/plans/v0.6.0.md):
//! - `last_insert_rowid()` becomes `RETURNING id`.
//! - Keyset pagination is unchanged (`id` order, `LIMIT $n`).
//! - The retention DELETE's `datetime('now', ?)` modifier becomes a cutoff
//!   string computed in Rust with chrono
//!   ([`util::utc_now_minus_days_text`]) and bound as text: `created_at`
//!   is `text COLLATE "C"`, so `created_at < $1` is the same lexicographic
//!   compare SQLite performed against the `datetime()` text.
//! - `metadata` is a TEXT passthrough; the defensive JSON parse (malformed
//!   blob → `null`, never an error) is shared Rust code, unchanged.

use tokio_postgres::GenericClient;

use crate::db::DatabaseError;
use crate::db::activity::{ActivityPage, ActivityRow, MAX_ACTIVITY_PAGE_SIZE};
use crate::db::pg::util;

// --- SQL (dialect-translated from the `db/activity.rs` constants) ------------

const INSERT_ACTIVITY_SQL: &str =
    "INSERT INTO activity_log (user_id, event_type, metadata) VALUES ($1, $2, $3) RETURNING id";

const SELECT_ACTIVITY_BEFORE_SQL: &str = "SELECT id, user_id, event_type, metadata, created_at \
       FROM activity_log \
      WHERE user_id = $1 AND id < $2 \
      ORDER BY id DESC \
      LIMIT $3";

const SELECT_ACTIVITY_FIRST_PAGE_SQL: &str = "SELECT id, user_id, event_type, metadata, created_at \
       FROM activity_log \
      WHERE user_id = $1 \
      ORDER BY id DESC \
      LIMIT $2";

const PRUNE_ACTIVITY_SQL: &str = "DELETE FROM activity_log WHERE created_at < $1";

/// Twin of `activity::add_activity`: record an event, return the new row
/// id. `metadata` is JSON-encoded exactly like the SQLite twin.
pub async fn add_activity(
    client: &impl GenericClient,
    user_id: i64,
    event_type: &str,
    metadata: Option<&serde_json::Value>,
) -> Result<i64, DatabaseError> {
    let payload = match metadata {
        Some(value) => Some(
            serde_json::to_string(value)
                .map_err(|exc| DatabaseError::Message(format!("Database error: {exc}")))?,
        ),
        None => None,
    };
    let row = client
        .query_one(INSERT_ACTIVITY_SQL, &[&user_id, &event_type, &payload])
        .await
        .map_err(util::db_error)?;
    Ok(row.get(0))
}

/// Shared row fetch (see the SQLite twin's `fetch_rows`): `fetch_limit` may
/// be `MAX_ACTIVITY_PAGE_SIZE + 1` for the page peek.
async fn fetch_rows(
    client: &impl GenericClient,
    user_id: i64,
    before: Option<i64>,
    limit: i64,
) -> Result<Vec<ActivityRow>, DatabaseError> {
    let raw_rows = match before {
        Some(before) => client
            .query(SELECT_ACTIVITY_BEFORE_SQL, &[&user_id, &before, &limit])
            .await
            .map_err(util::db_error)?,
        None => client
            .query(SELECT_ACTIVITY_FIRST_PAGE_SQL, &[&user_id, &limit])
            .await
            .map_err(util::db_error)?,
    };

    let mut rows = Vec::with_capacity(raw_rows.len());
    for raw in raw_rows {
        let id: i64 = raw.get(0);
        // Defensive JSON parse, identical to the SQLite twin: a malformed
        // blob is logged and nulled, never an error surfaced to the caller.
        let metadata = raw
            .get::<_, Option<String>>(3)
            .and_then(|blob| match serde_json::from_str(&blob) {
                Ok(value) => Some(value),
                Err(_) => {
                    tracing::warn!("Malformed activity metadata for row {id}");
                    None
                }
            });
        rows.push(ActivityRow {
            id,
            user_id: raw.get(1),
            event_type: raw.get(2),
            metadata,
            created_at: raw.get(4),
        });
    }
    Ok(rows)
}

/// Twin of `activity::get_activity`: a clamped page of the user's activity,
/// newest first.
pub async fn get_activity(
    client: &impl GenericClient,
    user_id: i64,
    before: Option<i64>,
    limit: i64,
) -> Result<Vec<ActivityRow>, DatabaseError> {
    let limit = limit.clamp(1, MAX_ACTIVITY_PAGE_SIZE);
    fetch_rows(client, user_id, before, limit).await
}

/// Twin of `activity::get_activity_page`: the clamped page plus the
/// peek-one-past-the-limit cursor computation (`next_cursor` is null
/// whenever no older rows exist, per DECISIONS.md post-cutover item #10).
pub async fn get_activity_page(
    client: &impl GenericClient,
    user_id: i64,
    before: Option<i64>,
    limit: i64,
) -> Result<ActivityPage, DatabaseError> {
    let clamped_limit = limit.clamp(1, MAX_ACTIVITY_PAGE_SIZE);
    // Peek: deliberately not re-clamped, so it works at limit == 200.
    let mut activities = fetch_rows(client, user_id, before, clamped_limit + 1).await?;
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

/// Twin of `activity::prune_activity`: delete rows older than `days` days.
/// The cutoff is computed in Rust (`datetime('now', '-N days')` semantics)
/// and compared lexicographically against the `COLLATE "C"` text column.
pub async fn prune_activity(client: &impl GenericClient, days: i64) -> Result<u64, DatabaseError> {
    let cutoff = util::utc_now_minus_days_text(days);
    let deleted = client
        .execute(PRUNE_ACTIVITY_SQL, &[&cutoff])
        .await
        .map_err(util::db_error)?;
    if deleted > 0 {
        tracing::info!("Pruned {deleted} activity rows older than {days} days");
    }
    Ok(deleted)
}

#[cfg(test)]
mod tests {
    //! Live-PG quirk tests (skipped unless `NIGHTLIO_PG_TEST_URL` is set).
    //! Seeds and expectations mirror the SQLite twin's Python-pinned tests.

    use serde_json::{Value, json};

    use super::*;
    use crate::db::pg::util::test_support::{connect_scratch, seed_default_user};

    /// Serialize a row and drop `created_at` (nondeterministic, `<TS>` in
    /// the contract fixtures too).
    fn rows_without_ts(rows: &[ActivityRow]) -> Vec<Value> {
        rows.iter()
            .map(|row| {
                assert!(
                    row.created_at.is_some(),
                    "nightlio_now() default must populate created_at (row {})",
                    row.id
                );
                let mut value = serde_json::to_value(row).unwrap();
                value.as_object_mut().unwrap().remove("created_at");
                value
            })
            .collect()
    }

    #[tokio::test]
    async fn keyset_pages_cursors_and_malformed_metadata_match_sqlite() {
        let Some(client) = connect_scratch("ws4a_activity_pages").await else {
            return;
        };
        let uid = seed_default_user(&client).await;
        let user2: i64 = client
            .query_one(
                "INSERT INTO users (google_id, email, name) \
                 VALUES ('other_user', 'other@example.com', 'Other') RETURNING id",
                &[],
            )
            .await
            .unwrap()
            .get(0);

        // Same 8-row seed as the SQLite twin's `seeded_db`: login, entry 1,
        // achievement unlock, entries 2-5, then user 2's login.
        let seed: [(i64, &str, Value); 8] = [
            (uid, "login", json!({"method": "selfhost"})),
            (
                uid,
                "entry_created",
                json!({"entry_id": 1, "date": "2026-08-10"}),
            ),
            (
                uid,
                "achievement_unlocked",
                json!({"achievement_type": "first_entry"}),
            ),
            (
                uid,
                "entry_created",
                json!({"entry_id": 2, "date": "2026-08-11"}),
            ),
            (
                uid,
                "entry_created",
                json!({"entry_id": 3, "date": "2026-08-12"}),
            ),
            (
                uid,
                "entry_created",
                json!({"entry_id": 4, "date": "2026-08-13"}),
            ),
            (
                uid,
                "entry_created",
                json!({"entry_id": 5, "date": "2026-08-14"}),
            ),
            (user2, "login", json!({"method": "selfhost"})),
        ];
        let mut ids = Vec::new();
        for (user, event, metadata) in &seed {
            ids.push(
                add_activity(&client, *user, event, Some(metadata))
                    .await
                    .unwrap(),
            );
        }
        assert_eq!(ids, vec![1, 2, 3, 4, 5, 6, 7, 8], "RETURNING id sequence");

        // Malformed metadata inserted raw.
        client
            .execute(
                "INSERT INTO activity_log (user_id, event_type, metadata) VALUES ($1, $2, $3)",
                &[&uid, &"entry_deleted", &"{not-json"],
            )
            .await
            .unwrap();

        // First page, limit 3: rows [9, 7, 6], cursor 6 (row 8 is user 2's).
        let page = get_activity_page(&client, uid, None, 3).await.unwrap();
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

        // Keyset page before=6: strictly older rows [5, 4, 3], cursor 3.
        let page = get_activity_page(&client, uid, Some(6), 3).await.unwrap();
        assert_eq!(
            page.activities.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![5, 4, 3]
        );
        assert_eq!(page.next_cursor, Some(3));

        // Clamp low; short page has no cursor at a big limit.
        let page = get_activity_page(&client, uid, None, 0).await.unwrap();
        assert_eq!(
            page.activities.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![9]
        );
        assert_eq!(page.next_cursor, Some(9));
        let page = get_activity_page(&client, uid, None, 500).await.unwrap();
        assert_eq!(
            page.activities.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![9, 7, 6, 5, 4, 3, 2, 1]
        );
        assert_eq!(page.next_cursor, None);

        // Full FINAL page: null cursor (DECISIONS.md item #10).
        let full_final = get_activity_page(&client, uid, Some(5), 4).await.unwrap();
        assert_eq!(
            full_final
                .activities
                .iter()
                .map(|r| r.id)
                .collect::<Vec<_>>(),
            vec![4, 3, 2, 1]
        );
        assert_eq!(full_final.next_cursor, None);

        // Scoping: user 2 sees only row 8.
        let rows = get_activity(&client, user2, None, 50).await.unwrap();
        assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![8]);
    }

    #[tokio::test]
    async fn prune_uses_a_rust_cutoff_against_the_text_column() {
        let Some(client) = connect_scratch("ws4a_activity_prune").await else {
            return;
        };
        let uid = seed_default_user(&client).await;

        // -100 days / -89 days / now, as text — the same shapes SQLite's
        // datetime() would have stored.
        let old = util::utc_now_minus_days_text(100);
        let recent = util::utc_now_minus_days_text(89);
        for created_at in [&old, &recent] {
            client
                .execute(
                    "INSERT INTO activity_log (user_id, event_type, metadata, created_at) \
                     VALUES ($1, 'login', NULL, $2)",
                    &[&uid, created_at],
                )
                .await
                .unwrap();
        }
        add_activity(&client, uid, "login", None).await.unwrap();

        // prune(90): exactly the 100-day-old row goes; ids [3, 2] remain.
        assert_eq!(prune_activity(&client, 90).await.unwrap(), 1);
        let remaining: Vec<i64> = get_activity(&client, uid, None, 50)
            .await
            .unwrap()
            .iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(remaining, vec![3, 2]);
        assert_eq!(prune_activity(&client, 90).await.unwrap(), 0);

        // NULL metadata stays null on the wire.
        let rows = get_activity(&client, uid, None, 1).await.unwrap();
        assert_eq!(rows[0].metadata, None);
    }
}
