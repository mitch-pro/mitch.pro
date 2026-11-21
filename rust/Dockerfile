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
#
# -j 6: an unbounded cargo build pegs every core it can see, which is fine
# on a dedicated 48-core server but will max out and heat-throttle a dev
# machine running the same build locally. 6 is enough parallelism to be
# well past the old hardcoded -j 2 without saturating either host.

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
    cargo chef cook --release --recipe-path recipe.json -j 6
COPY . .
RUN --mount=type=cache,target=/app/target \
    --mount=type=cache,target=/usr/local/cargo/registry \
    cargo build --release --workspace -j 6 && \
    mkdir -p /app/bin && \
    cp /app/target/release/mitch-* /app/bin/

FROM alpine:3.20
RUN apk add --no-cache openssh-client ffmpeg ca-certificates curl openssl nodejs
# yt-dlp for the music player (docs/music.md). The standalone musl build of a
# pinned release, checked against a pinned SHA-256 so a changed or corrupted
# download fails the build. YouTube changes often: bump the version and both
# hashes together (hashes: the release's SHA2-256SUMS file). nodejs above is the
# JavaScript runtime yt-dlp needs for YouTube.
ARG YTDLP_VERSION=2026.10.26
ARG YTDLP_SHA256_X86_64=e97a4dd919221d233e7720e18fa3f66db60fbd1d8c0846bf6deb8e88e04475a8
ARG YTDLP_SHA256_AARCH64=ea97b8564667f3cabc334c3216526de39bc684e99815ecf07b6642f4cba8af4d
RUN set -eu; \
    case "$(uname -m)" in \
      x86_64) asset=yt-dlp_musllinux; sum="$YTDLP_SHA256_X86_64" ;; \
      aarch64) asset=yt-dlp_musllinux_aarch64; sum="$YTDLP_SHA256_AARCH64" ;; \
      *) echo "no yt-dlp build for $(uname -m)" >&2; exit 1 ;; \
    esac; \
    curl -fsSL -o /usr/local/bin/yt-dlp "https://github.com/yt-dlp/yt-dlp/releases/download/${YTDLP_VERSION}/${asset}"; \
    echo "${sum}  /usr/local/bin/yt-dlp" | sha256sum -c -; \
    chmod 755 /usr/local/bin/yt-dlp; \
    /usr/local/bin/yt-dlp --version
RUN addgroup -g 1000 app && adduser -u 1000 -G app -D -s /bin/sh app
COPY --from=builder /app/bin/mitch-* /usr/local/bin/
USER app
# Each service selects its binary via MITCH_SERVICE; compose sets this.
ENV MITCH_SERVICE=mitch-server
# Tell yt-dlp which JavaScript runtime to use (music player).
ENV MUSIC_YTDLP_JS_RUNTIME=node
ENTRYPOINT ["/bin/sh", "-c", "exec /usr/local/bin/${MITCH_SERVICE:-mitch-server}"]
