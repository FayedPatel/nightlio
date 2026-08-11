"""Group and selection helpers.

All group reads and writes are scoped to a user as of Phase 2a. The
``user_id`` parameters default to None for backward compatibility, which
resolves to the seeded self-host user (see ``_resolve_user_id``); the auth
layer should always pass an explicit user_id. ``group_options`` rows are
scoped through their parent group rather than carrying their own user_id.
"""

from __future__ import annotations

import sqlite3
from typing import Dict, List, Optional

try:  # pragma: no cover - enable script execution fallback
    from .database_common import DatabaseConnectionMixin
except ImportError:  # pragma: no cover
    from database_common import DatabaseConnectionMixin  # type: ignore


class GroupsMixin(DatabaseConnectionMixin):
    """Provides CRUD helpers for groups and group options."""

    def get_all_groups(self, user_id: Optional[int] = None) -> List[Dict]:
        uid = self._resolve_user_id(user_id)
        if uid is None:
            return []
        with self._connect() as conn:
            conn.row_factory = sqlite3.Row
            cursor = conn.execute(
                "SELECT id, name FROM groups WHERE user_id = ? ORDER BY name",
                (uid,),
            )
            groups: List[Dict] = []
            for group_row in cursor.fetchall():
                group = dict(group_row)
                options_cursor = conn.execute(
                    """
                    SELECT id, name
                      FROM group_options
                     WHERE group_id = ?
                     ORDER BY name
                    """,
                    (group["id"],),
                )
                group["options"] = [
                    dict(option_row) for option_row in options_cursor.fetchall()
                ]
                groups.append(group)
            return groups

    def create_group(self, name: str, user_id: Optional[int] = None) -> int:
        uid = self._resolve_user_id(user_id)
        if uid is None:
            raise ValueError("No user available to own the group")
        with self._connect() as conn:
            cursor = conn.execute(
                "INSERT INTO groups (name, user_id) VALUES (?, ?)",
                (name, uid),
            )
            conn.commit()
            return int(cursor.lastrowid or 0)

    def create_group_option(
        self, group_id: int, name: str, user_id: Optional[int] = None
    ) -> int:
        uid = self._resolve_user_id(user_id)
        with self._connect() as conn:
            owner_row = conn.execute(
                "SELECT id FROM groups WHERE id = ? AND user_id = ?",
                (group_id, uid),
            ).fetchone()
            if not owner_row:
                raise ValueError("Group not found for user")
            cursor = conn.execute(
                "INSERT INTO group_options (group_id, name) VALUES (?, ?)",
                (group_id, name),
            )
            conn.commit()
            return int(cursor.lastrowid or 0)

    def delete_group(self, group_id: int, user_id: Optional[int] = None) -> bool:
        uid = self._resolve_user_id(user_id)
        with self._connect() as conn:
            cursor = conn.execute(
                "DELETE FROM groups WHERE id = ? AND user_id = ?",
                (group_id, uid),
            )
            conn.commit()
            return cursor.rowcount > 0

    def delete_group_option(
        self, option_id: int, user_id: Optional[int] = None
    ) -> bool:
        uid = self._resolve_user_id(user_id)
        with self._connect() as conn:
            cursor = conn.execute(
                """
                DELETE FROM group_options
                 WHERE id = ?
                   AND group_id IN (SELECT id FROM groups WHERE user_id = ?)
                """,
                (option_id, uid),
            )
            conn.commit()
            return cursor.rowcount > 0

    def add_entry_selections(self, entry_id: int, option_ids: List[int]) -> None:
        with self._connect() as conn:
            conn.executemany(
                "INSERT INTO entry_selections (entry_id, option_id) VALUES (?, ?)",
                [(entry_id, option_id) for option_id in option_ids],
            )
            conn.commit()

    def get_entry_selections(
        self, entry_id: int, user_id: Optional[int] = None
    ) -> List[Dict]:
        """Return the selected options for an entry.

        When user_id is provided the entry is additionally verified to belong
        to that user; without it the caller (e.g. mood_service) is expected to
        have verified entry ownership already.
        """
        with self._connect() as conn:
            conn.row_factory = sqlite3.Row
            if user_id is not None:
                cursor = conn.execute(
                    """
                    SELECT go.id, go.name, g.name as group_name
                      FROM entry_selections es
                      JOIN mood_entries me ON es.entry_id = me.id
                      JOIN group_options go ON es.option_id = go.id
                      JOIN groups g ON go.group_id = g.id
                     WHERE es.entry_id = ? AND me.user_id = ?
                     ORDER BY g.name, go.name
                    """,
                    (entry_id, user_id),
                )
            else:
                cursor = conn.execute(
                    """
                    SELECT go.id, go.name, g.name as group_name
                      FROM entry_selections es
                      JOIN group_options go ON es.option_id = go.id
                      JOIN groups g ON go.group_id = g.id
                     WHERE es.entry_id = ?
                     ORDER BY g.name, go.name
                    """,
                    (entry_id,),
                )
            return [dict(row) for row in cursor.fetchall()]


__all__ = ["GroupsMixin"]
