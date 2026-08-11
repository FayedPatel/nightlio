<div align="center">

<img src="https://raw.githubusercontent.com/shirsakm/nightlio/refs/heads/dev/public/logo.png" height="60px" />
<h1>Nightlio</h1>

[![License: AGPL-3.0](https://img.shields.io/badge/license-AGPL--3.0-blue?style=flat-square)](LICENSE)

*Private fork of [shirsakm/nightlio](https://github.com/shirsakm/nightlio): Pocket ID OIDC, hardened containers, expanded statistics, mobile UI/PWA.*

**Privacy-first mood tracker and daily journal, designed for effortless self-hosting.** <br />
**Your data, your server, your rules.**

</div>

<!-- <img width="1366" height="645" alt="image" src="https://github.com/user-attachments/assets/dd50ec1f-4c3f-4588-907c-dca6ac1f7f98" /> -->
![Preview](https://github.com/user-attachments/assets/77f52abc-b4f8-439d-9bb2-772e3996256c)

## Why Nightlio?

Nightlio was inspired by awesome mood-tracking apps like Daylio, but born out of frustration with aggressive subscription models, paywalls, and a lack of cross-platform access. I wanted a beautiful, effective tool to log my mood and journal my thoughts without compromising on privacy or being locked into a single device.

Nightlio is the result: a feature-complete, open-source alternative that you can run anywhere. It's fully web-based and responsive for use on both desktop and mobile. No ads, no subscriptions, and absolutely no data mining. Just you and your data.

### Key Features

* **Rich Journaling with Markdown:** Write detailed notes for every entry using Markdown for formatting, lists, and links.
* **Track Your Mood & Find Patterns:** Log your daily mood on a simple 5-point scale and use customizable tags (e.g., 'Sleep', 'Productivity') to discover what influences your state of mind.
* **Insightful Analytics:** View your mood history on a calendar, see your average mood over time, and track your journaling streak to stay motivated.
* **Privacy First, Always:** Built from the ground up to be self-hosted. Your sensitive data is stored in a simple SQLite database file on *your* server. No third-party trackers or analytics.
* **Simple Self-Hosting with Docker:** Get up and running in minutes with a single `docker compose up` command.
* **Gamified Achievements:** Stay consistent with built-in achievements that unlock as you build your journaling habit.

<div align="center">🌙</div>

## Usage

### Docker Quickstart (Recommended)

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
`main` (`latest`, `sha-*` tags) and on `v*` release tags (semver tags).

### Self-hosting

Nightlio ships a single `docker-compose.yml` that works for both local/LAN
use and public deployment.

#### Local / LAN

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

#### Public deployment

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

<div align="center">🌙</div>

### 🔧 Configuration (`.env`)

You can customize your Nightlio instance using environment variables in the `.env` file.

#### Server (API)
```
# Core
APP_ENV=production          # was RAILWAY_ENVIRONMENT; still honored as a fallback
FLASK_ENV=production
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

# Mood music (if enabled)
JAMENDO_CLIENT_ID=

# CORS - Add your frontend's domain if deploying publicly
CORS_ORIGINS=http://localhost:5173,https://your.domain.com

# Optional: only if the frontend is on a different origin than the api
# FRONTEND_URL=

# Optional: only if the api sits behind a trusted reverse proxy
# TRUST_PROXY_HEADERS=0
```

#### Frontend (Vite)
```
# This is only needed for local development outside of Docker
VITE_API_URL=http://localhost:5000
```

#### OIDC Single Sign-On (Optional)

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

Setting `OIDC_ISSUER_URL` also disables credential-free single-user login
(`POST /api/auth/local/login` with no body starts failing closed with 403) —
local username/password accounts (`/api/auth/local/register`) keep working
alongside OIDC.

<div align="center">🌙</div>

## Developer Reference

Interested in contributing or running the project without Docker? Here's what you need to know.

<details>
<summary><strong>Architecture Overview</strong></summary>

* **Frontend:** React 19 + Vite, served by Nginx.
* **Backend:** Flask (Python) serving a JSON API.
* **Database:** SQLite, with auto-migrations on startup.
* **Database layer:** Modular mixins live in `api/database_*.py` with `api/database.py` acting as the facade.
* **Authentication:** JWT-based (Bearer header or httpOnly cookie). Supports credential-free single-user self-host mode, local username/password accounts, and optional generic OIDC single sign-on (Pocket ID recommended).
</details>

<details>
<summary><strong>Local Development Setup</strong></summary>

**Prerequisites:** Node.js v18+, Yarn, Python v3.11+

```bash
# Install frontend dependencies
yarn install

# Setup and activate backend virtual environment
cd api
python -m venv venv
source venv/bin/activate   # On Windows: venv\Scripts\activate
pip install -r requirements.txt
cd ..

# Run both servers concurrently
yarn dev # Starts Vite frontend dev server
yarn api:dev   # Starts Flask backend API server
```
The frontend will be available at `http://localhost:5173`.
</details>

<details>
<summary><strong>API Reference</strong></summary>

All protected endpoints require an `Authorization: Bearer <jwt>` header unless otherwise noted.

**Auth**
* `POST /api/auth/local/login { username, password }` → 200 { token, user } — credentialed login, works regardless of OIDC config
* `POST /api/auth/local/login` (empty body, no credentials) → 200 { token, user } — credential-free single-user self-host login; only works when OIDC is **not** configured (`OIDC_ISSUER_URL` unset). Returns 403 once OIDC is configured.
* `POST /api/auth/local/register { username, password, email?, name? }` → 201 { user } — **requires auth**; any already-authenticated user may create another local account (family-account model, no admin role). 400 on weak/missing password (min 8 chars) or missing username, 409 if the username is taken.
* `POST /api/auth/verify` → 200 { user } — requires auth (Bearer header or the `nightlio_token` httpOnly cookie)
* `POST /api/auth/logout` → 200 { status } — clears the session cookie; no auth required, safe to call with no session
* `GET /api/auth/login/oidc` → redirects to the configured OIDC provider's authorization endpoint; 404 if OIDC is not configured
* `GET /api/auth/callback/oidc` → OIDC callback (register this as the redirect URI with your provider); on success, redirects to the SPA with the token in the URL fragment (`#sso_token=...`) and sets the session cookie; on failure, redirects with `#sso_error=...`

**Config & Misc**
* `GET /api/config` → { enable_oidc, enable_mood_music }
* `GET /api/` → health payload
* `GET /api/time` → { time }
* `GET /api/activity[?before=<id>&limit=50]` → { activities, next_cursor } — requires auth; per-user activity feed, keyset-paginated on `id` (pass the previous page's `next_cursor` as `before` to fetch older events; `limit` clamped 1–200)
* `POST /api/export/pdf { content }` → PDF file download (`entry_export.pdf`); 501 if the optional `markdown-pdf` module is not installed
* `GET /api/music/vibe[?tag=chill]` → track suggestion for the given mood tag (requires `ENABLE_MOOD_MUSIC=1` and `JAMENDO_CLIENT_ID`)

**Moods**
* `POST /api/mood { date, mood(1-5), content, time?, selected_options?: number[] }` → 201 { entry_id, new_achievements[] }
* `GET /api/moods[?start_date=YYYY-MM-DD&end_date=YYYY-MM-DD]` → list of entries
* `GET /api/mood/:id` → entry
* `PUT /api/mood/:id { mood?, content? }` → success
* `DELETE /api/mood/:id` → success
* `GET /api/mood/:id/selections` → options linked to the entry
* `GET /api/statistics` → { statistics, mood_distribution, current_streak }
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

</details>

<details>
<summary><strong>Data Model</strong></summary>

**Tables (SQLite):**
* `users`: id, auth_provider ('local' or 'oidc'), external_id (provider-scoped subject, unique per (auth_provider, external_id)), email, name, avatar_url, password_hash (local accounts only, nullable), created_at, last_login — a legacy unique-identifier column from the pre-OIDC schema remains for backward compatibility with rows created before this change but is no longer written by application logic
* `mood_entries`: id, user_id(FK), date, mood, content, ...
* `groups`: id, name
* `group_options`: id, group_id(FK), name
* `entry_selections`: entry_id(FK), option_id(FK)
* `achievements`: id, user_id(FK), achievement_type, earned_at, ...
* `activity_log`: id, user_id(FK), event_type, metadata, created_at — backs `GET /api/activity`
</details>

<div align="center">🌙</div>

## Security & Privacy

* **Data Ownership:** Your data is stored in a local SQLite file. You can back it up, move it, or delete it at any time.
* **No Telemetry:** This application does not collect any usage data or send information to third-party services.
* **Secure Authentication:** API endpoints are protected using JSON Web Tokens (JWT).
* **Configurable CORS:** Restrict API access to trusted domains via environment variables.

See [SECURITY.md](./SECURITY.md) for the full threat model, vulnerability disclosure process, and a record of hardening fixes self-hosters should be aware of when upgrading.

## Roadmap

Nightlio is actively developed. Here are some of the features planned for the future:
- [x] **Responsive Design:** Full support for usage on mobile devices, plus an installable PWA.
- [x] **Multi-User Support:** Multiple accounts on a single instance via local username/password and OIDC (Pocket ID) sign-in.
- [x] **Advanced Analytics:** Tag–mood correlations, rolling averages, day-of-week patterns, and mood volatility.
- [ ] **Data Import/Export:** Tools to import data from other services (like Daylio) and export your data to standard formats (JSON, CSV).
- [ ] **More Themes & Customization:** Additional themes and more options to personalize the look and feel of your journal.

## Contributing

Pull requests are welcome! For major changes, please open an issue first to discuss what you would like to change.

Please ensure you run the tests before opening a PR:

```bash
yarn test
```

This command runs the backend tests using `pytest`. Please ensure you add tests for any new API functionality.

## License

This project is licensed under the GNU Affero General Public License v3.0 - see the [LICENSE](LICENSE) file for details.
