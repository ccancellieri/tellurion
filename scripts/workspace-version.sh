# Sourced by the release, audit, and guard scripts that need the project
# version. `[workspace.package] version` in the root Cargo.toml is the single
# source of truth: every derived mention -- the first-party dependency pins,
# the release archive names, the install guide's asset table -- must be
# computed from it rather than repeat it, so that one edit moves the whole
# release.
#
# This reads the manifest with awk rather than `cargo metadata` on purpose.
# `scripts/check-release-workflow.sh` is a policy guard that has to run on a
# bare runner with no Cargo registry and no network; making it shell out to
# Cargo would turn a missing toolchain into a fake contract violation (the
# same failure mode `rg-compat.sh` removes for ripgrep).

# A strict SemVer core, optionally followed by the one prerelease form that
# Tellurion cuts. A version that cannot appear in a tag or a filename unchanged
# is refused at the source rather than sanitised downstream.
TELLURION_SEMVER_PATTERN='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-rc\.(0|[1-9][0-9]*))?$'

is_semver() {
    printf '%s' "${1-}" | grep -Eq "$TELLURION_SEMVER_PATTERN"
}

# Returns success only when target advances current. Within one numeric base,
# release candidates precede the final release and their numeric suffixes are
# ordered increasingly.
is_forward_release_version() {
    local current="$1"
    local target="$2"
    local current_base target_base current_rc target_rc
    local current_major current_minor current_patch
    local target_major target_minor target_patch

    is_semver "$current" && is_semver "$target" || return 1

    current_base="${current%%-*}"
    target_base="${target%%-*}"
    IFS=. read -r current_major current_minor current_patch <<<"$current_base"
    IFS=. read -r target_major target_minor target_patch <<<"$target_base"

    if [ "$target_major" -gt "$current_major" ] ||
        { [ "$target_major" -eq "$current_major" ] && [ "$target_minor" -gt "$current_minor" ]; } ||
        { [ "$target_major" -eq "$current_major" ] && [ "$target_minor" -eq "$current_minor" ] &&
            [ "$target_patch" -gt "$current_patch" ]; }; then
        return 0
    fi
    if [ "$target_major" -ne "$current_major" ] ||
        [ "$target_minor" -ne "$current_minor" ] ||
        [ "$target_patch" -ne "$current_patch" ]; then
        return 1
    fi

    current_rc="${current#*-rc.}"
    target_rc="${target#*-rc.}"
    [ "$current_rc" != "$current" ] || current_rc=""
    [ "$target_rc" != "$target" ] || target_rc=""

    if [ -z "$current_rc" ]; then
        return 1
    fi
    if [ -z "$target_rc" ]; then
        return 0
    fi
    [ "$target_rc" -gt "$current_rc" ]
}

# Prints the workspace version on stdout, or explains on stderr why it could
# not and returns non-zero. Never prints a fallback: a caller that cannot
# learn the version must fail, not guess one.
workspace_version() {
    local manifest="${1:-Cargo.toml}"
    local version

    if [ ! -f "$manifest" ]; then
        echo "FAIL: cannot read the workspace version: missing $manifest" >&2
        return 1
    fi

    version="$(awk '
        $0 == "[workspace.package]" { in_workspace_package = 1; next }
        in_workspace_package && /^\[/ { exit }
        in_workspace_package && /^version[[:space:]]*=/ {
            if (split($0, field, "\"") >= 2) {
                print field[2]
                exit
            }
        }
    ' "$manifest")"

    if [ -z "$version" ]; then
        echo "FAIL: $manifest has no [workspace.package] version" >&2
        return 1
    fi

    if ! is_semver "$version"; then
        echo "FAIL: workspace version '$version' is not MAJOR.MINOR.PATCH or MAJOR.MINOR.PATCH-rc.N" >&2
        return 1
    fi

    printf '%s\n' "$version"
}
