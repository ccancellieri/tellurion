#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

require_file() {
  test -f "$ROOT/$1" || {
    printf 'missing required raster deployment file: %s\n' "$1" >&2
    exit 1
  }
}

require_text() {
  file=$1
  text=$2
  grep -Fq -- "$text" "$ROOT/$file" || {
    printf 'missing required raster deployment contract in %s: %s\n' "$file" "$text" >&2
    exit 1
  }
}

require_file Dockerfile.raster
require_file render.yaml
require_file deploy/render/raster.yaml
require_file README.md

require_text Dockerfile.raster 'rust:1.97.1-slim-bookworm@sha256:99e09cb2284e2ddbb73a995deee3e91783fd04d177602ccf6eab326d778ee777'
require_text Dockerfile.raster 'debian:bookworm-slim@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818'
require_text Dockerfile.raster 'ARG TELLURION_VERSION=v0.3.0'
require_text Dockerfile.raster 'ARG TELLURION_REVISION=b6eb4a5'
require_text Dockerfile.raster 'sha256sum -c'
require_text Dockerfile.raster 'cargo build --release --locked -p tellurion --no-default-features --features cog'
require_text Dockerfile.raster 'tellurion-ingest cog author'
require_text Dockerfile.raster '--output /app/data/sample_landcover.tif'
require_text Dockerfile.raster 'USER 10001:10001'
require_text Dockerfile.raster 'ENTRYPOINT ["/app/tellurion"]'

require_text render.yaml 'name: tellurion-raster-demo'
require_text render.yaml 'dockerfilePath: ./Dockerfile.raster'
require_text render.yaml 'region: frankfurt'
require_text render.yaml 'plan: free'
require_text render.yaml 'Dockerfile.raster'
require_text render.yaml 'deploy/render/raster.yaml'

require_text deploy/render/raster.yaml 'driver: cog'
require_text deploy/render/raster.yaml 'id: sample_landcover'
require_text deploy/render/raster.yaml 'url_env: TELLURION_COG_PATH'
require_text demos/raster/index.html 'WebMercatorQuad/{z}/{y}/{x}.png'
require_text demos/raster/index.html 'Open sample PNG tile'
require_text demos/raster/index.html '/tiles/WebMercatorQuad/13/3044/4380.png'

require_text README.md '**Visual entry point:** <https://ccancellieri.github.io/tellurion-demos/>'
require_text README.md 'Cloud Optimized GeoTIFF-backed raster tiles'
require_text README.md 'Dockerfile.raster'
require_text README.md 'sample_landcover'

if grep -Eq 'routing:[[:space:]]*$|write:[[:space:]]' "$ROOT/deploy/render/raster.yaml"; then
  printf 'the public raster demo must not configure a write route\n' >&2
  exit 1
fi

if grep -Eq '(^|[[:space:]])PORT=[0-9]+' "$ROOT/Dockerfile.raster"; then
  printf 'the raster container must accept Render\047s dynamic PORT instead of fixing one\n' >&2
  exit 1
fi

if grep -Fq '/collections/sample_landcover">Inspect tileset metadata' "$ROOT/demos/raster/index.html"; then
  printf 'raster viewer still links to the unsupported collection metadata route\n' >&2
  exit 1
fi

printf 'Render raster deployment contract verified.\n'
