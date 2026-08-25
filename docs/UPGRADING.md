# Upgrading an existing Nightlio deployment

This doc is for self-hosters who already have a running Nightlio. It covers
what changes, why your data is safe, and the exact commands to run. Sections
are newest-first; older sections are kept as history for anyone upgrading
across several versions.

## Upgrading v0.5.x → v0.6.0

Nothing manual — the standard `git pull` / `docker compose up -d --build`
flow applies. SQLite stays the default backend and your data volume is
untouched. What to expect:

- **A `schema_migrations` ledger appears in your database.** v0.6.0 adds a
  real migration framework: after the existing legacy bootstrap runs
  (unchanged), startup records the schema in a `schema_migrations` table and
  applies any newer migrations, fully automatically. There is no command to
  run and no change to the upgrade steps; the rules the framework enforces
  (and what its "Refusing to start" messages mean) are in
  [docs/MIGRATIONS.md](MIGRATIONS.md).
- **PostgreSQL is now available, opt-in and experimental.** Set
  `DATABASE_URL=postgres://…` to run the main data store on PostgreSQL 16+;
  a matching `postgres` compose profile ships in `docker-compose.yml`.
  Leave `DATABASE_URL` unset and **nothing changes** — SQLite remains the
  default and the only fully e2e-graded backend in 0.6.0. Setup, TLS notes,
  and the SQLite-data import flow: [docs/POSTGRES.md](POSTGRES.md).
- **PostgreSQL deployments are backfilled with the default seed.** If you
  were already running the PG backend from a pre-release build, note that a
  fresh PG database previously got no self-host user or default tag groups
  (SQLite always seeded them). Startup now applies that baseline seed on
  PostgreSQL idempotently, so an affected deployment is backfilled
  automatically at its next restart — no action needed.
- **`/api/v1` is a new, purely additive URL alias.** Every route also
  answers under `/api/v1/…`, byte-identical to `/api/…`. Nothing existing
  moved; `/api` remains canonical and the shipped frontend keeps using it.
- **`GET /api/config` now carries a `version` field** — the running API's
  crate version. Additive; clients that ignore unknown fields are
  unaffected.
- **i18n needs zero configuration.** The UI now runs on a bundled English
  catalog, and additional languages arrive as hot-swappable language packs
  discovered at runtime. All six new `I18N_*` variables are optional
  (defaults work out of the box); air-gapped instances can set
  `I18N_OFFLINE=1` or serve packs from `I18N_LOCAL_DIR` — see
  [docs/I18N.md](I18N.md).
- **Ship the API and frontend images together, as usual.** v0.6.0 also
  improves deploy freshness for the *next* upgrade: nginx now serves
  `/index.html`, `/sw.js`, and the manifest with `Cache-Control: no-cache`,
  and the service worker re-checks for a new build hourly and on tab focus,
  so open tabs pick up deploys without hard reloads. The normal compose
  flow rebuilds both images at once; this only matters if you pin the two
  images to different versions.

## Upgrading to the Rust rewrite (statistics view tracking, OPTIONS 204)

Nothing manual — the standard `git pull` / `docker compose up -d --build`
flow applies, and the startup migration handles the schema change
automatically. What to expect:

- **The "Data Lover" achievement now counts *days*, not page loads.** It
  used to increment a counter on every statistics fetch (so one visit could
  count several times); it now counts at most one statistics view per day,
  and unlocks after viewing statistics on 10 different days. As part of
  this change the migration **resets the in-progress counter to 0** for
  every user — progress toward Data Lover restarts under the fairer
  per-day rule. **Already-earned badges are kept**; nothing you have
  unlocked disappears.
- **Upgrade the API and frontend images together.** An older frontend
  running against a rewritten API keeps working, but it does not call the new
  `POST /api/statistics/view` endpoint, so Data Lover progress stops
  accruing until the frontend is updated. The normal compose flow rebuilds
  both at once — this only matters if you pin the two images to different
  versions.
- **OPTIONS responses changed from 200 to 204.** Browsers and the shipped
  frontend do not care (CORS preflights work identically, with the same
  CORS headers). Only non-browser clients that literally assert
  `status == 200` on an OPTIONS request will notice.
- Under the hood the schema migration adds `user_metrics.last_view_date`
  and stamps `PRAGMA user_version = 3`. Rolling back the images after this
  migration is safe for the API (older code ignores the extra column), but
  the Data Lover counter reset is not reversible.

## Upgrading v0.3.x → v0.4.0 (Rust API cutover)

v0.4.0 replaces the Python/Flask API with a Rust binary (`api/`). From
the outside nothing moves: same `nightlio_data` volume, same
`DATABASE_PATH` default, same port `5000`, same `.env` contract, and both
images run as uid-1000 `appuser`, so file ownership in the volume is
unchanged. Live sessions survive the swap (same JWT format and secrets),
and the wire contract was held to the recorded Flask behavior via the
golden fixtures in `contract/fixtures/` (accepted, documented differences:
`contract/DECISIONS.md`).

Upgrade is the standard flow:

```bash
git pull
docker compose pull        # if using published images
docker compose up -d --build
```

Startup schema migrations run exactly as before, ported 1:1 — the cutover
was validated by running the migration plus full endpoint parity against
three generations of real production databases before removing the legacy
backend (see the "Legacy removal" entry in `contract/DECISIONS.md`).

Cheap insurance before upgrading — take a backup while the old (still
Python-based) container is running:

```bash
docker compose exec api python -c "import sqlite3; sqlite3.connect('/app/data/nightlio.db').backup(sqlite3.connect('/app/data/pre-v0.4.0.db'))"
```

After the upgrade the api image contains no Python or sqlite3 CLI; back up
with the volume-level command instead:

```bash
docker run --rm -v nightlio_nightlio_data:/data -v $(pwd):/backup alpine tar czf /backup/nightlio-backup.tar.gz -C /data .
```

WAL journal mode (new): the Rust API switches the main database to SQLite's
WAL journal mode, so you may now see `nightlio.db-wal` and `nightlio.db-shm`
files next to `nightlio.db` in the volume. This is normal. The `-wal` file
is bounded — SQLite's default 1000-page auto-checkpoint folds it back into
the main file during normal writes, `journal_size_limit` truncates it to at
most 4 MB after checkpoints, and the API truncates it to zero bytes at
startup and on graceful shutdown — it will not grow without limit.

This changes how you should take backups: copying `nightlio.db` alone while
the container is running can miss recent writes still sitting in the `-wal`
file. Either stop the container first (`docker compose stop api` — the
graceful shutdown folds the WAL into `nightlio.db`), or archive the whole
data directory including the `-wal`/`-shm` files, as the `tar` command above
already does (it copies `-C /data .`, i.e. everything).

Rollback: **there is no supported downgrade to a pre-v0.4.0 image.**

- Running an older `nightlio-api` image tag against a v0.4.0+ volume is
  **formally retired** as a rollback path (`contract/DECISIONS.md`, the WAL
  and legacy-removal entries). The legacy Python source and its compose
  rollback profile are gone from the repository, removal having been gated
  on the real-data validation described above.
- Going backwards is **best-effort only, and it means restoring a backup**.
  Take the `tar` archive above *before* you upgrade; to go back, stop the
  stack, restore that archive over the `nightlio_data` volume, and start the
  older image against the restored copy. Anything written since the backup
  is lost — there is no way to replay it into the older schema.
- Do not point an old image at a live v0.4.0+ volume and hope. Even though
  WAL journal mode is readable by any SQLite from 3.7.0 (2010) onward, the
  data has moved forward under migrations the old code does not know about,
  and nothing in the project verifies that combination any more.

Rolling *forward* remains the supported direction: upgrade, and keep the
pre-upgrade archive until you're satisfied.

CORS default change: the old Flask API's built-in `CORS_ORIGINS` default
included `https://nightlio.vercel.app`, which granted credentialed
cross-origin access to that third-party domain on any deployment that never
set `CORS_ORIGINS`. The v0.4.0 default is
`http://localhost:5173,http://localhost:5000` (localhost only). If your
deployment relied on the implicit vercel origin — or any non-localhost
origin — you must now set `CORS_ORIGINS` explicitly in `.env`
(comma-separated, no spaces around commas).

Also new in v0.4.0: PDF export (`POST /api/export/pdf`) is rendered
in-process by the Rust API (`markdown2pdf` crate) — no extra service, no
`.env` change; output styling differs from the old Python renderer but the
response contract is unchanged (`contract/DECISIONS.md` #15).

PDF export now requires authentication. The endpoint was unauthenticated in
every earlier release (an oversight inherited from the Flask backend, see
`SECURITY.md`); it now behaves like every other data route — send
`Authorization: Bearer <jwt>`, or the `nightlio_token` cookie together with
`Content-Type: application/json` and `X-Requested-With: nightlio`. The
Nightlio web UI always sent credentials, so nothing changes for normal use;
only a custom script or integration that called `/api/export/pdf` anonymously
needs updating (it will now get `401 {"error": "Authorization header
required"}`).

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
mode) so your identity provider is the only door. Note the current default:
unset is *not* the same as `0` — with `OIDC_ISSUER_URL` configured, local
login is off unless you set `DISABLE_LOCAL_LOGIN=0` explicitly.

The rest of this document covers the older compose rework — the upgrade that
consolidated the compose files and reworked the container topology.

> **Status:** final. The auth overhaul (local password login + generic OIDC,
> Google removal) and the session-cookie change (httpOnly `nightlio_token`,
> `POST /api/auth/logout`) have both shipped. This doc describes shipped
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
before the compose rework and is unchanged by it) — you do not need to run
anything by hand.

## What actually changed (compose/infra layer)

1. **`docker-compose.yml` now has `build:` stanzas** for `api` and
   `frontend`, alongside the existing `image:` keys. `docker compose up -d
   --build` on a fresh clone now builds from source instead of requiring the
   prebuilt `ghcr.io/shirsakm/*` images. If you were already overriding
   `API_IMAGE` / `WEB_IMAGE` to pull prebuilt images, that keeps working
   unchanged — nothing forces a rebuild if you don't pass `--build`.
2. **API healthcheck fixed** in `docker-compose.yml`. It used to run
   `python -c "import requests; requests.get('http://localhost:5000/api/')"`,
   but `requests` was never a declared dependency of the Flask api image —
   the check only ever "passed" by accident (something else pulled
   `requests` in transitively). That upgrade swapped it for a stdlib
   `urllib.request` one-liner. Both are pre-v0.4.0 history: the Rust api
   image ships no Python at all, and the current healthcheck is the binary's
   own subcommand, `nightlio-api --health-check`. If your container was
   reporting unhealthy for unrelated reasons, re-check after upgrading — the
   healthcheck itself is trustworthy now.
3. **Optional Pocket ID service** added to `docker-compose.yml` as an `oidc`
   compose profile, off by default. See "OIDC / Pocket ID" below.
4. **`docker-compose.prod.yml` was removed.** There is now a single
   `docker-compose.yml` for every environment — local, LAN, and public
   production. If you were deploying with `docker compose -f
   docker-compose.prod.yml ...`, switch to the plain `docker compose ...`
   commands against `docker-compose.yml`, and put your own TLS-terminating
   reverse proxy
   (Caddy, Traefik, or nginx) in front of the `frontend` service's port
   instead of the bundled nginx+TLS setup that file used to provide. Set
   `TRUST_PROXY_HEADERS=1` and `CORS_ORIGINS` to your public origin in
   `.env` — see the "Self-hosting" section of `docs/SETUP.md` for the exact
   steps.
   No compose file still ships a `./ssl` bind mount or nginx TLS config;
   certificates are your reverse proxy's responsibility now.

None of the compose/infra work above touches migrations — those already ran
automatically before the compose rework and still do.

Separately, in the same era (auth overhaul and session cookie, now shipped):
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

**Remove** from your `.env` (no longer read by anything — the Google-OAuth
removal deleted the corresponding api code):
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
All three default to empty string in `docker-compose.yml` if unset — you do
not need to add them to `.env` at all unless you're enabling OIDC.

**Configuring OIDC disables local login by default.** Once
`OIDC_ISSUER_URL` is set, your identity provider becomes the only door:
`POST /api/auth/local/login` returns `403` for both the credential-free
single-user path *and* local username/password accounts. If you want local
passwords to keep working next to OIDC, set `DISABLE_LOCAL_LOGIN=0`
explicitly — an explicit `0` or `1` always wins over the OIDC-derived
default. See "OIDC / Pocket ID" below for the exact callback URL to
register.

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
`APP_ENV` replaced `RAILWAY_ENVIRONMENT` as the name of the environment
selector. The old name is still honored as a fallback — the api reads
`APP_ENV` first, then `RAILWAY_ENVIRONMENT`, then defaults to `production`
— so you do not need to change anything. `docker-compose.yml` already sets
`APP_ENV=production` for you; the current implementation lives in
`api/src/config.rs`. This line documents intent for anyone editing the
compose file directly or running the api container standalone outside
compose.

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
callback endpoint, not a frontend page (`api/src/auth/oidc.rs` handles the
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
URL above).

**Image note:** compose pins `ghcr.io/pocket-id/pocket-id:latest`. For a
long-lived deployment, replace `:latest` with a specific version tag from
Pocket ID's release page so upgrades of your identity provider are something
you choose rather than something that happens on the next `docker compose
pull`.

## Migrations

Unchanged behavior: schema migrations against `DATABASE_PATH` run
automatically on every container start, before serving traffic. This was
true before the compose rework, and it is still true today — the current
bootstrap lives in `api/src/db/bootstrap.rs`, and the environment selector
rename (`APP_ENV`, falling back to `RAILWAY_ENVIRONMENT`) never touched
migration logic. The tables and columns added by the auth overhaul
(`auth_provider`, `external_id`, `password_hash` on `users`; the
`activity_log` table) are created and backfilled by that same automatic
path — you do not run any migration command by hand; `docker compose up -d
--build` is sufficient.

Since v0.6.0, that legacy bootstrap is joined by a real migration framework:
after the bootstrap completes, startup records the schema in a
`schema_migrations` ledger and applies any newer migrations, still fully
automatic (no command to run, no change to the upgrade steps above). The
rules it enforces — and what its "Refusing to start" messages mean — are
documented in [docs/MIGRATIONS.md](MIGRATIONS.md). v0.6.0 also adds an
opt-in, experimental PostgreSQL backend selected via `DATABASE_URL`; SQLite
stays the default and nothing changes unless you set that variable — see
[docs/POSTGRES.md](POSTGRES.md), including how to move existing SQLite data
over.

## Rollback (compose files only)

This section is about reverting the *compose/infra* changes described above,
not about downgrading the api itself — for that, see the v0.4.0 rollback
notes near the top of this document. If something goes wrong after the
compose rework:
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

The api container reports healthy once its healthcheck passes — today that
is the binary's own `nightlio-api --health-check` subcommand, wired up in
`docker-compose.yml`. Schema migrations have already run by the time it
goes healthy.
