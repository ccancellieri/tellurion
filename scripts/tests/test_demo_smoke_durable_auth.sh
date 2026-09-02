#!/bin/sh

set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
PHASE=$(awk '
  /^POLICY_CONFIG=/ { capture = 1 }
  capture && /^# --- phase 22:/ { exit }
  capture { print }
' "$ROOT/scripts/demo-smoke.sh")

if printf '%s\n' "$PHASE" | grep -Eq '^[[:space:]]+- token:'; then
  printf 'FAIL: phase 21 persists inline bearer-token values in its durable store\n' >&2
  exit 1
fi

for expected in \
  'token_env: TELLURION_SMOKE_PLATFORM_ADMIN_TOKEN' \
  'token_env: TELLURION_SMOKE_CATALOG_ADMIN_TOKEN' \
  'export TELLURION_SMOKE_PLATFORM_ADMIN_TOKEN TELLURION_SMOKE_CATALOG_ADMIN_TOKEN' \
  'unset TELLURION_SMOKE_PLATFORM_ADMIN_TOKEN TELLURION_SMOKE_CATALOG_ADMIN_TOKEN'; do
  printf '%s\n' "$PHASE" | grep -Fq "$expected" || {
    printf 'FAIL: phase 21 durable-auth fixture lacks %s\n' "$expected" >&2
    exit 1
  }
done

printf 'PASS: phase 21 durable-auth fixture persists only token_env references\n'
