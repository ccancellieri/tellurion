# syntax=docker/dockerfile:1
#
# Multi-stage build: compile in a full Rust toolchain image, run in a slim
# Debian image with only the two release binaries and ca-certificates.
# Runs as an arbitrary non-root UID with group 0 so it is compatible with
# OpenShift's restricted SCC (which assigns a random UID and keeps GID 0).

FROM rust:1.97-bookworm AS builder

# aws-lc-sys (pulled in transitively via rustls, used by the metrics exporter)
# builds its bundled C sources with cmake + cc; pkg-config is needed by a
# handful of other native deps resolved in Cargo.lock.
RUN apt-get update && apt-get install -y --no-install-recommends \
        cmake \
        pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY . .

RUN cargo build --release --locked -p tellurion -p tellurion-ingest

FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/tellurion /usr/local/bin/tellurion
COPY --from=builder /build/target/release/tellurion-ingest /usr/local/bin/tellurion-ingest

# /etc/tellurion holds the mounted config; /var/lib/tellurion is scratch
# space for whichever UID the runtime assigns, as long as it belongs to
# group 0 (true for OpenShift's restricted SCC and for the USER below).
RUN mkdir -p /etc/tellurion /var/lib/tellurion \
    && chgrp -R 0 /etc/tellurion /var/lib/tellurion \
    && chmod -R g=u /etc/tellurion /var/lib/tellurion

USER 10001:0
WORKDIR /var/lib/tellurion

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=15s --retries=3 \
    CMD curl -fsS http://localhost:8080/healthz || exit 1

ENTRYPOINT ["tellurion"]
