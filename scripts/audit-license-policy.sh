#!/usr/bin/env bash
# Verifies the repository-wide license, publication, and first-party version
# policy for the AGPL-3.0-only release line.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR/.."

# shellcheck source=scripts/workspace-version.sh
. "$SCRIPT_DIR/workspace-version.sh"

fail=0

# Everything below compares against the one declared version instead of a
# literal, so a `scripts/release.sh` bump moves the policy with the release
# rather than turning every audit red.
version="$(workspace_version)" || exit 1

for tool in cargo git jq; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "ERROR: required tool '$tool' not found on PATH" >&2
        exit 1
    }
done

check_workspace_package_value() {
    local expected="$1"

    awk -v expected="$expected" '
        $0 == "[workspace.package]" { in_workspace_package = 1; next }
        in_workspace_package && /^\[/ { exit }
        in_workspace_package && $0 == expected { found = 1 }
        END { exit(found ? 0 : 1) }
    ' Cargo.toml
}

require_workspace_package_value() {
    local expected="$1"

    if ! check_workspace_package_value "$expected"; then
        echo "FAIL workspace.package: missing $expected"
        fail=1
    fi
}

require_workspace_package_value "version = \"$version\""
require_workspace_package_value 'license = "AGPL-3.0-only"'
require_workspace_package_value 'publish = false'

metadata="$(cargo metadata --locked --no-deps --format-version 1)"

while IFS=$'\t' read -r name manifest resolved_version license publish; do
    if [ "$resolved_version" != "$version" ]; then
        echo "FAIL $name: resolved version is $resolved_version, expected $version"
        fail=1
    fi

    if [ "$license" != 'AGPL-3.0-only' ]; then
        echo "FAIL $name: resolved license is $license, expected AGPL-3.0-only"
        fail=1
    fi

    if [ "$publish" != 'true' ]; then
        echo "FAIL $name: resolved publish setting is not false"
        fail=1
    fi

    if ! grep -qx 'publish\.workspace = true' "$manifest"; then
        echo "FAIL $manifest: missing publish.workspace = true"
        fail=1
    fi
done < <(printf '%s\n' "$metadata" | jq -r '
    .workspace_members as $members
    | .packages[]
    | select(.id as $id | $members | index($id))
    | [.name, .manifest_path, .version, .license, (.publish == [])]
    | @tsv
')

while IFS=$'\t' read -r name manifest; do
    package_dir="$(dirname "$manifest")"

    if [ "$name" = 'tellurion-duckdb' ]; then
        if ! grep -qx 'license-file = "../../LICENSE"' "$manifest"; then
            echo "FAIL $manifest: missing license-file = \"../../LICENSE\""
            fail=1
        fi
        continue
    fi

    license_copy="$package_dir/LICENSE"
    if [ ! -f "$license_copy" ]; then
        echo "FAIL $manifest: missing crate-local LICENSE"
        fail=1
    elif ! cmp -s LICENSE "$license_copy"; then
        echo "FAIL $license_copy: does not match LICENSE"
        fail=1
    fi
done < <(printf '%s\n' "$metadata" | jq -r '
    .workspace_members as $members
    | .packages[]
    | select(.id as $id | $members | index($id))
    | [.name, .manifest_path]
    | @tsv
')

# The publication surface is the tracked tree. Scanning the physical checkout
# instead also enters ignored build products and nested worktrees, whose
# third-party LICENSE files are both legitimate and outside this policy.
tracked_license_list="$(mktemp)"
cleanup_tracked_license_list() {
    rm -f "$tracked_license_list"
}
trap cleanup_tracked_license_list EXIT
if ! git ls-files -z -- ':(glob)**/LICENSE' > "$tracked_license_list"; then
    echo "ERROR: could not inventory tracked LICENSE files" >&2
    exit 1
fi

while IFS= read -r -d '' license_copy; do
    if [ "$license_copy" = 'LICENSE' ]; then
        continue
    fi

    if ! cmp -s LICENSE "$license_copy"; then
        echo "FAIL $license_copy: does not match LICENSE"
        fail=1
    fi
done < "$tracked_license_list"
rm -f "$tracked_license_list"
trap - EXIT

first_party_names="$(printf '%s\n' "$metadata" | jq -r '
    .workspace_members as $members
    | .packages[]
    | select(.id as $id | $members | index($id))
    | .name
')"

# Workspace membership, not a hand-maintained name list, defines which
# dependency declarations are first-party. Consumers using `workspace = true`
# inherit these exact root declarations.
while IFS= read -r first_party_name; do
    declaration="$(awk -v name="$first_party_name" '
        $0 == "[workspace.dependencies]" { in_workspace_dependencies = 1; next }
        in_workspace_dependencies && /^\[/ { exit }
        in_workspace_dependencies && $0 ~ "^" name "[[:space:]]*=" { print; exit }
    ' Cargo.toml)"

    [ -n "$declaration" ] || continue
    if ! printf '%s\n' "$declaration" | grep -Fq "version = \"$version\""; then
        echo "FAIL first-party dependency $first_party_name: expected version = \"$version\", found: $declaration"
        fail=1
    fi
done <<< "$first_party_names"

require_document_text() {
    local document="$1"
    local expected="$2"

    if ! grep -Fq -- "$expected" "$document"; then
        echo "FAIL $document: missing required licensing text: $expected"
        fail=1
    fi
}

forbidden_document_text() {
    local document="$1"
    local forbidden="$2"

    if grep -Fq -- "$forbidden" "$document"; then
        echo "FAIL $document: contains obsolete licensing text: $forbidden"
        fail=1
    fi
}

require_document_text README.md 'AGPL-3.0-only'
require_document_text README.md 'open-source software'
require_document_text README.md 'Commercial use is allowed'
require_document_text README.md 'Tellurion Cloud is not currently offered'
require_document_text README.md 'Section 13'
forbidden_document_text README.md 'BUSL-1.1'
forbidden_document_text README.md 'Business Source License'
forbidden_document_text README.md 'Change Date'

require_document_text COMMERCIAL-LICENSE.md 'GNU Affero General Public License'
require_document_text COMMERCIAL-LICENSE.md 'commercial use'
require_document_text COMMERCIAL-LICENSE.md 'https://github.com/ccancellieri'
require_document_text COMMERCIAL-LICENSE.md 'Tellurion Cloud is not currently offered'
forbidden_document_text COMMERCIAL-LICENSE.md 'BUSL-1.1'
forbidden_document_text COMMERCIAL-LICENSE.md 'Change Date'

require_document_text CLA.md 'AGPL-3.0-only'
require_document_text CLA.md 'No contributor licence agreement'
require_document_text CONTRIBUTING.md 'External code and documentation pull requests are not merged yet'
require_document_text docs/licensing.md 'AGPL-3.0-only'
require_document_text docs/licensing.md 'Commercial use is allowed'
require_document_text docs/licensing.md 'Tellurion Cloud is not currently offered'
forbidden_document_text docs/licensing.md 'BUSL-1.1'
forbidden_document_text docs/licensing.md 'Change Date'
require_document_text docs/quickstart/install.md 'aarch64-apple-darwin'
require_document_text docs/quickstart/install.md 'x86_64-unknown-linux-musl'
require_document_text docs/quickstart/install.md 'x86_64-pc-windows-msvc'
require_document_text docs/quickstart/install.md "approved v$version public release"
if [ -f docs/design/2026-07-17-tellurion-v01-design.md ]; then
    require_document_text docs/design/2026-07-17-tellurion-v01-design.md 'Superseded for current licensing and distribution policy'
fi
for superseded_design in \
    docs/design/2026-07-19-distribution-editions.md \
    docs/design/2026-07-23-bsl-community-commercial-release.md \
    docs/design/2026-08-27-public-core-readiness-plan.md \
    docs/design/2026-08-27-public-core-readiness.md; do
    [ -f "$superseded_design" ] || continue
    require_document_text "$superseded_design" 'Superseded on 2026-09-02'
    require_document_text "$superseded_design" '2026-09-02-open-community-launch.md'
done

if [ "$fail" -ne 0 ]; then
    echo "license policy audit FAILED"
    exit 1
fi

echo "license policy audit passed"
