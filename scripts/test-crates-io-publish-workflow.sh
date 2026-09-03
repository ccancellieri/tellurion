#!/usr/bin/env bash
# Mutation tests for the only workflow permitted to upload crates to crates.io.

set -euo pipefail

fixture_root="$(mktemp -d)"
trap 'rm -rf "$fixture_root"' EXIT
failures=0

expect_rejected() {
    local name="$1"
    local fixture="$fixture_root/$name"
    mkdir -p "$fixture/workflows"
    cp .github/workflows/publish-crates.yml "$fixture/workflows/publish-crates.yml"

    case "$name" in
        automatic-trigger)
            perl -0pi -e 's/^  workflow_dispatch:/  push:\n    branches: [main]\n  workflow_dispatch:/m' "$fixture/workflows/publish-crates.yml"
            ;;
        pull-request-target-trigger)
            perl -0pi -e 's/^  workflow_dispatch:/  pull_request_target:\n  workflow_dispatch:/m' "$fixture/workflows/publish-crates.yml"
            ;;
        missing-environment)
            perl -0pi -e 's/^    environment: crates-io\n//m' "$fixture/workflows/publish-crates.yml"
            ;;
        broad-permissions)
            perl -0pi -e 's/^permissions:\n  contents: read$/permissions: write-all/m' "$fixture/workflows/publish-crates.yml"
            ;;
        missing-oidc)
            perl -0pi -e 's/^      id-token: write\n//m' "$fixture/workflows/publish-crates.yml"
            ;;
        missing-actions-read)
            perl -0pi -e 's/^      actions: read\n//mg' "$fixture/workflows/publish-crates.yml"
            ;;
        token-secret)
            perl -0pi -e 's/\$\{\{ steps\.auth\.outputs\.token \}\}/\$\{\{ secrets.CARGO_REGISTRY_TOKEN \}\}/' "$fixture/workflows/publish-crates.yml"
            ;;
        mutable-auth-action)
            perl -0pi -e 's/rust-lang\/crates-io-auth-action\@[0-9a-f]{40}/rust-lang\/crates-io-auth-action\@v1/' "$fixture/workflows/publish-crates.yml"
            ;;
        missing-version-input)
            perl -0pi -e 's/^      version:\n.*?(?=^      commit:)//ms' "$fixture/workflows/publish-crates.yml"
            ;;
        missing-commit-input)
            perl -0pi -e 's/^      commit:\n.*?(?=^      confirmation:)//ms' "$fixture/workflows/publish-crates.yml"
            ;;
        missing-confirmation)
            perl -0pi -e 's/^      confirmation:\n.*?(?=^      resume_from:)//ms' "$fixture/workflows/publish-crates.yml"
            ;;
        unchecked-head)
            perl -0pi -e 's/^.*GITHUB_SHA.*REQUESTED_COMMIT.*\n//mg' "$fixture/workflows/publish-crates.yml"
            ;;
        missing-tag-binding)
            perl -0pi -e 's/^.*verify-crates-io-release\.sh.*\n//mg' "$fixture/workflows/publish-crates.yml"
            ;;
        missing-ci-binding)
            perl -0pi -e 's/^.*verify-canonical-ci\.sh.*\n//mg' "$fixture/workflows/publish-crates.yml"
            ;;
        publish-before-gates)
            perl -0pi -e 's/^    needs: \[verify\]\n//m' "$fixture/workflows/publish-crates.yml"
            ;;
        cancelling-publication)
            perl -0pi -e 's/cancel-in-progress: false/cancel-in-progress: true/' "$fixture/workflows/publish-crates.yml"
            ;;
        unapproved-publish-command)
            printf '\n      - run: cargo +1.97.1 publish -p tellurion-core\n' >> "$fixture/workflows/publish-crates.yml"
            ;;
        alternate-registry)
            perl -0pi -e 's/--registry crates-io/--registry private/' "$fixture/workflows/publish-crates.yml"
            ;;
        no-resume-input)
            perl -0pi -e 's/^      resume_from:\n.*?(?=^permissions:)//ms' "$fixture/workflows/publish-crates.yml"
            ;;
        *)
            echo "unknown mutation: $name" >&2
            exit 2
            ;;
    esac

    if PUBLISH_WORKFLOW="$fixture/workflows/publish-crates.yml" \
        bash scripts/check-crates-io-publish-workflow.sh >/dev/null 2>&1; then
        echo "FAIL: $name mutation was accepted" >&2
        failures=$((failures + 1))
    else
        echo "ok: $name mutation rejected"
    fi
}

if ! bash scripts/check-crates-io-publish-workflow.sh; then
    echo "FAIL: canonical crates.io publish workflow was rejected" >&2
    failures=$((failures + 1))
fi

if ! bash scripts/check-crates-io-publisher.sh; then
    echo "FAIL: canonical ordered publisher was rejected" >&2
    failures=$((failures + 1))
fi

publisher_fixture="$fixture_root/publisher"
cp scripts/publish-crates-io.sh "$publisher_fixture"
perl -0pi -e 's/cmp -s "\$archive" "\$remote"/true/g' "$publisher_fixture"
if CRATES_IO_PUBLISHER="$publisher_fixture" bash scripts/check-crates-io-publisher.sh >/dev/null 2>&1; then
    echo "FAIL: publisher without byte-identity verification was accepted" >&2
    failures=$((failures + 1))
else
    echo "ok: publisher without byte-identity verification rejected"
fi

cp scripts/publish-crates-io.sh "$publisher_fixture"
perl -0pi -e 's/cargo \+1\.97\.1 package --workspace --locked --no-verify/cargo +1.97.1 package --locked --no-verify -p "$package"/' "$publisher_fixture"
if CRATES_IO_PUBLISHER="$publisher_fixture" bash scripts/check-crates-io-publisher.sh >/dev/null 2>&1; then
    echo "FAIL: publisher without one workspace package preflight was accepted" >&2
    failures=$((failures + 1))
else
    echo "ok: publisher without one workspace package preflight rejected"
fi

cp scripts/publish-crates-io.sh "$publisher_fixture"
perl -0pi -e 's#^\./scripts/check-crates-io-release-readiness\.sh\n##m' "$publisher_fixture"
if CRATES_IO_PUBLISHER="$publisher_fixture" bash scripts/check-crates-io-publisher.sh >/dev/null 2>&1; then
    echo "FAIL: publisher without source readiness gate was accepted" >&2
    failures=$((failures + 1))
else
    echo "ok: publisher without source readiness gate rejected"
fi

readiness_fixture="$fixture_root/source-readiness"
cp scripts/check-crates-io-release-readiness.sh "$readiness_fixture"
perl -0pi -e 's#^\. .*rg-compat\.sh.*\n##m' "$readiness_fixture"
if CRATES_IO_RELEASE_READINESS="$readiness_fixture" bash scripts/check-crates-io-publisher.sh >/dev/null 2>&1; then
    echo "FAIL: source readiness gate without portable grep support was accepted" >&2
    failures=$((failures + 1))
else
    echo "ok: source readiness gate without portable grep support rejected"
fi

cp scripts/publish-crates-io.sh "$publisher_fixture"
perl -0pi -e 's#/owners#/contributors#' "$publisher_fixture"
if CRATES_IO_PUBLISHER="$publisher_fixture" bash scripts/check-crates-io-publisher.sh >/dev/null 2>&1; then
    echo "FAIL: publisher without the crates.io ownership preflight was accepted" >&2
    failures=$((failures + 1))
else
    echo "ok: publisher without the crates.io ownership preflight rejected"
fi

for mutation in \
    automatic-trigger pull-request-target-trigger missing-environment broad-permissions missing-oidc \
    missing-actions-read \
    token-secret mutable-auth-action missing-version-input missing-commit-input \
    missing-confirmation unchecked-head missing-tag-binding missing-ci-binding publish-before-gates \
    cancelling-publication unapproved-publish-command alternate-registry \
    no-resume-input; do
    expect_rejected "$mutation"
done

if [ "$failures" -ne 0 ]; then
    echo "crates.io publish workflow mutation tests FAILED: $failures failure(s)" >&2
    exit 1
fi

echo "crates.io publish workflow mutation tests passed"
