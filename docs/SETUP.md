# Setup & deployment

Everything needed to run your own Nightlio: the Docker quickstart, LAN and public deployment, configuration reference, and OIDC single sign-on.

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

To pull this fork's prebuilt images instead of building locally, log in to
GHCR first (the packages are private, so a personal access token with
`read:packages` is required) and point compose at them:

```bash
echo <PAT> | docker login ghcr.io -u FayedPatel --password-stdin
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

Upgrade later:

```bash
git pull
docker compose pull   # if using published images
docker compose up -d --build
```

> [!NOTE]
> 1. Persistent data lives in the `nightlio_data` named Docker volume
>    (compose prefixes it with the project name, so it's usually
>    `nightlio_nightlio_data` — check with `docker volume ls`). Back it up
>    with `docker run --rm -v nightlio_nightlio_data:/data -v $(pwd):/backup alpine tar czf /backup/nightlio-backup.tar.gz -C /data .`
> 2. Pin `API_IMAGE`/`WEB_IMAGE` to a version tag for predictable upgrades
>    when using published images.
> 3. Upgrading an existing deployment across versions? See
>    [`UPGRADING.md`](UPGRADING.md).

## 🔧 Configuration (`.env`)

You can customize your Nightlio instance using environment variables in the `.env` file.

### Server (API)
```
# Core
APP_ENV=production          # was RAILWAY_ENVIRONMENT; still honored as a fallback
SECRET_KEY=change-this-to-a-long-random-string
JWT_SECRET=change-this-too
DATABASE_PATH=/app/data/nightlio.db

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
