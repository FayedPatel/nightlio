//! PostgreSQL twin of the SQLite bootstrap's *seed* responsibilities
//! (v0.6.0 parity fix): the schema itself comes from the migration runner
//! (`0001_baseline`), but a fresh PostgreSQL database used to start with
//! zero rows — no self-host user, no default tag groups — while a fresh
//! SQLite file gets both from `db::bootstrap`. This module closes that gap
//! at startup, idempotently, so a Postgres-backed self-host deploy answers
//! `GET /api/groups` with the same three default groups a SQLite deploy
//! does (and existing deploys are backfilled on their next restart).

use crate::db::DatabaseError;
use crate::db::bootstrap::SelfHostSeed;
use crate::db::pg::{groups as pg_groups, util};

/// Ensure the self-host baseline rows exist: the default self-host user
/// (get-or-insert keyed by `google_id == external_id`, exactly like the
/// SQLite bootstrap's `ensure_default_user`) and the default tag groups
/// via [`pg_groups::ensure_default_groups_for_user`].
///
/// Runs after `migrate::run_postgres` and before the pool serves queries.
/// Concurrent replicas serialize on
/// `pg_advisory_lock(hashtext('nightlio_bootstrap'))`, held on a session
/// detached from the pool (the same pattern as the migration runner): the
/// detached socket closes on drop, so the server releases the lock even on
/// an early error return.
///
/// Idempotent, and deliberately conservative on the user row: an existing
/// row is only read — its `email`, `name`, and `last_login` are never
/// refreshed here (login-time upserts own that).
pub async fn ensure_selfhost_baseline(
    pool: &deadpool_postgres::Pool,
    seed: &SelfHostSeed,
) -> Result<(), DatabaseError> {
    let object = pool
        .get()
        .await
        .map_err(|exc| DatabaseError::Message(format!("Database connection failed: {exc}")))?;
    let mut client = deadpool_postgres::Object::take(object);

    client
        .query_one(
            "SELECT pg_advisory_lock(hashtext('nightlio_bootstrap'))",
            &[],
        )
        .await
        .map_err(util::db_error)?;

    let result = baseline_locked(&mut client, seed).await;

    // Best-effort tidy unlock; dropping the detached session releases it
    // regardless.
    let _ = client
        .query_one(
            "SELECT pg_advisory_unlock(hashtext('nightlio_bootstrap'))",
            &[],
        )
        .await;
    result
}

async fn baseline_locked(
    client: &mut tokio_postgres::Client,
    seed: &SelfHostSeed,
) -> Result<(), DatabaseError> {
    let external_id = seed.external_id.as_str();
    let existing = client
        .query_opt("SELECT id FROM users WHERE google_id = $1", &[&external_id])
        .await
        .map_err(util::db_error)?;

    let user_id: i64 = match existing {
        Some(row) => row.get(0),
        None => {
            // Same defaults as the SQLite bootstrap's ensure_default_user:
            // auth_provider 'local', email falling back to
            // `<external_id>@localhost`.
            let email = seed
                .email
                .clone()
                .unwrap_or_else(|| format!("{external_id}@localhost"));
            let row = client
                .query_one(
                    "INSERT INTO users (google_id, email, name, auth_provider, external_id) \
                     VALUES ($1, $2, $3, 'local', $1) RETURNING id",
                    &[&external_id, &email, &seed.name],
                )
                .await
                .map_err(util::db_error)?;
            tracing::info!("Seeded default self-host user");
            row.get(0)
        }
    };

    pg_groups::ensure_default_groups_for_user(client, user_id)
        .await
        .map_err(|exc| DatabaseError::Message(exc.to_string()))
}
