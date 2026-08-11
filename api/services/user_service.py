import sqlite3
from typing import Optional, Dict
from api.database import MoodDatabase


class UserService:
    def __init__(self, db: MoodDatabase):
        self.db = db

    def get_user_by_id(self, user_id: int) -> Optional[Dict]:
        """Get user by ID"""
        return self.db.get_user_by_id(user_id)

    def update_last_login(self, user_id: int):
        """Update user's last login timestamp"""
        self.db.update_user_last_login(user_id)

    # --- OIDC ---------------------------------------------------------------
    def handle_oidc_login(
        self,
        subject: str,
        email: Optional[str],
        name: Optional[str],
        avatar_url: Optional[str] = None,
    ) -> Optional[Dict]:
        """Upsert a user from a validated OIDC identity and return the user.

        Keyed by (provider="oidc", external_id=subject). On the user's first
        login the default tag groups are seeded and a login activity event is
        recorded; on every login last_login is refreshed by the upsert.
        """
        first_login = self.db.get_user_by_provider("oidc", subject) is None
        user = self.db.upsert_oidc_user(
            external_id=subject,
            email=email,
            name=name,
            avatar_url=avatar_url,
            provider="oidc",
        )
        if user and first_login:
            self.db.ensure_default_groups_for_user(user["id"])
        if user:
            self.record_login_activity(user["id"], method="oidc")
        return user

    # --- Local password auth ------------------------------------------------
    def get_local_user(self, username: str) -> Optional[Dict]:
        """Return the local-auth user for a username, or None."""
        return self.db.get_user_by_provider("local", username)

    def get_password_hash(self, user_id: int) -> Optional[str]:
        """Return the stored password hash for a user, or None."""
        return self.db.get_user_password_hash(user_id)

    def register_local_user(
        self,
        username: str,
        password_hash: str,
        email: Optional[str] = None,
        name: Optional[str] = None,
    ) -> Optional[Dict]:
        """Create a local user (with default groups) and return it.

        Returns None when the username is already taken.
        """
        try:
            user_id = self.db.create_local_user(
                username, password_hash, email=email, name=name
            )
        except sqlite3.IntegrityError:
            return None
        self.db.ensure_default_groups_for_user(user_id)
        return self.db.get_user_by_id(user_id)

    # --- Activity -----------------------------------------------------------
    def record_login_activity(self, user_id: int, method: str) -> None:
        """Best-effort login event; never allowed to break the login itself."""
        try:
            self.db.add_activity(user_id, "login", {"method": method})
        except Exception:
            pass

    # Self-host local user provisioning
    def ensure_local_user(
        self,
        default_user_id: str,
        default_name: Optional[str] = None,
        default_email: Optional[str] = None,
    ) -> Dict:
        """Ensure a self-host single-user exists and return it.

        - Uses the google_id column to store a stable synthetic identifier.
        - Always upserts with provided friendly name/email to improve UX.
        - Remains idempotent and returns the full user row.
        """
        name = default_name or "Me"
        email = default_email or f"{default_user_id}@localhost"

        # Upsert ensures we set a friendly display name even if a previous
        # row exists with the raw id as name.
        user = self.db.upsert_user_by_google_id(
            google_id=default_user_id,
            email=email,
            name=name,
            avatar_url=None,
        )
        return user
