#!/usr/bin/env bash
# Proves the published server crate contains distinct operator and public-demo
# UIs and both feature combinations compile without the legacy workspace-
# relative ui/dist directory.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
TEST_DIR="$(mktemp -d)"
LEGACY_DIST="$ROOT/ui/dist"
restore() {
    if [ -d "$TEST_DIR/legacy-dist" ]; then
        mv "$TEST_DIR/legacy-dist" "$LEGACY_DIST"
    fi
    rm -rf "$TEST_DIR"
}
trap restore EXIT

fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

# Build from source first so the package check covers the current UI rather
# than passing against old generated bundles. The feature-matrix caller has
# already installed dependencies and built the operator bundle, so it reuses
# that exact state and only adds the public-demo build.
if [ "${TELLURION_UI_OPERATOR_READY:-0}" = "1" ]; then
    (cd "$ROOT/ui" && npm run build:public-demo)
else
    (cd "$ROOT/ui" && npm ci && npm run build && npm run build:public-demo)
fi

python3 "$ROOT/scripts/generate-ui-third-party-notices.py" \
    --lockfile "$ROOT/ui/package-lock.json" \
    --package-root "$ROOT/ui/node_modules" \
    --operator-bundle "$ROOT/crates/tellurion-server/ui/dist" \
    --public-demo-bundle "$ROOT/crates/tellurion-server/ui/public-demo-dist" \
    --fallbacks "$ROOT/ui/third-party-notice-fallbacks.json" \
    --output "$ROOT/crates/tellurion-server/ui/THIRD_PARTY_NOTICES.txt"

cargo package \
    --manifest-path "$ROOT/crates/tellurion-server/Cargo.toml" \
    --allow-dirty \
    --no-default-features \
    --features ui \
    --no-verify \
    --list \
    >"$TEST_DIR/package-list"

grep -qx 'ui/dist/index.html' "$TEST_DIR/package-list" \
    || fail 'tellurion package does not contain the operator UI'
grep -qx 'ui/public-demo-dist/index.html' "$TEST_DIR/package-list" \
    || fail 'tellurion package does not contain the public-demo UI'
grep -qx 'ui/THIRD_PARTY_NOTICES.txt' "$TEST_DIR/package-list" \
    || fail 'tellurion package does not contain the canonical UI third-party notices'

# A stale legacy bundle must not be able to make the feature compile. Moving
# it aside demonstrates that rust-embed and build.rs resolve within the
# server crate while retaining the directory for the developer after exit.
if [ -d "$LEGACY_DIST" ]; then
    mv "$LEGACY_DIST" "$TEST_DIR/legacy-dist"
fi

cargo check \
    --manifest-path "$ROOT/Cargo.toml" \
    --locked \
    -p tellurion \
    --no-default-features \
    --features ui

cargo check \
    --manifest-path "$ROOT/Cargo.toml" \
    --locked \
    -p tellurion \
    --no-default-features \
    --features public-demo,ui
