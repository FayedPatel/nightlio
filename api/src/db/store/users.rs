//! User-domain store ops: session verification, local login/register, the
//! self-host upsert, OIDC provisioning, and theme preferences. The closure
//! bodies are the former inline `spawn_blocking` bodies of
//! `routes/auth.rs`, `routes/extras.rs` (preferences), and
//! `auth/oidc.rs`, moved here verbatim. The Pg arms dispatch to the async
//! twins in `db/pg/users.rs`, composing multi-step ops on one checked-out
//! connection exactly like the SQLite closures share one `&Connection`;
//! CPU-bound password hashing stays on the blocking pool on both backends.

use serde_json::json;
use tokio::task::spawn_blocking;

use super::run_db;
use crate::auth::password;
use crate::db::pg::{
    activity as pg_activity, groups as pg_groups, users as pg_users, util as pg_util,
};
use crate::db::users::{self, UserRow};
use crate::db::{DatabaseError, DbHandle, activity, groups};

/// `POST /auth/verify` — the caller's user row, `None` when it vanished.
pub async fn get_user_by_id(db: &DbHandle, user_id: i64) -> Result<Option<UserRow>, DatabaseError> {
    match db {
        DbHandle::Sqlite(pool) => {
            run_db(pool, move |conn| users::get_user_by_id(conn, user_id)).await
        }
        DbHandle::Pg(pool) => {
            let client = pg_util::client(pool).await?;
            pg_users::get_user_by_id(&**client, user_id).await
        }
    }
}

/// The credentialed-login lookup result: the local user row plus its stored
/// password hash; `None` when the username is unknown.
type Lookup = Result<Option<(UserRow, Option<String>)>, DatabaseError>;

/// `POST /auth/local/login` (credentialed branch) — look up the local user
/// and its stored hash in one connection checkout.
pub async fn lookup_local_user(db: &DbHandle, username: String) -> Lookup {
    match db {
        DbHandle::Sqlite(pool) => {
            run_db(pool, move |conn| {
                let Some(user) = users::get_user_by_provider(conn, "local", &username)? else {
                    return Ok(None);
                };
                let stored_hash = users::get_user_password_hash(conn, user.id)?;
                Ok(Some((user, stored_hash)))
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let client = pg_util::client(pool).await?;
            let Some(user) = pg_users::get_user_by_provider(&**client, "local", &username).await?
            else {
                return Ok(None);
            };
            let stored_hash = pg_users::get_user_password_hash(&**client, user.id).await?;
            Ok(Some((user, stored_hash)))
        }
    }
}

/// scrypt/pbkdf2/argon2 verification is CPU-bound: blocking pool, never
/// the async executor. The legacy-hash upgrade (rehash-on-login) runs in
/// the same blocking context — argon2id hashing is just as CPU-bound,
/// and it must only ever happen right after the plaintext verified.
///
/// The nested result keeps the caller's match arms intact: the outer
/// error is a task/backend failure, the inner one an unparseable stored
/// hash (both map to the route's catch-all 500).
pub async fn verify_password_and_maybe_rehash(
    db: &DbHandle,
    stored_hash: String,
    password: String,
    user_id: i64,
) -> Result<Result<bool, password::PasswordHashError>, DatabaseError> {
    match db {
        DbHandle::Sqlite(pool) => {
            let pool = pool.clone();
            spawn_blocking(move || {
                let verdict = password::check_password_hash(&stored_hash, &password)?;
                if verdict && password::needs_rehash(&stored_hash) {
                    // Transparent upgrade of legacy Werkzeug rows to argon2id (the
                    // Flask rollback constraint is retired — see auth::password).
                    // Best-effort: a rehash/persist failure logs a warning and
                    // never fails the login; the legacy hash simply stays put
                    // until the next successful login.
                    let new_hash = password::generate_password_hash(&password);
                    let persisted = pool
                        .get()
                        .map_err(DatabaseError::from)
                        .and_then(|conn| users::set_user_password(&conn, user_id, &new_hash));
                    if let Err(exc) = persisted {
                        tracing::warn!(error = %exc, user_id, "Password rehash failed; keeping legacy hash");
                    }
                }
                Ok::<bool, password::PasswordHashError>(verdict)
            })
            .await
            .map_err(|exc| DatabaseError::Message(format!("blocking task failed: {exc}")))
        }
        DbHandle::Pg(pool) => {
            // CPU-bound verification (and the rehash, when needed) on the
            // blocking pool; only the best-effort persist runs async here.
            let verified = spawn_blocking(move || {
                let verdict = password::check_password_hash(&stored_hash, &password)?;
                let new_hash = (verdict && password::needs_rehash(&stored_hash))
                    .then(|| password::generate_password_hash(&password));
                Ok::<(bool, Option<String>), password::PasswordHashError>((verdict, new_hash))
            })
            .await
            .map_err(|exc| DatabaseError::Message(format!("blocking task failed: {exc}")))?;
            match verified {
                Ok((verdict, new_hash)) => {
                    if let Some(new_hash) = new_hash {
                        let persisted = async {
                            let client = pg_util::client(pool).await?;
                            pg_users::set_user_password(&**client, user_id, &new_hash).await
                        }
                        .await;
                        if let Err(exc) = persisted {
                            tracing::warn!(error = %exc, user_id, "Password rehash failed; keeping legacy hash");
                        }
                    }
                    Ok(Ok(verdict))
                }
                Err(exc) => Ok(Err(exc)),
            }
        }
    }
}

/// Post-verification bookkeeping for a credentialed login.
pub async fn update_last_login(db: &DbHandle, user_id: i64) -> Result<(), DatabaseError> {
    match db {
        DbHandle::Sqlite(pool) => {
            run_db(pool, move |conn| {
                users::update_user_last_login(conn, user_id)?;
                // record_login_activity is best-effort: never breaks the login.
                let _ = activity::add_activity(
                    conn,
                    user_id,
                    "login",
                    Some(&json!({"method": "local"})),
                );
                Ok(())
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let client = pg_util::client(pool).await?;
            pg_users::update_user_last_login(&**client, user_id).await?;
            // record_login_activity is best-effort: never breaks the login.
            let _ = pg_activity::add_activity(
                &**client,
                user_id,
                "login",
                Some(&json!({"method": "local"})),
            )
            .await;
            Ok(())
        }
    }
}

/// The credential-free self-host branch's upsert.
pub async fn selfhost_login_upsert(
    db: &DbHandle,
    default_user_id: String,
    email: String,
    name: String,
) -> Result<Option<UserRow>, DatabaseError> {
    match db {
        DbHandle::Sqlite(pool) => {
            run_db(pool, move |conn| {
                // ensure_local_user: upsert keyed by the legacy google_id column;
                // idempotent, refreshes the friendly name/email, burns an
                // autoincrement id on every conflicting insert attempt (known,
                // fixture-documented behavior).
                let user = users::upsert_user_by_google_id(
                    conn,
                    &default_user_id,
                    Some(&email),
                    Some(&name),
                    None,
                )?;
                if let Some(user) = &user {
                    let _ = activity::add_activity(
                        conn,
                        user.id,
                        "login",
                        Some(&json!({"method": "selfhost"})),
                    );
                }
                Ok(user)
            })
            .await
        }
        DbHandle::Pg(pool) => {
            // Same upsert semantics: the identity sequence burns a value on
            // every conflicting insert attempt, like AUTOINCREMENT did.
            let client = pg_util::client(pool).await?;
            let user = pg_users::upsert_user_by_google_id(
                &**client,
                &default_user_id,
                Some(&email),
                Some(&name),
                None,
            )
            .await?;
            if let Some(user) = &user {
                let _ = pg_activity::add_activity(
                    &**client,
                    user.id,
                    "login",
                    Some(&json!({"method": "selfhost"})),
                )
                .await;
            }
            Ok(user)
        }
    }
}

/// `sqlite3.IntegrityError` equivalent: any constraint violation (here the
/// UNIQUE index on `(auth_provider, external_id)`).
fn is_integrity_error(error: &DatabaseError) -> bool {
    matches!(
        error,
        DatabaseError::Sqlite(rusqlite::Error::SqliteFailure(inner, _))
            if inner.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

/// `POST /auth/local/register` — `Ok(user)` → 201; `Err(())` → 409
/// (username taken).
pub async fn register_local_user(
    db: &DbHandle,
    username: String,
    password: String,
    email: Option<String>,
    name: Option<String>,
) -> Result<Result<UserRow, ()>, DatabaseError> {
    match db {
        DbHandle::Sqlite(pool) => {
            let pool = pool.clone();
            spawn_blocking(move || -> Result<Result<UserRow, ()>, DatabaseError> {
                // argon2id hash (new-hash format since the Flask rollback
                // constraint was retired): CPU-bound, so it stays on the blocking
                // pool with the DB work.
                let password_hash = password::generate_password_hash(&password);
                let conn = pool.get()?;
                let user_id = match users::create_local_user(
                    &conn,
                    &username,
                    &password_hash,
                    email.as_deref(),
                    name.as_deref(),
                ) {
                    Ok(id) => id,
                    Err(exc) if is_integrity_error(&exc) => return Ok(Err(())),
                    Err(exc) => return Err(exc),
                };
                groups::ensure_default_groups_for_user(&conn, user_id)
                    .map_err(|exc| DatabaseError::Message(exc.to_string()))?;
                // Bug-compatible: register_local_user returning None (for any
                // reason) maps to the 409 branch in the Python route.
                match users::get_user_by_id(&conn, user_id)? {
                    Some(user) => Ok(Ok(user)),
                    None => Ok(Err(())),
                }
            })
            .await
            .map_err(|exc| DatabaseError::Message(format!("blocking task failed: {exc}")))?
        }
        DbHandle::Pg(pool) => {
            // CPU-bound argon2id hashing on the blocking pool; DB work async.
            let password_hash = spawn_blocking(move || password::generate_password_hash(&password))
                .await
                .map_err(|exc| DatabaseError::Message(format!("blocking task failed: {exc}")))?;
            let mut client = pg_util::client(pool).await?;
            // The twin maps any integrity violation (the UNIQUE
            // `(auth_provider, external_id)` index) to `None`, the same set
            // `is_integrity_error` matches on SQLite → the route's 409.
            let Some(user_id) = pg_users::create_local_user(
                &**client,
                &username,
                &password_hash,
                email.as_deref(),
                name.as_deref(),
            )
            .await?
            else {
                return Ok(Err(()));
            };
            pg_groups::ensure_default_groups_for_user(&mut **client, user_id)
                .await
                .map_err(|exc| DatabaseError::Message(exc.to_string()))?;
            // Bug-compatible: a vanished row maps to the 409 branch, like
            // the Python's None return.
            match pg_users::get_user_by_id(&**client, user_id).await? {
                Some(user) => Ok(Ok(user)),
                None => Ok(Err(())),
            }
        }
    }
}

/// OIDC provisioning (`handle_oidc_login` → `upsert_oidc_user`).
pub async fn upsert_oidc_user(
    db: &DbHandle,
    subject: String,
    email: Option<String>,
    name: Option<String>,
    avatar: Option<String>,
) -> Result<Option<UserRow>, DatabaseError> {
    match db {
        DbHandle::Sqlite(pool) => {
            run_db(pool, move |conn| {
                users::upsert_oidc_user(
                    conn,
                    &subject,
                    email.as_deref(),
                    name.as_deref(),
                    avatar.as_deref(),
                    "oidc",
                )
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let mut client = pg_util::client(pool).await?;
            pg_users::upsert_oidc_user(
                &mut **client,
                &subject,
                email.as_deref(),
                name.as_deref(),
                avatar.as_deref(),
                "oidc",
            )
            .await
        }
    }
}

/// `GET /api/preferences` — the stored theme string, `None` for fresh users.
pub async fn get_user_theme(db: &DbHandle, user_id: i64) -> Result<Option<String>, DatabaseError> {
    match db {
        DbHandle::Sqlite(pool) => {
            run_db(pool, move |conn| users::get_user_theme(conn, user_id)).await
        }
        DbHandle::Pg(pool) => {
            let client = pg_util::client(pool).await?;
            pg_users::get_user_theme(&**client, user_id).await
        }
    }
}

/// `PUT /api/preferences` — persist the validated theme.
pub async fn set_user_theme(
    db: &DbHandle,
    user_id: i64,
    theme: String,
) -> Result<(), DatabaseError> {
    match db {
        DbHandle::Sqlite(pool) => {
            run_db(pool, move |conn| {
                users::set_user_theme(conn, user_id, &theme)
            })
            .await
        }
        DbHandle::Pg(pool) => {
            let client = pg_util::client(pool).await?;
            pg_users::set_user_theme(&**client, user_id, &theme).await
        }
    }
}
