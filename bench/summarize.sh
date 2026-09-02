#!/usr/bin/env bash
# Folds one bench/scenarios.sh results directory into a single markdown table:
# scenario x p50/p95/p99 (ms), rps, error rate, RSS delta. Median is taken
# across the measured repetitions ("*.repN.json"); "*.warmupN.json" files are
# always excluded, per the fairness rule in docs/benchmarking.md.
#
# Usage: ./summarize.sh [RESULTS_DIR]
#   RESULTS_DIR defaults to the most recently modified directory under
#   bench/results/. RSS_METRIC (default process_resident_memory_bytes) picks
#   the Prometheus metric read from the *.metrics_{before,after}.prom scrapes.
#
# Requires: bash, awk, jq, sort, sed.

set -eu
export LC_ALL=C

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
RSS_METRIC="${RSS_METRIC:-process_resident_memory_bytes}"

DIR="${1:-}"
if [ -z "$DIR" ]; then
    DIR="$(ls -td "$SCRIPT_DIR"/results/*/ 2>/dev/null | head -1)"
    if [ -z "$DIR" ]; then
        echo "ERROR: no results directory found under $SCRIPT_DIR/results/, and none given" >&2
        exit 1
    fi
fi
DIR="${DIR%/}"

for tool in awk jq sort sed; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "ERROR: required tool '$tool' not found on PATH" >&2
        exit 1
    }
done

if [ ! -d "$DIR" ]; then
    echo "ERROR: not a directory: $DIR" >&2
    exit 1
fi

# median NUM... -> middle value of the sorted, non-"null" inputs; "n/a" if none.
median() {
    vals=""
    for v in "$@"; do
        [ "$v" = "null" ] && continue
        vals="$vals$v
"
    done
    [ -z "$vals" ] && { echo "n/a"; return; }
    n=$(printf '%s' "$vals" | grep -c .)
    sorted="$(printf '%s' "$vals" | sort -n)"
    mid=$(( (n + 1) / 2 ))
    if [ $((n % 2)) -eq 1 ]; then
        m="$(printf '%s\n' "$sorted" | sed -n "${mid}p")"
    else
        a="$(printf '%s\n' "$sorted" | sed -n "${mid}p")"
        b="$(printf '%s\n' "$sorted" | sed -n "$((mid + 1))p")"
        m="$(awk -v a="$a" -v b="$b" 'BEGIN{print (a+b)/2}')"
    fi
    awk -v m="$m" 'BEGIN{printf "%.3f", m}'
}

# rss_delta_mib SCENARIO -> "+N.NN" MiB (after - before), "n/a" if either
# scrape is missing/unparseable. Mixed sub-scenarios (name has a dot, e.g.
# "mixed_70_20_10.tiles") share one scrape filed under the parent name.
rss_delta_mib() {
    scenario="$1"
    before="$DIR/$scenario.metrics_before.prom"
    after="$DIR/$scenario.metrics_after.prom"
    if [ ! -f "$before" ] || [ ! -f "$after" ]; then
        parent="${scenario%.*}"
        before="$DIR/$parent.metrics_before.prom"
        after="$DIR/$parent.metrics_after.prom"
    fi
    [ -f "$before" ] && [ -f "$after" ] || { echo "n/a"; return; }

    b="$(grep -E "^${RSS_METRIC}([ {]|$)" "$before" 2>/dev/null | tail -1 | awk '{print $NF}')"
    a="$(grep -E "^${RSS_METRIC}([ {]|$)" "$after" 2>/dev/null | tail -1 | awk '{print $NF}')"
    [ -n "$a" ] && [ -n "$b" ] || { echo "n/a"; return; }
    awk -v a="$a" -v b="$b" 'BEGIN{printf "%+.2f", (a-b)/1048576}'
}

# Unique scenario names, derived from every "*.repN.json" file present
# (warmup-only scenarios -- none expected -- would be silently excluded).
scenarios="$(
    find "$DIR" -maxdepth 1 -name '*.rep[0-9]*.json' -print \
        | sed -E 's#.*/##; s/\.rep[0-9]+\.json$//' \
        | sort -u
)"

if [ -z "$scenarios" ]; then
    echo "ERROR: no *.repN.json files found in $DIR" >&2
    exit 1
fi

printf '# Bench summary: %s\n\n' "$DIR"
printf '| Scenario | p50 (ms) | p95 (ms) | p99 (ms) | RPS | Error rate | RSS delta (MiB) |\n'
printf '|---|---|---|---|---|---|---|\n'

echo "$scenarios" | while IFS= read -r name; do
    [ -z "$name" ] && continue
    files="$(find "$DIR" -maxdepth 1 -name "$name.rep[0-9]*.json" | sort)"

    p50s=""; p95s=""; p99s=""; rpss=""; errs=""
    for f in $files; do
        p50s="$p50s $(jq -r '.metrics.latency_ms.p50 // "null"' "$f")"
        p95s="$p95s $(jq -r '.metrics.latency_ms.p95 // "null"' "$f")"
        p99s="$p99s $(jq -r '.metrics.latency_ms.p99 // "null"' "$f")"
        rpss="$rpss $(jq -r '.summary.requestsPerSec // "null"' "$f")"
        errs="$errs $(jq -r '(1 - (.summary.successRate // 0)) * 100' "$f")"
    done

    p50="$(median $p50s)"
    p95="$(median $p95s)"
    p99="$(median $p99s)"
    rps="$(median $rpss)"
    err="$(median $errs)"
    rss="$(rss_delta_mib "$name")"

    printf '| %s | %s | %s | %s | %s | %s%% | %s |\n' \
        "$name" "$p50" "$p95" "$p99" "$rps" "$err" "$rss"
done
