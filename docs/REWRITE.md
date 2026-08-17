# The v0.4.0 rewrite, before and after

v0.4.0 replaced the Flask/Python backend with a Rust API (Axum + rusqlite) and converted the frontend and browser-test suite to strict TypeScript. This page is the honest scorecard: what measurably changed, what structurally changed, and what it cost.

## Comparison

| Dimension | Before — Flask (v0.3.0) | After — Rust + TS (v0.4.0) |
|---|---|---|
| API image (disk) | 624 MB | **158 MB** (−75%) |
| API image (compressed pull) | 161 MB | **41 MB** |
| PDF export | in-process, Python `markdown-pdf` library | in-process, `markdown2pdf` Rust crate — same wire contract (a Python sidecar container was the rewrite's interim design, retired before release: `contract/DECISIONS.md` #15) |
| API memory under load | ~290 MiB (gunicorn worker pool) | **~7.5 MiB** (single process) |
| p99 latency, health endpoint @ 20 concurrent | 12 ms | **6 ms** |
| p99 latency, authenticated statistics | 19 ms | **8 ms** |
| Throughput | ~4.0k req/s (client-limited) | ~4.4k req/s (client-limited) — effectively a wash |
| Runtime model | worker pool, GC pauses | single async binary, no GC |
| CPU architectures | amd64 only | **amd64 + arm64** |
| Wire contract | implicit — "whatever Flask does" | **explicit** — golden fixtures recorded against Flask's 46 URL rules (the contract now has 47, the addition being `POST /api/statistics/view`) + OpenAPI 3.1 + the `contract/DECISIONS.md` ledger |
| Backend tests | pytest, mostly happy paths (171) | **400 unit + 109 integration**, graded against the golden fixtures, clippy `-D warnings` clean |
| Frontend typing | untyped JS, silent shape drift possible | **strict TS**, client types derived from the same OpenAPI document the API is graded against |
| GETs with side effects | statistics and goals wrote on read | **all reads pure** |
| Latent bugs surfaced by the port | stale-week goal clamp, REAL mood values 500ing, achievement views counted 2–4× per visit, a credentialed CORS grant to a third-party domain, mixed date formats excluding rows from range queries | **found, fixed, and test-pinned** |
| Password hashes | legacy Werkzeug scrypt/pbkdf2 | **argon2id**, transparent rehash on login |
| DB journal mode | rollback journal | **WAL**, size-bounded, checkpoint-truncate on shutdown |
| Dev-loop cost | instant Python reload | cargo compile times; ~30 min first arm64 CI build (cached after) |

The top half of the table is what a small VPS notices; the bottom half is where the durable value sits. The last two rows are the price paid.

## How the numbers were measured

Both images ran side by side on the same host (2026-08-16): the published v0.3.0 `nightlio-api` image (gunicorn/Flask) and the v0.4.0 Rust image built from source, identical secrets, fresh SQLite each. Memory is `docker stats` under load; latency/throughput is ApacheBench at 20 concurrent connections (2,000 requests against the health endpoint, 1,000 against authenticated `GET /api/statistics`). Around 4k req/s the single-threaded `ab` client is the bottleneck, so throughput numbers are floors — the tail-latency comparison is the meaningful one. Image sizes are `docker images` disk size and registry-compressed size.

## Why throughput barely moved and it still mattered

For a single-user self-hosted journal, Flask was never latency-bound — mean response times were fine. The wins that justify the rewrite are the 99th-percentile latency (no worker-pool queuing or GC pauses), the ~40× memory reduction on hosts shared with Pocket ID and a reverse proxy, the 75% smaller image pulls, and — most durably — that the port forced every line of backend behavior through a recorded contract, which is how the latent bugs above were found. The full decision-by-decision record lives in [`contract/DECISIONS.md`](../contract/DECISIONS.md).
