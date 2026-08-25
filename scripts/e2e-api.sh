#!/usr/bin/env bash
# Boots the Rust API for Playwright e2e runs: throwaway SQLite DB, OIDC
# blanked so /api/config reports enable_oidc=false and the frontend
# auto-logs-in (credential-free self-host mode). Launched once per
# Playwright worker by e2e/support/fixtures.ts with E2E_API_PORT set, so
# parallel workers each get an isolated server + database; not meant for
# manual use. (The legacy Flask boot path was removed together with the
# Flask backend — see contract/DECISIONS.md, "Legacy removal".)
set -euo pipefail
cd "$(dirname "$0")/.."

export E2E_API_PORT="${E2E_API_PORT:-5000}"
export APP_ENV=development
export DATABASE_PATH="${E2E_DATABASE_PATH:-/tmp/nightlio_e2e_${E2E_API_PORT}.db}"
# Fresh rate-limiter DB too: login attempts accumulate there across runs
# (30/min on /api/auth/local/login) and back-to-back runs would 429.
# APP_ENV=development (not "testing") on purpose: the "testing" env
# hardcodes DATABASE_PATH to a single shared file, which would collide
# across parallel workers, and it also bypasses rate limiting entirely --
# the e2e boot has never relied on that bypass, only on this fresh
# per-worker rate-limit DB starting every run's counters at zero.
export RATE_LIMIT_DB_PATH="${DATABASE_PATH%.db}_rate_limit.db"
rm -f "$RATE_LIMIT_DB_PATH"
# Preset to empty so dotenvy (which never overrides existing vars) cannot
# leak a developer's real OIDC config from .env into the run.
# DISABLE_LOCAL_LOGIN blanked for the same reason: with OIDC blanked, empty
# defaults to local login enabled (credential-free auto-login), but a real
# .env's explicit =1 would otherwise 403 every e2e login.
export OIDC_ISSUER_URL="" OIDC_CLIENT_ID="" OIDC_CLIENT_SECRET="" \
  OIDC_CALLBACK_URL="" FRONTEND_URL="" TRUST_PROXY_HEADERS="" \
  DISABLE_LOCAL_LOGIN=""
# Hermetic i18n: never let e2e workers hit the live GitHub releases API.
# Once a real lang-*-v* release exists, discovery would otherwise fetch it
# (nondeterministic strings + unauthenticated 60/h rate limit shared across
# parallel workers). Offline with a cold cache = empty language list, which
# is exactly the degrade path the suite pins.
export I18N_OFFLINE=1
rm -f "$DATABASE_PATH"

# cargo build's own fingerprinting makes this a ~0.2s no-op once the release
# binary is up to date, so "build once, reuse if fresh" falls out of cargo's
# normal incremental behavior -- no hand-rolled staleness check needed. Cargo
# also serializes concurrent builds against the same target/ dir via its own
# lock file, so N workers racing this script on a cold cache is safe: the
# first pays the compile, the rest block briefly then no-op.
#
# Output is captured and only shown on failure, so a healthy build stays
# quiet under Playwright's ignored stdio.
BUILD_LOG="$(mktemp)"
if ! cargo build --release --manifest-path api/Cargo.toml --bin nightlio-api >"$BUILD_LOG" 2>&1; then
  cat "$BUILD_LOG" >&2
  rm -f "$BUILD_LOG"
  exit 1
fi
rm -f "$BUILD_LOG"

# The Rust config maps E2E_API_PORT -> PORT (Config::from_env reads PORT,
# default 5000). DATABASE_PATH / RATE_LIMIT_DB_PATH / OIDC_* / FRONTEND_URL /
# TRUST_PROXY_HEADERS are already exported above with the same names Rust's
# Config::from_env reads (api/src/config.rs), so no further translation
# is needed.
export PORT="$E2E_API_PORT"
exec api/target/release/nightlio-api
