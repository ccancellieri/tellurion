#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

require_file() {
  test -f "$ROOT/$1" || {
    printf 'missing required Zarr deployment file: %s\n' "$1" >&2
    exit 1
  }
}

require_text() {
  file=$1
  text=$2
  grep -Fq -- "$text" "$ROOT/$file" || {
    printf 'missing required Zarr deployment contract in %s: %s\n' "$file" "$text" >&2
    exit 1
  }
}

require_file Dockerfile-zarr
require_file render.yaml
require_file deploy/render/zarr.yaml
require_file deploy/render/build_zarr_demo.py
require_file README.md

require_text Dockerfile-zarr 'rust:1.97.1-slim-bookworm@sha256:99e09cb2284e2ddbb73a995deee3e91783fd04d177602ccf6eab326d778ee777'
require_text Dockerfile-zarr 'debian:bookworm-slim@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818'
require_text Dockerfile-zarr 'ARG TELLURION_VERSION=v0.3.0'
require_text Dockerfile-zarr 'ARG TELLURION_REVISION=b6eb4a5'
require_text Dockerfile-zarr 'sha256sum -c'
require_text Dockerfile-zarr 'cargo build --release --locked -p tellurion --no-default-features --features zarr'
require_text Dockerfile-zarr 'python3 /build/build_zarr_demo.py /app/data'
require_text Dockerfile-zarr 'USER 10001:10001'
require_text Dockerfile-zarr 'ENTRYPOINT ["/app/tellurion"]'

require_text render.yaml 'name: tellurion-zarr-demo'
require_text render.yaml 'dockerfilePath: ./Dockerfile-zarr'
require_text render.yaml 'region: frankfurt'
require_text render.yaml 'plan: free'
require_text render.yaml 'deploy/render/zarr.yaml'
require_text render.yaml 'deploy/render/build_zarr_demo.py'

require_text deploy/render/zarr.yaml 'driver: zarr'
require_text deploy/render/zarr.yaml 'id: sample_zarr_t0'
require_text deploy/render/zarr.yaml 'id: sample_zarr_t1'
require_text deploy/render/zarr.yaml 'ramp: viridis'
require_text demos/zarr/index.html 'WebMercatorQuad/{z}/{y}/{x}.png'

require_text deploy/render/build_zarr_demo.py '"shape": [2, 256, 256]'
require_text deploy/render/build_zarr_demo.py '"chunks": [1, 64, 64]'
require_text deploy/render/build_zarr_demo.py '"tellurion:fixed_index": [selected_time]'
require_text deploy/render/build_zarr_demo.py '"demo:synthetic": True'

require_text README.md 'two deterministic fixed slices from Zarr v2 arrays'
require_text README.md 'shape `[time, y, x]`'
require_text README.md 'does not claim on-the-wire dimension'

if grep -Eq 'routing:[[:space:]]*$|write:[[:space:]]' "$ROOT/deploy/render/zarr.yaml"; then
  printf 'the public Zarr demo must not configure a write route\n' >&2
  exit 1
fi

if grep -Eq '(^|[[:space:]])PORT=[0-9]+' "$ROOT/Dockerfile-zarr"; then
  printf 'the Zarr container must accept Render\047s dynamic PORT instead of fixing one\n' >&2
  exit 1
fi

fixture_root=$(mktemp -d "${TMPDIR:-/tmp}/tellurion-zarr-contract.XXXXXX")
trap 'rm -rf "$fixture_root"' EXIT HUP INT TERM
python3 "$ROOT/deploy/render/build_zarr_demo.py" "$fixture_root"
python3 - "$fixture_root" <<'PY'
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
for selected in (0, 1):
    store = root / f"sample_zarr_t{selected}"
    zarray = json.loads((store / ".zarray").read_text())
    zattrs = json.loads((store / ".zattrs").read_text())
    assert zarray["zarr_format"] == 2
    assert zarray["shape"] == [2, 256, 256]
    assert zarray["chunks"] == [1, 64, 64]
    assert zarray["dtype"] == "|u1"
    assert zattrs["tellurion:fixed_index"] == [selected]
    assert zattrs["tellurion:extent_crs84"] == [0.005, 0.005, 0.035, 0.025]
    assert zattrs["demo:synthetic"] is True
    chunks = sorted(store.glob("[01].[0-3].[0-3]"))
    assert len(chunks) == 32
    assert all(chunk.stat().st_size == 4096 for chunk in chunks)

assert (root / "sample_zarr_t0" / "0.0.0").read_bytes() != (
    root / "sample_zarr_t0" / "1.0.0"
).read_bytes()
print("Zarr fixtures verified: 2 stores, 2 time slices, 32 chunks per store.")
PY

printf 'Render Zarr deployment contract verified.\n'
