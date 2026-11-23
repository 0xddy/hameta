# syntax=docker/dockerfile:1.7
FROM rust:1-slim-bookworm AS builder
WORKDIR /app

COPY --from=dht-crawler Cargo.toml README.md /deps/dht-crawler/
COPY --from=dht-crawler src /deps/dht-crawler/src
COPY --from=dht-crawler examples /deps/dht-crawler/examples

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY web/dist ./web/dist

RUN mkdir -p .cargo && \
    printf '[patch."https://github.com/0xddy/dht-crawler.git"]\ndht-crawler = { path = "/deps/dht-crawler" }\n' > .cargo/config.toml

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/app/target \
    touch crates/hameta-panel/src/assets.rs && \
    CARGO_PROFILE_RELEASE_LTO=off CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 \
    cargo build --release -p hameta-cli --features mimalloc && \
    cp /app/target/release/hameta /usr/local/bin/hameta

FROM debian:12-slim AS runtime
RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates curl && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/local/bin/hameta /usr/local/bin/hameta
RUN mkdir -p /opt/hameta/data

ENV HAMETA_BIND=0.0.0.0:8080 \
    HAMETA_DATA_DIR=/opt/hameta/data \
    RUST_LOG=info

EXPOSE 8080/tcp 12313/udp

HEALTHCHECK --interval=10s --timeout=3s --start-period=10s --retries=6 \
    CMD curl -fsS http://127.0.0.1:8080/healthz >/dev/null || exit 1

ENTRYPOINT ["/usr/local/bin/hameta"]
CMD ["panel"]
