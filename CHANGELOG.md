# Changelog

Notable changes to this fork of [shirsakm/nightlio](https://github.com/shirsakm/nightlio).

## [0.6.0] - 2026-08-24

### Added

- **Hot-swappable language packs (i18n).** Every UI string now lives in a nested JSON catalog (`src/i18n/en.json`, ~410 keys) behind a tiny typed `t()` runtime — no i18n library added. A new language ships as a standalone GitHub prerelease tagged `lang-<code>-v<semver>` (workflow: `.github/workflows/i18n-release.yml`); the API discovers and pulls packs at runtime via new unauthenticated endpoints `GET /api/i18n/languages` and `GET /api/i18n/{code}` (ETag + `Cache-Control` HTTP caching, disk-cached next to the database, serve-stale-on-failure), so adding or fixing a language needs **no redeploy** — English itself is hot-swappable the same way, with the bundled catalog as a permanent offline fallback. Settings gains a Language picker. Self-hosters can point `I18N_LOCAL_DIR` at a directory of packs (air-gapped) or set `I18N_OFFLINE=1`; contributor guide in `i18n/README.md`.
- **Deploys reach open tabs without hard reloads.** nginx now serves `/sw.js`, `/index.html`, and `/manifest.webmanifest` with `Cache-Control: no-cache` (hashed assets keep their 1-year immutable cache), so update detection is never pinned by the HTTP cache; the service worker additionally re-checks for a new build hourly and whenever a tab regains focus, then applies it via the existing silent reload. The Settings footer shows the running frontend version (and the API version when it differs).
- **`/api/v1` URL alias.** Every route now also answers under `/api/v1/…`, byte-identical to `/api/…`, as a stable namespace for third-party clients and a future escape hatch for breaking changes. `/api` remains the canonical, contract-graded surface; the bundled frontend keeps calling `/api` until every published API answers v1 (no earlier than 0.7).
- **The API now reports its version.** `GET /api/config` carries a new `version` field — the crate version baked in at compile time (`CARGO_PKG_VERSION`), so a running deployment can finally say what it is (no version number was exposed anywhere before, and the git tag never reaches the published image: the release retag job re-points, it doesn't rebuild). The three committed version numbers (`package.json`, `api/Cargo.toml`, `contract/openapi.yaml` `info.version`) are bumped to 0.6.0 and kept in lockstep by a new CI check (`scripts/check-version-sync.sh`, run by test.yml which publishing gates on); `scripts/bump-version.sh X.Y.Z` bumps all three together.
- **A real database migration framework.** Startup now records the schema in a `schema_migrations` ledger (paired SQLite/Postgres migration files embedded in the binary, SHA-256 checksums, one transaction per migration) and refuses to start on checksum tampering, downgrades, or numbering gaps — each with an actionable message. Existing SQLite databases are **adopted, never rebuilt**: the legacy bootstrap still runs first, untouched, and remains the upgrade path from any historical file. Fully automatic; nothing changes in how you upgrade. Policy and details in `docs/MIGRATIONS.md`.
- **Opt-in PostgreSQL backend (experimental).** Set `DATABASE_URL=postgres://…` to run the main data store on PostgreSQL 16+ (tokio-postgres + deadpool, rustls TLS); a `postgres` compose profile ships in `docker-compose.yml` (`docker compose --profile postgres up -d`). A fresh database is schema-bootstrapped by the migration framework on first boot. SQLite stays the default and the only fully e2e-graded backend in 0.6.0 — unset `DATABASE_URL` and nothing changes. Setup, TLS notes, and the SQLite-data import flow: `docs/POSTGRES.md`.
- **`nightlio-api migrate-to-postgres`** — one-shot import of an existing SQLite database into the PostgreSQL backend. The SQLite source is opened read-only and never modified; PostgreSQL migrations run first; the tool refuses to touch a non-empty target; all 10 tables are copied in foreign-key order preserving row ids verbatim (including historical oddities like US-format dates); identity sequences are advanced past the copied ids; the whole import is one transaction with per-table row-count verification printed and enforced. Walkthrough in `docs/POSTGRES.md`.
- **The PostgreSQL backend is graded against the same wire contract as SQLite.** Every integration harness now replays its recorded fixtures against both backends: SQLite always (unchanged), and — when `NIGHTLIO_PG_TEST_URL` points at a disposable PostgreSQL 16+ server — a PG-backed app on a private template-cloned database per test. No PG-specific fixtures exist; a new `rust-pg` CI job (postgres:16-alpine service) runs the gated suite, including a full `migrate-to-postgres` round-trip test, on every push.
- **Journal data export & import (JSON).** The roadmap's last unchecked item: `GET /api/export/data` returns every entry and goal as a versioned JSON envelope (`schema_version: 1` — importers will accept v1 files forever; an unknown version is rejected with a clear 400), and `POST /api/import/data` merges one back in. Import is **merge with duplicate skipping** — entries matched by date + content, goals by title — so re-importing the same file is a no-op and restoring into a non-empty instance is safe; tag selections travel by group/option *name* and are resolved or created on the target; the whole import is one all-or-nothing transaction and the response reports imported/skipped counts. Settings gains a **Data** card with Export (downloads `nightlio-export-YYYY-MM-DD.json`) and Import buttons. Full format spec: `docs/EXPORT-FORMAT.md`.
- **One-command demo instance.** `docker compose -f docker-compose.demo.yml up --build` boots a self-contained, zero-`.env`, PostgreSQL-backed demo stack at `http://localhost:5173`, pre-seeded with 14 days of markdown journal entries, 6 goals with completion history, and naturally-unlocked achievements; `down -v` resets it. The seeding is a new `nightlio-api seed-demo` subcommand (idempotent — safe to re-run; `--force` appends another batch) that works against either backend, so a scratch SQLite instance can be filled with the same demo data.
- **i18n release polish.** The language-pack pipeline's loose ends are closed: `docker-compose.yml` now passes all six `I18N_*` variables through to the api (previously there was no way to set them through compose), with matching documentation in `.env.example`, `.env.docker`, and `docs/SETUP.md`; the release-tag grammar in `scripts/build-lang-pack.mjs` and the `i18n-release.yml` workflow is locked to exactly what the server-side parser accepts (`lang-<code>-vX.Y.Z`, code `[a-z]{2,8}` plus at most one subtag, strict version triple — no prerelease/build suffixes), and the pack validator now rejects language codes the server would refuse; a reference English pack ships at `i18n/packs/en.json` with `yarn i18n:validate` / `yarn i18n:build` wrapper scripts; and a new self-hoster guide, `docs/I18N.md`, covers the zero-config default, adding a language, and air-gapped mode.

### Fixed

- **PostgreSQL deployments now get the same default seed as SQLite.** A fresh SQLite database has always been seeded with the self-host user and the three default tag groups (Emotions, Sleep, Productivity — 27 options); a fresh PostgreSQL database got nothing, so a PG-backed deployment started with an empty tag picker. Startup now applies the same baseline seed on PostgreSQL, idempotently, on every boot — existing PG deployments are backfilled automatically at their next restart.

[0.6.0]: https://github.com/FayedPatel/nightlio/releases/tag/v0.6.0

## [0.5.0] - 2026-08-20

A small polish release: a friendlier entry editor and a rebuilt image-publishing pipeline.

### Fixed

- **The entry editor starts truly empty.** The "How was your day?" template used to be real editor content you had to delete before typing; it is now a faded native placeholder that disappears on focus, works under every theme, and never triggers an autosave when nothing has been typed.

### Publishing & CI

- **Exactly one image build per merge.** A release used to build every image twice — once for the merge to `main`, once for the `v*` tag — producing two different digests for the same commit. Tag pushes no longer rebuild: a lightweight retag job re-points the release version at the image the merge already built, so `0.5.0` and `latest` are byte-identical.
- Provenance attestations are disabled (`provenance: false`), removing two untagged "unknown/unknown" manifests per push from the package listings.
- New weekly **GHCR cleanup workflow** (multi-arch aware): deletes dangling untagged manifests and keeps only the newest 8 per-commit `sha-*` versions. Release versions, `latest`, and `main` are never touched.

### Docs

- README no longer claims single-digit megabytes of API memory; real-world usage with data is 10–16 MB.

[0.5.0]: https://github.com/FayedPatel/nightlio/releases/tag/v0.5.0

## [0.4.0] - 2026-08-16

The full rewrite: the Flask/Python backend is replaced by a Rust API (Axum + rusqlite) and the entire frontend and browser-test suite is converted to strict TypeScript. Same app, same data, same wire contract where it made sense — and a deliberately better one where it didn't.

### The Rust API

- Complete port of the backend as a single Rust binary: routes with Flask-parity semantics (slash aliases, int-converter 404s, response envelopes), the versioned SQLite bootstrap and migrations, JWT + `nightlio_token` cookie auth, CSRF checks, OIDC, and the login rate limiter.
- **Argon2 rehash-on-login**: new password hashes are argon2id; legacy Werkzeug scrypt/pbkdf2 hashes keep verifying and are transparently upgraded on the next successful login.
- **PDF export renders in-process** via the `markdown2pdf` crate (replacing the Python `markdown-pdf` library; the wire contract is unchanged).
- **WAL journal mode**, size-bounded with checkpoint-truncate on shutdown.
- Correctness fixes along the way: correctly-rounded float JSON parsing, a latent stale-week clamp bug in goal updates, and schema-legal REAL mood values no longer 500.

### Measured against the v0.3.0 image (same host, side by side)

- API image: 624 MB → **158 MB** on disk (161 MB → 41 MB compressed).
- API memory under load: ~290 MiB (gunicorn worker pool) → **~7.5 MiB** (single process).
- Tail latency at 20 concurrent requests: p99 12 ms → **6 ms** on health, 19 ms → **8 ms** on authenticated statistics, at equal-or-better throughput.

### Contract changes (the full ledger lives in `contract/DECISIONS.md`)

- **Reads are pure now.** `GET /api/goals` no longer writes the weekly rollover (it is projected into the response and persisted only by write operations), and `GET /api/statistics` no longer increments anything.
- **Data Lover counts days, not page loads**: the new `POST /api/statistics/view` records at most one statistics view per day, and the achievement means "viewed statistics on 10 different days". Existing progress counters reset under the fairer rule; already-earned badges are kept.
- **One `new_achievements` shape**: `POST /api/mood` returns the same metadata objects as `/api/achievements/check` instead of bare strings.
- Every `OPTIONS` answers `204` with an `Allow` header; `405`s use the standard JSON error envelope; `GET /api/mood/{id}/selections`, goal completions, and group options return consistent `404`s; `%m/%d/%Y` dates are normalized to ISO by a startup migration; `next_cursor` is `null` on an exactly-full final page.
- The default CORS origins no longer include a third-party domain — deployments relying on the implicit `nightlio.vercel.app` grant must set `CORS_ORIGINS`.
- **`POST /api/export/pdf` requires authentication** (2026-08-17), closing the last unauthenticated route — it was open only as Flask parity, leaving a render-cost DoS surface on exposed instances. The frontend has always sent credentials, so no client breaks.

### TypeScript frontend

- All of `src/` and every Playwright spec converted to strict TypeScript under a 4-project tsconfig (app, node, e2e, service worker), with a typed `ApiService` derived from the OpenAPI document.
- The statistics view no longer double-fetches on entry (one route-owner effect replaces duplicate Sidebar/BottomNav effects); accessibility lint (`jsx-a11y`) is at error severity.

### Contract oracle & testing

- New `contract/` directory: golden request/response fixtures for all 46 URL rules recorded against the live Flask app before its removal, a hand-written OpenAPI 3.1 document, corpus database snapshots for migration byte-parity, and `contract/DECISIONS.md` — the closed ledger of every deliberate behavior decision.
- Backend: 400 unit + 109 integration tests (fixture-graded, insta snapshots), `clippy -D warnings` clean. Migration parity asserted byte-for-byte against three generations of real production databases before the Flask code was deleted.
- CI: the e2e suite is sharded 4-way with the API binary pre-built per shard and a shared cargo cache; cargo fmt/clippy/test run alongside the yarn gates.
- Docker images publish for **linux/amd64 and linux/arm64**.

### Removed

- The entire Flask/Python backend, its requirements files, `test.sh`, and the pytest suite (−9,432 lines).
- The Python demo-data seeder and assorted personal operational scripts (now local-only).

### Upgrading

`git pull` / `docker compose up -d --build` as usual — the startup migration handles the schema change automatically. Ship the API and frontend images together, and see `docs/UPGRADING.md` for the Data Lover counter reset and the OPTIONS `200 → 204` note.

[0.4.0]: https://github.com/FayedPatel/nightlio/releases/tag/v0.4.0

## [0.3.0] - 2026-08-14

Themes, backdating, a redesigned welcome page, and a full browser test suite.

### Added

- **Configurable themes**: Default (Dracula purple), Light, Dark (near-black), and Synthwave, selectable from Settings → Appearance or the header toggle. The choice is saved to the account (`users.theme_preference`, new `GET`/`PUT /api/preferences`) and follows you across devices; the column is added automatically by the startup migrations on upgrade.
- **Backdated journal entries**: forgot a day? The editor has an entry-date row (date input + Yesterday/Today chips) for any past date; future dates are rejected client- and server-side. History ordering, streaks, the Today card, and the weekly chart all treat backdated days correctly regardless of stored date format.
- **Backdated goal completions**: a "Log past day" control on goal cards records a completion for the day it actually happened (`POST /api/goals/<id>/progress` now accepts an optional `date`). Same-day repeats never double-count; current-week days move the weekly counter (capped at the goal's frequency); previous-week days feed the statistics calendar without rewriting closed weeks or streaks.
- **Synthwave welcome page**: complete redesign of the landing page (banded sunset, perspective grid, terminal-style quickstart) with a sticky nav that keeps Sign in reachable at any scroll depth, plus a rewritten About page telling the fork's story and crediting the original author.
- **`DISABLE_LOCAL_LOGIN`** env flag for SSO-only deployments: hard-refuses `POST /api/auth/local/login` entirely — both the username/password form and credential-free self-host mode — so the identity provider is the only door. Fails closed; the login page hides the dead form via the new `enable_local_login` field in `/api/config`.
- One-click flows: Home's Add Goal card lands directly on the open creation form, and History's Add Entry card goes straight into the editor's mood prompt (it previously just bounced back to Home).
- `darkreader-lock` meta: Nightlio manages its own dark themes, so the Dark Reader extension no longer re-darkens the app (its rewriting broke gradient text into an unreadable ghost).

### Fixed

- Editing an entry from History no longer blank-screens (relative-navigation bug under the nested router); unknown dashboard paths bounce home instead of rendering nothing.
- The mobile purple floating button (FAB) that promised a new entry but only scrolled the page is removed.
- Creating categories/options in Manage Categories works again (the request sent a bare string instead of a JSON object).
- Search no longer freezes the History page in an infinite render loop.
- On phones, page content now clears the fixed bottom navigation — the last card was hidden underneath it and untappable.
- Theme choice no longer silently reverts when the login-time server preference sync resolves after a theme toggle.
- Entry cards in a grid row are equal height, and goal-card buttons are bottom-aligned across cards.
- `OIDC_SIGNUP_URL` and `VITE_API_URL` are validated so only absolute http(s) URLs are ever rendered or used (a `file:`/`javascript:` value can no longer reach the login page).

### Testing & CI

- Full test pyramid: 171 pytest API tests, 37 Vitest unit tests, and a 90-test Playwright browser suite covering every feature on desktop (1280 px), a Galaxy S25 Ultra profile (620 px), and Pixel 7 (412 px).
- Parallel e2e: each Playwright worker boots its own API and SQLite file behind one vite dev server (header-routed proxy) — the suite dropped from ~5 minutes to ~1.5.
- The harness never attaches to already-running servers (a busy port fails the run loudly) so a local run can no longer wipe real data.
- New CI workflow runs all three suites on every pull request and push to `main`.

[0.3.0]: https://github.com/FayedPatel/nightlio/releases/tag/v0.3.0

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
- **npm → Yarn** migration (`yarn.lock`, Node 24 engines requirement); Python requirements split into runtime/dev; eslint-plugin-jsx-a11y added.
- Test suite expanded with coverage for cookie auth, group ownership, schema migrations, secret validation, extended statistics, rate limiting, PDF export, and more.
- Docs refreshed throughout: README, `docs/DEPLOYMENT.md`, `docs/DOCKER.md`, and a new `docs/UPGRADING.md` for existing deployments.

### License

- Aligned with upstream's relicense: the project is **AGPL-3.0-only** (`package.json` previously still claimed MIT).

[0.2.0]: https://github.com/FayedPatel/nightlio/releases/tag/v0.2.0
