import os
import sys
from flask import Flask
from flask_cors import CORS
from dotenv import load_dotenv
from pathlib import Path

# Load environment variables from the project root (parent directory)
env_path = Path(__file__).parent.parent / ".env"
load_dotenv(env_path)

# Support running as a package (api.*) and from within the api/ directory
try:
    from api.database import MoodDatabase
    from api.services.mood_service import MoodService
    from api.services.goal_service import GoalService
    from api.services.group_service import GroupService
    from api.services.user_service import UserService
    from api.services.achievement_service import AchievementService
    from api.routes.mood_routes import create_mood_routes
    from api.routes.goal_routes import create_goal_routes
    from api.routes.group_routes import create_group_routes
    from api.routes.auth_routes import create_auth_routes
    from api.routes.misc_routes import create_misc_routes
    from api.routes.config_routes import create_config_routes
    from api.routes.achievement_routes import create_achievement_routes
    from api.routes.activity_routes import create_activity_routes
    from api.utils.error_handlers import setup_error_handlers
    from api.utils.security_headers import add_security_headers
    from api.services.mus_service import MusicService
    from api.routes.mus_routes import create_music_routes
except Exception:  # fallback for running from inside api/
    from database import MoodDatabase
    from services.mood_service import MoodService
    from services.goal_service import GoalService
    from services.group_service import GroupService
    from services.user_service import UserService
    from services.achievement_service import AchievementService
    from routes.mood_routes import create_mood_routes
    from routes.goal_routes import create_goal_routes
    from routes.group_routes import create_group_routes
    from routes.auth_routes import create_auth_routes
    from routes.misc_routes import create_misc_routes
    from routes.config_routes import create_config_routes
    from routes.achievement_routes import create_achievement_routes
    from routes.activity_routes import create_activity_routes
    from utils.error_handlers import setup_error_handlers
    from utils.security_headers import add_security_headers
    from services.mus_service import MusicService
    from routes.mus_routes import create_music_routes

def create_app(config_name="default"):
    """Application factory pattern"""
    try:
        from api.config import config as config_map
        from api.config import get_config, is_weak_secret
    except Exception:
        # Fallback when running from within api/ directory
        from config import config as config_map  # type: ignore[import-not-found]
        from config import get_config, is_weak_secret  # type: ignore[import-not-found]

    app = Flask(__name__)
    app.config.from_object(config_map[config_name])

    # Load typed runtime config and align secrets
    cfg = None
    try:
        cfg = get_config()
        if getattr(cfg, "JWT_SECRET", None):
            app.config["JWT_SECRET_KEY"] = getattr(cfg, "JWT_SECRET")
    except Exception:
        cfg = None  # fallback if typed config fails

    # Fail closed in production rather than silently signing every JWT
    # (local login, OIDC login) with a missing/placeholder/short key --
    # that key is the only thing standing between an attacker and a forged
    # session for any user_id. Deliberately scoped to config_name ==
    # "production" only: dev/testing keep working with no secret set at
    # all, matching prior behavior. See api/config.py:is_weak_secret and
    # docker-compose.yml / docker-compose.prod.yml, which now require
    # SECRET_KEY and JWT_SECRET to be set before `up` will even start the
    # container -- this check is defense in depth for any entrypoint that
    # bypasses compose (bare `docker run`, Railway, manual gunicorn).
    # Checked independently: app.config["JWT_SECRET_KEY"] may already be
    # strong (derived from a properly-set JWT_SECRET env var) while
    # app.config["SECRET_KEY"] -- Flask's own key, still assigned from
    # Config.SECRET_KEY's dev-fallback whenever the SECRET_KEY env var
    # itself was left unset -- stays the well-known default. Nothing in
    # this codebase signs with Flask's SECRET_KEY today, but it is
    # attacker-visible config surface (extensions, future features) and
    # the finding this check closes is specifically about that fallback
    # existing at all, not only about whether it happens to be reachable
    # by today's call sites. Both must be strong, not just one.
    if config_name == "production" and (
        is_weak_secret(app.config.get("SECRET_KEY"))
        or is_weak_secret(app.config.get("JWT_SECRET_KEY"))
    ):
        raise RuntimeError(
            "Refusing to start: SECRET_KEY/JWT_SECRET is missing, a known "
            "placeholder, or shorter than 16 characters. Set SECRET_KEY "
            "and JWT_SECRET in your .env to distinct, random values, e.g.: "
            "openssl rand -hex 32"
        )

    # supports_credentials=True is required for the httpOnly session cookie
    # (api/utils/auth_cookies.py) to be sent/accepted cross-origin (the
    # common self-host shape where the SPA and API are on different
    # ports/subdomains, e.g. dev, or FRONTEND_URL deployments). Per
    # flask-cors, supports_credentials and a wildcard '*' origin are mutually
    # exclusive (it raises) -- CORS_ORIGINS must therefore stay an explicit
    # list of trusted origins, never '*'. Bearer-token clients are
    # unaffected either way.
    CORS(app, origins=app.config["CORS_ORIGINS"], supports_credentials=True)

    # Behind a trusted reverse proxy (TRUST_PROXY_HEADERS=1) honor the
    # X-Forwarded-* headers so externally generated URLs (notably the OIDC
    # redirect_uri from url_for(..., _external=True)) carry the real public
    # scheme/host instead of the proxy-internal http one, and
    # request.is_secure reflects the TLS termination upstream. Same opt-in
    # flag the rate limiter and cookie layer already use — never enabled by
    # default, because these headers are client-spoofable without a proxy
    # that strips them.
    try:
        from api.utils.is_truthy import is_truthy as _is_truthy
    except ImportError:  # pragma: no cover - running from inside api/
        from utils.is_truthy import is_truthy as _is_truthy
    if _is_truthy(os.getenv("TRUST_PROXY_HEADERS", "")):
        from werkzeug.middleware.proxy_fix import ProxyFix

        app.wsgi_app = ProxyFix(app.wsgi_app, x_for=1, x_proto=1, x_host=1)

    # Rely on flask-cors to handle CORS and automatic OPTIONS responses per route

    # Setup error handlers
    setup_error_handlers(app)

    # Add security headers
    add_security_headers(app)

    # Initialize database
    db = MoodDatabase(app.config.get("DATABASE_PATH"))

    # Age-based activity retention; a failed prune must never block startup.
    try:
        db.prune_activity(90)
    except Exception:
        pass

    # Initialize services
    mood_service = MoodService(db)
    group_service = GroupService(db)
    goal_service = GoalService(db)
    user_service = UserService(db)
    achievement_service = AchievementService(db)

    # Initialize music service
    music_service = MusicService(db)

    # Register blueprints
    app.register_blueprint(create_auth_routes(user_service), url_prefix="/api")
    app.register_blueprint(create_mood_routes(mood_service), url_prefix="/api")
    app.register_blueprint(create_group_routes(group_service), url_prefix="/api")
    app.register_blueprint(create_goal_routes(goal_service), url_prefix="/api")
    app.register_blueprint(
        create_achievement_routes(achievement_service), url_prefix="/api"
    )
    app.register_blueprint(create_misc_routes(), url_prefix="/api")
    app.register_blueprint(create_config_routes(), url_prefix="/api")
    app.register_blueprint(create_activity_routes(db), url_prefix="/api")

    # Expose services for optional blueprints (e.g., OAuth) to reuse
    try:
        if not hasattr(app, "extensions") or app.extensions is None:  # type: ignore[attr-defined]
            app.extensions = {}  # type: ignore[attr-defined]
        app.extensions["user_service"] = user_service  # type: ignore[attr-defined]
    except Exception:
        pass

    # Conditional feature registration (lazy imports)
    if cfg is None:
        try:
            cfg = get_config()
        except Exception:
            cfg = None

    # Register music blueprint only when enabled.
    if cfg and getattr(cfg, "ENABLE_MOOD_MUSIC", False):
        app.register_blueprint(create_music_routes(music_service), url_prefix="/api")

    if cfg and getattr(cfg, "oidc_enabled", False):
        try:
            # Registered only when enabled; module can lazy-import heavy deps.
            from api.auth.oauth import oauth_bp  # type: ignore

            app.register_blueprint(oauth_bp, url_prefix="/api")
        except Exception as e:
            try:
                from auth.oauth import oauth_bp  # type: ignore

                app.register_blueprint(oauth_bp, url_prefix="/api")
            except Exception as e2:
                if app.debug:
                    print(
                        f"[warn] OIDC is configured but oauth blueprint not available: {e} / {e2}"
                    )

    # Web3 functionality removed from the application

    # Debug: Print all registered routes
    if app.debug:
        print("Registered routes:")
        for rule in app.url_map.iter_rules():
            methods = sorted(list(getattr(rule, "methods", []) or []))
            print(f"  {rule.rule} -> {rule.endpoint} [{', '.join(methods)}]")

    return app


if __name__ == "__main__":
    # Ensure project root is on sys.path when running this file directly
    root = str(Path(__file__).parent)
    if root not in sys.path:
        sys.path.insert(0, root)

    # Get environment from Railway or default to development
    env = os.getenv("APP_ENV") or os.getenv("RAILWAY_ENVIRONMENT", "development")
    app = create_app(env)

    print("Starting Flask app...")
    print(f"Environment: {env}")
    try:
        from api.config import get_config as _get_config
    except Exception:
        from config import get_config as _get_config  # type: ignore
    print(f"OIDC configured: {'Set' if _get_config().oidc_enabled else 'Missing'}")

    port = int(os.getenv("PORT", 5000))
    print(f"Starting Flask app on port {port}")
    if env == "production":
        print("WARNING: Running in production mode with Flask development server!")
        print("For production deployments, use: gunicorn wsgi:application")
        print("Or run: python3 wsgi.py")
        app.run(host="::", port=port, debug=False)
    else:
        print("Using Flask development server (debug mode on)")
        app.run(debug=True, host="127.0.0.1", port=port)
