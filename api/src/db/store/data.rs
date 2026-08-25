//! Export/import store ops (v0.6.0 `data` route family): the dual-backend
//! dispatch for `GET /api/export/data` and `POST /api/import/data`.
//!
//! Both operations are single data-layer calls — export is one pure read
//! composition, import is one transaction — so each arm is a straight
//! delegation: the `Sqlite` arm runs the blocking query under
//! `spawn_blocking`, the `Pg` arm awaits the async twin on one checked-out
//! connection. Deliberately NO activity writes and NO achievement checks on
//! import (pure restore, DECISIONS.md 2026-08-24).

use super::{db_err, with_db};
use crate::db::data::{ExportData, ImportCounts};
use crate::db::{DbHandle, data, pg};
use crate::error::ApiResult;

/// `GET /export/data` — everything the user owns, deterministic order,
/// weekly rollover projected (never persisted).
pub async fn export_data(db: &DbHandle, user_id: i64) -> ApiResult<ExportData> {
    match db {
        DbHandle::Sqlite(pool) => {
            with_db(pool, move |conn| {
                data::export_data(conn, user_id).map_err(db_err)
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let client = pg::util::client(pool).await.map_err(db_err)?;
            pg::data::export_data(&**client, user_id)
                .await
                .map_err(db_err)
        }
    }
}

/// `POST /import/data` — merge a validated v1 file inside one transaction,
/// skipping duplicates; all-or-nothing on failure.
pub async fn import_data(db: &DbHandle, user_id: i64, file: ExportData) -> ApiResult<ImportCounts> {
    match db {
        DbHandle::Sqlite(pool) => {
            with_db(pool, move |conn| {
                data::import_data(conn, user_id, &file).map_err(db_err)
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let mut client = pg::util::client(pool).await.map_err(db_err)?;
            pg::data::import_data(&mut **client, user_id, &file)
                .await
                .map_err(db_err)
        }
    }
}
