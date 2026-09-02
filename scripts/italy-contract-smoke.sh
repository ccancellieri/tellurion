#!/bin/sh
# The decisive gate: boot the server on the *real production configuration of
# the live Render demo* and assert that its contract is unchanged.
#
# The YAML embedded below is a verbatim copy of
# `ccancellieri/tellurion-italy-demo`'s `deploy/render/vector.yaml` — the file
# the demo's Dockerfile copies to `/app/config.yaml` and the deployed service
# actually reads. The GeoPackage is provisioned with the exact
# `tellurion-ingest geopackage create-tables` invocation that Dockerfile runs
# (same table, geometry column, SRID, geometry type and column list), so the
# physical shape the server introspects at boot is the deployed one. Only the
# row *content* differs: the real demo loads ~5,600 OSM road features from a
# release artifact, which this script neither downloads nor needs — nothing
# asserted here is a function of how many rows are in the table.
#
# A synthetic smoke can only prove that a change works in a world the test
# built. This one proves it does not break the world somebody is already
# running. In particular, for `#192` and `#182` it proves the negative: the
# demo declares no `kind:`, no `protocols:` block and no `server.processes`
# block at all, so after these changes every one of its surfaces — landing
# pages, link sets, conformance lists, collection listings, tiles — must be
# exactly what it was, and the records and processes lanes must both be
# invisible.
#
# Exit status is the gate: 0 and a final `PASS` line, or a `FAIL` line naming
# what disagreed.
#
# `#260`: the config lives in a directory of its own, holding nothing else,
# and this script's own preconditions are checked and named before any
# assertion runs. See the preflight block below for what each buys.

set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
WORK=$(mktemp -d)
SERVER_PID=""
PORT=18292

TELLURION="$ROOT/target/debug/tellurion"
INGEST="$ROOT/target/debug/tellurion-ingest"

cleanup() {
  if [ -n "$SERVER_PID" ]; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT INT TERM

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  if [ -f "$WORK/server.log" ]; then
    printf -- '--- server log (tail) ---\n' >&2
    tail -n 40 "$WORK/server.log" >&2
  fi
  exit 1
}

CHECKS=0
ok() {
  CHECKS=$((CHECKS + 1))
  printf '  ok %s\n' "$1"
}

# --- preconditions of the harness itself, named up front (`#260`) -----------
#
# A gate that fails for a reason unrelated to the change under test teaches
# people to re-run instead of to read, which is exactly how a real regression
# gets waved through. This campaign has already been misled three times by a
# harness fault presenting as a code defect — a sibling worktree's server
# answering on a shared port, a config reload invalidating readiness mid-run
# (`#260`, fixed by the config directory below), and a PostgreSQL cluster that
# was simply down — and each first appeared as a confident, wrong `FAIL` about
# a contract this script exists to protect. So each is refused BY NAME here,
# rather than left to surface as an assertion failure hundreds of lines later.
#
# The same three checks, with the same wording, guard `demo-smoke.sh`; they
# are duplicated rather than sourced because each script is a single
# self-contained file an operator can copy to a host and run.

# Whether anything is accepting TCP connections on port `$1`. `curl` is
# already this script's only network dependency, so the check reuses it rather
# than adding a hard one on `ss`/`lsof`/`nc`: exit code 7 is curl's "could not
# connect", i.e. nothing is there. Any other outcome — an HTTP answer, a TLS
# error, a timeout against something that accepted and went quiet — means the
# port is taken.
port_is_free() {
  code=0
  curl -s -o /dev/null --max-time 5 "http://127.0.0.1:$1/" >/dev/null 2>&1 || code=$?
  [ "$code" = "7" ]
}

# Best-effort "and here is who has it", appended to the refusal so an operator
# can act on it without a second investigation.
port_occupant() {
  if command -v ss >/dev/null 2>&1; then
    ss -ltnp 2>/dev/null | grep -F ":$1 " || true
  elif command -v lsof >/dev/null 2>&1; then
    lsof -nP -iTCP:"$1" -sTCP:LISTEN 2>/dev/null || true
  fi
}

require_free_port() {
  # An explicit `if` rather than `port_is_free "$1" && return 0`: this branch
  # only ever runs when something is already wrong, and it must not depend on
  # how a given `sh` applies `set -e` to an AND-list.
  if port_is_free "$1"; then
    return 0
  fi
  fail "port $1 is already in use, so this script's own server would not be the
  one answering it. Either a server from a sibling worktree or one that
  outlived its cleanup is listening there. Stop THAT process by pid — a broad
  pkill would take out concurrent worktrees' servers too. $(port_occupant "$1")"
}

# Every `driver: <name>` / `url_env: <VAR>` pair in the config about to be
# booted, read out of the file the server itself is about to read so it cannot
# drift from what this run genuinely depends on. The deployed config declares
# exactly one storage and it is a `geopackage`, so this run needs no database
# service — asserted rather than assumed. The day the demo's own
# `deploy/render/vector.yaml` grows a database-backed storage, a cluster that
# is down says so here by name instead of arriving as a storage-dependency
# failure indistinguishable from a regression.
require_bootable_storages() {
  # Scoped to the top-level `storages:` block: a `cache.l2` tier also carries
  # a `url_env`, and reading one as a storage DSN would turn this check into
  # exactly the kind of confident, wrong failure it exists to prevent.
  awk '/^storages:/ { in_storages = 1; next }
       /^[^[:space:]#]/ { in_storages = 0 }
       in_storages && /^[[:space:]]*driver:[[:space:]]*/ { driver = $2 }
       in_storages && /^[[:space:]]*url_env:[[:space:]]*/ { print driver, $2 }' "$1" \
    >"$WORK/storages.txt"
  while read -r driver url_env; do
    [ "$driver" = geopackage ] && continue
    eval "dsn=\${$url_env:-}"
    [ -n "$dsn" ] ||
      fail "$1 routes a '$driver' storage through \$$url_env, which is not set;
  this run boots a database-backed storage and has no DSN for it"
    if ! command -v pg_isready >/dev/null 2>&1; then
      printf 'note: pg_isready is not installed, so $%s could not be checked here;
  a database that is down will surface as a storage-dependency failure instead\n' \
        "$url_env" >&2
      continue
    fi
    pg_isready -d "$dsn" >/dev/null 2>&1 ||
      fail "the database \$$url_env names is not accepting connections. Start it
  (pg_ctlcluster 16 main start) rather than reading the assertions below as a
  regression in the change under test"
  done <"$WORK/storages.txt"
}

command -v curl >/dev/null 2>&1 ||
  fail 'curl is not on PATH, and every assertion in this script is an HTTP request'
ok 'curl is on PATH'

# Any process running THIS worktree's `tellurion` binary, one line each.
#
# Resolved through `/proc/<pid>/exe` where the kernel exposes it, rather than
# by matching a command line: a stray started as `./target/debug/tellurion`
# and one started by absolute path are the same process to this script, and
# only the resolved executable says so. A binary rebuilt since the process
# started reads back as `... (deleted)`, which still identifies it. Where
# there is no `/proc`, it compares the executable reported by `ps` exactly.
# Looking only at that executable field matters on macOS: a command-line grep
# for `$TELLURION` reports the grep process itself because its argument contains
# that path.
#
# A SIBLING worktree's server is deliberately not matched: it runs a
# different binary, it is somebody else's work in progress, and it is not
# this script's business unless it holds a port this run wants, which is
# what `require_free_port` catches.
stray_tellurion() {
  if [ -r /proc/self/exe ]; then
    for proc_dir in /proc/[0-9]*; do
      exe=$(readlink "$proc_dir/exe" 2>/dev/null || true)
      case "${exe% (deleted)}" in
      "$TELLURION") printf '  pid %s -> %s\n' "${proc_dir#/proc/}" "$exe" ;;
      esac
    done
  else
    ps -eo pid=,comm= 2>/dev/null |
      awk -v tellurion="$TELLURION" '
        {
          pid = $1
          command = $0
          sub(/^[[:space:]]*[0-9]+[[:space:]]+/, "", command)
          if (command == tellurion)
            printf "  pid %s -> %s\n", pid, command
        }'
  fi
}

STRAY=$(stray_tellurion)
[ -z "$STRAY" ] ||
  fail "a tellurion process from this worktree is already running and will take or
  answer this run's port. Stop that pid specifically — never a broad pkill,
  sibling worktrees are live:
$STRAY"
ok 'no tellurion process from this worktree is already running'

require_free_port "$PORT"
ok "port $PORT is free"

# --- build ------------------------------------------------------------------

CARGO_PROFILE_DEV_DEBUG=0 cargo build --quiet \
  -p tellurion -p tellurion-ingest >"$WORK/build.log" 2>&1 ||
  { cat "$WORK/build.log" >&2; fail 'cargo build'; }

test -x "$TELLURION" || fail "missing binary $TELLURION"
test -x "$INGEST" || fail "missing binary $INGEST"

# --- the deployed config, verbatim ------------------------------------------

# `#260`: the config gets a directory of its own, holding nothing else. The
# config-reload file watch is on the config file's PARENT DIRECTORY, not the
# file — a mounted ConfigMap swaps a symlink rather than rewriting the file,
# so watching the directory is right for the deployment shape it was designed
# for. The cost is that any file written beside the config looks like a config
# change: `server.log`, which the server appends to for the whole run, plus
# `rome-osm.gpkg`, `provision.sql` and the two `.mvt` bodies this script
# writes into `$WORK` — one of them (`tile-a.mvt`) while the server is up.
# Each such write triggered a reload, and every reload invalidates readiness
# for a short window, which is how `#227`'s verification collected a `FAIL` on
# a `/readyz` assertion belonging to neither the phase nor the branch under
# test. Isolating the config removes the feedback loop rather than shrinking
# the window; nothing else about this run moves.
mkdir -p "$WORK/cfg" || fail 'could not create the config directory'
CONFIG="$WORK/cfg/config.yaml"

cat >"$CONFIG" <<'YAML'
server:
  port: 10000
  request_timeout_s: 30
  log_json: true

cache:
  memory_percent: 10.0

storages:
  - id: rome
    driver: geopackage
    url_env: TELLURION_GEOPACKAGE_PATH

tenants:
  - id: public

catalogs:
  - id: default
    tenant: public

collections:
  - id: rome_roads
    catalog: default
    storage: rome
    tiles:
      minzoom: 0
      maxzoom: 14
      caps: {}
    schema:
      properties:
        - { name: highway, type: string }
        - { name: name, type: string }
        - { name: railway, type: string }
    settings:
      tile_properties: [highway, name, railway]
YAML

# The deployed config predates `control_store:` entirely and has no such
# block. Booting it as-is is itself part of the contract (see
# `bootstrap.rs`'s `ControlStoreLocator` doc and the regression it records):
# a config written before a field existed must still boot. Nothing is added
# to it here.
grep -q 'control_store' "$CONFIG" && fail 'the deployed config must stay free of a control_store block'
ok 'the deployed config declares no control_store block and is booted as-is'

# --- the deployed provisioning, verbatim ------------------------------------

GPKG="$WORK/rome-osm.gpkg"
"$INGEST" geopackage create-tables \
  --path "$GPKG" \
  --table rome_roads \
  --geometry geom \
  --srid 4326 \
  --geometry-type LINESTRING \
  --columns highway:TEXT,name:TEXT,railway:TEXT >"$WORK/provision.sql" 2>&1 ||
  fail 'ingest geopackage create-tables (rome_roads)'

cat >"$WORK/roads.geojson" <<'JSON'
{"type":"FeatureCollection","features":[
{"type":"Feature","id":"1","geometry":{"type":"LineString","coordinates":[[12.4900,41.9000],[12.4950,41.9040]]},"properties":{"highway":"residential","name":"Via Roma","railway":null}},
{"type":"Feature","id":"2","geometry":{"type":"LineString","coordinates":[[12.4800,41.8900],[12.4860,41.8950]]},"properties":{"highway":"primary","name":"Via Appia","railway":null}},
{"type":"Feature","id":"3","geometry":{"type":"LineString","coordinates":[[12.5000,41.9100],[12.5060,41.9160]]},"properties":{"highway":null,"name":"Linea A","railway":"subway"}}
]}
JSON

"$INGEST" geopackage load --path "$GPKG" --table rome_roads \
  "$WORK/roads.geojson" >/dev/null 2>&1 || fail 'ingest geopackage load (rome_roads)'

# --- boot -------------------------------------------------------------------

# Checked against the config that is about to be booted, not against a claim
# about it (`#260`).
require_bootable_storages "$CONFIG"
ok 'every storage the deployed config routes is a geopackage; this run needs no database'

TELLURION_CONFIG="$CONFIG" \
  TELLURION_GEOPACKAGE_PATH="$GPKG" \
  PORT="$PORT" \
  "$TELLURION" >"$WORK/server.log" 2>&1 &
SERVER_PID=$!

i=0
while [ "$i" -lt 150 ]; do
  if curl -fsS -o /dev/null "http://127.0.0.1:$PORT/healthz" 2>/dev/null; then
    break
  fi
  kill -0 "$SERVER_PID" 2>/dev/null || fail 'server exited during startup'
  i=$((i + 1))
  sleep 0.2
done
[ "$i" -lt 150 ] || fail 'server never became healthy'
ok 'the deployed config boots'

status_of() {
  curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$PORT$1"
}

body_of() {
  out=$(curl -s -w '\n%{http_code}' "http://127.0.0.1:$PORT$1")
  code=$(printf '%s' "$out" | tail -n 1)
  [ "$code" = "200" ] || fail "GET $1 returned $code, expected 200"
  printf '%s' "$out" | sed '$d'
}

expect_status() {
  actual=$(status_of "$1")
  [ "$actual" = "$2" ] || fail "GET $1 returned $actual, expected $2"
  ok "GET $1 -> $2"
}

expect_body_contains() {
  printf '%s' "$2" | grep -Fq -- "$3" || fail "$1: expected to find '$3'"
  ok "$1 contains '$3'"
}

expect_body_lacks() {
  printf '%s' "$2" | grep -Fq -- "$3" && fail "$1: must NOT contain '$3'"
  ok "$1 does not contain '$3'"
}

# `/healthz` (waited on above) is dependency-free and turns 200 before the
# first dependency probe has run, so `/readyz` needs its own wait.
#
# STABLY ready, not momentarily ready (`#260`). The caller's next act is three
# more requests against `/readyz` — status, then body, then `Content-Type` —
# and a single 200 says only that readiness was 200 at one instant. `#227`'s
# false failure was exactly that: the first of two back-to-back calls passed, a
# readiness invalidation landed 1.7ms later, and the second returned the
# `503`'s `application/problem+json`. Requiring consecutive 200s means what the
# caller then asserts on is a state the process is actually holding. Belt to
# the config directory's braces, not a substitute for it: with the config
# isolated there is nothing left to invalidate readiness mid-run, and a wait
# that had to paper over a real flap would be hiding a defect rather than a
# harness fault.
READY_STABLE_POLLS=3

wait_ready() {
  i=0
  stable=0
  while [ "$i" -lt 150 ]; do
    if [ "$(status_of /readyz)" = "200" ]; then
      stable=$((stable + 1))
      if [ "$stable" -ge "$READY_STABLE_POLLS" ]; then
        return 0
      fi
    else
      stable=0
    fi
    i=$((i + 1))
    sleep 0.2
  done
  fail "readiness never held 200 for $READY_STABLE_POLLS consecutive polls"
}

# --- the contract this deployment already had -------------------------------

expect_status '/' 200

DIR=$(body_of '/public')
expect_body_contains 'tenant directory' "$DIR" '/public/features/catalogs/default'
expect_body_contains 'tenant directory' "$DIR" '/public/tiles/catalogs/default'
expect_body_contains 'tenant directory' "$DIR" '/public/styles/catalogs/default'
expect_body_contains 'tenant directory' "$DIR" '/public/3dtiles/catalogs/default'
expect_body_contains 'tenant directory' "$DIR" '/public/stac/catalogs/default'

COLLECTIONS=$(body_of '/public/features/catalogs/default/collections')
expect_body_contains 'features /collections' "$COLLECTIONS" '"rome_roads"'

ITEMS=$(body_of '/public/features/catalogs/default/collections/rome_roads/items')
expect_body_contains 'features /items' "$ITEMS" '"FeatureCollection"'
expect_body_contains 'features /items' "$ITEMS" 'Via Appia'

expect_status '/public/features/catalogs/default/collections/rome_roads/items/1' 200
expect_status '/public/features/catalogs/default/collections/rome_roads/queryables' 200
expect_status '/public/features/catalogs/default/conformance' 200
expect_status '/public/features/catalogs/default/api' 200

# The tiles lane, which is what this demo exists to show.
expect_status '/public/tiles/catalogs/default/collections/rome_roads/tiles' 200
expect_status '/public/tiles/catalogs/default/collections/rome_roads/tiles/WebMercatorQuad/0/0/0' 200

STAC=$(body_of '/public/stac/catalogs/default/collections')
expect_body_contains 'stac /collections' "$STAC" '"rome_roads"'

# --- the STAC Collection items link (`#245`) --------------------------------
#
# The other deliberate contract CHANGE this file records, and the one that
# moves bytes on this deployment: every STAC Collection document now carries a
# `rel="items"` link. Before this slice they carried only `root`, `self`,
# `parent` and `alternate`, while this root's `/conformance` declared BOTH
# `https://api.stacspec.org/v1.0.0/ogcapi-features` and
# `http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/core` — each of
# which requires the link by name:
#
#   OGC API - Features - Part 1: Core (OGC 17-069r4, version 1.0.1),
#   Requirement 15 `/req/core/fc-md-items-links`: "For each feature collection
#   included in the response, the links property of the collection SHALL
#   include an item for each supported encoding with a link to the features
#   resource (relation: items)... All links SHALL include the rel and type
#   properties." Requirement 19 `/req/core/sfc-md-success` carries it onto
#   `/collections/{collectionId}`: that response's "links SHALL include all
#   links included for this feature collection in the /collections response".
#
#   STAC API - Features (stac-api-spec, v1.0.0): "This endpoint must be
#   exposed via a link in the individual collection's endpoint with
#   `rel=items`."
#
# So the direction of the change is towards honouring what was already
# declared, and the resource being linked has been reachable at this
# deployment all along — nothing new is served, a client is simply told how to
# find it. The assertions FOLLOW the link rather than merely matching a `rel`:
# a dangling `rel="items"` would be the same defect one level down.

ITEMS_HREF='/public/stac/catalogs/default/collections/rome_roads/items'
expect_body_contains 'stac /collections' "$STAC" '"rel":"items"'
expect_body_contains 'stac /collections' "$STAC" "\"href\":\"$ITEMS_HREF\""

STAC_COLLECTION=$(body_of '/public/stac/catalogs/default/collections/rome_roads')
expect_body_contains 'stac /collections/rome_roads' "$STAC_COLLECTION" '"rel":"items"'
expect_body_contains 'stac /collections/rome_roads' "$STAC_COLLECTION" "\"href\":\"$ITEMS_HREF\""
# Requirement 15.B / the STAC example Collection: `type` is the one encoding
# `/items` actually serves.
expect_body_contains 'stac /collections/rome_roads' "$STAC_COLLECTION" '"application/geo+json"'

# Followed, not just matched: the advertised href really serves this
# collection's items, in the media type the link declared.
STAC_ITEMS=$(body_of "$ITEMS_HREF")
expect_body_contains 'stac items (followed from the link)' "$STAC_ITEMS" '"FeatureCollection"'
expect_body_contains 'stac items (followed from the link)' "$STAC_ITEMS" 'Via Appia'
ITEMS_CT=$(curl -s -o /dev/null -w '%{content_type}' "http://127.0.0.1:$PORT$ITEMS_HREF")
[ "$ITEMS_CT" = "application/geo+json" ] ||
  fail "the items link declared application/geo+json but the resource served: $ITEMS_CT"
ok 'the items link resolves with the media type it declared'

# The rest of the Collection document is untouched: the pre-`#245` links are
# all still there, so this is an addition, not a rewrite.
for rel in root self parent alternate; do
  expect_body_contains 'stac /collections/rome_roads' "$STAC_COLLECTION" "\"rel\":\"$rel\""
done

# `#245` also narrowed the TileSet resource's styled-map links to the styles
# that actually paint this collection's layers. This deployment registers NO
# styles at all (the config above has no `styles:` block), so it had no `map`
# links before and has none now — the narrowing costs it exactly zero bytes,
# which is the whole reason it can ride along in this slice.
TILESET=$(body_of '/public/tiles/catalogs/default/collections/rome_roads/tiles/WebMercatorQuad')
expect_body_lacks 'tiles /tiles/WebMercatorQuad' "$TILESET" '/def/rel/ogc/1.0/map'
# ...and the layer name it advertises is unchanged: this driver reports no
# `vector_layers` metadata, so the resource falls back to the collection's
# external id, exactly as before.
expect_body_contains 'tiles /tiles/WebMercatorQuad' "$TILESET" '"id":"rome_roads"'

# --- STAC /search filter-crs (`#248`) ---------------------------------------
#
# The STAC API Filter Extension pins this parameter to CRS84: "server must
# only accept `http://www.opengis.net/def/crs/OGC/1.3/CRS84` as a valid value,
# may reject any others". Before `#248` this deployment accepted any
# `filter-crs`, dropped it, and answered 200 with the filter's geometries read
# in CRS84 regardless — rows selected in a CRS the client never named.
#
# This driver is GeoPackage: it filters, never reprojects, and stores at SRID
# 4326. So CRS84 costs it nothing (the common case, and the reason this demo's
# behaviour is unchanged for every request that does not name another CRS),
# while EPSG:4326-referenced-by-authority — datum-identical, opposite axis
# order — is refused by name instead of silently mis-read.

CRS84_ENC='http%3A%2F%2Fwww.opengis.net%2Fdef%2Fcrs%2FOGC%2F1.3%2FCRS84'
EPSG4326_ENC='http%3A%2F%2Fwww.opengis.net%2Fdef%2Fcrs%2FEPSG%2F0%2F4326'
SEARCH='/public/stac/catalogs/default/search?collections=rome_roads&filter=highway%3D%27primary%27'

SEARCH_BODY=$(body_of "$SEARCH")
expect_body_contains 'stac /search (no filter-crs)' "$SEARCH_BODY" 'Via Appia'

SEARCH_CRS84=$(body_of "$SEARCH&filter-crs=$CRS84_ENC")
expect_body_contains 'stac /search (filter-crs=CRS84)' "$SEARCH_CRS84" 'Via Appia'
expect_body_contains 'stac /search (filter-crs=CRS84)' "$SEARCH_CRS84" 'filter-crs='

expect_status "$SEARCH&filter-crs=$EPSG4326_ENC" 400

post_status_of() {
  curl -s -o /dev/null -w '%{http_code}' -X POST \
    -H 'Content-Type: application/json' -d "$2" \
    "http://127.0.0.1:$PORT$1"
}

expect_post_status() {
  actual=$(post_status_of "$1" "$2")
  [ "$actual" = "$3" ] || fail "POST $1 returned $actual, expected $3"
  ok "POST $1 ($4) -> $3"
}

# The POST body field carries the same name as the query parameter, and the
# same value space — a body-only client must not be able to sneak past the
# refusal the query string gets.
POST_PATH='/public/stac/catalogs/default/search'
expect_post_status "$POST_PATH" \
  '{"collections":["rome_roads"],"filter":"highway='"'"'primary'"'"'","filter-lang":"cql2-text","filter-crs":"http://www.opengis.net/def/crs/OGC/1.3/CRS84"}' \
  200 'filter-crs=CRS84'
expect_post_status "$POST_PATH" \
  '{"collections":["rome_roads"],"filter":"highway='"'"'primary'"'"'","filter-lang":"cql2-text","filter-crs":"http://www.opengis.net/def/crs/EPSG/0/4326"}' \
  400 'filter-crs=EPSG:0:4326'

# --- a default spatial filter, unmoved (`#247`) ------------------------------
#
# `#247` made an omitted `filter-crs` mean real work: OGC API - Features -
# Part 3 Requirement 7 (`/req/filter/filter-crs-wgs84`) says such a filter's
# geometries are processed in CRS84, which against a projected storage is a
# coordinate transform, and a driver that cannot perform one now refuses by
# name instead of answering in the wrong CRS.
#
# This deployment is on the other side of that branch, and this is where that
# is proved: `rome_roads` is stored at SRID 4326, so reading its filter
# literals as CRS84 asks the driver for nothing at all, and every request
# below must be exactly what it was before the slice. The whole authorised
# exception is conditional on the storage SRID, and this file is the gate that
# holds it to that — a change that made the transform (or the refusal)
# unconditional would fail right here.
#
# `S_INTERSECTS(geom, BBOX(12.475,41.885,12.487,41.896))` is degrees, and it
# encloses `Via Appia` alone — `Via Roma` starts at longitude 12.490 and
# `Linea A` at 12.500, both east of the box. So the assertion is not merely
# "a 200 came back": a filter that had quietly stopped selecting, or started
# reading the same four numbers as something other than degrees, changes which
# of the three roads appear. No `filter-crs` of any kind is sent — this is the
# plainest conformant Part 3 request there is, and the exact shape that could
# not be served at all against a projected collection until `#247`.

SPATIAL='filter=S_INTERSECTS%28geom%2CBBOX%2812.475%2C41.885%2C12.487%2C41.896%29%29'
ITEMS_SPATIAL=$(body_of "/public/features/catalogs/default/collections/rome_roads/items?$SPATIAL")
expect_body_contains 'features /items (no filter-crs)' "$ITEMS_SPATIAL" 'Via Appia'
expect_body_lacks 'features /items (no filter-crs)' "$ITEMS_SPATIAL" 'Via Roma'
expect_body_lacks 'features /items (no filter-crs)' "$ITEMS_SPATIAL" 'Linea A'

# ...and the same on the STAC lane, whose `filter-crs` defaults to CRS84 by the
# Filter Extension's own words rather than by Requirement 7 — same collection,
# same conclusion, still a 200.
SEARCH_SPATIAL=$(body_of "/public/stac/catalogs/default/search?collections=rome_roads&$SPATIAL")
expect_body_contains 'stac /search (no filter-crs)' "$SEARCH_SPATIAL" 'Via Appia'
expect_body_lacks 'stac /search (no filter-crs)' "$SEARCH_SPATIAL" 'Linea A'

# --- a bbox with no bbox-crs, unmoved (`#255`) -------------------------------
#
# The same argument one parameter over, and the same side of the same branch.
# `#255` made an omitted `bbox-crs` mean real work: Part 1 Requirement 23
# (`/req/core/fc-bbox-definition`) clause C fixes a four-number `bbox` as
# CRS84, which against a projected storage is a coordinate transform, and a
# driver that cannot perform one now refuses by name instead of comparing
# degrees against metres under a 200.
#
# `rome_roads` is stored at SRID 4326, so reading its bbox as CRS84 asks the
# driver for nothing at all, and every request below must be exactly what it
# was before the slice — no transform in the SQL, no refusal, the same rows.
# The whole authorised exception is conditional on the storage SRID, and this
# is the gate that holds it to that: a change that made either the transform or
# the refusal unconditional would fail right here, on the deployment somebody
# is actually running.
#
# `bbox=12.475,41.885,12.487,41.896` is the same window the spatial filter
# above uses, so it encloses `Via Appia` alone — the assertion is which of the
# three roads come back, never merely that a 200 did.
BBOX='bbox=12.475,41.885,12.487,41.896'
ROADS_ITEMS='/public/features/catalogs/default/collections/rome_roads/items'
ITEMS_BBOX=$(body_of "$ROADS_ITEMS?$BBOX")
expect_body_contains 'features /items (no bbox-crs)' "$ITEMS_BBOX" 'Via Appia'
expect_body_lacks 'features /items (no bbox-crs)' "$ITEMS_BBOX" 'Via Roma'
expect_body_lacks 'features /items (no bbox-crs)' "$ITEMS_BBOX" 'Linea A'

# Part 2 Abstract Test 10 (`/conf/crs/bbox-crs-parameter-default`) verbatim:
# "send the same request, but with no `bbox-crs` parameter ... verify that the
# responses include the same features." Byte equality over the features is
# stronger than the test asks for, and is what this deployment can honestly
# claim: on a 4326 storage the two readings compile to the identical statement.
#
# The comparison stops at `links`, the last member of this response, because
# its `self` href faithfully echoes whatever parameters the request carried
# (`params::items_href`) — the two bodies differ there BY DESIGN and must.
# Everything before it (`type`, `features`, `numberMatched`, `numberReturned`)
# is compared byte for byte.
features_of() {
  printf '%s' "$1" | sed 's/"links":.*//'
}
ITEMS_BBOX_CRS84=$(body_of "$ROADS_ITEMS?$BBOX&bbox-crs=$CRS84_ENC")
[ "$(features_of "$ITEMS_BBOX_CRS84")" = "$(features_of "$ITEMS_BBOX")" ] ||
  fail 'an omitted bbox-crs and an explicit CRS84 one must return the same features'
ok 'features /items: omitted bbox-crs == explicit CRS84'

# ...and the same on both STAC lanes, neither of which has a `bbox-crs`
# parameter to name at all — same collection, same conclusion, still a 200 with
# the same one road.
STAC_ROOT='/public/stac/catalogs/default'
STAC_ITEMS_BBOX=$(body_of "$STAC_ROOT/collections/rome_roads/items?$BBOX")
expect_body_contains 'stac /items (bbox)' "$STAC_ITEMS_BBOX" 'Via Appia'
expect_body_lacks 'stac /items (bbox)' "$STAC_ITEMS_BBOX" 'Linea A'

SEARCH_BBOX=$(body_of "$STAC_ROOT/search?collections=rome_roads&$BBOX")
expect_body_contains 'stac /search (bbox)' "$SEARCH_BBOX" 'Via Appia'
expect_body_lacks 'stac /search (bbox)' "$SEARCH_BBOX" 'Linea A'

# `#255` added a third "what this fan-out left out" list to the `/search`
# response. It is `skip_serializing_if = "Vec::is_empty"`, so for a deployment
# with nothing to report it must not appear at all — the absent-stays-absent
# rule, asserted rather than assumed, because a new member turning up on every
# response is exactly how an additive change leaks into a deployed contract.
expect_body_lacks 'stac /search (bbox)' "$SEARCH_BBOX" 'bboxIncapableCollections'

# --- basic-spatial-functions, withdrawn (`#134`) ----------------------------
#
# A further deliberate contract CHANGE this file records, and the one this
# GeoPackage-backed deployment is the decisive test of. The `S_INTERSECTS`
# just proved above is the only *positional* form this driver compiles: one
# predicate, in AND-position. Two of them, or one beneath `OR`/`NOT`, has
# always been refused by name (`S_INTERSECTS cannot be evaluated exactly:
# ...`, a 400) because the R*Tree bbox pre-filter it ANDs into the SQL is
# only sound while `AND` is the only thing narrowing the candidate set.
#
# CQL2 (OGC 21-065r2) defines `basic-spatial-functions` in terms of the
# general form. The class names Basic CQL2 as its Dependency, and Basic
# CQL2's Requirement 1 (`/req/basic-cql2/cql2-filter`) requires "a CQL2 filter
# expression composed of a logically connected series of one or more
# predicates as described by the BNF rule `booleanExpression` ... with the
# exception that the rules ... `spatialPredicate` ... do not have to be
# supported" — declaring this class is exactly what removes `spatialPredicate`
# from that exception list. Its only two permitted narrowings (Permission 6
# and Permission 7) are about which *operands* and which *literal types* a
# server must accept; neither says anything about where the predicate may
# sit. And its normative Abstract Test Suite settles it outright: Conformance
# Test 26 (`/conf/basic-spatial-functions/test-data`) asserts exact item
# counts for `S_INTERSECTS(...) and S_INTERSECTS(...)`, `S_INTERSECTS(...) and
# not S_INTERSECTS(...)` and `S_INTERSECTS(...) or S_INTERSECTS(...)`, and
# Conformance Test 27 (`/conf/basic-spatial-functions/logical`) composes the
# stored spatial predicates under `NOT`/`AND`/`OR` together.
#
# So the class is withdrawn, and this is where those bytes move: one URI
# leaves this deployment's `/conformance`. Same direction as the `#150` and
# `#208` withdrawals below — telling clients less than before, never more —
# and it costs this demo no capability at all: every spatial query it could
# serve a minute ago it still serves, proved immediately above.

# The general form, in the three shapes Conformance Test 26 lists. Each must
# be refused BY NAME. A 200 here would mean the driver had started answering
# a composition it cannot evaluate soundly — which is the failure this
# withdrawal exists to stop advertising.
BOX_A='BBOX%2812.475%2C41.885%2C12.487%2C41.896%29'
BOX_B='BBOX%2812.495%2C41.885%2C12.505%2C41.896%29'
for composition in \
  "S_INTERSECTS%28geom%2C$BOX_A%29%20AND%20S_INTERSECTS%28geom%2C$BOX_B%29" \
  "S_INTERSECTS%28geom%2C$BOX_A%29%20AND%20NOT%20S_INTERSECTS%28geom%2C$BOX_B%29" \
  "S_INTERSECTS%28geom%2C$BOX_A%29%20OR%20S_INTERSECTS%28geom%2C$BOX_B%29"; do
  GENERAL="/public/features/catalogs/default/collections/rome_roads/items?filter=$composition"
  expect_status "$GENERAL" 400
  REFUSAL=$(curl -s "http://127.0.0.1:$PORT$GENERAL")
  expect_body_contains 'general-form refusal' "$REFUSAL" 'S_INTERSECTS'
done

# ...and therefore the class is absent from both roots that fold the driver's
# declared CQL2 set in. This is the pairing that makes either half worth
# asserting: the advertisement and the behaviour have to say the same thing,
# and a `/conformance` list checked alone could be confidently wrong and
# still pass.
FEATURES_CONFORMANCE=$(body_of '/public/features/catalogs/default/conformance')
expect_body_lacks 'features /conformance' "$FEATURES_CONFORMANCE" \
  'cql2/1.0/conf/basic-spatial-functions'
STAC_SPATIAL_CONFORMANCE=$(body_of '/public/stac/catalogs/default/conformance')
expect_body_lacks 'stac /conformance' "$STAC_SPATIAL_CONFORMANCE" \
  'cql2/1.0/conf/basic-spatial-functions'

# The narrowing is exactly that narrow. Every other CQL2 class this driver
# earns is untouched — `basic-cql2` most pointedly, because its own
# Requirement 1 excepts `spatialPredicate` by name, so supporting only a
# restricted `S_INTERSECTS` is precisely what that exception permits.
for kept in \
  'cql2/1.0/conf/basic-cql2' \
  'cql2/1.0/conf/cql2-text' \
  'cql2/1.0/conf/cql2-json' \
  'cql2/1.0/conf/advanced-comparison-operators' \
  'cql2/1.0/conf/temporal-functions'; do
  expect_body_contains 'features /conformance' "$FEATURES_CONFORMANCE" "$kept"
done

# The per-collection surface says the same thing as the root — `#105` made
# these two independent code paths, and a narrowing that reached only one of
# them would leave a client reading the collection document a claim the root
# had already withdrawn.
ROADS_DOC=$(body_of '/public/features/catalogs/default/collections/rome_roads')
expect_body_contains 'collection cql2ConformanceClasses' "$ROADS_DOC" 'cql2ConformanceClasses'
expect_body_lacks 'collection cql2ConformanceClasses' "$ROADS_DOC" \
  'cql2/1.0/conf/basic-spatial-functions'
expect_body_contains 'collection cql2ConformanceClasses' "$ROADS_DOC" 'cql2/1.0/conf/basic-cql2'

# `#248` folded the Item Search Filter class out of `tellurion-stac`'s static
# list and behind `Router::item_search_filter_conformance_classes`. This
# deployment's driver filters, so the class must still be declared — the fold
# must cost an already-honest deployment nothing.
STAC_CONFORMANCE=$(body_of '/public/stac/catalogs/default/conformance')
expect_body_contains 'stac /conformance' "$STAC_CONFORMANCE" 'api.stacspec.org/v1.0.0/item-search#filter'
expect_body_contains 'stac /conformance' "$STAC_CONFORMANCE" 'cql2/1.0/conf/basic-cql2'

expect_status '/public/styles/catalogs/default/styles' 200

# --- and the records lane, invisible ----------------------------------------

# This deployment declares no `kind:` and no `protocols:` block. Everything
# below is `#192` costing it nothing.
expect_body_lacks 'tenant directory' "$DIR" '/public/records/catalogs/default'
expect_status '/public/records/catalogs/default' 404
expect_status '/public/records/catalogs/default/collections' 404
expect_status '/public/records/catalogs/default/collections/rome_roads/items' 404

# --- and the processes lane, invisible --------------------------------------

# `#182`. Two independent reasons this deployment must see nothing:
# `protocols.processes` defaults to `disabled`, AND the config declares no
# `server.processes` block, so there is no durable job ledger and therefore no
# root to serve even if the exposure key were turned on. Every path under the
# prefix answers the bare 404 an unmounted prefix answers — landing page,
# /conformance and /api included, which is what the availability gate being
# the OUTERMOST layer on that root buys.
expect_body_lacks 'tenant directory' "$DIR" '/public/processes/catalogs/default'
expect_status '/public/processes/catalogs/default' 404
expect_status '/public/processes/catalogs/default/conformance' 404
expect_status '/public/processes/catalogs/default/api' 404
expect_status '/public/processes/catalogs/default/processes' 404
expect_status '/public/processes/catalogs/default/jobs/anything' 404

for root in features tiles styles 3dtiles stac; do
  CONFORMANCE=$(body_of "/public/$root/catalogs/default/conformance")
  expect_body_lacks "$root /conformance" "$CONFORMANCE" 'ogcapi-records-1'
  expect_body_lacks "$root /conformance" "$CONFORMANCE" 'ogcapi-processes-1'
done

# `rome_roads` declares no `kind`, so it is `vector` — served by exactly the
# roots that served it before, with no `itemType: record` anywhere.
expect_body_lacks 'features /collections' "$COLLECTIONS" '"itemType":"record"'

# --- and the CRS this deployment is served in, unmoved (`#227`) --------------

# `#227` made `Content-Crs`, a collection's `crs` list and its `storageCrs`
# depend on the CRS a response is genuinely in rather than asserting CRS84
# unconditionally. That moves bytes for a collection stored in a projected
# CRS — and for nothing else.
#
# This deployment's `rome_roads` is EPSG:4326 on a GeoPackage, which is the
# case the whole distinction turns on being a no-op: CRS84 and a 4326 storage
# differ only in axis order, which lives in a different code path entirely. So
# each of these is a byte-for-byte assertion of "what it always was", stated
# explicitly here rather than left to be inferred from the 200s above.
#
# `storageCrs` stays absent for the reason `#217` gave and `#227` did not
# change: `.../EPSG/0/4326` is a different URI from CRS84, latitude-first
# where CRS84 is longitude-first, so a driver that cannot swap axes cannot
# offer it and must not name it.
CRS84_URI='http://www.opengis.net/def/crs/OGC/1.3/CRS84'
ROME='/public/features/catalogs/default/collections/rome_roads'

expect_header() {
  actual=$(curl -s -o /dev/null -D - "http://127.0.0.1:$PORT$2" |
    tr -d '\r' | grep -i "^$3:" | head -n 1 | cut -d' ' -f2-)
  [ "$actual" = "$4" ] || fail "$1: $3 was '$actual', expected '$4'"
  ok "$1: $3 is '$4'"
}

expect_header 'features /items' "$ROME/items" 'content-crs' "<$CRS84_URI>"
expect_header 'features /items/{fid}' "$ROME/items/1" 'content-crs' "<$CRS84_URI>"

# An explicit `crs=CRS84` is still served, and still under the same header —
# it asks this collection for exactly what it already produces.
CRS84_ENCODED='http%3A%2F%2Fwww.opengis.net%2Fdef%2Fcrs%2FOGC%2F1.3%2FCRS84'
expect_status "$ROME/items?crs=$CRS84_ENCODED" 200
expect_header 'features /items?crs=CRS84' "$ROME/items?crs=$CRS84_ENCODED" \
  'content-crs' "<$CRS84_URI>"

ROME_MD=$(body_of "$ROME")
expect_body_contains 'features /collections/rome_roads' "$ROME_MD" "\"crs\":[\"$CRS84_URI\"]"
# Present and explicitly `null`, which is this lane's "the fact is not
# available" shape (`extent` does the same) — not an omitted member.
expect_body_contains 'features /collections/rome_roads' "$ROME_MD" '"storageCrs":null'

# --- and the Optimistic Locking ETags class, no longer claimed ---------------

# A deliberate contract CHANGE this file records (`#150`; `#245`'s items link
# above is the other, in the opposite direction — this one withdraws a claim,
# that one honours one). This
# deployment's only storage is a GeoPackage, and until now it advertised
# `req/optimistic-locking-etags` on the strength of committing writes
# synchronously. That is not what the class promises: it exists to stop a lost
# update, and stopping one needs the `If-Match` precondition re-verified
# inside the write statement, which needs a per-row version SQLite does not
# have.
#
# So the claim was an overclaim, and it is withdrawn. Asserted here rather
# than left implicit, because the point of this file is that nothing about the
# deployed contract changes by accident — this one changed on purpose, and in
# the direction of telling clients less than before rather than more. The demo
# is read-only, so nothing an actual client of it does is affected either way;
# every read assertion above is what proves that.
CONFORMANCE=$(body_of '/public/features/catalogs/default/conformance')
expect_body_lacks 'features /conformance' "$CONFORMANCE" 'optimistic-locking-etags'

# --- the Allow header, now truthful (`#208`) --------------------------------

# The second deliberate contract CHANGE this file records, and the one this
# deployment is the decisive test of: `rome_roads` declares no
# `routing.write`, so this demo cannot write, has never been able to write,
# and already refuses every write it is sent. Until now its `OPTIONS`
# response said otherwise — `Allow: GET, PUT, PATCH, DELETE, OPTIONS`,
# derived from the shape of the URI rather than from anything this deployment
# can do.
#
# OGC API - Features - Part 4 (OGC 20-002r1) Requirement 16 clause C
# (`/req/create-replace-delete/options-response`): "The value of the `Allow`
# header SHALL be the list of methods that are allowed for the resource at
# the time and within the context of the request." Section 6.5.1 is explicit
# that this may vary per resource: "A server is not required to implement
# every method described in this Standard (i.e. POST, PUT, PATCH or DELETE)
# for every mutable resource that it offers."
#
# So these bytes move here, and only here — on a deployment whose write
# capability is narrower than its URI shape suggested. The direction is the
# same as the `#150` withdrawal above: telling clients less than before,
# never more. Every read assertion in this file is what proves nothing else
# moved with it.
allow_of() {
  curl -s -o /dev/null -D - -X OPTIONS "http://127.0.0.1:$PORT$1" |
    tr -d '\r' | sed -n 's/^[Aa]llow: //p'
}

expect_allow() {
  actual=$(allow_of "$1")
  [ "$actual" = "$2" ] || fail "OPTIONS $1 -> Allow: '$actual', expected '$2'"
  ok "OPTIONS $1 -> Allow: $2"
}

ROADS='/public/features/catalogs/default/collections/rome_roads'
expect_allow "$ROADS/items/1" 'GET, OPTIONS'
expect_allow "$ROADS/items" 'GET, OPTIONS'
# No read representation on the batch resource, so nothing at all remains but
# `OPTIONS` itself.
expect_allow "$ROADS/items/batch" 'OPTIONS'

# And the pairing that makes the header worth asserting: issue the method the
# header withheld, and confirm the two agree. A `204` here would mean this
# demo had silently become writable; anything else means the advertisement
# and the behaviour say the same thing.
PUT_STATUS=$(curl -s -o /dev/null -w '%{http_code}' -X PUT \
  -H 'Content-Type: application/geo+json' \
  --data '{"type":"Feature","geometry":null,"properties":{}}' \
  "http://127.0.0.1:$PORT$ROADS/items/1")
[ "$PUT_STATUS" != "204" ] ||
  fail "Allow withheld PUT on $ROADS/items/1, but a PUT succeeded"
ok 'the PUT this Allow withholds is still refused'

# Refused by name, naming the collection and the capability — the refusal is
# unchanged by this slice, only the advertisement that preceded it is.
PUT_BODY=$(curl -s -X PUT -H 'Content-Type: application/geo+json' \
  --data '{"type":"Feature","geometry":null,"properties":{}}' \
  "http://127.0.0.1:$PORT$ROADS/items/1")
printf '%s' "$PUT_BODY" | grep -Fq "does not support 'write'" ||
  fail "the refusal must name the missing capability; got: $PUT_BODY"
ok 'the withheld PUT is refused by name'

# The reads on those exact URIs are untouched — the reason `Allow` still
# names `GET` above, and the reason this is a narrowing of methods rather
# than a resource that stopped existing.
expect_status "$ROADS/items/1" 200
expect_status "$ROADS/items" 200

# --- and Part 4 Create/Replace/Delete, no longer claimed (`#263`) ------------

# The third deliberate contract CHANGE this file records, and the one the two
# above make unavoidable. `rome_roads` declares no `routing.write`, so this
# deployment offers no mutable resource at all: `#208` narrowed its `Allow` to
# `GET, OPTIONS` on every write-resource shape (asserted immediately above),
# and every write it is sent is refused by name (asserted immediately above).
# Until now its `/conformance` still declared
# `http://www.opengis.net/spec/ogcapi-features-4/1.0/conf/create-replace-delete`
# — a requirements class whose every method this same server declines on these
# same URIs, which is the two halves of one contract contradicting each other
# in the most visible way an API has.
#
# OGC API - Features - Part 4 (OGC 20-002r1) Requirement 1 clause A: "A server
# SHALL implement one or more of the methods HTTP POST, PUT and/or DELETE for
# each mutable resource." (The published text identifies that requirement
# `/req/core/methods`, inside a clause whose every other requirement is
# `/req/create-replace-delete/...` — that document's own inconsistency, so the
# prose is what is cited here rather than the identifier.) And the class's own
# overview: "A server that implements this requirements class provides the
# ability to add, replace and/or remove individual resources from a
# collection." This deployment provides none of it, and never could.
#
# So the claim is withdrawn, and this is where those bytes move: one more URI
# leaves this deployment's `/conformance`. Same direction as the `#150` and
# `#208` withdrawals above — telling clients less than before, never more —
# and it costs this demo no capability at all: it could not write a minute ago
# and it cannot write now, which every assertion in this section proves.

# The pairing, in the three method shapes Requirement 1 clause A names. Each
# must be refused BY NAME. A 2xx here would mean this demo had silently become
# writable — the only circumstance under which the class could be declared
# again, and the one this assertion exists to notice.
for verb in POST PUT DELETE; do
  case "$verb" in
  POST) TARGET="$ROADS/items" ;;
  *) TARGET="$ROADS/items/1" ;;
  esac
  CODE=$(curl -s -o /dev/null -w '%{http_code}' -X "$verb" \
    -H 'Content-Type: application/geo+json' \
    --data '{"type":"Feature","geometry":null,"properties":{}}' \
    "http://127.0.0.1:$PORT$TARGET")
  case "$CODE" in
  2*) fail "$verb $TARGET returned $CODE, but this deployment declares no write lane" ;;
  esac
  REFUSAL=$(curl -s -X "$verb" -H 'Content-Type: application/geo+json' \
    --data '{"type":"Feature","geometry":null,"properties":{}}' \
    "http://127.0.0.1:$PORT$TARGET")
  printf '%s' "$REFUSAL" | grep -Fq "does not support 'write'" ||
    fail "$verb $TARGET must be refused by name; got: $REFUSAL"
  ok "$verb $TARGET is refused by name"
done

# ...and therefore the class is absent. Asserted after the behaviour, never
# instead of it: a `/conformance` list checked alone could be confidently
# wrong and still pass, which is exactly how the overclaim survived until now.
PART4=$(body_of '/public/features/catalogs/default/conformance')
expect_body_lacks 'features /conformance' "$PART4" \
  'ogcapi-features-4/1.0/conf/create-replace-delete'

# Withheld alongside it, and now provably so rather than incidentally:
# `conf/features`'s own Dependency row names Requirements Class
# "Create/Replace/Delete" (clause 9.1), which clause 5.4 makes a dependency
# "Every server implementing the requirements class has to conform to", and
# `conf/update` needs a routed read/write pair this deployment does not have.
# Together with the `req/optimistic-locking-etags` assertion above, that is
# four of OGC 20-002r1 Table 2's five classes named absent here; the fifth,
# `req/optimistic-locking-timestamps`, is per-collection and has never been
# declared at a root.
expect_body_lacks 'features /conformance' "$PART4" 'ogcapi-features-4/1.0/conf/features'
expect_body_lacks 'features /conformance' "$PART4" 'ogcapi-features-4/1.0/conf/update'
expect_body_lacks 'features /conformance' "$PART4" \
  'ogcapi-features-4/1.0/req/optimistic-locking-timestamps'

# The narrowing is exactly that narrow. Every Part 1 and Part 3 class this
# deployment earned before is still declared, byte-for-byte — this slice
# removes one URI from one list and touches nothing else a client reads.
for kept in \
  'ogcapi-features-1/1.0/conf/core' \
  'ogcapi-features-1/1.0/conf/oas30' \
  'ogcapi-features-1/1.0/conf/geojson' \
  'ogcapi-features-3/1.0/conf/queryables'; do
  expect_body_contains 'features /conformance' "$PART4" "$kept"
done

# --- and readiness, unmoved -------------------------------------------------

# `#161`. This deployment's `cache:` block sets `memory_percent` and nothing
# else: no `cache.l2`, so no L2 tier is configured, so there is nothing about
# one to report. An L2 cache is an optimization; its absence when nobody asked
# for it is not a degradation and must not be described as one.
#
# The assertion is byte-level on purpose. A status-code check alone would pass
# even if readiness started volunteering a body, and a body is exactly how this
# change could leak into a deployment that never opted in.
#
# `/healthz` (waited on above) is dependency-free, so it turns 200 before the
# first dependency probe has run; readiness needs its own wait — see
# `wait_ready` for why it waits for a state that is held, not one that merely
# occurred.
wait_ready

READY=$(curl -s -w '\n%{http_code}' "http://127.0.0.1:$PORT/readyz")
READY_CODE=$(printf '%s' "$READY" | tail -n 1)
READY_BODY=$(printf '%s' "$READY" | sed '$d')
[ "$READY_CODE" = "200" ] || fail "GET /readyz returned $READY_CODE, expected 200"
ok 'GET /readyz -> 200'
[ -z "$READY_BODY" ] ||
  fail "GET /readyz must return an empty body with no cache.l2 configured, got: $READY_BODY"
ok '/readyz body is empty, exactly as before'
READY_CT=$(curl -s -o /dev/null -w '%{content_type}' "http://127.0.0.1:$PORT/readyz")
[ -z "$READY_CT" ] ||
  fail "GET /readyz must send no Content-Type with no cache.l2 configured, got: $READY_CT"
ok '/readyz sends no Content-Type'

# The availability gauge is scoped to deployments that configured a tier: an
# unconfigured one registers no series at all, so an alert on
# `tile_cache_l2_available == 0` can never fire for a cache nobody asked for.
METRICS=$(body_of '/metrics')
expect_body_lacks '/metrics' "$METRICS" 'tile_cache_l2_available'

# --- and no modified-column touch trigger, because none was asked for -------
#
# `#151` added `tellurion-ingest locking install-touch-trigger`, which
# provisions a `BEFORE INSERT OR UPDATE ... SET <modified_column> = now()`
# trigger next to an operator-declared `modified_column`. This deployment
# declares no `modified_column` on any collection — the config above is
# verbatim, and has none — so it never ran that command and nothing about it
# changes. Asserted rather than assumed, in three directions:
#
#  1. the Timestamps requirement class stays absent from `/conformance` and
#     from the Collection document. It is gated on the declaration, not on the
#     trigger, and the declaration is still absent.
#  2. no feature response carries a `Last-Modified`. That header is only ever
#     read out of a declared column and never fabricated, so its absence here
#     is the same absence it always was.
#  3. and there is no path by which this deployment could acquire a trigger
#     even if someone tried: its only storage is a GeoPackage, and the command
#     refuses every driver but PostGIS BY NAME. Executed against the real
#     binary and this deployment's own config file, so it is a fact about this
#     deployment rather than a claim about the code.
CONFORMANCE_TOUCH=$(body_of '/public/features/catalogs/default/conformance')
expect_body_lacks 'features /conformance' "$CONFORMANCE_TOUCH" 'optimistic-locking-timestamps'
ROADS_DOC=$(body_of '/public/features/catalogs/default/collections/rome_roads')
expect_body_lacks 'features /collections/rome_roads' "$ROADS_DOC" 'optimistic-locking-timestamps'

ITEM_HEADERS=$(curl -s -o /dev/null -D - \
  "http://127.0.0.1:$PORT/public/features/catalogs/default/collections/rome_roads/items/1")
printf '%s' "$ITEM_HEADERS" | grep -iq '^last-modified:' &&
  fail 'this deployment declares no modified_column and must emit no Last-Modified'
ok 'no Last-Modified, exactly as before'

if TOUCH_OUT=$(TELLURION_GEOPACKAGE_PATH="$GPKG" "$INGEST" locking install-touch-trigger \
  --config "$CONFIG" --collection rome_roads 2>&1); then
  fail 'locking install-touch-trigger must refuse this deployment by name'
fi
printf '%s' "$TOUCH_OUT" | grep -Fq "driver 'geopackage'" ||
  fail "the refusal must name this deployment's driver: $TOUCH_OUT"
ok "locking install-touch-trigger refuses this deployment's storage, naming its driver"
printf '%s' "$TOUCH_OUT" | grep -Fq "'postgis'" ||
  fail "the refusal must name the one driver it does support: $TOUCH_OUT"
ok 'the refusal names postgis, the one driver the command implements'

# --- and write-reactive tile invalidation, invisible (`#142`, `#141`) --------
#
# This deployment declares no `server.tile_invalidation` block and no
# per-collection `tile_invalidation:` flag, so `#142`/`#141` must be entirely
# absent from it. Absence is asserted three ways, because the interesting
# failure of a change like this one is not "the new thing is broken" but "the
# new thing showed up somewhere nobody asked for it".
#
# First: no metric series at all. A consumer that spawned would emit a lag
# gauge on its very first pass, and the conservative-fallback counter `#142`
# added would appear the moment it drained anything it could not map. Neither
# may exist here — the same "an alert can never fire for a thing nobody
# configured" rule the `tile_cache_l2_available` check above applies.
METRICS_INVALIDATION=$(body_of '/metrics')
expect_body_lacks '/metrics' "$METRICS_INVALIDATION" 'tile_invalidation_generation_lag'
expect_body_lacks '/metrics' "$METRICS_INVALIDATION" 'tile_invalidation_bumps_total'
expect_body_lacks '/metrics' "$METRICS_INVALIDATION" 'tile_invalidation_unrecorded_extent_total'

# Second: the tiles this demo exists to serve are byte-for-byte stable. Two
# fetches of the same coordinate must return the identical body — which is
# what a cache key whose generation component is a constant `0` guarantees,
# and what a generation that had started moving under a read-only deployment
# would break.
TILE_A="$WORK/tile-a.mvt"
TILE_B="$WORK/tile-b.mvt"
TILE_PATH='/public/tiles/catalogs/default/collections/rome_roads/tiles/WebMercatorQuad/0/0/0'
curl -sf -o "$TILE_A" "http://127.0.0.1:$PORT$TILE_PATH" ||
  fail "GET $TILE_PATH failed"
curl -sf -o "$TILE_B" "http://127.0.0.1:$PORT$TILE_PATH" ||
  fail "GET $TILE_PATH failed on the second fetch"
[ -s "$TILE_A" ] || fail "$TILE_PATH returned an empty body; the fixture must render something"
cmp -s "$TILE_A" "$TILE_B" ||
  fail 'two fetches of the same tile returned different bytes'
ok 'a tile served twice is byte-for-byte identical'

# Third: the DDL contract, executed on the deployed provisioning command
# itself. `#141`/`#142` made the server refuse a write by name when the outbox
# lacks `extent_crs84`, on the understanding that ingest is where the column
# comes from — so the invocation this demo actually runs must emit it. That is
# checked against the DDL the command printed (captured above), not inferred.
grep -Fq 'extent_crs84' "$WORK/provision.sql" ||
  fail 'the deployed create-tables invocation must provision the extent_crs84 column'
ok 'the deployed provisioning emits the extent_crs84 outbox column'

# And the change-feed lane, which is the one read surface that names that
# column in its SQL, stays exactly as absent as it was: this deployment
# declares no `routing.write`, so it has no outbox lane to serve from, and
# growing a column changed nothing about that.
expect_status '/public/features/catalogs/default/collections/rome_roads/changes' 404

printf 'PASS: italy contract smoke, %s checks\n' "$CHECKS"
