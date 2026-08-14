from typing import List, Optional, Dict
from api.database import MoodDatabase


class GoalService:
    def __init__(self, db: MoodDatabase):
        self.db = db

    def list_goals(self, user_id: int) -> List[Dict]:
        return self.db.get_goals(user_id)

    def create_goal(
        self, user_id: int, title: str, description: str, frequency_per_week: int
    ) -> int:
        return int(self.db.create_goal(user_id, title, description, frequency_per_week))

    def get_goal(self, user_id: int, goal_id: int) -> Optional[Dict]:
        return self.db.get_goal_by_id(user_id, goal_id)

    def update_goal(
        self,
        user_id: int,
        goal_id: int,
        title: Optional[str] = None,
        description: Optional[str] = None,
        frequency_per_week: Optional[int] = None,
    ) -> bool:
        return self.db.update_goal(
            user_id, goal_id, title, description, frequency_per_week
        )

    def delete_goal(self, user_id: int, goal_id: int) -> bool:
        return self.db.delete_goal(user_id, goal_id)

    def increment_progress(
        self, user_id: int, goal_id: int, date_str: Optional[str] = None
    ) -> Optional[Dict]:
        result = self.db.increment_goal_progress(user_id, goal_id, date_str)

        # Log activity only when this call recorded a new completion day;
        # the write is best-effort and must never break the mutation itself.
        try:
            if result and not result.get("already_logged"):
                self.db.add_activity(
                    user_id,
                    "goal_completed",
                    {
                        "goal_id": goal_id,
                        "title": result.get("title"),
                        "date": result.get("logged_date"),
                    },
                )
        except Exception:
            pass

        return result

    def get_completions(
        self,
        user_id: int,
        goal_id: int,
        start_date: Optional[str] = None,
        end_date: Optional[str] = None,
    ):
        return self.db.get_goal_completions(user_id, goal_id, start_date, end_date)
