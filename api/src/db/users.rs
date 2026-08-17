//! Port of `api/database_users.py` (`UsersMixin`). Owned by the users
//! data-mixin agent — no other agent edits this file.
//!
//! Every function takes a `&Connection` (callers run them under
//! `tokio::task::spawn_blocking`) and mirrors the corresponding Python
//! method 1:1, including the bug-compatible quirks:
//!
//! - `get_user_theme` / `get_user_password_hash` conflate "no such user"
//!   and "column is NULL" into a single `None`, exactly like the Python's
//!   `row[0] if row else None`.
//! - `upsert_user_by_google_id` raises `NOT NULL constraint failed:
//!   users.email` when `email` is `None` — SQLite checks NOT NULL before
//!   ON CONFLICT resolution, so the `COALESCE(excluded.email, users.email)`
//!   in the DO UPDATE clause never actually sees a NULL email (verified
//!   against the live Python on 2026-08-15).
//! - Create-path defaults use Python truthiness: an empty string counts as
//!   missing (`email or f"{username}@localhost"`), while the UPDATE paths'
//!   SQL `COALESCE` only treats NULL as missing.

use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use super::common::{DatabaseError, sql_queries};

// --- Users-specific SQL (verbatim from `database_users.py`; the shared
// --- constants live in `common::sql_queries`) --------------------------------

const GET_USER_THEME_SQL: &str = "SELECT theme_preference FROM users WHERE id = ?";

const SET_USER_THEME_SQL: &str = "UPDATE users SET theme_preference = ? WHERE id = ?";

/// Triple-quoted literal from `update_user_last_login`, whitespace intact.
const UPDATE_USER_LAST_LOGIN_SQL: &str = "
                UPDATE users
                   SET last_login = CURRENT_TIMESTAMP
                 WHERE id = ?
                ";

/// The `" RETURNING ..."` suffix `upsert_user_by_google_id` appends to
/// `SQLQueries.UPSERT_USER` (Python adjacent-literal concatenation).
const UPSERT_USER_RETURNING_SUFFIX: &str = concat!(
    " RETURNING id, google_id, email, name, avatar_url, ",
    "auth_provider, external_id, created_at, last_login"
);

/// Triple-quoted literal from `upsert_oidc_user`'s update branch.
const UPDATE_OIDC_USER_SQL: &str = "
                        UPDATE users
                           SET email = COALESCE(?, email),
                               name = COALESCE(?, name),
                               avatar_url = COALESCE(?, avatar_url),
                               last_login = CURRENT_TIMESTAMP
                         WHERE id = ?
                        ";

const GET_USER_PASSWORD_HASH_SQL: &str = "SELECT password_hash FROM users WHERE id = ?";

const SET_USER_PASSWORD_SQL: &str = "UPDATE users SET password_hash = ? WHERE id = ?";

/// One `users` row as the Python mixin returns it: `dict(sqlite3.Row)` over
/// the shared SELECT column list. Field names (= serialized JSON keys) match
/// the Flask wire shape exactly: `id, google_id, email, name, avatar_url,
/// auth_provider, external_id, created_at, last_login`.
///
/// `password_hash` and `theme_preference` are deliberately absent — the
/// Python SELECTs never include them; they have dedicated getters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UserRow {
    pub id: i64,
    pub google_id: String,
    pub email: String,
    pub name: String,
    pub avatar_url: Option<String>,
    /// NULL on rows created before the Phase 2a provider migration.
    pub auth_provider: Option<String>,
    pub external_id: Option<String>,
    /// Defensive `Option`: partially-migrated DBs exist in the wild.
    pub created_at: Option<String>,
    pub last_login: Option<String>,
}

impl UserRow {
    /// Map a row from any query that SELECTs the shared 9-column list in
    /// order (`GET_USER_BY_*`, the UPSERT's RETURNING clause).
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        Ok(UserRow {
            id: row.get(0)?,
            google_id: row.get(1)?,
            email: row.get(2)?,
            name: row.get(3)?,
            avatar_url: row.get(4)?,
            auth_provider: row.get(5)?,
            external_id: row.get(6)?,
            created_at: row.get(7)?,
            last_login: row.get(8)?,
        })
    }
}

/// Python truthiness for create-path string defaults: `None` and `""` both
/// count as missing (`email or default`).
fn non_empty(value: Option<&str>) -> Option<&str> {
    value.filter(|s| !s.is_empty())
}

/// Port of `UsersMixin._compat_google_id`: the synthetic
/// `"<provider>:<external_id>"` stored in the legacy `google_id` column so
/// inserts keep working on schemas where it is UNIQUE NOT NULL. New code
/// never looks users up by `google_id`.
pub fn compat_google_id(provider: &str, external_id: &str) -> String {
    format!("{provider}:{external_id}")
}

/// Port of `create_user`: plain INSERT keyed by `google_id`; returns the
/// new row id (`int(cursor.lastrowid or 0)`).
pub fn create_user(
    conn: &Connection,
    google_id: &str,
    email: &str,
    name: &str,
    avatar_url: Option<&str>,
) -> Result<i64, DatabaseError> {
    conn.execute(
        sql_queries::CREATE_USER,
        params![google_id, email, name, avatar_url],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Port of `get_user_by_google_id`.
pub fn get_user_by_google_id(
    conn: &Connection,
    google_id: &str,
) -> Result<Option<UserRow>, DatabaseError> {
    Ok(conn
        .query_row(
            sql_queries::GET_USER_BY_GOOGLE_ID,
            [google_id],
            UserRow::from_row,
        )
        .optional()?)
}

/// Port of `get_user_by_id`.
pub fn get_user_by_id(conn: &Connection, user_id: i64) -> Result<Option<UserRow>, DatabaseError> {
    Ok(conn
        .query_row(sql_queries::GET_USER_BY_ID, [user_id], UserRow::from_row)
        .optional()?)
}

/// Port of `get_user_theme`. Bug-compatible flattening: `None` for both a
/// missing user and a NULL `theme_preference` (`row[0] if row else None`).
pub fn get_user_theme(conn: &Connection, user_id: i64) -> Result<Option<String>, DatabaseError> {
    Ok(conn
        .query_row(GET_USER_THEME_SQL, [user_id], |row| {
            row.get::<_, Option<String>>(0)
        })
        .optional()?
        .flatten())
}

/// Port of `set_user_theme`. Like the Python, silently a no-op when the
/// user id does not exist (UPDATE matches zero rows).
pub fn set_user_theme(conn: &Connection, user_id: i64, theme: &str) -> Result<(), DatabaseError> {
    conn.execute(SET_USER_THEME_SQL, params![theme, user_id])?;
    Ok(())
}

/// Port of `update_user_last_login`.
pub fn update_user_last_login(conn: &Connection, user_id: i64) -> Result<(), DatabaseError> {
    conn.execute(UPDATE_USER_LAST_LOGIN_SQL, [user_id])?;
    Ok(())
}

/// Port of `upsert_user_by_google_id`: INSERT .. ON CONFLICT(google_id)
/// DO UPDATE .. RETURNING inside one immediate transaction.
///
/// The Python has an `except sqlite3.OperationalError` fallback (re-run
/// without RETURNING, then re-SELECT) for system SQLites older than 3.35.
/// It is not ported: the `bundled` feature pins a modern SQLite where
/// RETURNING always parses, so that branch is unreachable here.
pub fn upsert_user_by_google_id(
    conn: &Connection,
    google_id: &str,
    email: Option<&str>,
    name: Option<&str>,
    avatar_url: Option<&str>,
) -> Result<Option<UserRow>, DatabaseError> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| -> Result<Option<UserRow>, DatabaseError> {
        let sql = [sql_queries::UPSERT_USER, UPSERT_USER_RETURNING_SUFFIX].concat();
        Ok(conn
            .query_row(
                &sql,
                params![google_id, email, name, avatar_url],
                UserRow::from_row,
            )
            .optional()?)
    })();
    match result {
        Ok(row) => {
            conn.execute_batch("COMMIT")?;
            Ok(row)
        }
        Err(exc) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(exc)
        }
    }
}

/// Port of `get_user_by_provider`: provider-scoped identity lookup
/// (`auth_provider`, `external_id`), the only lookup new code should use.
pub fn get_user_by_provider(
    conn: &Connection,
    auth_provider: &str,
    external_id: &str,
) -> Result<Option<UserRow>, DatabaseError> {
    Ok(conn
        .query_row(
            sql_queries::GET_USER_BY_PROVIDER,
            [auth_provider, external_id],
            UserRow::from_row,
        )
        .optional()?)
}

/// Port of `upsert_oidc_user`: select-then-insert/update inside one
/// immediate transaction (not ON CONFLICT — the row is covered by two
/// unique constraints and upsert conflict targets only handle one).
///
/// The Python signature defaults `provider="oidc"`; Rust callers pass it
/// explicitly.
pub fn upsert_oidc_user(
    conn: &Connection,
    external_id: &str,
    email: Option<&str>,
    name: Option<&str>,
    avatar_url: Option<&str>,
    provider: &str,
) -> Result<Option<UserRow>, DatabaseError> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    let result = (|| -> Result<Option<UserRow>, DatabaseError> {
        let existing = conn
            .query_row(
                sql_queries::GET_USER_BY_PROVIDER,
                [provider, external_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?;
        let user_id = match existing {
            Some(id) => {
                conn.execute(UPDATE_OIDC_USER_SQL, params![email, name, avatar_url, id])?;
                id
            }
            None => {
                // Python: `email or f"{external_id}@{provider}.invalid"`,
                // `name or "User"` — empty strings count as missing.
                let email = non_empty(email)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("{external_id}@{provider}.invalid"));
                let name = non_empty(name).unwrap_or("User");
                conn.execute(
                    sql_queries::CREATE_PROVIDER_USER,
                    params![
                        compat_google_id(provider, external_id),
                        email,
                        name,
                        avatar_url,
                        provider,
                        external_id,
                        Option::<String>::None,
                    ],
                )?;
                conn.last_insert_rowid()
            }
        };
        Ok(conn
            .query_row(sql_queries::GET_USER_BY_ID, [user_id], UserRow::from_row)
            .optional()?)
    })();
    match result {
        Ok(row) => {
            conn.execute_batch("COMMIT")?;
            Ok(row)
        }
        Err(exc) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(exc)
        }
    }
}

/// Port of `create_local_user`: create a local-auth user and return its id.
///
/// Defaults (Python truthiness — `None` or `""`): `email` falls back to
/// `<username>@localhost`, `name` to the username. Errors with a UNIQUE
/// constraint violation when a local user with this username already exists
/// (unique on `(auth_provider, external_id)`), matching the Python's
/// `sqlite3.IntegrityError`.
pub fn create_local_user(
    conn: &Connection,
    username: &str,
    password_hash: &str,
    email: Option<&str>,
    name: Option<&str>,
) -> Result<i64, DatabaseError> {
    let email = non_empty(email)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{username}@localhost"));
    let name = non_empty(name).unwrap_or(username);
    conn.execute(
        sql_queries::CREATE_PROVIDER_USER,
        params![
            compat_google_id("local", username),
            email,
            name,
            Option::<String>::None,
            "local",
            username,
            password_hash,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Port of `get_user_password_hash`. Bug-compatible flattening like
/// [`get_user_theme`]: `None` for both a missing user and a NULL hash.
pub fn get_user_password_hash(
    conn: &Connection,
    user_id: i64,
) -> Result<Option<String>, DatabaseError> {
    Ok(conn
        .query_row(GET_USER_PASSWORD_HASH_SQL, [user_id], |row| {
            row.get::<_, Option<String>>(0)
        })
        .optional()?
        .flatten())
}

/// Port of `set_user_password`: set (or replace) the stored hash. Silently
/// a no-op for an unknown user id, like the Python.
pub fn set_user_password(
    conn: &Connection,
    user_id: i64,
    password_hash: &str,
) -> Result<(), DatabaseError> {
    conn.execute(SET_USER_PASSWORD_SQL, params![password_hash, user_id])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Expected values in these tests were pinned by running the Python
    //! `UsersMixin` (api/venv, `MoodDatabase` on a fresh file) through the
    //! same call sequences on 2026-08-15 and copying its outputs.

    use super::*;
    use crate::db::bootstrap::{SelfHostSeed, bootstrap};
    use crate::db::common::connect;

    fn test_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("users.db");
        let path = path.to_str().unwrap();
        bootstrap(path, &SelfHostSeed::default()).unwrap();
        (dir, connect(path).unwrap())
    }

    /// `CURRENT_TIMESTAMP` produces `YYYY-MM-DD HH:MM:SS`.
    fn assert_timestampish(value: &Option<String>) {
        let v = value.as_deref().expect("timestamp column must be set");
        assert_eq!(v.len(), 19, "unexpected timestamp shape: {v}");
        assert_eq!(&v[4..5], "-");
        assert_eq!(&v[10..11], " ");
    }

    #[test]
    fn bootstrap_seeds_default_self_host_user_as_row_one() {
        // Python: get_user_by_google_id("selfhost_default_user") ->
        // {"auth_provider": "local", "avatar_url": null, "email":
        //  "selfhost_default_user@localhost", "external_id":
        //  "selfhost_default_user", "google_id": "selfhost_default_user",
        //  "id": 1, "name": "Me", ...}
        let (_dir, conn) = test_db();
        let row = get_user_by_google_id(&conn, "selfhost_default_user")
            .unwrap()
            .expect("default user must be seeded");
        assert_eq!(row.id, 1);
        assert_eq!(row.google_id, "selfhost_default_user");
        assert_eq!(row.email, "selfhost_default_user@localhost");
        assert_eq!(row.name, "Me");
        assert_eq!(row.avatar_url, None);
        assert_eq!(row.auth_provider.as_deref(), Some("local"));
        assert_eq!(row.external_id.as_deref(), Some("selfhost_default_user"));
        assert_timestampish(&row.created_at);
        assert_timestampish(&row.last_login);
        // Seeded user has no password hash: NULL flattens to None.
        assert_eq!(get_user_password_hash(&conn, 1).unwrap(), None);
        // Same row by id.
        assert_eq!(get_user_by_id(&conn, 1).unwrap().unwrap(), row);
    }

    #[test]
    fn user_row_serializes_with_exact_flask_field_names() {
        let (_dir, conn) = test_db();
        let row = get_user_by_id(&conn, 1).unwrap().unwrap();
        let value = serde_json::to_value(&row).unwrap();
        let mut keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.as_str())
            .collect();
        keys.sort_unstable();
        // Python: sorted(db.get_user_by_id(1).keys())
        assert_eq!(
            keys,
            vec![
                "auth_provider",
                "avatar_url",
                "created_at",
                "email",
                "external_id",
                "google_id",
                "id",
                "last_login",
                "name",
            ]
        );
        assert_eq!(value["avatar_url"], serde_json::Value::Null);
    }

    #[test]
    fn create_user_roundtrips_via_google_id_lookup() {
        let (_dir, conn) = test_db();
        let id = create_user(&conn, "g-plain", "p@q.r", "Plain", None).unwrap();
        assert_eq!(id, 2);
        let row = get_user_by_google_id(&conn, "g-plain").unwrap().unwrap();
        assert_eq!(row.id, 2);
        assert_eq!(row.email, "p@q.r");
        assert_eq!(row.name, "Plain");
        // Plain create_user leaves the provider columns NULL.
        assert_eq!(row.auth_provider, None);
        assert_eq!(row.external_id, None);
        assert_eq!(get_user_by_google_id(&conn, "nope").unwrap(), None);
        assert_eq!(get_user_by_id(&conn, 9999).unwrap(), None);
    }

    #[test]
    fn upsert_by_google_id_inserts_then_updates_with_coalesce() {
        let (_dir, conn) = test_db();
        let row = upsert_user_by_google_id(
            &conn,
            "g-123",
            Some("a@b.c"),
            Some("Alice"),
            Some("http://old-avatar"),
        )
        .unwrap()
        .expect("insert path returns the row");
        assert_eq!(row.id, 2);
        assert_eq!(row.email, "a@b.c");
        assert_eq!(row.name, "Alice");
        assert_eq!(row.avatar_url.as_deref(), Some("http://old-avatar"));
        assert_eq!(row.auth_provider, None);

        // Freeze last_login, then upsert again: email/name replaced,
        // avatar_url kept via COALESCE, last_login refreshed.
        conn.execute(
            "UPDATE users SET last_login='2020-01-01 00:00:00' WHERE google_id='g-123'",
            [],
        )
        .unwrap();
        let row = upsert_user_by_google_id(&conn, "g-123", Some("new@b.c"), Some("Alice2"), None)
            .unwrap()
            .unwrap();
        assert_eq!(row.id, 2, "update path must not create a second row");
        assert_eq!(row.email, "new@b.c");
        assert_eq!(row.name, "Alice2");
        assert_eq!(row.avatar_url.as_deref(), Some("http://old-avatar"));
        assert_ne!(row.last_login.as_deref(), Some("2020-01-01 00:00:00"));
    }

    #[test]
    fn upsert_by_google_id_null_email_fails_not_null_even_on_existing_row() {
        // Bug-compatible: SQLite checks NOT NULL before ON CONFLICT, so
        // the Python raises IntegrityError("NOT NULL constraint failed:
        // users.email") even when the row exists and DO UPDATE would have
        // COALESCEd. The transaction rolls back and the row is untouched.
        let (_dir, conn) = test_db();
        upsert_user_by_google_id(&conn, "g-123", Some("a@b.c"), Some("Alice"), None)
            .unwrap()
            .unwrap();
        let err = upsert_user_by_google_id(&conn, "g-123", None, Some("X"), None).unwrap_err();
        assert!(
            err.to_string()
                .contains("NOT NULL constraint failed: users.email"),
            "unexpected error: {err}"
        );
        let row = get_user_by_google_id(&conn, "g-123").unwrap().unwrap();
        assert_eq!(row.name, "Alice", "rollback must leave the row untouched");
        // Connection is usable again (no dangling transaction).
        conn.execute_batch("BEGIN IMMEDIATE; ROLLBACK").unwrap();
    }

    #[test]
    fn upsert_oidc_user_creates_with_python_defaults_then_updates() {
        let (_dir, conn) = test_db();
        // Python: oidc-new -> {"auth_provider": "oidc", "email":
        //  "ext-1@oidc.invalid", "external_id": "ext-1", "google_id":
        //  "oidc:ext-1", "name": "User", "avatar_url": null}
        let row = upsert_oidc_user(&conn, "ext-1", None, None, None, "oidc")
            .unwrap()
            .expect("create path returns the row");
        assert_eq!(row.google_id, "oidc:ext-1");
        assert_eq!(row.email, "ext-1@oidc.invalid");
        assert_eq!(row.name, "User");
        assert_eq!(row.avatar_url, None);
        assert_eq!(row.auth_provider.as_deref(), Some("oidc"));
        assert_eq!(row.external_id.as_deref(), Some("ext-1"));
        let created_id = row.id;

        // Python: oidc-upd -> email/avatar updated, name COALESCEs to the
        // stored "User" (None passed), same row id.
        let row = upsert_oidc_user(
            &conn,
            "ext-1",
            Some("o@e.x"),
            None,
            Some("http://av"),
            "oidc",
        )
        .unwrap()
        .unwrap();
        assert_eq!(row.id, created_id);
        assert_eq!(row.email, "o@e.x");
        assert_eq!(row.name, "User");
        assert_eq!(row.avatar_url.as_deref(), Some("http://av"));

        // Provider-scoped lookup finds it; other provider does not.
        let found = get_user_by_provider(&conn, "oidc", "ext-1")
            .unwrap()
            .unwrap();
        assert_eq!(found, row);
        assert_eq!(get_user_by_provider(&conn, "local", "ext-1").unwrap(), None);

        // Empty strings count as missing on the create path (Python `or`).
        let row = upsert_oidc_user(&conn, "ext-2", Some(""), Some(""), None, "oidc")
            .unwrap()
            .unwrap();
        assert_eq!(row.email, "ext-2@oidc.invalid");
        assert_eq!(row.name, "User");
    }

    #[test]
    fn create_local_user_defaults_and_duplicate_constraint() {
        let (_dir, conn) = test_db();
        // Python: local-row -> {"auth_provider": "local", "email":
        //  "bob@localhost", "external_id": "bob", "google_id": "local:bob",
        //  "name": "bob"}; pwhash -> "hash1"
        let id = create_local_user(&conn, "bob", "hash1", None, None).unwrap();
        let row = get_user_by_provider(&conn, "local", "bob")
            .unwrap()
            .unwrap();
        assert_eq!(row.id, id);
        assert_eq!(row.google_id, "local:bob");
        assert_eq!(row.email, "bob@localhost");
        assert_eq!(row.name, "bob");
        assert_eq!(row.auth_provider.as_deref(), Some("local"));
        assert_eq!(row.external_id.as_deref(), Some("bob"));
        assert_eq!(
            get_user_password_hash(&conn, id).unwrap().as_deref(),
            Some("hash1")
        );

        // Explicit email/name are used as-is; empty strings fall back.
        let id2 = create_local_user(&conn, "carol", "h", Some("c@x.y"), Some("Carol")).unwrap();
        let row2 = get_user_by_id(&conn, id2).unwrap().unwrap();
        assert_eq!(row2.email, "c@x.y");
        assert_eq!(row2.name, "Carol");
        let id3 = create_local_user(&conn, "dave", "h", Some(""), Some("")).unwrap();
        let row3 = get_user_by_id(&conn, id3).unwrap().unwrap();
        assert_eq!(row3.email, "dave@localhost");
        assert_eq!(row3.name, "dave");

        // Duplicate username -> UNIQUE constraint error (Python:
        // sqlite3.IntegrityError "UNIQUE constraint failed:
        // users.auth_provider, users.external_id").
        let err = create_local_user(&conn, "bob", "hash2", None, None).unwrap_err();
        assert!(
            err.to_string().contains("UNIQUE constraint failed"),
            "unexpected error: {err}"
        );
        match err {
            DatabaseError::Sqlite(rusqlite::Error::SqliteFailure(inner, _)) => {
                assert_eq!(inner.code, rusqlite::ErrorCode::ConstraintViolation);
            }
            other => panic!("expected a constraint violation, got {other:?}"),
        }
    }

    #[test]
    fn password_hash_get_set_flattening() {
        let (_dir, conn) = test_db();
        let id = create_local_user(&conn, "bob", "hash1", None, None).unwrap();
        // Missing user and NULL hash are both None (Python
        // `row[0] if row else None`).
        assert_eq!(get_user_password_hash(&conn, 9999).unwrap(), None);
        assert_eq!(get_user_password_hash(&conn, 1).unwrap(), None);
        set_user_password(&conn, id, "hash2").unwrap();
        assert_eq!(
            get_user_password_hash(&conn, id).unwrap().as_deref(),
            Some("hash2")
        );
        // Unknown id: silent no-op, like the Python.
        set_user_password(&conn, 9999, "x").unwrap();
    }

    #[test]
    fn theme_get_set_flattening() {
        let (_dir, conn) = test_db();
        // Fresh user: theme_preference has NO default -> None until set
        // (contract fixture preferences_get_200_default.json).
        assert_eq!(get_user_theme(&conn, 1).unwrap(), None);
        // Missing user is also None (flattened, like the Python).
        assert_eq!(get_user_theme(&conn, 9999).unwrap(), None);
        set_user_theme(&conn, 1, "synthwave").unwrap();
        assert_eq!(
            get_user_theme(&conn, 1).unwrap().as_deref(),
            Some("synthwave")
        );
        // Unknown id: silent no-op.
        set_user_theme(&conn, 9999, "dark").unwrap();
        assert_eq!(get_user_theme(&conn, 9999).unwrap(), None);
    }

    #[test]
    fn update_user_last_login_touches_timestamp() {
        let (_dir, conn) = test_db();
        conn.execute(
            "UPDATE users SET last_login='2020-01-01 00:00:00' WHERE id=1",
            [],
        )
        .unwrap();
        update_user_last_login(&conn, 1).unwrap();
        let row = get_user_by_id(&conn, 1).unwrap().unwrap();
        assert_ne!(row.last_login.as_deref(), Some("2020-01-01 00:00:00"));
        assert_timestampish(&row.last_login);
    }

    #[test]
    fn compat_google_id_format() {
        assert_eq!(compat_google_id("local", "bob"), "local:bob");
        assert_eq!(compat_google_id("oidc", "ext-1"), "oidc:ext-1");
    }
}
