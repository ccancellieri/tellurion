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

reject_write_route() {
  python3 "$ROOT/tests/assert_demo_configs_read_only.py" "$1"
}

require_yaml_dependency() {
  python3 -c 'import yaml' >/dev/null 2>&1 || {
    printf 'missing test dependency: PyYAML\n' >&2
    printf 'install with: python3 -m pip install -r tests/requirements.txt\n' >&2
    exit 1
  }
}

require_file Dockerfile
require_file render.yaml
require_file deploy/render/vector.yaml
require_file reproduce/config-rome-osm.yaml
require_file tests/fixtures/write-routes/flow-map.yaml
require_file tests/fixtures/write-routes/quoted-key.yaml
require_file tests/fixtures/write-routes/spaced-key.yaml
require_file tests/requirements.txt

require_text Dockerfile 'sha256sum -c'
require_text Dockerfile 'USER 10001:10001'
require_text Dockerfile 'ENTRYPOINT ["/app/tellurion"]'
require_text Dockerfile 'ARG DEMO_VERSION=v0.2.0'
require_text Dockerfile 'DEMO_ARCHIVE=tellurion-italy-demo-${DEMO_VERSION}.zip'
require_text Dockerfile 'rome-roads.geojson'
require_text Dockerfile 'grep -Fq '\''"applied":5603'\'' /tmp/rome-load.jsonl'

require_text render.yaml 'name: tellurion-vector-rome'
require_text render.yaml 'region: frankfurt'
require_text render.yaml 'plan: free'
require_text render.yaml 'healthCheckPath: /readyz'

require_text deploy/render/vector.yaml 'driver: geopackage'
require_text deploy/render/vector.yaml 'id: rome_roads'
require_text deploy/render/vector.yaml 'url_env: TELLURION_GEOPACKAGE_PATH'
require_text tests/requirements.txt 'PyYAML==6.0.2'

require_yaml_dependency

for config in deploy/render/vector.yaml reproduce/config-rome-osm.yaml; do
  if ! reject_write_route "$ROOT/$config"; then
    printf 'the demo configuration must not configure a write route: %s\n' "$config" >&2
    exit 1
  fi
done

for fixture in \
  tests/fixtures/write-routes/flow-map.yaml \
  tests/fixtures/write-routes/quoted-key.yaml \
  tests/fixtures/write-routes/spaced-key.yaml
do
  if reject_write_route "$ROOT/$fixture" 2>/dev/null; then
    printf 'the write-route guard accepted a write-enabled fixture: %s\n' "$fixture" >&2
    exit 1
  fi
done

require_text index.html 'https://tellurion-vector-rome.onrender.com/public/features/catalogs/default/collections/rome_roads'
require_text index.html 'Technical API'
require_text README.md 'https://ccancellieri.github.io/tellurion-italy-demo/'
require_text README.md 'Technical API'

if grep -Eq '^Live service: <https://tellurion-vector-rome\.onrender\.com/>$' "$ROOT/README.md"; then
  printf 'the README must direct visitors to the static Pages site instead of the raw API root\n' >&2
  exit 1
fi

if grep -Eq '(^|[[:space:]])PORT=[0-9]+' "$ROOT/Dockerfile"; then
  printf 'the container must accept Render\047s dynamic PORT instead of fixing one\n' >&2
  exit 1
fi

if test "${SKIP_YAML_DEPENDENCY_CONTRACT:-0}" != 1; then
  sh "$ROOT/tests/yaml_dependency_contract.sh"
fi

printf 'Render deployment contract verified.\n'
