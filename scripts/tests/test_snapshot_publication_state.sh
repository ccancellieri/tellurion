#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HELPER="$ROOT/scripts/snapshot-publication-state.sh"
FIXTURE="$(mktemp -d)"
LINKED_ROOT="$FIXTURE/linked-worktree"
GH_LOG="$FIXTURE/gh.log"
trap 'rm -rf "$FIXTURE"' EXIT

fail() { printf 'FAIL: %s\n' "$*" >&2; exit 1; }
assert_contains() { grep -Fq -- "$2" "$1" || fail "expected $1 to contain $2"; }
assert_not_contains() {
    if grep -Fq -- "$2" "$1"; then fail "expected $1 not to contain $2"; fi
}

make_fake_tools() {
    mkdir -p "$FIXTURE/bin" "$LINKED_ROOT"
    cat > "$FIXTURE/bin/git" <<'EOF'
#!/usr/bin/env bash
if [[ "$*" == *"rev-parse --show-toplevel"* ]]; then
    printf '%s\n' "$SNAPSHOT_FAKE_ROOT"
elif [[ "$*" == *"worktree list --porcelain"* ]]; then
    printf 'worktree %s\nHEAD aaaaaaa\nbranch refs/heads/main\n\n' "$SNAPSHOT_FAKE_ROOT"
    printf 'worktree %s\nHEAD bbbbbbb\nbranch refs/heads/review\n\n' "$SNAPSHOT_LINKED_ROOT"
elif [[ "$*" == *"bundle create"* ]]; then
    previous=''
    for argument in "$@"; do
        if [[ "$previous" == create ]]; then
            printf 'fake bundle\n' > "$argument"
            exit 0
        fi
        previous="$argument"
    done
    exit 1
elif [[ "$*" == *"bundle verify"* ]]; then
    exit 0
else
    printf 'unexpected git invocation: %s\n' "$*" >&2
    exit 1
fi
EOF
    cat > "$FIXTURE/bin/df" <<'EOF'
#!/usr/bin/env bash
printf 'Filesystem 1024-blocks Used Available Capacity Mounted on\n'
printf 'fixture 99999999 1 %s 1%% /fixture\n' "${SNAPSHOT_FREE_KIB:-99999999}"
EOF
    cat > "$FIXTURE/bin/gh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$SNAPSHOT_GH_LOG"
endpoint=''
jq_expression=''
accept_header=''
previous=''
include=false
for argument in "$@"; do
    if [[ "$previous" == --jq ]]; then jq_expression="$argument"; fi
    if [[ "$previous" == -H ]]; then accept_header="$argument"; fi
    if [[ "$argument" == --include ]]; then include=true; fi
    case "$argument" in
        repos/*|https://api.github.test/*) endpoint="$argument" ;;
    esac
    previous="$argument"
done

emit_zip() {
    python3 - "$1" "$2" <<'PY'
import io
import sys
import zipfile

buffer = io.BytesIO()
entry = zipfile.ZipInfo(sys.argv[1], (1980, 1, 1, 0, 0, 0))
entry.compress_type = zipfile.ZIP_STORED
with zipfile.ZipFile(buffer, "w") as archive:
    archive.writestr(entry, sys.argv[2].encode())
sys.stdout.buffer.write(buffer.getvalue())
PY
}

zip_size() { emit_zip "$1" "$2" | wc -c | tr -d ' '; }

if [[ "$include" == true ]]; then
    if [[ "$endpoint" == *rulesets* ]]; then
        printf 'HTTP/2 403 Forbidden\n\n{"message":"private body"}\n'
        exit 1
    fi
    if [[ "$endpoint" == *'/releases/assets/301' && ${SNAPSHOT_EXPIRED_RELEASE:-false} == true ]]; then
        printf 'HTTP/2 410 Gone\n\n{"message":"expired private body"}\n'
        exit 1
    fi
    printf 'HTTP/2 200 OK\n\nfixture-download-body\n'
    exit 0
fi

if [[ -n "$jq_expression" ]]; then
    if [[ "$jq_expression" == '.secrets[] | {name, created_at, updated_at}' ]]; then
        printf '{"name":"SAFE_SECRET","created_at":"2026-08-27","updated_at":"2026-08-27"}\n'
        exit 0
    fi
    if [[ "$jq_expression" == '.variables[] | {name, created_at, updated_at}' ]]; then
        printf '{"name":"SAFE_VARIABLE","created_at":"2026-08-27","updated_at":"2026-08-27"}\n'
        exit 0
    fi
    case "$endpoint|$jq_expression" in
        *'/actions/runs?per_page=100|.workflow_runs[] | [.id] | @tsv')
            if [[ ${SNAPSHOT_FAIL_RUN_LISTING:-false} == true ]]; then exit 1; fi
            [[ ${SNAPSHOT_NO_RUNS:-false} == true ]] || printf '101\n'
            ;;
        *'/actions/runs/101/artifacts?per_page=100|.artifacts[] | [.id, .size_in_bytes] | @tsv')
            printf '201\t%s\n' "$(zip_size artifact/credential.txt 'token=synthetic-credential-for-scanner')"
            ;;
        *'/releases?per_page=100|.[] | .assets[] | [.id, .size] | @tsv')
            printf '301\t%s\n' "${SNAPSHOT_RELEASE_SIZE:-16}"
            ;;
        *'/environments?per_page=100|.environments[] | [.name] | @tsv')
            printf 'production\n'
            ;;
        *'/branches?per_page=100|.[].name')
            if [[ ${SNAPSHOT_SPECIAL_BRANCHES:-false} == true ]]; then
                printf 'foo/bar\nfoo%%2Fbar\nodd"branch\n'
            else
                printf 'main\nreview\n'
            fi
            ;;
        *) exit 1 ;;
    esac
    exit 0
fi

if [[ "$endpoint" == *rulesets* ]]; then
    printf '{"message":"private body"}\n'
    exit 1
fi
if [[ "$endpoint" == *'/releases/assets/301' && ${SNAPSHOT_EXPIRED_RELEASE:-false} == true ]]; then
    exit 1
fi
case "$endpoint" in
    *'/actions/runs/101/logs') emit_zip logs/credential.txt 'token=synthetic-credential-for-scanner' ; exit 0 ;;
    *'/actions/artifacts/201/zip') emit_zip artifact/credential.txt 'token=synthetic-credential-for-scanner' ; exit 0 ;;
    *'/releases/assets/301')
        [[ "$accept_header" == 'Accept: application/octet-stream' ]] || exit 1
        printf 'release-evidence'
        exit 0
        ;;
    *'/issues/comments?per_page=100')
        printf '[{"id":401,"body":"![evidence](https://github.com/user-attachments/assets/attachment-fixture)"}]\n'
        exit 0
        ;;
    *'/variables?per_page=100')
        printf '{"variables":[{"name":"SAFE_NAME","value":"fixture-variable-value","created_at":"2026-08-27","updated_at":"2026-08-27"}]}\n'
        exit 0
        ;;
esac
printf '[{"private":"do-not-print"}]\n'
EOF
    chmod +x "$FIXTURE/bin/git" "$FIXTURE/bin/df" "$FIXTURE/bin/gh"
}

run_snapshot() {
    local output="$1"
    shift
    : > "$GH_LOG"
    PATH="$FIXTURE/bin:$PATH" \
        SNAPSHOT_FAKE_ROOT="$ROOT" \
        SNAPSHOT_LINKED_ROOT="$LINKED_ROOT" \
        SNAPSHOT_GH_LOG="$GH_LOG" \
        "$HELPER" "$@" ccancellieri/tellurion "$output" >"$FIXTURE/stdout" 2>"$FIXTURE/stderr"
}

test_snapshot_refuses_repository_destination() {
    if run_snapshot "$ROOT/private-snapshot"; then
        fail 'snapshot accepted a destination inside the worktree'
    fi
    assert_contains "$FIXTURE/stderr" 'outside every Git worktree'
}

test_snapshot_refuses_linked_worktree_destination_before_writing() {
    output="$LINKED_ROOT/private-snapshot"
    if run_snapshot "$output"; then fail 'snapshot accepted a linked-worktree destination'; fi
    assert_contains "$FIXTURE/stderr" 'outside every Git worktree'
    [[ ! -e "$output" ]] || fail 'snapshot wrote inside a linked worktree before refusing it'
}

test_snapshot_refuses_symlinked_worktree_destination_before_writing() {
    link="$FIXTURE/link-to-worktree"
    ln -s "$LINKED_ROOT" "$link"
    output="$link/private-snapshot"
    if run_snapshot "$output"; then fail 'snapshot accepted a symlinked worktree destination'; fi
    assert_contains "$FIXTURE/stderr" 'outside every Git worktree'
    [[ ! -e "$LINKED_ROOT/private-snapshot" ]] || fail 'snapshot followed the symlink and wrote before refusing it'
}

test_snapshot_records_unavailable_endpoint_without_losing_other_results() {
    output="$FIXTURE/snapshot"
    run_snapshot "$output" || { cat "$FIXTURE/stderr" >&2; fail 'snapshot failed when one endpoint was unavailable'; }
    test -f "$output/repository.bundle" || fail 'missing bundle'
    assert_contains "$output/endpoint-status.json" 'rulesets'
    assert_contains "$output/endpoint-status.json" 'unavailable'
    test -f "$output/inventory-repository.json" || fail 'missing successful inventory'
}

test_snapshot_stdout_contains_status_only() {
    output="$FIXTURE/snapshot-stdout"
    run_snapshot "$output" || fail 'snapshot failed'
    assert_contains "$FIXTURE/stdout" 'snapshot complete'
    if grep -Eq 'private|endpoint|fixture-download-body' "$FIXTURE/stdout"; then
        fail 'snapshot printed response content'
    fi
}

test_snapshot_paginates_baseline_surfaces_and_all_branch_protection() {
    output="$FIXTURE/snapshot-pagination"
    run_snapshot "$output" || fail 'baseline snapshot failed'
    assert_contains "$GH_LOG" '--paginate --slurp repos/ccancellieri/tellurion/issues?state=all&per_page=100'
    assert_contains "$GH_LOG" '--paginate --slurp repos/ccancellieri/tellurion/pulls?state=all&per_page=100'
    assert_contains "$GH_LOG" '--paginate --slurp repos/ccancellieri/tellurion/releases?per_page=100'
    assert_contains "$GH_LOG" '--paginate --slurp repos/ccancellieri/tellurion/actions/runs?per_page=100'
    assert_contains "$GH_LOG" '--paginate --slurp repos/ccancellieri/tellurion/actions/artifacts?per_page=100'
    assert_contains "$GH_LOG" '--paginate --slurp repos/ccancellieri/tellurion/deployments?per_page=100'
    assert_contains "$GH_LOG" 'repos/ccancellieri/tellurion/actions/secrets?per_page=100'
    assert_contains "$GH_LOG" 'repos/ccancellieri/tellurion/actions/variables?per_page=100'
    assert_contains "$GH_LOG" '--paginate --slurp repos/ccancellieri/tellurion/issues/comments?per_page=100'
    assert_contains "$GH_LOG" 'repos/ccancellieri/tellurion/environments/production/secrets?per_page=100'
    assert_contains "$GH_LOG" 'repos/ccancellieri/tellurion/environments/production/variables?per_page=100'
    assert_contains "$GH_LOG" 'repos/ccancellieri/tellurion/branches/main/protection'
    assert_contains "$GH_LOG" 'repos/ccancellieri/tellurion/branches/review/protection'
}

test_snapshot_encodes_branch_evidence_injectively_and_keeps_status_json_valid() {
    output="$FIXTURE/snapshot-special-branches"
    SNAPSHOT_SPECIAL_BRANCHES=true run_snapshot "$output" || fail 'special-branch snapshot failed'
    inventory_count="$(find "$output" -maxdepth 1 -type f -name 'inventory-branch-protection-*' | wc -l | tr -d ' ')"
    [[ "$inventory_count" -eq 3 ]] || fail "expected three distinct branch-protection inventories, got $inventory_count"
    python3 -m json.tool "$output/endpoint-status.json" >/dev/null || fail 'endpoint status is not valid JSON'
    label_count="$(python3 -c 'import json,sys; data=json.load(open(sys.argv[1])); print(len({row["endpoint"] for row in data if row["endpoint"].startswith("branch-protection-") and row["endpoint"] != "branch-protection-listing"}))' "$output/endpoint-status.json")"
    [[ "$label_count" -eq 3 ]] || fail "expected three distinct branch-protection status labels, got $label_count"
}

test_snapshot_runs_without_standalone_jq() {
    cat > "$FIXTURE/bin/jq" <<'EOF'
#!/usr/bin/env bash
printf 'standalone jq was invoked\n' >> "$SNAPSHOT_GH_LOG"
exit 127
EOF
    chmod +x "$FIXTURE/bin/jq"
    output="$FIXTURE/snapshot-without-jq"
    run_snapshot "$output" || fail 'snapshot requires standalone jq'
    python3 -m json.tool "$output/endpoint-status.json" >/dev/null || fail 'endpoint status is not valid JSON without jq'
    assert_not_contains "$GH_LOG" 'standalone jq was invoked'
}

test_snapshot_records_disclosure_listing_failure() {
    output="$FIXTURE/snapshot-listing-failure"
    SNAPSHOT_FAIL_RUN_LISTING=true run_snapshot "$output" --download-disclosure-surfaces || fail 'snapshot discarded a listing failure as a fatal error'
    assert_contains "$output/endpoint-status.json" 'actions-run-download-listing'
    assert_contains "$output/endpoint-status.json" 'failed'
}

test_snapshot_refuses_known_download_that_crosses_reserve_before_attempt() {
    output="$FIXTURE/snapshot-reserve"
    set +e
    SNAPSHOT_NO_RUNS=true SNAPSHOT_FREE_KIB=10485761 SNAPSHOT_RELEASE_SIZE=4096 \
        run_snapshot "$output" --download-disclosure-surfaces
    result=$?
    set -e
    [[ $result -eq 2 ]] || fail "reserve crossing exited $result instead of 2"
    assert_contains "$FIXTURE/stderr" '10 GiB free-space reserve would be crossed'
    assert_not_contains "$GH_LOG" 'repos/ccancellieri/tellurion/releases/assets/301'
}

test_snapshot_rejects_release_asset_size_mismatch_in_either_direction() {
    output="$FIXTURE/snapshot-size-mismatch"
    set +e
    SNAPSHOT_NO_RUNS=true SNAPSHOT_FREE_KIB=99999999 SNAPSHOT_RELEASE_SIZE=32 \
        run_snapshot "$output" --download-disclosure-surfaces
    result=$?
    set -e
    [[ $result -eq 2 ]] || fail "release size mismatch exited $result instead of 2"
    assert_contains "$FIXTURE/stderr" 'object size differed from its recorded size'
}

test_snapshot_records_http_410_as_expired_without_response_body() {
    output="$FIXTURE/snapshot-expired"
    SNAPSHOT_NO_RUNS=true SNAPSHOT_EXPIRED_RELEASE=true SNAPSHOT_FREE_KIB=99999999 \
        run_snapshot "$output" --download-disclosure-surfaces || fail 'expired release asset made snapshot fatal'
    assert_contains "$output/endpoint-status.json" 'release-asset-301'
    assert_contains "$output/endpoint-status.json" 'expired'
    assert_not_contains "$output/endpoint-status.json" 'expired private body'
}

test_snapshot_downloads_and_extracts_every_surface_under_one_scan_root() {
    output="$FIXTURE/snapshot-disclosure-surfaces"
    SNAPSHOT_FREE_KIB=99999999 run_snapshot "$output" --download-disclosure-surfaces || fail 'disclosure downloads failed'
    scan_root="$output/disclosure-surfaces"
    test -f "$scan_root/actions/logs/101.zip" || fail 'missing Actions run log'
    test -f "$scan_root/actions/logs/101/logs/credential.txt" || fail 'missing extracted Actions log'
    test -f "$scan_root/actions/artifacts/201.zip" || fail 'missing Actions artifact'
    test -f "$scan_root/actions/artifacts/201/artifact/credential.txt" || fail 'missing extracted Actions artifact'
    test -f "$scan_root/releases/301" || fail 'missing release asset'
    [[ ! -e "$output/downloads" ]] || fail 'legacy downloads directory split the disclosure scan root'
    grep -R -Fq 'synthetic-credential-for-scanner' "$scan_root" || fail 'scanner root cannot see extracted credential fixture'
    assert_contains "$GH_LOG" '-H Accept: application/octet-stream repos/ccancellieri/tellurion/releases/assets/301'
    assert_not_contains "$GH_LOG" 'https://api.github.test/releases/assets/301'
    assert_not_contains "$GH_LOG" 'https://api.github.test/artifacts/201'
}

test_snapshot_inventories_attachment_references_without_variable_values() {
    output="$FIXTURE/snapshot-expanded-inventory"
    run_snapshot "$output" || fail 'expanded inventory failed'
    references="$output/disclosure-surfaces/github/attachment-references.json"
    test -f "$references" || fail 'missing attachment-reference inventory'
    assert_contains "$references" 'https://github.com/user-attachments/assets/attachment-fixture'
    if rg -Fq 'fixture-variable-value' "$output"; then
        fail 'snapshot retained an Actions variable value'
    fi
}

test_runbook_scans_the_same_disclosure_surface_root() {
    assert_contains "$ROOT/docs/publication-runbook.md" 'github/disclosure-surfaces'
}

make_fake_tools
test_snapshot_refuses_repository_destination
test_snapshot_refuses_linked_worktree_destination_before_writing
test_snapshot_refuses_symlinked_worktree_destination_before_writing
test_snapshot_records_unavailable_endpoint_without_losing_other_results
test_snapshot_stdout_contains_status_only
test_snapshot_paginates_baseline_surfaces_and_all_branch_protection
test_snapshot_encodes_branch_evidence_injectively_and_keeps_status_json_valid
test_snapshot_records_disclosure_listing_failure
test_snapshot_refuses_known_download_that_crosses_reserve_before_attempt
test_snapshot_rejects_release_asset_size_mismatch_in_either_direction
test_snapshot_records_http_410_as_expired_without_response_body
test_snapshot_downloads_and_extracts_every_surface_under_one_scan_root
test_snapshot_inventories_attachment_references_without_variable_values
test_runbook_scans_the_same_disclosure_surface_root
test_snapshot_runs_without_standalone_jq
printf 'ok: snapshot publication state tests\n'
