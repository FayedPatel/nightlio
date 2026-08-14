import logging
import os
from dataclasses import dataclass
from typing import Optional, Dict, Any
from pathlib import Path
from urllib.parse import urlsplit

logger = logging.getLogger(__name__)

# Optional .env loader: only if python-dotenv is installed.
try:
    from dotenv import load_dotenv  # type: ignore

    _ENV_PATH = Path(__file__).parent.parent / ".env"
    if _ENV_PATH.exists():
        # Load from project root so simple self-host works OOTB.
        load_dotenv(_ENV_PATH)
except Exception:
    # Silently ignore if dotenv is not installed or fails; env vars still work.
    pass


class Config:
    """Existing Flask-style configuration (kept for backward compatibility).

    Note: New features should prefer the typed config via get_config().
    """

    SECRET_KEY = os.environ.get("SECRET_KEY") or "dev-secret-key-change-in-production"

    # Database configuration
    DATABASE_PATH = os.environ.get("DATABASE_PATH") or os.path.join(
        Path(__file__).parent.parent, "data", "nightlio.db"
    )

    # CORS configuration
    CORS_ORIGINS = os.environ.get(
        "CORS_ORIGINS", "http://localhost:5173,https://nightlio.vercel.app"
    ).split(",")

    # JWT configuration (legacy)
    JWT_SECRET_KEY = os.environ.get("JWT_SECRET_KEY") or SECRET_KEY
    JWT_ACCESS_TOKEN_EXPIRES = 3600  # 1 hour


class DevelopmentConfig(Config):
    """Development configuration"""

    DEBUG = True
    TESTING = False


class ProductionConfig(Config):
    """Production configuration"""

    DEBUG = False
    TESTING = False

    # Use Railway's writable directory for database
    DATABASE_PATH = os.environ.get("DATABASE_PATH") or "/tmp/nightlio.db"


class TestingConfig(Config):
    """Testing configuration"""

    DEBUG = True
    TESTING = True
    # Use a file-backed SQLite DB so multiple connections see the same data
    DATABASE_PATH = "/tmp/nightlio_test.db"


# Configuration mapping (legacy app factory still uses this).
config = {
    "development": DevelopmentConfig,
    "production": ProductionConfig,
    "testing": TestingConfig,
    "default": DevelopmentConfig,
}


# --- New typed configuration for optional features ---

# Avoid importing optional helpers with package-level relative import here,
# as this module is also used directly in scripts. The utils package exists,
# and when imported from the app (which sets PYTHONPATH to api/) this works.
try:
    from utils.is_truthy import is_truthy  # type: ignore
except Exception:
    # Tiny fallback in case utils isn't importable for ad-hoc scripts.
    def is_truthy(value: Optional[str]) -> bool:  # type: ignore
        if value is None:
            return False
        return str(value).strip().lower() in {"1", "true", "yes", "on"}


# Secret/JWT keys sign every auth token this app issues (see
# api/routes/auth_routes.py and api/utils/auth_middleware.py). A predictable
# key lets an attacker forge a token for any user_id and bypass auth
# entirely, so any of these known placeholder values -- or anything short
# enough to brute-force/guess -- must never be accepted in production.
# Kept in sync with the compose/env-example placeholders so a self-hoster
# who copies an example file verbatim is still caught.
_KNOWN_WEAK_SECRETS = {
    "dev-secret-key-change-in-production",
    "your-secret-key-change-this",
    "your-jwt-secret-change-this",
    "your-secret-key-change-this-to-something-random-and-secure",
    "your-jwt-secret-change-this-to-something-different-and-secure",
    "changeme",
    "change-me",
    "change_me",
    "secret",
    "password",
    "nightlio",
}

# Below this many characters a value is rejected outright regardless of
# content -- 16 bytes is a conservative floor for an HS256 signing key.
_MIN_SECRET_LENGTH = 16


def is_weak_secret(value: Optional[str]) -> bool:
    """True if ``value`` is missing, a known placeholder, or too short to
    be a real signing key.

    Used to fail closed in production (see ``create_app`` in ``api/app.py``)
    rather than silently signing tokens with a predictable key.
    """
    if not value:
        return True
    normalized = value.strip().lower()
    if not normalized:
        return True
    if normalized in _KNOWN_WEAK_SECRETS:
        return True
    if len(value.strip()) < _MIN_SECRET_LENGTH:
        return True
    return False


@dataclass(frozen=True)
class ConfigData:
    """Typed runtime configuration for optional features.

    Only use these values on the server; never expose secrets to the client.

    If you use SQLAlchemy elsewhere in the project, prefer keeping this
    module's surface the same and swap underlying DB access in services.
    """

    PORT: int

    # Feature flags
    ENABLE_MOOD_MUSIC: bool

    # OIDC single sign-on (any spec-compliant provider; Pocket ID recommended).
    # Discovery document is derived as
    # <OIDC_ISSUER_URL>/.well-known/openid-configuration
    OIDC_ISSUER_URL: Optional[str]
    OIDC_CLIENT_ID: Optional[str]
    OIDC_CLIENT_SECRET: Optional[str]
    OIDC_CALLBACK_URL: Optional[str]

    # Auth
    JWT_SECRET: str
    DEFAULT_SELF_HOST_ID: str = "selfhost_default_user"
    # Optional friendly defaults for the self-hosted user display
    SELFHOST_USER_NAME: Optional[str] = None
    SELFHOST_USER_EMAIL: Optional[str] = None

    # Where the SPA lives, used for the post-SSO redirect back to the
    # frontend. Empty/None means same origin as the API (the standard
    # single-host deployment where nginx serves both).
    FRONTEND_URL: Optional[str] = None

    # Optional signup/registration URL at the identity provider (for
    # Pocket ID: its signup or invite URL). When set and OIDC is enabled,
    # the login page shows a "Create account" link pointing at it. The
    # provider owns registration; Nightlio only renders the link.
    OIDC_SIGNUP_URL: Optional[str] = None

    @property
    def oidc_enabled(self) -> bool:
        """OIDC SSO is considered configured when an issuer URL is set."""
        return bool(self.OIDC_ISSUER_URL and self.OIDC_ISSUER_URL.strip())


_CONFIG_SINGLETON: Optional[ConfigData] = None


def _safe_http_url(raw: Optional[str]) -> Optional[str]:
    """Return the value only if it is an absolute http(s) URL, else None.

    Values from env flow into <a href> on the login page; anything with
    another scheme (file:, javascript:, ...) must never reach the browser.
    """
    value = (raw or "").strip()
    if not value:
        return None
    try:
        parts = urlsplit(value)
    except ValueError:
        parts = None
    if parts and parts.scheme in ("http", "https") and parts.netloc:
        return value
    logger.warning("Ignoring configured URL with unsupported scheme: %r", value)
    return None


def _load_config_from_env() -> ConfigData:
    """Load ConfigData from environment variables.

    - Booleans parsed with is_truthy.
    - Secrets are not logged or exposed.
    - JWT_SECRET falls back to JWT_SECRET_KEY/SECRET_KEY/dev default.
    """

    enable_mood_music = is_truthy(os.getenv("ENABLE_MOOD_MUSIC"))

    # Secrets pulled from env; don't default to empty string.
    jwt_secret = (
        os.getenv("JWT_SECRET")
        or os.getenv("JWT_SECRET_KEY")
        or os.getenv("SECRET_KEY")
        or "dev-secret-key-change-in-production"
    )

    port_str = os.getenv("PORT", "5000")
    try:
        port = int(port_str)
    except (TypeError, ValueError):
        port = 5000

    return ConfigData(
        PORT=port,
        ENABLE_MOOD_MUSIC=enable_mood_music,
        OIDC_ISSUER_URL=os.getenv("OIDC_ISSUER_URL") or None,
        OIDC_CLIENT_ID=os.getenv("OIDC_CLIENT_ID") or None,
        OIDC_CLIENT_SECRET=os.getenv("OIDC_CLIENT_SECRET") or None,
        OIDC_CALLBACK_URL=os.getenv("OIDC_CALLBACK_URL") or None,
        JWT_SECRET=jwt_secret,
        DEFAULT_SELF_HOST_ID=os.getenv("DEFAULT_SELF_HOST_ID")
        or "selfhost_default_user",
        SELFHOST_USER_NAME=os.getenv("SELFHOST_USER_NAME") or "Me",
        SELFHOST_USER_EMAIL=os.getenv("SELFHOST_USER_EMAIL") or None,
        FRONTEND_URL=os.getenv("FRONTEND_URL") or None,
        OIDC_SIGNUP_URL=_safe_http_url(os.getenv("OIDC_SIGNUP_URL")),
    )


def get_config() -> ConfigData:
    """Return a process-wide ConfigData singleton.

    Loads from environment on first access; subsequent calls return the same instance.
    """
    global _CONFIG_SINGLETON
    if _CONFIG_SINGLETON is None:
        _CONFIG_SINGLETON = _load_config_from_env()
    return _CONFIG_SINGLETON


def config_to_public_dict(cfg: ConfigData) -> Dict[str, Any]:
    """Return a safe public configuration for the frontend.

    Only returns non-secret feature flags plus the optional signup link.
    """
    return {
        "enable_oidc": bool(cfg.oidc_enabled),
        "enable_mood_music": bool(cfg.ENABLE_MOOD_MUSIC),
        # The signup link only makes sense when SSO is on; null otherwise,
        # even if the env var is set.
        "signup_url": cfg.OIDC_SIGNUP_URL if cfg.oidc_enabled else None,
    }
