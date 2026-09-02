#!/usr/bin/env bash
# Checks that publication-facing legal surfaces name the canonical AGPL release.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="${AUDIT_ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"
cd "$ROOT"

for document in Cargo.toml LICENSE README.md docs/licensing.md docs/maturity.md COMMERCIAL-LICENSE.md CLA.md docs/quickstart/install.md; do
    if [ ! -f "$document" ]; then
        echo "ERROR: missing required legal surface: $document" >&2
        exit 2
    fi
done

version="$(awk '
    $0 == "[workspace.package]" { in_workspace_package = 1; next }
    in_workspace_package && /^\[/ { exit }
    in_workspace_package && /^version[[:space:]]*=/ {
        sub(/^[^"]*"/, ""); sub(/".*/, ""); print; exit
    }
' Cargo.toml)"
if [ -z "$version" ]; then
    echo "ERROR: unable to derive release metadata" >&2
    exit 2
fi

fail=0
expected_license_sha256='0d96a4ff68ad6d4b6f1f30f713b18d5184912ba8dd389f86aa7710db079abcb0'
if command -v sha256sum >/dev/null 2>&1; then
    actual_license_sha256="$(sha256sum LICENSE | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
    actual_license_sha256="$(shasum -a 256 LICENSE | awk '{print $1}')"
else
    echo "ERROR: sha256sum or shasum is required to verify LICENSE" >&2
    exit 2
fi

if [ "$actual_license_sha256" != "$expected_license_sha256" ]; then
    echo "FAIL LICENSE: file does not match the canonical AGPL version 3 text"
    fail=1
fi

if ! awk '
    $0 == "[workspace.package]" { in_workspace_package = 1; next }
    in_workspace_package && /^\[/ { exit }
    in_workspace_package && $0 == "license = \"AGPL-3.0-only\"" { found = 1 }
    END { exit(found ? 0 : 1) }
' Cargo.toml; then
    echo "FAIL Cargo.toml: workspace licence is not AGPL-3.0-only"
    fail=1
fi

for document in README.md docs/licensing.md docs/maturity.md COMMERCIAL-LICENSE.md CLA.md docs/quickstart/install.md; do
    if ! grep -Fq "Tellurion $version" "$document"; then
        echo "FAIL $document: release surface does not match workspace version"
        fail=1
    fi

    for obsolete in 'BUSL-1.1' 'Business Source License' 'Change Date'; do
        if grep -Fq "$obsolete" "$document"; then
            echo "FAIL $document: obsolete licensing term remains: $obsolete"
            fail=1
        fi
    done
done

for document in README.md docs/licensing.md COMMERCIAL-LICENSE.md CLA.md; do
    if ! grep -Fq 'AGPL-3.0-only' "$document"; then
        echo "FAIL $document: licensing surface does not name AGPL-3.0-only"
        fail=1
    fi
done

if [ "$fail" -ne 0 ]; then
    exit 1
fi

echo "publication license audit passed"
