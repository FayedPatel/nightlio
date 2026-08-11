# Security

Nightlio is designed to be self-hosted: a small Flask API backed by a single SQLite file,
served behind nginx, optionally exposed to the internet through a reverse proxy or tunnel
(e.g. Caddy + playit.gg) with OIDC login. The threat model below, and the fixes in this
document, are written with that deployment shape in mind — a single operator running one
instance for themselves or a small group, not a multi-tenant SaaS.

## Reporting a vulnerability

Open a GitHub issue or contact the maintainer directly (see the repository owner's profile).
Please avoid filing public issues for anything that would let an attacker take over an
existing self-hosted instance (auth bypass, RCE, secret disclosure) until a fix is available.

## Fixed findings (this pass)

The following were found by an adversarial audit of the container images, secret handling,
and the unauthenticated PDF export endpoint, and are fixed as of this change. Every
self-hoster upgrading past this point should read the "what you must do" line for each.

### 1. API container ran as root (`api/Dockerfile`)

**Old behavior:** no `USER` directive; the Flask process (and anything it spawns, e.g.
`markdown-pdf`'s rendering) ran as `root` (uid 0) inside the container.
**New behavior:** the image creates `appuser` (uid 1000), chowns `/app/data` to it, and
switches to it with `USER appuser` before `CMD`. A compromised dependency or a bug reachable
from an HTTP route can now only do what `appuser` can do: read the app code and write inside
`/app/data`. It cannot modify root-owned files or use a container-escape bug as a root
starting point.
**What a self-hoster must do:** if you have an existing `nightlio_data` volume created by an
older (root-run) image, its files are still owned by `root`/uid 0 and the new unprivileged
user will fail to write to them. Fix an existing volume once with:

```sh
docker run --rm -v nightlio_data:/data alpine chown -R 1000:1000 /data
```

A fresh volume created from this image needs no action.

### 2. Frontend (nginx) container ran as root (`Dockerfile`)

**Old behavior:** based on `nginx:stable-bookworm` with no `USER` directive; nginx's master
process ran as root to bind port 80, and any exploit reachable through the unauthenticated
static file server or the `/api/` proxy (path traversal, a malformed-request parser bug,
etc.) would land as root inside the container.
**New behavior:** based on `nginxinc/nginx-unprivileged:stable-bookworm`, which runs both the
master and worker processes as uid 101 (`nginx`) and listens on port 8080 instead of 80
(binding <1024 requires root). No `USER` directive is needed — the base image already
defaults to the unprivileged user.
**What a self-hoster must do:** nothing for the host-side port — `docker-compose.yml` and
`docker-compose.prod.yml` already map the host port (5173, or whatever you configured) to the
container's new internal 8080. If you run the image directly with `docker run` outside
compose, update any `-p hostport:80` to `-p hostport:8080`.

### 3 & 4. Hardcoded default `SECRET_KEY` / `JWT_SECRET` fallback (`api/config.py`)

**Old behavior:** if the `SECRET_KEY` / `JWT_SECRET` environment variables were unset, the
app silently fell back to the literal string `"dev-secret-key-change-in-production"` with no
warning and no startup check. Both tokens are signed with this value
(`api/routes/auth_routes.py`, `api/utils/auth_middleware.py`); anyone who read this public
repository knew the default and could forge a valid JWT for any `user_id`, bypassing
authentication entirely on any instance that forgot to set these variables.
**New behavior:** `api/config.py:is_weak_secret()` rejects a value that is missing, blank, a
known placeholder (the old hardcoded default plus every placeholder string that has ever
shipped in `.env.example` / `docker-compose.yml` / `docker-compose.prod.yml` / `.env.docker`),
or shorter than 16 characters. `api/app.py:create_app()` calls this for both
`app.config["SECRET_KEY"]` and `app.config["JWT_SECRET_KEY"]` and, **when
`config_name == "production"`**, raises `RuntimeError` and refuses to start rather than boot
with a guessable signing key. `api/docker_start.py` catches that specific error, prints it to
stderr, and exits non-zero, so the container fails its healthcheck/restart loop visibly
instead of silently serving traffic with a forgeable key. `docker-compose.yml` and
`docker-compose.prod.yml` additionally use `${SECRET_KEY:?...}` / `${JWT_SECRET:?...}` so
`docker compose up` refuses to even start the container if the `.env` file doesn't set them —
defense in depth for the common path, backed by the in-app check for anyone who runs the
image a different way (bare `docker run`, Railway, manual gunicorn).
**What a self-hoster must do:** set distinct, random values for both `SECRET_KEY` and
`JWT_SECRET` in `.env` before deploying with `APP_ENV=production` — e.g.
`openssl rand -hex 32` run twice. If you already have real values set, nothing changes for
you; this only blocks the case where they were never set (or left at a placeholder) in the
first place. **Development/testing configs are intentionally exempt** from the startup check
and keep working with no secret set at all, matching the previous behavior — this is a
deliberate, scoped decision (see "Risk-accepted items" below), not an oversight.

### 5. Unbounded PDF export content size (`api/routes/misc_routes.py`)

**Old behavior:** `POST /api/export/pdf` accepted a `content` field of any size (up to
Flask's default request-body ceiling) and passed it straight to `MarkdownPdf`, which renders
synchronously inside whichever gunicorn worker handles the request with no size or time limit
of its own. A large, hard-to-lay-out payload could tie up a worker's CPU and memory for
minutes on ordinary hardware.
**New behavior:** `MAX_PDF_CONTENT_SIZE = 1 * 1024 * 1024` (1 MiB, measured in UTF-8 encoded
bytes, not Python characters, so multi-byte text like emoji can't smuggle a larger payload
past a naive length check). Content over that cap is rejected with `413` before any rendering
happens.
**What a self-hoster must do:** nothing. A real journal entry is a few KB; 1 MiB is a
generous ceiling no legitimate export will hit.

## Risk-accepted items (not fixed in this pass)

* **`SECRET_KEY`/`JWT_SECRET` dev fallback still exists in source, scoped to non-production.**
  `api/config.py` still contains the literal fallback string for `config_name != "production"`
  so that `docker compose up` for local development, and the test suite, keep working with no
  `.env` at all. This is intentional — the production path is what actually gets exposed to
  the internet — but it does mean the fallback string is still present in the codebase for
  anyone who runs `APP_ENV=development` or `APP_ENV=testing` against a real network. Accepted
  because those configs are documented as local-only and are not the profile started by
  `docker-compose.yml`/`docker-compose.prod.yml` (both hardcode `APP_ENV=production`).

* **`/api/export/pdf` remains unauthenticated at the route level.** The size cap above closes
  the specific resource-exhaustion finding (unbounded payload size), but the endpoint itself
  still has no `@require_auth` and no per-endpoint rate limit (unlike
  `api/routes/auth_routes.py`'s login/register routes, which use the SQLite-backed limiter in
  `api/utils/rate_limiter.py`). The frontend (`src/services/api.js:exportPdf`) already sends
  the auth token/cookie when it calls this endpoint, which suggests the missing
  `@require_auth` is an oversight rather than a deliberate design choice, and a request-volume
  DoS (many requests just under the 1 MiB cap) is still possible from an unauthenticated
  caller. This is flagged here rather than fixed in this pass because it wasn't part of the
  specific confirmed finding being addressed (which was scoped to content-size validation) and
  touches route-level auth semantics owned elsewhere in the current work — follow-up:
  add `@require_auth` and/or wrap the route with `rate_limit(...)` from
  `api/utils/rate_limiter.py`.
