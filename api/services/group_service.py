from typing import List, Dict
from api.database import MoodDatabase


class GroupService:
    """Group CRUD scoped to the authenticated user.

    Every method requires an explicit user_id; the database mixin's
    default-user fallback is a compatibility shim, not something the
    service layer may rely on.
    """

    def __init__(self, db: MoodDatabase):
        self.db = db

    def get_all_groups(self, user_id: int) -> List[Dict]:
        """Get the user's groups with their options"""
        return self.db.get_all_groups(user_id=user_id)

    def create_group(self, user_id: int, name: str) -> int:
        """Create a new group owned by the user"""
        if not name.strip():
            raise ValueError("Group name cannot be empty")

        return self.db.create_group(name.strip(), user_id=user_id)

    def create_group_option(self, user_id: int, group_id: int, name: str) -> int:
        """Create a new option for a group the user owns"""
        if not name.strip():
            raise ValueError("Option name cannot be empty")

        return self.db.create_group_option(group_id, name.strip(), user_id=user_id)

    def delete_group(self, user_id: int, group_id: int) -> bool:
        """Delete a group the user owns, with all its options"""
        return self.db.delete_group(group_id, user_id=user_id)

    def delete_group_option(self, user_id: int, option_id: int) -> bool:
        """Delete a group option belonging to one of the user's groups"""
        return self.db.delete_group_option(option_id, user_id=user_id)
