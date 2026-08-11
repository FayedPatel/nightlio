"""Regression test for the gunicorn request-limit/worker-recycling flags.

api/docker_start.py:36 previously launched gunicorn with no explicit
--limit-request-line / --limit-request-fields / --limit-request-field_size
(an oversized request line or a flood of header fields ties up a sync
worker parsing input before Flask ever sees it) and no --max-requests
(workers never recycled, so slow per-request memory growth could
accumulate into an OOM kill under sustained/abusive traffic).

Note: gunicorn's real flag is --limit-request-field_size (underscore, not
a hyphen, unlike its two siblings above) -- this test previously asserted
the hyphenated form, which docker_start.py also produced, and both passed
against a fake `gunicorn` stub that only echoes argv without validating
it. That let a request gunicorn rejects outright (`unrecognized
arguments`) ship silently. Fixed in both docker_start.py and this test.

This runs the real script end-to-end (as the container's CMD does) with a
fake `gunicorn` executable on PATH that just echoes argv, so the assertion
is against the actual command produced -- not a re-read of the source.
"""

import os
import stat
import subprocess
import sys
from pathlib import Path

API_DIR = Path(__file__).resolve().parent.parent
# api/docker_start.py does `sys.path.insert(0, "/app")` and a bare
# `from app import create_app`, which only resolves without edits because
# the container has `ln -sf /app /app/api` (api/Dockerfile) making
# `api.database` (used deep in api/services/*) resolve to itself. Outside
# that container, api/app.py's dual-import fallback needs the *package*
# form (`api.database`) importable too, which means the repo root -- not
# just api/ -- has to be on PYTHONPATH, same as test.sh's own
# `PYTHONPATH=.`.
ROOT_DIR = API_DIR.parent


def test_docker_start_gunicorn_command_has_request_limits_and_recycling(tmp_path):
    fake_gunicorn = tmp_path / "gunicorn"
    fake_gunicorn.write_text('#!/bin/sh\necho GUNICORN_ARGS:"$@"\n')
    fake_gunicorn.chmod(
        fake_gunicorn.stat().st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH
    )

    env = dict(os.environ)
    env["PATH"] = f"{tmp_path}:{env.get('PATH', '')}"
    env["APP_ENV"] = "production"
    # Strong, distinct secrets so the production weak-secret check (see
    # test_secret_validation.py) doesn't short-circuit before gunicorn is
    # ever invoked.
    env["SECRET_KEY"] = "a" * 32
    env["JWT_SECRET"] = "b" * 32
    env["PORT"] = "5099"
    env["DATABASE_PATH"] = str(tmp_path / "nightlio.db")
    env["PYTHONPATH"] = f"{ROOT_DIR}{os.pathsep}{API_DIR}"
    for var in (
        "OIDC_ISSUER_URL",
        "OIDC_CLIENT_ID",
        "OIDC_CLIENT_SECRET",
        "OIDC_CALLBACK_URL",
    ):
        env.pop(var, None)

    result = subprocess.run(
        [sys.executable, str(API_DIR / "docker_start.py")],
        cwd=API_DIR,
        env=env,
        capture_output=True,
        text=True,
        timeout=30,
    )

    assert result.returncode == 0, result.stdout + result.stderr
    assert "GUNICORN_ARGS:" in result.stdout, result.stdout
    for flag in (
        "--limit-request-line 8190",
        "--limit-request-fields 100",
        "--limit-request-field_size 8190",
        "--max-requests 1000",
        "--max-requests-jitter 100",
    ):
        assert flag in result.stdout, (
            f"missing {flag!r} in gunicorn invocation: {result.stdout}"
        )
