# Development

Technical reference for contributors: architecture, local setup, the API surface, the data model, and how the README media is generated. For the measured before/after of the v0.4.0 Rust rewrite (image size, memory, latency, contract), see [`REWRITE.md`](REWRITE.md).

## Architecture Overview

* **Frontend:** React 19 + Vite, written in strict TypeScript, served by Nginx.
* **Backend:** Rust (Axum) JSON API in `api/`, shipped as a single static binary. The wire contract is pinned by `contract/openapi.yaml` plus golden request/response fixtures in `contract/fixtures/`, and mirrored in `src/types/api.ts`.
* **PDF export:** rendered in-process by the Rust API via the `markdown2pdf` crate (see `contract/DECISIONS.md` #15) — no extra service required.
* **Database:** SQLite (WAL journal mode), with auto-migrations on startup (`api/src/db/`).
* **Authentication:** JWT-based (Bearer header or httpOnly cookie). Supports credential-free single-user self-host mode, local username/password accounts (argon2id, with transparent rehash of legacy hashes), and optional generic OIDC single sign-on (Pocket ID recommended).

## Local Development Setup

**Prerequisites:** Node.js v18+, Yarn, Rust (stable toolchain with `cargo`)

```bash
# Install frontend dependencies
yarn install

# Terminal 1: Vite frontend dev server
yarn dev

# Terminal 2: Rust API dev server (dev-mode secrets, SQLite in api/data/)
cd api
APP_ENV=development cargo run
```
The frontend will be available at `http://localhost:5173`, the API at
`http://localhost:5000`. PDF export works out of the box — the API
renders it in-process (`markdown2pdf` crate), no extra service needed.

## API Reference

All protected endpoints require an `Authorization: Bearer <jwt>` header unless otherwise noted. The full wire contract is `contract/openapi.yaml`.

**Auth**
* `POST /api/auth/local/login { username, password }` → 200 { token, user } — credentialed login, works regardless of OIDC config
* `POST /api/auth/local/login` (empty body, no credentials) → 200 { token, user } — credential-free single-user self-host login; only works when OIDC is **not** configured (`OIDC_ISSUER_URL` unset). Returns 403 once OIDC is configured.
* `POST /api/auth/local/register { username, password, email?, name? }` → 201 { user } — **requires auth**; any already-authenticated user may create another local account (family-account model, no admin role). 400 on weak/missing password (min 8 chars) or missing username, 409 if the username is taken.
* `POST /api/auth/verify` → 200 { user } — requires auth (Bearer header or the `nightlio_token` httpOnly cookie)
* `POST /api/auth/logout` → 200 { status } — clears the session cookie; no auth required, safe to call with no session
* `GET /api/auth/login/oidc` → redirects to the configured OIDC provider's authorization endpoint; 404 if OIDC is not configured
* `GET /api/auth/callback/oidc` → OIDC callback (register this as the redirect URI with your provider); on success, redirects to the SPA with the token in the URL fragment (`#sso_token=...`) and sets the session cookie; on failure, redirects with `#sso_error=...`

**Config & Misc**
* `GET /api/config` → { enable_oidc, enable_mood_music, enable_local_login }
* `GET /api/` → health payload
* `GET /api/time` → { time }
* `GET /api/activity[?before=<id>&limit=50]` → { activities, next_cursor } — requires auth; per-user activity feed, keyset-paginated on `id` (pass the previous page's `next_cursor` as `before` to fetch older events; `limit` clamped 1–200)
* `POST /api/export/pdf { content }` → PDF file download (`entry_export.pdf`), rendered in-process (see `contract/DECISIONS.md` #15)
* `GET /api/music/vibe[?tag=chill]` → track suggestion for the given mood tag (requires `ENABLE_MOOD_MUSIC=1` and `JAMENDO_CLIENT_ID`)

**Moods & Statistics**
* `POST /api/mood { date, mood(1-5), content, time?, selected_options?: number[] }` → 201 { entry_id, new_achievements[] }
* `GET /api/moods[?start_date=YYYY-MM-DD&end_date=YYYY-MM-DD]` → list of entries
* `GET /api/mood/:id` → entry
* `PUT /api/mood/:id { mood?, content? }` → success
* `DELETE /api/mood/:id` → success
* `GET /api/mood/:id/selections` → options linked to the entry
* `GET /api/statistics` → { statistics, mood_distribution, current_streak } — pure read
* `POST /api/statistics/view` → { counted } — records at most one statistics view per day (feeds the Data Lover achievement)
* `GET /api/streak` → { current_streak, message }

**Groups & Options**
* `GET /api/groups` → [{ id, name, options: [{ id, name }] }]
* `POST /api/groups { name }` → { group_id }
* `POST /api/groups/:group_id/options { name }` → { option_id }
* `DELETE /api/groups/:group_id` → success
* `DELETE /api/options/:option_id` → success

**Achievements**
* `GET /api/achievements` → user achievements (with metadata)
* `POST /api/achievements/check` → { new_achievements, count }

## Data Model

**Tables (SQLite):**
* `users`: id, auth_provider ('local' or 'oidc'), external_id (provider-scoped subject, unique per (auth_provider, external_id)), email, name, avatar_url, password_hash (local accounts only, nullable), created_at, last_login — a legacy unique-identifier column from the pre-OIDC schema remains for backward compatibility with rows created before this change but is no longer written by application logic
* `mood_entries`: id, user_id(FK), date, mood, content, ...
* `groups`: id, name
* `group_options`: id, group_id(FK), name
* `entry_selections`: entry_id(FK), option_id(FK)
* `achievements`: id, user_id(FK), achievement_type, earned_at, ...
* `activity_log`: id, user_id(FK), event_type, metadata, created_at — backs `GET /api/activity`
* `user_metrics`: user_id(PK/FK), stats_views, last_view_date — per-day statistics-view tracking

## Regenerating the README media

The GIFs in the README are real captures from the running app (desktop at 720p, mobile at an S25-Ultra-class 620×1340 viewport, synthwave theme):

```bash
README_CAPTURES=1 yarn playwright test e2e/readme-captures.spec.ts --project=chromium
# then, with any directory that has `npm i gifenc pngjs`:
node scripts/build-readme-gifs.mjs <that-directory>
```

GIFs land in `docs/assets/`, raw frames in `screenshots/readme-frames/` (gitignored) before assembly.

## Contributing

Pull requests are welcome! For major changes, please open an issue first to discuss what you would like to change.

Please ensure you run the tests before opening a PR:

```bash
cd api && cargo test && cargo clippy -- -D warnings && cd ..
                 # backend (includes contract snapshot tests
                 # against contract/fixtures/)
yarn typecheck   # frontend type check (tsc)
yarn test:unit   # frontend unit tests (Vitest)
yarn test:e2e    # browser tests (Playwright; runs its own servers —
                 # stop any docker compose stack on ports 5000/5173 first)
```

CI runs all of these suites on every pull request (documentation-only changes skip the suites). Please add tests for any new API functionality or user-facing behavior.
