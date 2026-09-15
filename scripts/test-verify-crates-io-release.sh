#!/usr/bin/env bash
# Functional checks for exact version/tag/commit binding.

set -euo pipefail

fixture="$(mktemp -d)"
trap 'rm -rf "$fixture"' EXIT
mkdir -p "$fixture/scripts"
cp scripts/workspace-version.sh scripts/verify-crates-io-release.sh "$fixture/scripts/"
printf '[workspace.package]\nversion = "0.5.0-rc.1"\n' > "$fixture/Cargo.toml"

git -C "$fixture" init -q
git -C "$fixture" config user.name 'Tellurion release test'
git -C "$fixture" config user.email 'release-test.invalid'
git -C "$fixture" add Cargo.toml scripts
git -C "$fixture" commit -qm 'Create release fixture'
commit="$(git -C "$fixture" rev-parse HEAD)"
git -C "$fixture" tag v0.5.0-rc.1

(cd "$fixture" && ./scripts/verify-crates-io-release.sh 0.5.0-rc.1 "$commit")

expect_rejected() {
    local name="$1"
    shift
    if (cd "$fixture" && ./scripts/verify-crates-io-release.sh "$@") >/dev/null 2>&1; then
        echo "FAIL: $name mutation was accepted" >&2
        exit 1
    fi
    echo "ok: $name mutation rejected"
}

expect_rejected wrong-version 0.5.0 "$commit"
expect_rejected abbreviated-commit 0.5.0-rc.1 "${commit:0:12}"
expect_rejected wrong-commit 0.5.0-rc.1 0000000000000000000000000000000000000000

printf 'dirty\n' > "$fixture/untracked"
expect_rejected dirty-tree 0.5.0-rc.1 "$commit"
rm "$fixture/untracked"

git -C "$fixture" tag -d v0.5.0-rc.1 >/dev/null
expect_rejected missing-tag 0.5.0-rc.1 "$commit"

echo "crates.io release identity tests passed"
