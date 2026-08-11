# Changelog

Notable changes to this fork of [shirsakm/nightlio](https://github.com/shirsakm/nightlio).

## [0.2.0] - 2026-08-10

Complete overhaul of authentication, security posture, deployment, and analytics on top of upstream 0.1.7.

### Authentication & Multi-User

- **Removed Google OAuth entirely** and replaced it with generic OpenID Connect: any spec-compliant provider works, with [Pocket ID](https://github.com/pocket-id/pocket-id) (passkey-based) as the recommended self-hosted option, bundled as an opt-in `oidc` compose profile.
- **Local username/password accounts** (`POST /api/auth/local/register`, `POST /api/auth/local/login`) alongside OIDC — family-account model where any authenticated user can create another account.
- **Credential-free single-user login** for simple self-hosting remains the default; it fails closed (403) as soon as OIDC is configured.
- **httpOnly session cookie** (`nightlio_token`) issued in addition to the bearer token, plus a new `POST /api/auth/logout`; cookie `Secure` flag respects `TRUST_PROXY_HEADERS` behind TLS-terminating proxies.
- **Per-user data isolation**: groups/tags are now scoped to their owning user (schema migration with backfill), previously-unprotected routes require auth, and entry/group access is ownership-checked in SQL.

### Security Hardening

- `SECRET_KEY` / `JWT_SECRET` are **required with no insecure fallback** — compose refuses to start without them and the api rejects blank or well-known placeholder values in production.
- **Both containers run as non-root**: the api as a dedicated uid-1000 user, the frontend on `nginxinc/nginx-unprivileged` (container port 80 → 8080).
- Rate limiter backed by a shared store so limits hold across multiple workers; security headers middleware; `TRUST_PROXY_HEADERS` opt-in for correct client IPs behind a proxy.
- Added `SECURITY.md` with the threat model and disclosure process.
- Removed a committed personal-data file (`watch-history.json`) and documented the history purge procedure (`docs/history-purge.md`).

### Removed

- All Web3/NFT code: `api/routes/web3_routes.py`, wagmi/contracts config, `Web3Context`, `useNFT`.
- Platform leftovers: Railway debug script, `Procfile`, Vercel config, Heroku-era shell scripts, `nginx-prod.conf`, `.github/FUNDING.yml`.
- `docker-compose.prod.yml` is no longer shipped; a single `docker-compose.yml` covers local, LAN, and public deployment behind your own reverse proxy.

### Statistics & Activity

- Extended analytics: tag–mood correlation, rolling averages, day-of-week patterns, and mood volatility, served by a dedicated stats database layer and rendered in new statistics sections.
- Per-user **activity feed**: new `activity_log` table and keyset-paginated `GET /api/activity`, recording logins and journaling events.

### Mobile & PWA

- Responsive layout pass across the app: bottom navigation, mobile-friendly modals, reworked history view with a Today entry card and recent entries.
- Installable **PWA**: manifest, icons, service worker, and offline fallback page.

### Deployment & Tooling

- Compose rework: `build:` stanzas for from-source builds with `API_IMAGE`/`WEB_IMAGE` overrides, trustworthy stdlib healthchecks, full `.env` pass-through (CORS, proxy, OIDC, music vars), and the gated Pocket ID profile.
- **GHCR publish workflow**: builds and pushes `nightlio-api` / `nightlio-frontend` images on pushes to `main` and `v*` tags, with `latest`, branch, `sha-*`, and semver tags.
- Operational scripts: `scripts/upgrade-server.sh` (migrate an upstream-image deployment to this fork with volume backups), `scripts/relink-oidc-user.sh` (re-attach an account to a new OIDC identity, seeding default groups), `scripts/fix-group-ownership.sh` (one-time repair for the per-user groups migration).
- **npm → Yarn** migration (`yarn.lock`, Node 24 engines requirement); Python requirements split into runtime/dev; eslint-plugin-jsx-a11y added.
- Test suite expanded with coverage for cookie auth, group ownership, schema migrations, secret validation, extended statistics, rate limiting, PDF export, and more.
- Docs refreshed throughout: README, `docs/DEPLOYMENT.md`, `docs/DOCKER.md`, and a new `docs/UPGRADING.md` for existing deployments.

### License

- Aligned with upstream's relicense: the project is **AGPL-3.0-only** (`package.json` previously still claimed MIT).

[0.2.0]: https://github.com/FayedPatel/nightlio/releases/tag/v0.2.0
