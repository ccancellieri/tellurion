#!/usr/bin/env bash
# Bind publication to the live canonical GitHub main branch and version tag.

set -euo pipefail

version="${1-}"
commit="${2-}"
tag="v$version"
origin_url="$(git remote get-url origin 2>/dev/null || true)"
case "$origin_url" in
    https://github.com/ccancellieri/tellurion|https://github.com/ccancellieri/tellurion.git|git@github.com:ccancellieri/tellurion.git)
        ;;
    *)
        echo "FAIL: origin is not the canonical ccancellieri/tellurion repository" >&2
        exit 1
        ;;
esac

printf '%s' "$commit" | grep -Eq '^[0-9a-f]{40}$' || {
    echo "FAIL: canonical origin check requires a full lowercase commit ID" >&2
    exit 1
}

remote_refs="$(git ls-remote origin \
    refs/heads/main "refs/tags/$tag" "refs/tags/$tag^{}")" || {
    echo "FAIL: cannot read canonical origin refs" >&2
    exit 1
}
main_commit="$(printf '%s\n' "$remote_refs" | awk '$2 == "refs/heads/main" { print $1; exit }')"
tag_commit="$(printf '%s\n' "$remote_refs" | awk -v tag="refs/tags/$tag^{}" '$2 == tag { print $1; exit }')"
if [ -z "$tag_commit" ]; then
    tag_commit="$(printf '%s\n' "$remote_refs" | awk -v tag="refs/tags/$tag" '$2 == tag { print $1; exit }')"
fi
[ "$main_commit" = "$commit" ] || {
    echo "FAIL: canonical origin main does not equal requested commit" >&2
    exit 1
}
[ "$tag_commit" = "$commit" ] || {
    echo "FAIL: canonical origin tag $tag does not equal requested commit" >&2
    exit 1
}

echo "canonical origin verified: main and $tag -> $commit"
