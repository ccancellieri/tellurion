#!/usr/bin/env bash
# Enforce the fail-closed manual crates.io publication workflow contract.

set -euo pipefail

# shellcheck source=scripts/rg-compat.sh
. "$(dirname "$0")/rg-compat.sh"

workflow="${PUBLISH_WORKFLOW:-.github/workflows/publish-crates.yml}"
fail() { echo "FAIL: $*" >&2; exit 1; }
require() { rg -q -- "$1" "$workflow" || fail "missing publish workflow behavior: $1"; }

[ -f "$workflow" ] || fail "missing $workflow"

# The trigger mapping must contain one structural key, not merely avoid a
# blacklist that will become incomplete when GitHub adds another event.
trigger_block="$(sed -n '/^on:$/,/^permissions:$/p' "$workflow")"
trigger_keys="$(printf '%s\n' "$trigger_block" | awk '/^  [^[:space:]#][^:]*:/ { print }')"
[ "$trigger_keys" = '  workflow_dispatch:' ] || fail "workflow_dispatch must be the only trigger"

for input in version commit confirmation resume_from; do
    require "^[[:space:]]{6}$input:$"
done
require 'cancel-in-progress:[[:space:]]*false'
require "if:[[:space:]]*github\.repository == 'ccancellieri/tellurion' && github\.ref == 'refs/heads/main'"
require 'needs:[[:space:]]*\[verify\]'
require 'environment:[[:space:]]*crates-io'
require 'GITHUB_SHA.*REQUESTED_COMMIT'
require 'CONFIRMATION.*publish \$REQUESTED_VERSION from \$REQUESTED_COMMIT'
require 'verify-crates-io-release\.sh "\$REQUESTED_VERSION" "\$REQUESTED_COMMIT"'
require 'verify-canonical-origin\.sh "\$REQUESTED_VERSION" "\$REQUESTED_COMMIT"'
origin_binding_count="$(rg -c 'verify-canonical-origin\.sh "\$REQUESTED_VERSION" "\$REQUESTED_COMMIT"' "$workflow" || true)"
[ "$origin_binding_count" -eq 2 ] || fail "both workflow jobs must verify canonical origin"
ci_binding_count="$(rg -c 'verify-canonical-ci\.sh "\$REQUESTED_COMMIT"' "$workflow" || true)"
[ "$ci_binding_count" -eq 2 ] || fail "both workflow jobs must verify canonical CI before publication"
actions_read_count="$(rg -c '^[[:space:]]+actions:[[:space:]]*read$' "$workflow" || true)"
[ "$actions_read_count" -eq 2 ] || fail "only verification and publication jobs may read Actions state"
require 'audit-license-policy\.sh'
require 'audit-publication-license\.sh'
require 'audit-crates-io-policy\.sh'
require 'test-license-policy\.sh'
require 'test-crates-io-policy\.sh'
require 'test-crates-io-publish-workflow\.sh'
require 'test-publish-crates-io\.sh'
require 'test-crates-io-release-bindings\.sh'
require 'test-verify-crates-io-release\.sh'
if rg -q 'cargo \+1\.97\.1 test --workspace' "$workflow"; then
    fail "publication workflow must rely on the exact successful canonical CI run"
fi
require '^[[:space:]]*\./scripts/publish-crates-io\.sh[[:space:]]*\\$'
require '--preflight'
require '--execute'
require '--registry crates-io'
require 'CARGO_REGISTRY_TOKEN:[[:space:]]*\$\{\{ steps\.auth\.outputs\.token \}\}'
if rg -q 'secrets\.|CARGO_REGISTRY_TOKEN:[[:space:]]*[^[:space:]$]' "$workflow"; then
    fail "long-lived or literal registry credential is forbidden"
fi

auth_count="$(rg -c 'rust-lang/crates-io-auth-action@c6f97d42243bad5fab37ca0427f495c86d5b1a18[[:space:]]+# v1' "$workflow" || true)"
[ "$auth_count" -eq 1 ] || fail "crates.io auth action must appear once at its approved commit"
last_ci_line="$(awk '/verify-canonical-ci\.sh "\$REQUESTED_COMMIT"/ { line = NR } END { print line }' "$workflow")"
auth_line="$(awk '/rust-lang\/crates-io-auth-action@/ { print NR; exit }' "$workflow")"
[ "$last_ci_line" -lt "$auth_line" ] || fail "canonical CI must be revalidated before requesting a registry token"
publish_count="$(rg -c '^[[:space:]]*\./scripts/publish-crates-io\.sh[[:space:]]*\\$' "$workflow" || true)"
[ "$publish_count" -eq 2 ] || fail "only the guarded preflight and execute scripts are allowed"
if rg -n 'cargo[[:space:]]+(\+[^[:space:]]+[[:space:]]+)?publish\b' "$workflow"; then
    fail "direct cargo publish is forbidden; use the guarded publisher"
fi
if rg -q -- '--bootstrap' "$workflow"; then
    fail "first-publication bootstrap is local-only"
fi
if rg -n '(git[[:space:]]+push|gh[[:space:]]+release|docker[[:space:]].*push)' "$workflow"; then
    fail "publication workflow must not push tags, releases, or containers"
fi

python3 scripts/check-workflow-permissions.py --publish-workflow "$workflow" \
    || fail "publish workflow permissions are not least privilege"

echo "crates.io publish workflow contract passed"
