#!/usr/bin/env bash
# Proves the two published binaries compile from their packaged source without
# a workspace or a crates.io connection.  The Cargo lockfile in an archive
# records first-party dependencies as workspace sources, so the temporary
# `patch.crates-io` entries deliberately redirect every published first-party
# dependency to its own unpacked `.crate` archive.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

# shellcheck source=scripts/workspace-version.sh
. "$ROOT/scripts/workspace-version.sh"

version="$(workspace_version)"
test_dir="$(mktemp -d "${TMPDIR:-/tmp}/tellurion-package-install.XXXXXX")"
trap 'rm -rf "$test_dir"' EXIT

target_dir="$test_dir/target"
packages_dir="$test_dir/packages"
mkdir -p "$packages_dir"

# The complete public set is packaged once so every first-party dependency can
# be resolved from the exact source archive that publication would upload.
CARGO_NET_OFFLINE=true CARGO_TARGET_DIR="$target_dir" \
    cargo package --quiet --workspace --allow-dirty --locked --offline --no-verify

patch_args=()
while IFS= read -r package; do
    case "$package" in
        ''|'#'*) continue ;;
    esac

    archive="$target_dir/package/$package-$version.crate"
    [ -f "$archive" ] || {
        echo "FAIL: workspace packaging did not create $archive" >&2
        exit 1
    }
    tar -xzf "$archive" -C "$packages_dir"
    patch_args+=(
        --config
        "patch.crates-io.\"$package\".path=\"$packages_dir/$package-$version\""
    )
done < release/crates-io-packages.txt

check_archive() {
    local package="$1"
    local manifest="$packages_dir/$package-$version/Cargo.toml"

    [ -f "$manifest" ] || {
        echo "FAIL: $package archive did not contain Cargo.toml" >&2
        exit 1
    }

    # `--offline` is the boundary: Cargo may read the already-provisioned
    # registry cache for third-party crates, but it cannot contact crates.io.
    # Do not pass `--locked`: rewriting a packaged workspace dependency to the
    # unpacked archive changes only its temporary lockfile source, which Cargo
    # correctly records while retaining the archive's locked dependency set.
    CARGO_NET_OFFLINE=true CARGO_TARGET_DIR="$target_dir" \
        cargo check --quiet --manifest-path "$manifest" --offline "${patch_args[@]}"
}

check_archive tellurion
check_archive tellurion-ingest

echo 'Packaged server and ingest archives compile offline outside the workspace'
