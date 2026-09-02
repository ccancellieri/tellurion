#!/bin/sh

set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT INT TERM

awk '
  /^has_png_signature\(\) \{/ { capture = 1 }
  capture { print }
  capture && /^}$/ { exit }
' "$ROOT/scripts/demo-smoke.sh" >"$WORK/png-signature.sh"
. "$WORK/png-signature.sh"

printf '\211PNG\r\n\032\n' >"$WORK/valid.png"
printf 'not-a-png' >"$WORK/invalid.png"

has_png_signature "$WORK/valid.png" || {
  printf 'FAIL: PNG signature helper rejected a valid PNG prefix\n' >&2
  exit 1
}

if has_png_signature "$WORK/invalid.png"; then
  printf 'FAIL: PNG signature helper accepted a non-PNG prefix\n' >&2
  exit 1
fi

printf 'PASS: demo smoke PNG signature helper compares bytes portably\n'
