#!/usr/bin/env bash
# Network-free tests for canonical origin and successful-CI release bindings.

set -euo pipefail

fixture="$(mktemp -d)"
trap 'rm -rf "$fixture"' EXIT
fake_bin="$fixture/bin"
mkdir -p "$fake_bin"
real_git="$(command -v git)"

cat > "$fake_bin/git" <<SH
#!/usr/bin/env bash
set -euo pipefail
if [ "\${1-}" = remote ] && [ "\${2-}" = get-url ]; then
    printf '%s\n' "\${TEST_ORIGIN_URL}"
elif [ "\${1-}" = ls-remote ]; then
    printf '%s\trefs/heads/main\n' "\${TEST_REMOTE_MAIN}"
    printf '%s\trefs/tags/v0.5.0-rc.1\n' "\${TEST_REMOTE_TAG}"
else
    exec "$real_git" "\$@"
fi
SH

cat > "$fake_bin/curl" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
output=""
previous=""
for argument in "$@"; do
    [ "$previous" = --output ] && output="$argument"
    previous="$argument"
done
cp "$TEST_CI_RESPONSE" "$output"
SH
chmod +x "$fake_bin/"*

commit=1111111111111111111111111111111111111111
export PATH="$fake_bin:$PATH"
export TEST_ORIGIN_URL=https://github.com/ccancellieri/tellurion.git
export TEST_REMOTE_MAIN="$commit"
export TEST_REMOTE_TAG="$commit"
./scripts/verify-canonical-origin.sh 0.5.0-rc.1 "$commit" >/dev/null

expect_origin_failure() {
    if ./scripts/verify-canonical-origin.sh 0.5.0-rc.1 "$commit" >/dev/null 2>&1; then
        echo "FAIL: invalid canonical origin binding was accepted" >&2
        exit 1
    fi
}
TEST_ORIGIN_URL=https://example.invalid/tellurion.git expect_origin_failure
TEST_ORIGIN_URL=https://github.com/ccancellieri/tellurion.git \
    TEST_REMOTE_MAIN=2222222222222222222222222222222222222222 expect_origin_failure
TEST_REMOTE_MAIN="$commit" TEST_REMOTE_TAG=3333333333333333333333333333333333333333 \
    expect_origin_failure

write_ci_response() {
    local event="$1" conclusion="$2" path="$3" sha="$4"
    printf '{"workflow_runs":[{"head_sha":"%s","head_branch":"main","event":"%s","status":"completed","conclusion":"%s","path":"%s","repository":{"full_name":"ccancellieri/tellurion"},"head_repository":{"full_name":"ccancellieri/tellurion"}}]}\n' \
        "$sha" "$event" "$conclusion" "$path" > "$fixture/ci.json"
    export TEST_CI_RESPONSE="$fixture/ci.json"
}
write_ci_response push success .github/workflows/ci.yml "$commit"
./scripts/verify-canonical-ci.sh "$commit" >/dev/null

expect_ci_failure() {
    if ./scripts/verify-canonical-ci.sh "$commit" >/dev/null 2>&1; then
        echo "FAIL: invalid canonical CI binding was accepted" >&2
        exit 1
    fi
}
write_ci_response workflow_dispatch success .github/workflows/ci.yml "$commit"
expect_ci_failure
write_ci_response push failure .github/workflows/ci.yml "$commit"
expect_ci_failure
write_ci_response push success .github/workflows/other.yml "$commit"
expect_ci_failure
write_ci_response push success .github/workflows/ci.yml 4444444444444444444444444444444444444444
expect_ci_failure

echo "canonical origin and CI binding tests passed"
