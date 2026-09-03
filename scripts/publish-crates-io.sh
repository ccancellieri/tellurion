#!/usr/bin/env bash
# Publish Tellurion crates in dependency order, or validate a safe continuation
# after a partially successful immutable registry upload.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR/.."

mode=""
version=""
commit=""
resume_from=""
registry="crates-io"
while [ "$#" -gt 0 ]; do
    case "$1" in
        --preflight|--execute|--bootstrap)
            [ -z "$mode" ] || { echo "FAIL: choose exactly one mode" >&2; exit 2; }
            mode="$1"
            shift
            ;;
        --version|--commit|--resume-from|--registry)
            [ "$#" -ge 2 ] || { echo "FAIL: missing value for $1" >&2; exit 2; }
            case "$1" in
                --version) version="$2" ;;
                --commit) commit="$2" ;;
                --resume-from) resume_from="$2" ;;
                --registry) registry="$2" ;;
            esac
            shift 2
            ;;
        *)
            echo "FAIL: unknown argument: $1" >&2
            exit 2
            ;;
    esac
done

[ "$mode" = --preflight ] || [ "$mode" = --execute ] || [ "$mode" = --bootstrap ] || {
    echo "FAIL: --preflight, --execute, or --bootstrap is required" >&2
    exit 2
}
[ "$registry" = crates-io ] || {
    echo "FAIL: only the crates-io registry is permitted" >&2
    exit 2
}
if { [ "$mode" = --execute ] || [ "$mode" = --bootstrap ]; } && [ -z "${CARGO_REGISTRY_TOKEN:-}" ]; then
    echo "FAIL: the short-lived crates.io token is missing" >&2
    exit 1
fi
if [ "$mode" = --bootstrap ]; then
    [ "${GITHUB_ACTIONS:-false}" != true ] || {
        echo "FAIL: first-publication bootstrap is forbidden in GitHub Actions" >&2
        exit 1
    }
    [ "${TELLURION_BOOTSTRAP_CONFIRM:-}" = "publish first crates for $version from $commit" ] || {
        echo "FAIL: set the exact TELLURION_BOOTSTRAP_CONFIRM value documented in the runbook" >&2
        exit 1
    }
fi

./scripts/verify-crates-io-release.sh "$version" "$commit"
./scripts/audit-license-policy.sh
./scripts/audit-publication-license.sh
./scripts/audit-crates-io-policy.sh
./scripts/check-crates-io-release-readiness.sh
if [ "$mode" != --preflight ]; then
    ./scripts/verify-canonical-origin.sh "$version" "$commit"
    ./scripts/verify-canonical-ci.sh "$commit"
fi

package_list="release/crates-io-packages.txt"
packages=()
while IFS= read -r package; do
    packages+=("$package")
done < <(awk '!/^[[:space:]]*($|#)/ { print $1 }' "$package_list")
[ "${#packages[@]}" -eq 27 ] || {
    echo "FAIL: expected exactly 27 ordered crates, found ${#packages[@]}" >&2
    exit 1
}

# Package the complete workspace graph before inspecting individual registry
# versions. Cargo can resolve path+version dependencies from this temporary
# workspace registry even when this exact version has not been published yet.
archive_dir="target/package"
for package in "${packages[@]}"; do
    rm -f "$archive_dir/$package-$version.crate"
done
cargo +1.97.1 package --workspace --locked --no-verify
for package in "${packages[@]}"; do
    archive="$archive_dir/$package-$version.crate"
    [ -f "$archive" ] || {
        echo "FAIL: workspace packaging did not create $archive" >&2
        exit 1
    }
done

resume_index=0
if [ -n "$resume_from" ]; then
    resume_index=-1
    for index in "${!packages[@]}"; do
        if [ "${packages[$index]}" = "$resume_from" ]; then
            resume_index="$index"
            break
        fi
    done
    [ "$resume_index" -ge 0 ] || {
        echo "FAIL: resume crate is not in the publication allowlist: $resume_from" >&2
        exit 1
    }
fi

user_agent='tellurion-crates-publisher/0.5 (+https://github.com/ccancellieri/tellurion)'
expected_owner='ccancellieri'
http_get() {
    local url="$1"
    local output="$2"
    curl --silent --show-error --location --retry 3 \
        --user-agent "$user_agent" --output "$output" --write-out '%{http_code}' "$url"
}

command -v jq >/dev/null 2>&1 || {
    echo "FAIL: crates.io ownership verification requires jq" >&2
    exit 1
}

# Trusted Publishing cannot create a crate. Resolve every name and verify every
# existing owner before any possible upload, so the sequence fails atomically
# when a name has been claimed by another account.
missing_names=()
probe="$(mktemp)"
trap 'rm -f "$probe"' EXIT
for package in "${packages[@]}"; do
    status="$(http_get "https://crates.io/api/v1/crates/$package" "$probe")"
    case "$status" in
        200)
            owner_status="$(http_get "https://crates.io/api/v1/crates/$package/owners" "$probe")"
            [ "$owner_status" = 200 ] || {
                echo "FAIL: crates.io returned HTTP $owner_status while checking owners for $package" >&2
                exit 1
            }
            jq -e --arg owner "$expected_owner" '
                if ((.users | type) == "array" and (.teams | type) == "array")
                then any((.users + .teams)[]?; .login? == $owner)
                else false
                end
            ' "$probe" >/dev/null || {
                echo "FAIL: $package is not owned by expected crates.io account $expected_owner" >&2
                exit 1
            }
            ;;
        404) missing_names+=("$package") ;;
        *) echo "FAIL: crates.io returned HTTP $status while checking $package" >&2; exit 1 ;;
    esac
done
if [ "${#missing_names[@]}" -ne 0 ] && [ "$mode" != --bootstrap ]; then
    echo "FAIL: Trusted Publishing cannot perform these first publications:" >&2
    printf '  %s\n' "${missing_names[@]}" >&2
    echo "Bootstrap them manually, then configure this workflow as their trusted publisher." >&2
    exit 1
fi

for index in "${!packages[@]}"; do
    package="${packages[$index]}"
    archive="$archive_dir/$package-$version.crate"
    remote="$(mktemp)"

    status="$(http_get "https://crates.io/api/v1/crates/$package/$version/download" "$remote")"
    if [ "$status" = 200 ]; then
        if ! cmp -s "$archive" "$remote"; then
            echo "FAIL: crates.io already has different bytes for $package $version" >&2
            exit 1
        fi
        echo "verified existing $package $version; skipping immutable upload"
        rm -f "$remote"
        continue
    fi
    rm -f "$remote"
    [ "$status" = 404 ] || {
        echo "FAIL: crates.io returned HTTP $status while checking $package $version" >&2
        exit 1
    }

    if [ "$index" -lt "$resume_index" ]; then
        echo "FAIL: $package $version is missing before requested resume point $resume_from" >&2
        exit 1
    fi
    if [ "$mode" = --preflight ]; then
        echo "would publish $package $version"
        continue
    fi

    echo "publishing $package $version"
    publish_status=0
    cargo +1.97.1 publish --locked --registry crates-io -p "$package" || publish_status=$?

    # Cargo may time out while the accepted upload is becoming visible. Always
    # resolve that ambiguity from the immutable registry before stopping.
    verified=false
    for attempt in 1 2 3 4 5 6 7 8 9 10 11 12; do
        remote="$(mktemp)"
        status="$(http_get "https://crates.io/api/v1/crates/$package/$version/download" "$remote")"
        if [ "$status" = 200 ]; then
            if cmp -s "$archive" "$remote"; then
                verified=true
                rm -f "$remote"
                break
            fi
            rm -f "$remote"
            echo "FAIL: published $package $version differs from the local archive" >&2
            exit 1
        fi
        rm -f "$remote"
        [ "$status" = 404 ] || {
            echo "FAIL: crates.io returned HTTP $status while verifying $package $version" >&2
            exit 1
        }
        sleep 5
    done
    if [ "$verified" != true ]; then
        echo "FAIL: could not verify $package $version after cargo exit $publish_status" >&2
        echo "Rerun with --resume-from $package; existing byte-identical versions are skipped." >&2
        exit 1
    fi
    if [ "$publish_status" -ne 0 ]; then
        echo "STOP: Cargo returned $publish_status even though $package $version is now byte-identical on crates.io." >&2
        echo "Rerun the same commit with --resume-from $package; do not advance automatically." >&2
        exit 1
    fi
done

echo "crates.io publication sequence complete for $version"
