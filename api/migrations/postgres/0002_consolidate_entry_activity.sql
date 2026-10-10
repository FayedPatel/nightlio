-- 0002_consolidate_entry_activity (Postgres) — data only, no DDL.
--
-- Before v0.7.0 every entry autosave logged its own entry_created /
-- entry_edited row. This folds those historical rows into one row per
-- editing session, the shape v0.7.0 writes going forward:
--
-- - only entry_created / entry_edited rows with valid JSON metadata and a
--   non-null entry_id take part; everything else is untouched;
-- - rows are grouped per (user_id, entry_id) in id order; a row opens a new
--   session when it is the first of its group, is an entry_created, has (or
--   follows) an unparsable created_at, or comes more than 30 minutes after
--   the previous row;
-- - in each session of more than one row, the lowest id is kept and gets
--   metadata.last_saved_at = the session's latest created_at; the other rows
--   are deleted. Single-row sessions stay byte-identical.
--
-- Safe to run again on its own output: a kept row keeps its session's first
-- created_at, so every boundary that split two sessions still splits them.
--
-- metadata and created_at are text columns, so every cast is guarded by
-- pg_input_is_valid. One statement: the UPDATE (keepers) and the DELETE
-- (the rest) touch disjoint rows from the same snapshot. Runs inside the
-- runner's transaction (no BEGIN/COMMIT here). This file is frozen once
-- shipped: its checksum is recorded in schema_migrations.

WITH entry_events AS (
    SELECT id, user_id, event_type, created_at,
           CASE WHEN pg_input_is_valid(metadata, 'jsonb') THEN metadata::jsonb ->> 'entry_id' END AS entry_id,
           -- 'infinity' passes pg_input_is_valid but cannot be subtracted.
           CASE WHEN pg_input_is_valid(created_at, 'timestamp')
                THEN CASE WHEN isfinite(created_at::timestamp) THEN created_at::timestamp END
           END AS ts
      FROM activity_log
     WHERE event_type IN ('entry_created', 'entry_edited')
),
flagged AS (
    SELECT id, user_id, entry_id, created_at,
           CASE
               WHEN LAG(id) OVER w IS NULL THEN 1
               WHEN event_type = 'entry_created' THEN 1
               WHEN ts IS NULL OR LAG(ts) OVER w IS NULL THEN 1
               WHEN ts - LAG(ts) OVER w > interval '30 minutes' THEN 1
               ELSE 0
           END AS starts_session
      FROM entry_events
     WHERE entry_id IS NOT NULL
    WINDOW w AS (PARTITION BY user_id, entry_id ORDER BY id)
),
numbered AS (
    SELECT id, user_id, entry_id, created_at,
           SUM(starts_session) OVER (PARTITION BY user_id, entry_id ORDER BY id) AS session_no
      FROM flagged
),
sessions AS (
    SELECT id,
           MIN(id) OVER s AS keep_id,
           MAX(created_at) OVER s AS last_saved_at,
           COUNT(*) OVER s AS session_size
      FROM numbered
    WINDOW s AS (PARTITION BY user_id, entry_id, session_no)
),
kept AS (
    UPDATE activity_log a
       SET metadata = jsonb_set(a.metadata::jsonb, '{last_saved_at}', to_jsonb(x.last_saved_at))::text
      FROM sessions x
     WHERE a.id = x.id AND x.id = x.keep_id AND x.session_size > 1
)
DELETE FROM activity_log a
 USING sessions x
 WHERE a.id = x.id AND x.id <> x.keep_id;
