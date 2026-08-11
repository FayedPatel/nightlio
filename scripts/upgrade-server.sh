#!/usr/bin/env bash
# Upgrade a server running the original upstream Nightlio images
# (ghcr.io/shirsakm/nightlio-*) to this fork's docker-compose.prod.yml stack
# (built from source, Pocket ID OIDC, caddy + playit unchanged), while
# keeping all data in the existing named volumes.
#
# Data safety model:
#   - The SQLite database lives in the named volume `nightlio_data`. Both the
#     old and the new compose files reference it by that exact name, so the
#     new stack mounts the same volume — nothing is copied or renamed.
#   - Schema migrations run automatically when the new api container starts
#     (api/database_schema.py): existing Google users are backfilled with
#     auth_provider='legacy-google'. No manual SQL needed for the upgrade
#     itself.
#   - This script still takes a tarball backup of every nightlio volume
#     before touching anything, so a botched upgrade is one `tar xzf` away
#     from recovery.
#
# Usage (on the server):
#   NIGHTLIO_APP_DIR=/mnt/disk1/lush-game-server/apps/nightlio \
#   NIGHTLIO_REPO_DIR=/mnt/disk1/lush-game-server/apps/nightlio/repo \
#   OLD_APP_DIR=/path/to/old/compose/dir \
#   ./scripts/upgrade-server.sh
#
#   NIGHTLIO_APP_DIR  directory holding .env and Caddyfile for the new stack
#   NIGHTLIO_REPO_DIR git checkout of this fork (used for image builds)
#   OLD_APP_DIR       directory holding the old docker-compose.yaml; the old
#                     stack is stopped from there first (container names
#                     collide). Optional — skipped if unset.
#
# After this script succeeds you still have two manual steps (printed at the
# end): Pocket ID first-run admin setup, and relinking each legacy Google
# user to their new OIDC identity with scripts/relink-oidc-user.sh.

set -euo pipefail

APP_DIR="${NIGHTLIO_APP_DIR:?set NIGHTLIO_APP_DIR (dir with .env + Caddyfile)}"
REPO_DIR="${NIGHTLIO_REPO_DIR:?set NIGHTLIO_REPO_DIR (git checkout of the fork)}"
OLD_APP_DIR="${OLD_APP_DIR:-}"
COMPOSE_FILE="${REPO_DIR}/docker-compose.prod.yml"
BACKUP_DIR="${APP_DIR}/backups/$(date +%Y%m%d-%H%M%S)"

say()  { printf '\n==> %s\n' "$*"; }
fail() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

# ---------------------------------------------------------------- preflight
say "Preflight checks"

command -v docker >/dev/null || fail "docker not found"
docker compose version >/dev/null 2>&1 || fail "docker compose v2 not found"

[ -f "$COMPOSE_FILE" ] || fail "compose file missing: $COMPOSE_FILE (is NIGHTLIO_REPO_DIR a checkout of the fork?)"
[ -f "${APP_DIR}/.env" ] || fail "missing ${APP_DIR}/.env"

# The prod compose mounts ${NIGHTLIO_APP_DIR}/Caddyfile — exact case. The old
# deployment shipped a file named "CaddyFile", which silently fails to mount
# on a case-sensitive filesystem.
[ -f "${APP_DIR}/Caddyfile" ] || fail "missing ${APP_DIR}/Caddyfile (exact case; copy ${REPO_DIR}/Caddyfile and check the old 'CaddyFile' spelling)"

# Vars the new stack refuses to start without, plus the ones that must keep
# their old values so existing data/sessions stay valid.
required_vars=(
  SECRET_KEY JWT_SECRET
  NIGHTLIO_PUBLIC_DOMAIN
  POCKET_ID_APP_URL POCKET_ID_PUBLIC_DOMAIN POCKET_ID_ENCRYPTION_KEY
  OIDC_ISSUER_URL OIDC_CLIENT_ID OIDC_CLIENT_SECRET OIDC_CALLBACK_URL
)
missing=()
for v in "${required_vars[@]}"; do
  grep -Eq "^${v}=." "${APP_DIR}/.env" || missing+=("$v")
done
if [ "${#missing[@]}" -gt 0 ]; then
  fail "missing in ${APP_DIR}/.env: ${missing[*]}
  Keep SECRET_KEY/JWT_SECRET from the old .env verbatim.
  Generate POCKET_ID_ENCRYPTION_KEY with: openssl rand -hex 32
  OIDC_CLIENT_ID/SECRET come from Pocket ID admin after first-run setup —
  for the very first boot you may set placeholder values, finish Pocket ID
  setup, then update .env and 'docker compose ... up -d api' again."
fi

if grep -Eq '^ENABLE_GOOGLE_OAUTH=1' "${APP_DIR}/.env"; then
  echo "note: ENABLE_GOOGLE_OAUTH/GOOGLE_* are ignored by the fork (Google auth removed); safe to delete from .env"
fi

# ------------------------------------------------------------------ backup
say "Backing up volumes to ${BACKUP_DIR}"
mkdir -p "$BACKUP_DIR"
cp "${APP_DIR}/.env" "${BACKUP_DIR}/.env.bak"

for vol in nightlio_data nightlio_caddy_data nightlio_caddy_config; do
  if docker volume inspect "$vol" >/dev/null 2>&1; then
    docker run --rm -v "${vol}:/data:ro" -v "${BACKUP_DIR}:/backup" alpine \
      tar czf "/backup/${vol}.tar.gz" -C /data .
    echo "  ${vol} -> ${BACKUP_DIR}/${vol}.tar.gz"
  else
    echo "  ${vol} not found, skipping (fresh install?)"
  fi
done

# ----------------------------------------------------------- stop old stack
if [ -n "$OLD_APP_DIR" ]; then
  say "Stopping old stack in ${OLD_APP_DIR}"
  # Plain 'down' — never 'down -v'. Volumes must survive.
  (cd "$OLD_APP_DIR" && docker compose down)
else
  say "OLD_APP_DIR not set — assuming old stack is already stopped"
  for c in nightlio-api nightlio-frontend nightlio-caddy nightlio-playit; do
    if [ "$(docker ps -q -f "name=^${c}$")" ]; then
      fail "container ${c} still running; set OLD_APP_DIR or stop the old stack first"
    fi
  done
fi

# ------------------------------------------------------------ start new one
say "Building and starting the new stack"
compose() {
  NIGHTLIO_APP_DIR="$APP_DIR" NIGHTLIO_REPO_DIR="$REPO_DIR" \
    docker compose -f "$COMPOSE_FILE" "$@"
}
compose up -d --build

say "Waiting for api healthcheck"
for _ in $(seq 1 30); do
  status="$(docker inspect -f '{{.State.Health.Status}}' nightlio-api 2>/dev/null || echo starting)"
  [ "$status" = healthy ] && break
  sleep 5
done
if [ "${status:-}" != healthy ]; then
  docker logs --tail 50 nightlio-api || true
  fail "api never became healthy — logs above. Volumes and backup are intact; to roll back: 'docker compose -f ${COMPOSE_FILE} down' then start the old stack again."
fi
echo "api healthy — schema migration ran on startup"

# -------------------------------------------------------------- next steps
say "Done. Remaining manual steps:"
cat <<EOF
1. Pocket ID first run: open \$POCKET_ID_APP_URL, finish admin setup, create
   an OIDC client with callback \$OIDC_CALLBACK_URL, put its client id/secret
   in ${APP_DIR}/.env as OIDC_CLIENT_ID/OIDC_CLIENT_SECRET, then:
     NIGHTLIO_APP_DIR=$APP_DIR NIGHTLIO_REPO_DIR=$REPO_DIR \\
       docker compose -f $COMPOSE_FILE up -d api
2. Public exposure (adds a second playit tunnel for the Pocket ID domain):
     NIGHTLIO_APP_DIR=$APP_DIR NIGHTLIO_REPO_DIR=$REPO_DIR \\
       docker compose -f $COMPOSE_FILE --profile tunnel up -d
3. Each existing user: log in once through Pocket ID (creates a new empty
   account), then relink their old data:
     ./scripts/relink-oidc-user.sh <legacy_user_id> <new_oidc_sub>
   List users to find both ids:
     ./scripts/relink-oidc-user.sh --list
Backup kept at: ${BACKUP_DIR}
EOF
