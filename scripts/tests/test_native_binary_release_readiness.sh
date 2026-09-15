#!/usr/bin/env bash
# Native binary release must remain blocked until Rust notices are complete.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
output="$(mktemp)"
trap 'rm -f "$output"' EXIT

if "$ROOT/scripts/check-native-binary-release-readiness.sh" >"$output" 2>&1; then
    echo 'FAIL: native binary readiness accepted unresolved Rust notices' >&2
    exit 1
fi

rg -Fq 'prebuilt native binary archives are not ready' "$output" \
    || { echo 'FAIL: native binary gate did not state its scope' >&2; exit 1; }

echo 'native binary readiness keeps the Rust notice blocker explicit'
