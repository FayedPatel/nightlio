"""Extended statistics aggregations mixin.

All aggregation happens in SQL so the queries stay correct and fast as the
entry count grows; Python only does final shaping (and the square root for
volatility, since SQLite has no stddev function).

Every query filters by ``user_id`` — per-user isolation is load-bearing now
that multi-user auth has shipped.

Mood entry dates are stored as text and arrive in two shapes: ISO
(``YYYY-MM-DD``) and US locale (``M/D/YYYY``, what the frontend's
``toLocaleDateString()`` produces). The ``_ISO_DAY_EXPR`` fragment normalises
both to ISO inside SQL; rows whose date matches neither shape are excluded
from aggregates, mirroring how the streak calculator skips unparseable dates.
``goal_completions.date`` is always written as ISO by the backend, so it needs
no normalisation.
"""

from __future__ import annotations

import calendar
import math
import sqlite3
from typing import Any, Dict, List

try:  # pragma: no cover - allow top-level script usage
    from .database_common import DatabaseConnectionMixin
except ImportError:  # pragma: no cover
    from database_common import DatabaseConnectionMixin  # type: ignore

# Weekday labels indexed by strftime('%w') (0 = Sunday).
WEEKDAY_NAMES = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
]

# Normalise a stored mood_entries.date value to ISO YYYY-MM-DD, or NULL if it
# matches neither the ISO nor the US M/D/YYYY shape.
_ISO_DAY_EXPR = """
    CASE
        WHEN date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]' THEN date
        WHEN date GLOB '*/*/[0-9][0-9][0-9][0-9]' THEN
            substr(date, -4)
            || '-'
            || printf('%02d', CAST(substr(date, 1, instr(date, '/') - 1) AS INTEGER))
            || '-'
            || printf(
                   '%02d',
                   CAST(
                       substr(
                           substr(date, instr(date, '/') + 1),
                           1,
                           instr(substr(date, instr(date, '/') + 1), '/') - 1
                       ) AS INTEGER
                   )
               )
        ELSE NULL
    END
"""

# Shared CTE: one user's mood entries with a normalised ISO day column.
_ENTRIES_CTE = f"""
    entries AS (
        SELECT id, day, mood
          FROM (
                SELECT id, {_ISO_DAY_EXPR} AS day, mood
                  FROM mood_entries
                 WHERE user_id = :user_id
          )
         WHERE day IS NOT NULL
    )
"""


class StatisticsMixin(DatabaseConnectionMixin):
    """Read-only aggregation helpers for the extended statistics endpoints."""

    def rolling_averages(self, user_id: int) -> Dict[str, Any]:
        """Daily mood series with 7-day and 30-day trailing means.

        The trailing means are computed in SQL with window functions over the
        preceding logged days (ROWS BETWEEN), so a day with multiple entries
        contributes its daily average exactly once.
        """
        with self._connect() as conn:
            conn.row_factory = sqlite3.Row
            rows = conn.execute(
                f"""
                WITH {_ENTRIES_CTE},
                daily AS (
                    SELECT day,
                           AVG(mood) AS average_mood,
                           COUNT(*) AS entry_count
                      FROM entries
                     GROUP BY day
                )
                SELECT day AS date,
                       average_mood,
                       entry_count,
                       AVG(average_mood) OVER (
                           ORDER BY day
                           ROWS BETWEEN 6 PRECEDING AND CURRENT ROW
                       ) AS rolling_7,
                       AVG(average_mood) OVER (
                           ORDER BY day
                           ROWS BETWEEN 29 PRECEDING AND CURRENT ROW
                       ) AS rolling_30
                  FROM daily
                 ORDER BY day
                """,
                {"user_id": user_id},
            ).fetchall()
            series = [dict(row) for row in rows]
            return {"series": series, "count": len(series)}

    def weekday_averages(self, user_id: int) -> List[Dict[str, Any]]:
        """Average mood and entry count per weekday (0 = Sunday .. 6 = Saturday).

        Always returns all seven weekdays; days with no entries carry a null
        average and a zero count so callers never divide by zero.
        """
        with self._connect() as conn:
            conn.row_factory = sqlite3.Row
            rows = conn.execute(
                f"""
                WITH {_ENTRIES_CTE}
                SELECT CAST(strftime('%w', day) AS INTEGER) AS weekday,
                       AVG(mood) AS average_mood,
                       COUNT(*) AS entry_count
                  FROM entries
                 GROUP BY weekday
                """,
                {"user_id": user_id},
            ).fetchall()
            by_weekday = {row["weekday"]: row for row in rows}
            result: List[Dict[str, Any]] = []
            for weekday in range(7):
                row = by_weekday.get(weekday)
                result.append(
                    {
                        "weekday": weekday,
                        "name": WEEKDAY_NAMES[weekday],
                        "average_mood": row["average_mood"] if row else None,
                        "entry_count": row["entry_count"] if row else 0,
                    }
                )
            return result

    def mood_volatility(self, user_id: int, days: int = 30) -> Dict[str, Any]:
        """Sample standard deviation of mood over a trailing calendar window.

        The window covers today and the preceding ``days - 1`` calendar days,
        evaluated in local time because entry dates are written from the
        client's local calendar (``toLocaleDateString()``) — using UTC here
        would shift the window boundary for users in non-UTC timezones.
        SQLite has no stddev aggregate, so one SQL pass collects n, the sum,
        and the sum of squares; Python finishes with the square root. The
        stddev is null when fewer than two entries fall in the window.
        """
        days = int(days)
        if days < 1:
            raise ValueError("days must be a positive integer")
        with self._connect() as conn:
            conn.row_factory = sqlite3.Row
            row = conn.execute(
                f"""
                WITH {_ENTRIES_CTE}
                SELECT COUNT(*) AS n,
                       SUM(mood) AS total,
                       SUM(mood * mood) AS total_squares
                  FROM entries
                 WHERE day >= date('now', 'localtime', :window_offset)
                """,
                {"user_id": user_id, "window_offset": f"-{days - 1} days"},
            ).fetchone()

        n = int(row["n"] or 0)
        average = (row["total"] / n) if n > 0 else None
        stddev = None
        if n >= 2:
            variance = (row["total_squares"] - (row["total"] ** 2) / n) / (n - 1)
            # Floating point can push a zero variance a hair negative.
            stddev = math.sqrt(max(variance, 0.0))
        return {
            "window_days": days,
            "entry_count": n,
            "average_mood": average,
            "stddev": stddev,
        }

    def tag_correlations(self, user_id: int) -> List[Dict[str, Any]]:
        """Per group option: avg mood and entry count on days it was selected
        versus days it was not.

        A day counts as "selected" for an option when any of the user's
        entries on that day has the option selected; every entry on such a day
        lands on the "selected" side. Only options the user has actually
        selected at least once appear, and the owning group is explicitly
        filtered by ``user_id`` so a selection pointing at another user's
        option can never leak that user's group names or ids. Counts accompany
        every average so the UI can suppress noise from tiny samples; no
        p-values, by design.
        """
        with self._connect() as conn:
            conn.row_factory = sqlite3.Row
            rows = conn.execute(
                f"""
                WITH {_ENTRIES_CTE},
                option_days AS (
                    SELECT DISTINCT es.option_id, e.day
                      FROM entry_selections es
                      JOIN entries e ON e.id = es.entry_id
                ),
                user_options AS (
                    SELECT DISTINCT go.id AS option_id,
                           go.name AS option_name,
                           g.id AS group_id,
                           g.name AS group_name
                      FROM option_days od
                      JOIN group_options go ON go.id = od.option_id
                      JOIN groups g ON g.id = go.group_id
                     WHERE g.user_id = :user_id
                )
                SELECT uo.option_id,
                       uo.option_name,
                       uo.group_id,
                       uo.group_name,
                       AVG(CASE WHEN od.day IS NOT NULL THEN e.mood END)
                           AS average_mood_selected,
                       COUNT(CASE WHEN od.day IS NOT NULL THEN 1 END)
                           AS entry_count_selected,
                       AVG(CASE WHEN od.day IS NULL THEN e.mood END)
                           AS average_mood_not_selected,
                       COUNT(CASE WHEN od.day IS NULL THEN 1 END)
                           AS entry_count_not_selected
                  FROM user_options uo
                 CROSS JOIN entries e
                  LEFT JOIN option_days od
                         ON od.option_id = uo.option_id AND od.day = e.day
                 GROUP BY uo.option_id, uo.option_name, uo.group_id, uo.group_name
                 ORDER BY uo.group_name, uo.option_name
                """,
                {"user_id": user_id},
            ).fetchall()
            return [dict(row) for row in rows]

    def goal_correlations(self, user_id: int) -> List[Dict[str, Any]]:
        """Per goal: avg mood and entry count on days the goal was completed
        versus days it was not. Same shape as tag correlations.

        Goals with no completions still appear (null completed average, zero
        count); a user with no mood entries gets an empty list.
        """
        with self._connect() as conn:
            conn.row_factory = sqlite3.Row
            rows = conn.execute(
                f"""
                WITH {_ENTRIES_CTE},
                user_goals AS (
                    SELECT id AS goal_id, title AS goal_name
                      FROM goals
                     WHERE user_id = :user_id
                ),
                completion_days AS (
                    SELECT DISTINCT goal_id, date AS day
                      FROM goal_completions
                     WHERE user_id = :user_id
                )
                SELECT ug.goal_id,
                       ug.goal_name,
                       AVG(CASE WHEN cd.day IS NOT NULL THEN e.mood END)
                           AS average_mood_completed,
                       COUNT(CASE WHEN cd.day IS NOT NULL THEN 1 END)
                           AS entry_count_completed,
                       AVG(CASE WHEN cd.day IS NULL THEN e.mood END)
                           AS average_mood_not_completed,
                       COUNT(CASE WHEN cd.day IS NULL THEN 1 END)
                           AS entry_count_not_completed
                  FROM user_goals ug
                 CROSS JOIN entries e
                  LEFT JOIN completion_days cd
                         ON cd.goal_id = ug.goal_id AND cd.day = e.day
                 GROUP BY ug.goal_id, ug.goal_name
                 ORDER BY ug.goal_name, ug.goal_id
                """,
                {"user_id": user_id},
            ).fetchall()
            return [dict(row) for row in rows]

    def heatmap(self, user_id: int, year: int) -> Dict[str, Any]:
        """Average mood and entry count for every logged day of a year."""
        year = int(year)
        start = f"{year:04d}-01-01"
        end = f"{year:04d}-12-31"
        with self._connect() as conn:
            conn.row_factory = sqlite3.Row
            rows = conn.execute(
                f"""
                WITH {_ENTRIES_CTE}
                SELECT day AS date,
                       AVG(mood) AS average_mood,
                       COUNT(*) AS entry_count
                  FROM entries
                 WHERE day BETWEEN :start AND :end
                 GROUP BY day
                 ORDER BY day
                """,
                {"user_id": user_id, "start": start, "end": end},
            ).fetchall()
            days = [dict(row) for row in rows]
            return {"year": year, "days": days, "days_logged": len(days)}

    def monthly_digest(self, user_id: int, year: int, month: int) -> Dict[str, Any]:
        """Month in review: entries logged, average mood, trend versus the
        previous month, top 5 tags by frequency, and the longest streak of
        consecutive logged days within the month.

        ``mood_trend`` is the signed difference between this month's average
        and the previous month's, or null when either month has no entries.
        Built entirely from existing data — no new privacy surface.
        """
        year = int(year)
        month = int(month)
        if not (1 <= month <= 12):
            raise ValueError("month must be between 1 and 12")

        start = f"{year:04d}-{month:02d}-01"
        end = f"{year:04d}-{month:02d}-{calendar.monthrange(year, month)[1]:02d}"
        if month == 1:
            prev_year, prev_month = year - 1, 12
        else:
            prev_year, prev_month = year, month - 1
        prev_start = f"{prev_year:04d}-{prev_month:02d}-01"
        prev_end = (
            f"{prev_year:04d}-{prev_month:02d}-"
            f"{calendar.monthrange(prev_year, prev_month)[1]:02d}"
        )

        with self._connect() as conn:
            conn.row_factory = sqlite3.Row

            summary = conn.execute(
                f"""
                WITH {_ENTRIES_CTE}
                SELECT COUNT(*) AS entries_logged, AVG(mood) AS average_mood
                  FROM entries
                 WHERE day BETWEEN :start AND :end
                """,
                {"user_id": user_id, "start": start, "end": end},
            ).fetchone()

            previous = conn.execute(
                f"""
                WITH {_ENTRIES_CTE}
                SELECT AVG(mood) AS average_mood
                  FROM entries
                 WHERE day BETWEEN :start AND :end
                """,
                {"user_id": user_id, "start": prev_start, "end": prev_end},
            ).fetchone()

            top_tags = conn.execute(
                f"""
                WITH {_ENTRIES_CTE}
                SELECT go.id AS option_id,
                       go.name AS option_name,
                       g.name AS group_name,
                       COUNT(*) AS times_selected
                  FROM entry_selections es
                  JOIN entries e
                        ON e.id = es.entry_id
                       AND e.day BETWEEN :start AND :end
                  JOIN group_options go ON go.id = es.option_id
                  JOIN groups g
                        ON g.id = go.group_id
                       AND g.user_id = :user_id
                 GROUP BY go.id, go.name, g.name
                 ORDER BY times_selected DESC, go.name ASC
                 LIMIT 5
                """,
                {"user_id": user_id, "start": start, "end": end},
            ).fetchall()

            # Gaps-and-islands: consecutive days share the same
            # julianday - row_number value, so the biggest island is the
            # longest streak within the month.
            streak_row = conn.execute(
                f"""
                WITH {_ENTRIES_CTE},
                logged_days AS (
                    SELECT DISTINCT day
                      FROM entries
                     WHERE day BETWEEN :start AND :end
                ),
                runs AS (
                    SELECT julianday(day)
                           - ROW_NUMBER() OVER (ORDER BY day) AS island
                      FROM logged_days
                )
                SELECT COALESCE(MAX(run_length), 0) AS longest
                  FROM (
                        SELECT COUNT(*) AS run_length
                          FROM runs
                         GROUP BY island
                  )
                """,
                {"user_id": user_id, "start": start, "end": end},
            ).fetchone()

        average_mood = summary["average_mood"]
        previous_average = previous["average_mood"]
        mood_trend = None
        if average_mood is not None and previous_average is not None:
            mood_trend = average_mood - previous_average

        return {
            "year": year,
            "month": month,
            "entries_logged": int(summary["entries_logged"] or 0),
            "average_mood": average_mood,
            "previous_average_mood": previous_average,
            "mood_trend": mood_trend,
            "top_tags": [dict(row) for row in top_tags],
            "longest_streak": int(streak_row["longest"] or 0),
        }


__all__ = ["StatisticsMixin", "WEEKDAY_NAMES"]
