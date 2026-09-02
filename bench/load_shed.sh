#!/usr/bin/env bash
# Proves the load-shed 503 path (tellurion-server/src/app.rs: load_shed ahead
# of concurrency_limit) actually fires under overload, that it fires fast
# (rejects instead of queuing into the request timeout), and that the server
# serves 200s again once the burst subsides. See docs/benchmarking.md for how this
# fits alongside scenarios.sh.
#
# If BASE_URL is already serving, this runs the burst against it as-is --
# whatever server.max_concurrency that deployment is configured with. If
# nothing answers at BASE_URL, this builds (if needed) and starts its own
# server on an ephemeral port, using a generated config that pins
# server.max_concurrency low (MAX_CONCURRENCY) so a modest burst can exceed
# it without needing hundreds of real connections -- smoke scale by default.
# Kills whatever server it started on exit, success or failure.
#
# Requires: bash, awk, jq, curl, oha, cargo (only if a binary needs building).
#
# Env vars (all optional, defaults shown):
#   BASE_URL          http://127.0.0.1:18089   server under test; self-started if unreachable
#   DATABASE_URL      postgres://tellurion:tellurion@localhost:5433/tellurion
#                                               only used when self-starting a server
#   COLLECTION        pt_roads                 collection hit by the burst (must exist when self-starting)
#   TENANT            public                   tenant external id (see /{tenant}/{protocol}/catalogs/{catalog}/...)
#   CATALOG           default                  catalog external id, scoped to TENANT; also used as the
#                                               self-started config's catalog id
#   TABLE             $COLLECTION              physical table, self-started config only
#   GEOMETRY          geom                     geometry column, self-started config only
#   PK                ogc_fid                  primary key column, self-started config only
#   TELLURION_BIN     (auto)                   pre-built binary; skips the build step when set
#   MAX_CONCURRENCY   4                        server.max_concurrency in the self-started config
#   BURST_CONCURRENCY 40                       oha -c for the overload burst (keep above MAX_CONCURRENCY)
#   BURST_DURATION    5s                       oha -z for the overload burst
#   PROBES            10                       individual curl timing probes fired during the burst
#   SLOW_503_MS       1000                     a probe 503 slower than this fails the "fast" assertion
#   RECOVERY_TIMEOUT  15                       seconds to poll for the server to serve 200 again
#   OUT_DIR           bench/results/loadshed-<UTC timestamp>
#
# Usage (smoke scale, self-started server):
#   ./bench/load_shed.sh
# Usage (against a server already running with its own low max_concurrency):
#   BASE_URL=http://127.0.0.1:8080 ./bench/load_shed.sh

set -eu
export LC_ALL=C

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

BASE_URL="${BASE_URL:-http://127.0.0.1:18089}"
DATABASE_URL="${DATABASE_URL:-postgres://tellurion:tellurion@localhost:5433/tellurion}"
COLLECTION="${COLLECTION:-pt_roads}"
TENANT="${TENANT:-public}"
CATALOG="${CATALOG:-default}"
TABLE="${TABLE:-$COLLECTION}"
GEOMETRY="${GEOMETRY:-geom}"
PK="${PK:-ogc_fid}"
MAX_CONCURRENCY="${MAX_CONCURRENCY:-4}"
BURST_CONCURRENCY="${BURST_CONCURRENCY:-40}"
BURST_DURATION="${BURST_DURATION:-5s}"
PROBES="${PROBES:-10}"
SLOW_503_MS="${SLOW_503_MS:-1000}"
RECOVERY_TIMEOUT="${RECOVERY_TIMEOUT:-15}"
OUT_DIR="${OUT_DIR:-$SCRIPT_DIR/results/loadshed-$(date -u +%Y%m%dT%H%M%SZ)}"

for tool in awk jq curl oha; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "ERROR: required tool '$tool' not found on PATH" >&2
        exit 1
    }
done

mkdir -p "$OUT_DIR"
echo "results -> $OUT_DIR"

SERVER_PID=""
TMP_CONFIG=""
cleanup() {
    if [ -n "$SERVER_PID" ]; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
    [ -n "$TMP_CONFIG" ] && rm -f "$TMP_CONFIG"
}
trap cleanup EXIT

# --- start our own server if BASE_URL isn't already answering ------------
if curl -sf --max-time 2 "$BASE_URL/" -o /dev/null 2>/dev/null; then
    echo "using already-running server at $BASE_URL"
else
    echo "no server at $BASE_URL -- starting one"

    if [ -n "${TELLURION_BIN:-}" ]; then
        bin="$TELLURION_BIN"
    elif [ -x "$REPO_ROOT/target/release/tellurion" ]; then
        bin="$REPO_ROOT/target/release/tellurion"
    elif [ -x "$REPO_ROOT/target/debug/tellurion" ]; then
        bin="$REPO_ROOT/target/debug/tellurion"
    else
        echo "building the tellurion server binary (debug; set TELLURION_BIN to skip)..." >&2
        (cd "$REPO_ROOT" && cargo build -p tellurion)
        bin="$REPO_ROOT/target/debug/tellurion"
    fi

    port="$(printf '%s' "$BASE_URL" | sed -E 's#^https?://[^:/]+:?##; s#/.*$##')"
    [ -n "$port" ] || port=18089

    TMP_CONFIG="$(mktemp -t tellurion-load-shed-config.XXXXXX.yaml)"
    cat > "$TMP_CONFIG" <<EOF
server:
  port: $port
  request_timeout_s: 60
  max_concurrency: $MAX_CONCURRENCY
cache:
  memory_percent: 10.0
storages:
  - id: main
    driver: postgis
    url_env: DATABASE_URL
tenants:
  - id: $TENANT
catalogs:
  - id: $CATALOG
    tenant: $TENANT
collections:
  - id: $COLLECTION
    catalog: $CATALOG
    storage: main
    table: $TABLE
    geometry: $GEOMETRY
    pk: $PK
    tiles: { minzoom: 0, maxzoom: 14, caps: { z0: 1000 } }
EOF

    DATABASE_URL="$DATABASE_URL" TELLURION_CONFIG="$TMP_CONFIG" \
        "$bin" > "$OUT_DIR/server.log" 2>&1 &
    SERVER_PID=$!

    ready=0
    for _ in $(seq 1 30); do
        if curl -sf --max-time 2 "$BASE_URL/" -o /dev/null 2>/dev/null; then
            ready=1
            break
        fi
        sleep 0.5
    done
    if [ "$ready" -ne 1 ]; then
        echo "ERROR: server did not become ready at $BASE_URL within 15s; log:" >&2
        cat "$OUT_DIR/server.log" >&2
        exit 1
    fi
    echo "server up at $BASE_URL (pid $SERVER_PID, max_concurrency=$MAX_CONCURRENCY)"
fi

SHED_URL="$BASE_URL/$TENANT/features/catalogs/$CATALOG/collections/$COLLECTION/items?limit=1"

baseline_code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 "$SHED_URL")"
if [ "$baseline_code" != "200" ]; then
    echo "ERROR: baseline request to $SHED_URL returned $baseline_code, expected 200 -- is '$COLLECTION' really seeded?" >&2
    exit 1
fi
echo "baseline OK: $SHED_URL -> 200"

# --- overload burst + interleaved probes ----------------------------------
BURST_JSON="$OUT_DIR/burst.json"
PROBES_FILE="$OUT_DIR/probes.txt"
: > "$PROBES_FILE"

echo "burst: -c $BURST_CONCURRENCY -z $BURST_DURATION against $SHED_URL"
oha --no-tui --output-format json -c "$BURST_CONCURRENCY" -z "$BURST_DURATION" -o "$BURST_JSON" "$SHED_URL" &
BURST_PID=$!

# Duration in whole seconds for spacing the probes; oha accepts "5s"/"5"/"1m".
burst_secs="$(printf '%s' "$BURST_DURATION" | sed -E 's/[^0-9.]*$//')"
case "$BURST_DURATION" in
    *m) burst_secs=$(awk -v m="$burst_secs" 'BEGIN{print m*60}') ;;
esac
[ -n "$burst_secs" ] || burst_secs=5
sleep_between="$(awk -v d="$burst_secs" -v n="$PROBES" 'BEGIN{v=d/(n+1); if (v<0.05) v=0.05; print v}')"

sleep "$sleep_between"
p=1
while [ "$p" -le "$PROBES" ]; do
    curl -s -o /dev/null -w '%{http_code} %{time_total}\n' --max-time 5 "$SHED_URL" >> "$PROBES_FILE" || true
    sleep "$sleep_between"
    p=$((p + 1))
done

wait "$BURST_PID"

# --- assertion 1: 503s actually occurred -----------------------------------
burst_503="$(jq -r '.statusCodeDistribution["503"] // 0' "$BURST_JSON")"
burst_200="$(jq -r '.statusCodeDistribution["200"] // 0' "$BURST_JSON")"
probe_503_count="$(awk '$1 == 503 {c++} END{print c+0}' "$PROBES_FILE")"

echo "burst results: 200s=$burst_200 503s=$burst_503; probes: $(wc -l < "$PROBES_FILE" | tr -d ' ') fired, $probe_503_count got 503"

pass=1
if [ "$burst_503" -eq 0 ] && [ "$probe_503_count" -eq 0 ]; then
    echo "FAIL: no 503 observed anywhere -- load-shed never fired (MAX_CONCURRENCY=$MAX_CONCURRENCY, BURST_CONCURRENCY=$BURST_CONCURRENCY too close together?)" >&2
    pass=0
else
    echo "PASS: load-shed fired ($burst_503 503s in the burst, $probe_503_count in the timed probes)"
fi

# --- assertion 2: the 503s were fast, not queued -----------------------
if [ "$probe_503_count" -gt 0 ]; then
    slow_503_s="$(awk -v ms="$SLOW_503_MS" 'BEGIN{print ms/1000}')"
    max_503_time="$(awk '$1 == 503 {if ($2 > m) m = $2} END{print m+0}' "$PROBES_FILE")"
    if awk -v m="$max_503_time" -v t="$slow_503_s" 'BEGIN{exit !(m > t)}'; then
        echo "FAIL: slowest probed 503 took ${max_503_time}s, exceeds SLOW_503_MS=${SLOW_503_MS}ms -- looks queued, not shed" >&2
        pass=0
    else
        echo "PASS: probed 503s were fast (slowest ${max_503_time}s, under ${SLOW_503_MS}ms)"
    fi
else
    # The timed probes didn't happen to land on a shed request even though the
    # bulk burst above saw some -- fall back to the burst's own aggregate
    # latency (weaker: mixes 200s and 503s, but still catches a burst that's
    # queuing instead of shedding, since a shed 503 should not be dragging the
    # tail up toward the request timeout).
    echo "no probe landed on a 503; falling back to the burst's aggregate p99 for the 'fast' check"
    burst_p99_ms="$(jq -r '.metrics.latency_ms.p99 // empty' "$BURST_JSON")"
    if [ -n "$burst_p99_ms" ] && awk -v p="$burst_p99_ms" -v t="$SLOW_503_MS" 'BEGIN{exit !(p > t)}'; then
        echo "FAIL: burst p99 latency (${burst_p99_ms}ms) exceeds SLOW_503_MS=${SLOW_503_MS}ms -- looks queued, not shed" >&2
        pass=0
    else
        echo "PASS (weak signal): burst p99 latency ${burst_p99_ms:-unknown}ms stayed under ${SLOW_503_MS}ms"
    fi
fi

# --- assertion 3: recovers to 200 afterward -------------------------------
recovered=0
elapsed=0
while [ "$elapsed" -lt "$RECOVERY_TIMEOUT" ]; do
    code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 2 "$SHED_URL" || true)"
    if [ "$code" = "200" ]; then
        recovered=1
        break
    fi
    sleep 0.5
    elapsed=$((elapsed + 1))
done
if [ "$recovered" -eq 1 ]; then
    echo "PASS: server serves 200 again within ${RECOVERY_TIMEOUT}s of the burst ending"
else
    echo "FAIL: server did not recover to 200 within ${RECOVERY_TIMEOUT}s" >&2
    pass=0
fi

{
    echo "load-shed scenario: $([ "$pass" -eq 1 ] && echo PASS || echo FAIL)"
    echo "burst: c=$BURST_CONCURRENCY z=$BURST_DURATION url=$SHED_URL"
    echo "200s=$burst_200 503s=$burst_503 probe_503s=$probe_503_count/$PROBES"
} > "$OUT_DIR/summary.txt"

if [ "$pass" -eq 1 ]; then
    echo "load-shed scenario: PASS ($OUT_DIR/summary.txt)"
    exit 0
else
    echo "load-shed scenario: FAIL ($OUT_DIR/summary.txt)" >&2
    exit 1
fi
