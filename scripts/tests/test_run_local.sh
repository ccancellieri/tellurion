#!/usr/bin/env bash

set -euo pipefail

project_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
script="$project_root/scripts/run-local.sh"

fail() {
  printf 'FAIL: %s\n' "$*" >&2
  if [ -f "${TEST_LOG:-}" ]; then
    printf '%s\n' '--- quickstart log ---' >&2
    sed -n '1,120p' "$TEST_LOG" >&2
  fi
  if [ -f "${TEST_ROOT:-}/output" ]; then
    printf '%s\n' '--- quickstart output ---' >&2
    sed -n '1,120p' "$TEST_ROOT/output" >&2
  fi
  exit 1
}

wait_for_log() {
  local expected=$1
  local attempts=0
  while ! grep -Fq "$expected" "$TEST_LOG" 2>/dev/null; do
    attempts=$((attempts + 1))
    if [ "$attempts" -gt 100 ]; then
      fail "timed out waiting for $expected"
    fi
    sleep 0.05
  done
}

assert_not_running() {
  local pid=$1
  if kill -0 "$pid" 2>/dev/null; then
    fail "child process $pid is still running"
  fi
}

reset_sync() {
  rm -f -- "$TEST_SYNC_DIR/app-ready" "$TEST_SYNC_DIR/vite-ready"
}

make_fake_tools() {
  mkdir -p "$TEST_ROOT/bin"

  printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    'printf "cargo %s\\n" "$*" >> "$TEST_LOG"' \
    'mkdir -p "$CARGO_TARGET_DIR/debug"' \
    'cp "$TEST_FAKE_APP" "$CARGO_TARGET_DIR/debug/tellurion-ingest"' \
    'cp "$TEST_FAKE_APP" "$CARGO_TARGET_DIR/debug/tellurion"' \
    'chmod +x "$CARGO_TARGET_DIR/debug/tellurion-ingest"' \
    'chmod +x "$CARGO_TARGET_DIR/debug/tellurion"' \
    > "$TEST_ROOT/bin/cargo"
  chmod +x "$TEST_ROOT/bin/cargo"

  printf '%s\n' \
    '#!/usr/bin/env bash' \
    'set -euo pipefail' \
    'printf "npm origin=%s args=%s\\n" "${TELLURION_APP_ORIGIN:-}" "$*" >> "$TEST_LOG"' \
    'if [ "$1" = "ci" ]; then' \
    '  mkdir -p "$TEST_UI_NODE_MODULES/.bin"' \
    '  cp "$TEST_FAKE_VITE" "$TEST_UI_NODE_MODULES/.bin/vite"' \
    '  chmod +x "$TEST_UI_NODE_MODULES/.bin/vite"' \
    'fi' \
    > "$TEST_ROOT/bin/npm"
  chmod +x "$TEST_ROOT/bin/npm"
}

run_and_stop() {
  : > "$TEST_LOG"
  reset_sync
  (
    cd "$TEST_ROOT/caller"
    exec env PATH="$TEST_ROOT/bin:$PATH" \
      TMPDIR="$TEST_ROOT/tmp" \
      TEST_LOG="$TEST_LOG" \
      TEST_SYNC_DIR="$TEST_SYNC_DIR" \
      TEST_PROJECT_ROOT="$project_root" \
      TEST_FAKE_APP="$project_root/scripts/tests/fixtures/run_local_fake_app.sh" \
      TEST_FAKE_VITE="$project_root/scripts/tests/fixtures/run_local_fake_vite.sh" \
      TEST_UI_NODE_MODULES="$TEST_ROOT/ui/node_modules" \
      CARGO_TARGET_DIR="$TEST_ROOT/target" \
      TELLURION_UI_DIR="$TEST_ROOT/ui" \
      TELLURION_PORT=18080 \
      TELLURION_UI_HOST=127.0.0.1 \
      TELLURION_UI_PORT=14173 \
      "$script" >"$TEST_ROOT/output" 2>&1
  ) &
  local runner_pid=$!

  wait_for_log 'vite origin=http://127.0.0.1:18080 args=--host 127.0.0.1 --port 14173 --strictPort'
  wait_for_log 'app-pid '
  wait_for_log 'vite-pid '
  wait_for_log 'vite-worker-pid '

  grep -Eq 'app demo --path .*/tmp/tellurion-local\..*/demo\.gpkg --port 18080' "$TEST_LOG" \
    || fail 'default app command did not use a temporary demo GeoPackage and selected port'
  grep -Eq '^app-cwd .*/tmp/tellurion-local\.[^/]+$' "$TEST_LOG" \
    || fail 'default app command did not run from its temporary demo directory'
  [ ! -e "$project_root/demo.gpkg" ] || fail 'default quickstart created demo.gpkg in the checkout'

  local app_pid vite_pid vite_worker_pid
  app_pid=$(awk '/^app-pid / { print $2; exit }' "$TEST_LOG")
  vite_pid=$(awk '/^vite-pid / { print $2; exit }' "$TEST_LOG")
  vite_worker_pid=$(awk '/^vite-worker-pid / { print $2; exit }' "$TEST_LOG")
  kill -TERM "$runner_pid"
  wait "$runner_pid" || true
  assert_not_running "$app_pid"
  assert_not_running "$vite_pid"
  assert_not_running "$vite_worker_pid"
}

run_child_failure() {
  : > "$TEST_LOG"
  reset_sync
  set +e
  (
    cd "$TEST_ROOT/caller"
    exec env PATH="$TEST_ROOT/bin:$PATH" \
      TMPDIR="$TEST_ROOT/tmp" \
      TEST_LOG="$TEST_LOG" \
      TEST_SYNC_DIR="$TEST_SYNC_DIR" \
      TEST_PROJECT_ROOT="$project_root" \
      TEST_FAKE_APP="$project_root/scripts/tests/fixtures/run_local_fake_app.sh" \
      TEST_FAKE_VITE="$project_root/scripts/tests/fixtures/run_local_fake_vite.sh" \
      TEST_UI_NODE_MODULES="$TEST_ROOT/ui/node_modules" \
      CARGO_TARGET_DIR="$TEST_ROOT/target" \
      TELLURION_UI_DIR="$TEST_ROOT/ui" \
      FAKE_APP_FAIL=1 \
      "$script" >"$TEST_ROOT/output" 2>&1
  )
  local status=$?
  set -e
  [ "$status" -eq 23 ] || fail "app failure returned $status instead of 23"
  wait_for_log 'vite-pid '
  local vite_pid
  vite_pid=$(awk '/^vite-pid / { print $2; exit }' "$TEST_LOG")
  assert_not_running "$vite_pid"
}

run_ui_failure() {
  : > "$TEST_LOG"
  reset_sync
  set +e
  (
    cd "$TEST_ROOT/caller"
    exec env PATH="$TEST_ROOT/bin:$PATH" \
      TMPDIR="$TEST_ROOT/tmp" \
      TEST_LOG="$TEST_LOG" \
      TEST_SYNC_DIR="$TEST_SYNC_DIR" \
      TEST_PROJECT_ROOT="$project_root" \
      TEST_FAKE_APP="$project_root/scripts/tests/fixtures/run_local_fake_app.sh" \
      TEST_FAKE_VITE="$project_root/scripts/tests/fixtures/run_local_fake_vite.sh" \
      TEST_UI_NODE_MODULES="$TEST_ROOT/ui/node_modules" \
      CARGO_TARGET_DIR="$TEST_ROOT/target" \
      TELLURION_UI_DIR="$TEST_ROOT/ui" \
      FAKE_UI_FAIL=1 \
      "$script" >"$TEST_ROOT/output" 2>&1
  )
  local status=$?
  set -e
  [ "$status" -eq 24 ] || fail "UI failure returned $status instead of 24"
  wait_for_log 'app-pid '
  local app_pid
  app_pid=$(awk '/^app-pid / { print $2; exit }' "$TEST_LOG")
  assert_not_running "$app_pid"
}

run_ui_leader_failure() {
  : > "$TEST_LOG"
  reset_sync
  set +e
  (
    cd "$TEST_ROOT/caller"
    exec env PATH="$TEST_ROOT/bin:$PATH" \
      TMPDIR="$TEST_ROOT/tmp" \
      TEST_LOG="$TEST_LOG" \
      TEST_SYNC_DIR="$TEST_SYNC_DIR" \
      TEST_PROJECT_ROOT="$project_root" \
      TEST_FAKE_APP="$project_root/scripts/tests/fixtures/run_local_fake_app.sh" \
      TEST_FAKE_VITE="$project_root/scripts/tests/fixtures/run_local_fake_vite.sh" \
      TEST_UI_NODE_MODULES="$TEST_ROOT/ui/node_modules" \
      CARGO_TARGET_DIR="$TEST_ROOT/target" \
      TELLURION_UI_DIR="$TEST_ROOT/ui" \
      FAKE_UI_LEADER_FAIL=1 \
      "$script" >"$TEST_ROOT/output" 2>&1
  )
  local status=$?
  set -e
  [ "$status" -eq 24 ] || fail "UI leader failure returned $status instead of 24"
  wait_for_log 'app-pid '
  wait_for_log 'vite-worker-pid '
  local app_pid vite_worker_pid
  app_pid=$(awk '/^app-pid / { print $2; exit }' "$TEST_LOG")
  vite_worker_pid=$(awk '/^vite-worker-pid / { print $2; exit }' "$TEST_LOG")
  assert_not_running "$app_pid"
  assert_not_running "$vite_worker_pid"
}

run_relative_target_dir() {
  : > "$TEST_LOG"
  reset_sync
  (
    cd "$TEST_ROOT/caller"
    exec env PATH="$TEST_ROOT/bin:$PATH" \
      TMPDIR="$TEST_ROOT/tmp" \
      TEST_LOG="$TEST_LOG" \
      TEST_SYNC_DIR="$TEST_SYNC_DIR" \
      TEST_PROJECT_ROOT="$project_root" \
      TEST_FAKE_APP="$project_root/scripts/tests/fixtures/run_local_fake_app.sh" \
      TEST_FAKE_VITE="$project_root/scripts/tests/fixtures/run_local_fake_vite.sh" \
      TEST_UI_NODE_MODULES="$TEST_ROOT/ui/node_modules" \
      CARGO_TARGET_DIR=relative-target \
      TELLURION_UI_DIR="$TEST_ROOT/ui" \
      "$script" >"$TEST_ROOT/output" 2>&1
  ) &
  local runner_pid=$!

  wait_for_log 'app-pid '
  wait_for_log 'vite-pid '
  kill -TERM "$runner_pid"
  wait "$runner_pid" || true
}

run_relative_paths() {
  : > "$TEST_LOG"
  reset_sync
  mkdir -p "$TEST_ROOT/caller/config" "$TEST_ROOT/caller/data"
  : > "$TEST_ROOT/caller/config/custom.yaml"
  : > "$TEST_ROOT/caller/data/demo.gpkg"
  local caller_dir
  caller_dir=$(cd "$TEST_ROOT/caller" && pwd)

  (
    cd "$TEST_ROOT/caller"
    exec env PATH="$TEST_ROOT/bin:$PATH" \
      TMPDIR="$TEST_ROOT/tmp" \
      TEST_LOG="$TEST_LOG" \
      TEST_SYNC_DIR="$TEST_SYNC_DIR" \
      TEST_PROJECT_ROOT="$project_root" \
      TEST_FAKE_APP="$project_root/scripts/tests/fixtures/run_local_fake_app.sh" \
      TEST_FAKE_VITE="$project_root/scripts/tests/fixtures/run_local_fake_vite.sh" \
      TEST_UI_NODE_MODULES="$TEST_ROOT/ui/node_modules" \
      CARGO_TARGET_DIR="$TEST_ROOT/target" \
      TELLURION_UI_DIR="$TEST_ROOT/ui" \
      TELLURION_APP_CONFIG=config/custom.yaml \
      TELLURION_GEOPACKAGE_PATH=data/demo.gpkg \
      "$script" >"$TEST_ROOT/output" 2>&1
  ) &
  local runner_pid=$!

  wait_for_log 'app-pid '
  wait_for_log 'vite-pid '
  grep -Fq "app-config $caller_dir/config/custom.yaml" "$TEST_LOG" \
    || fail 'relative application config was not normalized before the checkout directory changed'
  grep -Fq "app-geopackage $caller_dir/data/demo.gpkg" "$TEST_LOG" \
    || fail 'relative GeoPackage path was not normalized before the checkout directory changed'

  kill -TERM "$runner_pid"
  wait "$runner_pid" || true
}

TEST_ROOT=$(mktemp -d "${TMPDIR:-/tmp}/tellurion-run-local-test.XXXXXX")
TEST_LOG="$TEST_ROOT/log"
trap 'rm -rf "$TEST_ROOT"' EXIT
mkdir -p "$TEST_ROOT/tmp"
mkdir -p "$TEST_ROOT/caller" "$TEST_ROOT/ui"
TEST_SYNC_DIR="$TEST_ROOT/sync"
mkdir -p "$TEST_SYNC_DIR"
make_fake_tools
run_and_stop
run_child_failure
run_ui_failure
run_ui_leader_failure
run_relative_paths
run_relative_target_dir

printf 'PASS: run-local starts a temporary demo, forwards custom ports, and reaps children\n'
