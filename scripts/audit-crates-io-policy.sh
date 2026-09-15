#!/usr/bin/env bash
# Validates the explicit, dependency-ordered crates.io publication boundary.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR/.."

# shellcheck source=scripts/workspace-version.sh
. "$SCRIPT_DIR/workspace-version.sh"

package_list="release/crates-io-packages.txt"
expected_version="$(workspace_version)" || exit 1

for tool in cargo jq grep awk sort uniq; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "ERROR: required tool '$tool' not found on PATH" >&2
        exit 1
    }
done

if [ ! -f "$package_list" ]; then
    echo "FAIL: missing $package_list" >&2
    exit 1
fi

packages="$(mktemp)"
workspace_packages="$(mktemp)"
positions="$(mktemp)"
cleanup() {
    rm -f "$packages" "$workspace_packages" "$positions"
}
trap cleanup EXIT

if ! awk '
    /^[[:space:]]*($|#)/ { next }
    NF != 1 { invalid = 1; next }
    { print $1 }
    END { exit invalid }
' "$package_list" > "$packages"; then
    echo "FAIL $package_list: entries must contain exactly one crate name" >&2
    exit 1
fi

if [ ! -s "$packages" ]; then
    echo "FAIL $package_list: no packages selected" >&2
    exit 1
fi

duplicates="$(sort "$packages" | uniq -d)"
if [ -n "$duplicates" ]; then
    echo "FAIL $package_list: duplicate package(s):" >&2
    printf '%s\n' "$duplicates" | sed 's/^/    /' >&2
    exit 1
fi

metadata="$(cargo metadata --locked --no-deps --format-version 1)"
printf '%s\n' "$metadata" | jq -r '
    .workspace_members as $members
    | .packages[]
    | select(.id as $id | $members | index($id))
    | .name
' | sort > "$workspace_packages"

unknown="$(grep -Fvx -f "$workspace_packages" "$packages" || true)"
if [ -n "$unknown" ]; then
    echo "FAIL $package_list: unknown workspace package(s):" >&2
    printf '%s\n' "$unknown" | sed 's/^/    /' >&2
    exit 1
fi

awk '{ print $1 "\t" NR }' "$packages" > "$positions"
fail=0

position_of() {
    awk -F '\t' -v package="$1" '$1 == package { print $2; exit }' "$positions"
}

while IFS=$'\t' read -r name manifest version license publish metadata_complete; do
    selected=false
    if grep -Fxq "$name" "$packages"; then
        selected=true
    fi

    if [ "$selected" = true ]; then
        if [ "$publish" != 'crates-io' ]; then
            echo "FAIL $name: selected package must resolve publish = [\"crates-io\"]"
            fail=1
        fi
        if ! grep -Fqx 'publish = ["crates-io"]' "$manifest"; then
            echo "FAIL $manifest: selected package must opt in explicitly"
            fail=1
        fi
    else
        if [ -n "$publish" ]; then
            echo "FAIL $name: package is publishable but absent from $package_list"
            fail=1
        fi
        if ! grep -Fqx 'publish.workspace = true' "$manifest"; then
            echo "FAIL $manifest: unselected package must inherit the publish=false default"
            fail=1
        fi
    fi

    if [ "$version" != "$expected_version" ]; then
        echo "FAIL $name: version $version does not match $expected_version"
        fail=1
    fi
    if [ "$license" != 'AGPL-3.0-only' ]; then
        echo "FAIL $name: license is '$license', expected AGPL-3.0-only"
        fail=1
    fi
    if [ "$selected" = true ] && [ "$metadata_complete" != true ]; then
        echo "FAIL $name: incomplete public registry metadata"
        fail=1
    fi
done < <(printf '%s\n' "$metadata" | jq -r '
    .workspace_members as $members
    | .packages[]
    | select(.id as $id | $members | index($id))
    | [
        .name,
        .manifest_path,
        .version,
        (.license // ""),
        ((.publish // []) | join(",")),
        (
            (.repository // "") != ""
            and (.homepage // "") != ""
            and (.readme // "") != ""
            and (.description // "") != ""
            and (.rust_version // "") != ""
            and (.authors | length) > 0
            and (.keywords | length) > 0
            and (.categories | length) > 0
        )
    ]
    | @tsv
')

# A publishable package must follow all of its first-party dependencies. Cargo
# packages dev dependencies too, so include them; ignore only a crate's tested
# self-dependency, which introduces no registry ordering edge.
while IFS=$'\t' read -r package dependency kind; do
    [ "$package" != "$dependency" ] || continue
    grep -Fxq "$package" "$packages" || continue

    dependency_position="$(position_of "$dependency")"
    package_position="$(position_of "$package")"
    if [ -z "$dependency_position" ]; then
        echo "FAIL $package: $kind dependency $dependency is not publishable"
        fail=1
    elif [ "$dependency_position" -ge "$package_position" ]; then
        echo "FAIL $package: $kind dependency $dependency must appear earlier in $package_list"
        fail=1
    fi
done < <(printf '%s\n' "$metadata" | jq -r '
    .workspace_members as $members
    | [.packages[] | select(.id as $id | $members | index($id)) | .name] as $names
    | .packages[]
    | select(.id as $id | $members | index($id))
    | .name as $package
    | .dependencies[]
    | select(.name as $dependency | $names | index($dependency))
    | [$package, .name, (.kind // "normal")]
    | @tsv
')

if [ "$fail" -ne 0 ]; then
    echo "crates.io publication policy FAILED"
    exit 1
fi

echo "crates.io publication policy passed: $(wc -l < "$packages" | tr -d ' ') explicitly ordered packages"
