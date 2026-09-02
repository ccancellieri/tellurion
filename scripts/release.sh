#!/usr/bin/env bash
# Moves the Tellurion version, and nothing else.
#
# `[workspace.package] version` in the root Cargo.toml is the single source of
# truth (see scripts/workspace-version.sh). This script advances it and every
# mention that is *derived* from it -- the first-party dependency pins, the
# lockfile, the install guide's asset table, the Nomad example artifact URL,
# the CHANGELOG heading -- so that one command moves the whole release instead
# of a dozen hand edits drifting apart.
#
# What it deliberately does NOT do:
#
#   * It never publishes anything. It creates no GitHub Release, uploads no
#     asset, and pushes no branch or tag. Its only remote access is one
#     read-only `git ls-remote` to see whether the target tag has already been
#     cut somewhere this clone cannot see; that writes no ref here and sends
#     nothing. Public publication remains a future, separately permissioned
#     gate after the runbook's owner, legal, and evidence reviews; this
#     build-only workflow does not implement that gate. `--tag` creates a
#     *local* annotated tag and says so.
#   * It never changes licence terms. The repository-wide AGPL choice is
#     stable, while the plain-language release surfaces name the version they
#     describe. It updates only those version labels and then runs the
#     publication audit before it can commit or tag the result.
#   * It never rewrites a dated design record. Those describe what was decided
#     on the day they were written; moving their version numbers would falsify
#     history.
#
# Usage:
#   scripts/release.sh <major|minor|patch> [options]
#   scripts/release.sh --set <MAJOR.MINOR.PATCH> [options]
#
# Options:
#   --dry-run   Apply the bump, print the resulting diff, restore the tree.
#   --commit    Commit the bump, staging exactly the files it changed.
#   --tag       Create a local annotated tag vX.Y.Z. Requires --commit.
#   -h, --help  This text.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

# shellcheck source=scripts/workspace-version.sh
. "$SCRIPT_DIR/workspace-version.sh"

# Every failure names itself and the thing it objected to. A release mechanism
# that degrades quietly is worse than no release mechanism.
refuse() {
    echo "REFUSED: $*" >&2
    exit 1
}

usage() {
    sed -n '2,/^set -euo/p' "$0" | sed -e 's/^# \{0,1\}//' -e '$d'
}

bump_kind=""
target_version=""
dry_run=0
do_commit=0
do_tag=0

while [ "$#" -gt 0 ]; do
    case "$1" in
        major | minor | patch)
            [ -z "$bump_kind" ] || refuse "more than one bump kind given: $bump_kind and $1"
            [ -z "$target_version" ] || refuse "--set and a bump kind ($1) are mutually exclusive"
            bump_kind="$1"
            ;;
        --set)
            shift || true
            [ "${1-}" != "" ] || refuse "--set needs a MAJOR.MINOR.PATCH argument"
            [ -z "$bump_kind" ] || refuse "--set and a bump kind ($bump_kind) are mutually exclusive"
            target_version="$1"
            ;;
        --dry-run) dry_run=1 ;;
        --commit) do_commit=1 ;;
        --tag) do_tag=1 ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            refuse "unknown argument '$1'; expected major, minor, patch, --set, --dry-run, --commit, --tag"
            ;;
    esac
    shift
done

if [ -z "$bump_kind" ] && [ -z "$target_version" ]; then
    usage >&2
    refuse "no bump kind given; expected major, minor, patch, or --set MAJOR.MINOR.PATCH"
fi
if [ "$dry_run" -eq 1 ] && { [ "$do_commit" -eq 1 ] || [ "$do_tag" -eq 1 ]; }; then
    refuse "--dry-run cannot be combined with --commit or --tag"
fi
if [ "$do_tag" -eq 1 ] && [ "$do_commit" -eq 0 ]; then
    refuse "--tag requires --commit: a tag must point at the commit that carries the bump"
fi

for tool in git cargo awk perl; do
    command -v "$tool" >/dev/null 2>&1 || refuse "required tool '$tool' is not on PATH"
done

git rev-parse --git-dir >/dev/null 2>&1 || refuse "$REPO_ROOT is not a Git repository"

dirty="$(git status --porcelain)"
if [ -n "$dirty" ]; then
    printf '%s\n' "$dirty" >&2
    refuse "the working tree is dirty; commit or stash the changes above before cutting a version"
fi

current_version="$(workspace_version)" || refuse "cannot read the current version from Cargo.toml"

current_major="${current_version%%.*}"
current_rest="${current_version#*.}"
current_minor="${current_rest%%.*}"
current_patch="${current_rest#*.}"

case "$bump_kind" in
    major) target_version="$((current_major + 1)).0.0" ;;
    minor) target_version="$current_major.$((current_minor + 1)).0" ;;
    patch) target_version="$current_major.$current_minor.$((current_patch + 1))" ;;
    "") ;;
esac

is_semver "$target_version" \
    || refuse "target version '$target_version' is not MAJOR.MINOR.PATCH"

# Strictly forward only. An equal or lower version would re-point an identity
# that may already have been built, tagged, or handed to someone.
target_major="${target_version%%.*}"
target_rest="${target_version#*.}"
target_minor="${target_rest%%.*}"
target_patch="${target_rest#*.}"
if [ "$target_major" -lt "$current_major" ] ||
    { [ "$target_major" -eq "$current_major" ] && [ "$target_minor" -lt "$current_minor" ]; } ||
    { [ "$target_major" -eq "$current_major" ] && [ "$target_minor" -eq "$current_minor" ] &&
        [ "$target_patch" -le "$current_patch" ]; }; then
    refuse "the version would not move forward: $current_version -> $target_version"
fi

target_tag="v$target_version"
if git rev-parse -q --verify "refs/tags/$target_tag" >/dev/null; then
    refuse "tag $target_tag already exists locally; a cut version is never re-pointed"
fi

# A clone that has never fetched tags has no local tags at all, which would
# silently turn the check above into a no-op -- and this repository's remote
# does carry tags its worktrees do not. So ask the remote directly.
# `ls-remote` is read-only: it pushes nothing and writes no ref here, so the
# script still cannot publish and still cannot change shared state. When the
# remote cannot be reached, say so rather than pretend the check ran.
remote="$(git remote | head -n 1)"
if [ -n "$remote" ]; then
    if remote_tag="$(git ls-remote --tags "$remote" "refs/tags/$target_tag" 2>/dev/null)"; then
        [ -z "$remote_tag" ] \
            || refuse "tag $target_tag already exists on '$remote'; a cut version is never re-pointed"
    else
        echo "NOTE: could not reach '$remote' to check whether $target_tag was already cut;" >&2
        echo "      only this clone's local tags were checked." >&2
    fi
fi

changelog="CHANGELOG.md"
[ -f "$changelog" ] || refuse "missing $changelog; a release needs an entry to promote"
grep -Fq '## [Unreleased]' "$changelog" \
    || refuse "$changelog has no '## [Unreleased]' section to promote to $target_version"

versioned_publication_surfaces=(
    README.md
    docs/licensing.md
    docs/maturity.md
    COMMERCIAL-LICENSE.md
    CLA.md
    docs/quickstart/install.md
)
changed_paths=(Cargo.toml Cargo.lock "$changelog" deploy/nomad/tellurion.nomad.hcl "${versioned_publication_surfaces[@]}")
for path in "${changed_paths[@]}"; do
    [ -f "$path" ] || refuse "expected to update $path, but it does not exist"
done

# The tree was verified clean above, so restoring these exact paths is exact.
# Any refusal from here on therefore leaves nothing half-bumped behind; a
# partially rewritten release is the silent degradation this avoids.
keep_changes=0
restore_tree() {
    [ "$keep_changes" -eq 1 ] || git checkout -- "${changed_paths[@]}" 2>/dev/null || true
    rm -f Cargo.toml.release-tmp "$changelog.release-tmp"
}
trap restore_tree EXIT INT TERM

echo "bumping $current_version -> $target_version"

# 1. The source of truth, and the first-party dependency pins that must equal
#    it (scripts/audit-license-policy.sh enforces that equality).
awk -v new_version="$target_version" '
    /^\[/ { section = $0 }
    section == "[workspace.package]" && /^version[[:space:]]*=/ {
        print "version = \"" new_version "\""
        next
    }
    section == "[workspace.dependencies]" && /^[A-Za-z0-9_-]+[[:space:]]*=[[:space:]]*\{[[:space:]]*path[[:space:]]*=[[:space:]]*"crates\// {
        sub(/version[[:space:]]*=[[:space:]]*"[0-9]+\.[0-9]+\.[0-9]+"/,
            "version = \"" new_version "\"")
    }
    { print }
' Cargo.toml >Cargo.toml.release-tmp
mv Cargo.toml.release-tmp Cargo.toml

check_version="$(workspace_version)" || refuse "the rewritten Cargo.toml no longer declares a usable version"
[ "$check_version" = "$target_version" ] \
    || refuse "Cargo.toml still declares $check_version after the rewrite"
# Scoped to the declaration and the first-party pins: a third-party crate may
# legitimately be pinned at a version string that once was ours.
stale_pins="$(awk -v old="version = \"$current_version\"" '
    /^\[/ { section = $0 }
    section == "[workspace.package]" && $0 == old { print FNR ": " $0 }
    section == "[workspace.dependencies]" && /path[[:space:]]*=[[:space:]]*"crates\// &&
        index($0, old) { print FNR ": " $0 }
' Cargo.toml)"
if [ -n "$stale_pins" ]; then
    printf '%s\n' "$stale_pins" >&2
    refuse "Cargo.toml still pins $current_version on the lines above"
fi

# 2. The lockfile. `--offline` keeps a version bump from turning into a
#    dependency update; `--workspace` keeps it from touching anything but our
#    own crates. Both matter for "no bump means byte-identical builds".
cargo update --workspace --offline --quiet \
    || refuse "cargo could not update Cargo.lock for $target_version"

# 3. Version-labelled public surfaces. These name the release that evaluators
#    receive; unlike dated design records, their labels must move with the
#    workspace version. The install guide's asset table describes exactly the
#    archives .github/workflows/release-artifacts.yml builds, and the Nomad
#    example names one of them.
current_pattern="${current_version//./\\.}"
for surface in "${versioned_publication_surfaces[@]}"; do
    perl -pi -e "s/$current_pattern/$target_version/g" "$surface"
    grep -Fq "Tellurion $target_version" "$surface" \
        || refuse "$surface does not name Tellurion $target_version after the rewrite"
done
perl -pi -e "s/tellurion-v$current_pattern-/tellurion-v$target_version-/g" deploy/nomad/tellurion.nomad.hcl
grep -Fq "v$target_version" docs/quickstart/install.md \
    || refuse "docs/quickstart/install.md does not mention v$target_version after the rewrite"

# 4. Promote the changelog's Unreleased section and open a fresh one.
awk -v heading="## [$target_version] - $(date -u +%Y-%m-%d)" '
    $0 == "## [Unreleased]" && !done {
        print "## [Unreleased]"
        print ""
        print heading
        done = 1
        next
    }
    { print }
' "$changelog" >"$changelog.release-tmp"
mv "$changelog.release-tmp" "$changelog"

# The guard is cheap and this is exactly the moment its subject matter moved.
bash "$SCRIPT_DIR/check-release-workflow.sh" >/dev/null \
    || refuse "the release workflow contract does not hold at $target_version"
bash "$SCRIPT_DIR/audit-publication-license.sh" >/dev/null \
    || refuse "the publication licence surfaces do not hold at $target_version"

echo
echo "updated:"
git --no-pager diff --stat -- "${changed_paths[@]}"

if [ "$dry_run" -eq 1 ]; then
    echo
    git --no-pager diff -- "${changed_paths[@]}"
    echo
    echo "dry run: restoring the working tree, nothing was kept"
    exit 0
fi

keep_changes=1

cat <<REVIEW

REVIEW REQUIRED -- not changed by this script:

  LICENSE and its per-crate copies   Canonical AGPL-3.0 text; normally unchanged.
  The version-labelled public surfaces were updated and audited automatically.

  docs/design/2026-07-17-*.md        Dated design records. Their version
  docs/design/2026-07-19-*.md        numbers are history, not configuration,
  docs/design/2026-07-23-*.md        and must stay as written.

Nothing was pushed and no release was created. This script only cuts a
version; publishing one remains a separate, ungranted decision.
REVIEW

if [ "$do_commit" -eq 1 ]; then
    git add -- "${changed_paths[@]}"
    git commit -q -m "Release $target_tag

Advance the workspace version and every mention derived from it."
    echo
    echo "committed $(git rev-parse --short HEAD) for $target_tag"
fi

if [ "$do_tag" -eq 1 ]; then
    git tag -a "$target_tag" -m "Tellurion $target_version"
    echo "created LOCAL annotated tag $target_tag at $(git rev-parse --short HEAD)"
    echo "it exists only in this clone; nothing was sent to a remote"
fi
