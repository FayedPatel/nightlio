"""Regression tests for the dual-import guard convention.

Backend modules run in two modes: imported as the ``api.*`` package (tests,
repo root on sys.path) and executed from inside the ``api/`` directory
(``docker_start.py``). In the second mode ``api/api.py`` shadows the ``api``
package, so a bare ``from api.x import ...`` raises ModuleNotFoundError;
modules must fall back to ``from x import ...``. Each test imports a guarded
module in a subprocess whose sys.path points inside ``api/`` (and not at the
repo root) to prove the fallback branch works.
"""

import os
import subprocess
import sys
from pathlib import Path

API_DIR = Path(__file__).resolve().parent.parent


def _run_inside_api_dir(code: str) -> subprocess.CompletedProcess:
    env = dict(os.environ)
    # Only api/ may be importable; the repo root must not be on sys.path,
    # otherwise the primary "from api.x import ..." branch would succeed
    # and the fallback would go untested.
    env["PYTHONPATH"] = str(API_DIR)
    return subprocess.run(
        [sys.executable, "-c", code],
        cwd=API_DIR,
        env=env,
        capture_output=True,
        text=True,
    )


def test_activity_routes_importable_from_inside_api_dir():
    result = _run_inside_api_dir(
        "from routes.activity_routes import create_activity_routes"
    )
    assert result.returncode == 0, result.stderr


def test_rate_limiter_importable_from_inside_api_dir():
    result = _run_inside_api_dir("from utils.rate_limiter import rate_limit")
    assert result.returncode == 0, result.stderr


def test_oauth_config_imports_work_from_inside_api_dir():
    # The guarded imports in oauth.py live inside function bodies, so
    # importing the module is not enough -- call both helpers.
    code = (
        "from flask import Flask\n"
        "from auth.oauth import _get_oidc_settings, _frontend_redirect\n"
        "_get_oidc_settings()\n"
        "app = Flask(__name__)\n"
        "with app.test_request_context('/api/auth/callback/oidc'):\n"
        "    response = _frontend_redirect('sso_error=test')\n"
        "assert response.status_code == 302, response.status_code\n"
    )
    result = _run_inside_api_dir(code)
    assert result.returncode == 0, result.stderr
