# Upgrading an existing Nightlio deployment

This doc is for self-hosters who already have a running Nightlio (any version
before the Phase 5 compose rework). It covers what changes, why your data is
safe, and the exact commands to run.

## Upgrading v0.2.0 → v0.3.0

Nothing manual. Pull the new image (or `git pull` + `--build`), then
`docker compose up -d`. Your data lives in the `nightlio_data` volume and is
untouched; the API's startup migrations add the one new column this release
introduces (`users.theme_preference`, for the theme picker) before serving
any requests. Goal backdating reuses the existing `goal_completions` table.
Rolling back to the 0.2.0 image is safe — it simply ignores the extra
column.

Cheap insurance before any upgrade:

```bash
# (the api image has no sqlite3 CLI; python's sqlite3 module does the same)
docker compose exec api python -c "import sqlite3; sqlite3.connect('/app/data/nightlio.db').backup(sqlite3.connect('/app/data/pre-v0.3.0.db'))"
```

Optional new setting for SSO-only deployments: `DISABLE_LOCAL_LOGIN=1` in
`.env` hard-disables all local logins (password form and credential-free
mode) so your identity provider is the only door. Leave it unset/`0` if you
use local or credential-free login.

The rest of this document covers the older Phase 5 compose-rework upgrade.

> **Status:** final. Phase 2 (local password login + generic OIDC, Google
> removal) and Phase 4 (httpOnly session cookie, `/api/auth/logout`) have
> both landed in `api/` and `src/`. This doc now reflects the actual shipped
> behavior, not a plan.

## TL;DR

```bash
cd /path/to/nightlio
git pull
cp .env .env.bak                     # keep a copy before editing
# edit .env per "Env var changes" below
docker compose up -d --build
docker compose logs -f api           # confirm it comes up healthy
```

Your data is untouched: the SQLite database lives in the `nightlio_data`
named volume, which this change never renames, and `DATABASE_PATH` keeps its
existing default (`/app/data/nightlio.db`) inside the container. Schema
migrations run automatically on container start (this was already true
before Phase 5 and is unchanged here) — you do not need to run anything by
hand.

## What actually changed (compose/infra layer)

1. **`docker-compose.yml` now has `build:` stanzas** for `api` and
   `frontend`, alongside the existing `image:` keys. `docker compose up -d
   --build` on a fresh clone now builds from source instead of requiring the
   prebuilt `ghcr.io/shirsakm/*` images. If you were already overriding
   `API_IMAGE` / `WEB_IMAGE` to pull prebuilt images, that keeps working
   unchanged — nothing forces a rebuild if you don't pass `--build`.
2. **API healthcheck fixed** in `docker-compose.yml`. It used to run
   `python -c "import requests; requests.get('http://localhost:5000/api/')"`,
   but `requests` was never a declared dependency of the api image — the
   check only ever "passed" by accident (something else pulled `requests` in
   transitively). It now uses the stdlib: `python -c "import urllib.request;
   urllib.request.urlopen('http://localhost:5000/api/')"`. If your container
   was reporting unhealthy before for unrelated reasons, re-check after this
   upgrade — the healthcheck itself is now trustworthy.
3. **Optional Pocket ID service** added to `docker-compose.yml` as an `oidc`
   compose profile, off by default. See "OIDC / Pocket ID" below.
4. **`docker-compose.prod.yml` was removed.** There is now a single
   `docker-compose.yml` for every environment — local, LAN, and public
   production. If you were deploying with `docker compose -f
   docker-compose.prod.yml ...` (formerly `docker-compose.prod.yml`,
   removed), switch to the plain `docker compose ...` commands against
   `docker-compose.yml`, and put your own TLS-terminating reverse proxy
   (Caddy, Traefik, or nginx) in front of the `frontend` service's port
   instead of the bundled nginx+TLS setup that file used to provide. Set
   `TRUST_PROXY_HEADERS=1` and `CORS_ORIGINS` to your public origin in
   `.env` — see the README's "Self-hosting" section for the exact steps.
   No compose file still ships a `./ssl` bind mount or nginx TLS config;
   certificates are your reverse proxy's responsibility now.

None of the compose/infra work above touches migrations — those already ran
automatically before Phase 5 and still do.

Separately (Phase 2/4, `api/` and `src/`, now shipped):
- Google OAuth is gone entirely. Auth is now local username/password
  (`POST /api/auth/local/register`, `POST /api/auth/local/login`), plus
  credential-free single-user self-host login when OIDC isn't configured,
  plus optional generic OIDC single sign-on (`GET /api/auth/login/oidc` /
  `GET /api/auth/callback/oidc`).
- The session token is now also set as an httpOnly cookie (`nightlio_token`)
  in addition to being returned in the login response body. **Existing
  localStorage-based sessions from before this change keep working
  unchanged** — the token format and `POST /api/auth/verify` behavior are
  the same; the cookie is additive, not a replacement. Logins that happen
  after the upgrade pick up the cookie automatically; nothing to do.
- `POST /api/auth/logout` is new — clears the cookie, safe to call anytime.

## Env var changes

**Remove** from your `.env` (no longer read by anything — Phase 2 deleted
the corresponding `api/` code):
```
ENABLE_GOOGLE_OAUTH
GOOGLE_CLIENT_ID
GOOGLE_CLIENT_SECRET
GOOGLE_CALLBACK_URL
```
Leaving them in place is harmless (simply ignored) if you'd rather clean
them up later, but they should be treated as dead.

**Add, only if you want OIDC login** (optional — omit entirely for
local-password/single-user auth, which remains the default):
```
OIDC_ISSUER_URL=
OIDC_CLIENT_ID=
OIDC_CLIENT_SECRET=
```
All three default to empty string in both compose files if unset — you do
not need to add them to `.env` at all unless you're enabling OIDC. Setting
`OIDC_ISSUER_URL` also disables the credential-free single-user login path
(`POST /api/auth/local/login` with no body now returns 403) — local
username/password accounts keep working alongside OIDC. See "OIDC / Pocket
ID" below for the exact callback URL to register.

**Add, both optional:**
```
FRONTEND_URL=
TRUST_PROXY_HEADERS=0
```
- `FRONTEND_URL`: only needed if your frontend is served from a different
  origin (host/port) than the api — it's the redirect target after an OIDC
  login. Empty/unset means same-origin, which is the standard single-host
  compose deployment (nginx serves both) and needs no change.
- `TRUST_PROXY_HEADERS`: only needed if the api sits behind a reverse proxy
  you trust to set `X-Forwarded-For`/`X-Forwarded-Proto` correctly. Enables
  real client IPs in the rate limiter and a correct `Secure` cookie flag
  behind TLS-terminating proxies. Leave `0`/unset for the default compose
  topology (nginx → api on the same docker network, no user-supplied
  forwarding headers to trust).

**Recommended, not required:**
```
APP_ENV=production
```
This is the D5 rename: `APP_ENV` replaces `RAILWAY_ENVIRONMENT` as the
environment selector read by `api/docker_start.py`. The old name is still
honored as a fallback (`APP_ENV` checked first, then `RAILWAY_ENVIRONMENT`,
then default `production`) — you do not need to change anything.
`docker-compose.yml` already sets both vars for you. This line documents
intent for anyone editing the compose file directly or running the api
container standalone outside compose.

One previously-soft requirement is now hard: `SECRET_KEY` and `JWT_SECRET`
no longer have insecure fallback defaults. `docker-compose.yml` refuses to
start the api container until both are set in `.env` (`${SECRET_KEY:?...}`),
and the api itself also rejects missing, blank, or well-known placeholder
values in production. Generate both with `openssl rand -hex 32` (two
different values) before `docker compose up`.

## OIDC / Pocket ID (new, optional, off by default)

`docker-compose.yml` gained a `pocket-id` service under the `oidc` profile.
It is **not** started by plain `docker compose up -d` — you must opt in:

```bash
docker compose --profile oidc up -d
```

Pocket ID's own admin UI is published on host port `1411`, bound to
localhost only — open `http://pocket-id.localhost:1411` (Pocket ID is
passkey-based, so the hostname must be a valid WebAuthn RPID; the dotted
`*.localhost` name qualifies and browsers resolve it to 127.0.0.1 on their
own) to create clients/users before wiring `OIDC_*`. Data persists in its
own named volume, `pocket_id_data` — separate from `nightlio_data`, so
enabling/disabling the profile never touches your Nightlio data.

For a public deployment, Pocket ID has no TLS of its own either — put it
behind your reverse proxy at a subdomain
(e.g. `id.yourdomain.com`) if you want it reachable from outside the host.
Set `POCKET_ID_APP_URL` to that public HTTPS URL (defaults to
`http://pocket-id.localhost:1411` for local use — override it for a real
public deployment).

Once Pocket ID is up and you've created a client in its admin UI, set on the
api service:
```
OIDC_ISSUER_URL=http://pocket-id.localhost:1411   # dev (compose network alias; browsers resolve *.localhost themselves)
OIDC_ISSUER_URL=https://id.yourdomain.com         # prod, public URL
OIDC_CLIENT_ID=<from Pocket ID admin>
OIDC_CLIENT_SECRET=<from Pocket ID admin>
```
Register the callback URL in Pocket ID's client settings as the **api's**
callback endpoint, not a frontend page (`api/auth/oauth.py` handles the
redirect; the frontend's `/login` route only picks up the token afterward,
from the URL fragment):
```
http://localhost:5173/api/auth/callback/oidc     # dev, via the frontend's nginx /api/ proxy
https://yourdomain.com/api/auth/callback/oidc    # prod, same pattern behind your proxy
```
(Or the api's own host/port directly, e.g.
`http://localhost:5000/api/auth/callback/oidc`, if `/api/` isn't routed
through nginx in your topology.)

Note: `docker-compose.yml`'s inline comments next to the `pocket-id` service
give the correct callback URL (`.../api/auth/callback/oidc`, matching the
URL above) — an earlier draft of this doc warned they were stale from before
Phase 2; that has since been corrected in the compose file.

**Image note:** compose pins `ghcr.io/pocket-id/pocket-id:latest`. This
was not pulled/run in this environment (no network egress in this pass) —
the image name and tag follow Pocket ID's documented GHCR convention, but
have not been verified by an actual pull here. Pin to a specific version tag
once you've confirmed it against Pocket ID's release page, rather than
trusting `:latest` in a long-lived deployment.

## Migrations

Unchanged behavior: schema migrations against `DATABASE_PATH` run
automatically on every container start, before serving traffic (triggered
from `create_app()`'s `MoodDatabase` construction, called by
`api/docker_start.py`). This was true before Phase 5 and remains true —
`api/docker_start.py`'s only change in this pass is the D5 env-selector
rename (`APP_ENV`, falling back to `RAILWAY_ENVIRONMENT`); it does not touch
migration logic. New tables/columns from Phase 2/4 (`auth_provider`,
`external_id`, `password_hash` on `users`; the new `activity_log` table)
are created/backfilled by the same auto-migration path — you do not run any
migration command by hand; `docker compose up -d --build` is sufficient.

## Rollback

If something goes wrong after upgrading:
```bash
docker compose down
git checkout <previous-tag-or-commit> -- docker-compose.yml
docker compose up -d --build
```
(If you're rolling back to a commit from before `docker-compose.prod.yml`
was removed and you were relying on it, also `git checkout
<previous-tag-or-commit> -- docker-compose.prod.yml nginx-prod.conf` to
restore those files alongside it.)
Your `nightlio_data` volume is never touched by any of the above — rolling
the compose files back does not roll back the database, and the database
was never modified in a way that requires rolling back (migrations are
additive/backward-compatible by the project's stated constraints).

## Verifying an upgrade

```bash
docker compose config                    # compose file renders, env vars resolve
docker compose --profile oidc config     # same, with the Pocket ID profile
docker compose logs -f api               # watch startup: migrations + healthcheck
curl -sf http://localhost:5000/api/      # api health endpoint
```

The api container reports healthy once the stdlib `urllib.request`
healthcheck passes; schema migrations have already run by then.
