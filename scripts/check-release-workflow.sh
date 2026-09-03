#!/usr/bin/env bash
# Enforces the build-only release-candidate contract and rejects public release
# publication capability anywhere in the repository's GitHub workflows.

set -euo pipefail

# shellcheck source=scripts/rg-compat.sh
. "$(dirname "$0")/rg-compat.sh"
# shellcheck source=scripts/workspace-version.sh
. "$(dirname "$0")/workspace-version.sh"

workflow_dir="${WORKFLOW_DIR:-.github/workflows}"
workflow="$workflow_dir/release-artifacts.yml"
ci_workflow="$workflow_dir/ci.yml"
codeowners_file="${CODEOWNERS_FILE:-.github/CODEOWNERS}"

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

require_match() {
    local pattern="$1"
    local file="$2"
    rg -q -- "$pattern" "$file" || fail "missing required workflow behavior: $pattern"
}

step_block() {
    local step_name="$1"
    awk -v step_name="$step_name" '
        $0 ~ "^[[:space:]]*-[[:space:]]+name: " step_name "$" { capture = 1 }
        capture && $0 ~ "^[[:space:]]*-[[:space:]]+(name:|uses:)" && $0 !~ "name: " step_name "$" { exit }
        capture { print }
    ' "$workflow"
}

job_block() {
    local job_name="$1"
    awk -v job_name="$job_name" '
        $0 ~ "^  " job_name ":$" { capture = 1 }
        capture && $0 ~ "^  [[:alnum:]_-]+:$" && $0 !~ "^  " job_name ":$" { exit }
        capture { print }
    ' "$workflow"
}

[ -d "$workflow_dir" ] || fail "missing workflow directory: $workflow_dir"
[ -f "$workflow" ] || fail "missing $workflow"
[ -f "$ci_workflow" ] || fail "missing $ci_workflow"

workflows=()
while IFS= read -r workflow_file; do
    workflows+=("$workflow_file")
done < <(find "$workflow_dir" -type f \( -name '*.yml' -o -name '*.yaml' \) -print | sort)
[ "${#workflows[@]}" -gt 0 ] || fail "no workflow files found"

# Validate immutable action identities before behavior-specific checks so a
# mutated action is always rejected for its unapproved identity, not for a
# downstream contract that happens to mention the same step.
while IFS=$'\t' read -r action comment; do
    if [ "$action" = invalid ]; then
        fail "workflow action must use canonical bare block-style uses: syntax: $comment"
    fi
    case "$action:$comment" in
        actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1:v7.0.1|actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a:v7.0.1|actions/download-artifact@634f93cb2916e3fdff6788551b99b062d0335ce0:v5|anchore/sbom-action@e22c389904149dbc22b58101806040fa8d37a610:v0|actions/attest@1e69f48acb82d1966a394da916b4c1698aa569d6:v4|dtolnay/rust-toolchain@032958afbdc797a9164d3bc0b56325c1308924a5:1.97.1|Swatinem/rust-cache@6323deb102c322ba6fcbdcafc7e3dddab59af2b6:v2.9.2|rust-lang/crates-io-auth-action@c6f97d42243bad5fab37ca0427f495c86d5b1a18:v1)
            ;;
        *)
            fail "unapproved workflow action or version comment: $action # $comment"
            ;;
    esac
done < <(
    python3 scripts/check-workflow-permissions.py --list-actions "${workflows[@]}"
)

require_match 'channel[[:space:]]*=[[:space:]]*"1\.97\.1"' rust-toolchain.toml

# The release trigger is a semver tag pattern, not a frozen version. The exact
# pattern is pinned, so `v*`, `v0.3.*`, `v1.[0-9]+.[0-9]+` or any other
# loosening or narrowing is still rejected -- but the pattern now admits every
# version this repository can ever declare, instead of exactly one.
#
# In GitHub's filter syntax `[0-9]+` is one-or-more digits and `.` is literal.
# The two pinned patterns admit stable tags and the only prerelease this
# repository cuts: `-rc.N`. `workspace_version` rejects all other forms, which
# makes "the tag the release script cuts always fires this workflow" an
# invariant rather than a coincidence -- and keeps derived archive names
# filesystem-safe.
require_match 'tags:[[:space:]]*\["v\[0-9\]\+\.\[0-9\]\+\.\[0-9\]\+", "v\[0-9\]\+\.\[0-9\]\+\.\[0-9\]\+-rc\.\[0-9\]\+"\]' "$workflow"
version="$(workspace_version)" || fail "workspace version is unusable as a release tag"
version_resolver_count="$(rg -c '^[[:space:]]*- name: Resolve workspace version$' "$workflow" || true)"
[ "$version_resolver_count" -eq 2 ] \
    || fail "source and native jobs must each resolve the workspace version"

for matrix_entry in \
    'target: aarch64-apple-darwin[[:space:]]*$' \
    'runner: macos-14[[:space:]]*$' \
    'target: x86_64-unknown-linux-musl[[:space:]]*$' \
    'runner: ubuntu-24.04[[:space:]]*$' \
    'target: x86_64-pc-windows-msvc[[:space:]]*$' \
    'runner: windows-2022[[:space:]]*$'; do
    require_match "$matrix_entry" "$workflow"
done

require_match 'dtolnay/rust-toolchain@032958afbdc797a9164d3bc0b56325c1308924a5' "$workflow"
require_match 'targets:[[:space:]]*\$\{\{ matrix\.target \}\}' "$workflow"
require_match 'workflow_dispatch:' "$workflow"
if rg -q 'legal_approval' "$workflow"; then
    fail "build-only workflow must not request a misleading legal approval input"
fi

policy_job="$(job_block 'policy-audit')"
printf '%s\n' "$policy_job" | rg -q 'runs-on:[[:space:]]*ubuntu-24\.04' \
    || fail "license policy audit must run on Ubuntu"
if printf '%s\n' "$policy_job" | sed -n '1,5p' | rg -q '^[[:space:]]+if:'; then
    fail "license policy audit must be executable for workflow_dispatch"
fi
license_policy_step="$(step_block 'Audit license policy')"
printf '%s\n' "$license_policy_step" | rg -q 'shell:[[:space:]]*bash' \
    || fail "license policy audit must run under Bash"
printf '%s\n' "$license_policy_step" | rg -q 'scripts/audit-license-policy\.sh' \
    || fail "license policy audit job must invoke the policy script"
publication_license_step="$(step_block 'Audit publication licence')"
printf '%s\n' "$publication_license_step" | rg -q 'shell:[[:space:]]*bash' \
    || fail "publication licence audit must run under Bash"
printf '%s\n' "$publication_license_step" | rg -q 'scripts/audit-publication-license\.sh' \
    || fail "license policy audit job must invoke the publication licence audit"

source_job="$(job_block 'source-artifact')"
printf '%s\n' "$source_job" | rg -q 'needs:[[:space:]]*\[policy-audit\]' \
    || fail "source artifact must depend on the license policy audit"
printf '%s\n' "$source_job" | rg -q 'version:[[:space:]]*\$\{\{ steps\.version\.outputs\.version \}\}' \
    || fail "source artifact job must expose the resolved workspace version"

if rg -q 'git archive' "$workflow"; then
    fail "release workflow must create exactly one source archive from the clean exported tree"
fi
for source_evidence_requirement in \
    'python3 scripts/audit-dependency-licenses\.py' \
    '--output "\$dependency_summary"' \
    '--inventory-output "\$dependency_inventory"' \
    'python3 scripts/export-public-core\.py' \
    '--source \.' \
    '--destination "\$public_core"' \
    'python3 scripts/generate-third-party-notices\.py' \
    '--inventory "\$dependency_inventory"' \
    '--output "\$public_core/THIRD_PARTY_NOTICES\.json"' \
    'cp "\$public_core/THIRD_PARTY_NOTICES\.json" dist/THIRD_PARTY_NOTICES\.json'; do
    printf '%s\n' "$source_job" | rg -q -- "$source_evidence_requirement" \
        || fail "clean source evidence flow is missing $source_evidence_requirement"
done
printf '%s\n' "$source_job" | rg -q 'path:[[:space:]]*\$\{\{ runner\.temp \}\}/tellurion-public-core' \
    || fail "workspace SBOM must use the clean exported tree"
source_archive_step="$(step_block 'Create source archive')"
archive_source_count="$(printf '%s\n' "$source_archive_step" | rg -c '^[[:space:]]+public_core=' || true)"
[ "$archive_source_count" -eq 1 ] \
    || fail "source archive must declare exactly one input tree"
printf '%s\n' "$source_archive_step" | rg -q '^[[:space:]]+public_core="\$\{RUNNER_TEMP\}/tellurion-public-core"$' \
    || fail "source archive must read the clean exported tree"
printf '%s\n' "$source_archive_step" | rg -q 'python3 - "\$public_core" "\$output" <<' \
    || fail "source archive generator must receive the clean exported tree"
printf '%s\n' "$source_archive_step" | rg -q 'zipfile\.ZipFile\(output' \
    || fail "exactly one source archive must be created from the clean exported tree"
printf '%s\n' "$source_job" | rg -q 'tellurion-v\$\{version\}-source-\$\{build_id\}\.zip' \
    || fail "source archive name must derive from workspace version and revision"
printf '%s\n' "$source_job" | rg -q 'dist/THIRD_PARTY_NOTICES\.json' \
    || fail "source evidence upload must carry THIRD_PARTY_NOTICES.json"

for sbom_requirement in \
    'anchore/sbom-action@e22c389904149dbc22b58101806040fa8d37a610[[:space:]]+# v0' \
    'path:[[:space:]]*\$\{\{ runner\.temp \}\}/tellurion-public-core' \
    'format:[[:space:]]*spdx-json' \
    'output-file:[[:space:]]*dist/tellurion\.spdx\.json' \
    'upload-artifact:[[:space:]]*false' \
    'upload-release-assets:[[:space:]]*false' \
    'syft-version:[[:space:]]*v1\.51\.0'; do
    printf '%s\n' "$source_job" | rg -q -- "$sbom_requirement" \
        || fail "workspace SBOM configuration is missing $sbom_requirement"
done

native_job="$(job_block 'native-artifacts')"
printf '%s\n' "$native_job" | rg -q 'needs:[[:space:]]*\[source-artifact\]' \
    || fail "native matrix must depend on clean source evidence"
if printf '%s\n' "$native_job" | sed -n '1,6p' | rg -q '^[[:space:]]+if:'; then
    fail "native matrix must be executable for workflow_dispatch"
fi
if printf '%s\n' "$native_job" | rg -q 'git archive'; then
    fail "release workflow must create exactly one source archive outside the native matrix"
fi

build_step="$(step_block 'Build default-feature binaries')"
printf '%s\n' "$build_step" | rg -q \
    'cargo \+1\.97\.1 build.*--release.*--locked.*--target[[:space:]]+\$\{\{ matrix\.target \}\}.*-p[[:space:]]+tellurion.*-p[[:space:]]+tellurion-ingest' \
    || fail "native build must invoke locked Cargo directly for both binaries"
if printf '%s\n' "$build_step" | rg -q '(^|[[:space:]])target='; then
    fail "native build uses shell-specific target assignment"
fi

if rg -n '\b(cargo|rustc)[[:space:]]+' "$workflow" | rg -v '\b(cargo|rustc)[[:space:]]+\+1\.97\.1\b' >/dev/null; then
    fail "release workflow uses an unqualified cargo or rustc command"
fi
require_match 'rustc \+1\.97\.1 --version --verbose' "$workflow"
require_match 'scripts/audit-license-policy\.sh' "$ci_workflow"

package_step="$(step_block 'Package native artifact')"
for package_requirement in \
    'Copy-Item LICENSE, COPYRIGHT\.md, README\.md' \
    'Copy-Item COMMERCIAL-LICENSE\.md' \
    'Copy-Item .*THIRD_PARTY_NOTICES\.json' \
    'example-geopackage\.yaml' \
    'Copy-Item docs/licensing\.md' \
    'Join-Path \$package_dir "docs"' \
    'BUILD-INFO' \
    'Compress-Archive' \
    'tar -C dist -czf'; do
    printf '%s\n' "$package_step" | rg -q -- "$package_requirement" \
        || fail "release package is missing $package_requirement"
done
printf '%s\n' "$package_step" | rg -q '\$package_name = "tellurion-v\$version-\$target"' \
    || fail "platform archive name must be derived from the workspace version"

source_download_step="$(step_block 'Download source evidence')"
printf '%s\n' "$source_download_step" | rg -q 'actions/download-artifact@634f93cb2916e3fdff6788551b99b062d0335ce0' \
    || fail "native packaging must download the generated dependency notice"
printf '%s\n' "$source_download_step" | rg -q 'path:[[:space:]]*\$\{\{ runner\.temp \}\}/release-source-evidence' \
    || fail "native packaging must use source evidence from the intermediate artifact"

# The archive name and the workspace version cannot disagree, because the
# workflow reads the version out of the workspace manifest rather than
# repeating it. Pin that derivation: the resolver step must anchor on
# `[workspace.package]` and accept only MAJOR.MINOR.PATCH[-rc.N], and no naming
# expression anywhere in the workflow may hard-code a version literal that
# could then drift from Cargo.toml.
version_step="$(step_block 'Resolve workspace version')"
for version_requirement in \
    'id: version' \
    'Get-Content -Raw -Path Cargo\.toml' \
    '\^\\\[workspace\\\.package\\\]' \
    '\$number = '\''\(\?:0\|\[1-9\]\\d\*\)'\''' \
    '\$versionPattern = "\$number\\\.\$number\\\.\$number\(\?:-rc\\\.\$number\)\?"' \
    'throw ' \
    '"version=\$version" \| Out-File -FilePath \$env:GITHUB_OUTPUT'; do
    printf '%s\n' "$version_step" | rg -q -- "$version_requirement" \
        || fail "workspace version resolver is missing $version_requirement"
done

tag_guard_count="$(rg -c '^[[:space:]]*- name: Verify tag matches workspace version$' "$workflow" || true)"
[ "$tag_guard_count" -eq 1 ] \
    || fail "release workflow must contain exactly one tag/version mismatch guard"
tag_guard="$(step_block 'Verify tag matches workspace version')"
printf '%s\n' "$tag_guard" | rg -q '\$env:GITHUB_REF_TYPE -eq "tag"' \
    || fail "tag/version mismatch guard must apply only to tag events"
printf '%s\n' "$tag_guard" | rg -q '\$env:GITHUB_REF_NAME -ne "v\$\{\{ steps\.version\.outputs\.version \}\}"' \
    || fail "tag/version mismatch guard must compare the exact v-prefixed workspace version"
tag_guard_line="$(rg -n '^[[:space:]]*- name: Verify tag matches workspace version$' "$workflow" | cut -d: -f1)"
source_prepare_line="$(rg -n '^[[:space:]]*- name: Prepare source evidence$' "$workflow" | cut -d: -f1)"
if [ -z "$tag_guard_line" ] || [ -z "$source_prepare_line" ] || [ "$tag_guard_line" -ge "$source_prepare_line" ]; then
    fail "tag/version mismatch guard must run before source packaging"
fi
if rg -n 'tellurion-v[0-9]' "$workflow"; then
    fail "workflow hard-codes a release version instead of deriving it from Cargo.toml"
fi

# The archive name still has to match the installation guide -- the property
# the old literal pin was really protecting. Now that the workflow computes the
# name, check the guide against the same workspace version the workflow will
# resolve, for every target the matrix actually builds.
install_guide="${INSTALL_GUIDE:-docs/quickstart/install.md}"
[ -f "$install_guide" ] || fail "missing $install_guide"
while IFS= read -r matrix_target; do
    rg -q -- "tellurion-v$version-$matrix_target" "$install_guide" \
        || fail "$install_guide does not document tellurion-v$version-$matrix_target"
done < <(
    rg --no-filename '^[[:space:]]*- target:[[:space:]]' "$workflow" \
        | sed -E 's/^[[:space:]]*- target:[[:space:]]*//'
)
rg -q 'shasum -a 256 -c SHA256SUMS' "$install_guide" \
    || fail "$install_guide does not document aggregate checksum verification"
rg -Fq "gh attestation verify tellurion-v$version-aarch64-apple-darwin.tar.gz" "$install_guide" \
    || fail "$install_guide does not document GitHub attestation verification"
rg -q -- '--repo ccancellieri/tellurion' "$install_guide" \
    || fail "$install_guide does not bind attestation verification to this repository"
rg -U -q 'internal candidate, not an approved[[:space:]]+public binary' "$install_guide" \
    || fail "$install_guide does not explain missing public attestations"

# A ref name can include `/`; it belongs only in BUILD-INFO, never in a path
# or artifact name. Archive identities derive from the workspace version, the
# SHA and the target instead. Only the one BUILD-INFO `ref=` line may name it
# -- pinning that exact line, rather than any line containing `version=`,
# closes the gap where a version-shaped expression could smuggle the ref back
# into an identity.
unsafe_ref_use="$(rg -n 'GITHUB_REF_NAME|github\.ref_name' "$workflow" \
    | rg -v '^[0-9]+:[[:space:]]*"ref=\$env:GITHUB_REF_NAME"$' \
    | rg -v '^[0-9]+:[[:space:]]*if \(\$env:GITHUB_REF_TYPE -eq "tag" -and \$env:GITHUB_REF_NAME -ne "v\$\{\{ steps\.version\.outputs\.version \}\}"\) \{$' \
    || true)"
if [ -n "$unsafe_ref_use" ]; then
    echo "$unsafe_ref_use" >&2
    fail "workflow uses a raw ref name outside the BUILD-INFO ref= line"
fi

smoke_step="$(step_block 'Smoke test packaged demo')"
for smoke_requirement in \
    'New-Item -ItemType Directory -Force -Path \$smoke_dir' \
    'tar -xzf.*-C \$smoke_dir' \
    'Start-Process -FilePath \$ingest' \
    '/healthz' \
    '/public/features/catalogs/default/collections/demo/items\?limit=1'; do
    printf '%s\n' "$smoke_step" | rg -q -- "$smoke_requirement" \
        || fail "packaged-demo smoke test is missing $smoke_requirement"
done
mkdir_line="$(printf '%s\n' "$smoke_step" | nl -ba | rg 'New-Item -ItemType Directory -Force -Path \$smoke_dir' | awk '{ print $1 }')"
extract_line="$(printf '%s\n' "$smoke_step" | nl -ba | rg 'tar -xzf.*-C \$smoke_dir' | awk '{ print $1 }')"
if [ -z "$mkdir_line" ] || [ -z "$extract_line" ] || [ "$mkdir_line" -ge "$extract_line" ]; then
    fail "smoke directory must be created before tar extraction"
fi

for intermediate_step_name in 'Upload source evidence' 'Upload native artifact'; do
    intermediate_step="$(step_block "$intermediate_step_name")"
    printf '%s\n' "$intermediate_step" | rg -q 'actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a' \
        || fail "$intermediate_step_name must use the approved upload action"
    printf '%s\n' "$intermediate_step" | rg -q 'retention-days:[[:space:]]*7' \
        || fail "$intermediate_step_name must retain intermediate evidence for seven days"
done
source_upload="$(step_block 'Upload source evidence')"
printf '%s\n' "$source_upload" | rg -q 'dist/THIRD_PARTY_NOTICES\.json' \
    || fail "source evidence upload must carry THIRD_PARTY_NOTICES.json"

candidate_job="$(job_block 'release-candidate')"
printf '%s\n' "$candidate_job" | rg -q 'needs:[[:space:]]*\[source-artifact, native-artifacts\]' \
    || fail "release candidate aggregation must wait for source and native artifacts"
for permission in 'contents:[[:space:]]*read' 'id-token:[[:space:]]*write' 'attestations:[[:space:]]*write'; do
    printf '%s\n' "$candidate_job" | rg -q -- "$permission" \
        || fail "release candidate aggregation is missing permission $permission"
done
id_token_writes="$(rg -c 'id-token:[[:space:]]*write' "$workflow" || true)"
[ "$id_token_writes" -eq 1 ] \
    || fail "id-token write permission is allowed only on aggregation job"
attestation_writes="$(rg -c 'attestations:[[:space:]]*write' "$workflow" || true)"
[ "$attestation_writes" -eq 1 ] \
    || fail "attestations write permission is allowed only on aggregation job"

for download_requirement in \
    'actions/download-artifact@634f93cb2916e3fdff6788551b99b062d0335ce0[[:space:]]+# v5' \
    'path:[[:space:]]*dist' \
    'merge-multiple:[[:space:]]*true'; do
    printf '%s\n' "$candidate_job" | rg -q -- "$download_requirement" \
        || fail "release candidate download is missing $download_requirement"
done

checksum_step="$(step_block 'Assemble aggregate checksums')"
for checksum_requirement in \
    'source_archives=\(dist/tellurion-v\*-source-\*\.zip\)' \
    '\$\{#source_archives\[@\]\}.*-ne 1' \
    '\$\{#native_archives\[@\]\}.*-ne 3' \
    'test -f dist/tellurion\.spdx\.json' \
    'test -f dist/THIRD_PARTY_NOTICES\.json' \
    'unzip -Z1 .*source_archives.*THIRD_PARTY_NOTICES\.json' \
    'tar -tzf .*THIRD_PARTY_NOTICES' \
    'for archive in dist/tellurion-v\*-pc-windows-msvc\.zip' \
    'unzip -Z1 "\$archive".*THIRD_PARTY_NOTICES' \
    'shasum -a 256 .*THIRD_PARTY_NOTICES\.json.*SHA256SUMS' \
    'shasum -a 256 .*SHA256SUMS'; do
    printf '%s\n' "$checksum_step" | rg -q -- "$checksum_requirement" \
        || fail "aggregate checksum step is missing $checksum_requirement"
done

attest_count="$(printf '%s\n' "$candidate_job" | rg -c 'actions/attest@1e69f48acb82d1966a394da916b4c1698aa569d6[[:space:]]+# v4' || true)"
[ "$attest_count" -eq 2 ] \
    || fail "release candidate must contain exactly two public attestation steps"
for attestation_step_name in 'Attest release archives' 'Attest the source SBOM'; do
    attestation_step="$(step_block "$attestation_step_name")"
    printf '%s\n' "$attestation_step" | rg -q 'if:[[:space:]]*github\.event\.repository\.private == false' \
        || fail "attestations must be public-only"
done
archives_attestation="$(step_block 'Attest release archives')"
printf '%s\n' "$archives_attestation" | rg -q 'dist/tellurion-v\*\.tar\.gz' \
    || fail "archive attestation must cover tar archives"
printf '%s\n' "$archives_attestation" | rg -q 'dist/tellurion-v\*\.zip' \
    || fail "archive attestation must cover zip archives"
sbom_attestation="$(step_block 'Attest the source SBOM')"
printf '%s\n' "$sbom_attestation" | rg -q 'subject-path:[[:space:]]*dist/tellurion-v\*-source-\*\.zip' \
    || fail "source SBOM attestation must target the single source archive"
printf '%s\n' "$sbom_attestation" | rg -q 'sbom-path:[[:space:]]*dist/tellurion\.spdx\.json' \
    || fail "source SBOM attestation must attach the SPDX JSON document"

candidate_upload="$(step_block 'Upload release candidate')"
candidate_upload_count="$(rg -c -- '- name: Upload release candidate' "$workflow" || true)"
[ "$candidate_upload_count" -eq 1 ] \
    || fail "release workflow must upload exactly one final candidate artifact"
printf '%s\n' "$candidate_upload" | rg -q 'actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a' \
    || fail "release workflow must upload one final candidate artifact"
printf '%s\n' "$candidate_upload" | rg -q 'name: tellurion-v\$\{\{ needs\.source-artifact\.outputs\.version \}\}-candidate-\$\{\{ github\.sha \}\}' \
    || fail "candidate artifact name must carry the resolved version and safe revision"
printf '%s\n' "$candidate_upload" | rg -q 'path:[[:space:]]*dist/' \
    || fail "final candidate artifact must contain all release evidence"

# No workflow may gain permissions or commands that can publish an asset,
# container, crate, tag, or GitHub Release. Keep this list explicit so new
# publication mechanisms must be consciously reviewed before they can land.
publication_patterns=(
    '(contents|packages)[[:space:]]*:[[:space:]]*write\b'
    'permissions:[[:space:]]*write-all\b'
    'actions/create-release'
    'softprops/action-gh-release'
    'ncipollo/release-action'
    'gh[[:space:]]+release\b'
    'gh[[:space:]]+api.*(/releases\b|releases\b)'
    'github\.rest\.repos\.(createRelease|uploadReleaseAsset)'
    'curl[^\n]*(api\.github\.com|uploads\.github\.com)[^\n]*(/releases\b|releases\b)'
    # Every cargo invocation in this repository is toolchain-qualified
    # (`cargo +1.97.1 ...`), which the old bare `cargo publish` pattern did not
    # see. The qualifier is optional here so both forms are refused.
    'cargo[[:space:]]+(\+[^[:space:]]+[[:space:]]+)?publish\b'
    '(docker|podman|oras)[[:space:]].*\bpush\b'
    'docker/(login|build-push)-action'
    'git[[:space:]]+push\b'
)
publish_workflow="$workflow_dir/publish-crates.yml"
for pattern in "${publication_patterns[@]}"; do
    pattern_workflows=("${workflows[@]}")
    if [ "$pattern" = 'cargo[[:space:]]+(\+[^[:space:]]+[[:space:]]+)?publish\b' ]; then
        pattern_workflows=()
        for workflow_file in "${workflows[@]}"; do
            [ "$workflow_file" = "$publish_workflow" ] || pattern_workflows+=("$workflow_file")
        done
    fi
    if [ "${#pattern_workflows[@]}" -gt 0 ] && rg -n -i -- "$pattern" "${pattern_workflows[@]}"; then
        fail "release-publication capability found in workflow files"
    fi
done

bash scripts/check-crates-io-publish-workflow.sh \
    || fail "crates.io publication workflow contract does not hold"

python3 scripts/check-workflow-permissions.py \
    --workflow-dir "$workflow_dir" \
    || fail "workflow permissions must use exact canonical mappings"

[ -f "$codeowners_file" ] || fail "missing CODEOWNERS policy"
for owned_path in \
    '/distribution/public-core.toml @ccancellieri' \
    '/scripts/export-public-core.py @ccancellieri' \
    '/scripts/tests/test_export_public_core.py @ccancellieri' \
    '/.github/CODEOWNERS @ccancellieri' \
    '/.github/dependabot.yml @ccancellieri' \
    '/SECURITY.md @ccancellieri'; do
    rg -Fqx -- "$owned_path" "$codeowners_file" \
        || fail "CODEOWNERS is missing required owner mapping: $owned_path"
done

echo "release workflow contract passed for v$version"
