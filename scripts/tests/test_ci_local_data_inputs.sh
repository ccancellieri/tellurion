#!/bin/sh
# Data files in scripts/ are visible to the coverage audit but never treated
# as executable CI commands.

set -eu

fixture_root="$(mktemp -d)"
trap 'rm -rf "$fixture_root"' EXIT

output="$(./scripts/ci-local.sh --audit 2>&1)" || {
    printf '%s\n' "$output" >&2
    exit 1
}

expected='dependency-license-overrides.json      n/a        data input, never executed'
printf '%s\n' "$output" | grep -F "$expected" >/dev/null || {
    echo "FAIL: dependency license overrides were not classified as data" >&2
    exit 1
}

manual_tool='run-local.sh                           manual     developer tool; run outside ordinary CI'
printf '%s\n' "$output" | grep -F "$manual_tool" >/dev/null || {
    echo "FAIL: run-local was not classified as a manual developer tool" >&2
    exit 1
}

make_fixture() {
    fixture="$1"
    mkdir -p "$fixture/scripts" "$fixture/.github/workflows"
    cp scripts/ci-local.sh "$fixture/scripts/ci-local.sh"
    cp scripts/dependency-license-overrides.json \
        "$fixture/scripts/dependency-license-overrides.json"
    printf '#!/bin/sh\nexit 0\n' > "$fixture/scripts/check-test-feature-coverage.py"
    chmod +x "$fixture/scripts/ci-local.sh" \
        "$fixture/scripts/check-test-feature-coverage.py"
    : > "$fixture/.github/workflows/release-artifacts.yml"
}

invoked_fixture="$fixture_root/invoked"
make_fixture "$invoked_fixture"
printf 'run: ./scripts/check-test-feature-coverage.py\nrun: ./scripts/dependency-license-overrides.json\n' \
    > "$invoked_fixture/.github/workflows/ci.yml"
if (cd "$invoked_fixture" && ./scripts/ci-local.sh --audit) >/dev/null 2>&1; then
    echo "FAIL: audit accepted a workflow that executes a data input" >&2
    exit 1
fi

executable_fixture="$fixture_root/executable"
make_fixture "$executable_fixture"
printf 'run: ./scripts/check-test-feature-coverage.py\n' \
    > "$executable_fixture/.github/workflows/ci.yml"
chmod +x "$executable_fixture/scripts/dependency-license-overrides.json"
if (cd "$executable_fixture" && ./scripts/ci-local.sh --audit) >/dev/null 2>&1; then
    echo "FAIL: audit accepted an executable data input" >&2
    exit 1
fi

argument_fixture="$fixture_root/argument"
make_fixture "$argument_fixture"
printf 'run: ./scripts/check-test-feature-coverage.py --policy scripts/dependency-license-overrides.json\n' \
    > "$argument_fixture/.github/workflows/ci.yml"
if ! (cd "$argument_fixture" && ./scripts/ci-local.sh --audit) >/dev/null 2>&1; then
    echo "FAIL: audit rejected a data input passed as a command argument" >&2
    exit 1
fi

echo "PASS: CI guard audit accounts for data inputs without executing them"
