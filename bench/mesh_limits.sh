#!/usr/bin/env bash
# Proves that the glb (3D places / meshing) lane actually respects the same
# per-zoom feature caps and load-shed ceiling as every other lane, instead of
# assuming the shared middleware/query-cap plumbing reaches it too. Meshing
# (MVT decode -> extrude -> ear-clip) is the first CPU+allocation-heavy lane
# in tellurion, so this is the one place a cap or the load-shed 503 path
# silently not engaging would actually cost something. See docs/benchmarking.md
# for how this fits alongside scenarios.sh and load_shed.sh.
#
# Always self-starts two short-lived servers of its own (never attaches to
# an already-running one the way load_shed.sh optionally does): the two
# assertions below need an *exact*, known per-zoom cap value, which only a
# config this script wrote itself can guarantee. Requires: bash, awk, jq,
# curl, oha, od, cargo (only if a binary needs building).
#
# What it proves, and how:
#
#   1. Per-zoom cap reaches the meshing path. The seeded "demo" collection
#      (tellurion-ingest seed) is a deterministic 25x20 grid alternating
#      POINT and POLYGON features (see tellurion-ingest/src/seed.rs); every
#      polygon is the same simple axis-aligned 4-vertex square. A single
#      global tile (z0/0/0) covers the whole grid. This script starts a
#      server with an explicit tiny cap (LOW_CAP) at z0 -- which the
#      operator-cap fallback chain (ZoomCaps::explicit) inherits at every
#      other zoom too, since no other zoom overrides it -- fetches that
#      tile's glb, and parses the returned glTF binary's own accessor
#      metadata (no protobuf/glTF library needed: the container's JSON
#      chunk directly states `accessors[0].count`, the vertex count, and
#      `accessors[1].count`, the index count -- see the byte layout in
#      `glb_json_chunk` below). A second server, identical except for a much
#      larger cap (HIGH_CAP, above the collection's total row count), serves
#      the same tile uncapped. Two assertions: the low-cap response stays
#      within a vertex ceiling derived from LOW_CAP (proves the cap bounds
#      the work, not just "happens to be small"), and the high-cap response
#      is strictly larger (proves the cap parameter isn't a no-op -- a
#      server that ignored `tiles.caps` entirely would produce the same
#      output either way).
#
#   2. Load-shed fires on the meshing lane under overload. Same mechanism
#      load_shed.sh proves for the features lane, pointed at the glb route
#      instead: the second (uncapped, heaviest-mesh) server also pins
#      `server.max_concurrency` low, then takes a burst of concurrent glb
#      requests against it. The load-shed layer sits ahead of every route in
#      `tellurion-server/src/app.rs` -- outermost, before the handler ever
#      runs -- so this is the same code path load_shed.sh already proves;
#      running it here specifically demonstrates the meshing lane was never
#      exempt from it.
#
#   3. No unbounded RSS growth. `/metrics` is scraped before and after the
#      burst, same best-effort, never-fatal scrape scenarios.sh uses.
#      Reported, not asserted against a threshold -- there's no established
#      "safe" growth number, and (per docs/benchmarking.md's "Known constraints")
#      the RSS gauge only exists on Linux, so this reads "n/a" here on
#      macOS. The real proof of boundedness is assertion 1: a request can
#      only ever mesh `cap` features' worth of geometry, on any host.
#
# Env vars (all optional, defaults shown):
#   DATABASE_URL      postgres://tellurion:tellurion@localhost:5433/tellurion
#   COLLECTION        demo                    must already exist with places3d
#                                              configured (see config/e2e.yaml's
#                                              own "demo" entry) -- the vertex
#                                              ceiling in assertion 1 is derived
#                                              from demo's known seed geometry;
#                                              overriding this to a collection
#                                              with more complex polygons will
#                                              likely fail that assertion
#   TENANT            public                  tenant external id
#   CATALOG           default                 catalog external id, scoped to TENANT
#   TELLURION_BIN     (auto)                  pre-built binary; skips the build step when set
#   PORT_LOW          18097                   port for the low-cap server (phase 1)
#   PORT_HIGH         18098                   port for the high-cap/load-shed server (phase 2)
#   LOW_CAP           10                      explicit tiles.caps z0 value for the low-cap server
#   HIGH_CAP          5000                    explicit tiles.caps z0 value for the high-cap server
#                                              (well above demo's 500 total rows -- effectively uncapped)
#   MAX_CONCURRENCY   4                       server.max_concurrency on the high-cap server
#   BURST_CONCURRENCY 40                      oha -c for the overload burst (keep above MAX_CONCURRENCY)
#   BURST_DURATION    5s                      oha -z for the overload burst
#   PROBES            10                      individual curl timing probes fired during the burst
#   SLOW_503_MS       1000                    a probe 503 slower than this fails the "fast" assertion
#   RECOVERY_TIMEOUT  15                      seconds to poll for the server to serve 200 again
#   RSS_METRIC        process_resident_memory_bytes   Prometheus metric name for the RSS scrape
#   OUT_DIR           bench/results/meshlimits-<UTC timestamp>
#
# Usage (smoke scale, self-started servers):
#   ./bench/mesh_limits.sh
#
# This always runs at smoke scale regardless of the env vars above -- a
# single glb request per cap plus one short burst. A real capacity number
# for the meshing lane (as opposed to "does the ceiling engage at all") is a
# separate, future exercise on a quiet host, same caveat scenarios.sh and
# load_shed.sh both carry.

set -eu
export LC_ALL=C

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

DATABASE_URL="${DATABASE_URL:-postgres://tellurion:tellurion@localhost:5433/tellurion}"
COLLECTION="${COLLECTION:-demo}"
TENANT="${TENANT:-public}"
CATALOG="${CATALOG:-default}"
PORT_LOW="${PORT_LOW:-18097}"
PORT_HIGH="${PORT_HIGH:-18098}"
LOW_CAP="${LOW_CAP:-10}"
HIGH_CAP="${HIGH_CAP:-5000}"
MAX_CONCURRENCY="${MAX_CONCURRENCY:-4}"
BURST_CONCURRENCY="${BURST_CONCURRENCY:-40}"
BURST_DURATION="${BURST_DURATION:-5s}"
PROBES="${PROBES:-10}"
SLOW_503_MS="${SLOW_503_MS:-1000}"
RECOVERY_TIMEOUT="${RECOVERY_TIMEOUT:-15}"
RSS_METRIC="${RSS_METRIC:-process_resident_memory_bytes}"
OUT_DIR="${OUT_DIR:-$SCRIPT_DIR/results/meshlimits-$(date -u +%Y%m%dT%H%M%SZ)}"

# Vertices a single demo polygon can contribute to a glb mesh, at most:
# `add_prism` (tellurion-render/src/extrude.rs) gives a hole-free ring its
# own top+bottom cap vertices (2 * ring length) plus its own top+bottom wall
# vertices (2 * ring length again), and every demo polygon is a 4-point
# square ring (tellurion-ingest/src/seed.rs's `square_wkt`) -- so 4*4 = 16
# vertices per polygon. Simplification (`ST_SimplifyPreserveTopology`, run
# before extrusion) can only ever remove ring points, never add them, so 16
# is a safe ceiling even if a given zoom's tolerance shaves a polygon down
# further -- not a claim that every polygon always keeps all 4 points.
DEMO_MAX_VERTICES_PER_POLYGON=16

for tool in awk jq curl oha od; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "ERROR: required tool '$tool' not found on PATH" >&2
        exit 1
    }
done

mkdir -p "$OUT_DIR"
echo "results -> $OUT_DIR"

SERVER_PID=""
CONFIG_FILES=""
cleanup() {
    if [ -n "$SERVER_PID" ]; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
    fi
    # shellcheck disable=SC2086 # CONFIG_FILES is a space-joined path list, deliberately unquoted
    [ -n "$CONFIG_FILES" ] && rm -f $CONFIG_FILES
}
trap cleanup EXIT

# --- binary resolution (build once, reuse for both servers) --------------
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

# write_config PATH PORT CAP [MAX_CONCURRENCY] -- a minimal single-collection
# config for `demo`, places3d-enabled the same way config/e2e.yaml's own
# "demo" entry is (no `height` column on the seeded table, so every
# footprint falls back to default_height), with an explicit z0 feature cap.
write_config() {
    path="$1"; port="$2"; cap="$3"; maxconc="${4:-}"
    {
        echo "server:"
        echo "  port: $port"
        echo "  request_timeout_s: 60"
        if [ -n "$maxconc" ]; then
            echo "  max_concurrency: $maxconc"
        fi
        cat <<EOF
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
    places3d:
      height_property: "height"
      default_height: 8.0
    tiles: { minzoom: 0, maxzoom: 14, caps: { z0: $cap } }
EOF
    } > "$path"
    CONFIG_FILES="$CONFIG_FILES $path"
}

# start_server CONFIG PORT -- boots the binary against CONFIG, polls until
# it answers on PORT or fails after 15s. Sets SERVER_PID.
start_server() {
    config="$1"; port="$2"
    DATABASE_URL="$DATABASE_URL" TELLURION_CONFIG="$config" \
        "$bin" > "$OUT_DIR/server_$port.log" 2>&1 &
    SERVER_PID=$!

    ready=0
    for _ in $(seq 1 30); do
        if curl -sf --max-time 2 "http://127.0.0.1:$port/" -o /dev/null 2>/dev/null; then
            ready=1
            break
        fi
        sleep 0.5
    done
    if [ "$ready" -ne 1 ]; then
        echo "ERROR: server did not become ready on port $port within 15s; log:" >&2
        cat "$OUT_DIR/server_$port.log" >&2
        exit 1
    fi
}

# stop_server -- kills and reaps SERVER_PID (if any), leaves it unset.
stop_server() {
    if [ -n "$SERVER_PID" ]; then
        kill "$SERVER_PID" 2>/dev/null || true
        wait "$SERVER_PID" 2>/dev/null || true
        SERVER_PID=""
    fi
}

tile_url() {
    echo "http://127.0.0.1:$1/$TENANT/3dtiles/catalogs/$CATALOG/collections/$COLLECTION/3dtiles/tiles/0/0/0.glb"
}

# glb_json_chunk_len GLB_FILE -- the JSON chunk's byte length, decoded from
# the 4-byte little-endian u32 at byte offset 12 of a glTF 2.0 binary
# container (header: magic[4] + version[4] + total_length[4], then this
# chunk's own length[4] + type[4] "JSON" -- see tellurion-render/src/mesh.rs
# `Mesh::to_glb`). `NF==4` skips BSD od's trailing blank summary line.
glb_json_chunk_len() {
    od -An -tu1 -j 12 -N 4 "$1" | awk 'NF==4{print $1 + $2*256 + $3*65536 + $4*16777216; exit}'
}

# glb_json_chunk GLB_FILE -- the JSON chunk itself: starts at byte 20 (12
# header bytes + 8 chunk-header bytes), may be space-padded to a 4-byte
# boundary (trailing whitespace jq ignores).
glb_json_chunk() {
    len="$(glb_json_chunk_len "$1")"
    tail -c +21 "$1" | head -c "$len"
}

glb_vertex_count() { glb_json_chunk "$1" | jq -r '.accessors[0].count // 0'; }
glb_index_count() { glb_json_chunk "$1" | jq -r '.accessors[1].count // 0'; }

# scrape_metrics NAME PORT PHASE -- best-effort Prometheus scrape, never fatal.
scrape_metrics() {
    curl -s --max-time 5 "http://127.0.0.1:$2/metrics" -o "$OUT_DIR/$1.metrics_$3.prom" || true
}

pass=1

# --- phase 1: low cap ------------------------------------------------------
echo "== phase 1: glb tile with tiles.caps.z0=$LOW_CAP =="
low_config="$(mktemp -t tellurion-mesh-limits-low.XXXXXX.yaml)"
write_config "$low_config" "$PORT_LOW" "$LOW_CAP"
start_server "$low_config" "$PORT_LOW"

low_glb="$OUT_DIR/z0_low_cap.glb"
low_code="$(curl -s -o "$low_glb" -w '%{http_code}' --max-time 10 "$(tile_url "$PORT_LOW")")"
stop_server

if [ "$low_code" != "200" ]; then
    echo "ERROR: low-cap glb request returned $low_code, expected 200 -- is '$COLLECTION' really seeded with places3d configured?" >&2
    exit 1
fi
low_vertices="$(glb_vertex_count "$low_glb")"
low_indices="$(glb_index_count "$low_glb")"
echo "low-cap tile: $low_vertices vertices, $low_indices indices"

# --- phase 2: high cap, then load-shed burst against it --------------------
echo "== phase 2: glb tile with tiles.caps.z0=$HIGH_CAP, then an overload burst (max_concurrency=$MAX_CONCURRENCY) =="
high_config="$(mktemp -t tellurion-mesh-limits-high.XXXXXX.yaml)"
write_config "$high_config" "$PORT_HIGH" "$HIGH_CAP" "$MAX_CONCURRENCY"
start_server "$high_config" "$PORT_HIGH"

high_glb="$OUT_DIR/z0_high_cap.glb"
high_code="$(curl -s -o "$high_glb" -w '%{http_code}' --max-time 10 "$(tile_url "$PORT_HIGH")")"
if [ "$high_code" != "200" ]; then
    echo "ERROR: high-cap glb baseline request returned $high_code, expected 200" >&2
    stop_server
    exit 1
fi
high_vertices="$(glb_vertex_count "$high_glb")"
high_indices="$(glb_index_count "$high_glb")"
echo "high-cap tile: $high_vertices vertices, $high_indices indices"

GLB_URL="$(tile_url "$PORT_HIGH")"
scrape_metrics "burst" "$PORT_HIGH" before

BURST_JSON="$OUT_DIR/burst.json"
PROBES_FILE="$OUT_DIR/probes.txt"
: > "$PROBES_FILE"

echo "burst: -c $BURST_CONCURRENCY -z $BURST_DURATION against $GLB_URL"
oha --no-tui --output-format json -c "$BURST_CONCURRENCY" -z "$BURST_DURATION" -o "$BURST_JSON" "$GLB_URL" &
BURST_PID=$!

burst_secs="$(printf '%s' "$BURST_DURATION" | sed -E 's/[^0-9.]*$//')"
case "$BURST_DURATION" in
    *m) burst_secs="$(awk -v m="$burst_secs" 'BEGIN{print m*60}')" ;;
esac
[ -n "$burst_secs" ] || burst_secs=5
sleep_between="$(awk -v d="$burst_secs" -v n="$PROBES" 'BEGIN{v=d/(n+1); if (v<0.05) v=0.05; print v}')"

sleep "$sleep_between"
p=1
while [ "$p" -le "$PROBES" ]; do
    curl -s -o /dev/null -w '%{http_code} %{time_total}\n' --max-time 5 "$GLB_URL" >> "$PROBES_FILE" || true
    sleep "$sleep_between"
    p=$((p + 1))
done

wait "$BURST_PID"
scrape_metrics "burst" "$PORT_HIGH" after

# --- assertion 1: low-cap stayed within a cap-derived ceiling --------------
max_allowed=$((LOW_CAP * DEMO_MAX_VERTICES_PER_POLYGON))
if [ "$low_vertices" -gt 0 ] && [ "$low_vertices" -le "$max_allowed" ]; then
    echo "PASS: low-cap glb tile stayed within the cap-derived ceiling ($low_vertices vertices <= $LOW_CAP features * $DEMO_MAX_VERTICES_PER_POLYGON vertices/feature = $max_allowed)"
else
    echo "FAIL: low-cap glb tile vertex count ($low_vertices) is 0 or exceeds $max_allowed -- the per-zoom cap does not appear to be reaching the meshing path" >&2
    pass=0
fi

# --- assertion 2: the cap isn't a no-op -------------------------------------
if [ "$high_vertices" -gt "$low_vertices" ]; then
    echo "PASS: raising tiles.caps.z0 from $LOW_CAP to $HIGH_CAP produced more mesh content ($low_vertices -> $high_vertices vertices)"
else
    echo "FAIL: raising tiles.caps.z0 from $LOW_CAP to $HIGH_CAP did not increase mesh content ($low_vertices -> $high_vertices vertices) -- the cap may be applied uniformly regardless of configured value" >&2
    pass=0
fi

# --- assertion 3: load-shed actually fired ----------------------------------
burst_503="$(jq -r '.statusCodeDistribution["503"] // 0' "$BURST_JSON")"
burst_200="$(jq -r '.statusCodeDistribution["200"] // 0' "$BURST_JSON")"
probe_503_count="$(awk '$1 == 503 {c++} END{print c+0}' "$PROBES_FILE")"

echo "burst results: 200s=$burst_200 503s=$burst_503; probes: $(wc -l < "$PROBES_FILE" | tr -d ' ') fired, $probe_503_count got 503"

if [ "$burst_503" -eq 0 ] && [ "$probe_503_count" -eq 0 ]; then
    echo "FAIL: no 503 observed anywhere -- load-shed never fired on the glb lane (MAX_CONCURRENCY=$MAX_CONCURRENCY, BURST_CONCURRENCY=$BURST_CONCURRENCY too close together?)" >&2
    pass=0
else
    echo "PASS: load-shed fired on the glb lane ($burst_503 503s in the burst, $probe_503_count in the timed probes)"
fi

# --- assertion 4: the 503s were fast, not queued ----------------------------
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
    echo "no probe landed on a 503; falling back to the burst's aggregate p99 for the 'fast' check"
    burst_p99_ms="$(jq -r '.metrics.latency_ms.p99 // empty' "$BURST_JSON")"
    if [ -n "$burst_p99_ms" ] && awk -v p="$burst_p99_ms" -v t="$SLOW_503_MS" 'BEGIN{exit !(p > t)}'; then
        echo "FAIL: burst p99 latency (${burst_p99_ms}ms) exceeds SLOW_503_MS=${SLOW_503_MS}ms -- looks queued, not shed" >&2
        pass=0
    else
        echo "PASS (weak signal): burst p99 latency ${burst_p99_ms:-unknown}ms stayed under ${SLOW_503_MS}ms"
    fi
fi

# --- assertion 5: recovers to 200 afterward ---------------------------------
recovered=0
elapsed=0
while [ "$elapsed" -lt "$RECOVERY_TIMEOUT" ]; do
    code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 2 "$GLB_URL" || true)"
    if [ "$code" = "200" ]; then
        recovered=1
        break
    fi
    sleep 0.5
    elapsed=$((elapsed + 1))
done
if [ "$recovered" -eq 1 ]; then
    echo "PASS: the glb lane serves 200 again within ${RECOVERY_TIMEOUT}s of the burst ending"
else
    echo "FAIL: the glb lane did not recover to 200 within ${RECOVERY_TIMEOUT}s" >&2
    pass=0
fi

stop_server

# --- RSS: reported, not asserted (see header comment) -----------------------
rss_before="$(grep -E "^${RSS_METRIC}([ {]|$)" "$OUT_DIR/burst.metrics_before.prom" 2>/dev/null | tail -1 | awk '{print $NF}')"
rss_after="$(grep -E "^${RSS_METRIC}([ {]|$)" "$OUT_DIR/burst.metrics_after.prom" 2>/dev/null | tail -1 | awk '{print $NF}')"
if [ -n "${rss_before:-}" ] && [ -n "${rss_after:-}" ]; then
    rss_delta_mib="$(awk -v a="$rss_after" -v b="$rss_before" 'BEGIN{printf "%+.2f", (a-b)/1048576}')"
    echo "RSS across the burst: ${rss_delta_mib} MiB (informational -- see header comment)"
else
    echo "RSS across the burst: n/a (RSS is Linux-only -- see docs/benchmarking.md's Known constraints)"
    rss_delta_mib="n/a"
fi

{
    echo "mesh-limits scenario: $([ "$pass" -eq 1 ] && echo PASS || echo FAIL)"
    echo "low cap=$LOW_CAP vertices=$low_vertices indices=$low_indices"
    echo "high cap=$HIGH_CAP vertices=$high_vertices indices=$high_indices"
    echo "burst: c=$BURST_CONCURRENCY z=$BURST_DURATION url=$GLB_URL"
    echo "200s=$burst_200 503s=$burst_503 probe_503s=$probe_503_count/$PROBES"
    echo "rss_delta_mib=$rss_delta_mib"
} > "$OUT_DIR/summary.txt"

if [ "$pass" -eq 1 ]; then
    echo "mesh-limits scenario: PASS ($OUT_DIR/summary.txt)"
    exit 0
else
    echo "mesh-limits scenario: FAIL ($OUT_DIR/summary.txt)" >&2
    exit 1
fi
