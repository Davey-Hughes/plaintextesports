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

WORKDIR /app

COPY --from=builder /out/plaintextesports /app/
COPY --from=builder /out/site /app/site
# With hash-files enabled, the server reads the content hashes from hash.txt
# next to the binary (current_exe dir), so it must sit alongside /app/plaintextesports.
COPY --from=builder /out/hash.txt /app/

RUN mkdir -p /app/data

ENV RUST_LOG="info"
ENV LEPTOS_SITE_ADDR="0.0.0.0:8080"
ENV LEPTOS_SITE_ROOT="./site"
# Must match `hash-files = true` in Cargo.toml so the server references the
# content-hashed pkg filenames (resolved via the hash.txt copied above) at runtime.
ENV LEPTOS_HASH_FILES="true"
ENV DB_PATH="/app/data/cache.db"
# Mount config.toml (token, vapid, etc.), the data volume, and — optionally — the
# site icons, e.g.:
#   docker run -p 8080:8080 \
#     -v ./config.toml:/app/config.toml -v pte-data:/app/data \
#     -v "$(pwd)/icons:/app/icons:ro" plaintextesports
# Individual settings can also be passed as env (e.g. -e PANDASCORE_TOKEN=xxxx).
# Without a token the app serves demo fixture data.
#
# Favicon / PWA icons are optional and read at runtime from `icons_dir` (default
# "icons", i.e. /app/icons given WORKDIR /app). They are deliberately NOT baked
# into the image (the rasters are gitignored), so generate them once with
# scripts/icons/generate.sh and bind-mount that dir at /app/icons as shown above
# (or point icons_dir / the ICONS_DIR env at another mounted path). With nothing
# mounted there the site runs fine with no favicon — the prior placeholder.

# Persist the SQLite cache across container restarts.
VOLUME ["/app/data"]

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
  CMD curl -f http://localhost:8080/healthz || exit 1

CMD ["/app/plaintextesports"]
