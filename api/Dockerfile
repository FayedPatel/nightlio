# syntax=docker/dockerfile:1
#
# Multi-stage build: rust:1.90-slim compiles a static-ish release binary,
# debian:bookworm-slim runs it. Not distroless: the HEALTHCHECK below execs
# a subcommand inside the container (`nightlio-api --health-check`), and
# compose/`docker inspect --health` need a shell-less `CMD [...]` exec form
# to work, which distroless images (no shell, sometimes no exec at all
# beyond the entrypoint) make awkward. debian:bookworm-slim keeps a normal
# userland for that and for any future `docker exec` debugging.
#
# Multi-platform strategy (linux/amd64 + linux/arm64): plain QEMU-emulated
# per-platform builds, NOT cross-compilation. None of the FROM lines below
# pin `--platform=$BUILDPLATFORM`, so under `docker buildx build --platform
# linux/amd64,linux/arm64` every stage (chef/planner/builder AND runtime)
# builds natively *for its target platform* -- on an amd64 host that means
# the arm64 leg runs entirely under QEMU emulation (binfmt_misc), including
# `cargo build` itself and the `apt-get install build-essential` step that
# gives rusqlite's `bundled` feature a C toolchain to compile SQLite
# against. This was picked over a cross-compiling builder (`cargo-zigbuild`
# / `aarch64-unknown-linux-gnu` + an aarch64 gcc, with `--platform=
# $BUILDPLATFORM` pinned on the chef/planner/builder stages so only the
# *compiler* runs natively) because it needs zero toolchain/target changes
# here -- rustc, cargo-chef, and apt already "just work" once QEMU can run
# aarch64 binaries. Trade-off: the arm64 leg is slow (emulated compilation;
# verified locally on an amd64 host, a from-scratch build -- `cargo chef
# cook` including cargo-chef's own compile, dominated by rusqlite's bundled
# SQLite C sources -- took ~29 minutes, plus ~4 more for `cargo build
# --release --bin nightlio-api`; a from-cache rebuild is fast like any
# other Docker layer cache hit), so CI cache hygiene (see publish.yml's
# cargo/target cache, scoped per matrix.image) matters even more for that
# leg. If arm64 build time in CI becomes a problem, switch to the
# cross-compiling builder described above instead of adding
# `--platform=$BUILDPLATFORM` piecemeal -- runtime must still target the
# *actual* target platform (unset `--platform` on that FROM, as today) or
# the resulting image runs arm64 userland under emulation on arm64 hosts
# for no reason.
#
# Locally verified (amd64 host): `docker run --privileged --rm
# tonistiigi/binfmt --install arm64`, then `docker buildx build --platform
# linux/arm64 --load -t nightlio-api:arm64-test ./api` completes;
# `docker image inspect` reports `Architecture: arm64`; `docker run --rm
# --platform linux/arm64 nightlio-api:arm64-test uname -m` reports
# `aarch64`; a running container answers `docker exec ... nightlio-api
# --health-check` with exit 0 and serves `GET /api/` with HTTP 200 to
# another container on the same network. amd64 rebuilt and re-checked the
# same way as a regression check (fast: fully layer-cached, no emulation).

# ---- cargo-chef base: split "compile dependencies" from "compile our code" ----
# Without this split, changing a single line in src/ invalidates the layer
# that rebuilds every dependency (rusqlite's bundled C sqlite3,
# openidconnect's TLS stack, ...), which is by far the most expensive part
# of the build. cargo-chef computes a recipe from Cargo.toml/Cargo.lock only,
# so that recipe -- and the dependency-build layer keyed on it -- stays
# cached across source-only changes and only invalidates when dependencies
# actually change.
FROM rust:1.90-slim AS chef
WORKDIR /app
RUN cargo install cargo-chef --locked

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
# rusqlite's `bundled` feature compiles SQLite's C sources itself (see
# Cargo.toml: this is load-bearing, it pins the SQLite version so
# GLOB/printf/instr/julianday/strftime('%w') semantics cannot drift under a
# system libsqlite3). That still needs a C toolchain, which rust:slim
# (unlike the non-slim rust:bookworm image) does not ship.
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential \
    && rm -rf /var/lib/apt/lists/*
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --bin nightlio-api

# ---- Runtime ----
FROM debian:bookworm-slim AS runtime
WORKDIR /app

# ca-certificates: reqwest (OIDC discovery + token exchange, Jamendo calls)
# uses rustls, which needs no system TLS library, but it still needs a
# trust store to validate certificates against.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Unprivileged runtime user, uid PINNED TO 1000 ON PURPOSE.
#
# Every existing deployment's `nightlio_data` volume was populated by the
# old Flask image, which also runs as a uid-1000 `appuser` (see
# api/Dockerfile) and chowns /app/data to it. If this image used a
# different uid -- even from an identically-named `appuser`, useradd
# without --uid picks the next free uid, which is not guaranteed to be
# 1000 -- the volume's files stay owned by the old uid across the upgrade
# and this process fails to open its own database on first boot. Keep this
# uid fixed at 1000 across any future base-image change.
RUN useradd --create-home --uid 1000 --shell /usr/sbin/nologin appuser \
    && mkdir -p /app/data \
    && chown -R appuser:appuser /app/data

COPY --from=builder /app/target/release/nightlio-api /usr/local/bin/nightlio-api

USER appuser

# Binds [::] (dual-stack; falls back to 0.0.0.0 if the host lacks IPv6 --
# see src/main.rs) on the port read from $PORT (default 5000), so nginx's
# default `proxy_pass http://api:5000` keeps resolving unchanged.
EXPOSE 5000

# The old Flask image's own HEALTHCHECK imported `requests`, a package that
# was never even in requirements.txt -- broken from day one. This image
# ships no python and no curl; the binary answers its own health check
# directly via a subcommand: GET http://localhost:$PORT/api/ against
# itself, exit 0 if it answered 2xx, 1 otherwise (see src/main.rs
# run_health_check). docker-compose.yml's api healthcheck must invoke this
# same subcommand, not a python/curl one-liner.
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD ["/usr/local/bin/nightlio-api", "--health-check"]

CMD ["/usr/local/bin/nightlio-api"]
