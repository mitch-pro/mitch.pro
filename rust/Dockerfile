# syntax=docker/dockerfile:1
# mitch Rust services — multi-stage build with cargo-chef for layer caching.
# Build-only for now; exercised in plan Step 2 (mail sidecar) and finalized at
# Step 15 (blue/green cutover). Final image is musl-static so the binaries run
# anywhere, mirroring the prod Dockerfile's non-root `bun` user.
#
# BuildKit cache mounts on /app/target and the cargo registry give real
# incremental compilation across deploys — without them, Docker's layer
# cache for the whole builder stage is invalidated by any source change
# (even one line, via `COPY . .`), forcing a full `cargo build --release
# --workspace` from a cold target dir on every single deploy. Cache mounts
# persist outside the image layers, so a small change only recompiles the
# crate(s) that actually changed.
#
# Cache mounts are ephemeral to the RUN they're attached to — they never
# become part of a committed image layer — so the compiled binaries are
# copied out to a plain (non-cache-mounted) directory inside the same RUN,
# and the final stage copies from there instead of from /app/target.

FROM lukemathwalker/cargo-chef:latest-rust-1-alpine AS chef
RUN apk add --no-cache pkgconfig openssl-dev openssl-libs-static musl-dev make perl
WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN --mount=type=cache,target=/app/target \
    --mount=type=cache,target=/usr/local/cargo/registry \
    cargo chef cook --release --recipe-path recipe.json -j 2
COPY . .
RUN --mount=type=cache,target=/app/target \
    --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --release --workspace -j 2 && \
    mkdir -p /app/bin && \
    cp /app/target/release/mitch-* /app/bin/

FROM alpine:3.20
RUN apk add --no-cache openssh-client ffmpeg ca-certificates curl openssl
RUN addgroup -g 1000 app && adduser -u 1000 -G app -D -s /bin/sh app
COPY --from=builder /app/bin/mitch-* /usr/local/bin/
USER app
# Each service selects its binary via MITCH_SERVICE; compose sets this.
ENV MITCH_SERVICE=mitch-server
ENTRYPOINT ["/bin/sh", "-c", "exec /usr/local/bin/${MITCH_SERVICE:-mitch-server}"]
