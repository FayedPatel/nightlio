# Nightlio API contract oracle

This directory is the single source of truth for what "parity" means for the
TypeScript/Rust rewrite. The original Flask API (since removed from the repo
— see the "Legacy removal" entry in `DECISIONS.md`) had no OpenAPI document,
no JSON Schema, and no typed client — its wire contract existed only as
`jsonify({...})` literals and the method names in the old `src/services/api.js`.
These artifacts pin it down, and with the Flask code gone they are the sole
remaining oracle (frozen goldens — nothing can be re-recorded):

| Artifact | What it is |
| --- | --- |
| `contract/openapi.yaml` | The merged, hand-written OpenAPI 3.1 document — all 41 paths / 54 operations (the rewrite added `POST /api/statistics/view`; v0.6.0 added the `/api/i18n` family and the `/api/export/data` + `/api/import/data` data family), assembled from the per-family fragments and cross-checked against the fixtures. **This is the graded document.** |
| `contract/openapi-parts/*.yaml` | The per-family source fragments (auth, mood+statistics, goals, groups, achievements, misc, i18n, data). Kept for provenance; the merged file supersedes them on any conflict. |
| `contract/fixtures/**` | ~180 golden request/response recordings taken against live Flask instances on throwaway SQLite databases, plus fixtures added or edited by owner-approved contract-change rounds (each such edit carries a "contract change" note and a `DECISIONS.md` entry). Organized by family (`auth/`, `mood/`, `goals/`, `groups/`, `achievements/`, `misc/`, `i18n/`, `data/`). The 8 fixtures in `i18n/` and the 15 in `data/` are hand-authored spec, not recordings — those families never existed in Flask (see the 2026-08-22 and 2026-08-24 `DECISIONS.md` entries) — but they are frozen goldens all the same. |
| `contract/corpus/` | Snapshot SQLite databases plus `PRAGMA table_info` / `PRAGMA index_list` records for migration parity testing. |
| `src/types/api.ts` | The TypeScript contract module derived from `openapi.yaml`, tailored to the ~35 methods in `src/services/api.ts` and `src/services/statsApi.ts`. Type-only, strict-mode clean. |

## Fixture format and how to replay

Each fixture is one JSON file:

```json
{
  "method": "POST",
  "path": "/api/mood",
  "request": { "headers_that_matter": { ... }, "body": { ... } },
  "response": {
    "status": 201,
    "content_type": "application/json",
    "headers_that_matter": { ... },
    "body": { ... }
  },
  "notes": ["..."]
}
```

To replay against a candidate server (the Rust API, e.g. as a regression
check):

1. **Boot a throwaway instance** the way the recorders did (see
   `scripts/e2e-api.sh`): fresh `DATABASE_PATH` and `RATE_LIMIT_DB_PATH`
   under a scratch directory, OIDC env vars blanked, `ENABLE_MOOD_MUSIC`
   unset. `/api/config` must report `enable_oidc: false` before you start.
2. **Authenticate** with the credential-free `POST /api/auth/local/login`
   (empty body) and use the returned Bearer token, unless the fixture's
   recorded request headers say otherwise (cookie/CSRF fixtures carry their
   own headers).
3. **Seed in fixture order.** Some fixtures depend on earlier writes in the
   same family (ids, streaks, achievement state). Replay a family's fixtures
   in the order its notes describe; do not interleave families. Historical
   note: the recordings were taken when two ordering hazards existed —
   `GET /api/statistics` was a **write** (it incremented `stats_views`,
   changing a later `new_achievements`) and goal reads persisted weekly
   rollover. Both are gone since the contract change (statistics GET
   and goal GETs are pure reads; `stats_views` moves only via
   `POST /api/statistics/view`), so GETs no longer perturb replay state —
   but the recorded bodies still reflect the old ordering where their notes
   say so.
4. **Normalize volatile fields before comparing.** The recordings already
   substitute placeholders; apply the same substitutions to the candidate's
   output:
   - JWTs -> `<JWT>`; HTTP dates in Set-Cookie -> `<TS>`
   - `created_at` / `updated_at` / `earned_at` -> `<TS>`
   - the `version` field in `/api/config` -> `<VERSION>` (substituted with the
     live `CARGO_PKG_VERSION` by `config_matches_fixture` in
     `api/tests/shell_router.rs` — future version bumps never touch the
     fixture; see the 2026-08-22 `DECISIONS.md` entry)
   - streak seed dates -> `<TODAY>` / `<YESTERDAY>`
   - current-month digest fields -> `<CURRENT_YEAR>` / `<CURRENT_MONTH>`
   - bare `YYYY-MM-DD` values in goals fixtures -> `<DATE>`
   - **Autoincrement ids are kept verbatim in fixtures but must NOT be graded
     exactly.** The self-host login upsert burns rowids on every conflict,
     and a fresh DB is pre-seeded (3 default groups / 27 options; first
     user-created group id is 4, first option id is 28). Compare id
     *presence and type*, not value — or replay onto an identically seeded DB
     and accept equal-by-construction values.

## The grading rule: compare shapes and status codes, never error text

Error bodies are always `{"error": "<string>"}` (including framework routing
404s, which app-level handlers rewrite to JSON). The *message text* is
explicitly **not** part of the contract: much of it is Python exception
`__str__` output (`"Python int too large to convert to SQLite INTEGER"`) that
no Rust implementation should reproduce. Grade:

- HTTP status code — exactly.
- Body **shape** — key set, nesting, JSON types (including int-vs-float and
  null), bare-array vs object envelope.
- Headers listed in `headers_that_matter` — Set-Cookie attribute-for-attribute
  (with `<JWT>`/`<TS>` normalization), `Content-Type`, `Content-Disposition`,
  `Allow`, `Location`, and the i18n caching pair `ETag` / `Cache-Control`.

**Exactly two error strings are contract-fixed** (the frontend does not parse
any others, but these gate CSRF debugging): the cookie-auth 403 bodies
`"Content-Type must be application/json"` and
`"Missing required request header"`.

Known deliberate shape quirks the port must reproduce (all fixture-encoded):
string-keyed sparse `mood_distribution`; `average_mood` integer `0` on empty
accounts; six bare-array endpoints; DELETEs returning 200+JSON, never 204;
the unified metadata-object `new_achievements` shape on `POST /api/mood` and
`POST /api/achievements/check` (contract change — formerly three distinct shapes; only
`GET /api/achievements` returns full rows); the PUT mood response embedding
`selections` while GET does not; `nft_minted` as integer 0/1; `next_cursor`
null when no older rows exist (peek-ahead contract change — formerly non-null on a full
final page); lenient (`get_json(silent=True)`) body parsing on login, goal
progress, and achievements check.

## Contract ambiguities — recorded at fixture time, all since resolved

Everything below was honestly recorded in fixtures/spec as an open question
the fixtures could not settle. Every item now has an owner decision — the
ledger in `DECISIONS.md` is fully closed. The original wording is kept as
history; each item carries its resolution.

1. **Overflow path ids -> 500.** Werkzeug's `<int:>` converter is unbounded;
   ids beyond SQLite's i64 range reach the handler and 500 with a leaked
   Python message (fixtures `routing_mood_overflow_id.json`,
   `routing__delete-group-overflow-id.json`). A Rust i64 path extractor
   would naturally 404.
   — **Resolved:** clean 404 (accepted drift, DECISIONS.md #1).
2. **OPTIONS parity across the two regimes.** Explicitly registered OPTIONS
   rules (goal-completions aliases, achievements progress) returned **204**
   empty; every other route got Flask automatic OPTIONS: **200** empty with
   an `Allow` header.
   — **Resolved (contract change):** unified — every OPTIONS answers 204 empty with
   `Allow` (DECISIONS.md, contract changes).
3. **405 and the `/api` 308 are text/html.** No JSON handler existed for 405
   (Werkzeug default page + `Allow`), and bare `GET /api` 308-redirected
   with an HTML body.
   — **Resolved 2026-08-16:** 405 uses the JSON envelope + `Allow`; 308 is
   status + `Location` with an empty body (DECISIONS.md, "405/308 bodies
   unified").
4. **Mixed date-format semantics in statistics and range filtering.**
   `first/last_entry_date` were lexicographic MIN/MAX over raw stored
   strings, and `GET /api/moods?start_date&end_date` did a raw-string
   BETWEEN — US `M/D/YYYY` entries sorted wrong and fell out of ISO ranges.
   — **Resolved (contract change):** stored dates ISO-normalized by migration + on
   write (DECISIONS.md #4).
5. **200 `[]` instead of 404 on ownership misses.**
   `GET /api/mood/{id}/selections` and `GET /api/goals/{id}/completions`
   returned 200 `[]` for nonexistent/foreign ids.
   — **Resolved (contract change):** both 404 now, matching their sibling GETs
   (DECISIONS.md #5/#6).
6. **`POST /api/groups/{id}/options` unknown/foreign group -> 400, not 404**
   (route mapped every ValueError to 400).
   — **Resolved (contract change):** 404 `{"error": "Group not found"}`
   (DECISIONS.md #5/#6).
7. **Goal update asymmetries** — resolved (DECISIONS.md #7): empty
   update body -> 400 "No fields to update", blank title -> 400 like
   create; 404 only for a missing/foreign goal.
8. **Three `new_achievements` shapes** (bare strings from `POST /api/mood`,
   metadata objects from `POST /api/achievements/check`, full rows from
   `GET /api/achievements`).
   — **Resolved (contract change):** unified — `POST /api/mood` returns the same
   metadata-object `NewAchievement` shape as `/achievements/check`; only
   `GET /api/achievements` returns full rows (DECISIONS.md contract
   changes).
9. **Achievement icon swap.** Backend metadata says
   `consistency_king=Target`, `mood_master=Crown`; the frontend's hardcoded
   map said the opposite, so icons visibly changed on unlock.
   — **Resolved:** backend metadata is truth; the frontend was aligned to
   it (DECISIONS.md, "Other fixed findings").
10. **`GET /api/statistics` write side effect** (`stats_views` increment
    feeding the `data_lover` unlock in a *later* check), which made fixture
    replay order-sensitive.
    — **Resolved (contract change):** the GET is a pure read; `data_lover` is fed by
    `POST /api/statistics/view` (per-day idempotent, `{"counted": bool}`)
    (DECISIONS.md, contract changes).
11. **`next_cursor` non-null on a full final page** (follow-up page is
    empty).
    — **Resolved (contract change):** peek-ahead pagination — null when no older rows
    exist (DECISIONS.md #10).
12. **429 rate-limit body shape was unverified** — limits (login 30/min,
    register 10/min) were code-derived; no 429 fixture was recorded.
    — **Resolved:** body string ported verbatim, pinned by tests
    (DECISIONS.md #12).
13. **OIDC routes were unrecorded.** `/api/auth/login/oidc` and
    `/api/auth/callback/oidc` were never fixture-recorded (OIDC blanked).
    — **Resolved:** wiremock-tested + live pocket-id verification
    (DECISIONS.md #13).
14. **Music enabled-mode was unrecorded.** `GET /api/music/vibe` with
    `ENABLE_MOOD_MUSIC` on was documented from code only.
    — **Resolved:** implemented and tested via the `JAMENDO_API_BASE`
    fake-upstream hook (DECISIONS.md #14).
15. **`POST /api/export/pdf` 501 branch was unrecorded** (renderer was
    installed in the recording env).
    — **Resolved:** renderer compiled in-process; the 501 branch is retired
    (DECISIONS.md #15).

Conditional (config-gated) paths are marked in `openapi.yaml` with the
`x-nightlio-conditional` vendor extension; registered alias rules carry
`x-nightlio-alias-of`; code-derived, fixture-less behavior carries
`x-nightlio-unrecorded`.
