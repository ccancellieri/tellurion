#!/usr/bin/env bash
# Network-free behavioral tests for package-graph preflight and safe resume.

set -euo pipefail

fixture="$(mktemp -d)"
trap 'rm -rf "$fixture"' EXIT
repo="$fixture/repo"
state="$fixture/registry"
fake_bin="$fixture/bin"
mkdir -p "$repo/scripts" "$repo/release" "$state/names" "$state/versions" "$fake_bin"
cp scripts/publish-crates-io.sh scripts/verify-crates-io-release.sh \
    scripts/workspace-version.sh "$repo/scripts/"
printf '[workspace.package]\nversion = "0.5.0-rc.1"\n' > "$repo/Cargo.toml"
printf '/target/\n' > "$repo/.gitignore"
for number in $(seq -w 1 27); do
    printf 'crate-%s\n' "$number" >> "$repo/release/crates-io-packages.txt"
    : > "$state/names/crate-$number"
done
for gate in audit-license-policy audit-publication-license audit-crates-io-policy \
    verify-canonical-origin verify-canonical-ci; do
    printf '#!/usr/bin/env bash\nexit 0\n' > "$repo/scripts/$gate.sh"
done

cat > "$fake_bin/cargo" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
action="${2-}"
package=""
previous=""
for argument in "$@"; do
    [ "$previous" = -p ] && package="$argument"
    previous="$argument"
done
case "$action" in
    package)
        count=0
        [ -f "$TEST_STATE/package-count" ] && count="$(cat "$TEST_STATE/package-count")"
        printf '%s\n' "$((count + 1))" > "$TEST_STATE/package-count"
        mkdir -p target/package
        while IFS= read -r name; do
            [ "$name" = "${TEST_OMIT_PACKAGE:-}" ] || printf '%s\n' "$name" > "target/package/$name-$TEST_VERSION.crate"
        done < release/crates-io-packages.txt
        ;;
    publish)
        cp "target/package/$package-$TEST_VERSION.crate" "$TEST_STATE/versions/$package"
        printf '%s\n' "$package" >> "$TEST_STATE/published"
        [ "$package" != "${TEST_FAIL_PUBLISH:-}" ] || exit 101
        ;;
    *) exit 2 ;;
esac
SH

cat > "$fake_bin/curl" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
output=""
previous=""
url="${!#}"
for argument in "$@"; do
    [ "$previous" = --output ] && output="$argument"
    previous="$argument"
done
path="${url#https://crates.io/api/v1/crates/}"
package="${path%%/*}"
if [ "$path" = "$package" ]; then
    [ -f "$TEST_STATE/names/$package" ] && printf 200 || printf 404
elif [ -f "$TEST_STATE/versions/$package" ]; then
    cp "$TEST_STATE/versions/$package" "$output"
    printf 200
else
    printf 404
fi
SH

chmod +x "$repo/scripts/"*.sh "$fake_bin/"*
git -C "$repo" init -q
git -C "$repo" config user.name 'Tellurion publisher test'
git -C "$repo" config user.email 'publisher-test.invalid'
git -C "$repo" add .
git -C "$repo" commit -qm 'Create publisher fixture'
commit="$(git -C "$repo" rev-parse HEAD)"
git -C "$repo" tag v0.5.0-rc.1

run_publisher() {
    (cd "$repo" && PATH="$fake_bin:$PATH" TEST_STATE="$state" \
        TEST_VERSION=0.5.0-rc.1 CARGO_REGISTRY_TOKEN=test \
        ./scripts/publish-crates-io.sh "$@")
}

run_publisher --preflight --version 0.5.0-rc.1 --commit "$commit" >/dev/null
[ "$(cat "$state/package-count")" -eq 1 ] && [ ! -e "$state/published" ]

run_publisher --execute --version 0.5.0-rc.1 --commit "$commit" >/dev/null
[ "$(wc -l < "$state/published" | tr -d ' ')" -eq 27 ]
run_publisher --execute --version 0.5.0-rc.1 --commit "$commit" >/dev/null
[ "$(wc -l < "$state/published" | tr -d ' ')" -eq 27 ]
[ "$(cat "$state/package-count")" -eq 3 ]

rm "$state/versions/crate-05" "$state/versions/crate-06"
if TEST_FAIL_PUBLISH=crate-05 run_publisher --execute --version 0.5.0-rc.1 \
    --commit "$commit" --resume-from crate-05 >/dev/null 2>&1; then
    echo "FAIL: ambiguous Cargo failure advanced publication" >&2
    exit 1
fi
[ -f "$state/versions/crate-05" ] && [ ! -f "$state/versions/crate-06" ]

rm -rf "$repo/target"
if TEST_OMIT_PACKAGE=crate-10 run_publisher --preflight --version 0.5.0-rc.1 \
    --commit "$commit" >/dev/null 2>&1; then
    echo "FAIL: incomplete workspace package graph was accepted" >&2
    exit 1
fi

echo "crates.io package graph and resume tests passed"
