#!/usr/bin/env bash
# Checks source-crate publication readiness. Native archives are governed by
# a separate gate because their feature-resolved Rust dependency graphs differ.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

./scripts/audit-crates-io-policy.sh
./scripts/audit-artifacts.sh

NOTICE='crates/tellurion-server/ui/THIRD_PARTY_NOTICES.txt'
EXPECTED_NOTICE_SHA256='ui/third-party-notice-sha256.txt'
if [ ! -s "$NOTICE" ]; then
    echo 'BLOCKED: generate the canonical UI third-party notice before packaging' >&2
    exit 1
fi

expected_notice_sha256="$(tr -d '[:space:]' < "$EXPECTED_NOTICE_SHA256")"
actual_notice_sha256="$(shasum -a 256 "$NOTICE" | awk '{print $1}')"
if [ "$actual_notice_sha256" != "$expected_notice_sha256" ]; then
    echo 'BLOCKED: canonical UI third-party notice does not match the reviewed digest' >&2
    exit 1
fi

if rg -q -i '[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}' "$NOTICE"; then
    echo 'BLOCKED: canonical UI third-party notice contains a contact address' >&2
    exit 1
fi

package_list="$(mktemp)"
trap 'rm -f "$package_list"' EXIT
cargo package \
    --manifest-path crates/tellurion-server/Cargo.toml \
    --allow-dirty \
    --no-default-features \
    --features ui \
    --no-verify \
    --list \
    >"$package_list"
rg -qx 'ui/THIRD_PARTY_NOTICES.txt' "$package_list" \
    || { echo 'BLOCKED: source crate omits the canonical UI third-party notice' >&2; exit 1; }

echo 'crates.io source readiness passed: the vendored UI notice is generated and packaged'
