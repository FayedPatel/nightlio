# Setup & deployment

Everything needed to run your own Nightlio: the Docker quickstart, LAN and
public deployment, the configuration reference, OIDC single sign-on, backups,
and production hardening.

### Prerequisites

- Docker and Docker Compose
- Basic command-line familiarity

Nightlio ships a **single** `docker-compose.yml` for every environment — local,
LAN, and public production. There is no separate prod compose file; for public
deployments you put your own TLS-terminating reverse proxy (Caddy, Traefik, or
nginx) in front of the `frontend` service instead.

## Docker Quickstart (Recommended)

> [!NOTE]
> By default, Nightlio runs in **single-user mode** with credential-free local login. Enable OIDC (any spec-compliant provider, Pocket ID recommended) for multi-user support with real sign-in.

Get your own Nightlio instance running in under 5 minutes.

```bash
# 1. Clone the repository
git clone https://github.com/FayedPatel/nightlio.git
cd nightlio

# 2. Create your configuration file
cp .env.docker .env

# 3. Set your secrets
# IMPORTANT: Open the .env file and set unique, random values for
# at least SECRET_KEY and JWT_SECRET.
nano .env

# 4. Launch the application (builds the images from source)
docker compose up -d --build
```

Your instance is now live at http://localhost:5173/.

To pull this fork's prebuilt images instead of building locally, point
compose at them — the packages are public, no GHCR login required:

```bash
API_IMAGE=ghcr.io/fayedpatel/nightlio-api:latest \
WEB_IMAGE=ghcr.io/fayedpatel/nightlio-frontend:latest \
docker compose up -d
```

Images are published by `.github/workflows/publish.yml` on every merge to
`main` (`latest`, `sha-*` tags) and on `v*` release tags (semver tags), for
both **linux/amd64 and linux/arm64**.

## Self-hosting

Nightlio ships a single `docker-compose.yml` that works for both local/LAN
use and public deployment.

### Local / LAN

```bash
git clone https://github.com/FayedPatel/nightlio.git
cd nightlio
cp .env.docker .env
# Edit .env: set strong SECRET_KEY and JWT_SECRET
docker compose up -d --build
```

This builds the images from source (the `build:` stanzas) and starts `api`
on `5000` and `frontend` on `5173`. Set `API_IMAGE` / `WEB_IMAGE` env vars
instead if you'd rather pull the published GHCR images
(`ghcr.io/fayedpatel/nightlio-api`, `ghcr.io/fayedpatel/nightlio-frontend`;
private, needs `docker login ghcr.io` with a `read:packages` token) than
build locally — `docker compose up -d` (no `--build`) then uses whichever
image tag you set, defaulting to `nightlio-api:local` /
`nightlio-frontend:local` if you built them yourself previously.

### Public deployment

Nightlio's `frontend` service does not terminate TLS itself. For a public
deployment, put your own reverse proxy (Caddy, Traefik, or nginx) in front
of the `frontend` container's port and let it handle certificates. Then, in
`.env`, set:

```bash
TRUST_PROXY_HEADERS=1
CORS_ORIGINS=https://your.domain
```

`TRUST_PROXY_HEADERS=1` tells the api to trust `X-Forwarded-For` /
`X-Forwarded-Proto` from your proxy (correct client IPs for rate limiting,
correct `Secure` cookie flag behind TLS). `CORS_ORIGINS` must match the
public origin your users actually hit.

Example with Caddy (`Caddyfile`, run alongside the compose stack or on the
host):

```
yourdomain.com {
    reverse_proxy localhost:5173
}
```

> [!NOTE]
> 1. Persistent data lives in the `nightlio_data` named Docker volume — see
>    [Backups](#backups) for snapshot and restore commands.
> 2. Pin `API_IMAGE`/`WEB_IMAGE` to a version tag for predictable upgrades
>    when using published images.
> 3. Upgrading an existing deployment across versions? See
>    [Upgrading](#upgrading) below and [`UPGRADING.md`](UPGRADING.md).

## 🔧 Configuration (`.env`)

You can customize your Nightlio instance using environment variables in the `.env` file.

### Server (API)
```
# Core
APP_ENV=production          # was RAILWAY_ENVIRONMENT; still honored as a fallback
SECRET_KEY=change-this-to-a-long-random-string
JWT_SECRET=change-this-too
DATABASE_PATH=/app/data/nightlio.db

# Optional PostgreSQL backend (v0.6.0, EXPERIMENTAL; PostgreSQL 16+).
# Unset/empty keeps the default SQLite backend at DATABASE_PATH. A
# postgres:// URL switches the main data store to Postgres — a matching
# compose profile service ships in docker-compose.yml
# (docker compose --profile postgres up -d). Full walkthrough, TLS notes,
# and the SQLite-data import flow: docs/POSTGRES.md
# DATABASE_URL=

# User id assigned to entries created in single-user (self-host) mode
DEFAULT_SELF_HOST_ID=selfhost_default_user

# Feature flags (1 to enable, 0 to disable)
ENABLE_MOOD_MUSIC=0

# OIDC single sign-on (optional; any OIDC-compliant provider, Pocket ID
# recommended). Leave empty for local-password/single-user auth.
OIDC_ISSUER_URL=
OIDC_CLIENT_ID=
OIDC_CLIENT_SECRET=
# Optional: signup/invite URL at the provider, shown as "Create account"
# OIDC_SIGNUP_URL=
# Optional: 1 hard-disables ALL local logins (password form and
# credential-free mode) so SSO is the only door; 0 keeps local login on.
# Unset/empty defaults to SSO-only whenever OIDC_ISSUER_URL is configured
# (and to enabled when it is not); an explicit 0 or 1 always wins.
# DISABLE_LOCAL_LOGIN=

# Mood music (if enabled)
JAMENDO_CLIENT_ID=

# CORS - Add your frontend's domain if deploying publicly
CORS_ORIGINS=http://localhost:5173,https://your.domain.com

# Optional: only if the frontend is on a different origin than the api
# FRONTEND_URL=

# Optional: only if the api sits behind a trusted reverse proxy
# TRUST_PROXY_HEADERS=0
```

### Frontend (Vite)
```
# This is only needed for local development outside of Docker
VITE_API_URL=http://localhost:5000
```

### Variable reference

**Required** — compose refuses to start the api without them:

| Variable | Purpose |
| --- | --- |
| `SECRET_KEY` | API secret key; also signs the OIDC state cookie. Generate with `openssl rand -hex 32`. |
| `JWT_SECRET` | JWT signing secret. Use a **different** random value. |

**Optional:**

| Variable | Purpose |
| --- | --- |
| `OIDC_ISSUER_URL` / `OIDC_CLIENT_ID` / `OIDC_CLIENT_SECRET` | Set all three to enable OIDC single sign-on. Leave empty for local-password / single-user auth. |
| `OIDC_CALLBACK_URL` | Override the derived callback URL if the api cannot infer its own public address. |
| `OIDC_SIGNUP_URL` | Signup/invite URL at the provider, surfaced as a "Create account" link. |
| `DISABLE_LOCAL_LOGIN` | `1` hard-disables `POST /api/auth/local/login` (both the password form and credential-free mode). Unset/empty derives it from OIDC: configured ⇒ disabled, otherwise enabled. An explicit `0` or `1` always wins. Warning: with OIDC unconfigured **and** this set to `1`, nobody can log in. |
| `ENABLE_MOOD_MUSIC` | `1` enables mood-based music recommendations (needs `JAMENDO_CLIENT_ID`). |
| `JAMENDO_CLIENT_ID` | Jamendo API client id, only read when mood music is enabled. |
| `DEFAULT_SELF_HOST_ID` | User id for entries created in single-user self-hosted mode. |
| `DATABASE_URL` | Database backend selector (v0.6.0, experimental): unset/empty keeps the default SQLite file; a `postgres://…` URL switches the api to PostgreSQL 16+. See [docs/POSTGRES.md](POSTGRES.md). |
| `POSTGRES_DB` / `POSTGRES_USER` / `POSTGRES_PASSWORD` | Only read by the bundled `postgres` compose profile (they configure its server container, not the api). `POSTGRES_PASSWORD` is required when that profile is active; db/user default to `nightlio`. |
| `APP_ENV` | Environment selector (`production` / `development`). Replaces `RAILWAY_ENVIRONMENT`, which is still honored as a fallback. |
| `CORS_ORIGINS` | Comma-separated allowed origins. Must match the public origin your users actually hit. |
| `FRONTEND_URL` | Only needed if the frontend is served from a different origin than the api. |
| `TRUST_PROXY_HEADERS` | `1` only if the api sits behind a **trusted** reverse proxy. |
| `API_IMAGE` / `WEB_IMAGE` | Pull published images instead of building from source. |
| `POCKET_ID_ENCRYPTION_KEY` | Required only when running the optional `oidc` compose profile; ≥ 16 bytes, `openssl rand -hex 32`. |
| `I18N_GITHUB_REPO` | `owner/repo` whose `lang-<code>-vX.Y.Z` releases carry language packs. Default `FayedPatel/nightlio`. |
| `I18N_GITHUB_API_BASE` | GitHub API base URL for language-pack discovery. Default `https://api.github.com`. |
| `I18N_REFRESH_SECS` | Seconds between language-pack discovery refreshes. Default `3600`. |
| `I18N_OFFLINE` | `1` never contacts GitHub for language packs; serves only what the disk cache already holds. |
| `I18N_LOCAL_DIR` | Directory of `<code>.json` language-pack files served straight from disk (air-gapped mode; takes precedence over GitHub discovery entirely). |
| `I18N_GITHUB_TOKEN` | Optional GitHub token for pack discovery (raises the unauthenticated 60 requests/hour API limit). |

The `I18N_*` group configures runtime language packs and is entirely
optional — bundled English works with zero configuration. The self-hoster
guide, including air-gapped setups and how new languages get added, is
[`I18N.md`](I18N.md).

### Ports

- **Frontend**: `http://localhost:5173` — same port in Docker and in development.
  The container itself listens on `8080` (non-root nginx cannot bind below 1024).
- **API**: `http://localhost:5000` — the Rust backend, same in Docker and development.

### Generating secure secrets

```bash
# Write fresh secrets straight into .env
sed -i "s|^SECRET_KEY=.*|SECRET_KEY=$(openssl rand -hex 32)|" .env
sed -i "s|^JWT_SECRET=.*|JWT_SECRET=$(openssl rand -hex 32)|" .env
```

## OIDC Single Sign-On (Optional)

Nightlio speaks generic OpenID Connect, so any spec-compliant provider works
(Authelia, Authentik, Keycloak, Auth0, ...). [Pocket ID](https://github.com/pocket-id/pocket-id)
is the recommended self-host option and ships as an optional compose
profile:

```bash
# Starts Pocket ID alongside the app (off by default otherwise)
docker compose --profile oidc up -d
```

1. Bring Pocket ID up (above), then open its admin UI
   (`http://pocket-id.localhost:1411` in dev — the dotted `*.localhost` name
   is required because Pocket ID is passkey-based and needs a valid WebAuthn
   RPID; put it behind your reverse proxy in prod) and create an OIDC client.
2. Register the callback URL in that client's settings as the **api's**
   callback endpoint (not a frontend page — the frontend's `/login` route
   only picks up the token *after* the api redirects back to it):
   ```
   http://localhost:5173/api/auth/callback/oidc     # dev, via the frontend's nginx /api/ proxy
   https://yourdomain.com/api/auth/callback/oidc    # prod, same pattern behind your proxy
   ```
   (Or point directly at the api's own port/host, e.g.
   `http://localhost:5000/api/auth/callback/oidc`, if you're not routing
   `/api/` through nginx.)
3. Set on the `api` service in `.env`:
   ```
   OIDC_ISSUER_URL=http://pocket-id.localhost:1411   # dev (compose network alias; browsers resolve *.localhost to 127.0.0.1)
   OIDC_ISSUER_URL=https://id.yourdomain.com         # prod, public URL
   OIDC_CLIENT_ID=<client id from Pocket ID admin>
   OIDC_CLIENT_SECRET=<client secret from Pocket ID admin>
   ```
4. Restart the api: `docker compose up -d`. `GET /api/config` will report
   `enable_oidc: true` once `OIDC_ISSUER_URL` is set, and the frontend's
   login page picks up the SSO button automatically.

Setting `OIDC_ISSUER_URL` also disables local login entirely by default —
with SSO configured, user management is delegated to the identity provider,
so `POST /api/auth/local/login` (both the username/password form and the
credential-free single-user mode) fails closed with 403. To keep local
username/password accounts working alongside OIDC, set
`DISABLE_LOCAL_LOGIN=0` explicitly; without OIDC, local login stays enabled
unless you set `DISABLE_LOCAL_LOGIN=1`.

Pocket ID's admin UI is published on host port `1411`, **bound to localhost
only**, so its pre-passkey `/setup` endpoint is never reachable from the LAN.
Its data lives in its own named volume (`pocket_id_data`), separate from
`nightlio_data` — enabling or disabling the profile never touches your
Nightlio data.

## Mood music (optional)

1. Create a free app at the [Jamendo API](https://developer.jamendo.com/v3.0).
2. Add the client id to `.env`:

```bash
ENABLE_MOOD_MUSIC=1
JAMENDO_CLIENT_ID=your-jamendo-client-id
```

## TLS and reverse proxies

Nightlio's `frontend` container serves plain HTTP only — it does not bundle a
TLS-terminating config. Run an external reverse proxy in front of it and let
that proxy handle certificates. Whichever proxy you pick, set
`TRUST_PROXY_HEADERS=1` and a matching `CORS_ORIGINS` in `.env`.

### Caddy (automatic HTTPS)

Caddy obtains and renews Let's Encrypt certificates on its own — no certbot
step, no `ssl/` directory, no manual cert placement.

```
yourdomain.com {
    reverse_proxy localhost:5173
}
```

### nginx

```nginx
server {
    listen 80;
    server_name yourdomain.com;
    return 301 https://$server_name$request_uri;
}

server {
    listen 443 ssl http2;
    server_name yourdomain.com;

    ssl_certificate     /path/to/fullchain.pem;
    ssl_certificate_key /path/to/privkey.pem;

    location / {
        proxy_pass http://localhost:5173;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}
```

Certificates via certbot:

```bash
sudo apt update && sudo apt install certbot python3-certbot-nginx
sudo certbot --nginx -d yourdomain.com
# auto-renewal
echo "0 12 * * * /usr/bin/certbot renew --quiet" | sudo crontab -
```

Optional static-asset caching and compression in that nginx site:

```nginx
location ~* \.(js|css|png|jpg|jpeg|gif|ico|svg)$ {
    expires 1y;
    add_header Cache-Control "public, immutable";
}

gzip on;
gzip_types text/plain text/css application/json application/javascript text/xml application/xml;
```

### Traefik

```yaml
services:
  nightlio-api:
    # ... your api config
    labels:
      - "traefik.enable=true"
      - "traefik.http.routers.nightlio-api.rule=Host(`yourdomain.com`) && PathPrefix(`/api`)"
      - "traefik.http.routers.nightlio-api.tls.certresolver=letsencrypt"

  nightlio-frontend:
    # ... your frontend config
    labels:
      - "traefik.enable=true"
      - "traefik.http.routers.nightlio.rule=Host(`yourdomain.com`)"
      - "traefik.http.routers.nightlio.tls.certresolver=letsencrypt"
```

When proxying, you can drop the published port mappings from
`docker-compose.yml` entirely and route to the service names (`api`,
`frontend`) on the compose network instead. If you use OIDC, update the
provider client's registered callback URL to your public domain.

## Everyday operation

```bash
# Start / stop
docker compose up -d
docker compose down

# Logs (all services, or one)
docker compose logs -f
docker compose logs -f api
docker compose logs -f frontend

# Restart, status, resource usage
docker compose restart
docker compose ps
docker stats

# Verify a config change before applying it
docker compose config
```

Both services declare healthchecks, so `docker compose ps` reports real
health rather than just "running". The api answers its own healthcheck via
the `nightlio-api --health-check` subcommand — the image ships no Python,
curl, or sqlite3 CLI.

## Backups

All persistent data lives in the `nightlio_data` named volume. Compose
prefixes it with the project name, so it is usually
`nightlio_nightlio_data` — confirm with `docker volume ls`.

The database runs in SQLite WAL mode with a bounded journal
(`journal_size_limit` 4 MiB) and checkpoint-truncates on clean shutdown. For
a guaranteed-consistent snapshot, stop the stack first so the WAL is folded
back into the single database file:

```bash
# Back up
docker compose down
docker run --rm -v nightlio_nightlio_data:/data -v $(pwd):/backup \
  alpine tar czf /backup/nightlio-backup.tar.gz -C /data .
docker compose up -d

# Restore
docker compose down
docker run --rm -v nightlio_nightlio_data:/data -v $(pwd):/backup \
  alpine tar xzf /backup/nightlio-backup.tar.gz -C /data
docker compose up -d

# Inspect the volume
docker volume inspect nightlio_nightlio_data
```

Scheduled backups — save as `backup.sh` and run daily from cron:

```bash
#!/bin/bash
BACKUP_DIR="/backups/nightlio"
DATE=$(date +%Y%m%d_%H%M%S)
mkdir -p "$BACKUP_DIR"

docker run --rm \
  -v nightlio_nightlio_data:/data \
  -v "$BACKUP_DIR":/backup \
  alpine tar czf "/backup/nightlio-$DATE.tar.gz" -C /data .

# Keep only the last 7 days
find "$BACKUP_DIR" -name "nightlio-*.tar.gz" -mtime +7 -delete
```

### Log rotation

Add to `/etc/logrotate.d/docker-nightlio`:

```
/var/lib/docker/containers/*/*-json.log {
    daily
    rotate 7
    compress
    delaycompress
    missingok
    notifempty
    create 0644 root root
    postrotate
        docker kill -s USR1 $(docker ps -q) 2>/dev/null || true
    endscript
}
```

## Upgrading

```bash
git pull
docker compose pull        # if using published images
docker compose up -d --build
```

Take a backup first. For version-specific notes — including the v0.4.0
cutover that replaced the Python API with the Rust binary on the same data
volume — see [`UPGRADING.md`](UPGRADING.md).

## Server requirements

|  | Minimum | Recommended |
| --- | --- | --- |
| RAM | 512 MB | 1 GB |
| CPU | 1 vCPU | 2 vCPU |
| Storage | 1 GB (grows with data) | 5 GB SSD |
| OS | any Linux with Docker | Ubuntu 22.04 LTS or newer |

## Hardening

Both images already run as non-root out of the box — no Dockerfile edits
needed:

- the **api** as `appuser` (uid 1000, see `api/Dockerfile`),
- the **frontend** on the `nginxinc/nginx-unprivileged` base, listening on
  container port `8080`.

Optional extra hardening per service in `docker-compose.yml`:

```yaml
security_opt:
  - no-new-privileges:true
```

Memory limits, if you want them:

```yaml
deploy:
  resources:
    limits:
      memory: 512M
```

Firewall — expose only what the reverse proxy needs:

```bash
sudo ufw default deny incoming
sudo ufw default allow outgoing
sudo ufw allow ssh
sudo ufw allow 80
sudo ufw allow 443
sudo ufw enable
```

Keep the host and images patched:

```bash
sudo apt update && sudo apt upgrade -y
cd /path/to/nightlio && git pull
docker compose down
docker compose build --no-cache
docker compose up -d
docker image prune -f
```

See [`../SECURITY.md`](../SECURITY.md) for the full threat model and the
security posture of each setting.

## Customization

### Changing ports

```yaml
services:
  frontend:
    ports:
      - "8080:8080"   # host 8080 instead of 5173 (container listens on 8080)
  api:
    ports:
      - "5001:5000"   # host 5001 instead of 5000
```

### Using an existing database file

```yaml
services:
  api:
    volumes:
      - ./path/to/your/nightlio.db:/app/data/nightlio.db
```

## Troubleshooting

**Services won't start**

```bash
docker compose logs
# check for port conflicts
netstat -tlnp | grep -E ':(80|443|5173|5000)'
# clean restart
docker compose down && docker compose up --build
```

**Reset the database** (⚠️ deletes all data)

```bash
docker compose down
docker volume rm nightlio_nightlio_data
docker compose up -d
```

**Database corruption** — restore the most recent backup using the restore
command above.

**Permission issues on the data volume**

```bash
docker compose exec api chown -R 1000:1000 /app/data
```

**Still stuck?** Check `docker compose logs`, confirm your `.env` is
complete, make sure ports 5173 and 5000 are free, then open a GitHub issue
with your logs and (redacted) configuration.
