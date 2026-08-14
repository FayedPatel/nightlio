from flask import Blueprint, request, jsonify, current_app
import os
from jose import jwt
from datetime import datetime, timedelta, timezone
from werkzeug.security import check_password_hash, generate_password_hash
from api.services.user_service import UserService
from api.utils.auth_middleware import require_auth, get_current_user_id
from api.utils.auth_cookies import set_auth_cookie, clear_auth_cookie
from api.utils.rate_limiter import rate_limit
from api.config import get_config

MIN_PASSWORD_LENGTH = 8


def create_auth_routes(user_service: UserService):
    auth_bp = Blueprint("auth", __name__)

    def generate_jwt_token(user_id: int) -> str:
        """Generate JWT token for user"""
        payload = {
            "user_id": user_id,
            "exp": datetime.now(timezone.utc)
            + timedelta(seconds=current_app.config["JWT_ACCESS_TOKEN_EXPIRES"]),
            "iat": datetime.now(timezone.utc),
        }

        return jwt.encode(
            payload, current_app.config["JWT_SECRET_KEY"], algorithm="HS256"
        )

    def _user_payload(user: dict) -> dict:
        return {
            "id": user["id"],
            "name": user.get("name"),
            "email": user.get("email"),
            "avatar_url": user.get("avatar_url"),
        }

    def _issue_login_response(status_code: int, token: str, user: dict):
        """Build the JSON login response and attach the session cookie.

        The token is returned in the body (unchanged, so existing
        localStorage-based sessions keep working) AND set as an httpOnly
        cookie (the new persistence path). See api/utils/auth_cookies.py.
        """
        response = jsonify({"token": token, "user": _user_payload(user)})
        response.status_code = status_code
        set_auth_cookie(
            response, token, current_app.config["JWT_ACCESS_TOKEN_EXPIRES"]
        )
        return response

    @auth_bp.route("/auth/verify", methods=["POST"])
    @require_auth
    def verify_token():
        """Verify the caller's session (Bearer header or cookie) and return user info.

        Routed through require_auth so a cookie-only session (no
        Authorization header, e.g. after a page reload with no token in
        memory) can also be verified -- the frontend uses this to restore a
        session it only holds via the httpOnly cookie.
        """
        try:
            user = user_service.get_user_by_id(get_current_user_id())
            if not user:
                return jsonify({"error": "User not found"}), 404

            return jsonify({"user": _user_payload(user)})
        except Exception as e:
            current_app.logger.error(f"Token verification error: {str(e)}")
            return jsonify({"error": "Token verification failed"}), 500

    @auth_bp.route("/auth/logout", methods=["POST"])
    def logout():
        """Clear the session cookie.

        Deliberately not behind require_auth: a client must be able to log
        out even with an already-expired or otherwise invalid token, and
        clearing a cookie is not a sensitive action worth failing closed
        over. Idempotent -- safe to call with no session at all.
        """
        response = jsonify({"status": "success"})
        clear_auth_cookie(response)
        return response

    @auth_bp.route("/auth/local/login", methods=["POST"])
    @rate_limit(max_requests=30, window_minutes=1)
    def local_login():
        """Local login.

        With a JSON body containing username and password, verifies the
        credentials against the stored password hash. Without credentials,
        falls back to single-user self-host mode: the default user gets a
        token, but only when OIDC is NOT configured — once an identity
        provider exists, anonymous token issuance must fail closed.
        """
        try:
            # DISABLE_LOCAL_LOGIN hard-refuses the whole endpoint — both the
            # credentialed form and credential-free self-host mode — so an
            # SSO-only deployment has exactly one door. Fail closed if the
            # config cannot be read.
            try:
                local_login_disabled = bool(get_config().DISABLE_LOCAL_LOGIN)
            except Exception:
                local_login_disabled = True
            if local_login_disabled:
                return jsonify({"error": "Local login is disabled"}), 403

            data = request.get_json(silent=True) or {}
            username = data.get("username")
            password = data.get("password")

            if username is not None or password is not None:
                # Credentialed login path.
                if not username or not password:
                    return (
                        jsonify({"error": "Username and password are required"}),
                        400,
                    )

                user = user_service.get_local_user(str(username))
                stored_hash = (
                    user_service.get_password_hash(user["id"]) if user else None
                )
                if not user or not stored_hash:
                    return jsonify({"error": "Invalid credentials"}), 401
                if not check_password_hash(stored_hash, str(password)):
                    return jsonify({"error": "Invalid credentials"}), 401

                user_service.update_last_login(user["id"])
                user_service.record_login_activity(user["id"], method="local")
                token = generate_jwt_token(user["id"])
                return _issue_login_response(200, token, user)

            # Credential-free path: only valid in pure self-host single-user
            # mode. Fail closed — if we cannot prove OIDC is unconfigured,
            # deny.
            try:
                oidc_configured = bool(get_config().oidc_enabled)
            except Exception:
                oidc_configured = True
            if oidc_configured:
                return jsonify({"error": "Credentials required"}), 403

            cfg = get_config()
            default_user_id = cfg.DEFAULT_SELF_HOST_ID

            # Use friendlier display for the self-hosted user
            default_name = os.getenv("SELFHOST_USER_NAME") or "Me"
            default_email = (
                os.getenv("SELFHOST_USER_EMAIL") or f"{default_user_id}@localhost"
            )

            user = user_service.ensure_local_user(
                default_user_id, default_name, default_email
            )

            user_service.record_login_activity(user["id"], method="selfhost")
            token = generate_jwt_token(user["id"])
            return _issue_login_response(200, token, user)
        except Exception as e:
            current_app.logger.error(f"Local login error: {e}")
            return jsonify({"error": "Authentication failed"}), 500

    @auth_bp.route("/auth/local/register", methods=["POST"])
    @require_auth
    @rate_limit(max_requests=10, window_minutes=1)
    def local_register():
        """Create an additional local account.

        Deliberately behind require_auth with family-account semantics: any
        authenticated user of this self-hosted instance may create another
        local account (e.g. a family member), rather than restricting the
        action to an admin role — the app has no role model. Open
        self-registration stays impossible because the endpoint requires a
        valid token.
        """
        try:
            data = request.get_json(silent=True) or {}
            username = data.get("username")
            password = data.get("password")
            email = data.get("email")
            name = data.get("name")

            if not username or not isinstance(username, str) or not username.strip():
                return jsonify({"error": "Username is required"}), 400
            username = username.strip()
            if not password or not isinstance(password, str):
                return jsonify({"error": "Password is required"}), 400
            if len(password) < MIN_PASSWORD_LENGTH:
                return (
                    jsonify(
                        {
                            "error": "Password must be at least "
                            f"{MIN_PASSWORD_LENGTH} characters"
                        }
                    ),
                    400,
                )

            password_hash = generate_password_hash(password)
            user = user_service.register_local_user(
                username,
                password_hash,
                email=email,
                name=name,
            )
            if user is None:
                return jsonify({"error": "Username is already taken"}), 409

            return (
                jsonify({"status": "success", "user": _user_payload(user)}),
                201,
            )
        except Exception as e:
            current_app.logger.error(f"Local register error: {e}")
            return jsonify({"error": "Registration failed"}), 500

    return auth_bp
