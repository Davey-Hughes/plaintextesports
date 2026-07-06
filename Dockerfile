# syntax=docker/dockerfile:1

# Build stage: Rust nightly + cargo-leptos + dart-sass
FROM rustlang/rust:nightly-alpine AS builder

# gcc + musl-dev are needed to compile the bundled SQLite (rusqlite).
RUN apk update && \
    apk add --no-cache bash curl npm libc-dev binaryen gcc musl-dev

RUN npm install -g sass

RUN curl --proto '=https' --tlsv1.3 -LsSf https://github.com/leptos-rs/cargo-leptos/releases/latest/download/cargo-leptos-installer.sh | sh

WORKDIR /work
COPY . .

# Compile with BuildKit cache mounts: the cargo registry, git deps, and the
# target/ dir persist across image builds, so only changed crates recompile
# (a plain build re-downloads and re-compiles every dependency every time).
# Cache mounts are build-time only and are NOT baked into the image layer, so
# the outputs are copied out to /out in this same step to survive into the image
# (the runner stage COPYs from /out, not from target/).
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/work/target \
    cargo leptos build --release -vv && \
    mkdir -p /out && \
    cp target/release/plaintextesports /out/ && \
    cp target/release/hash.txt /out/ && \
    cp -r target/site /out/site

# Runtime stage
FROM alpine:latest AS runner

RUN apk add --no-cache ca-certificates curl

# ── Build artifacts (immutable, baked into the image) ────────────────────
# Binary + hash.txt must live in the same directory (Leptos resolves
# hash.txt relative to current_exe).
COPY --from=builder /out/plaintextesports /usr/local/bin/
COPY --from=builder /out/hash.txt /usr/local/bin/
COPY --from=builder /out/site /usr/local/share/plaintextesports/site

# ── User data directory (single mount point) ─────────────────────────────
# Mount a single host directory at /data containing config.toml, data/,
# and optionally icons/. Example:
#
#   docker run -p 8080:8080 \
#     -v /path/to/plaintextesports:/data plaintextesports
#
# Or in docker-compose:
#
#   volumes:
#     - /mnt/user/appdata/plaintextesports:/data
#
# The host directory should contain at minimum a config.toml. The data/
# subdirectory (for the SQLite cache) will be created automatically.
# Icons are optional — see scripts/icons/generate.sh.
# Individual settings can also be passed as env (e.g. -e PANDASCORE_TOKEN=xxxx).
# Without a token the app serves demo fixture data.
WORKDIR /data
RUN mkdir -p /data/data

ENV RUST_LOG="info"
ENV LEPTOS_SITE_ADDR="0.0.0.0:8080"
ENV LEPTOS_SITE_ROOT="/usr/local/share/plaintextesports/site"
# Must match `hash-files = true` in Cargo.toml so the server references the
# content-hashed pkg filenames (resolved via the hash.txt in /usr/local/bin).
ENV LEPTOS_HASH_FILES="true"
# Paths are relative to WORKDIR /data, so config.toml, data/cache.db, and
# icons/ all resolve inside the mounted volume.
ENV CONFIG_PATH="/data/config.toml"
ENV DB_PATH="/data/data/cache.db"
ENV ICONS_DIR="/data/icons"

VOLUME ["/data"]

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD curl -f http://localhost:8080/healthz || exit 1

CMD ["plaintextesports"]
