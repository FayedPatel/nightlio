# Development

Technical reference for contributors: architecture, local setup, the API surface, the data model, and how the README media is generated. For the measured before/after of the v0.4.0 Rust rewrite (image size, memory, latency, contract), see [`REWRITE.md`](REWRITE.md).

## Architecture Overview

* **Frontend:** React 19 + Vite, written in strict TypeScript, served by Nginx.
* **Backend:** Rust (Axum) JSON API in `api/`, shipped as a single compiled binary (dynamically linked, running on a `debian:bookworm-slim` runtime image). The wire contract is pinned by `contract/openapi.yaml` plus golden request/response fixtures in `contract/fixtures/`, and mirrored in `src/types/api.ts`.
* **PDF export:** rendered in-process by the Rust API via the `markdown2pdf` crate (see `contract/DECISIONS.md` #15) — no extra service required.
* **Database:** SQLite (WAL journal mode), with auto-migrations on startup (`api/src/db/`).
* **Authentication:** JWT-based (Bearer header or httpOnly cookie). Supports credential-free single-user self-host mode, local username/password accounts (argon2id, with transparent rehash of legacy hashes), and optional generic OIDC single sign-on (Pocket ID recommended).

## Local Development Setup

**Prerequisites:** Node.js v24+ (see `engines` in `package.json`), Yarn, Rust (stable toolchain with `cargo`)

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

All protected endpoints require an `Authorization: Bearer <jwt>` header (or the `nightlio_token` httpOnly cookie) unless otherwise noted. The full wire contract is `contract/openapi.yaml` — 37 paths / 50 operations, of which 33 are authenticated. Every route also answers `OPTIONS` with an empty `204` plus an `Allow` header, before any token check.

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
* `POST /api/export/pdf { content }` → PDF file download (`entry_export.pdf`) — **requires auth** (since 2026-08-17; see `contract/DECISIONS.md`); rendered in-process (see `contract/DECISIONS.md` #15), `content` capped at 1 MiB of UTF-8 bytes → 413
* `GET /api/music/vibe[?tag=chill]` → track suggestion for the given mood tag (requires `ENABLE_MOOD_MUSIC=1` and `JAMENDO_CLIENT_ID`)
* `GET /api/preferences` → { theme } — requires auth; `theme` is `null` until the user has stored one
* `PUT /api/preferences { theme }` → { status, theme } — requires auth; `theme` must be one of `default`, `light`, `dark`, `synthwave`, otherwise 400

**Moods & Statistics**
* `POST /api/mood { date, mood(1-5), content, time?, selected_options?: number[] }` → 201 { entry_id, new_achievements[] }
* `GET /api/moods[?start_date=YYYY-MM-DD&end_date=YYYY-MM-DD]` → list of entries
* `GET /api/mood/:id` → entry
* `PUT /api/mood/:id { mood?, content? }` → success
* `DELETE /api/mood/:id` → success
* `GET /api/mood/:id/selections` → options linked to the entry
* `GET /api/statistics` → { statistics, mood_distribution, current_streak } — pure read
* `POST /api/statistics/view` → { counted } — records at most one statistics view per day (feeds the Data Lover achievement)
* `GET /api/statistics/extended` → { rolling_averages, weekday_averages, mood_volatility, tag_correlations, goal_correlations, monthly_digest } — partly clock-relative: the volatility window is the trailing 30 days and `monthly_digest` is always the current server-local month
* `GET /api/statistics/heatmap[?year=2026]` → { year, days: [{ date, average_mood, entry_count }], days_logged } — only logged days appear; `year` defaults to the current server-local year, 400 if it is not an integer in 1970–2100
* `GET /api/statistics/digest[?year=2026&month=8]` → { year, month, entries_logged, average_mood, previous_average_mood, mood_trend, top_tags (≤5), longest_streak } — month-in-review; `year`/`month` default to the current server-local month, 400 if out of range
* `GET /api/streak` → { current_streak, message }

**Goals**
* `GET /api/goals` → bare array of goals, `created_at DESC` — the weekly rollover is projected on read only (pure read; it is persisted solely by the write paths), and every row carries the computed `already_completed_today`
* `POST /api/goals { title, description?, frequency_per_week }` → 201 { id } — `frequency` is a legacy alias consulted only when `frequency_per_week` is absent; 400 on a blank title or an effective frequency outside 1–7
* `GET /api/goals/:id` → the goal (same projection and row shape as the list); 404 if missing or owned by another user
* `PUT /api/goals/:id { title?, description?, frequency_per_week? }` → { status } — partial update; lowering `frequency_per_week` clamps `completed`; 400 on an empty body or a blank title
* `PATCH /api/goals/:id` → alias of `PUT` (same rule, same handler, same responses)
* `DELETE /api/goals/:id` → { status } — `goal_completions` rows cascade-delete (200 with a body, never 204)
* `POST /api/goals/:id/progress { date? }` → the updated goal plus { already_logged, logged_date } — body is optional and means "today"; `date` accepts `YYYY-MM-DD` or `M/D/YYYY` and may be at most 1 day in the future; logging is idempotent per (goal, day)
* `GET /api/goals/:id/completions[?start=YYYY-MM-DD&end=YYYY-MM-DD]` → bare array of { date } ascending — without **both** bounds the window is the last 90 days ending today; 404 for a missing/foreign goal
* `GET /api/goals/:id/completions/` and `GET /api/goal/:id/completions` → byte-identical aliases of the above (separately registered rules, not redirects)

**Groups & Options**
* `GET /api/groups` → [{ id, name, options: [{ id, name }] }]
* `POST /api/groups { name }` → { group_id }
* `POST /api/groups/:group_id/options { name }` → { option_id }
* `DELETE /api/groups/:group_id` → success
* `DELETE /api/options/:option_id` → success

**Achievements**
* `GET /api/achievements` → user achievements (with metadata)
* `POST /api/achievements/check` → { new_achievements, count }
* `GET /api/achievements/progress` → fixed-key map of all five achievement types (`first_entry`, `week_warrior`, `consistency_king`, `data_lover`, `mood_master`) to { current, max }, with maxima 1 / 7 / 30 / 10 / 100 and `current` clamped to that range; `GET /api/achievements/progress/` is a byte-identical trailing-slash alias

## Data Model

**Tables (SQLite):**
* `users`: id, auth_provider ('local' or 'oidc'), external_id (provider-scoped subject, unique per (auth_provider, external_id)), email, name, avatar_url, password_hash (local accounts only, nullable), theme_preference (nullable, no default — backs `/api/preferences`), created_at, last_login — a legacy unique-identifier column from the pre-OIDC schema remains for backward compatibility with rows created before this change but is no longer written by application logic
* `mood_entries`: id, user_id(FK), date, mood, content, ...
* `groups`: id, name
* `group_options`: id, group_id(FK), name
* `entry_selections`: entry_id(FK), option_id(FK)
* `goals`: id, user_id(FK), title, description, frequency_per_week, completed, streak, period_start, last_completed_date, created_at, updated_at
* `goal_completions`: id, goal_id(FK), date — UNIQUE per (goal, day), which is what makes progress logging idempotent; cascades on goal delete
* `achievements`: id, user_id(FK), achievement_type, earned_at, ...
* `activity_log`: id, user_id(FK), event_type, metadata, created_at — backs `GET /api/activity`
* `user_metrics`: user_id(PK/FK), stats_views, last_view_date — per-day statistics-view tracking

## Regenerating the README media

The GIFs in the README are real captures from the running app (desktop at 720p, mobile at an S25-Ultra-class 620×1340 viewport, synthwave theme):

```bash
README_CAPTURES=1 yarn playwright test e2e/readme-captures.spec.ts --project=chromium
# then, in any throwaway scratch directory (outside the repo), run
# `npm i gifenc pngjs` once and point the script at it:
node scripts/build-readme-gifs.mjs <that-scratch-directory>
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
