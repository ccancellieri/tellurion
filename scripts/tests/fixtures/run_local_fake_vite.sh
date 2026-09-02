#!/usr/bin/env bash

set -euo pipefail

printf 'vite origin=%s args=%s\n' "${TELLURION_APP_ORIGIN:-}" "$*" >> "$TEST_LOG"
printf 'vite-pid %s\n' "$$" >> "$TEST_LOG"

if [ "${FAKE_UI_FAIL:-}" = 1 ]; then
  while [ ! -e "$TEST_SYNC_DIR/app-ready" ]; do sleep 0.01; done
  exit 24
fi

(
  trap 'exit 0' INT TERM
  while :; do sleep 0.1; done
) &
worker_pid=$!
printf 'vite-worker-pid %s\n' "$worker_pid" >> "$TEST_LOG"
: > "$TEST_SYNC_DIR/vite-ready"

if [ "${FAKE_UI_LEADER_FAIL:-}" = 1 ]; then
  while [ ! -e "$TEST_SYNC_DIR/app-ready" ]; do sleep 0.01; done
  exit 24
fi

trap 'exit 0' INT TERM
while :; do sleep 0.1; done
