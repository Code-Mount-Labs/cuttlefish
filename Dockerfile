FROM node:24-bookworm-slim AS web-builder
WORKDIR /build
COPY frontend/package.json frontend/package-lock.json ./frontend/
RUN npm ci --prefix frontend
COPY frontend ./frontend
RUN npm run build --prefix frontend

FROM rust:1.96.1-bookworm AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY prompts ./prompts
COPY install ./install
COPY --from=web-builder /build/web ./web
COPY assets ./assets
RUN cargo build --locked --release -p cuttlefish-platform

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home cuttlefish
COPY --from=builder /build/target/release/cuttlefish-platform /usr/local/bin/cuttlefish-platform
USER cuttlefish
ENV HOST=0.0.0.0 PORT=8080
EXPOSE 8080
ENTRYPOINT ["cuttlefish-platform"]
