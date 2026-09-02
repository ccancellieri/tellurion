#!/usr/bin/env bash
# Create private, disclosure-audit evidence. Never use this output as a public artifact.
set -euo pipefail

script_dir="$(cd "$(dirname "$0")" && pwd)"
zip_extractor="$script_dir/extract-disclosure-zip.py"
reserve_kib=$((10 * 1024 * 1024))
download_surfaces=false
if [[ ${1:-} == --download-disclosure-surfaces ]]; then
    download_surfaces=true
    shift
fi
if [[ $# -ne 2 ]]; then
    printf 'usage: %s [--download-disclosure-surfaces] OWNER/REPOSITORY OUTPUT_DIRECTORY\n' "$0" >&2
    exit 64
fi

repository="$1"
destination="$2"
git_root="$(git rev-parse --show-toplevel)"
command -v python3 >/dev/null 2>&1 || {
    printf 'required tool not found: python3\n' >&2
    exit 69
}
[[ -f "$zip_extractor" ]] || {
    printf 'required disclosure ZIP extractor not found\n' >&2
    exit 69
}

canonical_path() {
    local requested="$1" probe suffix='' parent name
    if [[ "$requested" != /* ]]; then requested="$(pwd -P)/$requested"; fi
    probe="$requested"
    while [[ ! -e "$probe" && ! -L "$probe" ]]; do
        name="$(basename "$probe")"
        suffix="/$name$suffix"
        parent="$(dirname "$probe")"
        [[ "$parent" != "$probe" ]] || break
        probe="$parent"
    done
    if [[ -d "$probe" ]]; then
        printf '%s%s\n' "$(cd "$probe" && pwd -P)" "$suffix"
    else
        printf '%s/%s%s\n' "$(cd "$(dirname "$probe")" && pwd -P)" "$(basename "$probe")" "$suffix"
    fi
}

git_root="$(canonical_path "$git_root")"
destination="$(canonical_path "$destination")"
while IFS= read -r worktree; do
    [[ -n "$worktree" ]] || continue
    worktree="$(canonical_path "$worktree")"
    case "$destination/" in
        "$worktree/"*)
            printf 'destination must be outside every Git worktree\n' >&2
            exit 64
            ;;
    esac
done < <(git -C "$git_root" worktree list --porcelain | sed -n 's/^worktree //p')

if [[ -e "$destination" || -L "$destination" ]]; then
    if [[ ! -d "$destination" ]]; then
        printf 'destination must be an empty directory\n' >&2
        exit 64
    fi
    if [[ -n "$(find "$destination" -mindepth 1 -maxdepth 1 -print -quit)" ]]; then
        printf 'destination must be empty\n' >&2
        exit 64
    fi
fi
disclosure_root="$destination/disclosure-surfaces"
mkdir -p "$disclosure_root/actions/logs" \
    "$disclosure_root/actions/artifacts" "$disclosure_root/releases" \
    "$disclosure_root/github"

status_file="$destination/endpoint-status.json"
printf '[\n' > "$status_file"
status_first=true
append_status() {
    local name="$1" state="$2" code="$3"
    if ! "$status_first"; then printf ',\n' >> "$status_file"; fi
    status_first=false
    python3 -c \
        'import json, sys; print(json.dumps({"endpoint": sys.argv[1], "state": sys.argv[2], "http_status": int(sys.argv[3])}, separators=(",", ":")))' \
        "$name" "$state" "$code" >> "$status_file"
}

append_http_status() {
    local name="$1" code="$2"
    case "$code" in
        410) append_status "$name" expired "$code" ;;
        403|404|422) append_status "$name" unavailable "$code" ;;
        *) append_status "$name" failed "$code" ;;
    esac
}

http_status() {
    local endpoint="$1" accept_header="${2:-}" raw="$destination/.http-status" code
    local -a arguments=(api --method HEAD --include)
    if [[ -n "$accept_header" ]]; then
        arguments+=(-H "$accept_header")
    fi
    arguments+=("$endpoint")
    set +e
    gh "${arguments[@]}" > "$raw" 2>/dev/null
    set -e
    code="$(sed -nE 's#^HTTP/[0-9.]+ ([0-9]{3}).*#\1#p' "$raw" | tail -1)"
    rm -f "$raw"
    printf '%s\n' "${code:-000}"
}

api_inventory() {
    local name="$1" endpoint="$2" paginated="${3:-false}"
    local output="${4:-$destination/inventory-$name.json}"
    local raw="$destination/.response-$name" result code
    set +e
    if [[ "$paginated" == true ]]; then
        gh api --paginate --slurp "$endpoint" > "$raw" 2>/dev/null
    else
        gh api "$endpoint" > "$raw" 2>/dev/null
    fi
    result=$?
    set -e
    if [[ $result -eq 0 ]]; then
        mkdir -p "$(dirname "$output")"
        mv "$raw" "$output"
        append_status "$name" available 200
        return
    fi
    rm -f "$raw"
    code="$(http_status "$endpoint")"
    append_http_status "$name" "$code"
}

urlencode_segment() {
    python3 -c 'import sys, urllib.parse; print(urllib.parse.quote(sys.argv[1], safe=""))' "$1"
}

api_listing() {
    local name="$1" endpoint="$2" expression="$3" output="$4"
    local result code
    set +e
    gh api --paginate "$endpoint" --jq "$expression" > "$output" 2>/dev/null
    result=$?
    set -e
    if [[ $result -eq 0 ]]; then
        append_status "$name" available 200
        return 0
    fi
    rm -f "$output"
    code="$(http_status "$endpoint")"
    append_http_status "$name" "$code"
    return 1
}

git -C "$git_root" bundle create "$destination/repository.bundle" --all
git -C "$git_root" bundle verify "$destination/repository.bundle" >/dev/null
api_inventory repository "repos/$repository"
api_inventory refs "repos/$repository/git/matching-refs/" true
api_inventory releases "repos/$repository/releases?per_page=100" true
api_inventory issues "repos/$repository/issues?state=all&per_page=100" true
api_inventory pull-requests "repos/$repository/pulls?state=all&per_page=100" true
api_inventory actions-permissions "repos/$repository/actions/permissions"
api_inventory actions-runs "repos/$repository/actions/runs?per_page=100" true
api_inventory actions-artifacts "repos/$repository/actions/artifacts?per_page=100" true
api_inventory environments "repos/$repository/environments?per_page=100" true
api_inventory deployments "repos/$repository/deployments?per_page=100" true
api_inventory issue-comments "repos/$repository/issues/comments?per_page=100" true \
    "$disclosure_root/github/issue-comments.json"
api_inventory pages "repos/$repository/pages"
api_inventory hooks "repos/$repository/hooks?per_page=100" true
api_inventory collaborators "repos/$repository/collaborators?per_page=100" true
api_inventory rulesets "repos/$repository/rulesets?per_page=100" true
api_inventory branches "repos/$repository/branches?per_page=100" true
api_listing repository-actions-secrets "repos/$repository/actions/secrets?per_page=100" \
    '.secrets[] | {name, created_at, updated_at}' \
    "$destination/inventory-actions-secrets.json" || true
api_listing repository-actions-variables "repos/$repository/actions/variables?per_page=100" \
    '.variables[] | {name, created_at, updated_at}' \
    "$destination/inventory-actions-variables.json" || true

environment_names="$destination/.environment-names"
if api_listing environment-secrets-variables-listing \
    "repos/$repository/environments?per_page=100" \
    '.environments[] | [.name] | @tsv' "$environment_names"; then
    while IFS= read -r environment; do
        [[ -n "$environment" ]] || continue
        environment_key="$(LC_ALL=C printf '%s' "$environment" | od -An -tx1 | tr -d ' \n')"
        environment_path="$(urlencode_segment "$environment")"
        api_listing "environment-secrets-$environment_key" \
            "repos/$repository/environments/$environment_path/secrets?per_page=100" \
            '.secrets[] | {name, created_at, updated_at}' \
            "$destination/inventory-environment-secrets-$environment_key.json" || true
        api_listing "environment-variables-$environment_key" \
            "repos/$repository/environments/$environment_path/variables?per_page=100" \
            '.variables[] | {name, created_at, updated_at}' \
            "$destination/inventory-environment-variables-$environment_key.json" || true
    done < "$environment_names"
    rm -f "$environment_names"
fi

branch_names="$destination/.branch-names"
if api_listing branch-protection-listing "repos/$repository/branches?per_page=100" '.[].name' "$branch_names"; then
    while IFS= read -r branch; do
        [[ -n "$branch" ]] || continue
        branch_key="$(LC_ALL=C printf '%s' "$branch" | od -An -tx1 | tr -d ' \n')"
        branch_path=''
        LC_ALL=C
        for ((index = 0; index < ${#branch}; index++)); do
            character="${branch:index:1}"
            case "$character" in
                [a-zA-Z0-9.~_-]) branch_path+="$character" ;;
                *)
                    printf -v octet '%02X' "'$character"
                    branch_path+="%$octet"
                    ;;
            esac
        done
        api_inventory "branch-protection-$branch_key" "repos/$repository/branches/$branch_path/protection"
    done < "$branch_names"
    rm -f "$branch_names"
fi

python3 - \
    "$destination/inventory-issues.json" \
    "$destination/inventory-pull-requests.json" \
    "$disclosure_root/github/issue-comments.json" \
    "$disclosure_root/github/attachment-references.json" <<'PY'
import json
from pathlib import Path
import re
import sys

references = set()
pattern = re.compile(
    r"https://(?:github\.com/user-attachments/assets/|user-images\.githubusercontent\.com/|"
    r"github\.com/[^\s<>()\"']+/(?:files|assets)/)[^\s<>()\"']+"
)

def bodies(value):
    if isinstance(value, dict):
        body = value.get("body")
        if isinstance(body, str):
            yield body
        for child in value.values():
            yield from bodies(child)
    elif isinstance(value, list):
        for child in value:
            yield from bodies(child)

for source_name in sys.argv[1:4]:
    source = Path(source_name)
    if not source.is_file():
        continue
    try:
        value = json.loads(source.read_text())
    except (OSError, json.JSONDecodeError):
        continue
    for body in bodies(value):
        references.update(pattern.findall(body))

Path(sys.argv[4]).write_text(json.dumps(sorted(references), indent=2) + "\n")
PY

available_kib() { df -Pk "$destination" | awk 'NR == 2 { print $4 }'; }
download_endpoint() {
    local label="$1" endpoint="$2" path="$3" expected_bytes="${4:-}" accept_header="${5:-}"
    local free_kib payload_kib limit_blocks result actual_bytes code
    local -a arguments=(api)
    if [[ -n "$expected_bytes" && ! "$expected_bytes" =~ ^[0-9]+$ ]]; then
        printf 'download stopped: invalid recorded object size\n' >&2
        exit 2
    fi
    if [[ -n "$accept_header" ]]; then
        arguments+=(-H "$accept_header")
    fi
    arguments+=("$endpoint")
    free_kib="$(available_kib)"
    if [[ -n "$expected_bytes" ]]; then
        payload_kib=$(((expected_bytes + 1023) / 1024))
        if (( free_kib < reserve_kib + payload_kib )); then
            printf 'download stopped: 10 GiB free-space reserve would be crossed\n' >&2
            exit 2
        fi
        limit_blocks=$(((expected_bytes + 511) / 512))
        (( limit_blocks > 0 )) || limit_blocks=1
    else
        if (( free_kib <= reserve_kib )); then
            printf 'download stopped: 10 GiB free-space reserve would be crossed\n' >&2
            exit 2
        fi
        limit_blocks=$(((free_kib - reserve_kib) * 2))
    fi

    set +e
    (ulimit -f "$limit_blocks"; gh "${arguments[@]}" > "$path" 2>/dev/null)
    result=$?
    set -e
    if [[ $result -eq 0 ]]; then
        actual_bytes="$(wc -c < "$path" | tr -d ' ')"
        if [[ -n "$expected_bytes" && "$actual_bytes" -ne "$expected_bytes" ]]; then
            rm -f "$path"
            printf 'download stopped: object size differed from its recorded size\n' >&2
            exit 2
        fi
        append_status "$label" downloaded 200
        return
    fi

    rm -f "$path"
    if [[ -z "$expected_bytes" && $result -ge 128 ]]; then
        printf 'download stopped: 10 GiB free-space reserve would be crossed\n' >&2
        exit 2
    fi
    code="$(http_status "$endpoint" "$accept_header")"
    append_http_status "$label" "$code"
}

if "$download_surfaces"; then
    run_listing="$destination/.download-run-listing"
    if api_listing actions-run-download-listing "repos/$repository/actions/runs?per_page=100" '.workflow_runs[] | [.id] | @tsv' "$run_listing"; then
        while IFS= read -r run_id; do
            [[ -n "$run_id" ]] || continue
            log_archive="$disclosure_root/actions/logs/$run_id.zip"
            download_endpoint "actions-log-$run_id" \
                "repos/$repository/actions/runs/$run_id/logs" "$log_archive"
            if [[ -f "$log_archive" ]]; then
                python3 "$zip_extractor" "$log_archive" \
                    "$disclosure_root/actions/logs/$run_id" >/dev/null || exit 2
            fi
            artifact_listing="$destination/.download-artifact-listing-$run_id"
            if api_listing "actions-artifact-listing-$run_id" "repos/$repository/actions/runs/$run_id/artifacts?per_page=100" '.artifacts[] | [.id, .size_in_bytes] | @tsv' "$artifact_listing"; then
                while IFS=$'\t' read -r artifact_id artifact_size; do
                    [[ -n "$artifact_id" ]] || continue
                    artifact_archive="$disclosure_root/actions/artifacts/$artifact_id.zip"
                    download_endpoint "actions-artifact-$artifact_id" \
                        "repos/$repository/actions/artifacts/$artifact_id/zip" \
                        "$artifact_archive" "$artifact_size"
                    if [[ -f "$artifact_archive" ]]; then
                        python3 "$zip_extractor" "$artifact_archive" \
                            "$disclosure_root/actions/artifacts/$artifact_id" >/dev/null || exit 2
                    fi
                done < "$artifact_listing"
                rm -f "$artifact_listing"
            fi
        done < "$run_listing"
        rm -f "$run_listing"
    fi

    release_listing="$destination/.download-release-listing"
    if api_listing release-asset-listing "repos/$repository/releases?per_page=100" '.[] | .assets[] | [.id, .size] | @tsv' "$release_listing"; then
        while IFS=$'\t' read -r asset_id asset_size; do
            [[ -n "$asset_id" ]] || continue
            download_endpoint "release-asset-$asset_id" \
                "repos/$repository/releases/assets/$asset_id" \
                "$disclosure_root/releases/$asset_id" "$asset_size" \
                'Accept: application/octet-stream'
        done < "$release_listing"
        rm -f "$release_listing"
    fi
fi

printf '\n]\n' >> "$status_file"
sha256_command="$(command -v sha256sum || command -v shasum)"
if [[ "$(basename "$sha256_command")" == shasum ]]; then
    find "$destination" -type f ! -name SHA256SUMS -print0 | sort -z | xargs -0 shasum -a 256 > "$destination/SHA256SUMS"
else
    find "$destination" -type f ! -name SHA256SUMS -print0 | sort -z | xargs -0 sha256sum > "$destination/SHA256SUMS"
fi
printf 'snapshot complete\n'
