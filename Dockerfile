# Build stage: Rust nightly + cargo-leptos + dart-sass
FROM rustlang/rust:nightly-alpine AS builder

# gcc + musl-dev are needed to compile the bundled SQLite (rusqlite).
RUN apk update && \
    apk add --no-cache bash curl npm libc-dev binaryen gcc musl-dev

RUN npm install -g sass

RUN curl --proto '=https' --tlsv1.3 -LsSf https://github.com/leptos-rs/cargo-leptos/releases/latest/download/cargo-leptos-installer.sh | sh

WORKDIR /work
COPY . .

RUN cargo leptos build --release -vv

# Runtime stage
FROM alpine:latest AS runner

RUN apk add --no-cache ca-certificates

WORKDIR /app

COPY --from=builder /work/target/release/plaintextesports /app/
COPY --from=builder /work/target/site /app/site

RUN mkdir -p /app/data

ENV RUST_LOG="info"
ENV LEPTOS_SITE_ADDR="0.0.0.0:8080"
ENV LEPTOS_SITE_ROOT="./site"
ENV DB_PATH="/app/data/cache.db"
# Mount config.toml (token, vapid, etc.) and the data volume, e.g.:
#   docker run -p 8080:8080 \
#     -v ./config.toml:/app/config.toml -v pte-data:/app/data plaintextesports
# Individual settings can also be passed as env (e.g. -e PANDASCORE_TOKEN=xxxx).
# Without a token the app serves demo fixture data.

# Persist the SQLite cache across container restarts.
VOLUME ["/app/data"]

EXPOSE 8080

CMD ["/app/plaintextesports"]
