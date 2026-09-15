#!/usr/bin/env bash
# Require a successful canonical CI push run for one exact release commit.

set -euo pipefail

commit="${1-}"
repository="${GITHUB_REPOSITORY:-ccancellieri/tellurion}"
api_url="${GITHUB_API_URL:-https://api.github.com}"

printf '%s' "$commit" | grep -Eq '^[0-9a-f]{40}$' || {
    echo "FAIL: canonical CI check requires a full lowercase commit ID" >&2
    exit 1
}
[ "$repository" = ccancellieri/tellurion ] || {
    echo "FAIL: canonical CI repository must be ccancellieri/tellurion" >&2
    exit 1
}
[ "$api_url" = https://api.github.com ] || {
    echo "FAIL: canonical CI API must be https://api.github.com" >&2
    exit 1
}
for tool in curl jq; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "FAIL: canonical CI check requires $tool" >&2
        exit 1
    }
done

response="$(mktemp)"
trap 'rm -f "$response"' EXIT
headers=(
    --header 'Accept: application/vnd.github+json'
    --header 'X-GitHub-Api-Version: 2022-11-28'
)
if [ -n "${GITHUB_TOKEN:-}" ]; then
    headers+=(--header "Authorization: Bearer $GITHUB_TOKEN")
fi

url="$api_url/repos/$repository/actions/workflows/ci.yml/runs?branch=main&event=push&status=completed&head_sha=$commit&per_page=100"
curl --fail --silent --show-error --location --retry 3 \
    --user-agent 'tellurion-release-verifier/0.5' \
    "${headers[@]}" --output "$response" "$url"

jq -e --arg commit "$commit" --arg repository "$repository" '
    any(.workflow_runs[]?;
        .head_sha == $commit
        and .head_branch == "main"
        and .event == "push"
        and .status == "completed"
        and .conclusion == "success"
        and .path == ".github/workflows/ci.yml"
        and .repository.full_name == $repository
        and .head_repository.full_name == $repository
    )
' "$response" >/dev/null || {
    echo "FAIL: no successful completed canonical ci.yml push run for $commit on main" >&2
    exit 1
}

echo "canonical CI verified for $commit"
