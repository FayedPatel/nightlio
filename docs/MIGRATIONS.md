# Database migrations

How Nightlio's schema evolves from v0.6.0 onward, and the rules every future
schema change must follow. The runner lives in `api/src/db/migrate.rs`; the
migration files live in `api/migrations/`.

## How it works

Nightlio ships a small, hand-rolled migration runner with refinery-style
semantics. (refinery itself was rejected: its rusqlite feature pins
libsqlite3-sys — a native `links` dependency of which only one version can
exist in a build — and that would dictate our deliberately pinned
rusqlite 0.40.2 with the bundled SQLite.)

- Every migration is a **pair** of dialect files, embedded into the binary at
  compile time:

  ```
  api/migrations/sqlite/NNNN_name.sql
  api/migrations/postgres/NNNN_name.sql
  ```

  Both files always exist for a version. A dialect that needs no work ships a
  comment-only file (see `0001_baseline.sql` on the SQLite side). A unit test
  fails the build if the two directories don't list identical
  `(version, name)` pairs or drift from the embedded manifest.

- Applied migrations are recorded in a `schema_migrations(version, name,
  checksum, applied_at)` table on both backends. The checksum is the SHA-256
  of the applied dialect file.

- Each migration applies inside its own transaction — `BEGIN IMMEDIATE` on
  SQLite; a regular transaction under
  `pg_advisory_lock(hashtext('nightlio_migrations'))` on Postgres (DDL is
  transactional there, and the advisory lock serializes concurrent
  replicas). Migration bodies must therefore never contain their own
  `BEGIN`/`COMMIT`.

- The runner **refuses to start** — with an actionable error message — when
  it finds:
  - a **checksum mismatch**: a recorded migration differs from this build's
    copy. Shipped migration files are immutable; fixes go in a *new*
    migration.
  - a **downgrade**: the database records a version newer than this build
    ships. Run the newer build, or restore a pre-upgrade backup.
  - a **numbering gap**: either in the embedded manifest (a packaging bug)
    or in the recorded ledger (a partial restore or manual edit).

## The SQLite baseline: adoption, never destruction

SQLite databases predate this framework, so the legacy `bootstrap()`
(`api/src/db/bootstrap.rs`) still runs first at every startup, completely
untouched — it *is* the idempotent v0 → v3 upgrade path for every database
file in the wild, and its output is graded byte-for-byte against
`contract/corpus/`. On top of that:

- A database at `PRAGMA user_version >= 3` without a `schema_migrations`
  table gets one created, with `0001_baseline` recorded as an **adopted**
  (never executed) baseline; migrations 0002+ then apply normally.
  `api/migrations/sqlite/0001_baseline.sql` is a comment-only no-op for
  exactly this reason — the baseline schema is owned by `bootstrap()`.
- A database still below `user_version 3` after bootstrap (a swallowed
  "non-critical" migration error — the historical lenient behavior) keeps
  serving **only while nothing beyond the baseline is pending**. As soon as a
  real 0002+ migration exists, such a database fails closed with a message
  pointing at the failed bootstrap step.
- `user_version` is **frozen at 3 forever**. All future schema changes go
  through this framework and its ledger, never through new pragma stamps.

On Postgres there is no pre-history: `0001_baseline.sql` creates the entire
schema (tables, indexes, and the `nightlio_now()` / `nightlio_safe_date()`
helper functions) on a fresh database.

## The expand–contract policy

Deployments upgrade by pulling an image; users roll back by pulling the
previous one. Migrations must keep both directions survivable:

1. **Additive-only DDL in the introducing release.** A release that
   introduces a schema change may only *add* things: new tables, new
   nullable-or-defaulted columns, new indexes. The previous release's code
   must still run correctly against the expanded schema.
2. **Destructive DDL at least one release later.** Dropping or renaming a
   table/column ships no earlier than one release *after* the last code that
   referenced it is gone. (A rename is a destructive drop plus an addition —
   ship the new name first, migrate the data, drop the old name a release
   later.)
3. **Backfills are idempotent.** Data-rewriting migrations must be safe to
   run again on their own output (guard with `WHERE ... IS NULL`, key resets
   to the structural change that introduces them, etc.), mirroring how the
   legacy bootstrap's v2/v3 steps behave.

## Adding a migration (checklist)

1. Pick the next version number `NNNN` (contiguous — the runner refuses
   gaps).
2. Create **both** `api/migrations/sqlite/NNNN_name.sql` and
   `api/migrations/postgres/NNNN_name.sql`, even if one side is a
   comment-only no-op.
3. Append the entry to `MIGRATIONS` in `api/src/db/migrate.rs`
   (`include_str!` both files).
4. Follow the expand–contract rules above; no `BEGIN`/`COMMIT` inside the
   body.
5. Never touch an already-shipped migration file — its checksum is recorded
   in every deployed database.
6. `cd api && cargo test migrate` — the manifest-parity and runner tests
   grade the rest.

## See also

- [docs/POSTGRES.md](POSTGRES.md) — the opt-in Postgres backend this
  framework co-serves.
- `contract/corpus/README.md` — why the SQLite baseline is frozen and graded
  byte-for-byte.
