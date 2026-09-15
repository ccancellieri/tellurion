#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)

require_file() {
  test -f "$ROOT/$1" || {
    printf 'missing required deployment file: %s\n' "$1" >&2
    exit 1
  }
}

require_text() {
  file=$1
  text=$2
  grep -Fq -- "$text" "$ROOT/$file" || {
    printf 'missing required text in %s: %s\n' "$file" "$text" >&2
    exit 1
  }
}

require_file Dockerfile
require_file render.yaml
require_file deploy/render/vector.yaml
require_file dist/README.md
require_file dist/tellurion-v0.5.0-rc.1-source-02e855affea9.zip

require_text Dockerfile 'sha256sum -c'
require_text Dockerfile 'USER 10001:10001'
require_text Dockerfile 'ENTRYPOINT ["/app/tellurion"]'
require_text Dockerfile 'ARG TELLURION_VERSION=v0.5.0-rc.1'
require_text Dockerfile 'ARG TELLURION_REVISION=02e855affea9'
require_text Dockerfile '21c243fc5164c2561a142b47c7a4cce1b7f1e0f29e74daab68275e81704b20d0'
require_text Dockerfile 'dist/tellurion-v0.5.0-rc.1-source-02e855affea9.zip'
require_text Dockerfile 'cargo build --release --locked -p tellurion --no-default-features --features geopackage'
require_text Dockerfile 'cargo build --release --locked -p tellurion-ingest'
require_text Dockerfile '/app/licenses/THIRD_PARTY_NOTICES.json'
require_text Dockerfile '/app/licenses/THIRD_PARTY_NOTICES.txt'
require_text Dockerfile 'ARG DEMO_VERSION=v0.2.0'
require_text Dockerfile 'DEMO_ARCHIVE=tellurion-italy-demo-${DEMO_VERSION}.zip'
require_text Dockerfile 'rome-roads.geojson'
require_text Dockerfile 'grep -Fq '\''"applied":5603'\'' /tmp/rome-load.jsonl'

require_text render.yaml 'name: tellurion-vector-demo'
require_text render.yaml 'region: frankfurt'
require_text render.yaml 'plan: free'
require_text render.yaml 'healthCheckPath: /'
require_text render.yaml 'autoDeployTrigger: checksPass'
require_text render.yaml 'deploy/render/vector.yaml'
require_text render.yaml 'dist/tellurion-v0.5.0-rc.1-source-02e855affea9.zip'
require_text dist/README.md 'Source revision: `02e855affea95ab8a22fcd4744bc00000fb3b4c4`'
require_text dist/README.md 'SHA-256: `21c243fc5164c2561a142b47c7a4cce1b7f1e0f29e74daab68275e81704b20d0`'

require_text deploy/render/vector.yaml 'driver: geopackage'
require_text deploy/render/vector.yaml 'id: sample_roads'
require_text deploy/render/vector.yaml 'url_env: TELLURION_GEOPACKAGE_PATH'
require_text demos/vector/index.html 'WebMercatorQuad/{z}/{y}/{x}.mvt'

python3 - "$ROOT/deploy/render/vector.yaml" <<'PY'
import sys

import yaml

with open(sys.argv[1], encoding="utf-8") as config_file:
    config = yaml.safe_load(config_file)

expected = "https://tellurion-vector-demo.onrender.com"
actual = config.get("server", {}).get("public_base_url")
if actual != expected:
    raise SystemExit(f"vector public_base_url must be {expected!r}, got {actual!r}")
PY

if grep -Eq 'routing:[[:space:]]*$|write:[[:space:]]' "$ROOT/deploy/render/vector.yaml"; then
  printf 'the public vector demo must not configure a write route\n' >&2
  exit 1
fi

if grep -Eq '(^|[[:space:]])PORT=[0-9]+' "$ROOT/Dockerfile"; then
  printf 'the container must accept Render\047s dynamic PORT instead of fixing one\n' >&2
  exit 1
fi

expected_source_hash='21c243fc5164c2561a142b47c7a4cce1b7f1e0f29e74daab68275e81704b20d0'
actual_source_hash=$(sha256sum "$ROOT/dist/tellurion-v0.5.0-rc.1-source-02e855affea9.zip" | awk '{print $1}')
test "$actual_source_hash" = "$expected_source_hash" || {
  printf 'the vector source archive hash does not match the checked-in build pin\n' >&2
  exit 1
}

for archive_member in Cargo.lock LICENSE THIRD_PARTY_NOTICES.json crates/tellurion-server/Cargo.toml; do
  unzip -Z1 "$ROOT/dist/tellurion-v0.5.0-rc.1-source-02e855affea9.zip" | grep -Fx "$archive_member" >/dev/null || {
    printf 'the vector source archive is missing required member: %s\n' "$archive_member" >&2
    exit 1
  }
done

printf 'Render deployment contract verified.\n'
