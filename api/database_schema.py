"""Database schema helpers for Nightlio."""

from __future__ import annotations

import sqlite3
from typing import Iterable

try:  # pragma: no cover - allow module to run outside package context
    from .database_common import (
        DatabaseConnectionMixin,
        default_self_host_external_id,
        logger,
    )
except ImportError:  # pragma: no cover - fallback for scripts
    from database_common import (  # type: ignore
        DatabaseConnectionMixin,
        default_self_host_external_id,
        logger,
    )

# Default tag groups seeded for a user on first initialization.
DEFAULT_GROUPS = {
    "Emotions": [
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
    "Sleep": [
        "well-rested",
        "refreshed",
        "tired",
        "exhausted",
        "restless",
        "insomniac",
    ],
    "Productivity": [
        "focused",
        "motivated",
        "accomplished",
        "busy",
        "distracted",
        "procrastinating",
        "overwhelmed",
        "lazy",
    ],
}


class DatabaseSchemaMixin(DatabaseConnectionMixin):
    """Provides table creation and bootstrap helpers."""

    def init_database(self) -> None:
        """Initialize the database with required tables and seed data."""
        try:
            logger.info("Initializing database at: %s", self.db_path)
            with sqlite3.connect(self.db_path) as conn:
                logger.info("Database connection successful. Creating tables...")

                # Core tables
                self._create_users_table(conn)

                # The seeded self-host user must exist before the groups
                # migration so pre-existing rows can be backfilled to it.
                default_user_id = self._ensure_default_user(conn)

                self._create_mood_entries_table(conn)
                self._create_groups_table(conn, default_user_id)
                self._create_group_options_table(conn)
                self._create_entry_selections_table(conn)
                self._create_achievements_table(conn)

                # Goals and metrics
                self._create_goals_table(conn)
                self._create_goal_completions_table(conn)
                self._create_user_metrics_table(conn)

                # Activity feed
                self._create_activity_log_table(conn)

                # Shared indexes
                self._create_database_indexes(conn)

                conn.commit()
                logger.info("Database initialization complete")

            self._insert_default_groups()
        except Exception as exc:  # pragma: no cover - initialization rarely fails
            logger.error("Database initialization failed: %s", exc)
            raise

    # --- Table creation helpers -------------------------------------------------
    def _create_users_table(self, conn: sqlite3.Connection) -> None:
        # New columns are declared after last_login so a freshly created table
        # has the same column order as one migrated via ALTER TABLE ADD COLUMN.
        conn.execute(
            """
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
            """
        )
        self._migrate_users_table_schema(conn)
        logger.info("Users table ready")

    def _migrate_users_table_schema(self, conn: sqlite3.Connection) -> None:
        """Add provider-scoped identity columns and backfill legacy rows.

        - auth_provider: 'local' for the seeded self-host user,
          'legacy-google' for rows created by the old Google OAuth flow.
        - external_id: the provider-scoped subject; backfilled from google_id.
        - password_hash: nullable, used by local auth (Phase 2b).

        The backfill only touches rows where the new columns are NULL, so
        re-running is a no-op and values written by newer code are preserved.
        """
        try:
            cur = conn.execute("PRAGMA table_info(users)")
            cols: Iterable[str] = {row[1] for row in cur.fetchall()}
            if "auth_provider" not in cols:
                conn.execute("ALTER TABLE users ADD COLUMN auth_provider TEXT")
                logger.info("Users table migrated to include auth_provider")
            if "external_id" not in cols:
                conn.execute("ALTER TABLE users ADD COLUMN external_id TEXT")
                logger.info("Users table migrated to include external_id")
            if "password_hash" not in cols:
                conn.execute("ALTER TABLE users ADD COLUMN password_hash TEXT")
                logger.info("Users table migrated to include password_hash")
            if "theme_preference" not in cols:
                # Per-user UI theme (default/light/dark/synthwave). NULL means
                # the client falls back to its default; validation lives in
                # the preferences route.
                conn.execute("ALTER TABLE users ADD COLUMN theme_preference TEXT")
                logger.info("Users table migrated to include theme_preference")

            conn.execute(
                "UPDATE users SET external_id = google_id WHERE external_id IS NULL"
            )
            conn.execute(
                """
                UPDATE users
                   SET auth_provider = CASE
                           WHEN google_id = ? THEN 'local'
                           ELSE 'legacy-google'
                       END
                 WHERE auth_provider IS NULL
                """,
                (default_self_host_external_id(),),
            )
            conn.execute(
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_users_provider_external "
                "ON users(auth_provider, external_id)"
            )
        except sqlite3.Error as exc:
            logger.warning("Users table migration failed (non-critical): %s", exc)

    def _ensure_default_user(self, conn: sqlite3.Connection) -> "int | None":
        """Ensure the seeded self-host user exists; return its id.

        Idempotent: an existing row (however it was created) is reused. The
        row keeps google_id equal to DEFAULT_SELF_HOST_ID so the legacy
        upsert-by-google_id path continues to hit the same user.
        """
        try:
            external_id = default_self_host_external_id()
            row = conn.execute(
                "SELECT id FROM users WHERE google_id = ?",
                (external_id,),
            ).fetchone()
            if row:
                return int(row[0])

            name = "Me"
            email = f"{external_id}@localhost"
            try:
                try:
                    from .config import get_config  # type: ignore
                except ImportError:
                    from config import get_config  # type: ignore
                cfg = get_config()
                name = cfg.SELFHOST_USER_NAME or name
                email = cfg.SELFHOST_USER_EMAIL or email
            except Exception:
                pass

            cursor = conn.execute(
                """
                INSERT INTO users (google_id, email, name, auth_provider, external_id)
                VALUES (?, ?, ?, 'local', ?)
                """,
                (external_id, email, name, external_id),
            )
            logger.info("Seeded default self-host user")
            return int(cursor.lastrowid or 0) or None
        except sqlite3.Error as exc:
            logger.warning("Default user seeding failed (non-critical): %s", exc)
            return None

    def _create_mood_entries_table(self, conn: sqlite3.Connection) -> None:
        conn.execute(
            """
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
            """
        )
        logger.info("Mood entries table ready")

    GROUPS_TABLE_DDL = """
        CREATE TABLE IF NOT EXISTS groups (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id INTEGER,
            name TEXT NOT NULL,
            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE
        )
    """

    def _create_groups_table(
        self, conn: sqlite3.Connection, default_user_id: "int | None" = None
    ) -> None:
        conn.execute(self.GROUPS_TABLE_DDL)
        self._migrate_groups_table_schema(conn, default_user_id)
        try:
            conn.execute(
                "CREATE UNIQUE INDEX IF NOT EXISTS idx_groups_user_name "
                "ON groups(user_id, name)"
            )
        except sqlite3.Error as exc:
            logger.warning("Groups unique index creation failed (non-critical): %s", exc)
        logger.info("Groups table ready")

    def _groups_table_needs_rebuild(self, conn: sqlite3.Connection) -> bool:
        """Detect the legacy groups shape: no user_id, or a global UNIQUE(name).

        The old inline UNIQUE(name) constraint cannot be dropped in place, so
        either condition means the table must be rebuilt.
        """
        cols = {row[1] for row in conn.execute("PRAGMA table_info(groups)").fetchall()}
        if not cols:
            return False  # table does not exist yet
        if "user_id" not in cols:
            return True
        for _, index_name, is_unique, origin, *_ in conn.execute(
            "PRAGMA index_list(groups)"
        ).fetchall():
            if not is_unique or origin != "u":
                continue
            # index_name comes from SQLite's own metadata, not user input;
            # PRAGMA does not accept bound parameters, so quote defensively.
            safe_name = index_name.replace('"', '""')
            index_cols = [
                row[2]
                for row in conn.execute(f'PRAGMA index_info("{safe_name}")').fetchall()
            ]
            if index_cols == ["name"]:
                return True
        return False

    def _migrate_groups_table_schema(
        self, conn: sqlite3.Connection, default_user_id: "int | None" = None
    ) -> None:
        """Migrate groups to per-user scoping.

        Legacy shape (`name TEXT NOT NULL UNIQUE`, no user_id) is rebuilt via
        the SQLite-documented create-new / copy / drop / rename sequence in a
        single explicit transaction, because SQLite cannot drop the inline
        UNIQUE(name) constraint in place. Row counts are verified before the
        rename is committed; any failure rolls back and leaves the old table
        untouched. Afterwards (and on every init) rows without an owner are
        backfilled to the seeded self-host user.
        """
        try:
            if self._groups_table_needs_rebuild(conn):
                old_cols = {
                    row[1]
                    for row in conn.execute("PRAGMA table_info(groups)").fetchall()
                }
                user_id_select = "user_id" if "user_id" in old_cols else "NULL"

                # init_database's connection keeps foreign_keys OFF (the
                # sqlite3 default), which the rebuild requires so that the
                # RENAME does not rewrite group_options' FK reference and the
                # DROP does not cascade. Set it explicitly rather than assume;
                # the PRAGMA is a no-op inside a transaction, so close any
                # pending implicit transaction first.
                if conn.in_transaction:
                    conn.commit()
                conn.execute("PRAGMA foreign_keys=OFF")
                conn.execute("BEGIN IMMEDIATE")
                try:
                    before_count = conn.execute(
                        "SELECT COUNT(*) FROM groups"
                    ).fetchone()[0]

                    conn.execute(
                        """
                        CREATE TABLE groups_migration_new (
                            id INTEGER PRIMARY KEY AUTOINCREMENT,
                            user_id INTEGER,
                            name TEXT NOT NULL,
                            created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                            FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE
                        )
                        """
                    )
                    conn.execute(
                        f"""
                        INSERT INTO groups_migration_new (id, user_id, name, created_at)
                        SELECT id, {user_id_select}, name, created_at FROM groups
                        """
                    )
                    after_count = conn.execute(
                        "SELECT COUNT(*) FROM groups_migration_new"
                    ).fetchone()[0]
                    if before_count != after_count:
                        raise sqlite3.IntegrityError(
                            f"groups rebuild row count mismatch: "
                            f"{before_count} != {after_count}"
                        )

                    conn.execute("DROP TABLE groups")
                    conn.execute(
                        "ALTER TABLE groups_migration_new RENAME TO groups"
                    )
                    conn.execute(
                        "CREATE UNIQUE INDEX idx_groups_user_name "
                        "ON groups(user_id, name)"
                    )

                    violations = conn.execute(
                        "PRAGMA foreign_key_check(groups)"
                    ).fetchall()
                    if violations:
                        raise sqlite3.IntegrityError(
                            f"groups rebuild produced FK violations: {violations!r}"
                        )

                    conn.execute("COMMIT")
                    logger.info(
                        "Groups table rebuilt for per-user scoping (%s rows)",
                        after_count,
                    )
                except Exception:
                    conn.execute("ROLLBACK")
                    raise

            # Backfill ownerless rows to the seeded self-host user. Runs on
            # every init and only touches NULL rows, so it is idempotent.
            if default_user_id is not None:
                conn.execute(
                    "UPDATE groups SET user_id = ? WHERE user_id IS NULL",
                    (default_user_id,),
                )
        except sqlite3.Error as exc:
            logger.warning("Groups table migration failed (non-critical): %s", exc)

    def _create_group_options_table(self, conn: sqlite3.Connection) -> None:
        conn.execute(
            """
            CREATE TABLE IF NOT EXISTS group_options (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                group_id INTEGER NOT NULL,
                name TEXT NOT NULL,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (group_id) REFERENCES groups (id) ON DELETE CASCADE
            )
            """
        )
        logger.info("Group options table ready")

    def _create_entry_selections_table(self, conn: sqlite3.Connection) -> None:
        conn.execute(
            """
            CREATE TABLE IF NOT EXISTS entry_selections (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                entry_id INTEGER NOT NULL,
                option_id INTEGER NOT NULL,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (entry_id) REFERENCES mood_entries (id) ON DELETE CASCADE,
                FOREIGN KEY (option_id) REFERENCES group_options (id) ON DELETE CASCADE
            )
            """
        )
        logger.info("Entry selections table ready")

    def _create_achievements_table(self, conn: sqlite3.Connection) -> None:
        conn.execute(
            """
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
            """
        )
        logger.info("Achievements table ready")

    def _create_goals_table(self, conn: sqlite3.Connection) -> None:
        try:
            conn.execute(
                """
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
                """
            )
            self._migrate_goals_table_schema(conn)
            conn.execute("CREATE INDEX IF NOT EXISTS idx_goals_user ON goals(user_id)")
            logger.info("Goals table ready")
        except sqlite3.Error as exc:
            logger.warning("Goals table creation failed (non-critical): %s", exc)

    def _migrate_goals_table_schema(self, conn: sqlite3.Connection) -> None:
        try:
            cur = conn.execute("PRAGMA table_info(goals)")
            cols: Iterable[str] = {row[1] for row in cur.fetchall()}
            if "last_completed_date" not in cols:
                conn.execute("ALTER TABLE goals ADD COLUMN last_completed_date TEXT")
                logger.info("Goals table migrated to include last_completed_date")
        except sqlite3.Error as exc:
            logger.warning("Goals table migration failed (non-critical): %s", exc)

    def _create_goal_completions_table(self, conn: sqlite3.Connection) -> None:
        try:
            conn.execute(
                """
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
                """
            )
            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_goal_completions_user_goal ON goal_completions(user_id, goal_id)"
            )
            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_goal_completions_date ON goal_completions(date)"
            )
            logger.info("Goal completions table ready")
        except sqlite3.Error as exc:
            logger.warning(
                "Goal completions table creation failed (non-critical): %s", exc
            )

    def _create_user_metrics_table(self, conn: sqlite3.Connection) -> None:
        try:
            conn.execute(
                """
                CREATE TABLE IF NOT EXISTS user_metrics (
                    user_id INTEGER PRIMARY KEY,
                    stats_views INTEGER NOT NULL DEFAULT 0,
                    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE
                )
                """
            )
            logger.info("User metrics table ready")
        except sqlite3.Error as exc:
            logger.warning("User metrics table creation failed (non-critical): %s", exc)

    def _create_activity_log_table(self, conn: sqlite3.Connection) -> None:
        try:
            conn.execute(
                """
                CREATE TABLE IF NOT EXISTS activity_log (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    user_id INTEGER NOT NULL,
                    event_type TEXT NOT NULL,
                    metadata TEXT,
                    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    FOREIGN KEY (user_id) REFERENCES users (id) ON DELETE CASCADE
                )
                """
            )
            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_activity_log_user_created "
                "ON activity_log(user_id, created_at DESC)"
            )
            logger.info("Activity log table ready")
        except sqlite3.Error as exc:
            logger.warning("Activity log table creation failed (non-critical): %s", exc)

    def _create_database_indexes(self, conn: sqlite3.Connection) -> None:
        try:
            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_mood_entries_date ON mood_entries(date)"
            )
            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_mood_entries_user_date "
                "ON mood_entries(user_id, date)"
            )
            logger.info("Mood entries indexes ready")
        except sqlite3.Error as exc:
            logger.warning("Index creation failed (non-critical): %s", exc)

    # --- Seed helpers -----------------------------------------------------------
    def ensure_default_groups_for_user(self, user_id: int) -> None:
        """Ensure the default tag groups exist for the given user.

        Idempotent per user; safe to call at first login for new users.
        """
        with self._connect() as conn:
            for group_name, options in DEFAULT_GROUPS.items():
                cursor = conn.execute(
                    "SELECT id FROM groups WHERE name = ? AND user_id = ?",
                    (group_name, user_id),
                )
                group_row = cursor.fetchone()

                if not group_row:
                    cursor = conn.execute(
                        "INSERT INTO groups (name, user_id) VALUES (?, ?)",
                        (group_name, user_id),
                    )
                    group_id = cursor.lastrowid
                    for option in options:
                        conn.execute(
                            "INSERT INTO group_options (group_id, name) VALUES (?, ?)",
                            (group_id, option),
                        )

            conn.commit()
            logger.info("Default groups ensured for user %s", user_id)

    def _insert_default_groups(self) -> None:
        default_user_id = self._get_default_user_id()
        if default_user_id is None:
            logger.warning(
                "Default groups not seeded: default self-host user missing"
            )
            return
        self.ensure_default_groups_for_user(default_user_id)


__all__ = ["DatabaseSchemaMixin", "DEFAULT_GROUPS"]
