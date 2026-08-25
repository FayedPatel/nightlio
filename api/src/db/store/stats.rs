//! Extended-statistics store ops — the former inline closure bodies of the
//! `GET /statistics/extended`, `GET /statistics/heatmap`, and
//! `GET /statistics/digest` handlers in `routes/mood.rs`, moved here
//! verbatim.
//!
//! The `Pg` arms call the async twins in [`crate::db::pg::stats`] with the
//! same error mapping the SQLite arms give the routes: driver failures →
//! generic 500, and only the digest's ported Python `ValueError` text →
//! 400.

use serde_json::{Value, json};

use super::{db_err, value_err, with_db};
use crate::db::stats::{Heatmap, MonthlyDigest};
use crate::db::{DatabaseError, DbHandle, pg, stats};
use crate::error::{ApiError, ApiResult};

/// `GET /statistics/extended` — the six extended aggregations in one
/// connection checkout (`year`/`month` are the caller's current
/// server-local values).
pub async fn extended_statistics(
    db: &DbHandle,
    user_id: i64,
    year: i64,
    month: i64,
) -> ApiResult<Value> {
    match db {
        DbHandle::Sqlite(pool) => {
            with_db(pool, move |conn| {
                Ok(json!({
                    "rolling_averages": stats::rolling_averages(conn, user_id).map_err(db_err)?,
                    "weekday_averages": stats::weekday_averages(conn, user_id).map_err(db_err)?,
                    "mood_volatility":
                        stats::mood_volatility(conn, user_id, stats::DEFAULT_VOLATILITY_WINDOW_DAYS)
                            .map_err(db_err)?,
                    "tag_correlations": stats::tag_correlations(conn, user_id).map_err(db_err)?,
                    "goal_correlations": stats::goal_correlations(conn, user_id).map_err(db_err)?,
                    "monthly_digest": stats::monthly_digest(conn, user_id, year, month)
                        .map_err(db_err)?,
                }))
            })
            .await
        }
        DbHandle::Pg(pool) => {
            // One checkout for the six aggregations, like the SQLite arm's
            // single connection.
            let client = pg::util::client(pool).await.map_err(db_err)?;
            Ok(json!({
                "rolling_averages": pg::stats::rolling_averages(&**client, user_id)
                    .await
                    .map_err(db_err)?,
                "weekday_averages": pg::stats::weekday_averages(&**client, user_id)
                    .await
                    .map_err(db_err)?,
                "mood_volatility": pg::stats::mood_volatility(
                    &**client,
                    user_id,
                    stats::DEFAULT_VOLATILITY_WINDOW_DAYS
                )
                .await
                .map_err(db_err)?,
                "tag_correlations": pg::stats::tag_correlations(&**client, user_id)
                    .await
                    .map_err(db_err)?,
                "goal_correlations": pg::stats::goal_correlations(&**client, user_id)
                    .await
                    .map_err(db_err)?,
                "monthly_digest": pg::stats::monthly_digest(&**client, user_id, year, month)
                    .await
                    .map_err(db_err)?,
            }))
        }
    }
}

/// `GET /statistics/heatmap?year=` — the per-day heatmap for one year.
pub async fn heatmap(db: &DbHandle, user_id: i64, year: i64) -> ApiResult<Heatmap> {
    match db {
        DbHandle::Sqlite(pool) => {
            with_db(pool, move |conn| {
                stats::heatmap(conn, user_id, year).map_err(db_err)
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let client = pg::util::client(pool).await.map_err(db_err)?;
            pg::stats::heatmap(&**client, user_id, year)
                .await
                .map_err(db_err)
        }
    }
}

/// `GET /statistics/digest?year=&month=` — the monthly digest.
pub async fn monthly_digest(
    db: &DbHandle,
    user_id: i64,
    year: i64,
    month: i64,
) -> ApiResult<MonthlyDigest> {
    match db {
        DbHandle::Sqlite(pool) => {
            with_db(pool, move |conn| {
                // The mixin's own ValueError port maps to 400, like Flask's
                // `except ValueError` in this route.
                stats::monthly_digest(conn, user_id, year, month).map_err(value_err)
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let client = pg::util::client(pool).await.map_err(db_err)?;
            pg::stats::monthly_digest(&**client, user_id, year, month)
                .await
                .map_err(|error| match &error {
                    // The pg twin wraps driver failures in
                    // `DatabaseError::Message` too, so a blanket `value_err`
                    // would turn an outage into a 400 with leaked driver
                    // text. Only the twin's verbatim ValueError port (raised
                    // before any query runs) maps to 400; everything else
                    // stays the generic 500, matching the SQLite arm's
                    // route-visible behavior.
                    DatabaseError::Message(message)
                        if message == "month must be between 1 and 12" =>
                    {
                        ApiError::Validation(message.clone())
                    }
                    _ => db_err(error),
                })
        }
    }
}
