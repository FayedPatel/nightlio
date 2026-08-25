//! Groups store ops — the former inline closure bodies of
//! `routes/groups.rs`, moved here verbatim. The route keeps its own error
//! remapping (the `Group not found for user` → 404 contract change) at the
//! call site, untouched. The Pg arms dispatch to the async twins in
//! `db/pg/groups.rs`, which return the same `GroupsError` values (verbatim
//! ValueError messages included), so the route-visible mapping is
//! backend-independent.

use rusqlite::Connection;

use crate::db::groups::{self as db_groups, Group, GroupsError};
use crate::db::pg::{groups as pg_groups, util as pg_util};
use crate::db::{DbHandle, SqlitePool};
use crate::error::{ApiError, ApiResult};

/// Run a groups data-layer call on a pooled connection under
/// `spawn_blocking` (rusqlite is synchronous).
async fn with_conn<T, F>(pool: &SqlitePool, op: F) -> Result<T, ApiError>
where
    T: Send + 'static,
    F: FnOnce(&Connection) -> Result<T, GroupsError> + Send + 'static,
{
    let pool = pool.clone();
    tokio::task::spawn_blocking(move || {
        let conn = pool.get()?;
        op(&conn).map_err(ApiError::from)
    })
    .await
    .map_err(|exc| ApiError::Internal(anyhow::anyhow!(exc)))?
}

/// Check a Pg connection out, mapping pool failure like the Sqlite arm's
/// `pool.get()?` (an internal 500).
async fn pg_client(pool: &deadpool_postgres::Pool) -> Result<deadpool_postgres::Client, ApiError> {
    pg_util::client(pool)
        .await
        .map_err(|exc| ApiError::Internal(anyhow::Error::new(exc)))
}

/// `GET /groups` — `group_service.get_all_groups(user_id)`.
pub async fn get_all_groups(
    db: &DbHandle,
    user_id: i64,
    default_id: String,
) -> ApiResult<Vec<Group>> {
    match db {
        DbHandle::Sqlite(pool) => {
            with_conn(pool, move |conn| {
                db_groups::get_all_groups(conn, Some(user_id), &default_id)
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let client = pg_client(pool).await?;
            pg_groups::get_all_groups(&**client, Some(user_id), &default_id)
                .await
                .map_err(ApiError::from)
        }
    }
}

/// `POST /groups` — insert with the stripped name, returns the new id.
pub async fn create_group(
    db: &DbHandle,
    trimmed: String,
    user_id: i64,
    default_id: String,
) -> ApiResult<i64> {
    match db {
        DbHandle::Sqlite(pool) => {
            with_conn(pool, move |conn| {
                db_groups::create_group(conn, &trimmed, Some(user_id), &default_id)
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let client = pg_client(pool).await?;
            pg_groups::create_group(&**client, &trimmed, Some(user_id), &default_id)
                .await
                .map_err(ApiError::from)
        }
    }
}

/// `POST /groups/{id}/options` — insert with the stripped name, returns the
/// new option id (the data layer's `Group not found for user` error passes
/// through for the route to remap).
pub async fn create_group_option(
    db: &DbHandle,
    group_id: i64,
    trimmed: String,
    user_id: i64,
    default_id: String,
) -> ApiResult<i64> {
    match db {
        DbHandle::Sqlite(pool) => {
            with_conn(pool, move |conn| {
                db_groups::create_group_option(conn, group_id, &trimmed, Some(user_id), &default_id)
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let client = pg_client(pool).await?;
            pg_groups::create_group_option(
                &**client,
                group_id,
                &trimmed,
                Some(user_id),
                &default_id,
            )
            .await
            .map_err(ApiError::from)
        }
    }
}

/// `DELETE /groups/{id}` — cascade delete; `false` for unknown/foreign ids.
pub async fn delete_group(
    db: &DbHandle,
    group_id: i64,
    user_id: i64,
    default_id: String,
) -> ApiResult<bool> {
    match db {
        DbHandle::Sqlite(pool) => {
            with_conn(pool, move |conn| {
                db_groups::delete_group(conn, group_id, Some(user_id), &default_id)
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let client = pg_client(pool).await?;
            pg_groups::delete_group(&**client, group_id, Some(user_id), &default_id)
                .await
                .map_err(ApiError::from)
        }
    }
}

/// `DELETE /options/{id}` — `false` for unknown/foreign ids.
pub async fn delete_group_option(
    db: &DbHandle,
    option_id: i64,
    user_id: i64,
    default_id: String,
) -> ApiResult<bool> {
    match db {
        DbHandle::Sqlite(pool) => {
            with_conn(pool, move |conn| {
                db_groups::delete_group_option(conn, option_id, Some(user_id), &default_id)
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let client = pg_client(pool).await?;
            pg_groups::delete_group_option(&**client, option_id, Some(user_id), &default_id)
                .await
                .map_err(ApiError::from)
        }
    }
}
