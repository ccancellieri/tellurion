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

# MAJOR.MINOR.PATCH only. A version that cannot appear in a tag or a filename
# unchanged is refused at the source rather than sanitised downstream.
TELLURION_SEMVER_PATTERN='^[0-9]+\.[0-9]+\.[0-9]+$'

is_semver() {
    printf '%s' "${1-}" | grep -Eq "$TELLURION_SEMVER_PATTERN"
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
        echo "FAIL: workspace version '$version' is not MAJOR.MINOR.PATCH" >&2
        return 1
    fi

    printf '%s\n' "$version"
}
