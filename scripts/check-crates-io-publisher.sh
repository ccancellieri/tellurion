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
require '\[ "\$registry" = crates-io \]'
require 'GITHUB_ACTIONS.*!= true'
require 'Trusted Publishing cannot perform these first publications'
require 'cmp -s "\$archive" "\$remote"'
require '\[ "\$index" -lt "\$resume_index" \]'
require 'cargo \+1\.97\.1 package --locked --no-verify -p "\$package"'
require 'cargo \+1\.97\.1 publish --locked --registry crates-io -p "\$package"'
require 'for attempt in 1 2 3 4 5 6 7 8 9 10 11 12'
require 'Rerun with --resume-from \$package'

if rg -q -- '--allow-dirty|cargo.*publish.*--no-verify' "$publisher"; then
    fail "publisher weakens Cargo publication verification"
fi

echo "crates.io publisher contract passed"
