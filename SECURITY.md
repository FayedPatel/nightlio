# Security

Nightlio is designed to be self-hosted: a single Rust (Axum) binary backed by one SQLite file,
with a static frontend served by nginx, optionally exposed to the internet through a reverse
proxy or tunnel with OIDC login. The threat model below is written with that deployment shape
in mind — one operator running one instance for themselves or a small group, not a
multi-tenant SaaS.

Concretely, that means:

* **In scope:** anything that lets an unauthenticated caller reach another user's data or the
  host — forged tokens, auth bypass, CSRF from a browser the operator is logged into, RCE,
  secret disclosure, container escape, resource exhaustion from an unauthenticated caller.
* **Out of scope by design:** a *logged-in* user abusing their own instance (they own the
  data and the CPU), and hardening against a hostile local network operator — the reverse
  proxy in front of Nightlio is trusted, and Nightlio only believes proxy headers when the
  operator explicitly opts in (`TRUST_PROXY_HEADERS`).

## Reporting a vulnerability

Open a GitHub issue or contact the maintainer directly (see the repository owner's profile).
Please avoid filing public issues for anything that would let an attacker take over an
existing self-hosted instance (auth bypass, RCE, secret disclosure) until a fix is available.

## Current posture

Every claim below points at the code that enforces it, so it can be re-checked rather than
trusted.

### Secrets and startup

* **Weak signing keys are rejected, not warned about.** `is_weak_secret()`
  (`api/src/config.rs:82-98`) rejects a value that is missing, blank, one of 11 known
  placeholder strings (`api/src/config.rs:61-73` — every placeholder that has ever shipped in
  an example env file or compose file, including the old hardcoded default), or shorter than
  `MIN_SECRET_LENGTH` = 16 characters (`api/src/config.rs:78`).
* **Production refuses to start on a weak key.** `Config::validate_production_secrets()`
  (`api/src/config.rs:318-331`) checks `SECRET_KEY` *and* `JWT_SECRET` and bails with
  "Refusing to start: …" plus an `openssl rand -hex 32` hint. It is called from
  `api/src/main.rs:34` before the runtime is even built, so the process exits non-zero and the
  container fails visibly instead of serving traffic with a forgeable key. The check is
  production-only (`APP_ENV=production`); see "Risk-accepted items".
* **Compose refuses to start too.** `docker-compose.yml:16-17` uses
  `${SECRET_KEY:?…}` / `${JWT_SECRET:?…}`, so `docker compose up` fails before the container
  exists if `.env` doesn't set them. `APP_ENV=production` is hardcoded for the api service
  (`docker-compose.yml:12`), so the in-app check always applies to the compose path.
  `POCKET_ID_ENCRYPTION_KEY` has the same `:?` guard (`docker-compose.yml:115`).
* **The env templates ship blank, not placeholder, secrets.** `SECRET_KEY=` / `JWT_SECRET=`
  in `.env.example` and `.env.docker` — copying a template verbatim fails loudly at startup
  rather than silently signing tokens with a value that is public in this repository.

**What a self-hoster must do:** set distinct random values for both `SECRET_KEY` and
`JWT_SECRET` in `.env` before deploying — e.g. `openssl rand -hex 32`, run twice.

### Containers

* **Neither container runs as root.** The API image creates `appuser` (uid 1000), chowns
  `/app/data` to it, and switches with `USER appuser` before `CMD`
  (`api/Dockerfile:99-105`); the runtime stage is `debian:bookworm-slim` carrying only the
  compiled binary and CA certificates. **The uid is pinned to 1000 on purpose**
  (`api/Dockerfile:89-98`): existing `nightlio_data` volumes were populated by a uid-1000
  process, and an unpinned `useradd` could pick a different uid and lock the new container out
  of its own database.
* **The frontend image runs nginx unprivileged.** `nginxinc/nginx-unprivileged`
  (`Dockerfile:40`) runs the master process — not just the workers — as uid 101 and listens on
  8080 instead of 80, because binding below 1024 needs root. Only the container-internal port
  changed; the host mapping (`5173:8080`) is unaffected.
* **The healthcheck needs no extra tooling.** The image ships no python and no curl; the
  binary answers its own check via `nightlio-api --health-check`
  (`api/Dockerfile:119-120`, `docker-compose.yml:48`), so the attack surface isn't widened by
  a shell utility that exists only for the healthcheck.
* **Pocket ID is opt-in and loopback-only.** The bundled OIDC provider sits behind
  `profiles: ["oidc"]` (`docker-compose.yml:106`), so a plain `docker compose up` never starts
  it, and its port is published as `127.0.0.1:1411:1411` — not reachable from the network.
  Before this, the service had no gate at all: it started on every `up`, forced every fresh
  clone to set `POCKET_ID_ENCRYPTION_KEY` just to bring up the base stack, and exposed Pocket
  ID's pre-passkey `/setup` and `/admin` endpoints to anyone who could reach the host.

**What a self-hoster must do:** if you have a `nightlio_data` volume created by an older
root-run image, its files are still owned by uid 0 and the unprivileged user cannot write to
them. Fix an existing volume once with:

```sh
docker run --rm -v nightlio_data:/data alpine chown -R 1000:1000 /data
```

A fresh volume needs no action.

### Authentication and sessions

* **Minimal, strictly validated JWTs.** The claim set is exactly
  `{user_id, exp, iat}`, all integers (`api/src/auth/jwt.rs:35-42`). Verification pins
  HS256, `required_spec_claims = ["exp"]`, and **leeway 0** (`api/src/auth/jwt.rs:94-97`) — no
  grace period on expiry. Float `exp`/`iat` are rejected outright (stricter than the
  python-jose the port replaced).
* **Session cookie flags.** `nightlio_token` is `HttpOnly; SameSite=Lax; Path=/;
  Max-Age=3600` (`api/src/auth/cookie.rs:64-73`), with `Max-Age` mirroring the JWT lifetime so
  the cookie never outlives the token. `Secure` is decided **per request**: direct TLS always
  sets it, and `X-Forwarded-Proto` (the value left by the trusted proxy — the proxy-header
  middleware consumes one value from the right) is consulted only when `TRUST_PROXY_HEADERS`
  is on, so a spoofable header can't flip the flag on an instance that hasn't opted into
  trusting its proxy.
* **CSRF is enforced for cookie-authenticated mutations.** The predicate
  (`api/src/auth/csrf.rs:67+`): a state-changing method (POST/PUT/PATCH/DELETE) authenticated
  by *cookie* must also send `Content-Type: application/json` and
  `X-Requested-With: nightlio`, else 403. Neither header is settable cross-origin without a
  preflight, so a form POST or an `<img>` from an attacker's page cannot ride the session
  cookie. `Authorization: Bearer` requests bypass the check entirely (they cannot be
  ambiently attached by a browser). Crucially, the check runs **inside the auth extractor,
  before the token is verified** (`api/src/auth/extract.rs:128-130`), so no route can
  accidentally skip it by ordering its extractors differently.
* **Passwords are argon2id, with legacy hashes upgraded in place.** New hashes use argon2id
  (PHC format, RustCrypto defaults, 16-byte OS-random salt —
  `api/src/auth/password.rs:184-237`). Legacy Werkzeug `scrypt`/`pbkdf2` rows still verify
  (constant-time comparison) and are transparently re-stored as argon2id right after a
  successful login (`api/src/routes/auth.rs:409-421`), on the blocking pool. A rehash or
  persist failure logs a warning and **never** fails the login — the legacy hash simply stays
  until next time.
* **SSO configured ⇒ local login disabled by default.** With `OIDC_ISSUER_URL` set and
  `DISABLE_LOCAL_LOGIN` unset, `/api/config` reports `enable_local_login: false` and local
  login answers 403 (`api/src/config.rs:264-272`). An explicit `0`/`1` always wins. The
  default fails toward the stricter posture: turning on SSO does not silently leave a password
  door open.
* **Login/register rate limiting.** A sliding 60-second window, 30/min for
  `/api/auth/local/login` and 10/min for `/api/auth/local/register`, keyed per client IP per
  endpoint. It lives in a **separate SQLite file** (`rate_limit.db`), never the application
  database, so limiter contention can never block or corrupt real data
  (`api/src/auth/rate_limit.rs:4-9; limits at :57-63`). Counting and insertion happen inside `BEGIN IMMEDIATE`
  (`api/src/auth/rate_limit.rs:157`) so concurrent workers cannot both slip through the same
  slot. `X-Forwarded-For` is honored only under `TRUST_PROXY_HEADERS`
  (`api/src/auth/rate_limit.rs:77-92`); otherwise the TCP peer address is used, so a direct
  client cannot spoof its way into a fresh bucket. There is a testing bypass, scoped to the
  test profile.
  **The limiter deliberately fails open** (`api/src/auth/rate_limit.rs:191-202`): a storage
  error logs a warning and lets the request through, because a corrupt limiter DB locking
  every operator out of their own instance is the worse failure for self-hosted software. The
  limiter is a brute-force speed bump, not the authentication boundary.

### Network and headers

* **CORS defaults are localhost-only.** `http://localhost:5173,http://localhost:5000`
  (`api/src/config.rs:46-51`). The inherited Flask default granted *credentialed* cross-origin
  access to a third-party domain (`https://nightlio.vercel.app`) on any deployment that never
  set `CORS_ORIGINS` — that grant is gone. Public deployments must set `CORS_ORIGINS`
  explicitly.
* **Security headers are stamped on every response.** `X-Frame-Options: DENY`,
  `X-Content-Type-Options: nosniff`, `X-XSS-Protection: 1; mode=block`, and
  `Referrer-Policy: strict-origin-when-cross-origin` (`api/src/routes/mod.rs:126-145`). They
  are layered **outside** the CORS layer (`api/src/routes/mod.rs:108-112`), so they also cover
  404s, 405s, redirects, and CORS preflights — the responses that header middleware most often
  misses.
* **CSP in production.** A `default-src 'self'` policy with `frame-src 'none'`
  (`api/src/routes/mod.rs:65-70`), applied only when `APP_ENV=production`
  (`api/src/routes/mod.rs:147-153`). See "Risk-accepted items" for the `'unsafe-inline'`
  caveat.
* **Proxy headers are opt-in.** The `X-Forwarded-*` handling middleware is only installed
  when `TRUST_PROXY_HEADERS` is truthy (`api/src/routes/mod.rs:158-162`), and consumes one
  value from the *right* of each header — the value the trusted proxy appended, not one a
  client injected. Without a stripping proxy in front, these headers are client-controlled,
  so the default is to ignore them.
* **Configured URLs are scheme-checked.** `OIDC_SIGNUP_URL` flows into an `<a href>` on the
  login page, so `safe_http_url()` (`api/src/config.rs:116-131`) accepts only absolute
  `http`/`https` URLs with a non-empty host; `javascript:`, `file:`, and friends are dropped
  with a warning.

### Data

* One SQLite file, opened with `foreign_keys=ON` and a 5-second busy timeout
  (`api/src/db/common.rs:170-171; WAL bounds at :174-213`). WAL is enabled but **bounded**:
  `journal_size_limit=4194304` truncates the `-wal` file back to ≤4 MiB after checkpoints, and
  `wal_checkpoint(TRUNCATE)` runs after bootstrap and on graceful shutdown — the log cannot
  grow without limit and disappears on a clean stop.
* Every per-user read and write carries its ownership predicate **in the SQL**
  (`WHERE user_id = ?`), rather than filtering after the fetch, so a missing check is a query
  that returns nothing rather than a query that leaks another user's row.

### PDF export

`POST /api/export/pdf` renders markdown to a PDF. As of 2026-08-17 it **requires
authentication** like every other data route (see the ledger entry in
`contract/DECISIONS.md`); a credential-less call gets 401, and a cookie-authenticated call
must satisfy the CSRF predicate above. Content is capped at 1 MiB measured in **UTF-8 encoded
bytes, not characters** (so multi-byte text like emoji cannot smuggle a larger payload past a
naive length check), rejected with 413 before any rendering happens; a separate 8 MiB
transport limit sits above it. Rendering is in-process via the `markdown2pdf` crate on a
blocking task — there is no subprocess, no sidecar service, and no shell involved.

It is deliberately **not** rate-limited. With authentication required, request-volume abuse is
an authenticated user's self-harm on their own single-operator instance, and the 1 MiB cap
already bounds the cost of any single request.

## Historical fixes (v0.2.0 audit)

An adversarial audit of the container images, secret handling, and the PDF export endpoint ran
against the pre-rewrite codebase. All five findings are fixed; the posture that replaced them
is documented above. **The file paths in these findings refer to the removed Flask backend —
see git history.** Kept here only so upgraders understand what changed and why:

1. **API container ran as root** (no `USER` directive) → unprivileged uid-1000 `appuser`.
2. **Frontend container ran as root** (nginx master bound port 80 as root) →
   `nginx-unprivileged`, uid 101 on port 8080.
3. **Hardcoded `SECRET_KEY` fallback** — a value published in this repository, usable to forge
   a JWT for any `user_id` on any instance that never set it → weak-secret detection plus a
   production refuse-to-start check.
4. **Same for `JWT_SECRET`** → same fix; both keys are validated.
5. **Unbounded PDF export content size** — arbitrarily large markdown rendered synchronously
   with no size or time limit → the 1 MiB UTF-8-byte cap.

The only remediation step that still applies to a real deployment is the volume ownership fix
for finding 1, repeated here for convenience — old volumes created by a root-run image need it
once:

```sh
docker run --rm -v nightlio_data:/data alpine chown -R 1000:1000 /data
```

The audit's one risk-accepted follow-up — `/api/export/pdf` having no auth — was closed on
2026-08-17 (see "PDF export" above).

## Risk-accepted items

Known, deliberate, and documented rather than fixed. Each is a trade-off, not an oversight.

* **The dev secret fallback still exists in source, scoped to non-production.**
  `DEV_SECRET_KEY` (`api/src/config.rs:55`) is the last resort of the signing-key chain
  (`JWT_SECRET` → `JWT_SECRET_KEY` legacy alias → `SECRET_KEY` → fallback,
  `api/src/config.rs:249-255`) so local development and the test suite work with no `.env` at
  all. Production can never reach it: `validate_production_secrets()` classifies it as weak and
  refuses to boot. Accepted because the production path is the one exposed to the internet —
  but it does mean anyone running `APP_ENV=development` on a reachable network is signing
  tokens with a value published in this repository. Don't do that.

* **The production CSP carries `'unsafe-inline'` in `script-src` and `style-src`**
  (`api/src/routes/mod.rs:65-70`). The Vite/React build emits inline styles and an inline
  bootstrap script, so a strict policy would need nonce or hash plumbing through the nginx
  layer. This meaningfully weakens the CSP's XSS mitigation value; the rest of the policy
  (`default-src 'self'`, `connect-src 'self'`, `frame-src 'none'`) still holds, and the API
  itself returns only JSON and PDFs — no server-rendered HTML that could reflect input.

* **`POST /api/auth/logout` requires no authentication** (`api/src/routes/auth.rs:9`). It is an
  idempotent cookie clear: the worst an unauthenticated caller achieves is clearing a cookie
  they must already control, and requiring auth would mean an expired session couldn't log
  itself out cleanly.

* **The rate limiter fails open.** Covered above (`api/src/auth/rate_limit.rs:191-202`) — a
  limiter storage failure lets requests through rather than locking the operator out of their
  own instance. Login still requires a correct password; the limiter only slows guessing.

* **Compose publishes the API on all host interfaces** (`5000:5000`,
  `docker-compose.yml:42`). The frontend proxies `/api/` internally over the compose network,
  so operators who only expose the frontend can narrow this to `127.0.0.1:5000:5000` and lose
  nothing. Left as-is because direct API access is genuinely useful (scripts, mobile clients,
  debugging) and the API enforces its own authentication either way. Documented, not changed —
  change it yourself if your host is on an untrusted network.
