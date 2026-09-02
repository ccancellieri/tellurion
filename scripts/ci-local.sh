#!/usr/bin/env bash
# Runs, on this machine and with no GitHub runner involved, the same gates
# `.github/workflows/ci.yml` runs -- in the same order, with the same
# commands.
#
# Why this exists: every CI run on `main` has failed for days without a single
# job ever starting. The API says so plainly -- `runner_id: 0`, an empty
# `runner_name`, `billable.UBUNTU.total_ms: 0` across all twelve jobs, and a
# 404 for every job log. Nothing in the repository can fix that, and nothing
# in the repository should have to wait for it. A pipeline whose correctness
# can only be observed by a runner that never arrives is not a pipeline; this
# script is how the pipeline stays checkable in the meantime, and how a change
# to `ci.yml` gets verified before it is pushed rather than after.
#
# It is deliberately a mirror, not a second definition. When a job is added to
# `ci.yml`, a phase is added here with the same name and the same command, and
# `--audit` below fails until every guard script in `scripts/` is accounted
# for on one side or the other.
#
# Usage:
#   ./scripts/ci-local.sh              # every phase, in ci.yml's order
#   ./scripts/ci-local.sh fmt clippy   # only the named phases
#   ./scripts/ci-local.sh --list       # phase names and the job each mirrors
#   ./scripts/ci-local.sh --audit      # ONLY the guard-script coverage audit
#
# Only the first form runs this pipeline. `--audit` and a named-phase list are
# SUBSETS of it. `--audit` runs `phase_audit` alone, which asks whether every
# guard script and every feature-gated test target is REACHED by some CI
# invocation -- never whether any of them passes. It does not build, does not
# lint, does not test, and does not run a single leg of the feature matrix.
#
# That distinction is not pedantry, it is the defect this script was mis-used
# to hide. Every agent briefing in this campaign asked for `--audit`, read its
# green verdict as the pipeline being green, and so a test that could not pass
# in nine of the eleven feature-matrix legs survived five merges after the one
# that introduced it -- while `--audit` reported, correctly and uselessly, that
# something in CI did build that test file. A run that skips any phase now says
# so by name, before and after, and never ends on the unqualified `PASS` line a
# full run ends on.
#
# Exit code 0 = every phase that ran passed. Non-zero = at least one failed;
# the summary at the end names which.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT"

# The verification profile this campaign builds under. Set here rather than
# left to the caller so a local run cannot accidentally be a debuginfo-heavy
# one -- this workspace has filled a disk three times.
export CARGO_PROFILE_DEV_DEBUG="${CARGO_PROFILE_DEV_DEBUG:-0}"
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"
export CARGO_TERM_COLOR="${CARGO_TERM_COLOR:-always}"

RESULTS=()
FAILURES=0

phase_header() {
    printf '\n\033[1m=== %s\033[0m\n' "$1"
}

record() {
    local status="$1" name="$2"
    RESULTS+=("$status	$name")
    if [ "$status" != PASS ]; then
        FAILURES=$((FAILURES + 1))
    fi
}

# Runs one gate and records its verdict by name. Never aborts the run: the
# point of a local mirror is to learn everything that is broken in one pass,
# the way twelve parallel CI jobs would have told you.
gate() {
    local name="$1"
    shift
    phase_header "$name"
    printf '  $ %s\n' "$*"
    if "$@"; then
        record PASS "$name"
    else
        record FAIL "$name"
    fi
}

# --- preconditions, named up front ------------------------------------------
#
# Same rule the two smoke scripts already apply to themselves: a gate that
# fails for a reason unrelated to the change under test teaches people to
# re-run instead of to read. Each precondition below is refused BY NAME rather
# than left to surface as an assertion failure hundreds of lines later.

require_tool() {
    command -v "$1" >/dev/null 2>&1 || {
        printf 'REFUSED: %s is not on PATH, and %s\n' "$1" "$2" >&2
        exit 2
    }
}

# The live-database DSNs are required, never defaulted.
#
# BOTH of them. 165 tests in this workspace skip themselves -- and PASS -- when
# their DSN is unset, and they do not all read the same variable: 152 read
# `TELLURION_TEST_DATABASE_URL`, while 13 (`binary.rs`'s entire real-binary
# lifecycle suite and `tellurion-ingest`'s create-tables checks) read plain
# `DATABASE_URL`. ci.yml's `test` job supplies both, so this mirror demands
# both -- requiring only the longer name reports a green suite while thirteen
# acceptance tests sit out, which is the same silent skip this script exists to
# make impossible, one level up.
#
# There is no fallback DSN here on purpose: guessing a connection string is
# inventing a default, and a wrong guess fails as an unreachable server rather
# than as the missing configuration it really is.
require_database_url() {
    local var
    for var in TELLURION_TEST_DATABASE_URL DATABASE_URL; do
        if [ -z "${!var:-}" ]; then
            cat >&2 <<EOF
REFUSED: $var is not set.

165 live tests in this workspace skip themselves -- and PASS -- when their DSN
is unset, so running the suite without one would report success for a suite
that never reached a database. Measured here: with no DSN the suite still exits
0 and still reports the identical 3397 passed. This script will not do that
silently, and it will not invent a DSN for you.

ci.yml's \`test\` job sets TELLURION_TEST_DATABASE_URL and DATABASE_URL both;
set both here too, e.g.

  export $var=postgres://USER:PASSWORD@HOST:PORT/DATABASE

and start the cluster if it is down (\`pg_ctlcluster 16 main start\`).
EOF
            exit 2
        fi
        if command -v pg_isready >/dev/null 2>&1; then
            pg_isready -d "${!var}" >/dev/null 2>&1 || {
                printf 'REFUSED: the server %s names is not accepting connections.\n' "$var" >&2
                printf 'Start it (pg_ctlcluster 16 main start) rather than reading the test\n' >&2
                printf 'results below as a regression in the change under test.\n' >&2
                exit 2
            }
        fi
    done
}

# --- phases, in ci.yml's own order ------------------------------------------

phase_fmt() { # ci.yml job: fmt
    cargo fmt --all -- --check
}

phase_clippy() { # ci.yml job: clippy
    cargo clippy --workspace --all-targets -- -D warnings
}

# ci.yml job: test. The `--nocapture` and the skip grep are not local
# embellishments -- they are what the `test` job itself does, for the reason
# spelled out in its own comment.
phase_test() {
    local log
    log="$(mktemp)"
    local status=0
    set -o pipefail
    cargo test --workspace --locked -- --nocapture 2>&1 | tee "$log" || status=$?
    if [ "$status" -ne 0 ]; then
        rm -f "$log"
        return "$status"
    fi
    # The bare `DATABASE_URL not set` is a substring of both skip messages the
    # workspace emits, so this one pattern catches all 165 -- see
    # `require_database_url` above for why matching only the longer name is a
    # silent skip of its own.
    local skipped
    skipped="$(grep -c 'DATABASE_URL not set' "$log" || true)"
    if [ "$skipped" -ne 0 ]; then
        printf 'FAIL: %s live test(s) skipped although a DSN was supplied.\n' "$skipped" >&2
        grep 'DATABASE_URL not set' "$log" >&2
        rm -f "$log"
        return 1
    fi
    printf 'no live test skipped: the database-backed suite genuinely ran\n'
    rm -f "$log"
}

phase_smoke() { # ci.yml job: smoke
    ./scripts/demo-smoke.sh && ./scripts/italy-contract-smoke.sh
}

phase_ui_test() { # ci.yml job: ui-test
    (cd ui && npm ci && npm test)
}

# ci.yml job: feature-matrix. Same legs, same flags, same order; run in
# sequence rather than in parallel because one machine has one disk.
FEATURE_LEGS=(
    "no-default-features:--no-default-features"
    "pmtiles:--no-default-features --features pmtiles"
    "flatgeobuf:--no-default-features --features flatgeobuf"
    "geoparquet:--no-default-features --features geoparquet"
    "cog:--no-default-features --features cog"
    "zarr:--no-default-features --features zarr"
    "duckdb:--no-default-features --features duckdb"
    "geopackage:--no-default-features --features geopackage"
    # `iceberg` had no leg in ci.yml OR here until #123 -- see that file's
    # own comment for why a driver that compiles only under --all-features
    # is not a proven independent driver. This list is a MIRROR of ci.yml's
    # matrix; adding a leg to one file and not the other is what destroys
    # the property that makes the mirror worth keeping.
    "iceberg:--no-default-features --features iceberg"
    "ui:--no-default-features --features ui"
    "public-demo-ui:--no-default-features --features public-demo,ui"
    "valkey:--no-default-features --features valkey"
    "all-features:--all-features"
)

phase_feature_matrix() {
    local leg name flags failed=0
    for leg in "${FEATURE_LEGS[@]}"; do
        name="${leg%%:*}"
        flags="${leg#*:}"
        case "$name" in
            public-demo-ui)
                printf '  building dedicated public-demo ui/dist for the %s leg\n' "$name"
                (cd ui && npm ci && npm run build:public-demo) || {
                    printf '  FAIL feature leg %s (public-demo ui/dist build)\n' "$name"
                    failed=1
                    continue
                }
                ;;
            ui | all-features)
                # `build.rs` fails with a clear message naming this command
                # when the `ui` feature is on and `ui/dist` does not exist.
                printf '  building ui/dist for the %s leg\n' "$name"
                (cd ui && npm ci && npm run build) || {
                    printf '  FAIL feature leg %s (ui/dist build)\n' "$name"
                    failed=1
                    continue
                }
                ;;
        esac
        printf '\n  --- feature leg: %s\n' "$name"
        # shellcheck disable=SC2086
        if cargo test -p tellurion --locked $flags; then
            printf '  PASS feature leg %s\n' "$name"
        else
            printf '  FAIL feature leg %s\n' "$name"
            failed=1
        fi
    done
    return "$failed"
}

phase_deploy_manifests() { # ci.yml job: deploy-manifests
    ./scripts/validate-deploy-manifests.sh
}

phase_artifact_audit() { # ci.yml job: artifact-audit
    local dependency_summary dependency_status=0
    dependency_summary="$(mktemp /private/tmp/tellurion-dependency-summary.XXXXXX)" || return 1
    ./scripts/audit-license-policy.sh &&
        ./scripts/audit-publication-license.sh &&
        ./scripts/audit-dependency-licenses.py --output "$dependency_summary" || dependency_status=$?
    rm -f "$dependency_summary"
    [ "$dependency_status" -eq 0 ] &&
        ./scripts/check-ci-workflows.sh &&
        ./scripts/test-ci-workflows.sh &&
        ./scripts/audit-artifacts.sh &&
        ./scripts/check-release-workflow.sh &&
        ./scripts/test-release-workflow-contract.sh &&
        ./scripts/test-license-policy.sh
}

# --- guard-script coverage audit --------------------------------------------
#
# The gap this closes is not hypothetical: `demo-smoke.sh` (202 checks) and
# `italy-contract-smoke.sh` (136 checks) sat in version control for a day
# without any job invoking them, and nothing anywhere said so. Enumerating
# `scripts/` by directory listing rather than from a list keeps that from
# being possible again -- a new guard script is covered by this audit the day
# it lands, and shows up here as UNCOVERED until somebody decides where it
# belongs.
#
# `dependency-license-overrides.json` is policy data read by the dependency
# audit, not a command. `rg-compat.sh` and `workspace-version.sh` are sourced by
# audit and guard scripts, never executed, and are correctly not executable.
# `workspace-version.sh` says so in its own opening line -- it resolves
# `[workspace.package] version` for the five scripts that need it, and reads the
# manifest with awk rather than `cargo metadata` precisely so a guard can still
# run on a bare runner with no Cargo registry.
DATA_ONLY=("dependency-license-overrides.json")
SOURCED_ONLY=("rg-compat.sh" "workspace-version.sh")
# Guards CI reaches through another script rather than invoking directly.
INDIRECT=(
    "check-pss-restricted.py:validate-deploy-manifests.sh"
    "check-workflow-permissions.py:check-ci-workflows.sh"
)
# These inspect or prepare publication state. They are intentional manual
# owner gates, not ordinary CI commands, and must remain visible in the audit.
MANUAL_PUBLICATION_GATES=(
    "audit-public-history.py"
    "export-public-core.py"
    "extract-disclosure-zip.py"
    "snapshot-publication-state.sh"
)
# This starts a local demo server and Vite UI. It is intentionally outside
# ordinary CI, but remains visible to the audit as a developer tool.
MANUAL_DEVELOPER_TOOLS=(
    "run-local.sh"
)

workflow_executes_data_input() {
    local base="$1" base_re prefix command suffix workflow
    shift
    base_re="${base//./\\.}"
    prefix='(^[[:space:]]*(-[[:space:]]*)?(run:[[:space:]]*)?|run:[[:space:]]*|(&&|\|\||[;|])[[:space:]]*)'
    command="(['\"]?(\\./)?scripts/${base_re}['\"]?|((ba)?sh|python3?|node)[[:space:]]+['\"]?(\\./)?scripts/${base_re}['\"]?)"
    suffix='([[:space:];&|]|$)'
    for workflow in "$@"; do
        grep -Eq "${prefix}${command}${suffix}" "$workflow" && return 0
    done
    return 1
}

phase_audit() {
    local script base status=0
    local -a workflows=(
        ".github/workflows/ci.yml"
        ".github/workflows/release-artifacts.yml"
    )

    # The same question one level down: not "does a job run this script" but
    # "does a job build this test file". A `[[test]]` target gated by
    # `required-features` plus an inner `#![cfg]` can match no CI invocation
    # at all and compile to nothing, which is how four driver acceptance
    # files -- one of them carrying an assertion that could never pass --
    # stayed invisible. Run first because it is static and instant.
    ./scripts/check-test-feature-coverage.py || status=1
    printf '\n'
    printf '%-38s %-10s %s\n' "GUARD SCRIPT" "IN CI" "HOW"
    for script in "$REPO_ROOT"/scripts/*; do
        base="$(basename "$script")"
        if [ -d "$script" ]; then
            printf '%-38s %-10s %s\n' "$base" "n/a" "directory; not a CI command"
            continue
        fi
        # Listed rather than skipped: a reader must be able to see that every
        # file in `scripts/` was accounted for, including this one.
        if [ "$base" = "ci-local.sh" ]; then
            printf '%-38s %-10s %s\n' "$base" "n/a" "this mirror; runs ci.yml's gates, is not one"
            continue
        fi

        local data_only=0 entry
        for entry in "${DATA_ONLY[@]}"; do
            [ "$base" = "$entry" ] && data_only=1
        done
        if [ "$data_only" = 1 ]; then
            if workflow_executes_data_input "$base" "${workflows[@]}"; then
                printf '%-38s %-10s %s\n' "$base" "NO" "data input is invoked by a hosted workflow"
                status=1
            elif [ -x "$script" ]; then
                printf '%-38s %-10s %s\n' "$base" "NO" "data input is executable"
                status=1
            else
                printf '%-38s %-10s %s\n' "$base" "n/a" "data input, never executed"
            fi
            continue
        fi

        local skip=0
        for entry in "${SOURCED_ONLY[@]}"; do
            [ "$base" = "$entry" ] && skip=1
        done
        if [ "$skip" = 1 ]; then
            printf '%-38s %-10s %s\n' "$base" "n/a" "sourced helper, never executed"
            continue
        fi

        local manual_gate=0
        for entry in "${MANUAL_PUBLICATION_GATES[@]}"; do
            [ "$base" = "$entry" ] && manual_gate=1
        done
        if [ "$manual_gate" = 1 ]; then
            printf '%-38s %-10s %s\n' "$base" "manual" "publication gate; owner-controlled outside ordinary CI"
            continue
        fi

        local manual_developer_tool=0
        for entry in "${MANUAL_DEVELOPER_TOOLS[@]}"; do
            [ "$base" = "$entry" ] && manual_developer_tool=1
        done
        if [ "$manual_developer_tool" = 1 ]; then
            printf '%-38s %-10s %s\n' "$base" "manual" "developer tool; run outside ordinary CI"
            continue
        fi

        local indirect_via=""
        for entry in "${INDIRECT[@]}"; do
            [ "$base" = "${entry%%:*}" ] && indirect_via="${entry#*:}"
        done
        if [ -n "$indirect_via" ]; then
            if grep -qF "$indirect_via" "${workflows[@]}"; then
                printf '%-38s %-10s %s\n' "$base" "yes" "via $indirect_via"
            else
                printf '%-38s %-10s %s\n' "$base" "NO" "via $indirect_via, which no hosted workflow runs"
                status=1
            fi
            continue
        fi

        if grep -qF "$base" "${workflows[@]}"; then
            printf '%-38s %-10s %s\n' "$base" "yes" "invoked by a hosted workflow"
        else
            printf '%-38s %-10s %s\n' "$base" "NO" "no hosted workflow invokes it"
            status=1
        fi

        if [ ! -x "$script" ]; then
            printf '  FAIL: %s is not executable, so `./scripts/%s` cannot run it\n' \
                "$base" "$base" >&2
            status=1
        fi
    done
    return "$status"
}

# --- driver ------------------------------------------------------------------

PHASES=(
    "audit:phase_audit:guard-script coverage (no ci.yml job; this script owns it)"
    "fmt:phase_fmt:ci.yml job 'rustfmt'"
    "clippy:phase_clippy:ci.yml job 'clippy'"
    "test:phase_test:ci.yml job 'test'"
    "smoke:phase_smoke:ci.yml job 'demo smokes'"
    "ui-test:phase_ui_test:ci.yml job 'UI tests'"
    "feature-matrix:phase_feature_matrix:ci.yml job 'feature matrix (*)'"
    "deploy-manifests:phase_deploy_manifests:ci.yml job 'deploy manifests'"
    "artifact-audit:phase_artifact_audit:ci.yml job 'artifact audit'"
)

if [ "${1:-}" = "--list" ]; then
    printf '%-18s %s\n' "PHASE" "MIRRORS"
    for entry in "${PHASES[@]}"; do
        printf '%-18s %s\n' "${entry%%:*}" "${entry##*:}"
    done
    exit 0
fi

selected=("$@")
AUDIT_FLAG=0
if [ "${1:-}" = "--audit" ]; then
    selected=(audit)
    AUDIT_FLAG=1
fi

wants() {
    [ "${#selected[@]}" -eq 0 ] && return 0
    local want
    for want in "${selected[@]}"; do
        [ "$want" = "$1" ] && return 0
    done
    return 1
}

# A misspelled phase name would otherwise select nothing and end on `PASS:
# every phase that ran passed` -- a green verdict for a run that checked
# nothing, which is the one outcome this script exists to make impossible.
for want in ${selected[@]+"${selected[@]}"}; do
    known=0
    for entry in "${PHASES[@]}"; do
        [ "${entry%%:*}" = "$want" ] && known=1
    done
    if [ "$known" = 0 ]; then
        printf 'REFUSED: no such phase: %s\n' "$want" >&2
        printf 'Run `%s --list` for the phases this script mirrors.\n' "$0" >&2
        exit 2
    fi
done

# --- what this invocation is NOT checking, named up front --------------------
#
# Derived from `wants` rather than from a second list, so the announcement
# cannot drift from the selection it describes.
RAN_PHASES=()
SKIPPED_PHASES=()
for entry in "${PHASES[@]}"; do
    if wants "${entry%%:*}"; then
        RAN_PHASES+=("${entry%%:*}")
    else
        SKIPPED_PHASES+=("${entry%%:*}")
    fi
done

# Printed BEFORE the phases run and again after the summary. Both, on purpose:
# a full run is long enough to scroll the top off the screen, and the reader
# who pastes a verdict into a review is looking at the bottom.
announce_subset() {
    [ "${#SKIPPED_PHASES[@]}" -eq 0 ] && return 0
    printf '\n\033[1m!!! PARTIAL RUN -- a SUBSET of the pipeline, not the pipeline\033[0m\n'
    if [ "$AUDIT_FLAG" = 1 ]; then
        printf '`--audit` runs the guard-script coverage audit and nothing else. It\n'
        printf 'checks that CI REACHES every guard script and every feature-gated test\n'
        printf 'target; it never checks that any of them PASSES.\n'
    fi
    printf '  ran     (%d/%d): %s\n' \
        "${#RAN_PHASES[@]}" "${#PHASES[@]}" "${RAN_PHASES[*]}"
    printf '  NOT run (%d/%d): %s\n' \
        "${#SKIPPED_PHASES[@]}" "${#PHASES[@]}" "${SKIPPED_PHASES[*]}"
    printf 'Nothing this run prints is evidence about the phases on that second line.\n'
    printf 'Run `%s` with no arguments for the whole pipeline.\n' "$0"
}

announce_subset

require_tool cargo "every phase below builds or tests the workspace"
require_tool curl "every assertion in the two smoke scripts is an HTTP request"
if wants test; then
    require_database_url
fi

for entry in "${PHASES[@]}"; do
    name="${entry%%:*}"
    fn="$(printf '%s' "$entry" | cut -d: -f2)"
    wants "$name" || continue
    gate "$name" "$fn"
done

printf '\n\033[1m=== summary\033[0m\n'
for result in "${RESULTS[@]}"; do
    printf '  %s\n' "$result"
done

announce_subset

if [ "$FAILURES" -ne 0 ]; then
    printf '\nFAIL: %s phase(s) failed\n' "$FAILURES" >&2
    exit 1
fi
# A subset never gets to print the sentence a full run prints. The exit code
# stays 0 -- the phases that ran did pass, and a caller asking for two phases
# is entitled to a truthful verdict about two phases -- but the word `PASS`
# alone, unqualified, is reserved for a run that checked everything.
if [ "${#SKIPPED_PHASES[@]}" -ne 0 ]; then
    printf '\nPARTIAL: the %d phase(s) that ran passed; %d phase(s) did NOT run: %s\n' \
        "${#RAN_PHASES[@]}" "${#SKIPPED_PHASES[@]}" "${SKIPPED_PHASES[*]}"
    printf 'This is a green subset, not a green pipeline.\n'
    exit 0
fi
printf '\nPASS: every phase that ran passed\n'
