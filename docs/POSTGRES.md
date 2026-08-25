# PostgreSQL backend (experimental)

Since v0.6.0 the API can run its main data store on PostgreSQL instead of
the embedded SQLite file. It is strictly **opt-in** and ships labeled
**experimental**:

- **SQLite remains the default** and the only backend graded by the full
  end-to-end suite in 0.6.0. If you don't set `DATABASE_URL`, nothing
  changes for you — ever.
- **PostgreSQL 16 or newer is required.** The schema uses `pg_input_is_valid`
  (16+) to reproduce SQLite's tolerant date handling.
- The Postgres schema deliberately mirrors SQLite's observable behavior
  (text timestamps in UTC, byte-order `COLLATE "C"` text ordering, float
  moods, 0/1 booleans) so the HTTP API behaves identically — the wire
  contract, not the storage bytes, is what's guaranteed.
- The login rate limiter keeps its own small SQLite file on both backends.

## Opting in

Backend selection is a single environment variable:

| `DATABASE_URL` | Backend |
| --- | --- |
| unset / empty | SQLite at `DATABASE_PATH` (default, unchanged) |
| `postgres://…` or `postgresql://…` | PostgreSQL |
| anything else | refuses to start (no silent fallback) |

```
DATABASE_URL=postgres://user:password@host:5432/nightlio?sslmode=disable
```

On first boot against an empty database the API creates the entire schema
itself (migration `0001_baseline` — see [docs/MIGRATIONS.md](MIGRATIONS.md)),
then seeds the same self-host baseline a fresh SQLite file gets: the default
self-host user and the three default tag groups (Emotions, Sleep,
Productivity, 27 options). Re-starts detect the recorded migrations and do
nothing; the baseline seed is idempotent and also backfills a deployment
that is missing it (e.g. one first booted on a pre-0.6.0 build of the
Postgres backend). Concurrent replicas are safe: the migration runner and
the baseline seed each serialize on a Postgres advisory lock.

### TLS

The driver stack is tokio-postgres + rustls. `sslmode` in the URL works as
usual (`disable`, `prefer` — the default — or `require`). Server
certificates are verified against the bundled web-PKI roots, which covers
managed providers with publicly-trusted certificates; **private CAs,
self-signed certificates, and client certificates are not supported** in
this experimental release — use `sslmode=disable` inside a trusted network
(such as the compose network below) in the meantime.

## Using the bundled compose service

`docker-compose.yml` ships a `postgres` profile service
(`postgres:16-alpine`, data on the named volume `nightlio_pg_data`, a
`pg_isready` healthcheck). It never starts unless you ask for the profile.

1. In `.env`, set:

   ```
   POSTGRES_PASSWORD=<openssl rand -hex 32>
   DATABASE_URL=postgres://nightlio:<that password>@postgres:5432/nightlio?sslmode=disable
   ```

   (Database name and user default to `nightlio`; override with
   `POSTGRES_DB` / `POSTGRES_USER` if you want.)

2. Start everything with the profile:

   ```bash
   docker compose --profile postgres up -d
   ```

   There is deliberately no `depends_on` between the api and the database (a
   profile-gated dependency would break the plain `docker compose up`). On
   the very first combined start the api may exit once while Postgres
   initializes its data directory; `restart: unless-stopped` retries and the
   stack converges.

3. Verify: `docker compose logs api` shows the experimental-backend notice
   and `migration 0001_baseline applied (postgres)` on first boot; the app
   works as before.

To go back to SQLite, remove `DATABASE_URL` from `.env` and `docker compose
up -d`. Your SQLite file was never touched.

## Bring your own PostgreSQL (external / managed)

Nothing ties the backend to the bundled compose service — any reachable
PostgreSQL **16 or newer** works, including managed offerings (RDS, Cloud
SQL, Neon, a NAS container, …).

Quickstart:

1. Create a role and an empty database for Nightlio. The repo ships this as
   [`scripts/setup-postgres.sql`](../scripts/setup-postgres.sql); its core is
   just:

   ```sql
   CREATE ROLE nightlio LOGIN PASSWORD 'change-this-password';
   CREATE DATABASE nightlio OWNER nightlio;
   ```

   (Managed providers usually do this from their console instead.) To reuse
   an existing shared database, skip `CREATE DATABASE` and grant the role
   `CREATE` on the schema — on PostgreSQL 15+ that grant is required:
   `GRANT CREATE ON SCHEMA public TO nightlio;`.

2. Point the api at it and start:

   ```
   DATABASE_URL=postgres://nightlio:change-this-password@your-db-host:5432/nightlio?sslmode=require
   ```

That is the whole setup: on the first connect the api creates every table,
function, and index itself (migration `0001_baseline`) and seeds the default
self-host user plus the three default tag groups. There is no schema to load
by hand, and re-starts are no-ops.

Notes for external servers:

- **`sslmode`**: `disable` (no TLS — trusted networks only), `prefer` (the
  default: TLS when the server offers it), `require` (refuse plaintext).
  Certificate verification uses the bundled web-PKI roots only — that covers
  managed providers with publicly-trusted certificates, but **private CAs,
  self-signed certificates, and client certificates are not supported** in
  this experimental release.
- The api container still needs its writable `/app/data` volume even on
  Postgres — the login rate limiter and the i18n cache live there.
- Back up with the provider's tooling or plain `pg_dump`; the `nightlio`
  database is self-contained.
- Rollback is the same as with the bundled service: unset `DATABASE_URL`
  and the api is back on its untouched SQLite file.

## Moving existing SQLite data: `migrate-to-postgres`

Switching backends does **not** copy data by itself — a fresh Postgres
database starts empty. The data import ships as a subcommand of the api
binary:

```bash
nightlio-api migrate-to-postgres
# reads DATABASE_PATH (source SQLite) and DATABASE_URL (target Postgres);
# override either with --sqlite <path> / --postgres-url <url>
```

Its guarantees:

- The SQLite source is opened **read-only**; it is never modified.
- Postgres migrations run first, then the tool **refuses to run unless the
  target's `users` table is empty** — it never merges into or overwrites
  existing data.
- All 10 tables are copied in foreign-key order **preserving row ids**, the
  identity sequences are `setval`-advanced past them, and the whole import
  is **one transaction** — it lands completely or not at all.
- Row counts are verified table-by-table after the copy; a mismatch aborts
  (and rolls back) the import.

Recommended order: stop the api, run the import, set `DATABASE_URL`, start
the api. Keep the SQLite file (or a copy) — it is your rollback path.

## Demo stack with fake data

Want to click around a filled-in Nightlio (or show one to someone) without
touching your real deployment? The repo ships a self-contained demo compose
file that boots the Postgres backend and pre-seeds it:

```bash
docker compose -f docker-compose.demo.yml up --build
# UI: http://localhost:5173   API: http://localhost:5000
```

No `.env` needed. A one-shot `seed` service runs `nightlio-api seed-demo`
before the api starts, filling the database with two weeks of markdown mood
entries (moods 1–5, tagged with the default groups), six goals with
backdated progress, and the achievements those naturally unlock. The seeder
is idempotent — restarting the stack does not duplicate data.

> **Warning:** demo only, never production. The compose file bakes in
> public, insecure secrets and database credentials so it can boot with
> zero configuration.

- Full reset: `docker compose -f docker-compose.demo.yml down -v` (drops
  the demo volumes; the next `up` re-seeds).
- Inspect the database directly (published on localhost only):
  `psql -h 127.0.0.1 -p 5433 -U nightlio_demo nightlio_demo` (password
  `nightlio-demo-password`).
- The UI/API ports match the main compose file, so stop the regular stack
  first; everything else (project name, volumes, containers) is namespaced
  `nightlio-demo` and never collides.

Manual smoke checklist after `up`:

1. `docker compose -f docker-compose.demo.yml ps` — `seed` exited `0`,
   `api` and `frontend` healthy.
2. http://localhost:5173 logs straight in (single-user mode) and the
   dashboard shows recent entries.
3. History shows 14 entries, Goals shows 6 goals with progress, Statistics
   renders charts, Achievements shows unlocked badges.
4. `down -v`, `up` again — the stack re-seeds from scratch; `down -v` plus
   `up` twice in a row proves the seed is idempotent.

## Troubleshooting

- **"cannot reach PostgreSQL via DATABASE_URL"** — the server is down,
  unreachable, or the URL's host/port/credentials/sslmode are wrong. With
  compose: is the `postgres` profile up and healthy?
- **"Refusing to start: DATABASE_URL has unsupported scheme"** — only
  `postgres://` and `postgresql://` select Postgres; unset the variable to
  get SQLite.
- **Checksum / downgrade / gap errors from the migration runner** — see
  [docs/MIGRATIONS.md](MIGRATIONS.md); these protect your data and are never
  safe to force past.
- **"Refusing to import"** from `migrate-to-postgres` — the target database
  already contains users. The import never merges: point it at a fresh,
  empty database (or drop and recreate it) and re-run.
