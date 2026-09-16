#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

require_file() {
  test -f "$ROOT/$1" || {
    printf 'missing required 3D deployment file: %s\n' "$1" >&2
    exit 1
  }
}

require_text() {
  file=$1
  text=$2
  grep -Fq -- "$text" "$ROOT/$file" || {
    printf 'missing required 3D deployment contract in %s: %s\n' "$file" "$text" >&2
    exit 1
  }
}

require_file Dockerfile-3d
require_file render.yaml
require_file deploy/render/three-d.yaml
require_file deploy/render/build_3d_demo.py
require_file demos/3d/index.html
require_file README.md

require_text Dockerfile-3d 'ARG TELLURION_VERSION=v0.3.0'
require_text Dockerfile-3d 'ARG TELLURION_TARGET=x86_64-unknown-linux-musl'
require_text Dockerfile-3d 'sha256sum -c'
require_text Dockerfile-3d 'geopackage create-tables'
require_text Dockerfile-3d '--geometry-type POLYGON'
require_text Dockerfile-3d '--columns name:TEXT,height:REAL,min_height:REAL'
require_text Dockerfile-3d '"applied":30'
require_text Dockerfile-3d 'USER 10001:10001'
require_text Dockerfile-3d 'ENTRYPOINT ["/app/tellurion"]'

require_text render.yaml 'name: tellurion-3d-demo'
require_text render.yaml 'dockerfilePath: ./Dockerfile-3d'
require_text render.yaml 'region: frankfurt'
require_text render.yaml 'plan: free'
require_text render.yaml 'deploy/render/three-d.yaml'
require_text render.yaml 'deploy/render/build_3d_demo.py'

require_text deploy/render/three-d.yaml 'id: sample_buildings_3d'
require_text deploy/render/three-d.yaml 'driver: geopackage'
require_text deploy/render/three-d.yaml 'height_property: height'
require_text deploy/render/three-d.yaml 'min_height_property: min_height'
require_text deploy/render/three-d.yaml 'tile_properties: [name, height, min_height]'

require_text deploy/render/build_3d_demo.py '"demo:synthetic": True'
require_text deploy/render/build_3d_demo.py '"height": height'
require_text deploy/render/build_3d_demo.py '"min_height": min_height'

require_text demos/3d/index.html 'Tellurion 3D capability demo'
require_text demos/3d/index.html 'https://tellurion-3d-demo.onrender.com'
require_text demos/3d/index.html 'Synthetic capability data'
require_text demos/3d/index.html '3D Tiles 1.1'
require_text demos/3d/index.html '3D GeoVolumes remains a draft'
require_text demos/3d/index.html 'const tileGroundMeters = 40075016.68557849 / (2 ** 13);'
require_text demos/3d/index.html '3dtiles/tiles/13/4095/4096.glb'

require_text README.md 'Interactive GLB scene'
require_text README.md 'no OGC API 3D GeoVolumes conformance claim'

if grep -Eq 'routing:[[:space:]]*$|write:[[:space:]]' "$ROOT/deploy/render/three-d.yaml"; then
  printf 'the public 3D demo must not configure a write route\n' >&2
  exit 1
fi

if grep -Eq '(^|[[:space:]])PORT=[0-9]+' "$ROOT/Dockerfile-3d"; then
  printf 'the 3D container must accept Render\047s dynamic PORT instead of fixing one\n' >&2
  exit 1
fi

fixture_file=$(mktemp "${TMPDIR:-/tmp}/tellurion-3d-contract.XXXXXX")
trap 'rm -f "$fixture_file"' EXIT HUP INT TERM
python3 "$ROOT/deploy/render/build_3d_demo.py" "$fixture_file"
python3 - "$fixture_file" <<'PY'
import json
import pathlib
import sys

fixture = json.loads(pathlib.Path(sys.argv[1]).read_text())
assert fixture["type"] == "FeatureCollection"
features = fixture["features"]
assert len(features) == 30
assert all(feature["geometry"]["type"] == "Polygon" for feature in features)
assert fixture["demo:synthetic"] is True
assert all(feature["properties"]["height"] > feature["properties"]["min_height"] for feature in features)
assert len({feature["properties"]["height"] for feature in features}) >= 8
print("3D fixture verified: 30 synthetic polygon footprints with varied heights.")
PY

printf 'Render 3D deployment contract verified.\n'
