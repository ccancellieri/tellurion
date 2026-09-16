FROM alpine:3.22 AS builder

RUN apk add --no-cache ca-certificates curl tar unzip

WORKDIR /build

ARG TELLURION_VERSION=v0.3.0
ARG TELLURION_TARGET=x86_64-unknown-linux-musl
ARG DEMO_VERSION=v0.2.0

ENV TELLURION_ARCHIVE=tellurion-${TELLURION_VERSION}-${TELLURION_TARGET}.tar.gz
ENV TELLURION_RELEASE=https://github.com/ccancellieri/tellurion/releases/download/archive-tellurion-${TELLURION_VERSION}
ENV DEMO_ARCHIVE=tellurion-italy-demo-${DEMO_VERSION}.zip
ENV DEMO_RELEASE=https://github.com/ccancellieri/tellurion/releases/download/archive-italy-demo-${DEMO_VERSION}

RUN curl --fail --location --retry 3 \
      --output "${TELLURION_ARCHIVE}" "${TELLURION_RELEASE}/${TELLURION_ARCHIVE}" \
    && curl --fail --location --retry 3 \
      --output SHA256SUMS "${TELLURION_RELEASE}/SHA256SUMS" \
    && grep "  ${TELLURION_ARCHIVE}$" SHA256SUMS > tellurion.sha256 \
    && sha256sum -c tellurion.sha256 \
    && mkdir tellurion \
    && tar -xzf "${TELLURION_ARCHIVE}" --strip-components=1 -C tellurion

RUN curl --fail --location --retry 3 \
      --output "${DEMO_ARCHIVE}" "${DEMO_RELEASE}/${DEMO_ARCHIVE}" \
    && curl --fail --location --retry 3 \
      --output "${DEMO_ARCHIVE}.sha256" "${DEMO_RELEASE}/${DEMO_ARCHIVE}.sha256" \
    && sha256sum -c "${DEMO_ARCHIVE}.sha256" \
    && mkdir demo \
    && unzip -q "${DEMO_ARCHIVE}" -d demo

RUN mkdir -p /app/data /app/licenses \
    && install -m 0755 tellurion/tellurion tellurion/tellurion-ingest /app/ \
    && install -m 0644 tellurion/LICENSE tellurion/COMMERCIAL-LICENSE.md /app/licenses/ \
    && /app/tellurion-ingest geopackage create-tables \
      --path /app/data/rome-osm.gpkg \
      --table rome_roads \
      --geometry geom \
      --srid 4326 \
      --geometry-type LINESTRING \
      --columns highway:TEXT,name:TEXT,railway:TEXT \
    && /app/tellurion-ingest geopackage load \
      --path /app/data/rome-osm.gpkg \
      --table rome_roads \
      demo/tellurion-italy-demo/output/rome-roads.geojson > /tmp/rome-load.jsonl \
    && grep -Fq '"applied":5603' /tmp/rome-load.jsonl \
    && chmod 0555 /app/data \
    && chmod 0444 /app/data/rome-osm.gpkg

FROM scratch

COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
COPY --from=builder /app/tellurion /app/tellurion
COPY --from=builder /app/data /app/data
COPY --from=builder /app/licenses /app/licenses
COPY deploy/render/vector.yaml /app/config.yaml

ENV TELLURION_CONFIG=/app/config.yaml
ENV TELLURION_GEOPACKAGE_PATH=/app/data/rome-osm.gpkg

USER 10001:10001
EXPOSE 10000

ENTRYPOINT ["/app/tellurion"]
