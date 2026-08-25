//! Achievement-domain store ops — the former inline closure bodies of
//! `routes/achievements.rs` plus the achievements-backed statistics
//! handlers of `routes/mood.rs` (`GET /statistics`, `POST /statistics/view`,
//! `GET /streak`), moved here verbatim.
//!
//! The `Pg` arms call the async twins in [`crate::db::pg::achievements`],
//! preserving the composition order and the error mapping (driver
//! failures → generic 500, best-effort activity writes ignored).

use rusqlite::Connection;
use serde_json::{Value, json};

use super::{db_err, with_db};
use crate::db::achievements::{AchievementRow, AchievementsProgress};
use crate::db::{DatabaseError, DbHandle, SqlitePool, achievements, pg};
use crate::error::{ApiError, ApiResult};

/// Check out a pooled connection and run a data-layer call under
/// `spawn_blocking`. Any failure maps to the generic 500 via [`ApiError`]
/// (the Python routes' bare `except Exception` → 500; per the contract we
/// match status and shape, never Python's leaked `str(e)` text).
async fn run_blocking<T, F>(pool: &SqlitePool, query: F) -> ApiResult<T>
where
    F: FnOnce(&Connection) -> Result<T, DatabaseError> + Send + 'static,
    T: Send + 'static,
{
    let pool = pool.clone();
    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        query(&conn)
    })
    .await
    .map_err(|join_error| ApiError::Internal(anyhow::anyhow!(join_error)))?
    .map_err(|error| match error {
        DatabaseError::Sqlite(cause) => ApiError::Database(cause),
        DatabaseError::Pool(cause) => ApiError::Pool(cause),
        DatabaseError::Message(message) => ApiError::Internal(anyhow::anyhow!(message)),
    })
}

/// `GET /achievements` — bare array of DB rows (metadata is merged by the
/// route).
pub async fn get_user_achievements(db: &DbHandle, user_id: i64) -> ApiResult<Vec<AchievementRow>> {
    match db {
        DbHandle::Sqlite(pool) => {
            run_blocking(pool, move |conn| {
                crate::db::achievements::get_user_achievements(conn, user_id)
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let client = pg::util::client(pool).await.map_err(db_err)?;
            pg::achievements::get_user_achievements(&**client, user_id)
                .await
                .map_err(db_err)
        }
    }
}

/// `POST /achievements/check` — award pass plus the best-effort
/// `achievement_unlocked` activity writes; returns the newly-awarded types.
pub async fn check_achievements(db: &DbHandle, user_id: i64) -> ApiResult<Vec<String>> {
    match db {
        DbHandle::Sqlite(pool) => {
            run_blocking(pool, move |conn| {
                let new_types = crate::db::achievements::check_achievements(conn, user_id)?;
                for achievement_type in &new_types {
                    // Best-effort activity write; must never break the award itself.
                    let _ = crate::db::activity::add_activity(
                        conn,
                        user_id,
                        "achievement_unlocked",
                        Some(&json!({ "achievement_type": achievement_type })),
                    );
                }
                Ok(new_types)
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let client = pg::util::client(pool).await.map_err(db_err)?;
            let new_types = pg::achievements::check_achievements(&**client, user_id)
                .await
                .map_err(db_err)?;
            for achievement_type in &new_types {
                // Best-effort activity write; must never break the award itself.
                let _ = pg::activity::add_activity(
                    &**client,
                    user_id,
                    "achievement_unlocked",
                    Some(&json!({ "achievement_type": achievement_type })),
                )
                .await;
            }
            Ok(new_types)
        }
    }
}

/// `GET /achievements/progress[/]` — straight passthrough.
pub async fn get_achievements_progress(
    db: &DbHandle,
    user_id: i64,
) -> ApiResult<AchievementsProgress> {
    match db {
        DbHandle::Sqlite(pool) => {
            run_blocking(pool, move |conn| {
                crate::db::achievements::get_achievements_progress(conn, user_id)
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let client = pg::util::client(pool).await.map_err(db_err)?;
            pg::achievements::get_achievements_progress(&**client, user_id)
                .await
                .map_err(db_err)
        }
    }
}

/// `GET /statistics` — statistics, mood distribution, and current streak in
/// one connection checkout.
pub async fn get_mood_statistics(db: &DbHandle, user_id: i64) -> ApiResult<Value> {
    match db {
        DbHandle::Sqlite(pool) => {
            with_db(pool, move |conn| {
                let statistics =
                    achievements::get_mood_statistics(conn, user_id).map_err(db_err)?;
                let mood_distribution =
                    achievements::get_mood_counts(conn, user_id).map_err(db_err)?;
                let current_streak = achievements::get_current_streak(conn, user_id);
                Ok(json!({
                    "statistics": statistics,
                    "mood_distribution": mood_distribution,
                    "current_streak": current_streak,
                }))
            })
            .await
        }
        DbHandle::Pg(pool) => {
            // One checkout for the three reads, like the SQLite arm's
            // single connection.
            let client = pg::util::client(pool).await.map_err(db_err)?;
            let statistics = pg::achievements::get_mood_statistics(&**client, user_id)
                .await
                .map_err(db_err)?;
            let mood_distribution = pg::achievements::get_mood_counts(&**client, user_id)
                .await
                .map_err(db_err)?;
            let current_streak = pg::achievements::get_current_streak(&**client, user_id).await;
            Ok(json!({
                "statistics": statistics,
                "mood_distribution": mood_distribution,
                "current_streak": current_streak,
            }))
        }
    }
}

/// `POST /statistics/view` — per-day idempotent `stats_views` increment.
pub async fn record_stats_view(db: &DbHandle, user_id: i64) -> ApiResult<bool> {
    match db {
        DbHandle::Sqlite(pool) => {
            with_db(pool, move |conn| {
                achievements::record_stats_view(conn, user_id).map_err(db_err)
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let client = pg::util::client(pool).await.map_err(db_err)?;
            pg::achievements::record_stats_view(&**client, user_id)
                .await
                .map_err(db_err)
        }
    }
}

/// `GET /streak` — the current streak count.
pub async fn current_streak(db: &DbHandle, user_id: i64) -> ApiResult<i64> {
    match db {
        DbHandle::Sqlite(pool) => {
            with_db(pool, move |conn| {
                Ok(achievements::get_current_streak(conn, user_id))
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let client = pg::util::client(pool).await.map_err(db_err)?;
            Ok(pg::achievements::get_current_streak(&**client, user_id).await)
        }
    }
}
