# Contract & rewrite decisions

Ledger of every deliberate behavior decision made during and after the Flask→Rust / JS→TS rewrite. The golden fixtures in `contract/fixtures/` + `contract/openapi.yaml` are the sole contract oracle (Flask is removed); any behavior change must update fixtures, OpenAPI, `src/types/api.ts`, and this file together.

**Status: fully closed — no open items.** All contract changes below ship together in the (unreleased) rewrite branch; the ledger uses dates, not contract versions.

## Numbered-item index

Code, fixtures, and specs cite decisions as "DECISIONS.md #N" / "item N". Where each landed:

| # | Subject | Outcome |
|---|---------|---------|
| 1 | Overflow path ids (> i64) | Contract fact — clean 404, test-pinned (below) |
| 2 | 405 / 308 HTML bodies | Contract change 2026-08-16 — JSON envelope / empty body (below) |
| 3 | OPTIONS two-regime | Retired 2026-08-16 — all OPTIONS answer 204 |
| 4 | Mixed date-format semantics | Fixed 2026-08-15 — ISO normalization + v2 migration |
| 5, 6 | Status-code inconsistencies (selections/completions/options) | Fixed 2026-08-15 — consistent 404s |
| 7 | Goal update asymmetries | Fixed 2026-08-15 — 400 on empty body / blank title |
| 8 | Three `new_achievements` shapes | Unified 2026-08-16 — metadata objects |
| 9 | `GET /api/statistics` write side effect | Purified 2026-08-16 — `POST /api/statistics/view` |
| 10 | `next_cursor` on exactly-full page | Fixed 2026-08-15 — peek-ahead `null` |
| 11 | Goals weekly rollover writes during GET | Purified 2026-08-16 — projected on read, persisted on write |
| 12 | 429 rate-limit body | Shipped — verbatim Python `RATE_LIMIT_MESSAGE`, limits pinned by tests |
| 13 | OIDC redirect-flow validation | Shipped — wiremock suite (`api/tests/oidc_flow.rs`) + live Pocket ID passkey leg |
| 14 | `/api/music/vibe` enabled mode | Shipped — tested via `JAMENDO_API_BASE` fake upstream |
| 15 | PDF export | Shipped — in-process `markdown2pdf`; sidecar and `PDF_SERVICE_URL` removed |
| 16 | linux/arm64 image publishing | Shipped — QEMU multi-platform builds in publish.yml |

---

## Permanent contract facts

Flask is gone, so nothing "drifts" from it anymore — these are simply how the API behaves, promoted from the old "accepted drift" / "residual serialization drift" lists to first-class contract facts on 2026-08-16 and pinned by tests where a silent regression is possible. Historical Flask behavior is noted only so old recordings and fixtures aren't surprising.

### Path-id parsing

1. **Out-of-range path ids (> i64) return a clean 404**, exactly like negative and non-integer ids — the route simply never matches. (Flask 500'd here with leaked Python `OverflowError` text; the recorded fixture documents that history.) Pinned by the int-converter tests in `api/tests/shell_router.rs`.

### Contract encoding facts

How responses are encoded on the wire. No client parses these byte-level details (JSON is parsed, not byte-compared), but they are pinned so they cannot silently change:

- JSON bodies are compact serde_json output — no pretty-printing, no trailing newline. (Flask appended a newline and pretty-printed in debug.) Test: `json_bodies_are_compact_without_trailing_newline` in `api/tests/shell_router.rs`.
- Non-ASCII is emitted as raw UTF-8, never `\uXXXX` escapes (same decoded value; Flask escaped). Test: `non_ascii_content_is_raw_utf8_not_escaped` in `api/tests/mood_routes.rs`.
- Login Set-Cookie carries `Max-Age` and omits the redundant `Expires` attribute (Max-Age is authoritative per RFC 6265); attribute order is unspecified. Test: `assert_session_cookie` in `api/tests/auth_family.rs` grades the full attribute set including `Expires` absence.
- The JWT verifier accepts integer `exp`/`iat` only — float claims are rejected (stricter than python-jose; no live-token impact since only ints were ever issued). Test: `float_exp_iat_token_is_invalid_here_though_jose_accepts_it` in `api/src/auth/jwt.rs`.
- Requests without an `Origin` header get no `Access-Control-*` headers at all (they are not CORS requests; browsers always send Origin on CORS requests). Test: `originless_request_gets_no_cors_headers` in `api/tests/shell_router.rs`.
- Float JSON values are parsed correctly rounded and re-serialize byte-identically (serde_json's `float_roundtrip` feature; the default parser is 1-ulp imprecise on full-precision floats, which would silently perturb REAL mood values and epoch timestamps on the way in). Test: the pinned round-trip in `json_bodies_are_compact_without_trailing_newline`.
- JSON key order is unspecified and may differ from old Flask recordings on `/statistics/heatmap`, `/statistics/digest`, `/api/moods`. Untested on purpose — key order carries no meaning.
- 405 `Allow` joins methods comma-only (recorded as-is in `time_post_405.json` since 2026-08-16 — contract, not drift; method order may differ from Werkzeug's).

---

## contract changes (owner-approved, executed 2026-08-16)

The four remaining "permanent" Flask quirks (items 3/8/9/11) retired as deliberate contract changes; every touched fixture carries a "contract change" note, and OpenAPI (merged + parts), `src/types/api.ts`, and the frontend moved together:

- **#11 Goals weekly rollover moved out of GET.** `GET /api/goals`, `GET /api/goals/{id}`, and the completions existence probe are pure reads: the rollover (completed reset, streak recompute, `period_start` = current ISO Monday) is *projected* into the response and persisted only by the write paths (`POST /api/goals/{id}/progress`, `PUT`/`PATCH /api/goals/{id}`). Wire shapes are unchanged — only the DB write disappeared from reads (`api/src/db/goals.rs`: `project_rollover` / `persist_rollover_if_changed`). This also fixed a latent bug: `update_goal` previously never rolled over, so its `completed = MIN(completed, ?)` clamp operated on stale week state; it now persists the rollover first and clamps the rolled value (regression test `update_goal_rolls_over_stale_week_before_clamping`).
- **#9 Statistics GET purified; data_lover is per-day.** `GET /api/statistics` no longer increments anything. The new `POST /api/statistics/view` (200 `{"counted": bool}`, require_auth + CSRF like every mutation, strict-slash) records at most one view per user per server-local day (`user_metrics.last_view_date`); `data_lover` now means "view statistics on 10 different days". Existing `stats_views` counters were RESET to 0 by the migration (`PRAGMA user_version` 2→3, guarded `ALTER TABLE user_metrics ADD COLUMN last_view_date TEXT` + one-shot `UPDATE user_metrics SET stats_views = 0`); already-earned badges are kept (achievements rows untouched). Rationale: the old write-on-GET double-counted (frontend double-fetch: 2 increments/visit, 4 in dev StrictMode) and made fixture replay order-sensitive.
- **#8 `new_achievements` unified.** `POST /api/mood` returns metadata objects (`{achievement_type, name, description, icon, rarity}` — the same `NewAchievement` shape as `POST /api/achievements/check`, built by the shared `new_achievement_objects` helper) instead of bare `achievement_type` strings. `GET /api/achievements` is unchanged (full rows merged with metadata). Frontend only ever read `?.length` on the mood-response field, so no client broke.
- **#3 OPTIONS unified on 204.** Every route answers OPTIONS with 204, an empty body, and an `Allow` header (Content-Type `text/html; charset=utf-8` kept); the explicit 204 handlers (`completions_options`, `achievements_progress_options`) were deleted and the automatic handlers flipped 200→204. CORS preflights ride the same 204 via the non-short-circuiting after-request CORS layer. Wire 204s carry no `Content-Length` (hyper strips it). Fixtures re-recorded/renamed: `misc/preferences_options_204_plain.json`, `misc/preferences_options_204_cors_preflight.json`, `auth/verify_options-204.json`.
- **Corpus re-pins.** `contract/corpus/fresh.pragmas.txt`, `legacy-groups.migrated.pragmas.txt`, and `half-migrated.migrated.pragmas.txt` gained the `user_metrics.last_view_date` column row, and the expected `.migrated.db` / `fresh.db` goldens were regenerated to match the v3 schema (pristine inputs untouched; `.counts.txt` unchanged). Owner-approved schema migration per the standing corpus rule.

## Other owner decisions (executed 2026-08-16)

- **#2 405/308 bodies unified on the JSON envelope.** Every 405 returns `{"error": "Method not allowed"}` + `Allow` via a router-level `method_not_allowed_fallback`; the `/api/music/vibe` hand-dispatched Werkzeug-HTML 405 replica and the `GET /api` 308 "Redirecting..." HTML replica were deleted (308 keeps status + `Location`, empty body — the sole empty-body response). Rationale: the API was self-inconsistent — empty 405 bodies everywhere except one hand-written HTML route, while every other error uses the envelope; no client parses these bodies. Fixtures `time_post_405.json` / `health_get_no_trailing_slash_308.json` re-pinned with "contract change" notes; 405 body graded by tests in `shell_router.rs`, `extras.rs`, `goals.rs`.
- **WAL journal mode enabled, size-bounded.** Pooled connections set `journal_mode=WAL` with `wal_autocheckpoint` at the 1000-page default (~4 MB auto-fold) and `journal_size_limit=4194304` (the `-wal` file is truncated back to ≤4 MB after checkpoints — it cannot grow unboundedly). `checkpoint_truncate` runs after bootstrap and on graceful shutdown (SIGINT/SIGTERM), so the log starts at 0 bytes and disappears on clean stop. Bootstrap's raw connection and the parity goldens are untouched. Pre-v0.4.0-image rollback is formally retired; backup guidance updated in `docs/UPGRADING.md`.
- **Argon2 rehash-on-login adopted.** New hashes are argon2id (PHC format, RustCrypto defaults); legacy Werkzeug scrypt/pbkdf2 hashes keep verifying and are transparently re-stored as argon2id after the next successful login (rehash failure never fails the login). Verified live: werkzeug-seeded hash became `$argon2id$` after first login and kept verifying.
- **SSO ⇒ local login disabled (production operating model).** `DISABLE_LOCAL_LOGIN` defaults to the OIDC-configured state: with `OIDC_ISSUER_URL` set and the flag unset, `/api/config` reports `enable_local_login:false` and local login answers 403 `Local login is disabled`; explicit `0`/`1` always wins. Verified live in both directions. E2E harness (OIDC blanked) unaffected.
- **Pocket ID SSO verified live (human passkey).** Owner completed Pocket ID setup at `http://pocket-id.localhost:1411`, registered a passkey, created an OIDC client with callback `http://localhost:5173/api/auth/callback/oidc`, set `OIDC_ISSUER_URL`, and signed in end-to-end via "Sign in with SSO" through the Docker stack; the same run confirmed the OIDC auto-disable path above. Setup gotcha: the Pocket ID UI must be visited on the exact `APP_URL` origin (`pocket-id.localhost`, the WebAuthn RPID) — browsing via `127.0.0.1:1411` fails passkey registration with "Passkeys cannot be used on the configured domain".
- **#16 linux/arm64 publishing adopted.** `publish.yml` builds and pushes `linux/amd64,linux/arm64` for both images via `docker/setup-qemu-action` + full per-platform QEMU emulation (see `api/Dockerfile`'s top-of-file comment for why that beat `cross`/`cargo-zigbuild`). Locally verified on an amd64 host: arm64 image built under emulation (~29 min first build, dominated by rusqlite's bundled SQLite compile; cached afterward), `Architecture: arm64` confirmed, `uname -m` → `aarch64` in-container, `--health-check` exit 0, `GET /api/` 200 from a second container; the amd64 leg rebuilt and health-checked as a regression check.

## contract changes (owner-approved, executed 2026-08-15)

Former "bug-compatible" items fixed as deliberate contract changes; every edited fixture carries a "contract change" note:

- **#4 Mixed date-format semantics → fixed.** Bootstrap v2 migration (`PRAGMA user_version` 1→2, no schema change) normalizes stored `%m/%d/%Y` dates to ISO; POST/PUT `/api/mood` normalize the same format on write; garbage strings stay verbatim; stats SQL (incl. `_ISO_DAY_EXPR`) unchanged and now chronologically correct. Verified on a copy of the real production DB (26 US-format rows normalized; range queries return the formerly-excluded rows).
- **#5/#6 Status-code consistency → fixed.** `GET /api/mood/{id}/selections` and `GET /api/goals/{id}/completions` (all alias paths) now 404 for nonexistent/foreign ids (`Entry not found` / `Not found`), matching their sibling GETs; an existing entry whose selections were cascade-deleted still returns 200 `[]`. `POST /api/groups/{id}/options` on an unknown/foreign group now 404s (`Group not found`) instead of 400. Frontend audited — no call site breaks.
- **#7 Goal update asymmetries → fixed.** Empty PUT/PATCH body → 400 `No fields to update`; blank/whitespace title → 400 `Title is required` (create's exact string); 404 now only means a missing/foreign goal.
- **#10 `next_cursor` → fixed.** Peek-ahead pagination: `null` when no older rows exist, even on an exactly-full page.
- **CORS default origins → fixed.** The inherited default included `https://nightlio.vercel.app` (credentialed cross-origin grant to a third-party domain). Now `http://localhost:5173,http://localhost:5000`; semantics otherwise unchanged. Deployments relying on the implicit vercel origin must set `CORS_ORIGINS` (noted in `docs/UPGRADING.md`).
- **a11y/hooks lint backlog → cleared.** All 13 downgraded jsx-a11y + react-hooks warnings fixed with real accessibility work; jsx-a11y restored to error severity in `eslint.config.js`.
- Determinism fix: `GET /api/goals` gained an `id ASC` tiebreaker on `created_at DESC` (order was SQL-unspecified on same-second ties; fixture-recorded order now guaranteed).

## Earlier resolutions

- **#12 429 rate-limit body** — body string ported verbatim from the Python source (`RATE_LIMIT_MESSAGE`); buckets, limits (30/min login, 10/min register), window, fail-open, and TESTING bypass pinned by tests.
- **#13 OIDC redirect-flow** — full validation set tested against a wiremock provider (`api/tests/oidc_flow.rs`, 11 tests) plus a live pocket-id smoke (discovery, issuer validation, authorize redirect, state cookie); the human passkey leg was verified live on 2026-08-16 (see above), closing the flow end-to-end.
- **#14 `/api/music/vibe` enabled mode** — implemented and tested via the `JAMENDO_API_BASE` fake-upstream hook; flag-off 404 matches the recorded fixture.
- **#15 PDF export** — rendered **in-process** by the `markdown2pdf` crate (superseded the Python-sidecar default; sidecar, its image, and `PDF_SERVICE_URL` removed). Wire contract unchanged (400 / 413 / `application/pdf` + attachment header); the renderer-missing 501 branch is retired (compiled in); visual styling differs from PyMuPDF (accepted). Probed: unicode/emoji, tables, empty, hostile markdown, 500KB in ~106ms.
- Wave 5 adversarial-review mediums (all fixed): ProxyFix rightmost `X-Forwarded-Proto` for the Secure cookie flag; `user_version` stamped only after a fully-clean bootstrap; schema-legal REAL mood values no longer 500; PDF deployment path.
- **Achievement icon swap** — fixed: `AchievementsView.tsx` matches backend metadata (`consistency_king=Target`, `mood_master=Crown`); no icon change on unlock.
- `moodDistribution` string-keyed typing encoded in `src/types/api.ts` (was a JS number-key coercion trap).

## Legacy removal (2026-08-15)

The Flask/Python backend was removed (the old `api/` tree, `test.sh`, pytest suite). Gate: three generations of real production databases passed Rust migration byte-parity and full endpoint parity (snapshots retained under `contract/corpus/prod/`, gitignored; harness: `api/tests/manual_prod_bootstrap.rs` + the tracked `api/tests/bootstrap_parity.rs`). Consequences: fixtures + OpenAPI are the sole frozen oracle; e2e boots the Rust API only. The interim rollback path (previously published pre-v0.4.0 image tags) was formally retired on 2026-08-16 when WAL was enabled — see the WAL decision above and `docs/UPGRADING.md`.
