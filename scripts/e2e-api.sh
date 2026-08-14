#!/usr/bin/env bash
# Boots the Flask API for Playwright e2e runs: throwaway SQLite DB, OIDC
# blanked so /api/config reports enable_oidc=false and the frontend
# auto-logs-in (credential-free self-host mode). Launched once per Playwright
# worker by e2e/support/fixtures.js with E2E_API_PORT set, so parallel
# workers each get an isolated server + database; not meant for manual use.
set -euo pipefail
cd "$(dirname "$0")/.."

export E2E_API_PORT="${E2E_API_PORT:-5000}"
export APP_ENV=development
export DATABASE_PATH="${E2E_DATABASE_PATH:-/tmp/nightlio_e2e_${E2E_API_PORT}.db}"
# Fresh rate-limiter DB too: login attempts accumulate there across runs
# (30/min on /api/auth/local/login) and back-to-back runs would 429.
export RATE_LIMIT_DB_PATH="${DATABASE_PATH%.db}_rate_limit.db"
rm -f "$RATE_LIMIT_DB_PATH"
# Preset to empty so load_dotenv (which never overrides existing vars)
# cannot leak a developer's real OIDC config from .env into the run.
export OIDC_ISSUER_URL="" OIDC_CLIENT_ID="" OIDC_CLIENT_SECRET="" \
  OIDC_CALLBACK_URL="" FRONTEND_URL="" TRUST_PROXY_HEADERS=""
rm -f "$DATABASE_PATH"

# Same interpreter ladder as test.sh: active venv, api/venv, .venv, system.
if [ -n "${VIRTUAL_ENV:-}" ] && [ -x "$VIRTUAL_ENV/bin/python" ]; then
  PY="$VIRTUAL_ENV/bin/python"
elif [ -x "api/venv/bin/python" ]; then
  PY="api/venv/bin/python"
elif [ -x ".venv/bin/python" ]; then
  PY=".venv/bin/python"
else
  PY="python3"
fi

# Plain app.run without debug: the Werkzeug reloader would fork a child
# process that can outlive Playwright's teardown.
exec "$PY" -c "
import os
from api.app import create_app
create_app('development').run(host='127.0.0.1', port=int(os.environ['E2E_API_PORT']))
"
