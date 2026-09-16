#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
DOCKERFILE="$ROOT/Dockerfile.stac-harvest"
CONFIG="$ROOT/deploy/render/stac-harvest.yaml"
RENDER="$ROOT/render.yaml"
WORKFLOW="$ROOT/.github/workflows/daily.yml"
MANIFEST="$ROOT/data/stac/esa-worldcover/manifest.json"
COLORMAP="$ROOT/data/stac/esa-worldcover/colormap.yaml"
ITALY_MANIFEST="$ROOT/data/stac/esa-worldcover-italy/manifest.json"
ITALY_MOSAIC="$ROOT/data/stac/esa-worldcover-italy/mosaic.json"
ITALY_BUILDER="$ROOT/deploy/render/build_stac_italy.py"
ENGINE_SOURCE="$ROOT/dist/tellurion-v0.5.0-rc.1-source-02e855affea9.zip"

require_file() {
  if [ ! -f "$1" ]; then
    printf 'missing required file: %s\n' "$1" >&2
    exit 1
  fi
}

require_text() {
  if ! grep -Fq "$2" "$ROOT/$1"; then
    printf 'missing required text in %s: %s\n' "$1" "$2" >&2
    exit 1
  fi
}

require_file "$DOCKERFILE"
require_file "$CONFIG"
require_file "$RENDER"
require_file "$WORKFLOW"
require_file "$MANIFEST"
require_file "$COLORMAP"
require_file "$ITALY_MANIFEST"
require_file "$ITALY_MOSAIC"
require_file "$ITALY_BUILDER"
require_file "$ENGINE_SOURCE"

require_text Dockerfile.stac-harvest 'ARG TELLURION_VERSION=v0.5.0-rc.1'
require_text Dockerfile.stac-harvest 'ARG TELLURION_REVISION=02e855affea9'
require_text Dockerfile.stac-harvest 'tellurion-v0.5.0-rc.1-source-02e855affea9.zip'
require_text Dockerfile.stac-harvest '21c243fc5164c2561a142b47c7a4cce1b7f1e0f29e74daab68275e81704b20d0'
require_text Dockerfile.stac-harvest 'sha256sum -c'
require_text Dockerfile.stac-harvest '/app/licenses/THIRD_PARTY_NOTICES.json'
require_text Dockerfile.stac-harvest '/app/licenses/THIRD_PARTY_NOTICES.txt'
require_text deploy/render/build_stac_italy.py '"cog", "mosaic"'
require_text Dockerfile.stac-harvest 'ogr2ogr -f GPKG'
require_text Dockerfile.stac-harvest 'USER 10001:10001'
require_text Dockerfile.stac-harvest 'TELLURION_GEOPACKAGE_PATH=/app/data/worldcover.gpkg'
require_text Dockerfile.stac-harvest 'TELLURION_COG_MOSAIC_MANIFEST=/app/data/worldcover/mosaic.yaml'
require_text Dockerfile.stac-harvest 'cargo build --release --locked -p tellurion --no-default-features --features cog,geopackage'
require_text Dockerfile.stac-harvest 'chmod 0755 /app/data'
require_text Dockerfile.stac-harvest 'chmod 0644 /app/data/worldcover.gpkg'
require_text Dockerfile.stac-harvest 'COPY --chown=10001:10001 --from=data-builder /app/data /app/data'
require_text deploy/render/stac-harvest.yaml 'features: harvested_items'
require_text deploy/render/stac-harvest.yaml 'tiles: harvested_cog'
require_text deploy/render/stac-harvest.yaml 'driver: cog-mosaic'
require_text deploy/render/stac-harvest.yaml 'tiles: harvested_italy_mosaic'
require_text deploy/render/stac-harvest.yaml 'id: esa_worldcover_2021_italy'
require_text deploy/render/stac-harvest.yaml 'source_item_id'
require_text deploy/render/stac-harvest.yaml '{ name: start_datetime, type: string }'
require_text deploy/render/stac-harvest.yaml '{ name: end_datetime, type: string }'
require_text render.yaml 'name: tellurion-stac-harvest-demo'
require_text render.yaml 'data/stac/esa-worldcover-italy'
require_text render.yaml 'deploy/render/build_stac_italy.py'
require_text render.yaml 'dist/tellurion-v0.5.0-rc.1-source-02e855affea9.zip'
require_text .github/workflows/daily.yml 'tests/render_stac_harvest_contract.sh'

if grep -Eq 'write:[[:space:]]' "$CONFIG"; then
  printf 'the STAC harvest demo must not configure a write route\n' >&2
  exit 1
fi

if grep -Fq 'colormap:' "$CONFIG"; then
  printf 'the paletted WorldCover COG must use its embedded colormap\n' >&2
  exit 1
fi

python3 - "$CONFIG" <<'PY'
import sys

import yaml

with open(sys.argv[1], encoding="utf-8") as config_file:
    config = yaml.safe_load(config_file)

expected = "https://tellurion-stac-harvest-demo.onrender.com"
actual = config.get("server", {}).get("public_base_url")
if actual != expected:
    raise SystemExit(f"STAC harvest public_base_url must be {expected!r}, got {actual!r}")

for collection in config["collections"]:
    keywords = collection.get("settings", {}).get("stac", {}).get("keywords", [])
    if not all(isinstance(keyword, str) for keyword in keywords):
        raise SystemExit(f"collection {collection['id']} has a non-string STAC keyword")
PY

manifest_url=$(python3 - "$MANIFEST" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as manifest_file:
    print(json.load(manifest_file)["served_asset"]["href"])
PY
)
image_url=$(sed -n 's/^ENV TELLURION_COG_URL=//p' "$DOCKERFILE" | head -n 1)

if [ -z "$manifest_url" ] || [ "$manifest_url" != "$image_url" ]; then
  printf 'the served COG URL must exactly match the committed manifest\n' >&2
  exit 1
fi

require_text data/stac/esa-worldcover/manifest.json '"relationship": "byte-identical-official-mirror"'
require_text data/stac/esa-worldcover/manifest.json '"sha256": "5d951afb19e5fdcb90773bac374b556d425f8945ba4b719114c7f7b03157464a"'

case "$image_url" in
  *\?*|*sig=*|*token=*|*se=*)
    printf 'the image COG URL must be public and unsigned\n' >&2
    exit 1
    ;;
esac

for property in source_item_id start_datetime end_datetime product_version grid_code; do
  require_text deploy/render/stac-harvest.yaml "$property"
done

for asset in source_cog source_snapshot provenance_manifest; do
  require_text deploy/render/stac-harvest.yaml "$asset"
done

expected_source_hash='21c243fc5164c2561a142b47c7a4cce1b7f1e0f29e74daab68275e81704b20d0'
actual_source_hash=$(sha256sum "$ENGINE_SOURCE" | awk '{print $1}')
test "$actual_source_hash" = "$expected_source_hash" || {
  printf 'the STAC source archive hash does not match the checked-in build pin\n' >&2
  exit 1
}

for archive_member in Cargo.lock LICENSE THIRD_PARTY_NOTICES.json crates/tellurion-server/Cargo.toml; do
  unzip -Z1 "$ENGINE_SOURCE" | grep -Fx "$archive_member" >/dev/null || {
    printf 'the STAC source archive is missing required member: %s\n' "$archive_member" >&2
    exit 1
  }
done

for stop in \
  '{ value: 0.0, rgba: [0, 0, 0, 0] }' \
  '{ value: 10.0, rgba: [0, 100, 0, 255] }' \
  '{ value: 20.0, rgba: [255, 187, 34, 255] }' \
  '{ value: 30.0, rgba: [255, 255, 76, 255] }' \
  '{ value: 40.0, rgba: [240, 150, 255, 255] }' \
  '{ value: 50.0, rgba: [250, 0, 0, 255] }' \
  '{ value: 60.0, rgba: [180, 180, 180, 255] }' \
  '{ value: 70.0, rgba: [240, 240, 240, 255] }' \
  '{ value: 80.0, rgba: [0, 100, 200, 255] }' \
  '{ value: 90.0, rgba: [0, 150, 160, 255] }' \
  '{ value: 95.0, rgba: [0, 207, 117, 255] }' \
  '{ value: 100.0, rgba: [250, 230, 160, 255] }'; do
  require_text data/stac/esa-worldcover/colormap.yaml "$stop"
done

printf 'render STAC harvest contract passed\n'
