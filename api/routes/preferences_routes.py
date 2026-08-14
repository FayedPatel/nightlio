from flask import Blueprint, request, jsonify

from api.services.user_service import UserService
from api.utils.auth_middleware import require_auth, get_current_user_id

# Theme ids must match src/contexts/ThemeContext.jsx.
ALLOWED_THEMES = {"default", "light", "dark", "synthwave"}


def create_preferences_routes(user_service: UserService):
    preferences_bp = Blueprint("preferences", __name__)

    @preferences_bp.route("/preferences", methods=["GET"])
    @require_auth
    def get_preferences():
        try:
            user_id = get_current_user_id()
            if user_id is None:
                return jsonify({"error": "Unauthorized"}), 401
            return jsonify({"theme": user_service.get_theme_preference(user_id)})
        except Exception as e:
            return jsonify({"error": str(e)}), 500

    @preferences_bp.route("/preferences", methods=["PUT"])
    @require_auth
    def update_preferences():
        try:
            user_id = get_current_user_id()
            if user_id is None:
                return jsonify({"error": "Unauthorized"}), 401
            data = request.get_json(silent=True) or {}
            theme = data.get("theme")
            if theme not in ALLOWED_THEMES:
                return (
                    jsonify(
                        {
                            "error": "theme must be one of: "
                            + ", ".join(sorted(ALLOWED_THEMES))
                        }
                    ),
                    400,
                )
            user_service.set_theme_preference(user_id, theme)
            return jsonify({"status": "success", "theme": theme})
        except Exception as e:
            return jsonify({"error": str(e)}), 500

    return preferences_bp
