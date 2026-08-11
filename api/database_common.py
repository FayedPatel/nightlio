"""Shared database utilities and definitions for Nightlio."""

from __future__ import annotations

import logging
import sqlite3
import time
from typing import Any, Dict, Iterable, Optional, Sequence

# Configure logging once for all database modules
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger("api.database")


class DatabaseError(Exception):
    """Raised when a database operation fails."""


class SQLQueries:
    """Constants for SQL queries to improve maintainability."""

    # User queries
    CREATE_USER = (
        "INSERT INTO users (google_id, email, name, avatar_url) VALUES (?, ?, ?, ?)"
    )

    GET_USER_BY_GOOGLE_ID = (
        "SELECT id, google_id, email, name, avatar_url, auth_provider, external_id, "
        "created_at, last_login FROM users WHERE google_id = ?"
    )

    GET_USER_BY_ID = (
        "SELECT id, google_id, email, name, avatar_url, auth_provider, external_id, "
        "created_at, last_login FROM users WHERE id = ?"
    )

    GET_USER_BY_PROVIDER = (
        "SELECT id, google_id, email, name, avatar_url, auth_provider, external_id, "
        "created_at, last_login FROM users WHERE auth_provider = ? AND external_id = ?"
    )

    CREATE_PROVIDER_USER = (
        "INSERT INTO users (google_id, email, name, avatar_url, auth_provider, "
        "external_id, password_hash) VALUES (?, ?, ?, ?, ?, ?, ?)"
    )

    UPSERT_USER = (
        "INSERT INTO users (google_id, email, name, avatar_url) "
        "VALUES (?, ?, ?, ?) "
        "ON CONFLICT(google_id) DO UPDATE SET "
        "  email=COALESCE(excluded.email, users.email), "
        "  name=COALESCE(excluded.name, users.name), "
        "  avatar_url=COALESCE(excluded.avatar_url, users.avatar_url), "
        "  last_login=CURRENT_TIMESTAMP"
    )

    # Goals queries
    GET_GOALS_BY_USER = (
        "SELECT id, user_id, title, description, frequency_per_week, completed, "
        "       streak, period_start, last_completed_date, created_at, updated_at "
        "FROM goals WHERE user_id = ? ORDER BY created_at DESC"
    )

    GET_GOAL_BY_ID = (
        "SELECT id, user_id, title, description, frequency_per_week, completed, "
        "       streak, period_start, last_completed_date, created_at, updated_at "
        "FROM goals WHERE id = ? AND user_id = ?"
    )

    # Mood entries queries
    GET_USER_ENTRY_DATES = (
        "SELECT DISTINCT date FROM mood_entries WHERE user_id = ? ORDER BY date DESC"
    )

    GET_MOOD_STATISTICS = (
        "SELECT "
        "  COUNT(*) as total_entries, "
        "  AVG(mood) as average_mood, "
        "  MIN(mood) as lowest_mood, "
        "  MAX(mood) as highest_mood, "
        "  MIN(date) as first_entry_date, "
        "  MAX(date) as last_entry_date "
        "FROM mood_entries WHERE user_id = ?"
    )


def default_self_host_external_id() -> str:
    """Return the configured DEFAULT_SELF_HOST_ID (seeded self-host user).

    Falls back to the historical literal if the config module cannot be
    imported (e.g. ad-hoc scripts running outside the app context).
    """
    try:
        try:
            from .config import get_config  # type: ignore
        except ImportError:
            from config import get_config  # type: ignore
        return get_config().DEFAULT_SELF_HOST_ID
    except Exception:
        return "selfhost_default_user"


class DatabaseConnectionMixin:
    """Provides connection helpers shared across database mixins."""

    db_path: str

    def _get_default_user_id(self) -> Optional[int]:
        """Return the id of the seeded self-host user, or None if absent."""
        with self._connect() as conn:
            row = conn.execute(
                "SELECT id FROM users WHERE google_id = ?",
                (default_self_host_external_id(),),
            ).fetchone()
            return int(row[0]) if row else None

    def _resolve_user_id(self, user_id: Optional[int]) -> Optional[int]:
        """Backward-compat shim: None means the default self-host user.

        Callers that predate per-user scoping (Phase 2a) may omit user_id;
        those calls resolve to the seeded self-host user. The auth layer
        (Phase 2b) should always pass an explicit user_id.
        """
        if user_id is not None:
            return user_id
        return self._get_default_user_id()

    def _connect(self) -> sqlite3.Connection:
        """Create a SQLite connection with safe defaults."""
        try:
            conn = sqlite3.connect(self.db_path)
            conn.execute("PRAGMA foreign_keys=ON")
            return conn
        except sqlite3.Error as exc:  # pragma: no cover - rare failure
            logger.error("Failed to connect to database: %s", exc)
            raise DatabaseError(f"Database connection failed: {exc}") from exc

    def _execute_with_retry(
        self,
        query: str,
        params: Sequence[Any] | Iterable[Any] = (),
        retries: int = 3,
    ) -> sqlite3.Cursor:
        """Execute a statement with basic retry handling for database locks."""
        for attempt in range(retries):
            try:
                conn = self._connect()
                try:
                    cursor = conn.execute(query, tuple(params))
                    conn.commit()
                    return cursor
                finally:
                    conn.close()
            except sqlite3.OperationalError as exc:
                if "database is locked" in str(exc) and attempt < retries - 1:
                    delay = 0.1 * (attempt + 1)
                    logger.warning(
                        "Database locked (attempt %s). Retrying in %.2fs...",
                        attempt + 1,
                        delay,
                    )
                    time.sleep(delay)
                    continue
                logger.error("Database operation failed: %s", exc)
                raise DatabaseError(f"Database operation failed: {exc}") from exc
            except sqlite3.Error as exc:
                logger.error("Database error: %s", exc)
                raise DatabaseError(f"Database error: {exc}") from exc

        raise DatabaseError("Database operation failed after all retries")


__all__ = [
    "DatabaseConnectionMixin",
    "DatabaseError",
    "SQLQueries",
    "default_self_host_external_id",
    "logger",
]
