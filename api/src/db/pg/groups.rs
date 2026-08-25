//! PostgreSQL twin of `db/groups.rs` (the `GroupsMixin` port) for the ops
//! the store facade dispatches, plus `ensure_default_groups_for_user` (the
//! first-login seed the users store op composes with).
//!
//! Wire-quirk translation notes (docs/plans/v0.6.0.md):
//! - Both nesting levels order by `name` on `text COLLATE "C"` columns, so
//!   a plain `ORDER BY name` reproduces SQLite's BINARY collation
//!   (uppercase < lowercase < multibyte UTF-8) byte for byte — verified
//!   against a live PostgreSQL 16 with the mixed-case fixture names.
//! - `last_insert_rowid()` sites become `RETURNING id`.
//! - The `ValueError` strings (`No user available to own the group`,
//!   `Group not found for user`) are verbatim — the route layer's 400/404
//!   remapping keys off the exact message.
//! - A duplicate `(user_id, name)` insert violates the same UNIQUE index as
//!   on SQLite and surfaces as `GroupsError::Database` (route-visible 500),
//!   identical to the SQLite arm's error path.

use tokio_postgres::GenericClient;

use crate::db::bootstrap::DEFAULT_GROUPS;
use crate::db::groups::{Group, GroupOption, GroupsError};
use crate::db::pg::util;

// --- SQL (dialect-translated from `groups_sql`) ------------------------------

const GET_GROUPS_BY_USER_SQL: &str = "SELECT id, name FROM groups WHERE user_id = $1 ORDER BY name";

const GET_OPTIONS_BY_GROUP_SQL: &str =
    "SELECT id, name FROM group_options WHERE group_id = $1 ORDER BY name";

const INSERT_GROUP_SQL: &str = "INSERT INTO groups (name, user_id) VALUES ($1, $2) RETURNING id";

const GET_GROUP_OWNED_BY_USER_SQL: &str = "SELECT id FROM groups WHERE id = $1 AND user_id = $2";

const INSERT_GROUP_OPTION_SQL: &str =
    "INSERT INTO group_options (group_id, name) VALUES ($1, $2) RETURNING id";

const DELETE_GROUP_SQL: &str = "DELETE FROM groups WHERE id = $1 AND user_id = $2";

const DELETE_GROUP_OPTION_SQL: &str = "DELETE FROM group_options \
      WHERE id = $1 \
        AND group_id IN (SELECT id FROM groups WHERE user_id = $2)";

const GET_GROUP_BY_NAME_AND_USER_SQL: &str =
    "SELECT id FROM groups WHERE name = $1 AND user_id = $2";

fn db_err(exc: tokio_postgres::Error) -> GroupsError {
    GroupsError::Database(util::db_error(exc))
}

/// Twin of `groups::get_all_groups`: every group owned by the (resolved)
/// user, ordered by name, each with its options ordered by name. Empty list
/// when `user_id` is `None` and no default self-host user exists.
pub async fn get_all_groups(
    client: &impl GenericClient,
    user_id: Option<i64>,
    default_self_host_id: &str,
) -> Result<Vec<Group>, GroupsError> {
    let Some(uid) = util::resolve_user_id(client, user_id, default_self_host_id).await? else {
        return Ok(Vec::new());
    };

    let bare_groups: Vec<(i64, String)> = client
        .query(GET_GROUPS_BY_USER_SQL, &[&uid])
        .await
        .map_err(db_err)?
        .into_iter()
        .map(|row| (row.get(0), row.get(1)))
        .collect();

    let mut groups = Vec::with_capacity(bare_groups.len());
    for (id, name) in bare_groups {
        let options = client
            .query(GET_OPTIONS_BY_GROUP_SQL, &[&id])
            .await
            .map_err(db_err)?
            .into_iter()
            .map(|row| GroupOption {
                id: row.get(0),
                name: row.get(1),
            })
            .collect();
        groups.push(Group { id, name, options });
    }
    Ok(groups)
}

/// Twin of `groups::create_group`. Returns the new group id; the verbatim
/// `ValueError` when no user can be resolved.
pub async fn create_group(
    client: &impl GenericClient,
    name: &str,
    user_id: Option<i64>,
    default_self_host_id: &str,
) -> Result<i64, GroupsError> {
    let Some(uid) = util::resolve_user_id(client, user_id, default_self_host_id).await? else {
        return Err(GroupsError::Value(
            "No user available to own the group".to_string(),
        ));
    };
    let row = client
        .query_one(INSERT_GROUP_SQL, &[&name, &uid])
        .await
        .map_err(db_err)?;
    Ok(row.get(0))
}

/// Twin of `groups::create_group_option`: ownership check first; unknown or
/// foreign groups get the verbatim `Group not found for user` ValueError
/// (the route remaps it — same as the SQLite arm).
pub async fn create_group_option(
    client: &impl GenericClient,
    group_id: i64,
    name: &str,
    user_id: Option<i64>,
    default_self_host_id: &str,
) -> Result<i64, GroupsError> {
    // Like the Python/SQLite twin, no None-check: a NULL uid simply matches
    // no group row and falls through to the same ValueError.
    let uid = util::resolve_user_id(client, user_id, default_self_host_id).await?;
    let owner_row = client
        .query_opt(GET_GROUP_OWNED_BY_USER_SQL, &[&group_id, &uid])
        .await
        .map_err(db_err)?;
    if owner_row.is_none() {
        return Err(GroupsError::Value("Group not found for user".to_string()));
    }
    let row = client
        .query_one(INSERT_GROUP_OPTION_SQL, &[&group_id, &name])
        .await
        .map_err(db_err)?;
    Ok(row.get(0))
}

/// Twin of `groups::delete_group`: true iff a row was deleted; the FK
/// cascade clears `group_options` and `entry_selections`.
pub async fn delete_group(
    client: &impl GenericClient,
    group_id: i64,
    user_id: Option<i64>,
    default_self_host_id: &str,
) -> Result<bool, GroupsError> {
    let uid = util::resolve_user_id(client, user_id, default_self_host_id).await?;
    let rows = client
        .execute(DELETE_GROUP_SQL, &[&group_id, &uid])
        .await
        .map_err(db_err)?;
    Ok(rows > 0)
}

/// Twin of `groups::delete_group_option`: true iff a row was deleted;
/// ownership scoped through the parent group.
pub async fn delete_group_option(
    client: &impl GenericClient,
    option_id: i64,
    user_id: Option<i64>,
    default_self_host_id: &str,
) -> Result<bool, GroupsError> {
    let uid = util::resolve_user_id(client, user_id, default_self_host_id).await?;
    let rows = client
        .execute(DELETE_GROUP_OPTION_SQL, &[&option_id, &uid])
        .await
        .map_err(db_err)?;
    Ok(rows > 0)
}

/// Twin of `groups::ensure_default_groups_for_user`: idempotent per user;
/// one transaction; a group's options are inserted only when the group row
/// itself is inserted (an existing group that lost its options is
/// deliberately not repaired — same as the Python and SQLite twins).
pub async fn ensure_default_groups_for_user(
    client: &mut impl GenericClient,
    user_id: i64,
) -> Result<(), GroupsError> {
    let tx = client.transaction().await.map_err(db_err)?;
    for (group_name, options) in DEFAULT_GROUPS {
        let group_row = tx
            .query_opt(GET_GROUP_BY_NAME_AND_USER_SQL, &[group_name, &user_id])
            .await
            .map_err(db_err)?;
        if group_row.is_none() {
            let group_id: i64 = tx
                .query_one(INSERT_GROUP_SQL, &[group_name, &user_id])
                .await
                .map_err(db_err)?
                .get(0);
            for option in *options {
                tx.execute(INSERT_GROUP_OPTION_SQL, &[&group_id, option])
                    .await
                    .map_err(db_err)?;
            }
        }
    }
    tx.commit().await.map_err(db_err)?;
    tracing::info!("Default groups ensured for user {user_id}");
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Live-PG quirk tests (skipped unless `NIGHTLIO_PG_TEST_URL` is set).
    //! The ordering assertions pin the same JSON the SQLite twin's
    //! `python_parity_full_scenario` pinned from the Python data layer —
    //! COLLATE "C" must reproduce BINARY collation byte for byte.

    use serde_json::json;

    use super::*;
    use crate::db::pg::util::test_support::{connect_scratch, seed_default_user};

    const SELF_HOST_ID: &str = "selfhost_default_user";

    #[tokio::test]
    async fn byte_order_collation_and_crud_match_the_sqlite_pins() {
        let Some(mut client) = connect_scratch("ws4a_groups_scenario").await else {
            return;
        };
        let uid = seed_default_user(&client).await;
        let user2: i64 = client
            .query_one(
                "INSERT INTO users (google_id, email, name) \
                 VALUES ('user2', 'u2@example.com', 'User Two') RETURNING id",
                &[],
            )
            .await
            .unwrap()
            .get(0);

        // Seed the default groups (ids 1-3, options 1-27 on a fresh schema),
        // then the same mixed-case fixture the SQLite parity test uses.
        ensure_default_groups_for_user(&mut client, uid)
            .await
            .unwrap();
        let g_weather = create_group(&client, "Weather", None, SELF_HOST_ID)
            .await
            .unwrap();
        let g_case = create_group(&client, "Case Test", None, SELF_HOST_ID)
            .await
            .unwrap();
        let mut opt_ids = Vec::new();
        for name in ["Exercise", "Reading", "angry", "Zebra", "apple", "École"] {
            opt_ids.push(
                create_group_option(&client, g_case, name, None, SELF_HOST_ID)
                    .await
                    .unwrap(),
            );
        }
        for name in ["sunny", "Rainy"] {
            opt_ids.push(
                create_group_option(&client, g_weather, name, None, SELF_HOST_ID)
                    .await
                    .unwrap(),
            );
        }

        // COLLATE "C" ordering: uppercase < lowercase < multibyte UTF-8, at
        // both nesting levels — the exact quirk the fixtures pin.
        let groups = get_all_groups(&client, None, SELF_HOST_ID).await.unwrap();
        let names: Vec<&str> = groups.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(
            names,
            ["Case Test", "Emotions", "Productivity", "Sleep", "Weather"]
        );
        let case_group = groups.iter().find(|g| g.name == "Case Test").unwrap();
        let case_options: Vec<&str> = case_group.options.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(
            case_options,
            ["Exercise", "Reading", "Zebra", "angry", "apple", "École"],
            "COLLATE \"C\" must reproduce SQLite BINARY ordering"
        );
        let weather = groups.iter().find(|g| g.name == "Weather").unwrap();
        assert_eq!(
            serde_json::to_value(&weather.options).unwrap(),
            json!([
                {"id": opt_ids[7], "name": "Rainy"},
                {"id": opt_ids[6], "name": "sunny"}
            ])
        );

        // A user with no groups gets an empty list.
        assert!(
            get_all_groups(&client, Some(user2), SELF_HOST_ID)
                .await
                .unwrap()
                .is_empty()
        );

        // Foreign group on option creation: verbatim ValueError message.
        let err = create_group_option(&client, g_weather, "X", Some(user2), SELF_HOST_ID)
            .await
            .unwrap_err();
        assert!(matches!(&err, GroupsError::Value(msg) if msg == "Group not found for user"));

        // Deletes: foreign ids delete nothing, owned ids cascade.
        assert!(
            !delete_group(&client, g_weather, Some(user2), SELF_HOST_ID)
                .await
                .unwrap()
        );
        assert!(
            !delete_group_option(&client, opt_ids[0], Some(user2), SELF_HOST_ID)
                .await
                .unwrap()
        );
        assert!(
            delete_group_option(&client, opt_ids[3], None, SELF_HOST_ID)
                .await
                .unwrap()
        );
        assert!(
            delete_group(&client, g_weather, None, SELF_HOST_ID)
                .await
                .unwrap()
        );
        let weather_options: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM group_options WHERE group_id = $1",
                &[&g_weather],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!(weather_options, 0, "delete must cascade to options");

        // ensure_default_groups_for_user: 3 groups / 27 options, idempotent,
        // and a group that lost its options is NOT repaired.
        ensure_default_groups_for_user(&mut client, user2)
            .await
            .unwrap();
        ensure_default_groups_for_user(&mut client, user2)
            .await
            .unwrap();
        client
            .execute(
                "DELETE FROM group_options WHERE group_id IN \
                 (SELECT id FROM groups WHERE user_id = $1 AND name = 'Sleep')",
                &[&user2],
            )
            .await
            .unwrap();
        ensure_default_groups_for_user(&mut client, user2)
            .await
            .unwrap();
        let group_count: i64 = client
            .query_one("SELECT COUNT(*) FROM groups WHERE user_id = $1", &[&user2])
            .await
            .unwrap()
            .get(0);
        let option_count: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM group_options WHERE group_id IN \
                 (SELECT id FROM groups WHERE user_id = $1)",
                &[&user2],
            )
            .await
            .unwrap()
            .get(0);
        assert_eq!((group_count, option_count), (3, 21));

        // No default user: empty list / verbatim ValueError.
        client.execute("DELETE FROM users", &[]).await.unwrap();
        assert!(
            get_all_groups(&client, None, SELF_HOST_ID)
                .await
                .unwrap()
                .is_empty()
        );
        let err = create_group(&client, "Orphan", None, SELF_HOST_ID)
            .await
            .unwrap_err();
        assert!(
            matches!(&err, GroupsError::Value(msg) if msg == "No user available to own the group")
        );
    }
}
