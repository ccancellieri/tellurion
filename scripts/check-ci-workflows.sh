#!/usr/bin/env bash
# Enforces one PR/main/manual full gate and one release gate.

set -euo pipefail

# shellcheck source=scripts/rg-compat.sh
. "$(dirname "$0")/rg-compat.sh"

workflow_dir="${WORKFLOW_DIR:-.github/workflows}"
ci_workflow="$workflow_dir/ci.yml"
release_workflow="$workflow_dir/release-artifacts.yml"
local_mirror="${CI_LOCAL_SCRIPT:-scripts/ci-local.sh}"

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

require_match() {
    local pattern="$1"
    local file="$2"
    rg -q -- "$pattern" "$file" || fail "missing required workflow behavior: $pattern"
}

[ -d "$workflow_dir" ] || fail "missing workflow directory: $workflow_dir"
[ -f "$ci_workflow" ] || fail "missing CI workflow: $ci_workflow"
[ -f "$release_workflow" ] || fail "missing release workflow: $release_workflow"
[ -f "$local_mirror" ] || fail "missing local CI mirror: $local_mirror"

# CI runs for pull requests targeting main, main pushes, and explicit manual
# dispatch. The release workflow retains its semver-tag/manual contract.
workflow_names="$(
    find "$workflow_dir" -type f \( -name '*.yml' -o -name '*.yaml' \) -print \
        | sed 's#^.*/##' \
        | sort
)"
expected_workflow_names="$(printf '%s\n' ci.yml release-artifacts.yml | sort)"
if [ "$workflow_names" != "$expected_workflow_names" ]; then
    fail "hosted workflows must be exactly ci.yml and release-artifacts.yml"
fi

python3 scripts/check-workflow-permissions.py \
    --read-only-workflow "$ci_workflow" \
    || fail "CI workflow permissions must be an exact read-only mapping"

require_match '^permissions:$' "$ci_workflow"
require_match '^  contents: read$' "$ci_workflow"
require_match '^concurrency:$' "$ci_workflow"
require_match '^  group: \$\{\{ github.workflow \}\}-\$\{\{ github.event.pull_request.number \|\| github.ref \}\}$' "$ci_workflow"
require_match '^  cancel-in-progress: true$' "$ci_workflow"
require_match '^concurrency:$' "$release_workflow"
require_match '^  group: \$\{\{ github.workflow \}\}-\$\{\{ github.ref \}\}$' "$release_workflow"
require_match '^  cancel-in-progress: true$' "$release_workflow"

ci_triggers="$(awk '
    $0 == "on:" { in_on = 1; next }
    in_on && $0 ~ /^[^[:space:]]/ { exit }
    in_on && $0 ~ /^  [^[:space:]]/ { print }
' "$ci_workflow")"
trigger_count="$(printf '%s\n' "$ci_triggers" | sed '/^$/d' | wc -l | tr -d ' ')"
push_count="$(printf '%s\n' "$ci_triggers" | rg -cx '  push:' || true)"
pull_request_count="$(printf '%s\n' "$ci_triggers" | rg -cx '  pull_request:' || true)"
dispatch_count="$(printf '%s\n' "$ci_triggers" | rg -cx '  workflow_dispatch:' || true)"
if [ "$trigger_count" -ne 3 ] || [ "$push_count" -ne 1 ] \
    || [ "$pull_request_count" -ne 1 ] || [ "$dispatch_count" -ne 1 ]; then
    fail "CI triggers must be exactly push, pull_request, and workflow_dispatch"
fi

main_scope_count="$(rg -c '^    branches: \[main\]$' "$ci_workflow" || true)"
if [ "$main_scope_count" -ne 2 ]; then
    fail "CI push and pull_request triggers must each target only main"
fi
if rg -q '^    tags:' "$ci_workflow" || rg -q '^  schedule:' "$ci_workflow"; then
    fail "CI must not run for tags or on a schedule"
fi

# A single stable aggregate check is the only check branch protection and
# deployments need to follow. It deliberately runs even if a dependency
# failed, then turns every non-successful substantive job into a red gate.
ci_gate_block="$(awk '
    $0 == "  ci-gate:" { capture = 1 }
    capture && $0 ~ /^  [^[:space:]]/ && $0 != "  ci-gate:" { exit }
    capture { print }
' "$ci_workflow")"
if [ -z "$ci_gate_block" ]; then
    fail "CI must define the ci-gate aggregate job"
fi

printf '%s\n' "$ci_gate_block" | rg -qx '    name: CI gate' \
    || fail "ci-gate must expose the stable CI gate check name"
printf '%s\n' "$ci_gate_block" | rg -qx '    if: always\(\)' \
    || fail "ci-gate must run after failed dependencies"

ci_gate_needs="$(printf '%s\n' "$ci_gate_block" | awk '
    $0 == "    needs:" { capture = 1; next }
    capture && $0 ~ /^      - / {
        job = $0
        sub(/^      - /, "", job)
        print job
        next
    }
    capture { exit }
')"
expected_ci_gate_needs="$(printf '%s\n' \
    fmt \
    clippy \
    test \
    smoke \
    ui-test \
    feature-matrix \
    deploy-manifests \
    artifact-audit)"
if [ "$ci_gate_needs" != "$expected_ci_gate_needs" ]; then
    fail "ci-gate must depend on every substantive CI job"
fi

for ci_job in $expected_ci_gate_needs; do
    printf '%s\n' "$ci_gate_block" | rg -q "needs\\.${ci_job}\\.result" \
        || fail "ci-gate must fail when $ci_job does not succeed"
done

hosted_feature_legs="$(awk '
    $0 == "        include:" { in_matrix = 1; next }
    in_matrix && $0 == "    steps:" { exit }
    in_matrix && $0 ~ /^          - name: / {
        name = $0
        sub(/^          - name: /, "", name)
        next
    }
    in_matrix && $0 ~ /^            flags: / {
        flags = $0
        sub(/^            flags: /, "", flags)
        print name ":" flags
    }
' "$ci_workflow")"

local_feature_legs="$(awk '
    $0 == "FEATURE_LEGS=(" { in_matrix = 1; next }
    in_matrix && $0 == ")" { exit }
    in_matrix {
        line = $0
        sub(/^[[:space:]]*"/, "", line)
        sub(/"[[:space:]]*$/, "", line)
        if (line ~ /^[^:]+:/) print line
    }
' "$local_mirror")"

if [ "$hosted_feature_legs" != "$local_feature_legs" ]; then
    fail "hosted and local feature-matrix legs must match exactly"
fi

public_demo_count="$(printf '%s\n' "$hosted_feature_legs" | rg -cx 'public-demo-ui:--no-default-features --features public-demo,ui' || true)"
if [ "$public_demo_count" -ne 1 ]; then
    fail "feature matrix must contain exactly one minimal public-demo-ui leg"
fi

public_demo_block="$(awk '
    $0 == "          - name: public-demo-ui" { capture = 1 }
    capture && $0 ~ /^          - name: / && $0 != "          - name: public-demo-ui" { exit }
    capture && $0 == "    steps:" { exit }
    capture { print }
' "$ci_workflow")"
printf '%s\n' "$public_demo_block" | rg -qx '            build_ui: public-demo' \
    || fail "public-demo-ui feature leg must build the dedicated public-demo UI bundle"
require_match 'npm run build:public-demo' "$ci_workflow"

hosted_ui_test_count="$(rg -c '^[[:space:]]+- run: cd ui && npm ci && npm test$' "$ci_workflow" || true)"
local_ui_test_count="$(rg -c '^[[:space:]]+\(cd ui && npm ci && npm test\)$' "$local_mirror" || true)"
hosted_ui_test_count="${hosted_ui_test_count:-0}"
local_ui_test_count="${local_ui_test_count:-0}"
if [ "$hosted_ui_test_count" -ne 1 ] || [ "$local_ui_test_count" -ne 1 ]; then
    fail "hosted CI workflow and local full mirror must each run npm test exactly once"
fi

hosted_package_ui_count="$(rg -c 'TELLURION_UI_OPERATOR_READY=1 \./scripts/tests/test_cargo_package_ui\.sh' "$ci_workflow" || true)"
local_package_ui_count="$(rg -c 'TELLURION_UI_OPERATOR_READY=1 \./scripts/tests/test_cargo_package_ui\.sh' "$local_mirror" || true)"
hosted_package_ui_count="${hosted_package_ui_count:-0}"
local_package_ui_count="${local_package_ui_count:-0}"
if [ "$hosted_package_ui_count" -ne 1 ] || [ "$local_package_ui_count" -ne 1 ]; then
    fail "hosted CI and its local mirror must each run the packaged UI boundary check once"
fi

publication_audit_count="$(rg -c '^[[:space:]]*\./scripts/audit-publication-license\.sh$' "$ci_workflow" || true)"
publication_audit_count="${publication_audit_count:-0}"
if [ "$publication_audit_count" -ne 1 ]; then
    fail "hosted CI must run the publication licence audit exactly once"
fi

local_publication_audit_count="$(rg -c '^[[:space:]]*\./scripts/audit-publication-license\.sh[[:space:]]*&&$' "$local_mirror" || true)"
local_publication_audit_count="${local_publication_audit_count:-0}"
if [ "$local_publication_audit_count" -ne 1 ]; then
    fail "local full mirror must run the publication licence audit exactly once"
fi

echo "CI workflow topology passed"
