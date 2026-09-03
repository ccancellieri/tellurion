#!/usr/bin/env bash
# The explicit crates.io gate must refuse publication until Rust dependency
# license texts have the same auditable treatment as the packaged UI.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
output="$(mktemp)"
trap 'rm -f "$output"' EXIT

if "$ROOT/scripts/check-crates-io-release-readiness.sh" >"$output" 2>&1; then
    echo 'FAIL: crates.io readiness accepted an unresolved Rust notice gate' >&2
    exit 1
fi

rg -Fq 'Rust third-party notice coverage is not yet complete' "$output" \
    || { echo 'FAIL: readiness gate did not state the Rust notice blocker' >&2; exit 1; }

echo 'crates.io readiness keeps the Rust notice blocker explicit'
