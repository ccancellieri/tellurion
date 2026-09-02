#!/usr/bin/env bash
# Tellurion benchmark runner. Drives oha against a running tellurion (or any
# OGC API Tiles/Features server -- per-lane URL templates below make servers
# with different path layouts drivable) and writes one JSON file per
# repetition into bench/results/<timestamp>/. See docs/benchmarking.md
# for the protocol these scenarios implement and the fairness rules around them.
#
# Requires: bash, awk, jq, curl, oha (>=1.9, tested against 1.15.0).
#
# Env vars (all optional, defaults shown):
#   BASE_URL      http://127.0.0.1:8080   server under test
#   COLLECTION    demo                    collection id to benchmark
#   TENANT        public                  tenant external id (see /{tenant}/{protocol}/catalogs/{catalog}/...)
#   CATALOG       default                 catalog external id, scoped to TENANT
#   ZMAX          14                      top zoom of the MVT/PNG sweep (sweep runs 0..ZMAX)
#   DURATION      10s                     oha -z value, applied to every run
#   CONCURRENCY   50                      oha -c value for single-shape scenarios
#   SEED          42                      base seed for the tile walk (reproducible)
#   REPS          3                       measured repetitions per scenario (summarize.sh takes the median)
#   WARMUP        1                       extra leading repetitions per scenario, discarded
#   WALK_COUNT    24                      distinct tile coordinates per zoom in the cold/warm sweep regex
#   MAXSTEP       2                       max per-axis tile step in the cold/warm walk
#   DB_ZMIN       10                      bottom zoom of the db-path sweep (see "Known constraints" in the README
#                                         for why cold/warm above don't honestly measure the DB path)
#   DB_ZMAX       14                      top zoom of the db-path sweep
#   DB_WALK_COUNT 1000                    distinct tile coordinates per (zoom, rep) in the db-path sweep, drawn
#                                         uniformly across the whole bbox-derived index range (cache-busting)
#   MIXED_ZOOM    10                      representative zoom used for the tile share of the mixed scenario
#   BBOX          (auto-discovered)       "west,south,east,north" override, skips the collection metadata call
#   ITEM_ID       (auto-discovered)       single-item id override, skips the items?limit=1 call
#   RSS_METRIC    process_resident_memory_bytes   Prometheus metric name scraped before/after each scenario
#   OUT_DIR       bench/results/<UTC timestamp>   where JSON + metrics land
#   COLLECTION_URL  $BASE_URL/$TENANT/features/catalogs/$CATALOG/collections/$COLLECTION   collection metadata
#                                                        URL (reachability + bbox discovery)
#   ITEMS_URL       $COLLECTION_URL/items               items endpoint; "?limit=N" and "/{id}" are appended
#   MVT_TILE_TMPL   (tellurion shape, see below)        MVT rand-regex URL template. "{z}" and "{alt}" are
#                                                        substituted per (zoom, rep); the result is fed to
#                                                        oha --rand-regex-url. Escape "?" as \? (else it is
#                                                        a regex quantifier), but write "." PLAIN: oha
#                                                        disables the dot metachar so a bare dot is already
#                                                        literal, while an escaped \. is generated as a
#                                                        random character (verified against oha 1.15.0).
#   PNG_TILE_TMPL   (tellurion shape, see below)        PNG template, same substitution and escaping rules;
#                                                        may target a different host:port than BASE_URL
#   METRICS_URL     $BASE_URL/metrics                    Prometheus scrape URL (best-effort, never fatal)
#   AUTH_HEADER     (unset)                              e.g. "Authorization: Bearer <token>"; sent on every
#                                                        curl and oha request when set
#   TILE_COORD_ORDER yx                                  order of the two coordinates inside {alt}: "yx" =
#                                                        tileRow/tileCol (OGC API Tiles), "xy" = col/row
#   BENCH_3D      0                       set to 1 to also run the glb + styled-PNG sweeps (v0.2, off by
#                                          default so existing runs/results are unchanged)
#   ZMIN_3D       4                       bottom zoom of the glb/styled-PNG sweeps
#   ZMAX_3D       12                      top zoom of the glb/styled-PNG sweeps (narrower than ZMAX -- glb
#                                          meshing is the first CPU+allocation-heavy lane, see design doc)
#   STYLE_ID      default                 style id for the styled-PNG lane; must be a registered id in the
#                                          target server's `styles:` config
#   LOAD_THRESHOLD_FACTOR   1.5           warn (see REFUSE_ON_HIGH_LOAD) when 1-min load average exceeds this
#                                         many times the CPU count -- something else is competing for the host
#   REFUSE_ON_HIGH_LOAD     0             set to 1 to exit instead of just warning on high load average
#
# Quick smoke check (few minutes, not a real measurement):
#   ZMAX=3 DB_ZMAX=11 REPS=1 WARMUP=0 DURATION=3s CONCURRENCY=10 ./scenarios.sh
#
# 3D smoke check (glb + styled-PNG lanes, requires places3d/styles configured):
#   BENCH_3D=1 ZMIN_3D=4 ZMAX_3D=5 STYLE_ID=default REPS=1 WARMUP=0 DURATION=3s CONCURRENCY=10 ./scenarios.sh

set -eu
export LC_ALL=C

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
LIB_DIR="$SCRIPT_DIR/lib"

BASE_URL="${BASE_URL:-http://127.0.0.1:8080}"
COLLECTION="${COLLECTION:-demo}"
TENANT="${TENANT:-public}"
CATALOG="${CATALOG:-default}"
ZMAX="${ZMAX:-14}"
DURATION="${DURATION:-10s}"
CONCURRENCY="${CONCURRENCY:-50}"
SEED="${SEED:-42}"
REPS="${REPS:-3}"
WARMUP="${WARMUP:-1}"
WALK_COUNT="${WALK_COUNT:-24}"
MAXSTEP="${MAXSTEP:-2}"
DB_ZMIN="${DB_ZMIN:-10}"
DB_ZMAX="${DB_ZMAX:-14}"
DB_WALK_COUNT="${DB_WALK_COUNT:-1000}"
MIXED_ZOOM="${MIXED_ZOOM:-10}"
RSS_METRIC="${RSS_METRIC:-process_resident_memory_bytes}"
OUT_DIR="${OUT_DIR:-$SCRIPT_DIR/results/$(date -u +%Y%m%dT%H%M%SZ)}"
BENCH_3D="${BENCH_3D:-0}"
ZMIN_3D="${ZMIN_3D:-4}"
ZMAX_3D="${ZMAX_3D:-12}"
STYLE_ID="${STYLE_ID:-default}"
LOAD_THRESHOLD_FACTOR="${LOAD_THRESHOLD_FACTOR:-1.5}"
REFUSE_ON_HIGH_LOAD="${REFUSE_ON_HIGH_LOAD:-0}"
COLLECTION_URL="${COLLECTION_URL:-$BASE_URL/$TENANT/features/catalogs/$CATALOG/collections/$COLLECTION}"
ITEMS_URL="${ITEMS_URL:-$COLLECTION_URL/items}"
# Plain conditional assignment, NOT ${VAR:-default}: the "}" of the "{z}"
# placeholder would terminate a ${...:-...} expansion early and corrupt the
# default template.
if [ -z "${MVT_TILE_TMPL:-}" ]; then
    MVT_TILE_TMPL="$BASE_URL/$TENANT/tiles/catalogs/$CATALOG/collections/$COLLECTION/tiles/WebMercatorQuad/{z}/({alt})\\?f=mvt"
fi
if [ -z "${PNG_TILE_TMPL:-}" ]; then
    PNG_TILE_TMPL="$BASE_URL/$TENANT/tiles/catalogs/$CATALOG/collections/$COLLECTION/tiles/WebMercatorQuad/{z}/({alt})\\?f=png"
fi
METRICS_URL="${METRICS_URL:-$BASE_URL/metrics}"
AUTH_HEADER="${AUTH_HEADER:-}"
TILE_COORD_ORDER="${TILE_COORD_ORDER:-yx}"

case "$TILE_COORD_ORDER" in
    yx|xy) ;;
    *)
        echo "ERROR: TILE_COORD_ORDER must be 'yx' or 'xy', got '$TILE_COORD_ORDER'" >&2
        exit 1
        ;;
esac

# Optional auth header for every curl/oha request. The ${arr[@]+...} expansion
# form stays safe under set -u on bash 3.2 when the array is empty.
AUTH_ARGS=()
if [ -n "$AUTH_HEADER" ]; then
    AUTH_ARGS=(-H "$AUTH_HEADER")
fi

for tool in awk jq curl oha; do
    command -v "$tool" >/dev/null 2>&1 || {
        echo "ERROR: required tool '$tool' not found on PATH" >&2
        exit 1
    }
done

if ! curl -sf --max-time 5 ${AUTH_ARGS[@]+"${AUTH_ARGS[@]}"} "$COLLECTION_URL" -o /dev/null; then
    echo "ERROR: cannot reach $COLLECTION_URL (is the server up?)" >&2
    exit 1
fi

# --- host load preflight -------------------------------------------------
# A bench host with something else competing for CPU produces numbers that
# look like tellurion regressed when the real cause is contention. Warn (or,
# with REFUSE_ON_HIGH_LOAD=1, refuse) rather than silently recording it.
cpu_count() {
    if command -v nproc >/dev/null 2>&1; then
        nproc
    elif command -v sysctl >/dev/null 2>&1; then
        sysctl -n hw.ncpu 2>/dev/null
    fi
}

load_average_1min() {
    if [ -r /proc/loadavg ]; then
        awk '{print $1}' /proc/loadavg
    elif command -v sysctl >/dev/null 2>&1; then
        # macOS/BSD: "{ 1.23 1.45 1.67 }" -- the 1-minute figure is the first
        # number after the opening brace.
        sysctl -n vm.loadavg 2>/dev/null | awk '{print $2}'
    fi
}

cores="$(cpu_count || true)"
load1="$(load_average_1min || true)"
if [ -n "${cores:-}" ] && [ -n "${load1:-}" ]; then
    if awk -v l="$load1" -v c="$cores" -v f="$LOAD_THRESHOLD_FACTOR" \
        'BEGIN{exit !(l > c * f)}'; then
        echo "WARNING: 1-min load average ($load1) exceeds ${LOAD_THRESHOLD_FACTOR}x the CPU count ($cores) -- something else is competing for this host; bench numbers will not be trustworthy" >&2
        if [ "$REFUSE_ON_HIGH_LOAD" = "1" ]; then
            echo "ERROR: refusing to run (REFUSE_ON_HIGH_LOAD=1)" >&2
            exit 1
        fi
    fi
fi

mkdir -p "$OUT_DIR"
echo "results -> $OUT_DIR"

# --- bbox discovery -----------------------------------------------------
if [ -n "${BBOX:-}" ]; then
    WEST="$(echo "$BBOX" | cut -d, -f1)"
    SOUTH="$(echo "$BBOX" | cut -d, -f2)"
    EAST="$(echo "$BBOX" | cut -d, -f3)"
    NORTH="$(echo "$BBOX" | cut -d, -f4)"
else
    BBOX_CSV="$(curl -sf --max-time 5 ${AUTH_ARGS[@]+"${AUTH_ARGS[@]}"} "$COLLECTION_URL" \
        | jq -r '.extent.spatial.bbox[0] | @csv' 2>/dev/null || true)"
    if [ -z "$BBOX_CSV" ]; then
        echo "ERROR: could not discover bbox from $COLLECTION_URL; set BBOX=west,south,east,north" >&2
        exit 1
    fi
    WEST="$(echo "$BBOX_CSV" | cut -d, -f1)"
    SOUTH="$(echo "$BBOX_CSV" | cut -d, -f2)"
    EAST="$(echo "$BBOX_CSV" | cut -d, -f3)"
    NORTH="$(echo "$BBOX_CSV" | cut -d, -f4)"
fi
echo "bbox: west=$WEST south=$SOUTH east=$EAST north=$NORTH"

# --- item id discovery ---------------------------------------------------
HAVE_ITEM_ID=1
if [ -z "${ITEM_ID:-}" ]; then
    ITEM_ID="$(curl -sf --max-time 5 ${AUTH_ARGS[@]+"${AUTH_ARGS[@]}"} "$ITEMS_URL?limit=1" \
        | jq -r '.features[0].id // empty' 2>/dev/null || true)"
fi
if [ -z "$ITEM_ID" ]; then
    echo "WARNING: no item id available (set ITEM_ID or seed a non-empty collection); skipping item_by_id and the item share of mixed_70_20_10" >&2
    HAVE_ITEM_ID=0
fi

TOTAL_REPS=$((WARMUP + REPS))

# scrape_metrics NAME PHASE -- best-effort Prometheus scrape, never fatal.
scrape_metrics() {
    curl -s --max-time 5 ${AUTH_ARGS[@]+"${AUTH_ARGS[@]}"} "$METRICS_URL" -o "$OUT_DIR/$1.metrics_$2.prom" || true
}

# rep_file NAME INDEX -- warmup* files sort before rep* files is irrelevant;
# summarize.sh only ever globs "*.rep*.json".
rep_file() {
    name="$1"; idx="$2"
    if [ "$idx" -le "$WARMUP" ]; then
        echo "$OUT_DIR/$name.warmup$idx.json"
    else
        echo "$OUT_DIR/$name.rep$((idx - WARMUP)).json"
    fi
}

# build_alt Z SEED COUNT MAXSTEP -- "row/col|row/col|..." fragment for the
# given zoom, via the deterministic tile walk in lib/tilewalk.awk.
# TILE_COORD_ORDER=xy flips each pair to "col/row" for servers whose tile
# path puts the column first.
build_alt() {
    awk -f "$LIB_DIR/tilewalk.awk" -v seed="$2" -v z="$1" \
        -v west="$WEST" -v south="$SOUTH" -v east="$EAST" -v north="$NORTH" \
        -v count="$3" -v maxstep="$4" \
        | awk -v ord="$TILE_COORD_ORDER" 'BEGIN{ORS=""}
            {a=$3; b=$2; if (ord=="xy") {a=$2; b=$3}
             printf "%s%s/%s", (NR>1?"|":""), a, b} END{print ""}'
}

# build_alt_uniform Z SEED COUNT -- like build_alt, but each coordinate is
# drawn independently and uniformly across the whole bbox-derived tile-index
# range at that zoom (lib/tilewalk.awk's "uniform" mode), instead of a small
# walk around the bbox center. Cache-busting: used by the db-path sweep.
build_alt_uniform() {
    awk -f "$LIB_DIR/tilewalk.awk" -v seed="$2" -v z="$1" \
        -v west="$WEST" -v south="$SOUTH" -v east="$EAST" -v north="$NORTH" \
        -v count="$3" -v mode="uniform" \
        | awk -v ord="$TILE_COORD_ORDER" 'BEGIN{ORS=""}
            {a=$3; b=$2; if (ord=="xy") {a=$2; b=$3}
             printf "%s%s/%s", (NR>1?"|":""), a, b} END{print ""}'
}

# subst_tile_url TMPL Z ALT -- substitute {z} and {alt} into a rand-regex URL
# template. Bash expansion, not sed: ALT carries "|" alternation separators
# that would collide with any sed delimiter.
subst_tile_url() {
    tmpl="${1//\{z\}/$2}"
    printf '%s' "${tmpl//\{alt\}/$3}"
}

# run_plain NAME URL CONCURRENCY -- fixed-URL scenario (no coordinate variety).
run_plain() {
    name="$1"; url="$2"; conc="$3"
    scrape_metrics "$name" before
    i=1
    while [ "$i" -le "$TOTAL_REPS" ]; do
        out="$(rep_file "$name" "$i")"
        oha --no-tui --output-format json ${AUTH_ARGS[@]+"${AUTH_ARGS[@]}"} -c "$conc" -z "$DURATION" -o "$out" "$url"
        i=$((i + 1))
    done
    scrape_metrics "$name" after
}

# run_regex NAME URL_REGEX CONCURRENCY -- rand-regex-url scenario (coordinate sweep).
run_regex() {
    name="$1"; url="$2"; conc="$3"
    scrape_metrics "$name" before
    i=1
    while [ "$i" -le "$TOTAL_REPS" ]; do
        out="$(rep_file "$name" "$i")"
        oha --no-tui --rand-regex-url --output-format json ${AUTH_ARGS[@]+"${AUTH_ARGS[@]}"} -c "$conc" -z "$DURATION" -o "$out" "$url"
        i=$((i + 1))
    done
    scrape_metrics "$name" after
}

echo "== MVT sweep, cached lane (cold/warm) z0..$ZMAX =="
# "Cold" here means "never-reused coordinates within this scenario", not
# "guaranteed cache miss" -- lib/tilewalk.awk's default walk mode clusters
# every (zoom, rep) coordinate set tightly around the same bbox center, so in
# practice both cold and warm mostly measure the cache-hit path. That's a
# legitimate thing to measure (see docs/benchmarking.md), just not the DB path --
# the "MVT sweep, db-path" section below is the honest DB-path measurement.
z=0
while [ "$z" -le "$ZMAX" ]; do
    # Cold: a fresh, never-reused coordinate set per (zoom, rep) so every
    # measured request is a genuine cache miss even across repetitions.
    i=1
    cold_name="mvt_cold_z$z"
    scrape_metrics "$cold_name" before
    while [ "$i" -le "$TOTAL_REPS" ]; do
        rep_seed=$((SEED + z * 10000 + i))
        alt="$(build_alt "$z" "$rep_seed" "$WALK_COUNT" "$MAXSTEP")"
        url="$(subst_tile_url "$MVT_TILE_TMPL" "$z" "$alt")"
        out="$(rep_file "$cold_name" "$i")"
        oha --no-tui --rand-regex-url --output-format json ${AUTH_ARGS[@]+"${AUTH_ARGS[@]}"} -c "$CONCURRENCY" -z "$DURATION" -o "$out" "$url"
        i=$((i + 1))
    done
    scrape_metrics "$cold_name" after

    # Warm: identical per-(zoom, rep) seed formula -> identical tile set,
    # requested again now that the cold pass above has populated the cache.
    i=1
    warm_name="mvt_warm_z$z"
    scrape_metrics "$warm_name" before
    while [ "$i" -le "$TOTAL_REPS" ]; do
        rep_seed=$((SEED + z * 10000 + i))
        alt="$(build_alt "$z" "$rep_seed" "$WALK_COUNT" "$MAXSTEP")"
        url="$(subst_tile_url "$MVT_TILE_TMPL" "$z" "$alt")"
        out="$(rep_file "$warm_name" "$i")"
        oha --no-tui --rand-regex-url --output-format json ${AUTH_ARGS[@]+"${AUTH_ARGS[@]}"} -c "$CONCURRENCY" -z "$DURATION" -o "$out" "$url"
        i=$((i + 1))
    done
    scrape_metrics "$warm_name" after

    z=$((z + 1))
done

echo "== MVT sweep, db-path lane (honest cache-miss measurement) z$DB_ZMIN..$DB_ZMAX =="
# Each (zoom, rep) draws DB_WALK_COUNT tiles independently and uniformly
# across the *entire* bbox-derived tile-index range at that zoom (tilewalk's
# "uniform" mode) instead of a small neighborhood -- at z10+ over any
# non-trivial extent that keyspace is far larger than DB_WALK_COUNT times the
# repetition count, so repeat coordinates within a run are rare and the
# scenario stays DB-bound rather than sliding back into the cache-hit path.
# There's no cache-clear admin endpoint in v0.1 to force a true cold boot
# (see docs/benchmarking.md "Known constraints"); this is the honest bench-only
# substitute. Verify with the RSS delta this scenario's *.metrics_{before,
# after}.prom pair records: a scenario that's actually hitting the DB path
# and inserting fresh tiles should show the cache's resident size grow,
# unlike the cached lane above (already-warm, near-zero growth expected).
z=$DB_ZMIN
while [ "$z" -le "$DB_ZMAX" ]; do
    i=1
    dbpath_name="dbpath_mvt_z$z"
    scrape_metrics "$dbpath_name" before
    while [ "$i" -le "$TOTAL_REPS" ]; do
        rep_seed=$((SEED + 500000 + z * 10000 + i))
        alt="$(build_alt_uniform "$z" "$rep_seed" "$DB_WALK_COUNT")"
        url="$(subst_tile_url "$MVT_TILE_TMPL" "$z" "$alt")"
        out="$(rep_file "$dbpath_name" "$i")"
        oha --no-tui --rand-regex-url --output-format json ${AUTH_ARGS[@]+"${AUTH_ARGS[@]}"} -c "$CONCURRENCY" -z "$DURATION" -o "$out" "$url"
        i=$((i + 1))
    done
    scrape_metrics "$dbpath_name" after
    z=$((z + 1))
done

echo "== PNG sweep (same coords as the MVT sweep) z0..$ZMAX =="
z=0
while [ "$z" -le "$ZMAX" ]; do
    i=1
    png_name="png_z$z"
    scrape_metrics "$png_name" before
    while [ "$i" -le "$TOTAL_REPS" ]; do
        rep_seed=$((SEED + z * 10000 + i))
        alt="$(build_alt "$z" "$rep_seed" "$WALK_COUNT" "$MAXSTEP")"
        url="$(subst_tile_url "$PNG_TILE_TMPL" "$z" "$alt")"
        out="$(rep_file "$png_name" "$i")"
        oha --no-tui --rand-regex-url --output-format json ${AUTH_ARGS[@]+"${AUTH_ARGS[@]}"} -c "$CONCURRENCY" -z "$DURATION" -o "$out" "$url"
        i=$((i + 1))
    done
    scrape_metrics "$png_name" after
    z=$((z + 1))
done

if [ "$BENCH_3D" = "1" ]; then
    echo "== glb sweep (3D Tiles, cold/warm) z$ZMIN_3D..$ZMAX_3D =="
    z=$ZMIN_3D
    while [ "$z" -le "$ZMAX_3D" ]; do
        # Cold: same fresh-per-(zoom, rep) coordinate formula as the MVT sweep,
        # against the .glb lane (probe Glb cache -> miss -> MVT lane -> extrude
        # + mesh -> glb -> cache; see the 3D places design doc).
        i=1
        glb_cold_name="glb_cold_z$z"
        scrape_metrics "$glb_cold_name" before
        while [ "$i" -le "$TOTAL_REPS" ]; do
            rep_seed=$((SEED + z * 10000 + i))
            alt="$(build_alt "$z" "$rep_seed" "$WALK_COUNT" "$MAXSTEP")"
            url="$BASE_URL/$TENANT/3dtiles/catalogs/$CATALOG/collections/$COLLECTION/3dtiles/tiles/$z/($alt).glb"
            out="$(rep_file "$glb_cold_name" "$i")"
            oha --no-tui --rand-regex-url --output-format json ${AUTH_ARGS[@]+"${AUTH_ARGS[@]}"} -c "$CONCURRENCY" -z "$DURATION" -o "$out" "$url"
            i=$((i + 1))
        done
        scrape_metrics "$glb_cold_name" after

        # Warm: identical seed formula -> identical tile set, requested again
        # now that the cold pass above has populated the glb cache.
        i=1
        glb_warm_name="glb_warm_z$z"
        scrape_metrics "$glb_warm_name" before
        while [ "$i" -le "$TOTAL_REPS" ]; do
            rep_seed=$((SEED + z * 10000 + i))
            alt="$(build_alt "$z" "$rep_seed" "$WALK_COUNT" "$MAXSTEP")"
            url="$BASE_URL/$TENANT/3dtiles/catalogs/$CATALOG/collections/$COLLECTION/3dtiles/tiles/$z/($alt).glb"
            out="$(rep_file "$glb_warm_name" "$i")"
            oha --no-tui --rand-regex-url --output-format json ${AUTH_ARGS[@]+"${AUTH_ARGS[@]}"} -c "$CONCURRENCY" -z "$DURATION" -o "$out" "$url"
            i=$((i + 1))
        done
        scrape_metrics "$glb_warm_name" after

        z=$((z + 1))
    done

    echo "== styled PNG sweep (style=$STYLE_ID, same coords as the glb sweep) z$ZMIN_3D..$ZMAX_3D =="
    z=$ZMIN_3D
    while [ "$z" -le "$ZMAX_3D" ]; do
        i=1
        png_styled_name="png_styled_z$z"
        scrape_metrics "$png_styled_name" before
        while [ "$i" -le "$TOTAL_REPS" ]; do
            rep_seed=$((SEED + z * 10000 + i))
            alt="$(build_alt "$z" "$rep_seed" "$WALK_COUNT" "$MAXSTEP")"
            url="$BASE_URL/$TENANT/tiles/catalogs/$CATALOG/collections/$COLLECTION/styles/$STYLE_ID/map/tiles/WebMercatorQuad/$z/($alt).png"
            out="$(rep_file "$png_styled_name" "$i")"
            oha --no-tui --rand-regex-url --output-format json ${AUTH_ARGS[@]+"${AUTH_ARGS[@]}"} -c "$CONCURRENCY" -z "$DURATION" -o "$out" "$url"
            i=$((i + 1))
        done
        scrape_metrics "$png_styled_name" after
        z=$((z + 1))
    done
else
    echo "skipping glb/styled-PNG sweeps (BENCH_3D=0); set BENCH_3D=1 to include them" >&2
fi

echo "== items pages =="
run_plain "items_limit100" "$ITEMS_URL?limit=100" "$CONCURRENCY"
run_plain "items_limit1000" "$ITEMS_URL?limit=1000" "$CONCURRENCY"

if [ "$HAVE_ITEM_ID" -eq 1 ]; then
    echo "== single item by id =="
    run_plain "item_by_id" "$ITEMS_URL/$ITEM_ID" "$CONCURRENCY"
fi

echo "== mixed 70/20/10 (tiles/items/item-by-id, concurrency-weighted, run concurrently) =="
# oha's --rand-regex-url picks uniformly among distinct alternation branches
# (regex-syntax collapses repeated identical branches before rand_regex ever
# sees them, so duplicating a branch N times does not weight it N times more
# likely -- verified empirically against oha 1.15.0). A single weighted
# regex therefore cannot express 70/20/10. Instead this scenario runs three
# oha processes concurrently for the same DURATION, one per request shape,
# with concurrency split 70/20/10 across CONCURRENCY. That approximates but
# does not guarantee an exact request-count ratio -- realized throughput per
# shape depends on each shape's own latency. See docs/benchmarking.md.
if [ "$HAVE_ITEM_ID" -eq 1 ]; then
    tile_conc=$(( CONCURRENCY * 70 / 100 )); [ "$tile_conc" -ge 1 ] || tile_conc=1
    items_conc=$(( CONCURRENCY * 20 / 100 )); [ "$items_conc" -ge 1 ] || items_conc=1
    item_conc=$(( CONCURRENCY - tile_conc - items_conc )); [ "$item_conc" -ge 1 ] || item_conc=1

    scrape_metrics "mixed_70_20_10" before
    i=1
    while [ "$i" -le "$TOTAL_REPS" ]; do
        rep_seed=$((SEED + 900000 + i))
        alt="$(build_alt "$MIXED_ZOOM" "$rep_seed" "$WALK_COUNT" "$MAXSTEP")"
        tile_url="$(subst_tile_url "$MVT_TILE_TMPL" "$MIXED_ZOOM" "$alt")"
        items_url="$ITEMS_URL?limit=100"
        item_url="$ITEMS_URL/$ITEM_ID"

        tile_out="$(rep_file "mixed_70_20_10.tiles" "$i")"
        items_out="$(rep_file "mixed_70_20_10.items" "$i")"
        item_out="$(rep_file "mixed_70_20_10.item" "$i")"

        oha --no-tui --rand-regex-url --output-format json ${AUTH_ARGS[@]+"${AUTH_ARGS[@]}"} -c "$tile_conc" -z "$DURATION" -o "$tile_out" "$tile_url" &
        oha --no-tui --output-format json ${AUTH_ARGS[@]+"${AUTH_ARGS[@]}"} -c "$items_conc" -z "$DURATION" -o "$items_out" "$items_url" &
        oha --no-tui --output-format json ${AUTH_ARGS[@]+"${AUTH_ARGS[@]}"} -c "$item_conc" -z "$DURATION" -o "$item_out" "$item_url" &
        wait
        i=$((i + 1))
    done
    scrape_metrics "mixed_70_20_10" after
else
    echo "skipping mixed_70_20_10 (no item id available)" >&2
fi

echo "done: $OUT_DIR"
