"""Per-user activity feed endpoint.

Keyset-paginated on the activity row id: pass ``before=<id>`` (the
``next_cursor`` from the previous page) to fetch older events. ``limit`` is
clamped server-side (1..200).
"""

from flask import Blueprint, jsonify, request

# Support running as a package (api.*) and from within the api/ directory
try:
    from api.database import MoodDatabase
    from api.utils.auth_middleware import require_auth, get_current_user_id
except ImportError:  # pragma: no cover - fallback for running from inside api/
    from database import MoodDatabase  # type: ignore
    from utils.auth_middleware import require_auth, get_current_user_id  # type: ignore


def create_activity_routes(db: MoodDatabase):
    activity_bp = Blueprint("activity", __name__)

    @activity_bp.route("/activity", methods=["GET"])
    @require_auth
    def get_activity():
        try:
            user_id = get_current_user_id()
            if user_id is None:
                return jsonify({"error": "Unauthorized"}), 401

            before_raw = request.args.get("before")
            limit_raw = request.args.get("limit")

            before = None
            if before_raw is not None:
                try:
                    before = int(before_raw)
                except (TypeError, ValueError):
                    return jsonify({"error": "before must be an integer"}), 400

            limit = 50
            if limit_raw is not None:
                try:
                    limit = int(limit_raw)
                except (TypeError, ValueError):
                    return jsonify({"error": "limit must be an integer"}), 400

            activities = db.get_activity(user_id, before=before, limit=limit)

            # The database clamps limit to 1..200; a full page means there may
            # be older rows, so hand back the last id as the next cursor.
            clamped_limit = max(1, min(limit, 200))
            next_cursor = (
                activities[-1]["id"] if len(activities) == clamped_limit else None
            )

            return jsonify({"activities": activities, "next_cursor": next_cursor})
        except Exception:
            return jsonify({"error": "Failed to load activity"}), 500

    return activity_bp
