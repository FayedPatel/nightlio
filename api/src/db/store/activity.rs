//! Activity store ops — the former inline closure body of the
//! `GET /api/activity` handler in `routes/extras.rs`, moved here verbatim.
//! (The best-effort activity writes bundled with other mutations live with
//! their owning ops in the sibling modules.)

use super::run_db;
use crate::db::activity::{self, ActivityPage};
use crate::db::pg::{activity as pg_activity, util as pg_util};
use crate::db::{DatabaseError, DbHandle};

/// `GET /api/activity` — keyset pagination on the row id.
pub async fn get_activity_page(
    db: &DbHandle,
    user_id: i64,
    before: Option<i64>,
    limit: i64,
) -> Result<ActivityPage, DatabaseError> {
    match db {
        DbHandle::Sqlite(pool) => {
            run_db(pool, move |conn| {
                activity::get_activity_page(conn, user_id, before, limit)
            })
            .await
        }
        DbHandle::Pg(pool) => {
            // Double deref: deadpool Object -> ClientWrapper -> the
            // tokio_postgres::Client the twins take.
            let client = pg_util::client(pool).await?;
            pg_activity::get_activity_page(&**client, user_id, before, limit).await
        }
    }
}
