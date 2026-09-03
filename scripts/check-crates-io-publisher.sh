#!/usr/bin/env bash
# Pin the safety properties of the ordered crates.io publisher implementation.

set -euo pipefail

# shellcheck source=scripts/rg-compat.sh
. "$(dirname "$0")/rg-compat.sh"

publisher="${CRATES_IO_PUBLISHER:-scripts/publish-crates-io.sh}"
fail() { echo "FAIL: $*" >&2; exit 1; }
require() { rg -q -- "$1" "$publisher" || fail "publisher is missing: $1"; }

[ -f "$publisher" ] || fail "missing $publisher"
require 'expected exactly 27 ordered crates'
require 'verify-crates-io-release\.sh "\$version" "\$commit"'
require 'audit-crates-io-policy\.sh'
require 'audit-license-policy\.sh'
require 'audit-publication-license\.sh'
require 'verify-canonical-origin\.sh "\$version" "\$commit"'
require 'verify-canonical-ci\.sh "\$commit"'
require '\[ "\$registry" = crates-io \]'
require 'GITHUB_ACTIONS.*!= true'
require 'Trusted Publishing cannot perform these first publications'
require 'cmp -s "\$archive" "\$remote"'
require '\[ "\$index" -lt "\$resume_index" \]'
workspace_package_count="$(rg -c 'cargo \+1\.97\.1 package --workspace --locked --no-verify' "$publisher" || true)"
[ "$workspace_package_count" -eq 1 ] || fail "publisher must package the workspace exactly once"
if rg -q 'cargo \+1\.97\.1 package .* -p "\$package"' "$publisher"; then
    fail "publisher must not repackage crates individually"
fi
require 'workspace packaging did not create \$archive'
require 'cargo \+1\.97\.1 publish --locked --registry crates-io -p "\$package"'
require 'for attempt in 1 2 3 4 5 6 7 8 9 10 11 12'
require 'Rerun with --resume-from \$package'
require 'Cargo returned \$publish_status even though \$package \$version is now byte-identical'

if rg -q -- '--allow-dirty|cargo.*publish.*--no-verify' "$publisher"; then
    fail "publisher weakens Cargo publication verification"
fi

echo "crates.io publisher contract passed"
