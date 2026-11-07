FROM rust:1.96.1-bookworm AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates ./crates
COPY prompts ./prompts
COPY install ./install
COPY web ./web
COPY assets ./assets
RUN cargo build --locked --release -p cuttlefish-platform --bin cuttlefish-relay

FROM debian:bookworm-slim AS transport
ARG TARGETARCH
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl \
    && case "$TARGETARCH" in \
      amd64) digest=daf719b32ca2778770bb92955e231a07414d754378b9af328aa2642862479804 ;; \
      arm64) digest=02180b857480302fd934e9ddbd50fe2f73229f99062f88756abc6de9df5e95cf ;; \
      *) exit 2 ;; esac \
    && curl --proto '=https' --tlsv1.2 -fsSL "https://github.com/fatedier/frp/releases/download/v0.72.0/frp_0.72.0_linux_${TARGETARCH}.tar.gz" -o /tmp/frp.tar.gz \
    && echo "$digest  /tmp/frp.tar.gz" | sha256sum -c - \
    && tar -xzf /tmp/frp.tar.gz -C /tmp "frp_0.72.0_linux_${TARGETARCH}/frps" \
    && mv "/tmp/frp_0.72.0_linux_${TARGETARCH}/frps" /frps

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates nginx-light tini \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --create-home cuttlefish
COPY --from=builder /build/target/release/cuttlefish-relay /usr/local/bin/cuttlefish-relay
COPY --from=transport /frps /usr/local/bin/frps
COPY deploy/relay/frps.toml deploy/relay/nginx.conf deploy/relay/entrypoint.sh /etc/cuttlefish/
USER cuttlefish
EXPOSE 8080
ENTRYPOINT ["/usr/bin/tini", "--", "/bin/bash", "/etc/cuttlefish/entrypoint.sh"]
