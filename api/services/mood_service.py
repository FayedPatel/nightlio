from datetime import date as _date
from typing import List, Optional, Dict
from api.database import MoodDatabase
from api.models.mood_entry import MoodEntry


class MoodService:
    def __init__(self, db: MoodDatabase):
        self.db = db

    def _log_activity(self, user_id: int, event_type: str, metadata: Dict) -> None:
        """Best-effort activity write; must never break the mutation."""
        try:
            self.db.add_activity(user_id, event_type, metadata)
        except Exception:
            pass

    def create_mood_entry(
        self,
        user_id: int,
        date: str,
        mood: int,
        content: str,
        time: Optional[str] = None,
        selected_options: Optional[List[int]] = None,
    ) -> Dict:
        """Create a new mood entry and check for achievements"""
        if not (1 <= mood <= 5):
            raise ValueError("Mood must be between 1 and 5")

        if not content.strip():
            raise ValueError("Content cannot be empty")

        entry_id = self.db.add_mood_entry(
            user_id, date, mood, content, time, selected_options
        )

        self._log_activity(
            user_id, "entry_created", {"entry_id": entry_id, "date": date}
        )

        # Check for new achievements
        new_achievements = self.db.check_achievements(user_id)
        for achievement_type in new_achievements:
            self._log_activity(
                user_id,
                "achievement_unlocked",
                {"achievement_type": achievement_type},
            )

        return {"entry_id": entry_id, "new_achievements": new_achievements}

    def get_all_entries(self, user_id: int) -> List[Dict]:
        """Get all mood entries for a user"""
        return self.db.get_all_mood_entries(user_id)

    def get_entries_by_date_range(
        self, user_id: int, start_date: str, end_date: str
    ) -> List[Dict]:
        """Get mood entries within a date range for a user"""
        return self.db.get_mood_entries_by_date_range(user_id, start_date, end_date)

    def get_entry_by_id(self, user_id: int, entry_id: int) -> Optional[Dict]:
        """Get a specific mood entry by ID for a user"""
        return self.db.get_mood_entry_by_id(user_id, entry_id)

    def update_entry(
        self,
        user_id: int,
        entry_id: int,
        mood: Optional[int] = None,
        content: Optional[str] = None,
        date: Optional[str] = None,
        time: Optional[str] = None,
        selected_options: Optional[List[int]] = None,
    ) -> Optional[Dict]:
        """Update an existing mood entry for a user and return the updated record"""
        if mood is not None and not (1 <= mood <= 5):
            raise ValueError("Mood must be between 1 and 5")

        if content is not None and not content.strip():
            raise ValueError("Content cannot be empty")

        updated = self.db.update_mood_entry(
            user_id,
            entry_id,
            mood=mood,
            content=content,
            date=date,
            time=time,
            selected_options=selected_options,
        )

        if not updated:
            return None

        entry = self.db.get_mood_entry_by_id(user_id, entry_id)
        if not entry:
            return None

        self._log_activity(user_id, "entry_edited", {"entry_id": entry_id})

        selections = self.db.get_entry_selections(entry_id)
        entry["selections"] = selections
        return entry

    def delete_entry(self, user_id: int, entry_id: int) -> bool:
        """Delete a mood entry for a user"""
        deleted = self.db.delete_mood_entry(user_id, entry_id)
        if deleted:
            self._log_activity(user_id, "entry_deleted", {"entry_id": entry_id})
        return deleted

    def get_statistics(self, user_id: int) -> Dict:
        """Get mood statistics for a user"""
        # Track statistics view for achievements (Data Lover)
        try:
            self.db.increment_stats_view(user_id)
        except Exception:
            # Metrics should not break stats
            pass
        stats = self.db.get_mood_statistics(user_id)
        mood_counts = self.db.get_mood_counts(user_id)
        current_streak = self.db.get_current_streak(user_id)

        return {
            "statistics": stats,
            "mood_distribution": mood_counts,
            "current_streak": current_streak,
        }

    def get_extended_statistics(self, user_id: int) -> Dict:
        """Bundle the extended aggregations plus the current month's digest.

        Deliberately separate from get_statistics so the existing response
        shape (and its consumers) stays untouched.
        """
        today = _date.today()
        return {
            "rolling_averages": self.db.rolling_averages(user_id),
            "weekday_averages": self.db.weekday_averages(user_id),
            "mood_volatility": self.db.mood_volatility(user_id),
            "tag_correlations": self.db.tag_correlations(user_id),
            "goal_correlations": self.db.goal_correlations(user_id),
            "monthly_digest": self.db.monthly_digest(user_id, today.year, today.month),
        }

    def get_heatmap(self, user_id: int, year: int) -> Dict:
        """Calendar heatmap data for a year: avg mood + count per logged day."""
        return self.db.heatmap(user_id, year)

    def get_monthly_digest(self, user_id: int, year: int, month: int) -> Dict:
        """Server-computed month in review for an explicit year/month."""
        return self.db.monthly_digest(user_id, year, month)

    def get_current_streak(self, user_id: int) -> int:
        """Get current consecutive days streak for a user"""
        return self.db.get_current_streak(user_id)

    def get_entry_selections(self, user_id: int, entry_id: int) -> List[Dict]:
        """Get selected options for an entry (with user verification)"""
        # First verify the entry belongs to the user
        entry = self.db.get_mood_entry_by_id(user_id, entry_id)
        if not entry:
            return []
        return self.db.get_entry_selections(entry_id)
