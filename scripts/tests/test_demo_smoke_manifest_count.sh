#!/bin/sh

set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
WORK=$(mktemp -d)

cleanup() {
  rm -rf "$WORK"
}
trap cleanup EXIT INT TERM

MOSAIC_MANIFEST="$WORK/smoke_mosaic.yaml"
cat >"$MOSAIC_MANIFEST" <<'YAML'
version: 1
sources:
- id: mosaic_a_west
- id: mosaic_b_east
- id: mosaic_c_overlap
YAML

mkdir -p "$WORK/bin"
cat >"$WORK/bin/wc" <<'EOF'
#!/bin/sh
printf '       3\n'
EOF
chmod +x "$WORK/bin/wc"

awk '
  /^grep -E .*MOSAIC_MANIFEST/ { capture = 1 }
  capture { print }
  capture && /^ok .the manifest lists its three sources/ { exit }
' "$ROOT/scripts/demo-smoke.sh" >"$WORK/assertion.sh"

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}
ok() { :; }

PATH="$WORK/bin:$PATH"
export PATH
. "$WORK/assertion.sh"

printf 'PASS: padded BSD wc output cannot break the manifest source count\n'
