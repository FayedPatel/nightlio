#!/usr/bin/env python3
"""
Docker startup script for Nightlio API
"""
import os
import sys
import subprocess

# Set up path for imports
sys.path.insert(0, "/app")

# Import the app creation function
from app import create_app

if __name__ == "__main__":
    # Get environment. APP_ENV is the current name (D5); RAILWAY_ENVIRONMENT
    # is honored as a fallback for existing deployments that still set it
    # (compose sets both, so this only matters for standalone/manual runs).
    env = os.getenv("APP_ENV") or os.getenv("RAILWAY_ENVIRONMENT", "production")

    port = int(os.getenv("PORT", 5000))

    print(f"Starting Nightlio API on port {port}")
    print(f"Environment: {env}")

    try:
        app = create_app(env)
    except RuntimeError as e:
        # create_app raises this specific, plain RuntimeError only for the
        # production weak-secret check (see api/app.py); anything else is a
        # real bug and should keep its traceback, so re-raise those.
        if "SECRET_KEY/JWT_SECRET" not in str(e):
            raise
        print(f"FATAL: {e}", file=sys.stderr)
        sys.exit(1)
    try:
        from config import get_config

        # Status only — never print configuration values here.
        print(f"OIDC configured: {'Set' if get_config().oidc_enabled else 'Missing'}")
    except Exception:
        print("OIDC configured: Unknown")

    if env == "production":
        cmd = [
            "gunicorn",
            "--bind",
            f"[::]:{port}",
            "--workers",
            "4",
            "--timeout",
            "120",
            "--worker-class",
            "sync",
            # Explicit request-line/header limits rather than relying on
            # gunicorn's implicit defaults (which happen to match these
            # values today, but silently change across versions): an
            # oversized request line or header flood otherwise ties up a
            # sync worker parsing input before Flask even sees it. None of
            # this app's real requests get close to these limits.
            "--limit-request-line",
            "8190",
            "--limit-request-fields",
            "100",
            "--limit-request-field_size",
            "8190",
            # Recycle each worker after ~1000 requests (jittered so all 4
            # workers don't restart in the same instant) so any slow
            # per-request memory growth can't accumulate into an
            # out-of-memory kill under sustained/abusive traffic.
            "--max-requests",
            "1000",
            "--max-requests-jitter",
            "100",
            "--access-logfile",
            "-",
            "--error-logfile",
            "-",
            "wsgi:application",
        ]
        print(f"Using Gunicorn: {' '.join(cmd)}")
        subprocess.run(cmd)
    else:
        print("Using Flask development server")
        app.run(debug=True, host="::", port=port)
