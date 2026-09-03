#!/usr/bin/env bash
# Source-crate publication is gated on the vendored UI notice, not on native
# binary dependency evidence.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
output="$(mktemp)"
trap 'rm -f "$output"' EXIT

if ! "$ROOT/scripts/check-crates-io-release-readiness.sh" >"$output" 2>&1; then
    cat "$output" >&2
    echo 'FAIL: crates.io readiness rejected the packaged UI notice' >&2
    exit 1
fi

rg -Fq 'crates.io source readiness passed' "$output" \
    || { echo 'FAIL: readiness gate did not confirm source-crate scope' >&2; exit 1; }

echo 'crates.io readiness accepts the packaged UI notice'
