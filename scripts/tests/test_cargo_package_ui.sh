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

# Generate every tracked UI artifact outside the worktree. A stale committed
# bundle or notice must fail verification, not be silently repaired by CI.
(cd "$ROOT/ui" \
    && npm ci \
    && npm run build -- --outDir "$TEST_DIR/operator" \
    && npm run build:public-demo -- --outDir "$TEST_DIR/public-demo")

python3 "$ROOT/scripts/generate-ui-third-party-notices.py" \
    --lockfile "$ROOT/ui/package-lock.json" \
    --package-root "$ROOT/ui/node_modules" \
    --operator-bundle "$TEST_DIR/operator" \
    --public-demo-bundle "$TEST_DIR/public-demo" \
    --fallbacks "$ROOT/ui/third-party-notice-fallbacks.json" \
    --output "$TEST_DIR/THIRD_PARTY_NOTICES.txt"

diff -r -q "$TEST_DIR/operator" "$ROOT/crates/tellurion-server/ui/dist" \
    || fail 'tracked operator UI differs from a clean build'
diff -r -q "$TEST_DIR/public-demo" "$ROOT/crates/tellurion-server/ui/public-demo-dist" \
    || fail 'tracked public-demo UI differs from a clean build'
cmp "$TEST_DIR/THIRD_PARTY_NOTICES.txt" \
    "$ROOT/crates/tellurion-server/ui/THIRD_PARTY_NOTICES.txt" \
    || fail 'tracked UI third-party notice differs from clean generation'
cmp "$TEST_DIR/operator/THIRD_PARTY_NOTICES.txt" \
    "$ROOT/crates/tellurion-server/ui/THIRD_PARTY_NOTICES.txt" \
    || fail 'operator bundle does not carry the canonical UI third-party notice'
cmp "$TEST_DIR/public-demo/THIRD_PARTY_NOTICES.txt" \
    "$ROOT/crates/tellurion-server/ui/THIRD_PARTY_NOTICES.txt" \
    || fail 'public-demo bundle does not carry the canonical UI third-party notice'

expected_notice_sha256="$(tr -d '[:space:]' < "$ROOT/ui/third-party-notice-sha256.txt")"
generated_notice_sha256="$(shasum -a 256 "$TEST_DIR/THIRD_PARTY_NOTICES.txt" | awk '{print $1}')"
[ "$generated_notice_sha256" = "$expected_notice_sha256" ] \
    || fail 'tracked UI third-party notice digest is stale'

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
grep -qx 'ui/dist/THIRD_PARTY_NOTICES.txt' "$TEST_DIR/package-list" \
    || fail 'tellurion package does not contain the operator bundle notice'
grep -qx 'ui/public-demo-dist/THIRD_PARTY_NOTICES.txt' "$TEST_DIR/package-list" \
    || fail 'tellurion package does not contain the public-demo bundle notice'
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
