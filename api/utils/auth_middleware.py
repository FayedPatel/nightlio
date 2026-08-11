from functools import wraps
from flask import request, jsonify, current_app, g
from jose import jwt, JWTError

try:
    from api.utils.auth_cookies import (
        COOKIE_NAME,
        CSRF_HEADER_NAME,
        CSRF_HEADER_VALUE,
    )
except ImportError:  # pragma: no cover - fallback for running from inside api/
    from utils.auth_cookies import (  # type: ignore
        COOKIE_NAME,
        CSRF_HEADER_NAME,
        CSRF_HEADER_VALUE,
    )

# Methods that mutate state. Cookie-authenticated requests using one of
# these are subject to the CSRF checks below; Bearer-token requests never
# are (see module docstring in api/utils/auth_cookies.py).
_STATE_CHANGING_METHODS = {"POST", "PUT", "PATCH", "DELETE"}


def _extract_token():
    """Return (token, is_header_auth) from the request, or (None, False).

    Authorization: Bearer wins when present, so existing localStorage-based
    sessions keep working unchanged. Falling back to the httpOnly cookie
    only when no header is present means a header can never be
    "downgraded" to cookie auth by a malicious page that also happens to
    have a valid cookie.
    """
    auth_header = request.headers.get("Authorization")
    if auth_header and auth_header.startswith("Bearer "):
        return auth_header.split(" ", 1)[1], True
    cookie_token = request.cookies.get(COOKIE_NAME)
    if cookie_token:
        return cookie_token, False
    return None, False


def _csrf_check_failure():
    """Return a Flask response tuple if the cookie-auth CSRF checks fail.

    Only reached for cookie-authenticated state-changing requests. Two
    independent checks, both required:
      - Content-Type must be application/json: a plain HTML form cannot
        submit a cross-site request with this content type.
      - X-Requested-With: nightlio must be present: a custom header forces
        the browser to send a CORS preflight, which our CORS config
        (explicit origins, no wildcard) blocks for foreign origins.
    Bearer-token requests skip this entirely: a token that only lives in
    another origin's localStorage cannot be attached by a cross-site page,
    so there is nothing for CSRF to exploit there.
    """
    content_type = (request.content_type or "").split(";")[0].strip().lower()
    if content_type != "application/json":
        return jsonify({"error": "Content-Type must be application/json"}), 403
    if request.headers.get(CSRF_HEADER_NAME) != CSRF_HEADER_VALUE:
        return jsonify({"error": "Missing required request header"}), 403
    return None


def require_auth(f):
    """Decorator to require JWT authentication.

    Accepts either an ``Authorization: Bearer`` header or the httpOnly
    ``nightlio_token`` cookie (header takes priority). Cookie-authenticated
    state-changing requests additionally pass a CSRF check; see
    ``_csrf_check_failure``.
    """

    @wraps(f)
    def decorated_function(*args, **kwargs):
        # Allow CORS preflight requests (OPTIONS) to succeed without auth
        # Return 204 directly so the route handler isn't invoked.
        if request.method == "OPTIONS":
            return ("", 204)

        token, header_auth = _extract_token()
        if not token:
            return jsonify({"error": "Authorization header required"}), 401

        if not header_auth and request.method in _STATE_CHANGING_METHODS:
            csrf_failure = _csrf_check_failure()
            if csrf_failure:
                return csrf_failure

        try:
            payload = jwt.decode(
                token, current_app.config["JWT_SECRET_KEY"], algorithms=["HS256"]
            )

            # Store user_id in Flask's g object for use in the route
            g.user_id = payload["user_id"]

        except JWTError as e:
            if "expired" in str(e).lower():
                return jsonify({"error": "Token expired"}), 401
            else:
                return jsonify({"error": "Invalid token"}), 401
        except Exception as e:
            current_app.logger.error(f"Auth middleware error: {str(e)}")
            return jsonify({"error": "Authentication failed"}), 500

        return f(*args, **kwargs)

    return decorated_function


def get_current_user_id():
    """Get current authenticated user ID from Flask g object"""
    return getattr(g, "user_id", None)
