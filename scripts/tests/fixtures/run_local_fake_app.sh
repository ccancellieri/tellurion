#!/usr/bin/env bash

set -euo pipefail

printf 'app %s\n' "$*" >> "$TEST_LOG"
printf 'app-cwd %s\n' "$PWD" >> "$TEST_LOG"
printf 'app-config %s\n' "${TELLURION_CONFIG:-}" >> "$TEST_LOG"
printf 'app-geopackage %s\n' "${TELLURION_GEOPACKAGE_PATH:-}" >> "$TEST_LOG"
printf 'app-pid %s\n' "$$" >> "$TEST_LOG"

if [ "${FAKE_APP_FAIL:-}" = 1 ]; then
  while [ ! -e "$TEST_SYNC_DIR/vite-ready" ]; do sleep 0.01; done
  exit 23
fi

: > "$TEST_SYNC_DIR/app-ready"
trap 'exit 0' INT TERM

while :; do sleep 0.1; done
