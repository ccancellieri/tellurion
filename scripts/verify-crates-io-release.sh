#!/usr/bin/env bash
# Verify that the current checkout, workspace version, and pre-existing tag are
# one immutable crates.io publication identity.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR/.."

# shellcheck source=scripts/workspace-version.sh
. "$SCRIPT_DIR/workspace-version.sh"

version="${1-}"
commit="${2-}"

is_semver "$version" || {
    echo "FAIL: version must be MAJOR.MINOR.PATCH or MAJOR.MINOR.PATCH-rc.N" >&2
    exit 1
}
printf '%s' "$commit" | grep -Eq '^[0-9a-f]{40}$' || {
    echo "FAIL: commit must be a lowercase 40-character Git object ID" >&2
    exit 1
}

[ "$(workspace_version)" = "$version" ] || {
    echo "FAIL: requested version does not match [workspace.package]" >&2
    exit 1
}
[ "$(git rev-parse HEAD)" = "$commit" ] || {
    echo "FAIL: requested commit does not match HEAD" >&2
    exit 1
}
[ -z "$(git status --porcelain --untracked-files=normal)" ] || {
    echo "FAIL: publication checkout is not clean" >&2
    exit 1
}

tag="v$version"
git rev-parse --verify --quiet "refs/tags/$tag^{commit}" >/dev/null || {
    echo "FAIL: required pre-existing tag $tag is missing" >&2
    exit 1
}
[ "$(git rev-parse "refs/tags/$tag^{commit}")" = "$commit" ] || {
    echo "FAIL: tag $tag does not point to requested commit" >&2
    exit 1
}

echo "crates.io release identity verified: $tag -> $commit"
