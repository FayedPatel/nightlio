from flask import Blueprint, request, jsonify
from api.services.group_service import GroupService
from api.utils.auth_middleware import require_auth, get_current_user_id


def create_group_routes(group_service: GroupService):
    group_bp = Blueprint("group", __name__)

    @group_bp.route("/groups", methods=["GET"])
    @require_auth
    def get_groups():
        try:
            user_id = get_current_user_id()
            if user_id is None:
                return jsonify({"error": "Unauthorized"}), 401
            groups = group_service.get_all_groups(user_id)
            return jsonify(groups)

        except Exception as e:
            return jsonify({"error": str(e)}), 500

    @group_bp.route("/groups", methods=["POST"])
    @require_auth
    def create_group():
        try:
            user_id = get_current_user_id()
            if user_id is None:
                return jsonify({"error": "Unauthorized"}), 401
            data = request.json
            name = data.get("name")

            if not name:
                return jsonify({"error": "Group name is required"}), 400

            group_id = group_service.create_group(user_id, name)

            return (
                jsonify(
                    {
                        "status": "success",
                        "group_id": group_id,
                        "message": "Group created successfully",
                    }
                ),
                201,
            )

        except ValueError as e:
            return jsonify({"error": str(e)}), 400
        except Exception as e:
            return jsonify({"error": str(e)}), 500

    @group_bp.route("/groups/<int:group_id>/options", methods=["POST"])
    @require_auth
    def create_group_option(group_id):
        try:
            user_id = get_current_user_id()
            if user_id is None:
                return jsonify({"error": "Unauthorized"}), 401
            data = request.json
            name = data.get("name")

            if not name:
                return jsonify({"error": "Option name is required"}), 400

            option_id = group_service.create_group_option(user_id, group_id, name)

            return (
                jsonify(
                    {
                        "status": "success",
                        "option_id": option_id,
                        "message": "Option created successfully",
                    }
                ),
                201,
            )

        except ValueError as e:
            return jsonify({"error": str(e)}), 400
        except Exception as e:
            return jsonify({"error": str(e)}), 500

    @group_bp.route("/groups/<int:group_id>", methods=["DELETE"])
    @require_auth
    def delete_group(group_id):
        try:
            user_id = get_current_user_id()
            if user_id is None:
                return jsonify({"error": "Unauthorized"}), 401
            success = group_service.delete_group(user_id, group_id)

            if success:
                return jsonify(
                    {"status": "success", "message": "Group deleted successfully"}
                )
            else:
                return jsonify({"error": "Group not found"}), 404

        except Exception as e:
            return jsonify({"error": str(e)}), 500

    @group_bp.route("/options/<int:option_id>", methods=["DELETE"])
    @require_auth
    def delete_option(option_id):
        try:
            user_id = get_current_user_id()
            if user_id is None:
                return jsonify({"error": "Unauthorized"}), 401
            success = group_service.delete_group_option(user_id, option_id)

            if success:
                return jsonify(
                    {"status": "success", "message": "Option deleted successfully"}
                )
            else:
                return jsonify({"error": "Option not found"}), 404

        except Exception as e:
            return jsonify({"error": str(e)}), 500

    return group_bp
