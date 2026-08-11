"""Activity log helpers.

Stores an append-only per-user event feed (logins, entry mutations,
achievements, goal completions). Reads are keyset-paginated on the row id —
never OFFSET, which degrades as the table grows — and old rows can be pruned
so a self-hosted database file stays small and portable.

Known event types: ``login``, ``entry_created``, ``entry_edited``,
``entry_deleted``, ``achievement_unlocked``, ``goal_completed``.
"""

from __future__ import annotations

import json
import sqlite3
from typing import Any, Dict, List, Optional

try:  # pragma: no cover - enable script execution fallback
    from .database_common import DatabaseConnectionMixin, logger
except ImportError:  # pragma: no cover
    from database_common import DatabaseConnectionMixin, logger  # type: ignore

# Hard ceiling for a single page of activity rows.
MAX_ACTIVITY_PAGE_SIZE = 200


class ActivityLogMixin(DatabaseConnectionMixin):
    """CRUD helpers for the activity_log table."""

    def add_activity(
        self,
        user_id: int,
        event_type: str,
        metadata: Optional[Dict[str, Any]] = None,
    ) -> int:
        """Record an event for a user; returns the new row id.

        ``metadata`` is stored as a JSON blob. Callers must never put tokens
        or IDP responses in it.
        """
        payload = json.dumps(metadata) if metadata is not None else None
        with self._connect() as conn:
            cursor = conn.execute(
                """
                INSERT INTO activity_log (user_id, event_type, metadata)
                VALUES (?, ?, ?)
                """,
                (user_id, event_type, payload),
            )
            conn.commit()
            return int(cursor.lastrowid or 0)

    def get_activity(
        self,
        user_id: int,
        before: Optional[int] = None,
        limit: int = 50,
    ) -> List[Dict[str, Any]]:
        """Return a page of the user's activity, newest first.

        Keyset pagination: ``before`` is the ``id`` of the last row from the
        previous page; pass it back to fetch the next (older) page. Row ids
        are monotonically increasing, so id order matches insertion order and
        breaks created_at ties deterministically.
        """
        limit = max(1, min(int(limit), MAX_ACTIVITY_PAGE_SIZE))
        with self._connect() as conn:
            conn.row_factory = sqlite3.Row
            if before is not None:
                cursor = conn.execute(
                    """
                    SELECT id, user_id, event_type, metadata, created_at
                      FROM activity_log
                     WHERE user_id = ? AND id < ?
                     ORDER BY id DESC
                     LIMIT ?
                    """,
                    (user_id, before, limit),
                )
            else:
                cursor = conn.execute(
                    """
                    SELECT id, user_id, event_type, metadata, created_at
                      FROM activity_log
                     WHERE user_id = ?
                     ORDER BY id DESC
                     LIMIT ?
                    """,
                    (user_id, limit),
                )
            rows = []
            for row in cursor.fetchall():
                item = dict(row)
                if item.get("metadata") is not None:
                    try:
                        item["metadata"] = json.loads(item["metadata"])
                    except (ValueError, TypeError):
                        logger.warning(
                            "Malformed activity metadata for row %s", item.get("id")
                        )
                        item["metadata"] = None
                rows.append(item)
            return rows

    def prune_activity(self, days: int = 90) -> int:
        """Delete activity rows older than ``days`` days; returns count deleted.

        Maintenance operation across all users (age-based retention), not a
        per-user query. Not called automatically; wire it to a periodic task
        or run it at startup.
        """
        days = int(days)
        with self._connect() as conn:
            cursor = conn.execute(
                "DELETE FROM activity_log WHERE created_at < datetime('now', ?)",
                (f"-{days} days",),
            )
            conn.commit()
            deleted = cursor.rowcount
            if deleted:
                logger.info("Pruned %s activity rows older than %s days", deleted, days)
            return deleted


__all__ = ["ActivityLogMixin", "MAX_ACTIVITY_PAGE_SIZE"]
