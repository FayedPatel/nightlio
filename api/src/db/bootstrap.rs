//! Verbatim port of `api/database_schema.py` — the version-0 bootstrap.
//!
//! There is no version marker in the wild (`PRAGMA user_version` = 0, no
//! migrations table); every existing self-hoster's file is version 0
//! detected by introspection, exactly as the Python does it. Ordering,
//! backfill semantics, the groups rebuild, and the swallow-and-log error
//! handling below all mirror `DatabaseSchemaMixin.init_database` one for
//! one. The DDL strings are byte-identical to the Python triple-quoted
//! strings (including indentation) so `sqlite_master.sql` comes out
//! identical to what the Flask binary writes.
//!
//! The bootstrap runs on a dedicated connection where `PRAGMA foreign_keys`
//! stays OFF (SQLite's default — same as Python's `sqlite3.connect`),
//! because the groups rebuild depends on it: with FK enforcement ON,
//! `DROP TABLE groups` would cascade `group_options` away. `PRAGMA
//! foreign_keys` is a no-op inside an open transaction, so the rebuild
//! re-asserts the pragma and *verifies* it reads 0 before `BEGIN
//! IMMEDIATE`.

use std::collections::HashSet;

use rusqlite::{Connection, OptionalExtension};

use super::common::{
    DatabaseError, connect, ensure_parent_dir, get_default_user_id, table_columns,
};
use crate::config::Config;

/// Default tag groups seeded for a user on first initialization
/// (`DEFAULT_GROUPS` in `api/database_schema.py`; order preserved).
pub const DEFAULT_GROUPS: &[(&str, &[&str])] = &[
    (
        "Emotions",
        &[
            "happy",
            "excited",
            "grateful",
            "relaxed",
            "content",
            "tired",
            "unsure",
            "bored",
            "anxious",
            "angry",
            "stressed",
            "sad",
            "desperate",
        ],
    ),
    (
        "Sleep",
        &[
            "well-rested",
            "refreshed",
            "tired",
            "exhausted",
            "restless",
            "insomniac",
        ],
    ),
    (
        "Productivity",
        &[
            "focused",
            "motivated",
            "accomplished",
            "busy",
            "distracted",
            "procrastinating",
            "overwhelmed",
            "lazy",
        ],
    ),
];

/// Identity of the seeded self-host user (`_ensure_default_user` inputs:
/// `DEFAULT_SELF_HOST_ID`, `SELFHOST_USER_NAME`, `SELFHOST_USER_EMAIL`).
#[derive(Debug, Clone)]
pub struct SelfHostSeed {
    pub external_id: String,
    pub name: String,
    pub email: Option<String>,
}

impl Default for SelfHostSeed {
    /// The historical fallbacks `default_self_host_external_id` /
    /// `_ensure_default_user` use when no config is reachable.
    fn default() -> Self {
        SelfHostSeed {
            external_id: "selfhost_default_user".to_string(),
            name: "Me".to_string(),
            email: None,
        }
    }
}

impl From<&Config> for SelfHostSeed {
    fn from(cfg: &Config) -> Self {
        SelfHostSeed {
            external_id: cfg.default_self_host_id.clone(),
            name: cfg.selfhost_user_name.clone(),
            email: cfg.selfhost_user_email.clone(),
        }
    }
}

// --- DDL strings (byte-identical to the Python triple-quoted literals) ------
//
// SQLite stores the CREATE statement text in `sqlite_master.sql` with the
// interior whitespace preserved, so these must not be reformatted.

const CREATE_USERS_TABLE_SQL: &str = "
            CREATE TABLE IF NOT EXISTS users (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                google_id TEXT UNIQUE NOT NULL,
                email TEXT NOT NULL,
                name TEXT NOT NULL,
                avatar_url TEXT,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                last_login TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                auth_provider TEXT,
                external_id TEXT,
                password_hash TEXT,
                theme_preference TEXT
            )
            ";

const CREATE_MOOD_ENTRIES_TABLE_SQL: &str = "
            CREATE TABLE IF NOT EXISTS mood_entries (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL,
                date TEXT NOT NULL,
                mood INTEGER NOT NULL CHECK (mood >= 1 AND mood <= 5),
                content TEXT NOT NULL,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE
            )
            ";

/// `DatabaseSchemaMixin.GROUPS_TABLE_DDL`.
pub const GROUPS_TABLE_DDL: &str = "
        CREATE TABLE IF NOT EXISTS groups (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER,
            name TEXT NOT NULL,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE
        )
    ";

const CREATE_GROUPS_MIGRATION_NEW_SQL: &str = "
                        CREATE TABLE groups_migration_new (
                            id INTEGER PRIMARY KEY AUTOINCREMENT,
                            user_id INTEGER,
                            name TEXT NOT NULL,
                            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                            FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE
                        )
                        ";

const CREATE_GROUP_OPTIONS_TABLE_SQL: &str = "
            CREATE TABLE IF NOT EXISTS group_options (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                group_id INTEGER NOT NULL,
                name TEXT NOT NULL,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (group_id) REFERENCES groups (id) ON DELETE CASCADE
            )
            ";

const CREATE_ENTRY_SELECTIONS_TABLE_SQL: &str = "
            CREATE TABLE IF NOT EXISTS entry_selections (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                entry_id INTEGER NOT NULL,
                option_id INTEGER NOT NULL,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (entry_id) REFERENCES mood_entries (id) ON DELETE CASCADE,
                FOREIGN KEY (option_id) REFERENCES group_options (id) ON DELETE CASCADE
            )
            ";

const CREATE_ACHIEVEMENTS_TABLE_SQL: &str = "
            CREATE TABLE IF NOT EXISTS achievements (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                user_id INTEGER NOT NULL,
                achievement_type TEXT NOT NULL,
                earned_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                nft_minted BOOLEAN DEFAULT FALSE,
                nft_token_id INTEGER,
                nft_tx_hash TEXT,
                FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE,
                UNIQUE(user_id, achievement_type)
            )
            ";

const CREATE_GOALS_TABLE_SQL: &str = "
                CREATE TABLE IF NOT EXISTS goals (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    user_id INTEGER NOT NULL,
                    title TEXT NOT NULL,
                    description TEXT,
                    frequency_per_week INTEGER NOT NULL CHECK (frequency_per_week >= 1 AND frequency_per_week <= 7),
                    completed INTEGER NOT NULL DEFAULT 0,
                    streak INTEGER NOT NULL DEFAULT 0,
                    period_start TEXT,
                    last_completed_date TEXT,
                    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE
                )
                ";

const CREATE_GOAL_COMPLETIONS_TABLE_SQL: &str = "
                CREATE TABLE IF NOT EXISTS goal_completions (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    user_id INTEGER NOT NULL,
                    goal_id INTEGER NOT NULL,
                    date TEXT NOT NULL,
                    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE,
                    FOREIGN KEY (goal_id) REFERENCES goals (id) ON DELETE CASCADE,
                    UNIQUE(user_id, goal_id, date)
                )
                ";

/// contract change: `last_view_date` (ISO `YYYY-MM-DD`) tracks the
/// last day a statistics view was counted, making `stats_views` per-day
/// idempotent. Declared after `updated_at` so a freshly created table has
/// the same column order as one migrated via ALTER TABLE ADD COLUMN.
const CREATE_USER_METRICS_TABLE_SQL: &str = "
                CREATE TABLE IF NOT EXISTS user_metrics (
                    user_id INTEGER PRIMARY KEY,
                    stats_views INTEGER NOT NULL DEFAULT 0,
                    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    last_view_date TEXT,
                    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE
                )
                ";

const CREATE_ACTIVITY_LOG_TABLE_SQL: &str = "
                CREATE TABLE IF NOT EXISTS activity_log (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    user_id INTEGER NOT NULL,
                    event_type TEXT NOT NULL,
                    metadata TEXT,
                    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE
                )
                ";

// --- Entry point -------------------------------------------------------------

/// Port of `DatabaseSchemaMixin.init_database`. Runs at startup, before the
/// pool serves queries. Safe to run on version-0 files from any historical
/// build, and idempotent.
pub fn bootstrap(db_path: &str, seed: &SelfHostSeed) -> Result<(), DatabaseError> {
    match bootstrap_inner(db_path, seed) {
        Ok(()) => Ok(()),
        Err(exc) => {
            tracing::error!("Database initialization failed: {exc}");
            Err(exc)
        }
    }
}

fn bootstrap_inner(db_path: &str, seed: &SelfHostSeed) -> Result<(), DatabaseError> {
    tracing::info!("Initializing database at: {db_path}");
    ensure_parent_dir(db_path)?;

    // Plain connection, exactly like Python's `sqlite3.connect(self.db_path)`
    // in init_database: PRAGMA foreign_keys stays at SQLite's default (OFF),
    // which the groups rebuild requires. Do NOT reuse the pool here — pooled
    // connections turn foreign keys ON.
    let conn = Connection::open(db_path)?;
    tracing::info!("Database connection successful. Creating tables...");

    // Whether every "non-critical" (swallow-and-log) step below actually
    // succeeded. The swallowing itself is bug-compatible with the Flask
    // bootstrap — partially-migrated DBs must still boot — but the
    // user_version stamp at the end is Rust-only and must not assert a
    // completion this run did not achieve.
    let mut clean = true;

    // Core tables
    clean &= create_users_table(&conn, seed)?;

    // The seeded self-host user must exist before the groups migration so
    // pre-existing rows can be backfilled to it.
    let default_user_id = ensure_default_user(&conn, seed);
    clean &= default_user_id.is_some();

    create_mood_entries_table(&conn)?;
    clean &= create_groups_table(&conn, default_user_id)?;
    create_group_options_table(&conn)?;
    create_entry_selections_table(&conn)?;
    create_achievements_table(&conn)?;

    // Goals and metrics
    clean &= create_goals_table(&conn);
    clean &= create_goal_completions_table(&conn);
    clean &= create_user_metrics_table(&conn);

    // Activity feed
    clean &= create_activity_log_table(&conn);

    // Shared indexes
    clean &= create_database_indexes(&conn);

    tracing::info!("Database initialization complete");
    drop(conn);

    insert_default_groups(db_path, seed)?;

    // Post-cutover markers: version 1 is stamped only after the legacy
    // (version-less) bootstrap provably ran to completion — if any swallowed
    // step failed (locked DB, leftover rebuild relic, ...), user_version is
    // left untouched (0), exactly what the Flask binary leaves behind, so
    // the next startup re-runs the legacy bootstrap and can self-heal. The
    // Python never reads or writes user_version, so a rollback to the Flask
    // binary is unaffected; future Rust-only schema changes go through
    // refinery keyed off this.
    {
        let conn = connect(db_path)?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if clean {
            if version < 1 {
                conn.pragma_update(None, "user_version", 1)?;
            }
        } else {
            tracing::warn!(
                "user_version stamp skipped: bootstrap swallowed at least one migration error; \
                 the next startup will re-run the legacy bootstrap"
            );
        }

        // Version-2 step (owner-approved contract change,
        // contract/DECISIONS.md "Post-cutover cleanup candidates" item #4):
        // normalize legacy US-format mood_entries.date values to ISO. Runs
        // whenever user_version < 2 (idempotent, purely row-value UPDATEs —
        // no schema change, sqlite_master stays byte-stable); the stamp
        // lands only when both the legacy bootstrap and this step
        // completed, so version 2 always implies version 1.
        let mut v2_complete = version >= 2;
        if version < 2 {
            let normalized = migrate_mood_entry_dates_to_iso(&conn);
            if clean && normalized {
                conn.pragma_update(None, "user_version", 2)?;
                v2_complete = true;
            } else {
                tracing::warn!(
                    "user_version=2 stamp skipped: mood entry date normalization did not \
                     provably complete; the next startup will retry"
                );
            }
        }

        // Version-3 step (owner-approved contract change, supersedes
        // contract/DECISIONS.md #9): statistics views become per-day
        // idempotent — guarded `ALTER TABLE user_metrics ADD COLUMN
        // last_view_date TEXT`, plus a one-shot `stats_views` reset
        // (owner-approved: old counters counted raw GETs, the new meaning
        // is "distinct view days"; earned achievements rows are untouched).
        // The reset runs only when the column is actually added, so a rerun
        // after a skipped stamp never re-zeroes freshly accrued view days.
        // The stamp lands only when every earlier step completed, so
        // version 3 always implies versions 1 and 2.
        if version < 3 {
            let migrated = migrate_user_metrics_daily_views(&conn);
            if clean && v2_complete && migrated {
                conn.pragma_update(None, "user_version", 3)?;
            } else {
                tracing::warn!(
                    "user_version=3 stamp skipped: user_metrics daily-view migration did not \
                     provably complete; the next startup will retry"
                );
            }
        }
    }

    Ok(())
}

// --- Table creation helpers --------------------------------------------------

/// Returns whether the (swallowed-error) users migration fully succeeded.
fn create_users_table(conn: &Connection, seed: &SelfHostSeed) -> Result<bool, DatabaseError> {
    // New columns are declared after last_login so a freshly created table
    // has the same column order as one migrated via ALTER TABLE ADD COLUMN.
    conn.execute(CREATE_USERS_TABLE_SQL, [])?;
    let migrated = migrate_users_table_schema(conn, seed);
    tracing::info!("Users table ready");
    Ok(migrated)
}

/// Port of `_migrate_users_table_schema`: add provider-scoped identity
/// columns and backfill legacy rows.
///
/// - `auth_provider`: 'local' for the seeded self-host user,
///   'legacy-google' for rows created by the old Google OAuth flow.
/// - `external_id`: the provider-scoped subject; backfilled from google_id.
///
/// The backfill only touches rows where the new columns are NULL, so
/// re-running is a no-op and values written by newer code are preserved.
///
/// Returns `true` when every step ran; `false` when an error was swallowed
/// (the caller must then not stamp `user_version`).
fn migrate_users_table_schema(conn: &Connection, seed: &SelfHostSeed) -> bool {
    fn run(conn: &Connection, seed: &SelfHostSeed) -> rusqlite::Result<()> {
        let cols = table_columns(conn, "users")?;
        if !cols.contains("auth_provider") {
            conn.execute("ALTER TABLE users ADD COLUMN auth_provider TEXT", [])?;
            tracing::info!("Users table migrated to include auth_provider");
        }
        if !cols.contains("external_id") {
            conn.execute("ALTER TABLE users ADD COLUMN external_id TEXT", [])?;
            tracing::info!("Users table migrated to include external_id");
        }
        if !cols.contains("password_hash") {
            conn.execute("ALTER TABLE users ADD COLUMN password_hash TEXT", [])?;
            tracing::info!("Users table migrated to include password_hash");
        }
        if !cols.contains("theme_preference") {
            // Per-user UI theme (default/light/dark/synthwave). NULL means
            // the client falls back to its default.
            conn.execute("ALTER TABLE users ADD COLUMN theme_preference TEXT", [])?;
            tracing::info!("Users table migrated to include theme_preference");
        }

        conn.execute(
            "UPDATE users SET external_id = google_id WHERE external_id IS NULL",
            [],
        )?;
        conn.execute(
            "
                UPDATE users
                   SET auth_provider = CASE
                           WHEN google_id = ? THEN 'local'
                           ELSE 'legacy-google'
                       END
                 WHERE auth_provider IS NULL
                ",
            [seed.external_id.as_str()],
        )?;
        conn.execute(
            concat!(
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_users_provider_external ",
                "ON users(auth_provider, external_id)"
            ),
            [],
        )?;
        Ok(())
    }
    match run(conn, seed) {
        Ok(()) => true,
        Err(exc) => {
            tracing::warn!("Users table migration failed (non-critical): {exc}");
            false
        }
    }
}

/// Port of `_ensure_default_user`: ensure the seeded self-host user exists;
/// return its id. Idempotent — an existing row (however it was created) is
/// reused, keyed by `google_id == DEFAULT_SELF_HOST_ID`.
fn ensure_default_user(conn: &Connection, seed: &SelfHostSeed) -> Option<i64> {
    fn run(conn: &Connection, seed: &SelfHostSeed) -> rusqlite::Result<Option<i64>> {
        let external_id = seed.external_id.as_str();
        if let Some(id) = conn
            .query_row(
                "SELECT id FROM users WHERE google_id = ?",
                [external_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
        {
            return Ok(Some(id));
        }

        let name = &seed.name;
        let email = seed
            .email
            .clone()
            .unwrap_or_else(|| format!("{external_id}@localhost"));

        conn.execute(
            "
                INSERT INTO users (google_id, email, name, auth_provider, external_id)
                VALUES (?, ?, ?, 'local', ?)
                ",
            rusqlite::params![external_id, email, name, external_id],
        )?;
        tracing::info!("Seeded default self-host user");
        let id = conn.last_insert_rowid();
        Ok((id != 0).then_some(id))
    }
    match run(conn, seed) {
        Ok(id) => id,
        Err(exc) => {
            tracing::warn!("Default user seeding failed (non-critical): {exc}");
            None
        }
    }
}

fn create_mood_entries_table(conn: &Connection) -> Result<(), DatabaseError> {
    conn.execute(CREATE_MOOD_ENTRIES_TABLE_SQL, [])?;
    tracing::info!("Mood entries table ready");
    Ok(())
}

/// Returns whether the (swallowed-error) groups migration and unique index
/// creation fully succeeded.
fn create_groups_table(
    conn: &Connection,
    default_user_id: Option<i64>,
) -> Result<bool, DatabaseError> {
    conn.execute(GROUPS_TABLE_DDL, [])?;
    let migrated = migrate_groups_table_schema(conn, default_user_id);
    let indexed = match conn.execute(
        concat!(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_groups_user_name ",
            "ON groups(user_id, name)"
        ),
        [],
    ) {
        Ok(_) => true,
        Err(exc) => {
            tracing::warn!("Groups unique index creation failed (non-critical): {exc}");
            false
        }
    };
    tracing::info!("Groups table ready");
    Ok(migrated && indexed)
}

/// Port of `_groups_table_needs_rebuild`: detect the legacy groups shape —
/// no `user_id` column, or a UNIQUE constraint solely on `name`. The old
/// inline UNIQUE(name) cannot be dropped in place, so either condition
/// means the table must be rebuilt.
pub(crate) fn groups_table_needs_rebuild(conn: &Connection) -> rusqlite::Result<bool> {
    let cols = table_columns(conn, "groups")?;
    if cols.is_empty() {
        return Ok(false); // table does not exist yet
    }
    if !cols.contains("user_id") {
        return Ok(true);
    }
    let indexes: Vec<(String, i64, String)> = conn
        .prepare("PRAGMA index_list(groups)")?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?, // name
                row.get::<_, i64>(2)?,    // unique
                row.get::<_, String>(3)?, // origin
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    for (index_name, is_unique, origin) in indexes {
        if is_unique == 0 || origin != "u" {
            continue;
        }
        // index_name comes from SQLite's own metadata, not user input;
        // PRAGMA does not accept bound parameters, so quote defensively.
        let safe_name = index_name.replace('"', "\"\"");
        let index_cols: Vec<String> = conn
            .prepare(&format!("PRAGMA index_info(\"{safe_name}\")"))?
            .query_map([], |row| row.get::<_, String>(2))?
            .collect::<rusqlite::Result<_>>()?;
        if index_cols == ["name"] {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Re-assert foreign_keys=OFF outside any transaction, and return the value
/// SQLite actually reports afterwards.
///
/// `PRAGMA foreign_keys` is a **no-op inside an open transaction** — if the
/// connection has already begun one, FK enforcement stays wherever it was
/// and `DROP TABLE groups` would cascade `group_options` away. Mirrors the
/// Python's `if conn.in_transaction: conn.commit()` guard; the caller must
/// verify the returned value is 0 before opening the rebuild transaction.
fn fk_off_for_rebuild(conn: &Connection) -> rusqlite::Result<i64> {
    if !conn.is_autocommit() {
        conn.execute_batch("COMMIT")?;
    }
    conn.execute_batch("PRAGMA foreign_keys=OFF")?;
    conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))
}

/// Port of `_migrate_groups_table_schema`: migrate groups to per-user
/// scoping, then backfill ownerless rows to the seeded self-host user.
/// Errors are swallowed and logged ("non-critical") like the Python —
/// partially-migrated DBs must still boot. Returns `true` when every step
/// ran; `false` when an error was swallowed (the caller must then not stamp
/// `user_version`).
fn migrate_groups_table_schema(conn: &Connection, default_user_id: Option<i64>) -> bool {
    fn run(conn: &Connection, default_user_id: Option<i64>) -> Result<(), DatabaseError> {
        if groups_table_needs_rebuild(conn)? {
            rebuild_groups_table(conn)?;
        }

        // Backfill ownerless rows to the seeded self-host user. Runs on
        // every init and only touches NULL rows, so it is idempotent.
        if let Some(user_id) = default_user_id {
            conn.execute(
                "UPDATE groups SET user_id = ? WHERE user_id IS NULL",
                [user_id],
            )?;
        }
        Ok(())
    }
    match run(conn, default_user_id) {
        Ok(()) => true,
        Err(exc) => {
            tracing::warn!("Groups table migration failed (non-critical): {exc}");
            false
        }
    }
}

/// The SQLite-documented create-new / copy / drop / rename sequence, in a
/// single explicit transaction with a row-count check and
/// `PRAGMA foreign_key_check(groups)` before COMMIT. Any failure rolls back
/// and leaves the old table untouched.
fn rebuild_groups_table(conn: &Connection) -> Result<(), DatabaseError> {
    let old_cols: HashSet<String> = table_columns(conn, "groups")?;
    let user_id_select = if old_cols.contains("user_id") {
        "user_id"
    } else {
        "NULL"
    };

    let fk = fk_off_for_rebuild(conn)?;
    if fk != 0 {
        // Never reached on this dedicated connection, but guarded hard:
        // proceeding with FK ON would cascade group_options on DROP.
        return Err(DatabaseError::Message(format!(
            "groups rebuild aborted: PRAGMA foreign_keys still reads {fk} \
             (pragma is a no-op inside an open transaction)"
        )));
    }
    conn.execute_batch("BEGIN IMMEDIATE")?;
    fn run(conn: &Connection, user_id_select: &str) -> Result<i64, DatabaseError> {
        let before_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM groups", [], |row| row.get(0))?;

        conn.execute(CREATE_GROUPS_MIGRATION_NEW_SQL, [])?;
        conn.execute(
            &format!(
                "
                        INSERT INTO groups_migration_new (id, user_id, name, created_at)
                        SELECT id, {user_id_select}, name, created_at FROM groups
                        "
            ),
            [],
        )?;
        let after_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM groups_migration_new", [], |row| {
                row.get(0)
            })?;
        if before_count != after_count {
            return Err(DatabaseError::Message(format!(
                "groups rebuild row count mismatch: {before_count} != {after_count}"
            )));
        }

        conn.execute("DROP TABLE groups", [])?;
        conn.execute("ALTER TABLE groups_migration_new RENAME TO groups", [])?;
        conn.execute(
            concat!(
                "CREATE UNIQUE INDEX idx_groups_user_name ",
                "ON groups(user_id, name)"
            ),
            [],
        )?;

        let violations: Vec<String> = conn
            .prepare("PRAGMA foreign_key_check(groups)")?
            .query_map([], |row| {
                Ok(format!(
                    "({:?}, {:?}, {:?}, {:?})",
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        if !violations.is_empty() {
            return Err(DatabaseError::Message(format!(
                "groups rebuild produced FK violations: {violations:?}"
            )));
        }
        Ok(after_count)
    }
    match run(conn, user_id_select) {
        Ok(after_count) => {
            conn.execute_batch("COMMIT")?;
            tracing::info!("Groups table rebuilt for per-user scoping ({after_count} rows)");
            Ok(())
        }
        Err(exc) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(exc)
        }
    }
}

fn create_group_options_table(conn: &Connection) -> Result<(), DatabaseError> {
    conn.execute(CREATE_GROUP_OPTIONS_TABLE_SQL, [])?;
    tracing::info!("Group options table ready");
    Ok(())
}

fn create_entry_selections_table(conn: &Connection) -> Result<(), DatabaseError> {
    conn.execute(CREATE_ENTRY_SELECTIONS_TABLE_SQL, [])?;
    tracing::info!("Entry selections table ready");
    Ok(())
}

fn create_achievements_table(conn: &Connection) -> Result<(), DatabaseError> {
    conn.execute(CREATE_ACHIEVEMENTS_TABLE_SQL, [])?;
    tracing::info!("Achievements table ready");
    Ok(())
}

/// Returns `true` when every (swallowed-error) step ran; `false` otherwise.
fn create_goals_table(conn: &Connection) -> bool {
    fn run(conn: &Connection) -> rusqlite::Result<bool> {
        conn.execute(CREATE_GOALS_TABLE_SQL, [])?;
        let migrated = migrate_goals_table_schema(conn);
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_goals_user ON goals(user_id)",
            [],
        )?;
        Ok(migrated)
    }
    match run(conn) {
        Ok(migrated) => {
            tracing::info!("Goals table ready");
            migrated
        }
        Err(exc) => {
            tracing::warn!("Goals table creation failed (non-critical): {exc}");
            false
        }
    }
}

/// Returns `true` when the migration ran; `false` when its error was swallowed.
fn migrate_goals_table_schema(conn: &Connection) -> bool {
    fn run(conn: &Connection) -> rusqlite::Result<()> {
        let cols = table_columns(conn, "goals")?;
        if !cols.contains("last_completed_date") {
            conn.execute("ALTER TABLE goals ADD COLUMN last_completed_date TEXT", [])?;
            tracing::info!("Goals table migrated to include last_completed_date");
        }
        Ok(())
    }
    match run(conn) {
        Ok(()) => true,
        Err(exc) => {
            tracing::warn!("Goals table migration failed (non-critical): {exc}");
            false
        }
    }
}

/// Returns `true` when creation ran; `false` when its error was swallowed.
fn create_goal_completions_table(conn: &Connection) -> bool {
    fn run(conn: &Connection) -> rusqlite::Result<()> {
        conn.execute(CREATE_GOAL_COMPLETIONS_TABLE_SQL, [])?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_goal_completions_user_goal ON goal_completions(user_id, goal_id)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_goal_completions_date ON goal_completions(date)",
            [],
        )?;
        Ok(())
    }
    match run(conn) {
        Ok(()) => {
            tracing::info!("Goal completions table ready");
            true
        }
        Err(exc) => {
            tracing::warn!("Goal completions table creation failed (non-critical): {exc}");
            false
        }
    }
}

/// Returns `true` when creation ran; `false` when its error was swallowed.
fn create_user_metrics_table(conn: &Connection) -> bool {
    match conn.execute(CREATE_USER_METRICS_TABLE_SQL, []) {
        Ok(_) => {
            tracing::info!("User metrics table ready");
            true
        }
        Err(exc) => {
            tracing::warn!("User metrics table creation failed (non-critical): {exc}");
            false
        }
    }
}

/// Returns `true` when creation ran; `false` when its error was swallowed.
fn create_activity_log_table(conn: &Connection) -> bool {
    fn run(conn: &Connection) -> rusqlite::Result<()> {
        conn.execute(CREATE_ACTIVITY_LOG_TABLE_SQL, [])?;
        conn.execute(
            concat!(
                "CREATE INDEX IF NOT EXISTS idx_activity_log_user_created ",
                "ON activity_log(user_id, created_at DESC)"
            ),
            [],
        )?;
        Ok(())
    }
    match run(conn) {
        Ok(()) => {
            tracing::info!("Activity log table ready");
            true
        }
        Err(exc) => {
            tracing::warn!("Activity log table creation failed (non-critical): {exc}");
            false
        }
    }
}

/// Returns `true` when creation ran; `false` when its error was swallowed.
fn create_database_indexes(conn: &Connection) -> bool {
    fn run(conn: &Connection) -> rusqlite::Result<()> {
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_mood_entries_date ON mood_entries(date)",
            [],
        )?;
        conn.execute(
            concat!(
                "CREATE INDEX IF NOT EXISTS idx_mood_entries_user_date ",
                "ON mood_entries(user_id, date)"
            ),
            [],
        )?;
        Ok(())
    }
    match run(conn) {
        Ok(()) => {
            tracing::info!("Mood entries indexes ready");
            true
        }
        Err(exc) => {
            tracing::warn!("Index creation failed (non-critical): {exc}");
            false
        }
    }
}

// --- Version-2 migration -----------------------------------------------------

/// Strict CPython `datetime.strptime(value, '%m/%d/%Y')` equivalent — the
/// exact pattern the Python streak parser accepted: month/day are 1-2
/// digits, the year exactly 4 digits, and impossible calendar dates fail.
/// Anything else (ISO dates, `2025/08/01`, two-digit years, whitespace,
/// garbage) returns `None`.
fn parse_us_date(value: &str) -> Option<chrono::NaiveDate> {
    let mut parts = value.split('/');
    let (month_str, day_str, year_str) = (parts.next()?, parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    if year_str.len() != 4 || !year_str.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let field = |text: &str| -> Option<u32> {
        if text.is_empty() || text.len() > 2 || !text.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        text.parse().ok()
    };
    let year: i32 = year_str.parse().ok()?;
    chrono::NaiveDate::from_ymd_opt(year, field(month_str)?, field(day_str)?)
}

/// Version-2 migration (Rust-only; contract change): rewrite every
/// `mood_entries.date` matching `%m/%d/%Y` (padded or unpadded) to ISO
/// `YYYY-MM-DD`. Everything else — ISO dates (padded or not) and garbage
/// the streak parser also skipped — is left untouched. Purely row-value
/// UPDATEs: no tables, no schema change. Idempotent (a normalized row no
/// longer matches). Same swallow-and-log resilience as the other steps;
/// returns `true` when the pass provably completed (the caller must not
/// stamp `user_version = 2` otherwise).
fn migrate_mood_entry_dates_to_iso(conn: &Connection) -> bool {
    fn run(conn: &Connection) -> rusqlite::Result<usize> {
        let candidates: Vec<(i64, String)> = conn
            .prepare("SELECT id, date FROM mood_entries WHERE date LIKE '%/%'")?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut normalized = 0;
        for (id, date) in candidates {
            // Garbage stays untouched, exactly like the streak parser
            // silently skips it.
            let Some(day) = parse_us_date(&date) else {
                continue;
            };
            conn.execute(
                "UPDATE mood_entries SET date = ? WHERE id = ?",
                rusqlite::params![day.format("%Y-%m-%d").to_string(), id],
            )?;
            normalized += 1;
        }
        Ok(normalized)
    }
    match run(conn) {
        Ok(0) => true,
        Ok(count) => {
            tracing::info!("Mood entry dates normalized to ISO ({count} rows)");
            true
        }
        Err(exc) => {
            tracing::warn!("Mood entry date normalization failed (non-critical): {exc}");
            false
        }
    }
}

// --- Version-3 migration -----------------------------------------------------

/// Version-3 migration (Rust-only; contract change): add
/// `user_metrics.last_view_date` and reset `stats_views` to 0 — but only
/// when the column is actually being added (first run), so the reset can
/// never fire twice. Freshly created tables already carry the column via
/// [`CREATE_USER_METRICS_TABLE_SQL`] and are left alone. Same
/// swallow-and-log resilience as the other steps; returns `true` when the
/// pass provably completed (the caller must not stamp `user_version = 3`
/// otherwise).
fn migrate_user_metrics_daily_views(conn: &Connection) -> bool {
    fn run(conn: &Connection) -> rusqlite::Result<()> {
        let cols = table_columns(conn, "user_metrics")?;
        if !cols.contains("last_view_date") {
            conn.execute(
                "ALTER TABLE user_metrics ADD COLUMN last_view_date TEXT",
                [],
            )?;
            conn.execute("UPDATE user_metrics SET stats_views = 0", [])?;
            tracing::info!("User metrics migrated to per-day statistics views (stats_views reset)");
        }
        Ok(())
    }
    match run(conn) {
        Ok(()) => true,
        Err(exc) => {
            tracing::warn!("User metrics daily-view migration failed (non-critical): {exc}");
            false
        }
    }
}

// --- Seed helpers ------------------------------------------------------------

/// Port of `ensure_default_groups_for_user`: ensure the default tag groups
/// exist for the given user. Idempotent per user; options are only inserted
/// when the group row itself is inserted.
pub fn ensure_default_groups_for_user(db_path: &str, user_id: i64) -> Result<(), DatabaseError> {
    let conn = connect(db_path)?;
    for (group_name, options) in DEFAULT_GROUPS {
        let group_row: Option<i64> = conn
            .query_row(
                "SELECT id FROM groups WHERE name = ? AND user_id = ?",
                rusqlite::params![group_name, user_id],
                |row| row.get(0),
            )
            .optional()?;

        if group_row.is_none() {
            conn.execute(
                "INSERT INTO groups (name, user_id) VALUES (?, ?)",
                rusqlite::params![group_name, user_id],
            )?;
            let group_id = conn.last_insert_rowid();
            for option in *options {
                conn.execute(
                    "INSERT INTO group_options (group_id, name) VALUES (?, ?)",
                    rusqlite::params![group_id, option],
                )?;
            }
        }
    }
    tracing::info!("Default groups ensured for user {user_id}");
    Ok(())
}

/// Port of `_insert_default_groups`.
fn insert_default_groups(db_path: &str, seed: &SelfHostSeed) -> Result<(), DatabaseError> {
    let default_user_id = {
        let conn = connect(db_path)?;
        get_default_user_id(&conn, &seed.external_id)?
    };
    let Some(user_id) = default_user_id else {
        tracing::warn!("Default groups not seeded: default self-host user missing");
        return Ok(());
    };
    ensure_default_groups_for_user(db_path, user_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal pre-Phase-2a groups shape with dependents, enough to
    /// exercise the rebuild path.
    fn build_legacy_groups_db(conn: &Connection) {
        conn.execute_batch(
            "
            CREATE TABLE users (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                google_id TEXT UNIQUE NOT NULL,
                email TEXT NOT NULL,
                name TEXT NOT NULL,
                avatar_url TEXT,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                last_login TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            );
            CREATE TABLE groups (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL UNIQUE,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            );
            CREATE TABLE group_options (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                group_id INTEGER NOT NULL,
                name TEXT NOT NULL,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (group_id) REFERENCES groups (id) ON DELETE CASCADE
            );
            INSERT INTO users (google_id, email, name) VALUES ('selfhost_default_user', 'me@localhost', 'Me');
            INSERT INTO groups (name) VALUES ('Emotions');
            INSERT INTO groups (name) VALUES ('Workout');
            INSERT INTO group_options (group_id, name) VALUES (1, 'happy');
            INSERT INTO group_options (group_id, name) VALUES (2, 'gym');
            ",
        )
        .unwrap();
    }

    /// The pragma-ordering invariant the whole rebuild hangs on: `PRAGMA
    /// foreign_keys=OFF` must actually take effect (read back as 0) before
    /// the rebuild transaction opens — even if the connection previously
    /// had FK ON and an open transaction (the pragma is a no-op inside one).
    #[test]
    fn fk_pragma_reads_zero_before_rebuild_transaction_opens() {
        let dir = tempfile::tempdir().unwrap();
        let conn = Connection::open(dir.path().join("t.db")).unwrap();
        build_legacy_groups_db(&conn);

        // Simulate the worst case: a pool-style connection with FK ON and
        // an already-open transaction.
        conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        conn.execute_batch("BEGIN").unwrap();
        conn.execute("INSERT INTO groups (name) VALUES ('Pending')", [])
            .unwrap();
        assert!(
            !conn.is_autocommit(),
            "test setup: transaction must be open"
        );

        let fk = fk_off_for_rebuild(&conn).unwrap();

        // Inside the rebuild path, before BEGIN IMMEDIATE: FK reads 0 and
        // no transaction is open (rebuild_groups_table refuses otherwise).
        assert_eq!(fk, 0, "PRAGMA foreign_keys must read 0 before the rebuild");
        assert!(
            conn.is_autocommit(),
            "no transaction may be open when the pragma is asserted"
        );
    }

    /// End-to-end proof that FK OFF took effect: with FK enforcement still
    /// ON, `DROP TABLE groups` would cascade-delete group_options.
    #[test]
    fn groups_rebuild_preserves_group_options_even_with_fk_on_connection() {
        let dir = tempfile::tempdir().unwrap();
        let conn = Connection::open(dir.path().join("t.db")).unwrap();
        build_legacy_groups_db(&conn);
        conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();

        assert!(groups_table_needs_rebuild(&conn).unwrap());
        assert!(
            migrate_groups_table_schema(&conn, Some(1)),
            "clean rebuild must report success"
        );

        let option_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM group_options", [], |row| row.get(0))
            .unwrap();
        assert_eq!(option_count, 2, "group_options must survive the rebuild");

        let group_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM groups", [], |row| row.get(0))
            .unwrap();
        assert_eq!(group_count, 2);

        let owners: Vec<Option<i64>> = conn
            .prepare("SELECT DISTINCT user_id FROM groups")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(owners, vec![Some(1)], "ownerless rows backfilled to user 1");

        assert!(
            !groups_table_needs_rebuild(&conn).unwrap(),
            "rebuild must be a one-shot migration"
        );
    }

    /// Every SQLQueries constant must prepare against the bootstrapped
    /// schema (catches column/table drift in the verbatim strings).
    #[test]
    fn sql_queries_prepare_against_bootstrapped_schema() {
        use crate::db::common::sql_queries as q;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fresh.db");
        bootstrap(path.to_str().unwrap(), &SelfHostSeed::default()).unwrap();
        let conn = Connection::open(&path).unwrap();
        for sql in [
            q::CREATE_USER,
            q::GET_USER_BY_GOOGLE_ID,
            q::GET_USER_BY_ID,
            q::GET_USER_BY_PROVIDER,
            q::CREATE_PROVIDER_USER,
            q::UPSERT_USER,
            q::GET_GOALS_BY_USER,
            q::GET_GOAL_BY_ID,
            q::GET_USER_ENTRY_DATES,
            q::GET_MOOD_STATISTICS,
        ] {
            conn.prepare(sql)
                .unwrap_or_else(|exc| panic!("constant failed to prepare: {exc}\n{sql}"));
        }
    }

    /// Fresh bootstrap leaves the journal mode alone and stamps
    /// user_version = 3 (v1 legacy bootstrap + v2 date normalization + v3
    /// per-day statistics views) only after completing.
    #[test]
    fn bootstrap_keeps_journal_mode_and_stamps_user_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fresh.db");
        bootstrap(path.to_str().unwrap(), &SelfHostSeed::default()).unwrap();
        let conn = Connection::open(&path).unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            mode, "delete",
            "bootstrap's raw connection must not switch journal mode \
             (WAL is opted into by the pool, after bootstrap)"
        );
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 3);
    }

    fn user_version(path: &std::path::Path) -> i64 {
        Connection::open(path)
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap()
    }

    /// Regression (parity finding "user_version=1 stamped despite silently
    /// failed/incomplete migrations", repro A): a leftover
    /// `groups_migration_new` relic makes the groups rebuild fail; the
    /// failure is swallowed (bootstrap still returns Ok, like Flask) but the
    /// stamp must be skipped so the next startup retries — and once the
    /// relic is gone, the rebuild completes and the stamp lands.
    #[test]
    fn stamp_skipped_when_groups_rebuild_swallows_error_then_self_heals() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("relic.db");
        let path_str = path.to_str().unwrap();
        {
            let conn = Connection::open(&path).unwrap();
            // groups has user_id but still carries the legacy inline
            // UNIQUE(name) -> needs rebuild; the relic table aborts it.
            conn.execute_batch(
                "
                CREATE TABLE users (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    google_id TEXT UNIQUE NOT NULL,
                    email TEXT NOT NULL,
                    name TEXT NOT NULL,
                    avatar_url TEXT,
                    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    last_login TIMESTAMP DEFAULT CURRENT_TIMESTAMP
                );
                INSERT INTO users (google_id, email, name)
                    VALUES ('selfhost_default_user', 'me@localhost', 'Me');
                CREATE TABLE groups (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    user_id INTEGER,
                    name TEXT NOT NULL UNIQUE,
                    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
                );
                INSERT INTO groups (user_id, name) VALUES (1, 'Emotions');
                INSERT INTO groups (user_id, name) VALUES (1, 'Sleep');
                INSERT INTO groups (user_id, name) VALUES (1, 'Productivity');
                CREATE TABLE groups_migration_new (id INTEGER PRIMARY KEY);
                ",
            )
            .unwrap();
        }

        // Bug-compatible: the swallowed failure must not fail the bootstrap.
        bootstrap(path_str, &SelfHostSeed::default()).unwrap();
        assert_eq!(
            user_version(&path),
            0,
            "must not stamp while the legacy UNIQUE(name) shape survives"
        );
        {
            let conn = Connection::open(&path).unwrap();
            assert!(
                groups_table_needs_rebuild(&conn).unwrap(),
                "test premise: the rebuild really was skipped"
            );
        }

        // Operator (or a crashed run's cleanup) removes the relic: the next
        // startup completes the migration and only then stamps.
        Connection::open(&path)
            .unwrap()
            .execute_batch("DROP TABLE groups_migration_new")
            .unwrap();
        bootstrap(path_str, &SelfHostSeed::default()).unwrap();
        assert_eq!(user_version(&path), 3);
        let conn = Connection::open(&path).unwrap();
        assert!(!groups_table_needs_rebuild(&conn).unwrap());
    }

    /// Regression (same finding, repro B analogue): when the users-table
    /// migration fails mid-way (here simulated with an aborting trigger, in
    /// the wild a writer lock held by the Flask process), bootstrap still
    /// returns Ok but must not stamp; a later run with the obstacle gone
    /// finishes the migration and stamps.
    #[test]
    fn stamp_skipped_when_users_migration_swallows_error_then_self_heals() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blocked-users.db");
        let path_str = path.to_str().unwrap();
        {
            let conn = Connection::open(&path).unwrap();
            build_legacy_groups_db(&conn);
            conn.execute_batch(
                "CREATE TRIGGER users_update_blocker BEFORE UPDATE ON users
                 BEGIN SELECT RAISE(ABORT, 'simulated mid-migration failure'); END;",
            )
            .unwrap();
        }

        bootstrap(path_str, &SelfHostSeed::default()).unwrap();
        assert_eq!(
            user_version(&path),
            0,
            "must not stamp a half-migrated users table"
        );
        {
            let conn = Connection::open(&path).unwrap();
            let backfilled: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM users WHERE auth_provider IS NOT NULL",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                backfilled, 0,
                "test premise: the backfill really was blocked"
            );
        }

        Connection::open(&path)
            .unwrap()
            .execute_batch("DROP TRIGGER users_update_blocker")
            .unwrap();
        bootstrap(path_str, &SelfHostSeed::default()).unwrap();
        assert_eq!(user_version(&path), 3);
        let conn = Connection::open(&path).unwrap();
        let unfilled: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM users WHERE auth_provider IS NULL OR external_id IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unfilled, 0, "second run must complete the users backfill");
    }

    /// contract change, version-2 step: US-format (`%m/%d/%Y`) dates
    /// are normalized to ISO; ISO and garbage rows stay byte-identical; the
    /// step is idempotent and (with the later steps) stamps user_version 3.
    #[test]
    fn v2_migration_normalizes_us_dates_and_leaves_everything_else_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v2.db");
        let path_str = path.to_str().unwrap();
        bootstrap(path_str, &SelfHostSeed::default()).unwrap();
        assert_eq!(user_version(&path), 3);

        // Simulate a pre-v2 file: seed mixed rows, roll the marker back.
        let seeds: &[(&str, i64)] = &[
            ("8/2/2025", 2),   // unpadded US -> 2025-08-02
            ("08/02/2025", 3), // padded US -> 2025-08-02
            ("12/31/2024", 4), // US -> 2024-12-31
            ("2025-08-01", 4), // ISO: untouched
            ("2025-8-1", 3),   // unpadded ISO: NOT %m/%d/%Y, untouched
            ("not-a-date", 1), // garbage: untouched
            ("13/45/2025", 2), // impossible month/day: untouched
            ("2/29/2025", 2),  // impossible calendar date: untouched
            ("8/2/25", 2),     // two-digit year: untouched
            ("2025/08/01", 2), // year-first slashes: untouched
        ];
        {
            let conn = Connection::open(&path).unwrap();
            for (date, mood) in seeds {
                conn.execute(
                    "INSERT INTO mood_entries (user_id, date, mood, content) VALUES (1, ?, ?, 'x')",
                    rusqlite::params![date, mood],
                )
                .unwrap();
            }
            conn.pragma_update(None, "user_version", 1).unwrap();
        }

        bootstrap(path_str, &SelfHostSeed::default()).unwrap();
        assert_eq!(user_version(&path), 3);

        let dates = |conn: &Connection| -> Vec<String> {
            conn.prepare("SELECT date FROM mood_entries ORDER BY id")
                .unwrap()
                .query_map([], |row| row.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        let conn = Connection::open(&path).unwrap();
        let expected = vec![
            "2025-08-02".to_string(),
            "2025-08-02".to_string(),
            "2024-12-31".to_string(),
            "2025-08-01".to_string(),
            "2025-8-1".to_string(),
            "not-a-date".to_string(),
            "13/45/2025".to_string(),
            "2/29/2025".to_string(),
            "8/2/25".to_string(),
            "2025/08/01".to_string(),
        ];
        assert_eq!(dates(&conn), expected);
        drop(conn);

        // Idempotent: a third bootstrap changes nothing.
        bootstrap(path_str, &SelfHostSeed::default()).unwrap();
        let conn = Connection::open(&path).unwrap();
        assert_eq!(dates(&conn), expected);
        assert_eq!(user_version(&path), 3);
    }

    /// contract change, version-3 step: a v2-stamped DB gains
    /// `user_metrics.last_view_date`, its `stats_views` counters are reset
    /// to 0 exactly once (achievements rows untouched), and reruns never
    /// re-zero views accrued after the migration.
    #[test]
    fn v3_migration_adds_last_view_date_and_resets_counters_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v3.db");
        let path_str = path.to_str().unwrap();
        bootstrap(path_str, &SelfHostSeed::default()).unwrap();
        assert_eq!(user_version(&path), 3);

        // Simulate a pre-v3 file: drop the column (SQLite 3.35+), seed a
        // legacy counter and an earned achievement, roll the marker back.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "
                ALTER TABLE user_metrics DROP COLUMN last_view_date;
                INSERT INTO user_metrics (user_id, stats_views) VALUES (1, 37);
                INSERT INTO achievements (user_id, achievement_type) VALUES (1, 'data_lover');
                PRAGMA user_version = 2;
                ",
            )
            .unwrap();
        }

        bootstrap(path_str, &SelfHostSeed::default()).unwrap();
        assert_eq!(user_version(&path), 3);
        {
            let conn = Connection::open(&path).unwrap();
            let (views, last_view): (i64, Option<String>) = conn
                .query_row(
                    "SELECT stats_views, last_view_date FROM user_metrics WHERE user_id = 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(views, 0, "legacy raw-GET counter must reset to 0");
            assert_eq!(last_view, None);
            // Earned achievements are kept.
            let achievements: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM achievements WHERE user_id = 1 \
                     AND achievement_type = 'data_lover'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(achievements, 1);
            // Accrue a post-migration view day...
            conn.execute(
                "UPDATE user_metrics SET stats_views = 2, last_view_date = '2026-08-15' \
                 WHERE user_id = 1",
                [],
            )
            .unwrap();
        }

        // ...and a rerun must NOT re-zero it (reset is one-shot, keyed on
        // the column addition).
        bootstrap(path_str, &SelfHostSeed::default()).unwrap();
        assert_eq!(user_version(&path), 3);
        let conn = Connection::open(&path).unwrap();
        let views: i64 = conn
            .query_row(
                "SELECT stats_views FROM user_metrics WHERE user_id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(views, 2, "rerun must not reset post-migration view days");
    }

    /// Statistics correctness on formerly-US-format data: after the v2
    /// migration the verbatim lexicographic MIN/MAX (first/last_entry_date)
    /// and the raw-string BETWEEN range filter become chronologically
    /// correct with no SQL change.
    #[test]
    fn v2_migration_makes_stats_and_range_filter_correct_on_us_format_data() {
        use crate::db::{achievements, moods};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("v2-stats.db");
        let path_str = path.to_str().unwrap();
        bootstrap(path_str, &SelfHostSeed::default()).unwrap();
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "
                INSERT INTO mood_entries (user_id, date, mood, content) VALUES (1, '2025-08-01', 4, 'a');
                INSERT INTO mood_entries (user_id, date, mood, content) VALUES (1, '8/2/2025', 2, 'b');
                INSERT INTO mood_entries (user_id, date, mood, content) VALUES (1, '2025-08-03', 5, 'c');
                PRAGMA user_version = 1;
                ",
            )
            .unwrap();
        }

        // Pre-migration quirk (pinned by the pre-rewrite fixtures): the US row
        // is MAX() lexicographically and outside the ISO BETWEEN range.
        {
            let conn = Connection::open(&path).unwrap();
            let stats = achievements::get_mood_statistics(&conn, 1).unwrap();
            assert_eq!(stats.last_entry_date.as_deref(), Some("8/2/2025"));
            let range = moods::get_mood_entries_by_date_range(&conn, 1, "2025-08-01", "2025-08-31")
                .unwrap();
            assert_eq!(range.len(), 2, "US row excluded before the migration");
        }

        bootstrap(path_str, &SelfHostSeed::default()).unwrap();

        let conn = Connection::open(&path).unwrap();
        let stats = achievements::get_mood_statistics(&conn, 1).unwrap();
        assert_eq!(stats.total_entries, 3);
        assert_eq!(stats.first_entry_date.as_deref(), Some("2025-08-01"));
        assert_eq!(stats.last_entry_date.as_deref(), Some("2025-08-03"));

        let range =
            moods::get_mood_entries_by_date_range(&conn, 1, "2025-08-01", "2025-08-31").unwrap();
        let range_dates: Vec<&str> = range.iter().map(|entry| entry.date.as_str()).collect();
        assert_eq!(
            range_dates,
            vec!["2025-08-03", "2025-08-02", "2025-08-01"],
            "formerly-US row now inside the ISO range, normalized-day DESC order"
        );
    }
}
