"""User management helpers."""

from __future__ import annotations

import sqlite3
from typing import Dict, Optional

try:  # pragma: no cover - support script imports
    from .database_common import DatabaseConnectionMixin, SQLQueries
except ImportError:  # pragma: no cover
    from database_common import DatabaseConnectionMixin, SQLQueries  # type: ignore


class UsersMixin(DatabaseConnectionMixin):
    """CRUD helpers for user records."""

    def create_user(
        self,
        google_id: str,
        email: str,
        name: str,
        avatar_url: Optional[str] = None,
    ) -> int:
        with self._connect() as conn:
            cursor = conn.execute(
                SQLQueries.CREATE_USER,
                (google_id, email, name, avatar_url),
            )
            conn.commit()
            return int(cursor.lastrowid or 0)

    def get_user_by_google_id(self, google_id: str) -> Optional[Dict]:
        with self._connect() as conn:
            conn.row_factory = sqlite3.Row
            cursor = conn.execute(SQLQueries.GET_USER_BY_GOOGLE_ID, (google_id,))
            row = cursor.fetchone()
            return dict(row) if row else None

    def get_user_by_id(self, user_id: int) -> Optional[Dict]:
        with self._connect() as conn:
            conn.row_factory = sqlite3.Row
            cursor = conn.execute(SQLQueries.GET_USER_BY_ID, (user_id,))
            row = cursor.fetchone()
            return dict(row) if row else None

    def get_user_theme(self, user_id: int) -> Optional[str]:
        with self._connect() as conn:
            cursor = conn.execute(
                "SELECT theme_preference FROM users WHERE id = ?", (user_id,)
            )
            row = cursor.fetchone()
            return row[0] if row else None

    def set_user_theme(self, user_id: int, theme: str) -> None:
        with self._connect() as conn:
            conn.execute(
                "UPDATE users SET theme_preference = ? WHERE id = ?",
                (theme, user_id),
            )
            conn.commit()

    def update_user_last_login(self, user_id: int) -> None:
        with self._connect() as conn:
            conn.execute(
                """
                UPDATE users
                   SET last_login = CURRENT_TIMESTAMP
                 WHERE id = ?
                """,
                (user_id,),
            )
            conn.commit()

    def upsert_user_by_google_id(
        self,
        google_id: str,
        email: Optional[str],
        name: Optional[str],
        avatar_url: Optional[str] = None,
    ) -> Optional[Dict]:
        with self._connect() as conn:
            conn.row_factory = sqlite3.Row
            conn.execute("BEGIN IMMEDIATE")
            try:
                try:
                    cursor = conn.execute(
                        SQLQueries.UPSERT_USER
                        + " RETURNING id, google_id, email, name, avatar_url, "
                        "auth_provider, external_id, created_at, last_login",
                        (google_id, email, name, avatar_url),
                    )
                    row = cursor.fetchone()
                except sqlite3.OperationalError:
                    conn.execute(
                        SQLQueries.UPSERT_USER,
                        (google_id, email, name, avatar_url),
                    )
                    row = conn.execute(
                        SQLQueries.GET_USER_BY_GOOGLE_ID,
                        (google_id,),
                    ).fetchone()
                conn.commit()
                return dict(row) if row else None
            except Exception:
                conn.rollback()
                raise

    # --- Provider-scoped identity (Phase 2a) ---------------------------------
    #
    # New users are keyed by (auth_provider, external_id), enforced by the
    # unique index idx_users_provider_external. The legacy google_id column
    # (UNIQUE NOT NULL on existing databases) is populated with the synthetic
    # value "<provider>:<external_id>" so inserts keep working on both old and
    # new schemas; new code should never look users up by google_id.

    @staticmethod
    def _compat_google_id(provider: str, external_id: str) -> str:
        return f"{provider}:{external_id}"

    def get_user_by_provider(
        self, auth_provider: str, external_id: str
    ) -> Optional[Dict]:
        """Look up a user by provider-scoped identity."""
        with self._connect() as conn:
            conn.row_factory = sqlite3.Row
            cursor = conn.execute(
                SQLQueries.GET_USER_BY_PROVIDER, (auth_provider, external_id)
            )
            row = cursor.fetchone()
            return dict(row) if row else None

    def upsert_oidc_user(
        self,
        external_id: str,
        email: Optional[str],
        name: Optional[str],
        avatar_url: Optional[str] = None,
        provider: str = "oidc",
    ) -> Optional[Dict]:
        """Insert or update a user identified by (provider, external_id).

        Implemented as select-then-insert/update inside one immediate
        transaction (rather than ON CONFLICT) because the row is covered by
        two unique constraints (google_id and the provider index) and upsert
        conflict targets only handle one.
        """
        with self._connect() as conn:
            conn.row_factory = sqlite3.Row
            conn.execute("BEGIN IMMEDIATE")
            try:
                row = conn.execute(
                    SQLQueries.GET_USER_BY_PROVIDER, (provider, external_id)
                ).fetchone()
                if row:
                    conn.execute(
                        """
                        UPDATE users
                           SET email = COALESCE(?, email),
                               name = COALESCE(?, name),
                               avatar_url = COALESCE(?, avatar_url),
                               last_login = CURRENT_TIMESTAMP
                         WHERE id = ?
                        """,
                        (email, name, avatar_url, row["id"]),
                    )
                    user_id = int(row["id"])
                else:
                    cursor = conn.execute(
                        SQLQueries.CREATE_PROVIDER_USER,
                        (
                            self._compat_google_id(provider, external_id),
                            email or f"{external_id}@{provider}.invalid",
                            name or "User",
                            avatar_url,
                            provider,
                            external_id,
                            None,
                        ),
                    )
                    user_id = int(cursor.lastrowid or 0)
                row = conn.execute(
                    SQLQueries.GET_USER_BY_ID, (user_id,)
                ).fetchone()
                conn.commit()
                return dict(row) if row else None
            except Exception:
                conn.rollback()
                raise

    def create_local_user(
        self,
        username: str,
        password_hash: str,
        email: Optional[str] = None,
        name: Optional[str] = None,
    ) -> int:
        """Create a local-auth user and return its id.

        Raises sqlite3.IntegrityError if a local user with this username
        already exists (unique on (auth_provider, external_id)).
        """
        with self._connect() as conn:
            cursor = conn.execute(
                SQLQueries.CREATE_PROVIDER_USER,
                (
                    self._compat_google_id("local", username),
                    email or f"{username}@localhost",
                    name or username,
                    None,
                    "local",
                    username,
                    password_hash,
                ),
            )
            conn.commit()
            return int(cursor.lastrowid or 0)

    def get_user_password_hash(self, user_id: int) -> Optional[str]:
        """Return the stored password hash for a user, or None."""
        with self._connect() as conn:
            row = conn.execute(
                "SELECT password_hash FROM users WHERE id = ?", (user_id,)
            ).fetchone()
            return row[0] if row else None

    def set_user_password(self, user_id: int, password_hash: str) -> None:
        """Set (or replace) the password hash for a user."""
        with self._connect() as conn:
            conn.execute(
                "UPDATE users SET password_hash = ? WHERE id = ?",
                (password_hash, user_id),
            )
            conn.commit()


__all__ = ["UsersMixin"]
