"""Nightlio database facade built from modular mixins."""

from __future__ import annotations

from pathlib import Path
from typing import Optional

try:  # pragma: no cover - fallback for script execution
    from .database_achievements import AchievementsMixin
    from .database_activity import ActivityLogMixin
    from .database_common import (
        DatabaseConnectionMixin,
        DatabaseError,
        SQLQueries,
        logger,
    )
    from .database_goals import GoalsMixin
    from .database_groups import GroupsMixin
    from .database_moods import MoodEntriesMixin
    from .database_schema import DatabaseSchemaMixin
    from .database_stats import StatisticsMixin
    from .database_users import UsersMixin
except ImportError:  # pragma: no cover - executed when run as a script module
    from database_achievements import AchievementsMixin  # type: ignore
    from database_activity import ActivityLogMixin  # type: ignore
    from database_common import (  # type: ignore
        DatabaseConnectionMixin,
        DatabaseError,
        SQLQueries,
        logger,
    )
    from database_goals import GoalsMixin  # type: ignore
    from database_groups import GroupsMixin  # type: ignore
    from database_moods import MoodEntriesMixin  # type: ignore
    from database_schema import DatabaseSchemaMixin  # type: ignore
    from database_stats import StatisticsMixin  # type: ignore
    from database_users import UsersMixin  # type: ignore


class MoodDatabase(
    DatabaseSchemaMixin,
    UsersMixin,
    GoalsMixin,
    MoodEntriesMixin,
    GroupsMixin,
    AchievementsMixin,
    ActivityLogMixin,
    StatisticsMixin,
):
    """High-level facade composing all database-related mixins."""

    db_path: str

    def __init__(self, db_path: Optional[str] = None, *, init: bool = True) -> None:
        if db_path is not None:
            resolved_path = Path(db_path)
        else:
            # Default: <repo_root>/data/nightlio.db when run from source
            # (api/database.py -> api/ -> repo root). Only reached by
            # ad-hoc scripts (api/scripts/seed_selfhost_user.py); the app
            # itself always passes an explicit db_path (Config.DATABASE_PATH
            # always has its own default -- see api/config.py).
            default_data_dir = Path(__file__).resolve().parent.parent / "data"
            resolved_path = default_data_dir / "nightlio.db"

        # Only ever create the directory the resolved path actually lives
        # in. This used to unconditionally mkdir a *second*,
        # separately-computed "<two levels up from this file>/data"
        # directory regardless of db_path -- harmless when __file__ is
        # api/database.py (repo root/data, usually already present), but
        # wrong inside the Docker image, where this module loads as
        # /app/database.py and that computation resolves to "/data" at the
        # container root: an unrelated, root-owned path with no connection
        # to the actual (correctly configured) db_path. Running the
        # container as root silently masked this, since root can always
        # mkdir "/data"; running it as an unprivileged user (see
        # api/Dockerfile) turned the silent no-op into a real
        # PermissionError on startup.
        resolved_path.parent.mkdir(parents=True, exist_ok=True)
        self.db_path = str(resolved_path)

        logger.debug("MoodDatabase configured with db_path=%s", self.db_path)

        if init:
            self.init_database()

    def __repr__(self) -> str:  # pragma: no cover - convenience helper
        return f"MoodDatabase(db_path={self.db_path!r})"


__all__ = [
    "MoodDatabase",
    "DatabaseConnectionMixin",
    "DatabaseError",
    "SQLQueries",
    "logger",
]
