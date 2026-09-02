#!/bin/sh

set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
WORK=$(mktemp -d)
SERVER_PID=""

cleanup() {
  if [ -n "$SERVER_PID" ]; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT INT TERM

awk '
  /^start_server\(\) \{/ { capture = 1 }
  capture { print }
  capture && /^}$/ { exit }
' "$ROOT/scripts/demo-smoke.sh" >"$WORK/start-server.sh"
. "$WORK/start-server.sh"

require_free_port() { return 0; }
require_bootable_storages() { return 0; }
fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}
curl() {
  i=0
  while [ ! -f "$RUST_LOG_CAPTURE" ] && [ "$i" -lt 100 ]; do
    i=$((i + 1))
    sleep 0.01
  done
  [ -f "$RUST_LOG_CAPTURE" ]
}

cat >"$WORK/tellurion" <<'EOF'
#!/bin/sh
printf '%s\n' "${RUST_LOG-unset}" >"$RUST_LOG_CAPTURE"
exec sleep 30
EOF
chmod +x "$WORK/tellurion"

TELLURION="$WORK/tellurion"
GPKG="$WORK/smoke.gpkg"
SMOKE_PORT=18192
RUST_LOG_CAPTURE="$WORK/rust-log"
export RUST_LOG_CAPTURE
RUST_LOG=warn
export RUST_LOG

start_server "$WORK/config.yaml"

ACTUAL=$(cat "$RUST_LOG_CAPTURE")
[ "$ACTUAL" = info ] || {
  printf 'FAIL: smoke server inherited RUST_LOG=%s, expected info\n' "$ACTUAL" >&2
  exit 1
}

printf 'PASS: smoke server forces RUST_LOG=info\n'
