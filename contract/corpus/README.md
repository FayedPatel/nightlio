# Migration-test DB corpus

Snapshot SQLite databases for grading the Rust rewrite's schema bootstrap
against the Python one (`init_database` in `api/database_schema.py`, run via
the `MoodDatabase` constructor). The `.db` files are inputs and expected
outputs; the `.pragmas.txt` / `.counts.txt` files are the normalized
comparison targets so schema parity is asserted mechanically, not eyeballed.

Regenerate everything with:

```sh
api/venv/bin/python contract/corpus/build_corpus.py
```

(The script removes and rebuilds the generated `.db`/`.pragmas.txt`/`.counts.txt`
files, leaving this README and itself in place. Row timestamps we insert are
pinned to `2026-08-15 00:00:00`; rows created by the bootstrap itself — the
seeded self-host user and the default groups/options — carry
`CURRENT_TIMESTAMP`, so the `.db` files are not byte-identical across
regenerations. The pragma and count dumps are.)

## Files

| File | What it is |
|------|------------|
| `fresh.db` | `init_database` run on an empty path. Current schema, seeded self-host user (`users.id = 1`, `google_id = 'selfhost_default_user'`), 3 default groups with 27 options. |
| `legacy-groups.db` | **Input, pre-migration.** Pre-Phase-2a schema built with raw SQL (mirrors `_build_old_populated_db` in `api/tests/test_schema_migrations.py`): `users` keyed only by `google_id` (no `auth_provider`/`external_id`/`password_hash`/`theme_preference`), `groups` with a global inline `UNIQUE(name)` and **no `user_id` column**, `goals` without `last_completed_date`, no `activity_log` table. Populated: 2 users, 4 groups (the 3 defaults plus "Workout"), 5 options, 3 mood entries, 4 selections, 1 goal, 1 completion, 1 achievement, 1 user_metrics row. Exercises the groups-rebuild (create-new/copy/drop/rename) and ownerless-backfill path. |
| `legacy-groups.migrated.db` | **Expected output.** A copy of `legacy-groups.db` after running the Python bootstrap once. |
| `half-migrated.db` | **Input, partially migrated.** Current schema with two pieces missing: `users.theme_preference` dropped and the `idx_users_provider_external` unique index dropped. User 2 additionally has `auth_provider`/`external_id` NULL, so the identity backfill must run. Populated with mood entries, selections, a goal, a completion, an achievement, metrics, and 3 activity_log rows. Simulates a production DB migrated by an older build. |
| `half-migrated.migrated.db` | **Expected output.** A copy of `half-migrated.db` after running the Python bootstrap once. |
| `<name>.pragmas.txt` | For every non-`sqlite_*` table (sorted by name): `PRAGMA table_info` rows, `PRAGMA index_list` rows, and the column list from `PRAGMA index_info` for each listed index. Raw tuples, one per line. |
| `<name>.counts.txt` | Tab-separated `table<TAB>row_count` for every non-`sqlite_*` table, sorted by name. |
| `build_corpus.py` | The generator (run with `api/venv/bin/python` from anywhere; it locates the repo from its own path). |

## Assertion procedure for the Rust bootstrap

1. Copy `legacy-groups.db` and `half-migrated.db` to temp paths (never run
   against the originals — they are the pristine pre-migration inputs).
2. Run the Rust bootstrap on each copy, and on an empty path for the fresh
   case.
3. Dump each result with the same logic as `dump_pragmas` / `dump_counts` in
   `build_corpus.py` and diff against the corresponding checked-in
   `*.migrated.pragmas.txt` / `*.migrated.counts.txt` (or `fresh.*` for the
   empty-path case). Any diff is a parity failure.
4. Additionally assert data-level facts on the migrated legacy copy:
   - `SELECT DISTINCT user_id FROM groups` returns only `1` (ownerless
     backfill to the seeded self-host user).
   - `users` rows: `google_id = 'selfhost_default_user'` has
     `auth_provider = 'local'`; the Google row has
     `auth_provider = 'legacy-google'`; both have `external_id = google_id`.
   - Row counts of users, groups, group_options, mood_entries,
     entry_selections, goals, goal_completions, achievements are unchanged
     from the input (`legacy-groups.counts.txt`).
5. Run the bootstrap a second time on the already-migrated copy and re-dump:
   the second run must be a no-op (dumps identical to the first run).

## Facts the dumps encode (do not "fix" these in Rust)

- **`goals` column order depends on migration history.** A fresh DB declares
  `last_completed_date` inline at cid 8; a DB migrated from the legacy shape
  gets it appended by `ALTER TABLE ADD COLUMN` at cid 10 (after
  `created_at`/`updated_at`). This is the only table_info difference between
  `fresh.pragmas.txt` and `legacy-groups.migrated.pragmas.txt`. The `users`
  table does *not* have this problem — its CREATE TABLE deliberately lists
  the new columns last so fresh and migrated orders match.
- `half-migrated.migrated.pragmas.txt` is byte-identical to
  `fresh.pragmas.txt`: re-adding `theme_preference` appends it, which is
  where a fresh DB puts it anyway, and the unique index is recreated.
- The groups rebuild names the replacement table `groups_migration_new` and
  renames it, so the migrated `groups` table has **no**
  `sqlite_autoindex_groups_1` — its only index is the explicitly created
  unique `idx_groups_user_name` (origin `'c'`, not `'u'`). A fresh DB looks
  the same because the current DDL has no inline UNIQUE on groups.
- Migrating `legacy-groups.db` creates `activity_log` (empty) and
  `user_metrics` indexes but seeds **no** new groups or options: the three
  default group names already exist, and `ensure_default_groups_for_user`
  only inserts options when it inserts the group row itself.
- `nft_minted` keeps its textual default `'FALSE'` (SQLite stores the DDL
  token verbatim; `PRAGMA table_info` reports `dflt_value = 'FALSE'`).
