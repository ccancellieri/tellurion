FROM rust:1.97.1-slim-bookworm@sha256:99e09cb2284e2ddbb73a995deee3e91783fd04d177602ccf6eab326d778ee777 AS source-builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
      build-essential ca-certificates cmake pkg-config unzip \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

ARG TELLURION_VERSION=v0.5.0-rc.1
ARG TELLURION_REVISION=02e855affea9
ENV SOURCE_ARCHIVE=tellurion-v0.5.0-rc.1-source-02e855affea9.zip

COPY dist/tellurion-v0.5.0-rc.1-source-02e855affea9.zip /build/tellurion-v0.5.0-rc.1-source-02e855affea9.zip

RUN printf '%s  %s\n' \
      '21c243fc5164c2561a142b47c7a4cce1b7f1e0f29e74daab68275e81704b20d0' \
      "${SOURCE_ARCHIVE}" > source.sha256 \
    && sha256sum -c source.sha256 \
    && unzip -q "${SOURCE_ARCHIVE}"

RUN cargo build --release --locked -p tellurion --no-default-features --features geopackage \
    && cargo build --release --locked -p tellurion-ingest

FROM debian:bookworm-slim@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818 AS data-builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl unzip \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

ARG DEMO_VERSION=v0.2.0
ENV DEMO_ARCHIVE=tellurion-italy-demo-${DEMO_VERSION}.zip
ENV DEMO_RELEASE=https://github.com/ccancellieri/tellurion-italy-demo/releases/download/demo-${DEMO_VERSION}

COPY --from=source-builder /build/target/release/tellurion-ingest /usr/local/bin/tellurion-ingest

RUN curl --fail --location --retry 3 \
      --output "${DEMO_ARCHIVE}" "${DEMO_RELEASE}/${DEMO_ARCHIVE}" \
    && curl --fail --location --retry 3 \
      --output "${DEMO_ARCHIVE}.sha256" "${DEMO_RELEASE}/${DEMO_ARCHIVE}.sha256" \
    && sha256sum -c "${DEMO_ARCHIVE}.sha256" \
    && mkdir demo \
    && unzip -q "${DEMO_ARCHIVE}" -d demo

RUN mkdir -p /app/data \
    && tellurion-ingest geopackage create-tables \
      --path /app/data/sample-roads.gpkg \
      --table sample_roads \
      --geometry geom \
      --srid 4326 \
      --geometry-type LINESTRING \
      --columns highway:TEXT,name:TEXT,railway:TEXT \
    && tellurion-ingest geopackage load \
      --path /app/data/sample-roads.gpkg \
      --table sample_roads \
      demo/tellurion-italy-demo/output/rome-roads.geojson > /tmp/rome-load.jsonl \
    && grep -Fq '"applied":5603' /tmp/rome-load.jsonl \
    && chmod 0555 /app/data \
    && chmod 0444 /app/data/sample-roads.gpkg

FROM debian:bookworm-slim@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818

COPY --from=source-builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=source-builder /build/target/release/tellurion /app/tellurion
COPY --from=source-builder /build/LICENSE /app/licenses/LICENSE
COPY --from=source-builder /build/COMMERCIAL-LICENSE.md /app/licenses/COMMERCIAL-LICENSE.md
COPY --from=source-builder /build/THIRD_PARTY_NOTICES.json /app/licenses/THIRD_PARTY_NOTICES.json
COPY --from=source-builder /build/THIRD_PARTY_NOTICES.txt /app/licenses/THIRD_PARTY_NOTICES.txt
COPY --chown=10001:10001 --from=data-builder /app/data /app/data
COPY deploy/render/vector.yaml /app/config.yaml
COPY deploy/render/roads-style.json /app/styles/roads.json

ENV TELLURION_CONFIG=/app/config.yaml
ENV TELLURION_GEOPACKAGE_PATH=/app/data/sample-roads.gpkg

USER 10001:10001
EXPOSE 10000

ENTRYPOINT ["/app/tellurion"]
