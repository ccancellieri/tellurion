#!/usr/bin/env bash
# Proves the packaged-UI check compares temporary generation with tracked files.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
FIXTURE_ROOT="$(mktemp -d)"
trap 'rm -rf "$FIXTURE_ROOT"' EXIT

fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }

make_fixture() {
    local fixture="$1"
    mkdir -p \
        "$fixture/repo/scripts/tests" \
        "$fixture/repo/ui" \
        "$fixture/repo/crates/tellurion-server/ui/dist" \
        "$fixture/repo/crates/tellurion-server/ui/public-demo-dist" \
        "$fixture/bin"
    cp "$ROOT/scripts/tests/test_cargo_package_ui.sh" "$fixture/repo/scripts/tests/"
    printf '{}\n' > "$fixture/repo/ui/package-lock.json"
    printf '{}\n' > "$fixture/repo/ui/third-party-notice-fallbacks.json"
    printf 'fresh operator\n' > "$fixture/repo/crates/tellurion-server/ui/dist/index.html"
    printf 'fresh public demo\n' > "$fixture/repo/crates/tellurion-server/ui/public-demo-dist/index.html"
    printf 'fresh notice\n' > "$fixture/repo/crates/tellurion-server/ui/THIRD_PARTY_NOTICES.txt"
    shasum -a 256 "$fixture/repo/crates/tellurion-server/ui/THIRD_PARTY_NOTICES.txt" \
        | awk '{print $1}' > "$fixture/repo/ui/third-party-notice-sha256.txt"

    printf '%s\n' \
        '#!/bin/sh' \
        'set -eu' \
        '[ "$1" = ci ] && exit 0' \
        'mode="$2"' \
        'shift 2' \
        'output=' \
        'while [ "$#" -gt 0 ]; do' \
        '    if [ "$1" = --outDir ]; then output="$2"; break; fi' \
        '    shift' \
        'done' \
        'repo_root="$(cd "$PWD/.." && pwd)"' \
        'case "$mode" in' \
        '    build) default="$repo_root/crates/tellurion-server/ui/dist"; content="fresh operator" ;;' \
        '    build:public-demo) default="$repo_root/crates/tellurion-server/ui/public-demo-dist"; content="fresh public demo" ;;' \
        '    *) exit 2 ;;' \
        'esac' \
        'output="${output:-$default}"' \
        'mkdir -p "$output"' \
        'printf "%s\n" "$content" > "$output/index.html"' \
        > "$fixture/bin/npm"

    printf '%s\n' \
        '#!/bin/sh' \
        'set -eu' \
        'output=' \
        'while [ "$#" -gt 0 ]; do' \
        '    if [ "$1" = --output ]; then output="$2"; break; fi' \
        '    shift' \
        'done' \
        '[ -n "$output" ]' \
        'mkdir -p "$(dirname "$output")"' \
        'printf "fresh notice\n" > "$output"' \
        > "$fixture/bin/python3"

    printf '%s\n' \
        '#!/bin/sh' \
        'set -eu' \
        'if [ "$1" = package ]; then' \
        '    printf "%s\n" ui/dist/index.html ui/public-demo-dist/index.html ui/THIRD_PARTY_NOTICES.txt' \
        'fi' \
        > "$fixture/bin/cargo"

    chmod +x "$fixture/bin/npm" "$fixture/bin/python3" "$fixture/bin/cargo"
}

tracked_snapshot() {
    local repo="$1"
    find \
        "$repo/crates/tellurion-server/ui/dist" \
        "$repo/crates/tellurion-server/ui/public-demo-dist" \
        "$repo/crates/tellurion-server/ui/THIRD_PARTY_NOTICES.txt" \
        "$repo/ui/third-party-notice-sha256.txt" \
        -type f -exec shasum -a 256 {} \; | sort
}

run_check() {
    local fixture="$1"
    PATH="$fixture/bin:$PATH" bash "$fixture/repo/scripts/tests/test_cargo_package_ui.sh"
}

matching="$FIXTURE_ROOT/matching"
make_fixture "$matching"
before="$(tracked_snapshot "$matching/repo")"
run_check "$matching" >/dev/null || fail 'matching generated UI artifacts were rejected'
[ "$(tracked_snapshot "$matching/repo")" = "$before" ] \
    || fail 'matching verification changed tracked UI artifacts'

bundle_drift="$FIXTURE_ROOT/bundle-drift"
make_fixture "$bundle_drift"
printf 'stale operator\n' > "$bundle_drift/repo/crates/tellurion-server/ui/dist/index.html"
printf 'unexpected\n' > "$bundle_drift/repo/crates/tellurion-server/ui/dist/stale.js"
before="$(tracked_snapshot "$bundle_drift/repo")"
if run_check "$bundle_drift" >/dev/null 2>&1; then
    fail 'bundle drift was accepted'
fi
[ "$(tracked_snapshot "$bundle_drift/repo")" = "$before" ] \
    || fail 'bundle drift verification changed tracked UI artifacts'

notice_drift="$FIXTURE_ROOT/notice-drift"
make_fixture "$notice_drift"
printf 'stale notice\n' > "$notice_drift/repo/crates/tellurion-server/ui/THIRD_PARTY_NOTICES.txt"
before="$(tracked_snapshot "$notice_drift/repo")"
if run_check "$notice_drift" >/dev/null 2>&1; then
    fail 'notice drift was accepted'
fi
[ "$(tracked_snapshot "$notice_drift/repo")" = "$before" ] \
    || fail 'notice drift verification changed tracked UI artifacts'

digest_drift="$FIXTURE_ROOT/digest-drift"
make_fixture "$digest_drift"
printf '%064d\n' 0 > "$digest_drift/repo/ui/third-party-notice-sha256.txt"
before="$(tracked_snapshot "$digest_drift/repo")"
if run_check "$digest_drift" >/dev/null 2>&1; then
    fail 'notice digest drift was accepted'
fi
[ "$(tracked_snapshot "$digest_drift/repo")" = "$before" ] \
    || fail 'digest drift verification changed tracked UI artifacts'

echo 'Packaged UI currentness tests passed'
