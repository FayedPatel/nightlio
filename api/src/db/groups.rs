//! Port of `api/database_groups.py` (`GroupsMixin`) plus the
//! `ensure_default_groups_for_user` seed helper from
//! `api/database_schema.py` that the user service calls at first login.
//!
//! Semantics preserved from the Python side:
//! - all group reads/writes are scoped to a user; `user_id: None` is the
//!   backward-compat shim that resolves to the seeded self-host user
//!   (`_resolve_user_id`);
//! - both nesting levels are ordered by `name` under SQLite's BINARY
//!   collation (uppercase before lowercase — the bundled SQLite pins this);
//! - creating an option under an unknown or foreign group raises
//!   `ValueError("Group not found for user")`; the HTTP layer remaps it to
//!   404 (contract change — DECISIONS.md; Flask returned 400);
//! - `get_entry_selections` returns an empty list for nonexistent/foreign
//!   entries (contract/DECISIONS.md item 5);
//! - multi-statement writes commit-or-rollback as a unit, mirroring
//!   Python's `with conn:` connection context manager;
//! - `ensure_default_groups_for_user` inserts a group's options only when
//!   the group row itself is inserted (a group left without its options is
//!   deliberately not repaired — matches the Python).
//!
//! Every function takes a `&Connection`; callers run them under
//! `tokio::task::spawn_blocking`.

use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

use super::bootstrap::DEFAULT_GROUPS;
use super::common::{DatabaseError, resolve_user_id};
use crate::error::ApiError;

/// Groups-specific SQL, kept byte-identical to the string literals in
/// `api/database_groups.py` / `api/database_schema.py` (including the
/// leading newline and indentation of the Python triple-quoted strings) so
/// the executed SQL text cannot drift from what the Flask binary runs.
pub mod groups_sql {
    /// `GroupsMixin.get_all_groups` — outer query.
    pub const GET_GROUPS_BY_USER: &str =
        "SELECT id, name FROM groups WHERE user_id = ? ORDER BY name";

    /// `GroupsMixin.get_all_groups` — per-group options query.
    pub const GET_OPTIONS_BY_GROUP: &str = "
                    SELECT id, name
                      FROM group_options
                     WHERE group_id = ?
                     ORDER BY name
                    ";

    /// `GroupsMixin.create_group` and the seed helper's group insert.
    pub const INSERT_GROUP: &str = "INSERT INTO groups (name, user_id) VALUES (?, ?)";

    /// `GroupsMixin.create_group_option` — ownership check.
    pub const GET_GROUP_OWNED_BY_USER: &str = "SELECT id FROM groups WHERE id = ? AND user_id = ?";

    /// `GroupsMixin.create_group_option` and the seed helper's option insert.
    pub const INSERT_GROUP_OPTION: &str =
        "INSERT INTO group_options (group_id, name) VALUES (?, ?)";

    /// `GroupsMixin.delete_group`.
    pub const DELETE_GROUP: &str = "DELETE FROM groups WHERE id = ? AND user_id = ?";

    /// `GroupsMixin.delete_group_option`.
    pub const DELETE_GROUP_OPTION: &str = "
                DELETE FROM group_options
                 WHERE id = ?
                   AND group_id IN (SELECT id FROM groups WHERE user_id = ?)
                ";

    /// `GroupsMixin.add_entry_selections`.
    pub const INSERT_ENTRY_SELECTION: &str =
        "INSERT INTO entry_selections (entry_id, option_id) VALUES (?, ?)";

    /// `GroupsMixin.get_entry_selections` — with entry-ownership check.
    pub const GET_ENTRY_SELECTIONS_FOR_USER: &str = "
                    SELECT go.id, go.name, g.name as group_name
                      FROM entry_selections es
                      JOIN mood_entries me ON es.entry_id = me.id
                      JOIN group_options go ON es.option_id = go.id
                      JOIN groups g ON go.group_id = g.id
                     WHERE es.entry_id = ? AND me.user_id = ?
                     ORDER BY g.name, go.name
                    ";

    /// `GroupsMixin.get_entry_selections` — ownership verified by caller.
    pub const GET_ENTRY_SELECTIONS: &str = "
                    SELECT go.id, go.name, g.name as group_name
                      FROM entry_selections es
                      JOIN group_options go ON es.option_id = go.id
                      JOIN groups g ON go.group_id = g.id
                     WHERE es.entry_id = ?
                     ORDER BY g.name, go.name
                    ";

    /// `ensure_default_groups_for_user` — per-group existence probe.
    pub const GET_GROUP_BY_NAME_AND_USER: &str =
        "SELECT id FROM groups WHERE name = ? AND user_id = ?";
}

/// Error type for the groups mixin. The Python raises `ValueError` for the
/// two client-caused failures; everything else is a database error.
#[derive(Debug, thiserror::Error)]
pub enum GroupsError {
    /// Port of the Python `ValueError`s. The Flask app's `ValueError`
    /// handler turns these into HTTP 400 with the message verbatim
    /// (contract/DECISIONS.md item 6: unknown/foreign group on option
    /// creation is 400, NOT 404).
    #[error("{0}")]
    Value(String),

    #[error(transparent)]
    Database(#[from] DatabaseError),
}

impl From<rusqlite::Error> for GroupsError {
    fn from(exc: rusqlite::Error) -> Self {
        GroupsError::Database(DatabaseError::Sqlite(exc))
    }
}

impl From<GroupsError> for ApiError {
    fn from(exc: GroupsError) -> Self {
        match exc {
            GroupsError::Value(message) => ApiError::Validation(message),
            GroupsError::Database(err) => ApiError::Internal(anyhow::Error::new(err)),
        }
    }
}

/// One `group_options` row as the Flask API emits it (`{id, name}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GroupOption {
    pub id: i64,
    pub name: String,
}

/// One group with its nested options, as `GET /api/groups` emits it
/// (`{id, name, options}` — no user_id, created_at, or counts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Group {
    pub id: i64,
    pub name: String,
    pub options: Vec<GroupOption>,
}

/// One selected option for an entry, as `GET /api/mood/{id}/selections`
/// emits it (`{id, name, group_name}` — ids are `group_options` ids).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EntrySelection {
    pub id: i64,
    pub name: String,
    pub group_name: String,
}

/// Port of `GroupsMixin.get_all_groups`: every group owned by the user,
/// ordered by name, each with its options ordered by name. Returns an empty
/// list when `user_id` is None and no default self-host user exists.
pub fn get_all_groups(
    conn: &Connection,
    user_id: Option<i64>,
    default_self_host_id: &str,
) -> Result<Vec<Group>, GroupsError> {
    let Some(uid) = resolve_user_id(conn, user_id, default_self_host_id)? else {
        return Ok(Vec::new());
    };

    let mut group_stmt = conn.prepare(groups_sql::GET_GROUPS_BY_USER)?;
    let bare_groups = group_stmt
        .query_map([uid], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    let mut options_stmt = conn.prepare(groups_sql::GET_OPTIONS_BY_GROUP)?;
    let mut groups = Vec::with_capacity(bare_groups.len());
    for (id, name) in bare_groups {
        let options = options_stmt
            .query_map([id], |row| {
                Ok(GroupOption {
                    id: row.get(0)?,
                    name: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        groups.push(Group { id, name, options });
    }
    Ok(groups)
}

/// Port of `GroupsMixin.create_group`. Returns the new group id. Errors
/// with `ValueError("No user available to own the group")` when no user can
/// be resolved.
pub fn create_group(
    conn: &Connection,
    name: &str,
    user_id: Option<i64>,
    default_self_host_id: &str,
) -> Result<i64, GroupsError> {
    let Some(uid) = resolve_user_id(conn, user_id, default_self_host_id)? else {
        return Err(GroupsError::Value(
            "No user available to own the group".to_string(),
        ));
    };
    conn.execute(groups_sql::INSERT_GROUP, rusqlite::params![name, uid])?;
    Ok(conn.last_insert_rowid())
}

/// Port of `GroupsMixin.create_group_option`. Verifies the parent group is
/// owned by the (resolved) user before inserting; an unknown or foreign
/// group errors with `ValueError("Group not found for user")` — the route
/// handler remaps this to HTTP 404 (contract change, DECISIONS.md).
pub fn create_group_option(
    conn: &Connection,
    group_id: i64,
    name: &str,
    user_id: Option<i64>,
    default_self_host_id: &str,
) -> Result<i64, GroupsError> {
    // The Python does not None-check here: a NULL uid simply matches no
    // group row and falls through to the same ValueError.
    let uid = resolve_user_id(conn, user_id, default_self_host_id)?;
    let owner_row: Option<i64> = conn
        .query_row(
            groups_sql::GET_GROUP_OWNED_BY_USER,
            rusqlite::params![group_id, uid],
            |row| row.get(0),
        )
        .optional()?;
    if owner_row.is_none() {
        return Err(GroupsError::Value("Group not found for user".to_string()));
    }
    conn.execute(
        groups_sql::INSERT_GROUP_OPTION,
        rusqlite::params![group_id, name],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Port of `GroupsMixin.delete_group`: true iff a row was deleted (foreign
/// or unknown ids delete nothing). With `PRAGMA foreign_keys=ON` the
/// delete cascades to `group_options` and on to `entry_selections`.
pub fn delete_group(
    conn: &Connection,
    group_id: i64,
    user_id: Option<i64>,
    default_self_host_id: &str,
) -> Result<bool, GroupsError> {
    let uid = resolve_user_id(conn, user_id, default_self_host_id)?;
    let rows = conn.execute(groups_sql::DELETE_GROUP, rusqlite::params![group_id, uid])?;
    Ok(rows > 0)
}

/// Port of `GroupsMixin.delete_group_option`: true iff a row was deleted.
/// Ownership is scoped through the parent group (`group_options` rows carry
/// no user_id of their own).
pub fn delete_group_option(
    conn: &Connection,
    option_id: i64,
    user_id: Option<i64>,
    default_self_host_id: &str,
) -> Result<bool, GroupsError> {
    let uid = resolve_user_id(conn, user_id, default_self_host_id)?;
    let rows = conn.execute(
        groups_sql::DELETE_GROUP_OPTION,
        rusqlite::params![option_id, uid],
    )?;
    Ok(rows > 0)
}

/// Port of `GroupsMixin.add_entry_selections`. All rows are inserted in one
/// transaction: the Python runs `executemany` inside `with conn:`, which
/// commits on success and rolls the whole batch back on any failure.
pub fn add_entry_selections(
    conn: &Connection,
    entry_id: i64,
    option_ids: &[i64],
) -> Result<(), GroupsError> {
    let tx = conn.unchecked_transaction()?;
    {
        let mut stmt = tx.prepare(groups_sql::INSERT_ENTRY_SELECTION)?;
        for option_id in option_ids {
            stmt.execute(rusqlite::params![entry_id, option_id])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Port of `GroupsMixin.get_entry_selections`: the selected options for an
/// entry, ordered by group name then option name.
///
/// When `user_id` is provided the entry is additionally verified to belong
/// to that user; without it the caller (e.g. the mood service) is expected
/// to have verified entry ownership already. Note there is NO
/// `_resolve_user_id` here — `None` means "skip the ownership check", not
/// "the default user". Nonexistent or foreign entries yield an empty list,
/// never an error (contract/DECISIONS.md item 5).
pub fn get_entry_selections(
    conn: &Connection,
    entry_id: i64,
    user_id: Option<i64>,
) -> Result<Vec<EntrySelection>, GroupsError> {
    let map_row = |row: &rusqlite::Row<'_>| -> rusqlite::Result<EntrySelection> {
        Ok(EntrySelection {
            id: row.get(0)?,
            name: row.get(1)?,
            group_name: row.get(2)?,
        })
    };
    let selections = match user_id {
        Some(uid) => {
            let mut stmt = conn.prepare(groups_sql::GET_ENTRY_SELECTIONS_FOR_USER)?;
            let rows = stmt.query_map(rusqlite::params![entry_id, uid], map_row)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        }
        None => {
            let mut stmt = conn.prepare(groups_sql::GET_ENTRY_SELECTIONS)?;
            let rows = stmt.query_map(rusqlite::params![entry_id], map_row)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        }
    };
    Ok(selections)
}

/// Port of `database_schema.ensure_default_groups_for_user` for a live
/// connection (the user service calls this at first login). Idempotent per
/// user. A group's options are inserted only when the group row itself is
/// inserted: an existing group that lost its options is deliberately left
/// alone, exactly like the Python. Runs in one transaction, mirroring the
/// Python's `with conn:` commit-or-rollback.
pub fn ensure_default_groups_for_user(conn: &Connection, user_id: i64) -> Result<(), GroupsError> {
    let tx = conn.unchecked_transaction()?;
    for (group_name, options) in DEFAULT_GROUPS {
        let group_row: Option<i64> = tx
            .query_row(
                groups_sql::GET_GROUP_BY_NAME_AND_USER,
                rusqlite::params![group_name, user_id],
                |row| row.get(0),
            )
            .optional()?;

        if group_row.is_none() {
            tx.execute(
                groups_sql::INSERT_GROUP,
                rusqlite::params![group_name, user_id],
            )?;
            let group_id = tx.last_insert_rowid();
            for option in *options {
                tx.execute(
                    groups_sql::INSERT_GROUP_OPTION,
                    rusqlite::params![group_id, option],
                )?;
            }
        }
    }
    tx.commit()?;
    tracing::info!("Default groups ensured for user {user_id}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::bootstrap::SelfHostSeed;
    use crate::db::{bootstrap, connect};
    use serde_json::json;

    /// The seed identity `SelfHostSeed::default()` bootstraps with.
    const SELF_HOST_ID: &str = "selfhost_default_user";

    /// Bootstrapped tempfile DB (default user id 1; default groups 1-3 with
    /// options 1-27) plus a second user (id 2) with no groups.
    fn seeded_db() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("groups.db");
        let path = path.to_str().unwrap();
        bootstrap(path, &SelfHostSeed::default()).unwrap();
        let conn = connect(path).unwrap();
        conn.execute(
            "INSERT INTO users (google_id, email, name) VALUES ('user2', 'u2@example.com', 'User Two')",
            [],
        )
        .unwrap();
        (dir, conn)
    }

    fn count(conn: &Connection, sql: &str, params: &[&dyn rusqlite::ToSql]) -> i64 {
        conn.query_row(sql, params, |row| row.get(0)).unwrap()
    }

    /// End-to-end parity test. Every operation below was mirrored 1:1 in
    /// `scratchpad/groups_crosscheck.py` against the Python data layer
    /// (`api/venv/bin/python`, importing `database.MoodDatabase` from
    /// `api/`) on an identically-seeded DB; the asserted JSON payloads,
    /// return values, and row counts are pinned verbatim from that run.
    /// The mixed-case/accented option names pin SQLite's BINARY collation
    /// (uppercase < lowercase < multibyte UTF-8).
    #[test]
    fn python_parity_full_scenario() {
        let (_dir, conn) = seeded_db();
        let uid = super::super::common::get_default_user_id(&conn, SELF_HOST_ID)
            .unwrap()
            .expect("bootstrap seeds the default user");
        assert_eq!(uid, 1, "Python run: default_user_id: 1");
        let user2: i64 = conn
            .query_row("SELECT id FROM users WHERE google_id = 'user2'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(user2, 2, "Python run: user2_id: 2");

        // Seed writes — same order as the Python script.
        let g_weather = create_group(&conn, "Weather", None, SELF_HOST_ID).unwrap();
        let g_case = create_group(&conn, "Case Test", None, SELF_HOST_ID).unwrap();
        assert_eq!(
            (g_weather, g_case),
            (4, 5),
            "Python run: g_weather: 4 g_case: 5"
        );

        let mut opt_ids = Vec::new();
        for name in ["Exercise", "Reading", "angry", "Zebra", "apple", "École"] {
            opt_ids.push(create_group_option(&conn, g_case, name, None, SELF_HOST_ID).unwrap());
        }
        for name in ["sunny", "Rainy"] {
            opt_ids.push(create_group_option(&conn, g_weather, name, None, SELF_HOST_ID).unwrap());
        }
        assert_eq!(
            opt_ids,
            vec![28, 29, 30, 31, 32, 33, 34, 35],
            "Python run: option_ids: [28, 29, 30, 31, 32, 33, 34, 35]"
        );

        conn.execute(
            "INSERT INTO mood_entries (user_id, date, mood, content) VALUES (?, '2026-08-01', 4, 'seed')",
            [uid],
        )
        .unwrap();
        let entry_id = conn.last_insert_rowid();
        assert_eq!(entry_id, 1, "Python run: entry_id: 1");

        // happy (default-seed option id 1), sunny, Rainy, Exercise, Zebra.
        add_entry_selections(&conn, entry_id, &[1, 34, 35, 28, 31]).unwrap();

        // get_all_groups: pinned from the Python run (GET_ALL_GROUPS).
        let groups = get_all_groups(&conn, None, SELF_HOST_ID).unwrap();
        let expected = json!([
            {"id": 5, "name": "Case Test", "options": [
                {"id": 28, "name": "Exercise"}, {"id": 29, "name": "Reading"},
                {"id": 31, "name": "Zebra"}, {"id": 30, "name": "angry"},
                {"id": 32, "name": "apple"}, {"id": 33, "name": "École"}]},
            {"id": 1, "name": "Emotions", "options": [
                {"id": 10, "name": "angry"}, {"id": 9, "name": "anxious"},
                {"id": 8, "name": "bored"}, {"id": 5, "name": "content"},
                {"id": 13, "name": "desperate"}, {"id": 2, "name": "excited"},
                {"id": 3, "name": "grateful"}, {"id": 1, "name": "happy"},
                {"id": 4, "name": "relaxed"}, {"id": 12, "name": "sad"},
                {"id": 11, "name": "stressed"}, {"id": 6, "name": "tired"},
                {"id": 7, "name": "unsure"}]},
            {"id": 3, "name": "Productivity", "options": [
                {"id": 22, "name": "accomplished"}, {"id": 23, "name": "busy"},
                {"id": 24, "name": "distracted"}, {"id": 20, "name": "focused"},
                {"id": 27, "name": "lazy"}, {"id": 21, "name": "motivated"},
                {"id": 26, "name": "overwhelmed"}, {"id": 25, "name": "procrastinating"}]},
            {"id": 2, "name": "Sleep", "options": [
                {"id": 17, "name": "exhausted"}, {"id": 19, "name": "insomniac"},
                {"id": 15, "name": "refreshed"}, {"id": 18, "name": "restless"},
                {"id": 16, "name": "tired"}, {"id": 14, "name": "well-rested"}]},
            {"id": 4, "name": "Weather", "options": [
                {"id": 35, "name": "Rainy"}, {"id": 34, "name": "sunny"}]}
        ]);
        assert_eq!(serde_json::to_value(&groups).unwrap(), expected);

        // A user with no groups gets an empty list (GET_ALL_GROUPS_USER2).
        assert!(
            get_all_groups(&conn, Some(user2), SELF_HOST_ID)
                .unwrap()
                .is_empty()
        );

        // Selections, both branches (SELECTIONS_NO_USER / SELECTIONS_OWNER).
        let expected_selections = json!([
            {"id": 28, "name": "Exercise", "group_name": "Case Test"},
            {"id": 31, "name": "Zebra", "group_name": "Case Test"},
            {"id": 1, "name": "happy", "group_name": "Emotions"},
            {"id": 35, "name": "Rainy", "group_name": "Weather"},
            {"id": 34, "name": "sunny", "group_name": "Weather"}
        ]);
        let no_user = get_entry_selections(&conn, entry_id, None).unwrap();
        assert_eq!(serde_json::to_value(&no_user).unwrap(), expected_selections);
        let owner = get_entry_selections(&conn, entry_id, Some(uid)).unwrap();
        assert_eq!(serde_json::to_value(&owner).unwrap(), expected_selections);
        // Foreign user sees nothing (SELECTIONS_FOREIGN: []).
        assert!(
            get_entry_selections(&conn, entry_id, Some(user2))
                .unwrap()
                .is_empty()
        );

        // Foreign group on option creation: ValueError, message verbatim.
        let err =
            create_group_option(&conn, g_weather, "X", Some(user2), SELF_HOST_ID).unwrap_err();
        assert!(matches!(&err, GroupsError::Value(msg) if msg == "Group not found for user"));

        // Deletes, pinned booleans from the Python run.
        assert!(!delete_group(&conn, g_weather, Some(user2), SELF_HOST_ID).unwrap());
        assert!(!delete_group_option(&conn, opt_ids[0], Some(user2), SELF_HOST_ID).unwrap());
        assert!(delete_group_option(&conn, opt_ids[3], None, SELF_HOST_ID).unwrap()); // Zebra
        assert!(delete_group(&conn, g_weather, None, SELF_HOST_ID).unwrap());

        // Cascades pruned the deleted rows out of the selections
        // (SELECTIONS_AFTER_DELETES).
        let after = get_entry_selections(&conn, entry_id, None).unwrap();
        assert_eq!(
            serde_json::to_value(&after).unwrap(),
            json!([
                {"id": 28, "name": "Exercise", "group_name": "Case Test"},
                {"id": 1, "name": "happy", "group_name": "Emotions"}
            ])
        );

        // ensure_default_groups_for_user: seeds 3 groups / 27 options.
        ensure_default_groups_for_user(&conn, user2).unwrap();
        let groups_sql = "SELECT COUNT(*) FROM groups WHERE user_id = ?";
        let options_sql = "SELECT COUNT(*) FROM group_options WHERE group_id IN (SELECT id FROM groups WHERE user_id = ?)";
        assert_eq!(count(&conn, groups_sql, &[&user2]), 3);
        assert_eq!(count(&conn, options_sql, &[&user2]), 27);

        // Idempotent; and a group that lost its options is NOT repaired
        // (options are inserted only when the group row itself is inserted).
        ensure_default_groups_for_user(&conn, user2).unwrap();
        conn.execute(
            "DELETE FROM group_options WHERE group_id IN (SELECT id FROM groups WHERE user_id = ? AND name = 'Sleep')",
            [user2],
        )
        .unwrap();
        ensure_default_groups_for_user(&conn, user2).unwrap();
        assert_eq!(count(&conn, groups_sql, &[&user2]), 3);
        assert_eq!(
            count(&conn, options_sql, &[&user2]),
            21,
            "Python run: user2 after idempotent+partial: groups: 3 options: 21"
        );
    }

    #[test]
    fn get_all_groups_returns_empty_when_no_default_user() {
        let (_dir, conn) = seeded_db();
        conn.execute("DELETE FROM users", []).unwrap();
        assert!(
            get_all_groups(&conn, None, SELF_HOST_ID)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn create_group_without_any_user_is_value_error() {
        let (_dir, conn) = seeded_db();
        conn.execute("DELETE FROM users", []).unwrap();
        let err = create_group(&conn, "Orphan", None, SELF_HOST_ID).unwrap_err();
        assert!(
            matches!(&err, GroupsError::Value(msg) if msg == "No user available to own the group")
        );
    }

    #[test]
    fn create_group_option_unknown_group_is_value_error() {
        let (_dir, conn) = seeded_db();
        let err = create_group_option(&conn, 999, "X", None, SELF_HOST_ID).unwrap_err();
        assert!(matches!(&err, GroupsError::Value(msg) if msg == "Group not found for user"));
        // Maps to HTTP 400 with the message verbatim, not 404
        // (contract/fixtures/groups/post-group-options__404-unknown-group.json).
        let api: ApiError = err.into();
        assert!(matches!(&api, ApiError::Validation(msg) if msg == "Group not found for user"));
    }

    #[test]
    fn get_entry_selections_nonexistent_entry_is_empty_not_error() {
        let (_dir, conn) = seeded_db();
        assert!(get_entry_selections(&conn, 999, None).unwrap().is_empty());
        assert!(
            get_entry_selections(&conn, 999, Some(1))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn delete_ops_return_false_for_unknown_ids() {
        let (_dir, conn) = seeded_db();
        assert!(!delete_group(&conn, 999, None, SELF_HOST_ID).unwrap());
        assert!(!delete_group_option(&conn, 999, None, SELF_HOST_ID).unwrap());
    }

    #[test]
    fn add_entry_selections_rolls_back_the_whole_batch_on_failure() {
        let (_dir, conn) = seeded_db();
        conn.execute(
            "INSERT INTO mood_entries (user_id, date, mood, content) VALUES (1, '2026-08-01', 3, 'x')",
            [],
        )
        .unwrap();
        let entry_id = conn.last_insert_rowid();
        // Option 9999 violates the FK; Python's `with conn:` rolls back the
        // whole executemany batch, so the valid first row must not persist.
        let err = add_entry_selections(&conn, entry_id, &[1, 9999]).unwrap_err();
        assert!(matches!(err, GroupsError::Database(_)));
        assert_eq!(
            count(
                &conn,
                "SELECT COUNT(*) FROM entry_selections WHERE entry_id = ?",
                &[&entry_id],
            ),
            0
        );
    }

    #[test]
    fn serialized_shapes_match_flask_payloads() {
        // Field sets exactly match the Flask JSON
        // (contract/fixtures/groups/get-groups__happy.json,
        //  get-mood-selections__happy.json).
        let group = Group {
            id: 4,
            name: "Activities".to_string(),
            options: vec![GroupOption {
                id: 28,
                name: "Exercise".to_string(),
            }],
        };
        assert_eq!(
            serde_json::to_value(&group).unwrap(),
            json!({"id": 4, "name": "Activities", "options": [{"id": 28, "name": "Exercise"}]})
        );
        let selection = EntrySelection {
            id: 2,
            name: "excited".to_string(),
            group_name: "Emotions".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&selection).unwrap(),
            json!({"id": 2, "name": "excited", "group_name": "Emotions"})
        );
    }
}
