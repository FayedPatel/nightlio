# Journal data export format (v1)

Nightlio can export everything you have written — journal entries (with their
tag selections) and goals (with their completion history) — as a single plain
JSON file, and import such a file back into any Nightlio instance. This
document is the normative field-by-field specification of that file.

Where it surfaces:

* **In the app**: Settings → **Data** → Export downloads
  `nightlio-export-YYYY-MM-DD.json`; Import reads one back in.
* **On the wire**: `GET /api/export/data` returns the envelope below as the
  JSON response body (the frontend only saves it to a file);
  `POST /api/import/data` accepts the same envelope as plain
  `application/json` (no multipart). Both routes require authentication.
  The machine-readable schema lives in [`contract/openapi.yaml`](../contract/openapi.yaml)
  (schemas `DataExport` / `DataImportResult`), with golden request/response
  fixtures in [`contract/fixtures/data/`](../contract/fixtures/data/).

The file is deliberately human-readable and hand-editable: pretty-printed
JSON, no ids, no binary blobs. You can write one from scratch (see the
[minimal example](#a-minimal-hand-authored-file)) to bulk-load a journal from
another app.

## Versioning policy

The envelope carries an integer `schema_version`, read by the importer
**before anything else**:

* The current — and so far only — version is **1**.
* An importer accepts every schema version **up to and including** the one it
  was built with. A version bump is reserved for *breaking* changes to the
  envelope shape; **v1 files will import forever**, on every future Nightlio.
* A file with a schema version **newer** than the server understands is
  rejected with `400` — for a current server, anything other than `1`
  (e.g. `Unsupported schema_version: 2 (supported: 1)`). A missing or
  non-integer `schema_version` is also a `400`.
* **Additive evolution does not bump the version.** Unknown JSON keys are
  ignored everywhere in the file, so a future release can add fields to v1
  and older files (and files stripped of those fields) keep importing
  unchanged.

## The envelope

```json
{
  "schema_version": 1,
  "exported_at": "2025-08-04 09:00:00",
  "app_version": "0.6.0",
  "data": {
    "entries": [ ... ],
    "goals": [ ... ]
  }
}
```

| Field | Type | Import | Notes |
| --- | --- | --- | --- |
| `schema_version` | integer | **required** | Must be `1` (see [Versioning policy](#versioning-policy)). |
| `exported_at` | string | ignored | UTC `YYYY-MM-DD HH:MM:SS` at export time. Informational only; may be omitted in hand-authored files. |
| `app_version` | string | ignored | Semver of the server that produced the file. Informational only — compatibility is governed by `schema_version` alone. |
| `data` | object | **required** | Must contain `entries` and `goals`, both arrays. |
| `data.entries` | array | **required** | Exported ascending by `date` (insertion order within a day). May be empty. |
| `data.goals` | array | **required** | Exported in creation order. May be empty. |

An export always contains all four top-level keys and both arrays — an empty
account still gets the full envelope with `"entries": []` and `"goals": []`
(re-importing that file is a valid no-op).

There are **no ids and no `user_id` anywhere** in the file. Database ids are
not portable across instances (a fresh database pre-seeds 3 default tag
groups with 27 options, so numbering diverges immediately); everything that
needs to reference something else does so by *name*.

## Entries

```json
{
  "date": "2025-08-01",
  "mood": 4,
  "content": "Great workout day.",
  "created_at": "2025-08-01 21:15:00",
  "updated_at": "2025-08-01 21:15:00",
  "selections": [
    { "group_name": "Emotions", "option_name": "content" },
    { "group_name": "Emotions", "option_name": "happy" }
  ]
}
```

| Field | Type | Import | Notes |
| --- | --- | --- | --- |
| `date` | string | **required**, non-empty | `YYYY-MM-DD`. Half of the duplicate key (see [Merge rules](#merge-rules-duplicate-detection)). |
| `mood` | integer 1–5 | **required** | The importer only accepts integers. (Exports from very old databases can contain a legacy non-integer mood such as `4.5` — see [Limitations](#limitations--what-is-not-in-the-file).) |
| `content` | string | **required**, non-empty | The Markdown body; the other half of the duplicate key. |
| `created_at` | string | optional | `YYYY-MM-DD HH:MM:SS`. Preserved verbatim on import; if absent or `null`, the import time is used. |
| `updated_at` | string | optional | Same format and same default as `created_at`. |
| `selections` | array | optional (default `[]`) | The entry's tags, denormalized by exact name. Exported ordered by group name, then option name. |

Each selection:

| Field | Type | Import | Notes |
| --- | --- | --- | --- |
| `group_name` | string | **required**, non-empty | Resolved against the importing user's tag groups by exact name, or created. |
| `option_name` | string | **required**, non-empty | Resolved within that group by exact name, or created. |

## Goals

```json
{
  "title": "Meditate",
  "description": "10 minutes",
  "frequency_per_week": 3,
  "completed": 2,
  "streak": 1,
  "period_start": "2025-07-28",
  "last_completed_date": "2025-08-01",
  "created_at": "2025-07-01 08:00:00",
  "updated_at": "2025-08-01 08:00:00",
  "completions": ["2025-07-31", "2025-08-01"]
}
```

| Field | Type | Import | Notes |
| --- | --- | --- | --- |
| `title` | string | **required**, non-empty | The duplicate key: a goal whose title already exists is skipped whole. |
| `description` | string or `null` | optional | Exports carry a (possibly empty) string; legacy rows can surface `null`. |
| `frequency_per_week` | integer 1–7 | **required** | Target days per week. |
| `completed` | integer | optional (default `0`) | Completions in the goal's current week, with the weekly rollover *projected* at export time (exactly what `GET /api/goals` would report). Written raw on import — never recomputed; the rollover projection self-heals stale weeks on the next goals read. |
| `streak` | integer | optional (default `0`) | Consecutive fully-completed weeks; same raw-in, raw-out treatment. |
| `period_start` | string | optional | ISO date of the Monday of the goal's current week as projected at export. If omitted, the goal simply starts a fresh week on its next read. |
| `last_completed_date` | string or `null` | optional | ISO date of the latest completion; `null` until the first log. |
| `created_at` | string | optional | As for entries: preserved verbatim, import time if absent. |
| `updated_at` | string | optional | Same. |
| `completions` | array of strings | optional (default `[]`) | Bare ISO `YYYY-MM-DD` dates (note: *not* the `{date}` objects of `GET /api/goals/{id}/completions`), exported ascending. Inserted with at most one completion per (goal, day); ignored entirely when the goal is skipped as a duplicate. |

## Import semantics

`POST /api/import/data` with the envelope as the JSON body. Bearer-token
callers just send it; cookie-authenticated callers must also satisfy the
app-wide CSRF predicate (`Content-Type: application/json` plus
`X-Requested-With: nightlio`), like every other mutation.

**Import is a merge, and it never modifies existing rows.** It only ever
*adds* entries and goals you don't already have — restoring a backup into a
non-empty instance is safe, and importing the same file twice is a no-op.

### Validation and atomicity

The whole file is validated before anything is written. Any invalid row
rejects the **entire file** with a row-indexed `400` — for example
`entries[1]: mood must be between 1 and 5` (zero-based index into the
failing array) — and **nothing is imported**. The write itself happens
inside a single transaction, so a partially-imported file cannot exist.
Fixing the reported row and re-POSTing the whole file is free: everything
that would have imported the first time still imports, and nothing
double-imports (see below).

A body that is not JSON at all (or is sent without a JSON `Content-Type`)
is a `400` with `{"error": "Bad request"}`.

### Merge rules (duplicate detection)

Rows are processed in file order, entries first, then goals. Duplicate
matching is an exact, case-sensitive string comparison, scoped to the
importing user:

* **Entries** dedupe on the pair **(`date`, `content`)**. A match is
  counted as skipped; otherwise the entry is inserted with its selections.
* **Goals** dedupe on **`title`**. A duplicate goal is skipped **whole** —
  its `completions` are ignored and the existing goal's streak state is
  never touched. Otherwise the goal is inserted and its completion rows
  with it.
* The duplicate sets are updated as rows import, so duplicates *within* the
  file skip too.

Selections resolve-or-create tag groups and options by exact name: an
existing `Emotions` group is reused (matching the target instance's option
if one with that name exists, oldest first), a group or option the target
has never seen is created on the fly.

### What import does *not* do

Import is a pure restore ([`contract/DECISIONS.md`](../contract/DECISIONS.md),
2026-08-24): it performs **no achievement checks** and writes **no activity
feed events**. Achievements are not retroactively granted by the import
itself; the imported history counts normally the next time an achievement
check runs (e.g. when you create your next entry, or via
`POST /api/achievements/check`).

### Size cap

The request body is capped at **8 MiB** (8,388,608 UTF-8 bytes) → `413`.
For scale: 8 MiB of pretty-printed JSON is on the order of a decade of
daily journaling, so an export bouncing off this limit almost certainly
indicates something malformed rather than a real journal.

### The response

```json
{
  "status": "success",
  "entries": { "imported": 1, "skipped": 1 },
  "goals": { "imported": 1, "skipped": 1 }
}
```

Exactly these three keys, integer counts only — the response never echoes
rows or ids. `imported + skipped` equals the number of rows in the file for
that category (a skipped goal counts once, whole). Re-importing the same
file yields `imported: 0` everywhere.

## A full annotated example

The file below round-trips: it is byte-shaped like a real export, and
POSTing it to a fresh account imports two entries and one goal.

```json
{
  "schema_version": 1,
  "exported_at": "2025-08-04 09:00:00",
  "app_version": "0.6.0",
  "data": {
    "entries": [
      {
        "date": "2025-08-01",
        "mood": 4,
        "content": "Great workout day.",
        "created_at": "2025-08-01 21:15:00",
        "updated_at": "2025-08-01 21:15:00",
        "selections": [
          { "group_name": "Emotions", "option_name": "content" },
          { "group_name": "Emotions", "option_name": "happy" }
        ]
      },
      {
        "date": "2025-08-03",
        "mood": 5,
        "content": "Beach day.",
        "created_at": "2025-08-03 19:30:00",
        "updated_at": "2025-08-03 19:30:00",
        "selections": [
          { "group_name": "Custom Tags", "option_name": "beach" }
        ]
      }
    ],
    "goals": [
      {
        "title": "Meditate",
        "description": "10 minutes",
        "frequency_per_week": 3,
        "completed": 2,
        "streak": 1,
        "period_start": "2025-07-28",
        "last_completed_date": "2025-08-01",
        "created_at": "2025-07-01 08:00:00",
        "updated_at": "2025-08-01 08:00:00",
        "completions": ["2025-07-31", "2025-08-01"]
      }
    ]
  }
}
```

Walking through it:

* `exported_at` / `app_version` describe where the file came from; the
  importer reads neither.
* The first entry's two selections both point at the default **Emotions**
  group — on import they resolve to the target's existing options by name.
  The second entry's `Custom Tags` / `beach` selection creates both the
  group and the option if the target has never seen them.
* The goal's `completed: 2` / `streak: 1` / `period_start: "2025-07-28"`
  are the weekly state *as projected at export time*. Imported long after
  that week ended, they are stored as-is and the next goals read rolls the
  week forward exactly as if the goal had sat untouched in the database.
* The two `completions` become completion rows for the new goal; if
  `"Meditate"` already existed on the target, the goal (completions and
  all) would be skipped and counted in `goals.skipped`.

## A minimal hand-authored file

Everything optional omitted — this is a valid import (for bulk-loading a
journal from another app, for instance):

```json
{
  "schema_version": 1,
  "data": {
    "entries": [
      { "date": "2025-08-01", "mood": 3, "content": "hi" }
    ],
    "goals": [
      { "title": "Walk", "frequency_per_week": 3 }
    ]
  }
}
```

Missing timestamps become the import time, `selections`/`completions`
default to none, and the goal starts with a clean weekly state (`completed`
0, `streak` 0, a fresh week on first read).

## Limitations — what is *not* in the file

* **Tag groups and options travel only inside entry selections.** A custom
  group (or option) that no exported entry references does not appear in
  the file and will not be recreated on import. (A likely v2 addition — see
  below.)
* The export is **per-user**: it contains the authenticated user's journal
  and goals, nothing about other accounts on the instance.
* Account data (username/password, OIDC linkage), the theme preference,
  earned **achievements**, the **activity feed**, and statistics-view
  counters are not exported. Statistics are always derived from entries, so
  they need no exporting.
* The database schema tolerates non-integer moods (the API itself never
  writes them, but a historical or hand-edited row can hold e.g. `4.5`);
  such a row exports verbatim, but the v1 *importer* only accepts integers
  1–5, so it must be rounded by hand before the file will import.

A **v2** would only exist for a breaking shape change; the current
candidates (exporting unreferenced groups/options, for instance) are
additive and would ship inside v1 with unknown-key tolerance doing the
compatibility work. Whenever a v2 does happen, servers of that era will
accept both v1 and v2 files, per the versioning policy above.
