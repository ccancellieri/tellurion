#!/usr/bin/env bash
# Proves a loose first-party dependency version cannot pass the license policy.

set -euo pipefail

# shellcheck source=scripts/rg-compat.sh
. "$(dirname "$0")/rg-compat.sh"
# shellcheck source=scripts/workspace-version.sh
. "$(dirname "$0")/workspace-version.sh"

version="$(workspace_version)" || exit 1

if ! rg -q 'cargo metadata --locked --no-deps --format-version 1' scripts/audit-license-policy.sh; then
    echo "FAIL: license policy metadata command is not locked" >&2
    exit 1
fi

manifest="Cargo.toml"
backup="$(mktemp)"
cp "$manifest" "$backup"
mkdir -p .worktrees
ignored_fixture="$(mktemp -d .worktrees/license-policy-audit-fixture.XXXXXX)"
git_fixture="$(mktemp -d)"
index_fixture="$(mktemp)"
rm -f "$index_fixture"
printf 'third-party fixture, deliberately not the project license\n' \
    > "$ignored_fixture/LICENSE"

cleanup() {
    cp "$backup" "$manifest"
    rm -f "$backup" "$ignored_fixture/LICENSE" \
        "$ignored_fixture/THIRD_PARTY_LICENSE" "$git_fixture/git" \
        "$index_fixture"
    rmdir "$ignored_fixture" 2>/dev/null || true
    rmdir "$git_fixture" 2>/dev/null || true
}
trap cleanup EXIT

if ! bash scripts/audit-license-policy.sh >/dev/null 2>&1; then
    echo "FAIL: license policy scanned a gitignored worktree artifact" >&2
    exit 1
fi

printf 'a separately named third-party license\n' \
    > "$ignored_fixture/THIRD_PARTY_LICENSE"
GIT_INDEX_FILE="$index_fixture" git read-tree HEAD
GIT_INDEX_FILE="$index_fixture" git add -f -- \
    "$ignored_fixture/THIRD_PARTY_LICENSE"
if ! GIT_INDEX_FILE="$index_fixture" \
    bash scripts/audit-license-policy.sh >/dev/null 2>&1; then
    echo "FAIL: license policy treated THIRD_PARTY_LICENSE as a project LICENSE" >&2
    exit 1
fi

real_git="$(command -v git)"
printf '#!/bin/sh\nif [ "$1" = "ls-files" ]; then exit 42; fi\nexec "%s" "$@"\n' \
    "$real_git" > "$git_fixture/git"
chmod +x "$git_fixture/git"
if PATH="$git_fixture:$PATH" bash scripts/audit-license-policy.sh >/dev/null 2>&1; then
    echo "FAIL: license policy accepted a failed tracked-file inventory" >&2
    exit 1
fi

# The mutation is built from the declared version, so this test keeps proving
# the same thing after a release bump instead of silently mutating nothing.
perl -0pi -e 's/(tellurion-core = \{ path = "crates\/tellurion-core", version = )"\Q'"$version"'\E"/${1}">='"$version"'"/' "$manifest"

if ! grep -Fq "version = \">=$version\"" "$manifest"; then
    echo "FAIL: the loose-version mutation did not apply to $manifest" >&2
    exit 1
fi

if bash scripts/audit-license-policy.sh >/dev/null 2>&1; then
    echo "FAIL: loose first-party dependency version was accepted" >&2
    exit 1
fi

echo "license policy rejects a loose first-party dependency version"
