#!/usr/bin/env bash

# Start an isolated GeoPackage-backed Tellurion instance and its Vite UI.
# The default dataset lives in a temporary directory and is removed when this
# command exits. Set TELLURION_APP_CONFIG to run an explicit configuration
# instead, or TELLURION_GEOPACKAGE_PATH to retain the generated GeoPackage.

set -euo pipefail

project_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
caller_dir=$PWD
target_dir=${CARGO_TARGET_DIR:-"$project_root/target"}
app_port=${TELLURION_PORT:-8080}
ui_host=${TELLURION_UI_HOST:-127.0.0.1}
ui_port=${TELLURION_UI_PORT:-4173}
app_origin=${TELLURION_APP_ORIGIN:-"http://127.0.0.1:$app_port"}
app_config=${TELLURION_APP_CONFIG:-}
geopackage_path=${TELLURION_GEOPACKAGE_PATH:-}
ui_dir=${TELLURION_UI_DIR:-"$project_root/ui"}
app_pid=
ui_pid=
temporary_demo_dir=

require_port() {
  case "$1" in
    ''|*[!0-9]*)
      printf 'invalid %s: %s\n' "$2" "$1" >&2
      exit 2
      ;;
  esac

  if [ "$1" -gt 65535 ]; then
    printf 'invalid %s: %s\n' "$2" "$1" >&2
    exit 2
  fi
}

absolute_path() {
  local path=$1
  local directory
  directory=$(cd "$(dirname "$path")" && pwd)
  printf '%s/%s\n' "$directory" "$(basename "$path")"
}

if [ "${target_dir#/}" = "$target_dir" ]; then
  target_dir="$caller_dir/$target_dir"
fi

stop_process_group() {
  local pid=$1
  if [ -z "$pid" ]; then
    return
  fi
  if kill -0 -- "-$pid" 2>/dev/null; then
    kill -TERM -- "-$pid" 2>/dev/null || true
  elif kill -0 "$pid" 2>/dev/null; then
    kill -TERM "$pid" 2>/dev/null || true
  fi
}

cleanup() {
  local status=$?
  trap - EXIT INT TERM
  stop_process_group "$app_pid"
  stop_process_group "$ui_pid"
  [ -z "$app_pid" ] || wait "$app_pid" 2>/dev/null || true
  [ -z "$ui_pid" ] || wait "$ui_pid" 2>/dev/null || true
  [ -z "$temporary_demo_dir" ] || rm -rf "$temporary_demo_dir"
  exit "$status"
}

wait_for_first_exit() {
  while :; do
    if ! kill -0 "$app_pid" 2>/dev/null; then
      wait "$app_pid"
      return
    fi
    if ! kill -0 "$ui_pid" 2>/dev/null; then
      wait "$ui_pid"
      return
    fi
    sleep 0.1
  done
}

require_port "$app_port" TELLURION_PORT
require_port "$ui_port" TELLURION_UI_PORT
if [ -n "$app_config" ] && [ ! -f "$app_config" ]; then
  printf 'TELLURION_APP_CONFIG does not name a readable file: %s\n' "$app_config" >&2
  exit 2
fi
if [ -n "$app_config" ]; then
  app_config=$(absolute_path "$app_config")
fi
if [ -n "$geopackage_path" ]; then
  geopackage_path=$(absolute_path "$geopackage_path")
fi
if [ "${TELLURION_UI_DIR:-}" != "" ]; then
  ui_dir=$(absolute_path "$ui_dir")
fi

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

cd "$project_root"
CARGO_TARGET_DIR="$target_dir" CARGO_BUILD_JOBS="${TELLURION_BUILD_JOBS:-2}" \
  cargo build -p tellurion -p tellurion-ingest
(
  cd "$ui_dir"
  npm ci
)

# Each background service owns a process group, so cleanup reaches direct
# descendants such as a Vite worker rather than leaving them behind.
set -m

if [ -n "$app_config" ]; then
  if [ -n "$geopackage_path" ]; then
    PORT="$app_port" TELLURION_CONFIG="$app_config" \
      TELLURION_GEOPACKAGE_PATH="$geopackage_path" "$target_dir/debug/tellurion" &
  else
    PORT="$app_port" TELLURION_CONFIG="$app_config" "$target_dir/debug/tellurion" &
  fi
else
  if [ -z "$geopackage_path" ]; then
    temporary_demo_dir=$(mktemp -d "${TMPDIR:-/tmp}/tellurion-local.XXXXXX")
    geopackage_path="$temporary_demo_dir/demo.gpkg"
  fi
  demo_working_dir=$(cd "$(dirname "$geopackage_path")" && pwd)
  (
    cd "$demo_working_dir"
    exec "$target_dir/debug/tellurion-ingest" demo --path "$geopackage_path" --port "$app_port"
  ) &
fi
app_pid=$!

(
  cd "$ui_dir"
  exec env TELLURION_APP_ORIGIN="$app_origin" \
    "$ui_dir/node_modules/.bin/vite" --host "$ui_host" --port "$ui_port" --strictPort
) &
ui_pid=$!

printf 'Tellurion API: %s\nTellurion UI: http://%s:%s\n' "$app_origin" "$ui_host" "$ui_port"
wait_for_first_exit
