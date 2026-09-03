#!/usr/bin/env bash
# Mutation tests for the workflow contract. Each fixture represents a release
# boundary regression that must be rejected before CI can accept the workflow.
#
# Two families live here. The `*-publication` family proves the guard still
# refuses any way to publish an asset, image, crate, tag or GitHub Release --
# that prohibition is the point of the contract and predates the versioning
# mechanism. The version family proves the guard enforces the *invariant* that
# archive identities derive from `[workspace.package] version`, now that it no
# longer pins one frozen literal.

set -euo pipefail

# shellcheck source=scripts/rg-compat.sh
. "$(dirname "$0")/rg-compat.sh"
# shellcheck source=scripts/workspace-version.sh
. "$(dirname "$0")/workspace-version.sh"

fixture_root="$(mktemp -d)"
trap 'rm -rf "$fixture_root"' EXIT

failures=0

expect_valid_version() {
    local version="$1"
    if ! is_semver "$version"; then
        echo "FAIL: expected version to be accepted: $version" >&2
        failures=$((failures + 1))
    fi
}

expect_invalid_version() {
    local version="$1"
    if is_semver "$version"; then
        echo "FAIL: expected version to be rejected: $version" >&2
        failures=$((failures + 1))
    fi
}

expect_forward_transition() {
    local current="$1"
    local target="$2"
    if ! is_forward_release_version "$current" "$target"; then
        echo "FAIL: expected forward release transition: $current -> $target" >&2
        failures=$((failures + 1))
    fi
}

expect_rejected_transition() {
    local current="$1"
    local target="$2"
    if is_forward_release_version "$current" "$target"; then
        echo "FAIL: expected release transition to be rejected: $current -> $target" >&2
        failures=$((failures + 1))
    fi
}

# Release candidates are intentionally narrow: only `-rc.N` is accepted, and
# their ordering must make the promotion path forward-only.
expect_valid_version '0.4.0'
expect_valid_version '0.5.0-rc.0'
expect_valid_version '0.5.0-rc.1'
for malformed_version in \
    '0.5.0-rc' \
    '0.5.0-rc.01' \
    '00.5.0-rc.1' \
    '0.5.0-rc.1+build.1' \
    '0.5.0-beta.1' \
    'v0.5.0-rc.1'; do
    expect_invalid_version "$malformed_version"
done

expect_forward_transition '0.4.0' '0.5.0-rc.1'
expect_forward_transition '0.5.0-rc.1' '0.5.0-rc.2'
expect_forward_transition '0.5.0-rc.2' '0.5.0'
expect_rejected_transition '0.5.0-rc.2' '0.5.0-rc.1'
expect_rejected_transition '0.5.0' '0.5.0-rc.1'
expect_rejected_transition '0.5.0' '0.4.9'

if ! rg -Fq 'is_forward_release_version "$current_version" "$target_version"' scripts/release.sh; then
    echo "FAIL: release script does not use release-candidate ordering" >&2
    failures=$((failures + 1))
fi

if ! rg -Fq 'tags: ["v[0-9]+.[0-9]+.[0-9]+", "v[0-9]+.[0-9]+.[0-9]+-rc.[0-9]+"]' \
    .github/workflows/release-artifacts.yml; then
    echo "FAIL: release workflow does not accept the canonical release-candidate tag pattern" >&2
    failures=$((failures + 1))
fi

if ! rg -Fq '$versionPattern = "$number\.$number\.$number(?:-rc\.$number)?"' \
    .github/workflows/release-artifacts.yml; then
    echo "FAIL: release workflow does not resolve release-candidate workspace versions" >&2
    failures=$((failures + 1))
fi

expect_rejected() {
    local name="$1"
    local fixture="$fixture_root/$name"
    local expected_message=""
    mkdir -p "$fixture"
    cp -R .github/workflows "$fixture/workflows"
    cp .github/CODEOWNERS "$fixture/CODEOWNERS"

    case "$name" in
        windows-shell)
            perl -0pi -e 's#(- name: Build default-feature binaries\n)#$1        shell: pwsh\n        run: |\n          target=windows-target\n#' "$fixture/workflows/release-artifacts.yml"
            ;;
        missing-native-release-gate)
            perl -0pi -e 's#\n      - name: Gate prebuilt native binary release\n        run: \./scripts/check-native-binary-release-readiness\.sh\n##' "$fixture/workflows/release-artifacts.yml"
            ;;
        smoke-directory)
            perl -0pi -e 's#\n          New-Item -ItemType Directory -Force -Path \$smoke_dir \| Out-Null##' "$fixture/workflows/release-artifacts.yml"
            ;;
        unsafe-ref-name)
            printf '%s\n' '      - name: unsafe-${{ github.ref_name }}' >> "$fixture/workflows/release-artifacts.yml"
            ;;
        # A ref name may contain `/`. Naming an identity `version=` used to be
        # enough to slip past the guard's exclusion; it must not be.
        ref-name-as-version)
            perl -0pi -e 's#\$package_name = "tellurion-v\$version-\$target"#$package_name = "tellurion-version=$env:GITHUB_REF_NAME-$target"#' "$fixture/workflows/release-artifacts.yml"
            ;;
        release-api)
            printf '\n      - run: gh api repos/example/project/releases\n' >> "$fixture/workflows/ci.yml"
            ;;
        gh-release-publication)
            printf '\n      - run: gh release create v9.9.9 dist/tellurion.tar.gz\n' >> "$fixture/workflows/ci.yml"
            ;;
        gh-release-action-publication)
            printf '\n      - uses: softprops/action-gh-release@v2\n' >> "$fixture/workflows/ci.yml"
            ;;
        contents-write-publication)
            printf '\npermissions:\n  contents: write\n' >> "$fixture/workflows/release-artifacts.yml"
            ;;
        cargo-publish-publication)
            printf '\n      - run: cargo +1.97.1 publish -p tellurion-core\n' >> "$fixture/workflows/release-artifacts.yml"
            ;;
        crates-publisher-outside-publish-workflow)
            printf '\n      - run: ./scripts/publish-crates-io.sh --execute\n' >> "$fixture/workflows/ci.yml"
            ;;
        crates-auth-outside-publish-workflow)
            printf '\n      - uses: rust-lang/crates-io-auth-action@c6f97d42243bad5fab37ca0427f495c86d5b1a18 # v1\n' >> "$fixture/workflows/ci.yml"
            ;;
        crates-token-outside-publish-workflow)
            printf '\n      - run: true\n        env:\n          CARGO_REGISTRY_TOKEN: literal\n' >> "$fixture/workflows/ci.yml"
            ;;
        image-push)
            printf '\n      - run: podman push registry.example/tellurion\n' >> "$fixture/workflows/ci.yml"
            ;;
        git-push)
            printf '\n      - run: git push origin v0.3.0\n' >> "$fixture/workflows/ci.yml"
            ;;
        ci-mutable-action-reference)
            perl -0pi -e 's#actions/checkout\@3d3c42e5aac5ba805825da76410c181273ba90b1#actions/checkout\@v7#' "$fixture/workflows/ci.yml"
            ;;
        release-mutable-action-reference)
            perl -0pi -e 's#actions/upload-artifact\@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a#actions/upload-artifact\@v7#' "$fixture/workflows/release-artifacts.yml"
            ;;
        ci-unknown-pinned-action)
            printf '\n      - uses: example/release-action@0123456789abcdef0123456789abcdef01234567 # v4\n' >> "$fixture/workflows/ci.yml"
            ;;
        release-unknown-pinned-action)
            printf '\n      - uses: example/release-action@0123456789abcdef0123456789abcdef01234567 # v4\n' >> "$fixture/workflows/release-artifacts.yml"
            ;;
        canonical-local-action)
            printf '\n      - uses: ./actions/local # v4\n' >> "$fixture/workflows/ci.yml"
            ;;
        canonical-docker-action)
            printf '\n      - uses: docker://alpine:3.20 # v4\n' >> "$fixture/workflows/ci.yml"
            ;;
        canonical-reusable-workflow)
            printf '\n  reusable-workflow:\n    uses: example/reusable/.github/workflows/reusable.yml@0123456789abcdef0123456789abcdef01234567 # v4\n' >> "$fixture/workflows/ci.yml"
            ;;
        quoted-uses-key)
            printf '\n      - "uses": actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1\n' >> "$fixture/workflows/ci.yml"
            ;;
        flow-uses-key)
            printf '\n      - { uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 }\n' >> "$fixture/workflows/ci.yml"
            ;;
        spaced-uses-key)
            printf '\n      - uses : actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1\n' >> "$fixture/workflows/ci.yml"
            ;;
        quoted-reusable-workflow-key)
            printf '\n  quoted-reusable-workflow:\n    "uses": example/reusable/.github/workflows/reusable.yml@0123456789abcdef0123456789abcdef01234567\n' >> "$fixture/workflows/ci.yml"
            ;;
        missing-action-version-comment)
            perl -0pi -e 's/ # v7\.0\.1//' "$fixture/workflows/ci.yml"
            ;;
        wrong-action-version-comment)
            perl -0pi -e 's/# v7\.0\.1/# v6.0.0/' "$fixture/workflows/ci.yml"
            ;;
        extra-action-version-comment)
            perl -0pi -e 's/# v7\.0\.1/# v7.0.1 extra/' "$fixture/workflows/ci.yml"
            ;;
        unqualified-cargo)
            printf '\n      - run: cargo build --release\n' >> "$fixture/workflows/release-artifacts.yml"
            ;;
        unqualified-rustc)
            printf '\n      - run: rustc --version --verbose\n' >> "$fixture/workflows/release-artifacts.yml"
            ;;
        missing-license-audit-ci)
            perl -0pi -e 's#\n          \./scripts/audit-license-policy\.sh##' "$fixture/workflows/ci.yml"
            ;;
        missing-license-audit-release)
            perl -0pi -e 's#run: \./scripts/audit-license-policy\.sh#run: true#' "$fixture/workflows/release-artifacts.yml"
            ;;
        missing-publication-license-audit-release)
            perl -0pi -e 's#run: \./scripts/audit-publication-license\.sh#run: true#' "$fixture/workflows/release-artifacts.yml"
            ;;
        missing-license-docs)
            perl -0pi -e 's#^.*Copy-Item docs/licensing\.md.*\n##m' "$fixture/workflows/release-artifacts.yml"
            ;;
        wrong-archive-name)
            perl -0pi -e 's#tellurion-v\$version-\$target#tellurion-$build_id-$target#' "$fixture/workflows/release-artifacts.yml"
            ;;
        # The invariant that replaced the old literal pin: an archive name may
        # not carry a version of its own, because that version could drift from
        # the one the workspace declares.
        hardcoded-archive-version)
            perl -0pi -e 's#\$package_name = "tellurion-v\$version-\$target"#$package_name = "tellurion-v0.3.0-$target"#' "$fixture/workflows/release-artifacts.yml"
            ;;
        hardcoded-artifact-version)
            perl -0pi -e 's#name: tellurion-v\$\{\{ needs\.source-artifact\.outputs\.version \}\}-candidate-#name: tellurion-v0.3.0-candidate-#' "$fixture/workflows/release-artifacts.yml"
            ;;
        missing-version-resolver)
            perl -0pi -e 's#\n      - name: Resolve workspace version\n.*?(?=\n      - uses:)##s' "$fixture/workflows/release-artifacts.yml"
            ;;
        # Reading *a* version is not enough; it has to be the workspace one.
        unanchored-version-resolver)
            perl -0pi -e 's#\^\\\[workspace\\\.package\\\]#^\\[package\\]#' "$fixture/workflows/release-artifacts.yml"
            ;;
        # The resolver must reject anything outside the supported SemVer forms,
        # or a version containing `/` could reach a path again.
        unconstrained-version-resolver)
            perl -0pi -e 's#^          \$versionPattern = .*$#          \$versionPattern = ".+"#m' "$fixture/workflows/release-artifacts.yml"
            ;;
        missing-policy-gate)
            perl -0pi -e 's#\n    needs: \[policy-audit\]##' "$fixture/workflows/release-artifacts.yml"
            ;;
        policy-audit-not-bash)
            perl -0pi -e 's#shell: bash#shell: pwsh#' "$fixture/workflows/release-artifacts.yml"
            ;;
        broad-release-tag)
            perl -0pi -e 's#^    tags: .*$#    tags: ["v*"]#m' "$fixture/workflows/release-artifacts.yml"
            ;;
        # Still a pattern, but pointed at a different tag namespace: the
        # release script's `vMAJOR.MINOR.PATCH` tag would never fire it.
        unmatchable-release-tag)
            perl -0pi -e 's#^    tags: .*$#    tags: ["release-[0-9]+.[0-9]+.[0-9]+"]#m' "$fixture/workflows/release-artifacts.yml"
            ;;
        manual-dispatch-blocked)
            perl -0pi -e "s#(policy-audit:\\n    name: license policy audit\\n)#\$1    if: github.ref == 'refs/tags/v0.3.0'\\n#; s#(native-artifacts:\\n    name: \\\$\\{\\{ matrix.target \\\}}\\n)#\$1    if: github.ref == 'refs/tags/v0.3.0'\\n#" "$fixture/workflows/release-artifacts.yml"
            ;;
        duplicate-source-archive)
            perl -0pi -e 's#(          build_id="\$\{GITHUB_SHA:0:12\}"\n)#$1          git archive --format=zip --output=dist/duplicate-source.zip "\$GITHUB_SHA"\n#' "$fixture/workflows/release-artifacts.yml"
            ;;
        source-archive-inside-matrix)
            perl -0pi -e 's#(\$archive = Join-Path \(Join-Path \(Get-Location\) "dist"\) \$archive_name\n)#$1          git archive --format=zip --output="dist/duplicate-source.zip" "$env:GITHUB_SHA"\n#' "$fixture/workflows/release-artifacts.yml"
            ;;
        missing-sbom)
            perl -0pi -e 's#\n      - name: Generate workspace SBOM\n.*?(?=\n      - name:)##s' "$fixture/workflows/release-artifacts.yml"
            ;;
        missing-aggregate-checksums)
            perl -0pi -e 's#^[[:space:]]*shasum -a 256 .*SHA256SUMS[[:space:]]*$##m' "$fixture/workflows/release-artifacts.yml"
            ;;
        missing-public-attestation)
            perl -0pi -e 's#\n      - name: Attest release archives\n.*?(?=\n      - name:)##s' "$fixture/workflows/release-artifacts.yml"
            ;;
        attestation-on-private-repository)
            perl -0pi -e 's#github\.event\.repository\.private == false#github.event.repository.private == true#' "$fixture/workflows/release-artifacts.yml"
            ;;
        id-token-write-outside-attestation-job)
            perl -0pi -e 's#(  source-artifact:\n)#$1    permissions:\n      id-token: write\n#' "$fixture/workflows/release-artifacts.yml"
            ;;
        contents-write)
            perl -0pi -e 's#contents: read#contents: write#' "$fixture/workflows/release-artifacts.yml"
            ;;
        release-workflow-unexpected-read-scope)
            perl -0pi -e 's/(permissions:\n  contents: read\n)/$1  deployments: read\n/' "$fixture/workflows/release-artifacts.yml"
            ;;
        release-job-permission-override)
            perl -0pi -e 's/(  source-artifact:\n)/$1    permissions:\n      contents: read\n/' "$fixture/workflows/release-artifacts.yml"
            ;;
        release-aggregation-extra-scope)
            perl -0pi -e 's/(      attestations: write\n)/$1      security-events: read\n/' "$fixture/workflows/release-artifacts.yml"
            ;;
        release-unsupported-write)
            perl -0pi -e 's/(  source-artifact:\n)/$1    permissions:\n      deployments: write\n/' "$fixture/workflows/release-artifacts.yml"
            ;;
        release-nested-step-permission-block)
            perl -0pi -e 's#(      - name: Download source evidence\n)#$1        permissions:\n          contents: read\n#' "$fixture/workflows/release-artifacts.yml"
            ;;
        adversarial-quoted-uses-key)
            printf '\n      - name: adversarial action key # run: |\n        "uses" : example/unapproved-action@v1 # v1\n' \
                >> "$fixture/workflows/ci.yml"
            ;;
        scalar-dedent-uses-key)
            printf '\n      - name: |\n          scalar documentation\n        uses: example/unapproved-action@v1\n' \
                >> "$fixture/workflows/ci.yml"
            ;;
        scalar-indicator-dedent-uses-key)
            printf '\n      - name: |2-\n          scalar documentation\n        uses: example/unapproved-action@v1\n' \
                >> "$fixture/workflows/ci.yml"
            ;;
        flow-nonleading-bare-uses-key)
            printf '\n      - { name: adversarial, uses: example/unapproved-action@v1 }\n' \
                >> "$fixture/workflows/ci.yml"
            ;;
        flow-nonleading-quoted-uses-key)
            printf '\n      - { name: adversarial, "uses": example/unapproved-action@v1 }\n' \
                >> "$fixture/workflows/ci.yml"
            ;;
        scalar-dedent-permission-block)
            printf '\n      - name: |2\n          scalar documentation\n        permissions:\n          contents: read\n' \
                >> "$fixture/workflows/ci.yml"
            ;;
        extra-workflow-permission)
            printf 'name: Extra\non:\n  workflow_dispatch:\npermissions:\n  issues: write\njobs:\n  check:\n    runs-on: ubuntu-24.04\n    steps:\n      - run: true\n' \
                > "$fixture/workflows/extra.yml"
            ;;
        missing-codeowner-coverage)
            perl -ni -e 'print unless m#^/SECURITY\.md # ' "$fixture/CODEOWNERS"
            ;;
        source-export-bypassed)
            perl -0pi -e 's#python3 scripts/export-public-core\.py#cp -R . "\$public_core" \##' "$fixture/workflows/release-artifacts.yml"
            ;;
        source-sbom-private-checkout)
            perl -0pi -e 's#path: \$\{\{ runner\.temp \}\}/tellurion-public-core#path: .#' "$fixture/workflows/release-artifacts.yml"
            ;;
        source-archive-private-checkout)
            perl -0pi -e 's#(- name: Create source archive.*?public_core=)"\$\{RUNNER_TEMP\}/tellurion-public-core"#$1"\$\{GITHUB_WORKSPACE\}"#s' "$fixture/workflows/release-artifacts.yml"
            ;;
        missing-generated-notice)
            perl -0pi -e 's#python3 scripts/generate-third-party-notices\.py#true \##' "$fixture/workflows/release-artifacts.yml"
            ;;
        missing-ui-notice-verification)
            perl -0pi -e 's#python3 scripts/generate-ui-third-party-notices\.py#true \##' "$fixture/workflows/release-artifacts.yml"
            ;;
        missing-source-upload-notice)
            perl -0pi -e 's#^[[:space:]]*dist/THIRD_PARTY_NOTICES\.json\n##m' "$fixture/workflows/release-artifacts.yml"
            ;;
        missing-native-notice)
            perl -0pi -e 's#^[[:space:]]*Copy-Item .*THIRD_PARTY_NOTICES\.json.*\n##m' "$fixture/workflows/release-artifacts.yml"
            ;;
        unexpected-native-ui-notice)
            perl -0pi -e 's#(Copy-Item .*THIRD_PARTY_NOTICES\.json.*\n)#$1          Copy-Item "$env:RUNNER_TEMP/release-source-evidence/THIRD_PARTY_NOTICES.txt" -Destination "$package_dir"\n#' "$fixture/workflows/release-artifacts.yml"
            ;;
        missing-notice-checksum)
            perl -0pi -e 's# THIRD_PARTY_NOTICES\.txt(?= > SHA256SUMS)##' "$fixture/workflows/release-artifacts.yml"
            ;;
        tag-version-mismatch-accepted)
            perl -0pi -e 's#GITHUB_REF_NAME -ne#GITHUB_REF_NAME -eq#' "$fixture/workflows/release-artifacts.yml"
            ;;
        release-publication-command)
            printf '\n      - run: gh release create candidate dist/tellurion.zip\n' >> "$fixture/workflows/release-artifacts.yml"
            ;;
        *)
            echo "unknown mutation: $name" >&2
            exit 2
            ;;
    esac

    # A substitution that silently matched nothing would make its case vacuous:
    # the guard would "reject" a workflow it had never been asked to judge.
    if diff -r -q .github/workflows "$fixture/workflows" >/dev/null 2>&1 \
        && cmp -s .github/CODEOWNERS "$fixture/CODEOWNERS"; then
        echo "FAIL: $name mutation left the workflows unchanged" >&2
        failures=$((failures + 1))
        return
    fi

    case "$name" in
        *mutable-action-reference|*-unknown-pinned-action|canonical-local-action|canonical-docker-action|canonical-reusable-workflow)
            expected_message='unapproved workflow action or version comment'
            ;;
        quoted-uses-key|flow-uses-key|spaced-uses-key|quoted-reusable-workflow-key|adversarial-quoted-uses-key|scalar-dedent-uses-key|scalar-indicator-dedent-uses-key|flow-nonleading-bare-uses-key|flow-nonleading-quoted-uses-key)
            expected_message='workflow action must use canonical bare block-style uses: syntax'
            ;;
        duplicate-source-archive|source-archive-inside-matrix)
            expected_message='exactly one source archive'
            ;;
        missing-sbom)
            expected_message='workspace SBOM'
            ;;
        missing-aggregate-checksums)
            expected_message='aggregate checksum'
            ;;
        missing-public-attestation)
            expected_message='exactly two public attestation steps'
            ;;
        attestation-on-private-repository)
            expected_message='attestations must be public-only'
            ;;
        id-token-write-outside-attestation-job)
            expected_message='id-token write permission is allowed only on aggregation job'
            ;;
        contents-write|release-publication-command)
            expected_message='release-publication capability found'
            ;;
        release-workflow-unexpected-read-scope|release-job-permission-override|release-aggregation-extra-scope|release-unsupported-write|release-nested-step-permission-block|scalar-dedent-permission-block|extra-workflow-permission)
            expected_message='exact canonical mappings'
            ;;
        missing-codeowner-coverage)
            expected_message='CODEOWNERS'
            ;;
        source-export-bypassed|missing-generated-notice|missing-ui-notice-verification)
            expected_message='clean source evidence flow'
            ;;
        source-sbom-private-checkout)
            expected_message='workspace SBOM'
            ;;
        source-archive-private-checkout)
            expected_message='clean exported tree'
            ;;
        missing-source-upload-notice)
            expected_message='source evidence upload'
            ;;
        missing-native-notice)
            expected_message='release package'
            ;;
        unexpected-native-ui-notice)
            expected_message='must not mislabel the UI notice'
            ;;
        missing-native-release-gate)
            expected_message='gate prebuilt binary release readiness'
            ;;
        missing-notice-checksum)
            expected_message='aggregate checksum'
            ;;
        tag-version-mismatch-accepted)
            expected_message='tag/version mismatch'
            ;;
    esac

    local output
    if output="$(WORKFLOW_DIR="$fixture/workflows" CODEOWNERS_FILE="$fixture/CODEOWNERS" bash scripts/check-release-workflow.sh 2>&1)"; then
        echo "FAIL: $name mutation was accepted" >&2
        failures=$((failures + 1))
    elif [ -n "$expected_message" ] && ! printf '%s\n' "$output" | rg -q -- "$expected_message"; then
        echo "FAIL: $name mutation was rejected for the wrong reason: $output" >&2
        failures=$((failures + 1))
    else
        echo "ok: $name mutation rejected"
    fi
}

MUTATIONS=(
    duplicate-source-archive
    source-archive-inside-matrix
    missing-sbom
    missing-aggregate-checksums
    missing-public-attestation
    attestation-on-private-repository
    id-token-write-outside-attestation-job
    contents-write
    release-publication-command
)

FINAL_FIX_MUTATIONS=(
    release-workflow-unexpected-read-scope
    release-job-permission-override
    release-aggregation-extra-scope
    release-unsupported-write
    release-nested-step-permission-block
    adversarial-quoted-uses-key
    scalar-dedent-uses-key
    scalar-indicator-dedent-uses-key
    flow-nonleading-bare-uses-key
    flow-nonleading-quoted-uses-key
    scalar-dedent-permission-block
    extra-workflow-permission
    missing-codeowner-coverage
    source-export-bypassed
    source-sbom-private-checkout
    source-archive-private-checkout
    missing-generated-notice
    missing-ui-notice-verification
    missing-source-upload-notice
    missing-native-notice
    unexpected-native-ui-notice
    missing-native-release-gate
    missing-notice-checksum
    tag-version-mismatch-accepted
)

CORE_CONTRACT_MUTATIONS=(
    windows-shell smoke-directory unsafe-ref-name ref-name-as-version
    release-api gh-release-publication gh-release-action-publication
    contents-write-publication cargo-publish-publication
    crates-publisher-outside-publish-workflow crates-auth-outside-publish-workflow
    crates-token-outside-publish-workflow image-push git-push
    ci-mutable-action-reference release-mutable-action-reference
    ci-unknown-pinned-action release-unknown-pinned-action
    canonical-local-action canonical-docker-action canonical-reusable-workflow
    quoted-uses-key flow-uses-key spaced-uses-key quoted-reusable-workflow-key
    missing-action-version-comment wrong-action-version-comment extra-action-version-comment
    unqualified-cargo unqualified-rustc missing-license-audit-ci
    missing-license-audit-release missing-publication-license-audit-release missing-license-docs wrong-archive-name
    hardcoded-archive-version hardcoded-artifact-version missing-version-resolver
    unanchored-version-resolver unconstrained-version-resolver missing-policy-gate
    policy-audit-not-bash broad-release-tag unmatchable-release-tag
    manual-dispatch-blocked
)

mutation_partition="${MUTATION_PARTITION:-all}"
case "$mutation_partition" in
    all|release-evidence|final-fixes|core-contract|guide)
        ;;
    *)
        echo "unknown mutation partition: $mutation_partition" >&2
        exit 2
        ;;
esac

if [ "$mutation_partition" = all ] || [ "$mutation_partition" = release-evidence ]; then
    for mutation in "${MUTATIONS[@]}"; do
        expect_rejected "$mutation"
    done
fi

if [ "$mutation_partition" = all ] || [ "$mutation_partition" = final-fixes ]; then
    for mutation in "${FINAL_FIX_MUTATIONS[@]}"; do
        expect_rejected "$mutation"
    done

    safe_uses_fixture="$fixture_root/safe-uses-like-run"
    mkdir -p "$safe_uses_fixture"
    cp -R .github/workflows "$safe_uses_fixture/workflows"
    printf '\n      - name: uses scalar documentation example\n        run: |\n          uses: example/unapproved-action@v1 # v1\n' \
        >> "$safe_uses_fixture/workflows/ci.yml"
    printf '\n      - name: |2-\n          uses: literal scalar text\n          permissions:\n            contents: read\n' \
        >> "$safe_uses_fixture/workflows/ci.yml"
    printf '\n      - { run: "echo literal, uses: documentation" }\n' \
        >> "$safe_uses_fixture/workflows/ci.yml"
    printf '\n      - name: scalar indentation control\n        run: |\n          # literal scalar comment\n            deeper literal line\n          uses: literal scalar text\n' \
        >> "$safe_uses_fixture/workflows/ci.yml"
    if ! WORKFLOW_DIR="$safe_uses_fixture/workflows" \
        bash scripts/check-release-workflow.sh >/dev/null 2>&1; then
        echo "FAIL: literal uses/permissions lines inside block scalars were rejected" >&2
        failures=$((failures + 1))
    else
        echo "ok: literal uses/permissions lines inside block scalars accepted"
    fi

    for required_behavior in \
        'python3 scripts/export-public-core\.py' \
        'python3 scripts/audit-dependency-licenses\.py' \
        '--inventory-output "\$dependency_inventory"' \
        'python3 scripts/generate-third-party-notices\.py' \
        'path:[[:space:]]*\$\{\{ runner\.temp \}\}/tellurion-public-core' \
        'Copy-Item .*THIRD_PARTY_NOTICES\.json' \
        'shasum -a 256 .*THIRD_PARTY_NOTICES\.json.*SHA256SUMS' \
        '\$env:GITHUB_REF_TYPE -eq "tag"' \
        '\$env:GITHUB_REF_NAME -ne "v\$\{\{ steps\.version\.outputs\.version \}\}"'; do
        if ! rg -q -- "$required_behavior" .github/workflows/release-artifacts.yml; then
            echo "FAIL: final release evidence behavior is missing $required_behavior" >&2
            failures=$((failures + 1))
        fi
    done
    if rg -q 'git archive' .github/workflows/release-artifacts.yml; then
        echo "FAIL: source candidate still archives the private checkout" >&2
        failures=$((failures + 1))
    fi
fi

if [ "$mutation_partition" = all ] || [ "$mutation_partition" = core-contract ]; then
    for mutation in "${CORE_CONTRACT_MUTATIONS[@]}"; do
        expect_rejected "$mutation"
    done
fi

# The install guide is the derived mention outside the workflows that the guard
# checks, and it is what the archive-name literal used to be pinned against.
# Prove a guide that disagrees with the workspace version is still refused.
expect_guide_rejected() {
    local name="$1"
    local sed_script="$2"
    local guide="$fixture_root/$name.md"

    sed "$sed_script" docs/quickstart/install.md >"$guide"
    if cmp -s docs/quickstart/install.md "$guide"; then
        echo "FAIL: $name mutation left the install guide unchanged" >&2
        failures=$((failures + 1))
        return
    fi

    if INSTALL_GUIDE="$guide" bash scripts/check-release-workflow.sh >/dev/null 2>&1; then
        echo "FAIL: $name mutation was accepted" >&2
        failures=$((failures + 1))
    else
        echo "ok: $name mutation rejected"
    fi
}

if [ "$mutation_partition" = all ] || [ "$mutation_partition" = guide ]; then
    current_version="$(workspace_version)"
    expect_guide_rejected stale-install-guide \
        "s/tellurion-v$current_version-aarch64/tellurion-v9.9.9-aarch64/"
    expect_guide_rejected undocumented-target \
        '/x86_64-pc-windows-msvc/d'
fi

# The unmutated workflows must still pass, or every "rejected" above could be
# an artefact of the guard failing for an unrelated reason.
if ! bash scripts/check-release-workflow.sh >/dev/null; then
    echo "FAIL: the repository's own workflows do not satisfy the contract" >&2
    failures=$((failures + 1))
else
    echo "ok: unmutated workflows accepted"
fi

# GitHub's Ubuntu runner uses GNU grep, which rejects a combined ERE and
# fixed-string matcher. Exercise the no-ripgrep shim through the exact
# CODEOWNERS lookup flags so a local BSD grep cannot mask that CI failure.
no_rg_path="$fixture_root/no-rg-fixed-string-bin"
mkdir -p "$no_rg_path"
for tool in bash grep; do
    tool_path="$(command -v "$tool")"
    ln -s "$tool_path" "$no_rg_path/$tool"
done
if ! PATH="$no_rg_path" "$no_rg_path/bash" -c '
    set -euo pipefail
    grep() {
        local arg has_extended=0 has_fixed=0
        for arg in "$@"; do
            case "$arg" in
                -*E*) has_extended=1 ;;
            esac
            case "$arg" in
                -*F*) has_fixed=1 ;;
            esac
        done
        if [ "$has_extended" -eq 1 ] && [ "$has_fixed" -eq 1 ]; then
            echo "grep: conflicting matchers specified" >&2
            return 2
        fi
        command grep "$@"
    }
    . scripts/rg-compat.sh
    printf "%s\\n" "/distribution/public-core.toml @ccancellieri" \
        | rg -Fqx -- "/distribution/public-core.toml @ccancellieri"
'; then
    echo "FAIL: no-ripgrep fixed-string lookup is incompatible with GNU grep" >&2
    failures=$((failures + 1))
else
    echo "ok: no-ripgrep fixed-string lookup supports combined -Fqx"
fi

if [ "$failures" -ne 0 ]; then
    exit 1
fi

echo "release workflow mutation tests passed"
