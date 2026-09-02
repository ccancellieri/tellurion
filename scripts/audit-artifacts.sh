#!/usr/bin/env bash
# Audits what `cargo package` would actually ship for every first-party
# workspace crate, including crates deliberately marked `publish = false`.
# For each crate this
# checks three things:
#
#   1. The license text ships in the archive (a `LICENSE` entry in
#      `cargo package --list`) -- the SPDX `license` field in Cargo.toml is
#      metadata, not the license text itself; crates.io only guarantees the
#      text ships if a LICENSE* file is actually in the package.
#   2. No path in the listing looks like a leak: an absolute home-directory
#      path, a hidden dot-directory cargo didn't put there itself, or a
#      secret/key-looking filename.
#   3. Every normal/build dependency needed to actually compile the
#      published crate resolves from crates.io -- a git or
#      out-of-workspace path dependency would build fine here and break for
#      anyone who isn't on this machine.
#
# Usage: ./scripts/audit-artifacts.sh
#
# Requires: bash, cargo, jq.
#
# Exit code 0 = every first-party crate passed every check. Non-zero = at
# least one check failed; the FAIL lines above the summary say which one.

set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR/.."

# shellcheck source=scripts/workspace-version.sh
. "$SCRIPT_DIR/workspace-version.sh"

# The expected version is the one the workspace declares, not a literal this
# audit would have to be edited to keep in step with a release.
expected_version="$(workspace_version)" || exit 1

CARGO_BIN="${CARGO:-cargo}"

for tool in "$CARGO_BIN" jq; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "ERROR: required tool '$tool' not found on PATH" >&2
        exit 1
    }
done

fail=0

metadata="$("$CARGO_BIN" metadata --no-deps --format-version=1)"
workspace_root="$(printf '%s\n' "$metadata" | jq -r '.workspace_root')"

# `publish = false` prevents publication but does not make a first-party
# package exempt from release auditing. Select workspace members explicitly;
# `metadata.packages` can also include non-member path packages.
crates="$(printf '%s\n' "$metadata" | jq -r '
    .workspace_members as $members
    | .packages[]
    | select(.id as $id | $members | index($id))
    | .name
')"

if [ -z "$crates" ]; then
    echo "ERROR: no first-party workspace crates found in metadata" >&2
    exit 1
fi

for crate in $crates; do
    echo "== $crate =="

    crate_version="$(printf '%s\n' "$metadata" | jq -r --arg crate "$crate" \
        '.packages[] | select(.name == $crate) | .version')"
    crate_license="$(printf '%s\n' "$metadata" | jq -r --arg crate "$crate" \
        '.packages[] | select(.name == $crate) | .license // empty')"

    if [ "$crate_version" != "$expected_version" ]; then
        echo "FAIL $crate: expected version $expected_version, found '$crate_version'"
        fail=1
    fi
    if [ "$crate_license" != "AGPL-3.0-only" ]; then
        echo "FAIL $crate: expected license AGPL-3.0-only, found '$crate_license'"
        fail=1
    fi

    if ! listing="$("$CARGO_BIN" package --list -p "$crate" --allow-dirty 2>&1)"; then
        echo "FAIL $crate: cargo package --list failed"
        echo "$listing" | sed 's/^/    /'
        fail=1
        continue
    fi

    # 1. license text must ship in the archive.
    if ! printf '%s\n' "$listing" | grep -qx 'LICENSE'; then
        echo "FAIL $crate: no LICENSE file in the package listing"
        fail=1
    fi

    # 2. no leaked absolute home path, unexpected hidden directory, or
    #    secret/key-looking file. `.cargo_vcs_info.json` is the one
    #    dot-file cargo always injects itself and is not a leak.
    leaks="$(printf '%s\n' "$listing" \
        | grep -v '^\.cargo_vcs_info\.json$' \
        | grep -iE '(^|/)\.[^/]+|^/Users/|^/home/|/Users/|/home/|(^|/)id_rsa|secret|credential|\.pem$|\.key$|\.p12$|\.pfx$|token' \
        || true)"
    if [ -n "$leaks" ]; then
        echo "FAIL $crate: leak-pattern path(s) in package listing:"
        printf '%s\n' "$leaks" | sed 's/^/    /'
        fail=1
    fi

    # 3. every normal/build dependency needed to compile the published
    #    crate must resolve from crates.io. `cargo tree`'s default `{p}`
    #    format only prints a parenthesized source for a non-default
    #    source (a path or a git URL) or a `(proc-macro)` marker; a bare
    #    "name vX.Y.Z" with nothing in parens is a plain crates.io
    #    dependency. Path dependencies on our own workspace members are
    #    expected (cargo resolves them to the published version at publish
    #    time) and show as an absolute path under $workspace_root.
    bad_sources="$("$CARGO_BIN" tree -p "$crate" -e normal,build --prefix none --no-dedupe 2>&1 \
        | sort -u \
        | grep -E '\([^)]*\)$' \
        | grep -v '(proc-macro)$' \
        | grep -vF "($workspace_root" \
        || true)"
    if [ -n "$bad_sources" ]; then
        echo "FAIL $crate: non-crates.io dependency source(s):"
        printf '%s\n' "$bad_sources" | sed 's/^/    /'
        fail=1
    fi

    echo "  ok: version/license, LICENSE present, no leak patterns, all deps from crates.io"
done

echo
if [ "$fail" -ne 0 ]; then
    echo "artifact audit FAILED"
    exit 1
fi

echo "artifact audit passed: all first-party crates clean"
