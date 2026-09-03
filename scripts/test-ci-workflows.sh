#!/usr/bin/env bash
# Mutation tests for the PR/main/manual CI topology and its local mirror.

set -euo pipefail

fixture_root="$(mktemp -d)"
trap 'rm -rf "$fixture_root"' EXIT

failures=0

make_public_ci_fixture() {
    local fixture="$1"

    perl -0pi -e 's#on:\n  schedule:\n    - cron: "17 3 \* \* \*"\n  workflow_dispatch:\n\npermissions:\n  contents: read\n\nconcurrency:\n  group: tellurion-full-verification\n  cancel-in-progress: true#on:\n  push:\n    branches: [main]\n  pull_request:\n    branches: [main]\n  workflow_dispatch:\n\npermissions:\n  contents: read\n\nconcurrency:\n  group: \$\{\{ github.workflow \}\}-\$\{\{ github.event.pull_request.number || github.ref \}\}\n  cancel-in-progress: true#' "$fixture/workflows/ci.yml"
    perl -0pi -e 's#permissions:\n  contents: read\n\njobs:#permissions:\n  contents: read\n\nconcurrency:\n  group: \$\{\{ github.workflow \}\}-\$\{\{ github.ref \}\}\n  cancel-in-progress: true\n\njobs:#' "$fixture/workflows/release-artifacts.yml"
}

expect_accepted() {
    local fixture="$fixture_root/public-ci"
    mkdir -p "$fixture"
    cp -R .github/workflows "$fixture/workflows"
    cp scripts/ci-local.sh "$fixture/ci-local.sh"
    make_public_ci_fixture "$fixture"

    if ! WORKFLOW_DIR="$fixture/workflows" CI_LOCAL_SCRIPT="$fixture/ci-local.sh" \
        bash scripts/check-ci-workflows.sh >/dev/null 2>&1; then
        echo "FAIL: public PR/main/manual CI topology was rejected" >&2
        failures=$((failures + 1))
    else
        echo "ok: public PR/main/manual CI topology accepted"
    fi
}

expect_rejected() {
    local name="$1"
    local fixture="$fixture_root/$name"
    mkdir -p "$fixture"
    cp -R .github/workflows "$fixture/workflows"
    cp scripts/ci-local.sh "$fixture/ci-local.sh"
    make_public_ci_fixture "$fixture"

    case "$name" in
        unexpected-pr-workflow)
            printf 'name: PR checks\non:\n  pull_request:\n' > "$fixture/workflows/pr-checks.yml"
            ;;
        unexpected-push-workflow)
            printf 'name: Push checks\non:\n  push:\n' > "$fixture/workflows/push-checks.yml"
            ;;
        missing-release-workflow)
            rm "$fixture/workflows/release-artifacts.yml"
            ;;
        missing-publish-workflow)
            rm "$fixture/workflows/publish-crates.yml"
            ;;
        ci-has-schedule)
            perl -0pi -e 's/(  workflow_dispatch:\n)/$1  schedule:\n    - cron: "17 3 * * *"\n/' "$fixture/workflows/ci.yml"
            ;;
        ci-has-unexpected-read-scope)
            perl -0pi -e 's/(permissions:\n  contents: read\n)/$1  security-events: read\n/' "$fixture/workflows/ci.yml"
            ;;
        ci-job-permission-override)
            perl -0pi -e 's/(  fmt:\n)/$1    permissions:\n      contents: read\n/' "$fixture/workflows/ci.yml"
            ;;
        ci-has-tag-trigger)
            perl -0pi -e 's/(    branches: \[main\]\n)/$1    tags: ["v*"]\n/' "$fixture/workflows/ci.yml"
            ;;
        ci-push-missing-main-scope)
            perl -0pi -e 's/  push:\n    branches: \[main\]/  push:\n    branches: [release]/' "$fixture/workflows/ci.yml"
            ;;
        ci-pr-missing-main-scope)
            perl -0pi -e 's/  pull_request:\n    branches: \[main\]/  pull_request:\n    branches: [release]/' "$fixture/workflows/ci.yml"
            ;;
        ci-has-unscoped-push)
            perl -0pi -e 's/  push:\n    branches: \[main\]/  push: {}/' "$fixture/workflows/ci.yml"
            ;;
        ci-has-unscoped-pr)
            perl -0pi -e 's/  pull_request:\n    branches: \[main\]/  pull_request: {}/' "$fixture/workflows/ci.yml"
            ;;
        ci-has-extra-trigger)
            perl -0pi -e 's/(  workflow_dispatch:\n)/$1  workflow_call:\n/' "$fixture/workflows/ci.yml"
            ;;
        missing-ci-concurrency)
            perl -0pi -e 's/\nconcurrency:\n  group: \$\{\{ github\.workflow \}\}-\$\{\{ github\.event\.pull_request\.number \|\| github\.ref \}\}\n  cancel-in-progress: true\n//' "$fixture/workflows/ci.yml"
            ;;
        ci-has-global-concurrency)
            perl -0pi -e 's/group: \$\{\{ github\.workflow \}\}-\$\{\{ github\.event\.pull_request\.number \|\| github\.ref \}\}/group: tellurion-full-verification/' "$fixture/workflows/ci.yml"
            ;;
        missing-release-concurrency)
            perl -0pi -e 's/\nconcurrency:\n  group: \$\{\{ github\.workflow \}\}-\$\{\{ github\.ref \}\}\n  cancel-in-progress: true\n//' "$fixture/workflows/release-artifacts.yml"
            ;;
        release-has-global-concurrency)
            perl -0pi -e 's/group: \$\{\{ github\.workflow \}\}-\$\{\{ github\.ref \}\}/group: release-artifacts/' "$fixture/workflows/release-artifacts.yml"
            ;;
        missing-public-demo-ui-leg)
            perl -0pi -e 's#\n          - name: public-demo-ui\n            flags: --no-default-features --features public-demo,ui##' "$fixture/workflows/ci.yml"
            ;;
        hosted-builds-tracked-ui)
            perl -0pi -e 's#      - name: Verify packaged UI boundary#      - run: cd ui \&\& npm run build\n      - name: Verify packaged UI boundary#' "$fixture/workflows/ci.yml"
            ;;
        local-builds-tracked-ui)
            perl -0pi -e 's#            \./scripts/tests/test_cargo_package_ui\.sh \|\| \{#            (cd ui \&\& npm run build)\n            ./scripts/tests/test_cargo_package_ui.sh || {#' "$fixture/ci-local.sh"
            ;;
        missing-local-public-demo-ui-leg)
            perl -ni -e 'print unless /"public-demo-ui:--no-default-features --features public-demo,ui"/' "$fixture/ci-local.sh"
            ;;
        missing-hosted-ui-test)
            perl -0pi -e 's/run: cd ui && npm ci && npm test/run: cd ui && npm ci/' "$fixture/workflows/ci.yml"
            ;;
        duplicate-hosted-ui-test)
            perl -0pi -e 's/(run: cd ui && npm ci && npm test)/$1\n      - run: cd ui && npm ci && npm test/' "$fixture/workflows/ci.yml"
            ;;
        missing-local-ui-test)
            perl -0pi -e 's/\n[[:space:]]*\(cd ui && npm ci && npm test\)//' "$fixture/ci-local.sh"
            ;;
        missing-hosted-package-ui-check)
            perl -0pi -e 's#\n      - name: Verify packaged UI boundary\n        if: matrix\.name == '\''ui'\''\n        run: \./scripts/tests/test_cargo_package_ui\.sh##' "$fixture/workflows/ci.yml"
            ;;
        missing-local-package-ui-check)
            perl -0pi -e 's#\n[[:space:]]*\./scripts/tests/test_cargo_package_ui\.sh \|\| \{\n[[:space:]]*printf '\''  FAIL feature leg %s \(packaged UI boundary\)\\n'\'' "\$name"\n[[:space:]]*failed=1\n[[:space:]]*continue\n[[:space:]]*\}##' "$fixture/ci-local.sh"
            ;;
        missing-publication-license-audit)
            perl -0pi -e 's#^[[:space:]]*\./scripts/audit-publication-license\.sh\n##m' "$fixture/workflows/ci.yml"
            ;;
        missing-local-publication-license-audit)
            perl -0pi -e 's#^[[:space:]]*\./scripts/audit-publication-license\.sh[[:space:]]*&&\n##m' "$fixture/ci-local.sh"
            ;;
        missing-ci-gate)
            perl -0pi -e 's#\n  ci-gate:\n.*\z#\n#s' "$fixture/workflows/ci.yml"
            ;;
        ci-gate-missing-*)
            ci_job="${name#ci-gate-missing-}"
            perl -0pi -e "s#\\n      - \\Q$ci_job\\E##" "$fixture/workflows/ci.yml"
            ;;
        *)
            echo "unknown mutation: $name" >&2
            exit 2
            ;;
    esac

    if diff -r -q .github/workflows "$fixture/workflows" >/dev/null 2>&1 \
        && cmp -s scripts/ci-local.sh "$fixture/ci-local.sh"; then
        echo "FAIL: $name mutation left the workflow contract unchanged" >&2
        failures=$((failures + 1))
        return
    fi

    if WORKFLOW_DIR="$fixture/workflows" CI_LOCAL_SCRIPT="$fixture/ci-local.sh" \
        bash scripts/check-ci-workflows.sh >/dev/null 2>&1; then
        echo "FAIL: $name mutation was accepted" >&2
        failures=$((failures + 1))
    else
        echo "ok: $name mutation rejected"
    fi
}

expect_accepted

for mutation in \
    unexpected-pr-workflow \
    unexpected-push-workflow \
    missing-release-workflow \
    missing-publish-workflow \
    ci-has-schedule \
    ci-has-unexpected-read-scope \
    ci-job-permission-override \
    ci-has-tag-trigger \
    ci-push-missing-main-scope \
    ci-pr-missing-main-scope \
    ci-has-unscoped-push \
    ci-has-unscoped-pr \
    ci-has-extra-trigger \
    missing-ci-concurrency \
    ci-has-global-concurrency \
    missing-release-concurrency \
    release-has-global-concurrency \
    missing-public-demo-ui-leg \
    hosted-builds-tracked-ui \
    local-builds-tracked-ui \
    missing-local-public-demo-ui-leg \
    missing-hosted-ui-test \
    duplicate-hosted-ui-test \
    missing-local-ui-test \
    missing-hosted-package-ui-check \
    missing-local-package-ui-check \
    missing-publication-license-audit \
    missing-local-publication-license-audit \
    missing-ci-gate \
    ci-gate-missing-fmt \
    ci-gate-missing-clippy \
    ci-gate-missing-test \
    ci-gate-missing-smoke \
    ci-gate-missing-ui-test \
    ci-gate-missing-feature-matrix \
    ci-gate-missing-deploy-manifests \
    ci-gate-missing-artifact-audit; do
    expect_rejected "$mutation"
done

if ! bash scripts/check-ci-workflows.sh >/dev/null; then
    echo "FAIL: the repository's own workflows do not satisfy the CI topology" >&2
    failures=$((failures + 1))
else
    echo "ok: unmutated workflows accepted"
fi

no_rg_path="$fixture_root/no-rg-bin"
mkdir -p "$no_rg_path"
for tool in bash dirname grep find sed sort python3 awk wc tr; do
    tool_path="$(command -v "$tool")"
    ln -s "$tool_path" "$no_rg_path/$tool"
done
if ! PATH="$no_rg_path" "$no_rg_path/bash" scripts/check-ci-workflows.sh >/dev/null; then
    echo "FAIL: CI topology check requires ripgrep on the runner" >&2
    failures=$((failures + 1))
else
    echo "ok: unmutated workflows accepted without ripgrep"
fi

if ! sh scripts/tests/test_demo_smoke_process_detection.sh; then
    echo "FAIL: demo smoke process preflight regression" >&2
    failures=$((failures + 1))
fi

if ! sh scripts/tests/test_demo_smoke_log_level.sh; then
    echo "FAIL: demo smoke log-level regression" >&2
    failures=$((failures + 1))
fi

if ! sh scripts/tests/test_demo_smoke_durable_auth.sh; then
    echo "FAIL: demo smoke durable-auth regression" >&2
    failures=$((failures + 1))
fi

if ! sh scripts/tests/test_demo_smoke_png_signature.sh; then
    echo "FAIL: demo smoke PNG-signature regression" >&2
    failures=$((failures + 1))
fi

if ! sh scripts/tests/test_demo_smoke_manifest_count.sh; then
    echo "FAIL: demo smoke manifest-count regression" >&2
    failures=$((failures + 1))
fi

if ! sh scripts/tests/test_ci_local_data_inputs.sh; then
    echo "FAIL: CI local data-input classification regression" >&2
    failures=$((failures + 1))
fi

if ! bash scripts/tests/test_cargo_package_ui_currentness.sh; then
    echo "FAIL: packaged UI currentness regression" >&2
    failures=$((failures + 1))
fi

if [ "$failures" -ne 0 ]; then
    exit 1
fi

echo "CI workflow mutation tests passed"
