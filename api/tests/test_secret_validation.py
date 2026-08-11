"""Regression tests for the production fail-closed weak-secret check.

``api/app.py:create_app`` refuses to start with ``config_name="production"``
when ``SECRET_KEY``/``JWT_SECRET`` is missing, a known placeholder, or too
short -- see ``api/config.py:is_weak_secret``. A predictable key would let
an attacker forge a JWT for any ``user_id`` (both
``api/routes/auth_routes.py`` and ``api/utils/auth_middleware.py`` sign and
verify with this same key), so this is fail-closed by design: the process
must not come up with a guessable secret in production.

These run in a subprocess with a tightly controlled environment.
``Config.SECRET_KEY``/``Config.JWT_SECRET_KEY`` are class attributes
evaluated once at module import time, and ``get_config()`` caches a
process-wide singleton -- testing this in-process would leak state into
(or be leaked into by) other tests in the same pytest run. A subprocess
also proves the real container entrypoint's failure mode: a missing/weak
secret must produce a nonzero exit, not just a caught-and-ignored warning.
"""

import os
import subprocess
import sys
from pathlib import Path

# api/tests/test_secret_validation.py -> api/tests -> api -> repo root.
ROOT = Path(__file__).resolve().parent.parent.parent

# These must be absent (not just falsy) for the "no secret set at all"
# cases, and OIDC/proxy vars are cleared so a developer's real .env
# (loaded via python-dotenv in api/app.py) can't influence the outcome.
_UNSET_FOR_CLEAN_ENV = (
    "SECRET_KEY",
    "JWT_SECRET",
    "JWT_SECRET_KEY",
    "OIDC_ISSUER_URL",
    "OIDC_CLIENT_ID",
    "OIDC_CLIENT_SECRET",
    "OIDC_CALLBACK_URL",
    "FRONTEND_URL",
    "TRUST_PROXY_HEADERS",
)

_CREATE_APP_PRODUCTION = (
    "from api.app import create_app\n"
    "create_app('production')\n"
    "print('APP_CREATED_OK')\n"
)


def _run(code: str, extra_env: dict) -> subprocess.CompletedProcess:
    env = dict(os.environ)
    # Blank, don't delete: api/app.py and api/config.py both call
    # load_dotenv() against the repo-root .env unconditionally at import
    # time. python-dotenv's load_dotenv() only ever fills in a variable
    # that is *absent* from os.environ -- it never overrides one that is
    # merely empty. A machine with a real .env at the repo root (e.g. a
    # self-hoster's own checkout, or this one) would otherwise have
    # SECRET_KEY/JWT_SECRET silently refilled with real values by
    # load_dotenv the moment this subprocess deletes them, defeating the
    # "nothing set at all" scenario these tests exist to cover. Same
    # pattern already used by api/tests/conftest.py for the OIDC/proxy
    # vars, for the same reason.
    for var in _UNSET_FOR_CLEAN_ENV:
        env[var] = ""
    env.update(extra_env)
    env["PYTHONPATH"] = str(ROOT)
    return subprocess.run(
        [sys.executable, "-c", code],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
    )


def test_production_refuses_to_start_with_no_secret():
    result = _run(_CREATE_APP_PRODUCTION, {})
    assert result.returncode != 0
    assert "SECRET_KEY/JWT_SECRET" in result.stderr


def test_production_refuses_to_start_with_known_placeholder():
    result = _run(
        _CREATE_APP_PRODUCTION,
        {
            "SECRET_KEY": "your-secret-key-change-this",
            "JWT_SECRET": "your-jwt-secret-change-this",
        },
    )
    assert result.returncode != 0
    assert "SECRET_KEY/JWT_SECRET" in result.stderr


def test_production_refuses_to_start_with_short_secret():
    result = _run(
        _CREATE_APP_PRODUCTION,
        {"SECRET_KEY": "short", "JWT_SECRET": "short"},
    )
    assert result.returncode != 0
    assert "SECRET_KEY/JWT_SECRET" in result.stderr


def test_production_starts_with_strong_secret(tmp_path):
    db_path = tmp_path / "nightlio.db"
    result = _run(
        _CREATE_APP_PRODUCTION,
        {
            "SECRET_KEY": "a" * 32,
            "JWT_SECRET": "b" * 32,
            "DATABASE_PATH": str(db_path),
        },
    )
    assert result.returncode == 0, result.stderr
    assert "APP_CREATED_OK" in result.stdout


def test_production_refuses_to_start_with_secret_key_unset_but_jwt_secret_strong():
    # Regression: app.config["JWT_SECRET_KEY"] is overwritten inside
    # create_app with the typed config's JWT_SECRET, which can be strong
    # on its own (set directly, independent of SECRET_KEY). That must not
    # let the check pass while Flask's own app.config["SECRET_KEY"] --
    # populated from Config.SECRET_KEY's dev fallback whenever the
    # SECRET_KEY env var itself is unset -- is still the public
    # "dev-secret-key-change-in-production" default. Both keys are
    # checked independently; see api/app.py:create_app.
    result = _run(
        _CREATE_APP_PRODUCTION,
        {"JWT_SECRET": "b" * 32},
    )
    assert result.returncode != 0
    assert "SECRET_KEY/JWT_SECRET" in result.stderr


def test_production_starts_with_strong_secret_key_only(tmp_path):
    # JWT_SECRET_KEY legitimately derives from SECRET_KEY when JWT_SECRET
    # is not set separately (see api/config.py's fallback chain) -- a
    # single strong SECRET_KEY must be enough on its own.
    db_path = tmp_path / "nightlio.db"
    result = _run(
        _CREATE_APP_PRODUCTION,
        {"SECRET_KEY": "a" * 32, "DATABASE_PATH": str(db_path)},
    )
    assert result.returncode == 0, result.stderr
    assert "APP_CREATED_OK" in result.stdout


def test_development_still_starts_with_no_secret_set(tmp_path):
    # Dev/testing must keep working with nothing set: the check is scoped
    # to config_name == "production" only, matching pre-fix behavior, so a
    # developer running the Flask dev server locally is unaffected.
    db_path = tmp_path / "nightlio.db"
    code = (
        "from api.app import create_app\n"
        "create_app('development')\n"
        "print('APP_CREATED_OK')\n"
    )
    result = _run(code, {"DATABASE_PATH": str(db_path)})
    assert result.returncode == 0, result.stderr
    assert "APP_CREATED_OK" in result.stdout
