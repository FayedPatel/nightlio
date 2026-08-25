//! Mood-entry store ops — the former inline closure bodies of the entry
//! CRUD handlers in `routes/mood.rs`, moved here verbatim (including the
//! best-effort activity writes and the achievement check bundled with
//! entry creation).
//!
//! The `Pg` arms call the async twins in [`crate::db::pg::moods`] /
//! [`crate::db::pg::achievements`], keeping the same composition order
//! and the same error mapping (driver failures → generic 500 via
//! [`db_err`], best-effort activity writes ignored).

use serde_json::json;

use super::{db_err, with_db};
use crate::db::moods::{EntrySelectionRow, MoodEntryRow, MoodEntryUpdate, MoodEntryWithSelections};
use crate::db::{DbHandle, achievements, activity, moods, pg};
use crate::error::{ApiError, ApiResult};

/// `POST /mood` — create an entry, log activity, check achievements.
/// Returns the new entry id plus the newly-awarded achievement types.
pub async fn create_entry(
    db: &DbHandle,
    user_id: i64,
    date_value: String,
    mood_value: i64,
    content_value: String,
    time_value: Option<String>,
    selected_options: Vec<i64>,
) -> ApiResult<(i64, Vec<String>)> {
    match db {
        DbHandle::Sqlite(pool) => {
            with_db(pool, move |conn| {
                let entry_id = moods::add_mood_entry(
                    conn,
                    user_id,
                    &date_value,
                    mood_value,
                    &content_value,
                    time_value.as_deref(),
                    Some(&selected_options),
                )
                .map_err(db_err)?;
                // Best-effort activity writes must never break the mutation.
                let _ = activity::add_activity(
                    conn,
                    user_id,
                    "entry_created",
                    Some(&json!({ "entry_id": entry_id, "date": date_value })),
                );
                let new_achievements =
                    achievements::check_achievements(conn, user_id).map_err(db_err)?;
                for achievement_type in &new_achievements {
                    let _ = activity::add_activity(
                        conn,
                        user_id,
                        "achievement_unlocked",
                        Some(&json!({ "achievement_type": achievement_type })),
                    );
                }
                Ok((entry_id, new_achievements))
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let mut client = pg::util::client(pool).await.map_err(db_err)?;
            let entry_id = pg::moods::add_mood_entry(
                &mut **client,
                user_id,
                &date_value,
                mood_value,
                &content_value,
                time_value.as_deref(),
                Some(&selected_options),
            )
            .await
            .map_err(db_err)?;
            // Best-effort activity writes must never break the mutation.
            let _ = pg::activity::add_activity(
                &**client,
                user_id,
                "entry_created",
                Some(&json!({ "entry_id": entry_id, "date": date_value })),
            )
            .await;
            let new_achievements = pg::achievements::check_achievements(&**client, user_id)
                .await
                .map_err(db_err)?;
            for achievement_type in &new_achievements {
                let _ = pg::activity::add_activity(
                    &**client,
                    user_id,
                    "achievement_unlocked",
                    Some(&json!({ "achievement_type": achievement_type })),
                )
                .await;
            }
            Ok((entry_id, new_achievements))
        }
    }
}

/// `GET /moods` — full list, or the raw-string date-range filter.
pub async fn get_entries(
    db: &DbHandle,
    user_id: i64,
    start_date: Option<String>,
    end_date: Option<String>,
) -> ApiResult<Vec<MoodEntryRow>> {
    match db {
        DbHandle::Sqlite(pool) => {
            with_db(pool, move |conn| {
                // Python: `if start_date and end_date` — truthiness, so empty
                // strings fall through to the full list.
                match (start_date.as_deref(), end_date.as_deref()) {
                    (Some(start), Some(end)) if !start.is_empty() && !end.is_empty() => {
                        moods::get_mood_entries_by_date_range(conn, user_id, start, end)
                            .map_err(db_err)
                    }
                    _ => moods::get_all_mood_entries(conn, user_id).map_err(db_err),
                }
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let client = pg::util::client(pool).await.map_err(db_err)?;
            match (start_date.as_deref(), end_date.as_deref()) {
                // Python: `if start_date and end_date` — truthiness, so empty
                // strings fall through to the full list.
                (Some(start), Some(end)) if !start.is_empty() && !end.is_empty() => {
                    pg::moods::get_mood_entries_by_date_range(&**client, user_id, start, end)
                        .await
                        .map_err(db_err)
                }
                _ => pg::moods::get_all_mood_entries(&**client, user_id)
                    .await
                    .map_err(db_err),
            }
        }
    }
}

/// `GET /mood/{id}` — the user-scoped entry row, `None` when missing.
pub async fn get_entry(
    db: &DbHandle,
    user_id: i64,
    entry_id: i64,
) -> ApiResult<Option<MoodEntryRow>> {
    match db {
        DbHandle::Sqlite(pool) => {
            with_db(pool, move |conn| {
                moods::get_mood_entry_by_id(conn, user_id, entry_id).map_err(db_err)
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let client = pg::util::client(pool).await.map_err(db_err)?;
            pg::moods::get_mood_entry_by_id(&**client, user_id, entry_id)
                .await
                .map_err(db_err)
        }
    }
}

/// `PUT /mood/{id}` — apply the partial update, then re-read the entry WITH
/// its selections; `None` when the entry is missing or nothing changed.
pub async fn update_entry(
    db: &DbHandle,
    user_id: i64,
    entry_id: i64,
    update: MoodEntryUpdate,
) -> ApiResult<Option<MoodEntryWithSelections>> {
    match db {
        DbHandle::Sqlite(pool) => {
            with_db(pool, move |conn| {
                if !moods::update_mood_entry(conn, user_id, entry_id, &update).map_err(db_err)? {
                    return Ok(None);
                }
                let Some(entry) = moods::get_mood_entry_with_selections(conn, user_id, entry_id)
                    .map_err(db_err)?
                else {
                    return Ok(None);
                };
                let _ = activity::add_activity(
                    conn,
                    user_id,
                    "entry_edited",
                    Some(&json!({ "entry_id": entry_id })),
                );
                Ok(Some(entry))
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let mut client = pg::util::client(pool).await.map_err(db_err)?;
            if !pg::moods::update_mood_entry(&mut **client, user_id, entry_id, &update)
                .await
                .map_err(db_err)?
            {
                return Ok(None);
            }
            let Some(entry) =
                pg::moods::get_mood_entry_with_selections(&**client, user_id, entry_id)
                    .await
                    .map_err(db_err)?
            else {
                return Ok(None);
            };
            let _ = pg::activity::add_activity(
                &**client,
                user_id,
                "entry_edited",
                Some(&json!({ "entry_id": entry_id })),
            )
            .await;
            Ok(Some(entry))
        }
    }
}

/// `DELETE /mood/{id}` — delete plus the best-effort activity write.
pub async fn delete_entry(db: &DbHandle, user_id: i64, entry_id: i64) -> ApiResult<bool> {
    match db {
        DbHandle::Sqlite(pool) => {
            with_db(pool, move |conn| {
                let deleted = moods::delete_mood_entry(conn, user_id, entry_id).map_err(db_err)?;
                if deleted {
                    let _ = activity::add_activity(
                        conn,
                        user_id,
                        "entry_deleted",
                        Some(&json!({ "entry_id": entry_id })),
                    );
                }
                Ok(deleted)
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let client = pg::util::client(pool).await.map_err(db_err)?;
            let deleted = pg::moods::delete_mood_entry(&**client, user_id, entry_id)
                .await
                .map_err(db_err)?;
            if deleted {
                let _ = pg::activity::add_activity(
                    &**client,
                    user_id,
                    "entry_deleted",
                    Some(&json!({ "entry_id": entry_id })),
                )
                .await;
            }
            Ok(deleted)
        }
    }
}

/// `GET /mood/{id}/selections` — user-scoped existence check (404 when the
/// entry is missing/foreign), then the selections read.
pub async fn get_entry_selections(
    db: &DbHandle,
    user_id: i64,
    entry_id: i64,
) -> ApiResult<Vec<EntrySelectionRow>> {
    match db {
        DbHandle::Sqlite(pool) => {
            with_db(pool, move |conn| {
                if moods::get_mood_entry_by_id(conn, user_id, entry_id)
                    .map_err(db_err)?
                    .is_none()
                {
                    return Err(ApiError::NotFound("Entry not found".to_string()));
                }
                moods::get_entry_selections(conn, entry_id, Some(user_id)).map_err(db_err)
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let client = pg::util::client(pool).await.map_err(db_err)?;
            if pg::moods::get_mood_entry_by_id(&**client, user_id, entry_id)
                .await
                .map_err(db_err)?
                .is_none()
            {
                return Err(ApiError::NotFound("Entry not found".to_string()));
            }
            pg::moods::get_entry_selections(&**client, entry_id, Some(user_id))
                .await
                .map_err(db_err)
        }
    }
}
