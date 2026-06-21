# Build stage: Rust nightly + cargo-leptos + dart-sass
FROM rustlang/rust:nightly-alpine AS builder

RUN apk update && \
    apk add --no-cache bash curl npm libc-dev binaryen

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

ENV RUST_LOG="info"
ENV LEPTOS_SITE_ADDR="0.0.0.0:8080"
ENV LEPTOS_SITE_ROOT="./site"
ENV DISPLAY_TZ="America/Los_Angeles"
# Provide the API token at run time, e.g.:
#   docker run -p 8080:8080 -e PANDASCORE_TOKEN=xxxx plaintextesports
# Without a token the app serves demo fixture data.

EXPOSE 8080

CMD ["/app/plaintextesports"]
