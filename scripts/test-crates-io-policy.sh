#!/usr/bin/env bash
# Mutation tests for the explicit crates.io publication boundary.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR/.."

policy="scripts/audit-crates-io-policy.sh"
package_list="release/crates-io-packages.txt"

if ! bash "$policy" >/dev/null 2>&1; then
    echo "FAIL: the unmodified crates.io publication policy is not valid" >&2
    exit 1
fi

list_backup="$(mktemp)"
core_backup="$(mktemp)"
cp "$package_list" "$list_backup"
cp crates/tellurion-core/Cargo.toml "$core_backup"

cleanup() {
    cp "$list_backup" "$package_list"
    cp "$core_backup" crates/tellurion-core/Cargo.toml
    rm -f "$list_backup" "$core_backup"
}
trap cleanup EXIT

# A listed crate must be explicitly opted into crates.io. Falling back to the
# workspace's publish=false default must never pass silently.
perl -0pi -e 's/publish = \["crates-io"\]/publish.workspace = true/' \
    crates/tellurion-core/Cargo.toml
if bash "$policy" >/dev/null 2>&1; then
    echo "FAIL: a listed but non-publishable crate was accepted" >&2
    exit 1
fi
cp "$core_backup" crates/tellurion-core/Cargo.toml

# Removing a publishable workspace crate from the ordered allow-list must close
# the gate even though its manifest still opts in.
grep -v '^tellurion-memory$' "$list_backup" > "$package_list"
if bash "$policy" >/dev/null 2>&1; then
    echo "FAIL: a publishable crate outside the allow-list was accepted" >&2
    exit 1
fi
cp "$list_backup" "$package_list"

# Publication is sequential. A crate may only appear after every first-party
# dependency it needs for package verification.
awk '
    $0 == "tellurion-core" { print "tellurion-vector-tile"; next }
    $0 == "tellurion-vector-tile" { print "tellurion-core"; next }
    { print }
' "$list_backup" > "$package_list"
if bash "$policy" >/dev/null 2>&1; then
    echo "FAIL: a dependency-after-dependent publication order was accepted" >&2
    exit 1
fi

echo "crates.io publication policy rejects boundary and ordering mutations"
