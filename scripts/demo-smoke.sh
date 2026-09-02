#!/bin/sh
# Synthetic end-to-end smoke over the real binaries: `tellurion-ingest`
# provisions a GeoPackage, `tellurion` serves it, and every assertion below
# is made over HTTP against that running server. No database service, no
# container runtime, no network.
#
# What it is for (`#192`): the records lane is a *partition* of a catalog
# across protocol roots, and a partition is only correct if both halves agree.
# Asserting that in a unit test means building a world in which it is true by
# construction; asserting it against a booted server over real HTTP does not.
#
# The script serves the same catalog under a series of configurations, each
# on its own port so no phase can be answered by an earlier phase's server:
#
#   Phase 1 — a config that never mentions records at all. Every assertion is
#             an assertion of *absence*: this is the "an unconfigured
#             deployment is byte-for-byte what it was" rule, executed.
#   Phase 2 — the same catalog with `kind: record` on one collection and
#             `protocols: { records: enabled }`. Now the partition must hold
#             in both directions.
#   Phase 3 — (`#182`) the Processes lane asked for by an operator whose only
#             storage cannot hold a durable job ledger. The root must still be
#             absent, and the refusal must be named in the log — the
#             capability gate, executed rather than asserted.
#   Phase 4 — (`#150`) optimistic locking asked for on a write lane that
#             cannot evaluate the precondition atomically. The ETags class
#             must not be advertised, a conditional write must be refused by
#             name, and an unconditional write must be untouched.
#   Phase 5 — (`#161`) an L2 tile cache configured against a backend that is
#             not there. The server must boot, serve, and report the tier by
#             name on a still-200 readiness — a cache outage degrades a
#             replica, it never removes one.
#   Phase 6 — (`#247`) a collection whose storage CRS is projected, served by a
#             driver that cannot transform a filter's spatial literals. A
#             spatial filter carrying no `filter-crs` must be refused BY NAME
#             rather than answered in the wrong CRS — while the CRS84
#             collection beside it, and every non-spatial filter, are untouched.
#   Phase 7 — (`#245`) two registered styles, one of which paints none of the
#             collection's MVT layers. The TileSet resource must advertise
#             only the one that draws something — while both styled-map
#             routes keep serving, because this narrows an advertisement, not
#             a capability.
#   Phase 8 — (`#227`) the same projected collection phase 6 uses, asked what
#             CRS its responses are in. `Content-Crs`, the collection's `crs`
#             list and its `storageCrs` must all name the storage CRS the
#             coordinates are genuinely in, and a request for CRS84 must be
#             refused by name — while the 4326 collection beside it, on the
#             same driver and the same file, is untouched.
#   Phase 9 — (`#208`) two collections in one catalog, alike in everything but
#             `routing.write`. The `Allow` an `OPTIONS` reports must be the
#             methods that collection really accepts — checked by then issuing
#             the method it named, so a header that is confidently wrong
#             cannot pass.
#  Phase 10 — (`#255`) the same projected collection phases 6 and 8 use, asked
#             for a `bbox` with no `bbox-crs`. Such a bbox is CRS84 (Part 1
#             Requirement 23 clause C), this driver cannot transform it into
#             metres, and comparing the four numbers raw is a 200 with the
#             wrong rows — so it must be refused BY NAME on every lane that
#             takes a bbox, while the 4326 collection beside it, and the same
#             collection asked with `bbox-crs` naming its own storage CRS, are
#             served.
#  Phase 11 — (`#142`, `#141`) write-reactive tile invalidation, asked the
#             only question that matters: after a write lands, does the very
#             next tile fetch show it? Once for a write submitted in a
#             PROJECTED CRS (`#142` — its coordinates are metres, and reading
#             them as degrees invalidated a bucket on the far side of the
#             world while the tile that renders the feature kept serving its
#             pre-write bytes with a 200), and once for a DELETE (`#141` —
#             an obligation carrying no geometry at all). The tile cache is
#             what makes this a gate rather than a formality: a tile whose
#             generation does not move is answered from the cache, so a
#             broken invalidation shows up as a stale body, not an error.
#   Phase 12 — (`#134`) the CQL2 `basic-spatial-functions` class, which the
#             GeoPackage driver no longer declares because it compiles
#             `S_INTERSECTS` only in a restricted positional form. The three
#             general-form compositions the class's own Abstract Test Suite
#             lists must be refused by name, the restricted form must keep
#             working, and the class must be absent from `/conformance` and
#             from the collection document alike — advertisement and
#             behaviour checked against each other, never separately.
#  Phase 14 — (`#151`) the opt-in `modified_column` touch trigger. Every
#             driver but PostGIS is refused BY NAME, a collection that
#             declares no `modified_column` is refused rather than given an
#             invented one, the PostGIS DDL is printed with no database in
#             sight, and the booted deployment — which asks for none of it —
#             advertises and serves exactly what it did before.
#             (Phase 13 is claimed by a slice in flight.)
#  Phase 19 — (`#162`) the registry-backend seam, named. A deployment that
#             says nothing about `registry` at all still serves what it
#             always did, while the boot log enumerates what this binary
#             actually contains: `file` as the direct built-in backend and
#             `postgis` as the registered relational implementation, for both
#             halves of the one knob. A `registry.implementation` naming
#             something absent then refuses to boot, by name, listing what IS
#             registered — never a silent fall back to the file backend.
#  Phase 22 — (`#37`) a COG-backed collection asked for an OGC API Maps
#             `/map`. A raster collection has no vector `TileSource` at all,
#             so before this slice the route 404'd for it and nothing
#             advertised it. The map must now render through `RasterSource`,
#             carry both Maps Part 1 Core response headers, be classified by
#             the collection's own colormap, refuse a `style` and an
#             over-budget window BY NAME, and be advertised exactly where it
#             resolves — while the GeoPackage-backed vector collection
#             beside it is untouched.
#  Phase 23 — (`#254`) a bounded COG MOSAIC: one raster TileSet composed from
#             a manifest sidecar `tellurion-ingest cog mosaic` measured out of
#             three real GeoTIFFs. The manifest must be authored with measured
#             provenance, the composed tiles must reach the wire as PNG, the
#             composition order must be observable in the bytes (the tile on
#             each side of the seam is the OVERLAPPING source's own pixels,
#             because its id sorts last), MVT must be refused BY NAME, and two
#             hand-broken manifests — one over the 32-source bound, one with a
#             SHA-256 that no longer matches its object — must refuse the BOOT
#             by name rather than serve anything at all.
#
# Exit status is the gate: 0 and a final `PASS` line, or a `FAIL` line naming
# what disagreed.
#
# `#260`: every phase's config lives in a directory of its own, holding
# nothing else, and this script's own preconditions are checked and named
# before any assertion runs. See `config_for` and the preflight block below
# for what each of those buys and why a harness that skips them is worse than
# no gate at all.

set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
WORK=$(mktemp -d)
SERVER_PID=""

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
# gets waved through. Three separate times this campaign has been misled by a
# harness fault presenting as a code defect: a server from a sibling worktree
# answering on a shared port, a config reload invalidating readiness mid-phase
# (`#260` itself, fixed by `config_for` below), and a PostgreSQL cluster that
# was simply down. Each one first appeared as a confident, wrong `FAIL` about
# a collection document or a readiness body.
#
# So this script holds itself to the same rule its server code is held to:
# a precondition it cannot meet is refused BY NAME, never left to surface as
# an assertion failure two hundred lines later.

# Whether anything is accepting TCP connections on port `$1`. `curl` is
# already this script's only network dependency, so the check reuses it
# rather than adding a hard one on `ss`/`lsof`/`nc`, neither of which is
# present everywhere this runs: curl's exit code 7 is "could not connect",
# i.e. nothing is there. Any other outcome — an HTTP answer, a TLS error, a
# timeout against something that accepted and then went quiet — means the
# port is taken, which is the condition that let a sibling worktree's server
# answer a phase's requests.
port_is_free() {
  code=0
  curl -s -o /dev/null --max-time 5 "http://127.0.0.1:$1/" >/dev/null 2>&1 || code=$?
  [ "$code" = "7" ]
}

# Best-effort "and here is who has it", appended to the refusal so an
# operator can act on it without a second investigation. Enrichment of a
# refusal that is already named, not a check of its own — a host with neither
# tool still gets the refusal, just without the pid.
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
  fail "port $1 is already in use, so this phase's own server would not be the
  one answering it. Either a server from a sibling worktree or one that
  outlived its stop_server is listening there. Stop THAT process by pid — a
  broad pkill would take out concurrent worktrees' servers too. $(port_occupant "$1")"
}

# Every `driver: <name>` / `url_env: <VAR>` pair in the config about to be
# booted, read out of the file the server itself is about to read so it
# cannot drift from what this run genuinely depends on. Anything but
# `geopackage` needs a service this script does not start, so its DSN must be
# set, and — where `pg_isready` exists to say so — reachable. No phase boots
# such a storage today; the day one does, a cluster that is down says so here
# by name instead of arriving as an unexplained storage-dependency failure.
require_bootable_storages() {
  # Scoped to the top-level `storages:` block: `cache.l2` also carries a
  # `url_env`, and phase 5's is a Valkey URL that is deliberately unreachable
  # — reading it as a storage DSN would turn this check into exactly the kind
  # of confident, wrong failure it exists to prevent.
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
    # `#37`: a `cog` storage's locator is a local GeoTIFF path, not a DSN --
    # handing it to `pg_isready` below would turn this check into exactly the
    # confident, wrong failure it exists to prevent. Its own precondition is
    # that the file is there and readable, named here for the same reason.
    # `#254`: a `cog-mosaic` storage's locator is the path of a manifest
    # SIDECAR (authored by `tellurion-ingest cog mosaic`), not a DSN and not
    # a GeoTIFF -- but the precondition is the same shape as `cog`'s: the file
    # this run is about to serve from has to be there and readable.
    if [ "$driver" = cog ] || [ "$driver" = cog-mosaic ]; then
      [ -r "$dsn" ] ||
        fail "$1 routes a '$driver' storage at \$$url_env=$dsn, which is not a
  readable file. This phase serves a committed GeoTIFF fixture (or a manifest
  authored from one) out of the worktree; a missing one is a harness fault,
  not a regression"
      continue
    fi
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

command -v timeout >/dev/null 2>&1 ||
  fail 'timeout is not on PATH, and phases 18 and 19 bound boots they expect to refuse'
ok 'timeout is on PATH'

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
  answer this run's ports. Stop that pid specifically — never a broad pkill,
  sibling worktrees are live:
$STRAY"
ok 'no tellurion process from this worktree is already running'

# Phase 14 proves `--dry-run` reaches no database by naming an environment
# variable that is never set; a value here would make that proof vacuous. This
# is the one database-shaped variable any config in this run mentions, and the
# statement about it is "must be unset", not "must be reachable".
[ -z "${TELLURION_SMOKE_UNSET_DATABASE_URL:-}" ] ||
  fail 'TELLURION_SMOKE_UNSET_DATABASE_URL is set, which would make phase 14 prove
  nothing: it exists precisely so a command that reached for a database would
  fail. Unset it and re-run'
ok 'TELLURION_SMOKE_UNSET_DATABASE_URL is unset, as phase 14 requires'

# --- build ------------------------------------------------------------------

CARGO_PROFILE_DEV_DEBUG=0 cargo build --quiet \
  -p tellurion -p tellurion-ingest >"$WORK/build.log" 2>&1 ||
  { cat "$WORK/build.log" >&2; fail 'cargo build'; }

test -x "$TELLURION" || fail "missing binary $TELLURION"
test -x "$INGEST" || fail "missing binary $INGEST"

# --- fixture: one vector collection and one geometry-less record collection --

GPKG="$WORK/smoke.gpkg"

# Ingest owns every piece of DDL here; the server never issues any. Both
# tables are provisioned by the same `geopackage create-tables` subcommand,
# because a record collection is an ordinary collection — the difference is
# declared in config, not built into the file.
"$INGEST" geopackage create-tables \
  --path "$GPKG" --table smoke_points --geometry geom --srid 4326 \
  --geometry-type POINT --columns name:TEXT >/dev/null 2>&1 ||
  fail 'ingest geopackage create-tables (smoke_points)'

"$INGEST" geopackage create-tables \
  --path "$GPKG" --table smoke_records --geometry geom --srid 4326 \
  --geometry-type GEOMETRY --columns title:TEXT,subject:TEXT >/dev/null 2>&1 ||
  fail 'ingest geopackage create-tables (smoke_records)'

# `#247`'s fixture: the same file, one more table, registered at EPSG:3857
# instead of 4326. Ingest owns this DDL exactly as it owns the other two —
# the server issues none of it, and phase 6 is the only phase that names this
# table at all. Left empty on purpose: what phase 6 asserts is a refusal
# decided from the collection's storage SRID and the driver's capabilities,
# neither of which is a function of how many rows are in the table.
"$INGEST" geopackage create-tables \
  --path "$GPKG" --table smoke_mercator --geometry geom --srid 3857 \
  --geometry-type POINT --columns name:TEXT >/dev/null 2>&1 ||
  fail 'ingest geopackage create-tables (smoke_mercator)'

cat >"$WORK/points.geojson" <<'JSON'
{"type":"FeatureCollection","features":[
{"type":"Feature","id":"1","geometry":{"type":"Point","coordinates":[12.49,41.90]},"properties":{"name":"alpha"}},
{"type":"Feature","id":"2","geometry":{"type":"Point","coordinates":[9.19,45.46]},"properties":{"name":"bravo"}}
]}
JSON

# Every record carries `"geometry": null`. OGC API - Records - Part 1: Core
# lists `geometry` as an OPTIONAL core property of a record (Table 9, "Can be
# null if there is no associated spatial extent"), and Permission 4
# (/per/record-core/geometry) leaves making it mandatory to specific
# communities of interest. This is the geometry-less content the lane exists
# to serve, stored in a real GeoPackage through the real write path.
cat >"$WORK/records.geojson" <<'JSON'
{"type":"FeatureCollection","features":[
{"type":"Feature","id":"1","geometry":null,"properties":{"title":"Hydrography thesaurus","subject":"water"}},
{"type":"Feature","id":"2","geometry":null,"properties":{"title":"Road network register","subject":"transport"}}
]}
JSON

"$INGEST" geopackage load --path "$GPKG" --table smoke_points \
  "$WORK/points.geojson" >/dev/null 2>&1 || fail 'ingest geopackage load (smoke_points)'
"$INGEST" geopackage load --path "$GPKG" --table smoke_records \
  "$WORK/records.geojson" >/dev/null 2>&1 || fail 'ingest geopackage load (smoke_records)'

# --- server helpers ---------------------------------------------------------

# The port the phase currently under test serves on. A variable rather than a
# literal because the phases no longer all share one: each binds its own, so a
# phase whose server outlives `stop_server` cannot leave a later phase quietly
# answering from the wrong process — a failure mode that reads as a real
# assertion failure and is miserable to diagnose. Phases 1-4 keep the port
# they have always used; each new phase claims the next one (5 -> 18193, 6 ->
# 18194, 7 -> 18195, 8 -> 18196, 9 -> 18197, 10 -> 18198, 11 -> 18199, 12 ->
# 18200, 14 -> 18202, 18 -> 18206, 19 -> 18207, 21 -> 18209, 22 -> 18210,
# 23 -> 18211, 24 -> 18212, 25 -> 18213, 26 -> 18214, 27 -> 18215). A new
# phase claims its NUMBER and its PORT together, and takes the next of each even
# if the phase before it has not landed yet — two
# slices in flight at once have already collided by each independently reaching
# for "the next one". That is why the numbering skips: phase 13 (port 18201)
# was claimed by a slice still in flight when phase 14 was written. (Phase 11
# was reserved the same way while `#142`/`#141` was in flight, and has since
# landed below, between phases 10 and 12; phase 26 was reserved the same way
# by `#287` while phase 27 was written, and has since landed below too.)
SMOKE_PORT=18192

# Where a phase's config file must live (`#260`). Prints
# `$WORK/cfg/<name>/<name>.yaml`, creating the directory.
#
# The config-reload file watch is on the config file's PARENT DIRECTORY, not
# the file — a mounted ConfigMap swaps a symlink rather than rewriting the
# file, so watching the directory is the right thing for the deployment shape
# it was designed for. The cost is that ANY file written beside the config
# looks like a config change: `server.log`, which this script truncates and
# the server then appends to for the whole phase, and `smoke.gpkg`, which the
# write phases really do modify. Every one of those writes used to trigger a
# reload, and every reload invalidates readiness for a short window — which is
# how `#227`'s verification collected a `FAIL` on a `/readyz` assertion
# belonging to neither the phase nor the branch under test.
#
# A directory per config, holding that config and nothing else, removes the
# feedback loop rather than shrinking the window. Per config rather than one
# shared `cfg/` because the isolation must hold in both directions: a phase's
# server must not see its own log, and must not see the NEXT phase's config
# being written either. `#161`'s phase 5 established the pattern with its own
# `$WORK/cfg/`; this generalises it, and phase 5 now goes through the same
# helper as everyone else.
config_for() {
  mkdir -p "$WORK/cfg/$1" ||
    fail "could not create the config directory for '$1'"
  printf '%s/cfg/%s/%s.yaml' "$WORK" "$1" "$1"
}

start_server() {
  config=$1
  # Checked here rather than from a list at the top of the file: every phase
  # sets `SMOKE_PORT` and calls this, so there is no second registry of ports
  # to drift out of step with the first.
  require_free_port "$SMOKE_PORT"
  require_bootable_storages "$config"
  : >"$WORK/server.log"
  RUST_LOG=info \
    TELLURION_CONFIG="$config" \
    TELLURION_SMOKE_GPKG="$GPKG" \
    PORT="$SMOKE_PORT" \
    "$TELLURION" >"$WORK/server.log" 2>&1 &
  SERVER_PID=$!
  i=0
  # 200 * 0.2s = 40s. Generous on purpose: a phase that configures an optional
  # backend the server has to give up on before it can serve (`cache.l2`,
  # phase 5) spends real seconds inside `build_cache` before binding a port.
  while [ "$i" -lt 200 ]; do
    if curl -fsS -o /dev/null "http://127.0.0.1:$SMOKE_PORT/healthz" 2>/dev/null; then
      return 0
    fi
    kill -0 "$SERVER_PID" 2>/dev/null || fail "server exited during startup ($config)"
    i=$((i + 1))
    sleep 0.2
  done
  fail "server never became healthy ($config)"
}

# `/healthz` is dependency-free and turns 200 before the first dependency
# probe has run, so anything asserting on `/readyz` waits for it separately.
#
# STABLY ready, not momentarily ready (`#260`). The caller's next act is
# always a second and third request against `/readyz` — status, then body,
# then `Content-Type` — and a single 200 says only that readiness was 200 at
# one instant. `#227`'s false failure was exactly that: the first of two
# back-to-back calls passed, a readiness invalidation landed 1.7ms later, and
# the second returned the `503`'s `application/problem+json`. Requiring
# consecutive 200s over a window means what the caller then asserts on is a
# state the process is actually holding.
#
# This is belt to `config_for`'s braces, not a substitute for it: with the
# config isolated there is nothing left to invalidate readiness mid-phase, and
# a wait that had to paper over a real flap would be hiding a defect rather
# than a harness fault. Deliberately NOT folded into `start_server`, which
# waits on `/healthz` alone: making every phase wait for readiness would
# change what phases that never mention readiness do, and this slice changes
# a phase's isolation, never its behaviour.
READY_STABLE_POLLS=3

wait_ready() {
  i=0
  stable=0
  while [ "$i" -lt 200 ]; do
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

stop_server() {
  [ -n "$SERVER_PID" ] || return 0
  kill "$SERVER_PID" 2>/dev/null || true
  wait "$SERVER_PID" 2>/dev/null || true
  SERVER_PID=""
}

# Prints the HTTP status code of a GET.
status_of() {
  curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$SMOKE_PORT$1"
}

# Prints the body of a GET, failing the script if the status is not 200.
body_of() {
  out=$(curl -s -w '\n%{http_code}' "http://127.0.0.1:$SMOKE_PORT$1")
  code=$(printf '%s' "$out" | tail -n 1)
  [ "$code" = "200" ] || fail "GET $1 returned $code, expected 200"
  printf '%s' "$out" | sed '$d'
}

expect_status() {
  actual=$(status_of "$1")
  [ "$actual" = "$2" ] || fail "GET $1 returned $actual, expected $2"
  ok "GET $1 -> $2"
}

# grep -F over a body, with a human-readable label.
expect_body_contains() {
  printf '%s' "$2" | grep -Fq -- "$3" || fail "$1: expected to find '$3'"
  ok "$1 contains '$3'"
}

expect_body_lacks() {
  printf '%s' "$2" | grep -Fq -- "$3" && fail "$1: must NOT contain '$3'"
  ok "$1 does not contain '$3'"
}

has_png_signature() {
  [ "$(od -An -tx1 -N 4 "$1" | tr -d ' \n')" = '89504e47' ]
}

# --- phase 1: a config that never mentions records --------------------------

PLAIN_CONFIG=$(config_for plain)
cat >"$PLAIN_CONFIG" <<'YAML'
control_store:
  backend: legacy_file
server:
  port: 18192
storages:
  - id: main
    driver: geopackage
    url_env: TELLURION_SMOKE_GPKG
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: smoke_points
    catalog: default
    storage: main
  - id: smoke_records
    catalog: default
    storage: main
YAML

printf 'phase 1: a deployment that never asked for the records lane\n'
start_server "$PLAIN_CONFIG"

DIR=$(body_of "/public")
expect_body_contains 'tenant directory' "$DIR" '/public/features/catalogs/default'
expect_body_contains 'tenant directory' "$DIR" '/public/stac/catalogs/default'
# The whole of rule 1 in one assertion: no `records` root is advertised to a
# deployment that never asked for one.
expect_body_lacks 'tenant directory' "$DIR" '/public/records/catalogs/default'

# ... and the prefix answers exactly what an unmounted prefix answers.
expect_status '/public/records/catalogs/default' 404
expect_status '/public/records/catalogs/default/collections' 404

COLLECTIONS=$(body_of "/public/features/catalogs/default/collections")
# With no `kind` declared, both collections are `vector` — which is exactly
# how each of them behaved before `kind` existed.
expect_body_contains 'features /collections' "$COLLECTIONS" '"smoke_points"'
expect_body_contains 'features /collections' "$COLLECTIONS" '"smoke_records"'

CONFORMANCE=$(body_of "/public/features/catalogs/default/conformance")
expect_body_lacks 'features /conformance' "$CONFORMANCE" 'ogcapi-records-1'

# --- Part 4 Create/Replace/Delete, withheld from a read-only deployment -----
#
# `#263`. Neither collection here declares `routing.write`, and there is no
# "defaults to the single storage" fallback for the write lane, so this
# deployment offers no mutable resource at all.
#
# OGC API - Features - Part 4 (OGC 20-002r1) Requirement 1 clause A: "A server
# SHALL implement one or more of the methods HTTP POST, PUT and/or DELETE for
# each mutable resource." (The published text identifies that requirement
# `/req/core/methods`, inside a clause whose every other requirement is
# `/req/create-replace-delete/...`; the prose is what is cited.) And the
# class's own overview: "A server that implements this requirements class
# provides the ability to add, replace and/or remove individual resources from
# a collection." This deployment provides none of it.
#
# The pair is what makes either half worth asserting: issue the three methods
# the class is about, then read the list. A `/conformance` list checked alone
# could be confidently wrong and still pass.
for verb in POST PUT DELETE; do
  case "$verb" in
  POST) TARGET='/public/features/catalogs/default/collections/smoke_points/items' ;;
  *) TARGET='/public/features/catalogs/default/collections/smoke_points/items/1' ;;
  esac
  CODE=$(curl -s -o /dev/null -w '%{http_code}' -X "$verb" \
    -H 'Content-Type: application/geo+json' \
    --data '{"type":"Feature","geometry":null,"properties":{}}' \
    "http://127.0.0.1:$SMOKE_PORT$TARGET")
  case "$CODE" in
  2*) fail "$verb $TARGET returned $CODE, but this deployment declares no write lane" ;;
  esac
  REFUSAL=$(curl -s -X "$verb" -H 'Content-Type: application/geo+json' \
    --data '{"type":"Feature","geometry":null,"properties":{}}' \
    "http://127.0.0.1:$SMOKE_PORT$TARGET")
  printf '%s' "$REFUSAL" | grep -Fq "does not support 'write'" ||
    fail "$verb $TARGET must be refused by name; got: $REFUSAL"
  ok "$verb $TARGET is refused by name"
done

expect_body_lacks 'features /conformance' "$CONFORMANCE" \
  'ogcapi-features-4/1.0/conf/create-replace-delete'
# Withheld alongside it, and for the same reason: `conf/features`'s own
# Dependency row names Requirements Class "Create/Replace/Delete", and
# `conf/update` needs a routed read/write pair this deployment does not have.
expect_body_lacks 'features /conformance' "$CONFORMANCE" \
  'ogcapi-features-4/1.0/conf/features'
expect_body_lacks 'features /conformance' "$CONFORMANCE" \
  'ogcapi-features-4/1.0/conf/update'
# And nothing else moved: the Part 1 and Part 3 classes this read-only
# deployment genuinely earns are exactly what they were.
for kept in \
  'ogcapi-features-1/1.0/conf/core' \
  'ogcapi-features-1/1.0/conf/oas30' \
  'ogcapi-features-1/1.0/conf/geojson' \
  'ogcapi-features-3/1.0/conf/queryables'; do
  expect_body_contains 'features /conformance' "$CONFORMANCE" "$kept"
done

# --- the STAC Collection items link (`#245`) --------------------------------
#
# This root declares `https://api.stacspec.org/v1.0.0/ogcapi-features` and
# `http://www.opengis.net/spec/ogcapi-features-1/1.0/conf/core`, and both
# require a Collection document to link to that collection's items resource
# (OGC 17-069r4 Requirement 15 `/req/core/fc-md-items-links`, carried onto the
# single-collection resource by Requirement 19 `/req/core/sfc-md-success`;
# STAC API - Features states the same rule in prose). Until `#245` the
# documents carried only `root`/`self`/`parent`/`alternate`.
#
# Every assertion follows the link. A `rel="items"` pointing at nothing would
# be the same overclaim one level down.

STAC_ITEMS_HREF='/public/stac/catalogs/default/collections/smoke_points/items'
STAC_COLLECTION=$(body_of '/public/stac/catalogs/default/collections/smoke_points')
expect_body_contains 'stac /collections/smoke_points' "$STAC_COLLECTION" '"rel":"items"'
expect_body_contains 'stac /collections/smoke_points' "$STAC_COLLECTION" \
  "\"href\":\"$STAC_ITEMS_HREF\""

STAC_LISTING=$(body_of '/public/stac/catalogs/default/collections')
expect_body_contains 'stac /collections' "$STAC_LISTING" "\"href\":\"$STAC_ITEMS_HREF\""

STAC_ITEMS=$(body_of "$STAC_ITEMS_HREF")
expect_body_contains 'stac items (followed from the link)' "$STAC_ITEMS" '"FeatureCollection"'
expect_body_contains 'stac items (followed from the link)' "$STAC_ITEMS" '"alpha"'

# --- STAC /search filter-crs (`#248`) ---------------------------------------
#
# The STAC API Filter Extension pins this parameter to CRS84: "server must
# only accept `http://www.opengis.net/def/crs/OGC/1.3/CRS84` as a valid value,
# may reject any others". Before `#248` the parameter was accepted, dropped,
# and the filter's geometries processed in CRS84 regardless — a 200 carrying
# rows selected in a CRS the client never named.
#
# The filter below is longitude-first and covers `alpha` at (12.49, 41.90).
# Read latitude-first — which is what EPSG:4326-referenced-by-authority means —
# the same four numbers describe a box near (41.9, 12.5), where nothing is
# seeded. That is the whole stake of the parameter, so this deployment must
# either honour it or say it cannot; what it must never do again is answer
# `alpha` to both.

SEARCH_BASE='/public/stac/catalogs/default/search?collections=smoke_points'
SPATIAL_FILTER='filter=S_INTERSECTS%28geom%2CBBOX%2812%2C41.5%2C13%2C42.5%29%29'
CRS84_ENC='http%3A%2F%2Fwww.opengis.net%2Fdef%2Fcrs%2FOGC%2F1.3%2FCRS84'
EPSG4326_ENC='http%3A%2F%2Fwww.opengis.net%2Fdef%2Fcrs%2FEPSG%2F0%2F4326'

SEARCH=$(body_of "$SEARCH_BASE&$SPATIAL_FILTER")
expect_body_contains 'stac /search (no filter-crs)' "$SEARCH" '"alpha"'
expect_body_lacks 'stac /search (no filter-crs)' "$SEARCH" '"bravo"'

# CRS84 named explicitly: this collection is stored at SRID 4326, so honouring
# it asks the driver for nothing and the answer is identical. That is the
# common case, and it is why turning `filter-crs` on costs this demo nothing.
SEARCH_CRS84=$(body_of "$SEARCH_BASE&$SPATIAL_FILTER&filter-crs=$CRS84_ENC")
expect_body_contains 'stac /search (filter-crs=CRS84)' "$SEARCH_CRS84" '"alpha"'
expect_body_lacks 'stac /search (filter-crs=CRS84)' "$SEARCH_CRS84" '"bravo"'
# A `next`-followable link must carry the parameter, or page two would read
# the same geometry in a different CRS than page one.
expect_body_contains 'stac /search (filter-crs=CRS84)' "$SEARCH_CRS84" 'filter-crs='

# ...and the axis-flipped CRS is refused by name rather than mis-read.
expect_status "$SEARCH_BASE&$SPATIAL_FILTER&filter-crs=$EPSG4326_ENC" 400

# The Item Search Filter class is folded per deployment as of `#248`. This
# driver filters, so it must still be declared alongside the CQL2 classes it
# binds — a fold that quietly dropped a class from an honest deployment would
# be its own overclaim in reverse.
STAC_CONFORMANCE=$(body_of "/public/stac/catalogs/default/conformance")
expect_body_contains 'stac /conformance' "$STAC_CONFORMANCE" 'api.stacspec.org/v1.0.0/item-search#filter'
expect_body_contains 'stac /conformance' "$STAC_CONFORMANCE" 'cql2/1.0/conf/basic-cql2'

# `#161`, the same "never asked for it" rule applied to the optional L2 tile
# cache: this config has no `cache:` block at all, so no L2 tier is configured
# and readiness has nothing to say about one. Byte-level, because a status-code
# check would pass even if readiness started volunteering a body. Phase 5 is
# the mirror image, with a tier configured and missing.
wait_ready
READY=$(curl -s -w '\n%{http_code}' "http://127.0.0.1:$SMOKE_PORT/readyz")
[ "$(printf '%s' "$READY" | tail -n 1)" = "200" ] || fail 'GET /readyz expected 200'
[ -z "$(printf '%s' "$READY" | sed '$d')" ] ||
  fail "GET /readyz must be empty with no cache.l2 configured: $READY"
ok '/readyz is an empty 200 with no cache.l2 configured'
READY_CT=$(curl -s -o /dev/null -w '%{content_type}' "http://127.0.0.1:$SMOKE_PORT/readyz")
[ -z "$READY_CT" ] ||
  fail "GET /readyz must send no Content-Type with no cache.l2 configured: $READY_CT"
ok '/readyz sends no Content-Type'
METRICS=$(body_of '/metrics')
expect_body_lacks '/metrics' "$METRICS" 'tile_cache_l2_available'

stop_server

# --- phase 2: the records lane, declared ------------------------------------

RECORDS_CONFIG=$(config_for records)
cat >"$RECORDS_CONFIG" <<'YAML'
control_store:
  backend: legacy_file
server:
  port: 18192
settings:
  protocols:
    records: enabled
storages:
  - id: main
    driver: geopackage
    url_env: TELLURION_SMOKE_GPKG
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: smoke_points
    catalog: default
    storage: main
  - id: smoke_records
    catalog: default
    storage: main
    kind: record
    settings:
      stac:
        license: CC-BY-4.0
        keywords: [thesaurus, registry]
YAML

printf 'phase 2: kind: record plus protocols.records: enabled\n'
start_server "$RECORDS_CONFIG"

DIR=$(body_of "/public")
expect_body_contains 'tenant directory' "$DIR" '/public/records/catalogs/default'

# The partition, both directions. A record collection is gone from the
# Features root's listing...
COLLECTIONS=$(body_of "/public/features/catalogs/default/collections")
expect_body_contains 'features /collections' "$COLLECTIONS" '"smoke_points"'
expect_body_lacks 'features /collections' "$COLLECTIONS" '"smoke_records"'

# ...and gone from its per-collection resources too, so the listing is not a
# lie a direct fetch contradicts.
expect_status '/public/features/catalogs/default/collections/smoke_records' 404
expect_status '/public/features/catalogs/default/collections/smoke_records/items' 404
expect_status '/public/tiles/catalogs/default/collections/smoke_records/tiles' 404

# ...while the vector collection is untouched on every root it always served.
expect_status '/public/features/catalogs/default/collections/smoke_points/items' 200

# The Records root serves the mirror image.
CATALOGS=$(body_of "/public/records/catalogs/default/collections")
expect_body_contains 'records /collections' "$CATALOGS" '"smoke_records"'
expect_body_lacks 'records /collections' "$CATALOGS" '"smoke_points"'
# Requirement 12 (/req/record-collection/itemType) and Requirement 16
# (/req/record-collection/links-records).
expect_body_contains 'records /collections' "$CATALOGS" '"itemType":"record"'
expect_body_contains 'records /collections' "$CATALOGS" '"rel":"items"'
# Declared metadata reaches the Records projection from the same canonical
# descriptor the STAC projection reads.
expect_body_contains 'records /collections' "$CATALOGS" 'CC-BY-4.0'
expect_body_contains 'records /collections' "$CATALOGS" 'thesaurus'

# A non-record collection is refused by name here, never served and never
# silently empty.
expect_status '/public/records/catalogs/default/collections/smoke_points' 404
expect_status '/public/records/catalogs/default/collections/smoke_points/items' 404

RECORDS=$(body_of "/public/records/catalogs/default/collections/smoke_records/items")
expect_body_contains 'records /items' "$RECORDS" '"FeatureCollection"'
expect_body_contains 'records /items' "$RECORDS" 'Hydrography thesaurus'
# The geometry-less content really is geometry-less on the wire.
expect_body_contains 'records /items' "$RECORDS" '"geometry":null'
# Requirement 8 (/req/record-core/links): each record links to its catalog.
expect_body_contains 'records /items' "$RECORDS" '"rel":"collection"'

RECORD=$(body_of "/public/records/catalogs/default/collections/smoke_records/items/1")
expect_body_contains 'records /items/1' "$RECORD" 'Hydrography thesaurus'
expect_body_contains 'records /items/1' "$RECORD" '"rel":"collection"'

# The anti-overclaim rule, executed: no Records conformance class is declared
# anywhere. See `tellurion_records::conformance` for the per-class reasoning.
CONFORMANCE=$(body_of "/public/records/catalogs/default/conformance")
expect_body_contains 'records /conformance' "$CONFORMANCE" 'ogcapi-common-1/1.0/conf/core'
expect_body_lacks 'records /conformance' "$CONFORMANCE" 'ogcapi-records-1'

expect_status '/public/records/catalogs/default/api' 200

# STAC serves every kind — a record collection stays describable.
STAC=$(body_of "/public/stac/catalogs/default/collections")
expect_body_contains 'stac /collections' "$STAC" '"smoke_records"'
expect_body_contains 'stac /collections' "$STAC" '"smoke_points"'

stop_server

# --- phase 3: the processes lane asked for, but not capable ------------------

# `#182`'s capability gate, executed against a booted server rather than
# asserted in a unit test. This config turns `protocols.processes` on AND
# declares `server.processes` pointing at the one storage this deployment has
# — a GeoPackage, which advertises no `JobStore`. So the operator has asked
# for the lane twice over and still gets no root, because there is nowhere
# durable to record a job.
#
# That is the whole rule: a deployment with no ledger capability does not get
# a half-working Processes root, it gets no root. A job accepted with nowhere
# to record it would be far worse than a 404.

PROCESSES_CONFIG=$(config_for processes)
cat >"$PROCESSES_CONFIG" <<'YAML'
control_store:
  backend: legacy_file
server:
  port: 18192
  processes:
    storage: main
settings:
  protocols:
    processes: enabled
storages:
  - id: main
    driver: geopackage
    url_env: TELLURION_SMOKE_GPKG
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: smoke_points
    catalog: default
    storage: main
YAML

printf 'phase 3: processes asked for, but no storage can hold a job ledger\n'
start_server "$PROCESSES_CONFIG"

# Boot did NOT fail — the rest of the deployment serves exactly as before.
expect_status '/public/features/catalogs/default/collections/smoke_points/items' 200

# But the refusal is named, not silent: the boot log says which storage could
# not provide a ledger.
grep -Fq 'no durable job ledger' "$WORK/server.log" ||
  fail 'the boot log must name the missing ledger capability'
ok 'the boot log names the missing job-ledger capability'

DIR=$(body_of "/public")
expect_body_lacks 'tenant directory' "$DIR" '/public/processes/catalogs/default'
expect_status '/public/processes/catalogs/default' 404
expect_status '/public/processes/catalogs/default/conformance' 404
expect_status '/public/processes/catalogs/default/api' 404
expect_status '/public/processes/catalogs/default/processes' 404
expect_status '/public/processes/catalogs/default/jobs/anything' 404

stop_server

# --- phase 4: optimistic locking asked for, but not honourable atomically ----

# `#150`'s capability gate, executed. This config gives `smoke_points` a real
# write lane on the one storage this deployment has — a GeoPackage, which has
# no per-row version it could compare INSIDE its write statement. So the
# `If-Match` guard cannot actually hold there: between the read the guard
# hashes and the write it protects, another writer can commit, and both
# writers' checks would already have passed.
#
# This proves the same rule phase 3 proves for the job ledger: a capability
# that cannot be honoured is neither advertised nor quietly approximated.
# `/conformance` must not name the ETags class, a conditional write must be
# refused BY NAME, and — the part that matters most — an ORDINARY write,
# carrying no precondition at all, must be completely untouched by any of it.

WRITE_CONFIG=$(config_for write)
cat >"$WRITE_CONFIG" <<'YAML'
control_store:
  backend: legacy_file
server:
  port: 18192
storages:
  - id: main
    driver: geopackage
    url_env: TELLURION_SMOKE_GPKG
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: smoke_points
    catalog: default
    storage: main
    routing: { write: main }
YAML

printf 'phase 4: optimistic locking asked for, but not honourable atomically\n'
start_server "$WRITE_CONFIG"

ITEM='/public/features/catalogs/default/collections/smoke_points/items/1'
FEATURE='{"type":"Feature","geometry":{"type":"Point","coordinates":[12.49,41.90]},"properties":{"name":"unconditional"}}'

# The anti-overclaim rule: no ETags class from a write lane that cannot
# evaluate the precondition atomically.
CONFORMANCE=$(body_of "/public/features/catalogs/default/conformance")
expect_body_lacks 'features /conformance' "$CONFORMANCE" 'optimistic-locking-etags'

# Rule 1, executed: a request carrying no precondition writes exactly as it
# always did. Nothing in this slice may touch it.
STATUS=$(curl -s -o /dev/null -w '%{http_code}' -X PUT \
  -H 'Content-Type: application/geo+json' --data "$FEATURE" \
  "http://127.0.0.1:$SMOKE_PORT$ITEM")
[ "$STATUS" = "204" ] ||
  fail "PUT with no precondition returned $STATUS, expected 204"
ok 'PUT with no precondition still succeeds unchanged'

# `#263`, the direction phase 1 does not cover: this deployment DOES offer a
# mutable resource, and the `PUT` immediately above is the proof, so Part 4's
# Create/Replace/Delete class must still be declared. Requirement 1 clause A
# asks for "one or more of the methods HTTP POST, PUT and/or DELETE for each
# mutable resource"; `smoke_points` has one, and folding the class out of the
# static list must cost a deployment that earns it nothing. Re-read after the
# write, so the assertion sits on the same side of the behaviour it describes.
EARNED=$(body_of "/public/features/catalogs/default/conformance")
expect_body_contains 'features /conformance' "$EARNED" \
  'ogcapi-features-4/1.0/conf/create-replace-delete'

# And the refusal is named, not silent — never a racy check pretending to be
# a guard, and never a header quietly ignored.
REFUSAL=$(curl -s -X PUT -H 'Content-Type: application/geo+json' \
  -H 'If-Match: "anything"' --data "$FEATURE" "http://127.0.0.1:$SMOKE_PORT$ITEM")
printf '%s' "$REFUSAL" | grep -Fq 'optimistic-locking' ||
  fail "a conditional write must be refused by name; got: $REFUSAL"
ok 'a conditional write against this lane is refused by name'

# The refused write left the stored feature exactly as the unconditional one
# had written it.
ITEM_BODY=$(body_of "$ITEM")
expect_body_contains 'refused conditional write' "$ITEM_BODY" '"unconditional"'

stop_server

# --- phase 5: an L2 tile cache configured, and not there ---------------------

# `#161`'s decision, executed rather than asserted. This config selects
# `cache.l2.backend: valkey` against a port nothing is listening on, so the
# backend is genuinely unreachable — the boot-down case.
#
# Three things must all hold at once, and they are exactly the three a unit
# test cannot prove together:
#
#   1. the server BOOTS. A cache tier being down must never be the reason a
#      replica cannot start.
#   2. it SERVES. Every request is still answered correctly from L1 plus the
#      origin storage, just without the cache in front.
#   3. readiness stays 200 and NAMES what is missing. Not-ready would evict a
#      still-correct replica from the load balancer at precisely the moment
#      the cache stopped absorbing load; a bare 200 would be a lie.
#
# This phase needs the `valkey` feature compiled in — the default binary
# refuses a `cache.l2` valkey selection at boot by name, which is its own
# (unchanged) contract.

printf 'phase 5: an L2 cache configured against a backend that is not there\n'
CARGO_PROFILE_DEV_DEBUG=0 cargo build --quiet \
  -p tellurion --features valkey >"$WORK/build-valkey.log" 2>&1 ||
  { cat "$WORK/build-valkey.log" >&2; fail 'cargo build --features valkey'; }

# This phase's config lives in its own directory, away from `server.log` — see
# `config_for`. `#161` isolated this phase first, because it is the one whose
# whole point is a readiness assertion; `#260` then found the same feedback
# loop failing a phase that had nothing to do with readiness, and gave every
# phase the same isolation through this one helper.
VALKEY_CONFIG=$(config_for valkey)
cat >"$VALKEY_CONFIG" <<'YAML'
control_store:
  backend: legacy_file
server:
  port: 18193
cache:
  memory_percent: 10.0
  l2:
    backend: valkey
    url_env: TELLURION_SMOKE_VALKEY_URL
    ttl_s: 60
storages:
  - id: main
    driver: geopackage
    url_env: TELLURION_SMOKE_GPKG
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: smoke_points
    catalog: default
    storage: main
YAML

# Its own port, so nothing this phase observes can have been answered by an
# earlier phase's server. Port 1 is reserved and nothing listens on it, so the
# Valkey connect cannot accidentally succeed on a developer machine that
# happens to be running one.
SMOKE_PORT=18193
export TELLURION_SMOKE_VALKEY_URL="redis://127.0.0.1:1"

# (1) it boots — `start_server` fails the script if it never turns healthy.
start_server "$VALKEY_CONFIG"
ok 'the server boots with a configured cache.l2 that is unreachable'

# (2) it serves.
expect_status '/public/features/catalogs/default/collections/smoke_points/items' 200
expect_status '/public/tiles/catalogs/default/collections/smoke_points/tiles/WebMercatorQuad/0/0/0' 200

# (3) readiness is 200 — the replica stays in the load balancer — and says
# which tier is missing rather than a generic "degraded". `wait_ready` is
# itself the first half of the assertion: readiness has to reach 200 at all
# with a configured cache that is not there.
wait_ready
READY=$(curl -s -w '\n%{http_code}' "http://127.0.0.1:$SMOKE_PORT/readyz")
READY_CODE=$(printf '%s' "$READY" | tail -n 1)
READY_BODY=$(printf '%s' "$READY" | sed '$d')
[ "$READY_CODE" = "200" ] ||
  fail "an unreachable cache must not make the process unready; got $READY_CODE"
ok '/readyz stays 200 with an unreachable cache.l2'
expect_body_contains '/readyz' "$READY_BODY" '"component":"cache.l2"'
expect_body_contains '/readyz' "$READY_BODY" '"backend":"valkey"'
expect_body_contains '/readyz' "$READY_BODY" '"reason":"never-connected-at-boot"'

# The operational metric, and the log line naming the backend.
METRICS=$(body_of '/metrics')
expect_body_contains '/metrics' "$METRICS" 'tile_cache_l2_available'
expect_body_contains '/metrics' "$METRICS" 'backend="valkey"'
grep -Fq 'cache.l2' "$WORK/server.log" ||
  fail 'the log must name the unavailable cache tier'
ok 'the log names the unavailable cache tier'

stop_server

# --- phase 6: a projected collection under a driver that cannot transform ----

# `#247`, executed. Two collections in one deployment, on one driver
# (GeoPackage: it filters, and it neither reprojects a response nor transforms
# a filter's spatial literals), differing only in storage CRS.
#
# OGC API - Features - Part 3 Requirement 7 (`/req/filter/filter-crs-wgs84`)
# says a `filter` sent WITHOUT a `filter-crs` has its geometries processed in
# CRS84. Against `smoke_points` (4326) that costs the driver nothing and has
# always worked. Against `smoke_mercator` (3857) it is a real coordinate
# transform this driver cannot perform, and the two things it could do instead
# are both forbidden: PostGIS's version of "hand it down anyway" is the
# mixed-SRID 500 this issue is named for, and this driver's version is a 200
# whose rows were selected by comparing degrees against metres.
#
# So it refuses BY NAME. The three assertions that matter are that the refusal
# happens, that it is narrow (a filter with no geometry in it is untouched, and
# so is an unfiltered request), and that the CRS84 collection beside it is
# completely unaffected — the last being the one an all-4326 gate like
# `italy-contract-smoke.sh` can never check, because it has no projected
# collection to contrast against.

PROJECTED_CONFIG=$(config_for projected)
cat >"$PROJECTED_CONFIG" <<'YAML'
control_store:
  backend: legacy_file
server:
  port: 18194
storages:
  - id: main
    driver: geopackage
    url_env: TELLURION_SMOKE_GPKG
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: smoke_points
    catalog: default
    storage: main
  - id: smoke_mercator
    catalog: default
    storage: main
YAML

printf 'phase 6: a projected collection under a driver that cannot transform a filter literal\n'
SMOKE_PORT=18194
start_server "$PROJECTED_CONFIG"

FEATURES='/public/features/catalogs/default/collections'
# `S_INTERSECTS(geom, BBOX(12,41.5,13,42.5))`, degrees, covering `alpha` — and
# carrying no `filter-crs` of any kind. This is the plainest conformant Part 3
# request there is.
SPATIAL='filter=S_INTERSECTS%28geom%2CBBOX%2812%2C41.5%2C13%2C42.5%29%29'

# The CRS84 collection: unchanged, and correct. Rule 1 for every deployment
# that exists today, executed before the refusal below rather than after, so a
# regression that refused everything cannot hide behind a passing 400.
#
# Asserted on the count and on the row the box excludes, not on the included
# row's name: phase 4 wrote over feature 1's `name` through the real write
# path, so its geometry (12.49, 41.90 — inside the box) is what this phase can
# still rely on. `bravo` at (9.19, 45.46) is outside, and a filter that had
# quietly stopped filtering would return both.
POINTS=$(body_of "$FEATURES/smoke_points/items?$SPATIAL")
expect_body_contains 'features /items (4326, spatial filter)' "$POINTS" '"numberReturned":1'
expect_body_lacks 'features /items (4326, spatial filter)' "$POINTS" '"bravo"'

# The projected collection: the same filter, refused by name.
expect_status "$FEATURES/smoke_mercator/items?$SPATIAL" 400
REFUSAL=$(curl -s "http://127.0.0.1:$SMOKE_PORT$FEATURES/smoke_mercator/items?$SPATIAL")
expect_body_contains 'spatial filter refusal' "$REFUSAL" 'CRS84'
expect_body_contains 'spatial filter refusal' "$REFUSAL" 'spatial filter'

# ...and the refusal is exactly that narrow. An unfiltered request, and a
# filter carrying no coordinates at all, are both served as before: this slice
# is about geometries, and it must not cost a projected deployment anything
# else it had.
expect_status "$FEATURES/smoke_mercator/items" 200
expect_status "$FEATURES/smoke_mercator/items?filter=geom%20IS%20NOT%20NULL" 200

# The STAC /search lane reaches the same conclusion for the same collection —
# the Filter Extension's own "filter-crs always defaults to CRS84" is the same
# statement Requirement 7 makes — and still serves the CRS84 one.
SEARCH_ROOT='/public/stac/catalogs/default/search'
expect_status "$SEARCH_ROOT?collections=smoke_mercator&$SPATIAL" 400
SEARCH_OK=$(body_of "$SEARCH_ROOT?collections=smoke_points&$SPATIAL")
expect_body_contains 'stac /search (4326, spatial filter)' "$SEARCH_OK" '"numberReturned":1'
expect_body_lacks 'stac /search (4326, spatial filter)' "$SEARCH_OK" '"bravo"'

stop_server

# --- phase 7: a registered style that paints nothing on the collection -------

# `#245`, the tiles half. The style registry is global — `tellurion-styles`'
# own doc: every root serves the same registry — but a MapLibre style document
# is not: `tellurion_render::resolve_layer_paints` keys every layer's paint by
# `source-layer`, so a style naming no layer this collection's tiles actually
# carry renders a blank tile. Until now the TileSet resource advertised one
# `map`-rel link per REGISTERED style, with no applicability check at all, so a
# deployment with three styles and eight collections published twenty-four
# links, most of them promising a picture that would come back empty. `#220`
# closed exactly this on the link-contributor side; this phase is the same rule
# on the resource that describes the tileset.
#
# Two styles are registered. `applies` targets `smoke_points`, which is the
# layer name this driver's tiles genuinely carry (the GeoPackage driver reports
# no `vector_layers` metadata, so the name is the collection's external id —
# `TileSource::vector_layers`' own documented fallback). `elsewhere` targets a
# source layer nothing here has.
#
# The distinction that matters, and the reason this is narrowing an
# ADVERTISEMENT rather than removing a capability: BOTH styled-map routes must
# still serve. Nothing is taken away from a client that already knows a style
# id; the resource simply stops recommending one that would draw nothing.

printf 'phase 7: a registered style that paints nothing on the collection\n'

mkdir -p "$WORK/styles"
cat >"$WORK/styles/applies.json" <<'JSON'
{
  "version": 8,
  "name": "Applies",
  "layers": [
    { "id": "pts", "type": "circle", "source-layer": "smoke_points",
      "paint": { "circle-color": "#cc3366", "circle-radius": 4 } }
  ]
}
JSON
cat >"$WORK/styles/elsewhere.json" <<'JSON'
{
  "version": 8,
  "name": "Elsewhere",
  "layers": [
    { "id": "other", "type": "fill", "source-layer": "some-other-collection",
      "paint": { "fill-color": "#3388ff" } }
  ]
}
JSON

# Unquoted heredoc: the style paths are absolute, into this run's temp dir.
# `FileStyleStore` reads them relative to the process's working directory, so
# an absolute path is what makes this phase independent of where the script
# was invoked from. The style documents live in `$WORK/styles/`, deliberately
# not in the config's own directory: they are read once at boot, and a file
# beside the config is a file the reload watch would report (`config_for`).
STYLES_CONFIG=$(config_for styles)
cat >"$STYLES_CONFIG" <<YAML
control_store:
  backend: legacy_file
server:
  port: 18195
storages:
  - id: main
    driver: geopackage
    url_env: TELLURION_SMOKE_GPKG
styles:
  - id: applies
    path: $WORK/styles/applies.json
  - id: elsewhere
    path: $WORK/styles/elsewhere.json
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: smoke_points
    catalog: default
    storage: main
YAML

SMOKE_PORT=18195
start_server "$STYLES_CONFIG"

# The registry is untouched: both styles are still registered, still listed,
# still individually retrievable. This slice changes what is RECOMMENDED for
# one collection, never what exists.
STYLES=$(body_of '/public/styles/catalogs/default/styles')
expect_body_contains 'styles /styles' "$STYLES" '"applies"'
expect_body_contains 'styles /styles' "$STYLES" '"elsewhere"'

TILESET=$(body_of '/public/tiles/catalogs/default/collections/smoke_points/tiles/WebMercatorQuad')
# The layer name applicability is checked against — the same one this resource
# advertises, which is what makes the two impossible to disagree about.
expect_body_contains 'tiles TileSet' "$TILESET" '"id":"smoke_points"'
expect_body_contains 'tiles TileSet' "$TILESET" '"title":"applies"'
expect_body_lacks 'tiles TileSet' "$TILESET" '"title":"elsewhere"'

# The advertised link is followable: substituting the placeholders and
# honouring the `type` the link itself declares (`image/png`) reaches a real
# styled tile. Sent as an `Accept` header rather than a `.png` suffix
# precisely because that is what a client reading the link's own `type` would
# do — a `map` link whose declared media type the route refuses would be its
# own kind of dangling promise.
MAP_TILES='/public/tiles/catalogs/default/collections/smoke_points/styles'
png_status_of() {
  curl -s -o /dev/null -w '%{http_code}' -H 'Accept: image/png' \
    "http://127.0.0.1:$SMOKE_PORT$1"
}
expect_png_status() {
  actual=$(png_status_of "$1")
  [ "$actual" = "$2" ] || fail "GET $1 (Accept: image/png) returned $actual, expected $2"
  ok "GET $1 (Accept: image/png) -> $2"
}
expect_png_status "$MAP_TILES/applies/map/tiles/WebMercatorQuad/0/0/0" 200

# ...and the un-advertised style's route is NOT gone. A capability is not
# withdrawn here; only the recommendation is.
expect_png_status "$MAP_TILES/elsewhere/map/tiles/WebMercatorQuad/0/0/0" 200

# A style id that was never registered is still the ordinary 404, so "not
# advertised" and "does not exist" stay distinguishable.
expect_png_status "$MAP_TILES/never-registered/map/tiles/WebMercatorQuad/0/0/0" 404

stop_server

# --- phase 8: the CRS a response is actually in, named on the wire -----------

# `#227`, executed over the real binaries. The same two-collection deployment
# phase 6 uses — GeoPackage, which cannot reproject a response
# (`FeatureSource::crs_capable` is `false`; its own module doc: items are
# "always emitted in the collection's own storage CRS, unchanged") — but the
# question here is the response header rather than the filter.
#
# `smoke_mercator` is stored in EPSG:3857, so every response it serves is in
# metres. Until this issue `Content-Crs` said CRS84 on all of them, which
# names degrees: a client that trusted the header — the only thing Part 2
# gives it to trust — plotted the data in the wrong place, with no error and
# nothing in the response contradicting it.
#
# So the server now stamps the truth, advertises the same truth in the
# collection's own `crs`/`storageCrs`, and refuses `crs=CRS84` BY NAME, since
# producing CRS84 from this collection is a coordinate transform this driver
# cannot perform. What a client loses is a wrong answer; what it gains is a
# 400 it can act on.
#
# `smoke_points` (4326) sits beside it in the same deployment, on the same
# driver and the same file, and must be untouched in every one of those
# respects — the side-by-side contrast an all-4326 gate like
# `italy-contract-smoke.sh` proves globally but has no projected collection
# to check against.

CONTENT_CRS_CONFIG=$(config_for content-crs)
cat >"$CONTENT_CRS_CONFIG" <<'YAML'
control_store:
  backend: legacy_file
server:
  port: 18196
storages:
  - id: main
    driver: geopackage
    url_env: TELLURION_SMOKE_GPKG
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: smoke_points
    catalog: default
    storage: main
  - id: smoke_mercator
    catalog: default
    storage: main
YAML

printf 'phase 8: the CRS a response is actually in, named on the wire\n'
SMOKE_PORT=18196
start_server "$CONTENT_CRS_CONFIG"

# Asserts one response header's exact value. Header names are matched
# case-insensitively (HTTP does not promise a case), and the CR every header
# line ends with is stripped before comparison.
expect_header() {
  actual=$(curl -s -o /dev/null -D - "http://127.0.0.1:$SMOKE_PORT$2" |
    tr -d '\r' | grep -i "^$3:" | head -n 1 | cut -d' ' -f2-)
  [ "$actual" = "$4" ] || fail "$1: $3 was '$actual', expected '$4'"
  ok "$1: $3 is '$4'"
}

FEATURES='/public/features/catalogs/default/collections'
CRS84_URI='http://www.opengis.net/def/crs/OGC/1.3/CRS84'
MERCATOR_URI='http://www.opengis.net/def/crs/EPSG/0/3857'
# Percent-encoded for a query string — the same two characters the Rust-side
# `percent_encode_crs_uri` handles.
CRS84_Q='http%3A%2F%2Fwww.opengis.net%2Fdef%2Fcrs%2FOGC%2F1.3%2FCRS84'
MERCATOR_Q='http%3A%2F%2Fwww.opengis.net%2Fdef%2Fcrs%2FEPSG%2F0%2F3857'

# Rule 1 first, so a regression that moved everything cannot hide behind the
# projected assertions below: the CRS84 collection is what it always was.
# Header, metadata, and an explicit CRS84 request, all unchanged.
expect_header 'features /items (4326)' "$FEATURES/smoke_points/items" \
  'content-crs' "<$CRS84_URI>"
expect_header 'features /items/{fid} (4326)' "$FEATURES/smoke_points/items/1" \
  'content-crs' "<$CRS84_URI>"
expect_status "$FEATURES/smoke_points/items?crs=$CRS84_Q" 200
POINTS_MD=$(body_of "$FEATURES/smoke_points")
expect_body_contains 'collection metadata (4326)' "$POINTS_MD" "\"crs\":[\"$CRS84_URI\"]"
expect_body_contains 'collection metadata (4326)' "$POINTS_MD" '"storageCrs":null'

# The projected collection: the header names the CRS the coordinates are
# genuinely in. This is the byte that moves, and the only kind of deployment
# it moves for.
expect_header 'features /items (3857)' "$FEATURES/smoke_mercator/items" \
  'content-crs' "<$MERCATOR_URI>"

# ...and the collection's own metadata says the same thing, so the header and
# the document a client read before choosing a CRS cannot disagree.
# `storageCrs` reappears here: `#217` had to omit it only because the `crs`
# list it must be a member of was CRS84-only.
MERCATOR_MD=$(body_of "$FEATURES/smoke_mercator")
expect_body_contains 'collection metadata (3857)' "$MERCATOR_MD" "\"crs\":[\"$MERCATOR_URI\"]"
expect_body_contains 'collection metadata (3857)' "$MERCATOR_MD" \
  "\"storageCrs\":\"$MERCATOR_URI\""
expect_body_lacks 'collection metadata (3857)' "$MERCATOR_MD" "$CRS84_URI"

# A client that requires CRS84 negotiates a 400 instead of mis-plotting, and
# the refusal names what this collection IS served in.
expect_status "$FEATURES/smoke_mercator/items?crs=$CRS84_Q" 400
CRS_REFUSAL=$(curl -s "http://127.0.0.1:$SMOKE_PORT$FEATURES/smoke_mercator/items?crs=$CRS84_Q")
expect_body_contains 'crs=CRS84 refusal' "$CRS_REFUSAL" "$MERCATOR_URI"

# The refusal is exactly that narrow: what the collection advertises, it
# serves — and asking for it by name gets the same header the default does.
expect_status "$FEATURES/smoke_mercator/items?crs=$MERCATOR_Q" 200
expect_header 'features /items?crs=<storage> (3857)' \
  "$FEATURES/smoke_mercator/items?crs=$MERCATOR_Q" 'content-crs' "<$MERCATOR_URI>"

stop_server

# --- phase 9: Allow must report live write capability, not URI shape --------

# `#208`, executed. OGC API - Features - Part 4 (OGC 20-002r1) Requirement 16
# clause C (`/req/create-replace-delete/options-response`): "The value of the
# `Allow` header SHALL be the list of methods that are allowed for the
# resource at the time and within the context of the request."
#
# This config gives two collections in ONE catalog, on ONE storage, with one
# single difference between them: `smoke_points` declares `routing.write`,
# `smoke_records` does not (there is no "defaults to the single storage"
# fallback for the write lane). Both are served by the same driver, under the
# same exposure matrix, so anything this phase observes is the write lane and
# nothing else.
#
# Every assertion below is a PAIR: read the `Allow`, then issue the method it
# named and check the two agree. Asserting the header alone would pass on a
# header that is confidently wrong — which is exactly what the old
# shape-derived `Allow` was on the read-only half.

ALLOW_CONFIG=$(config_for allow)
cat >"$ALLOW_CONFIG" <<'YAML'
control_store:
  backend: legacy_file
server:
  port: 18197
storages:
  - id: main
    driver: geopackage
    url_env: TELLURION_SMOKE_GPKG
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: smoke_points
    catalog: default
    storage: main
    routing: { write: main }
  - id: smoke_records
    catalog: default
    storage: main
YAML

printf 'phase 9: Allow reports live write capability, not URI shape\n'
SMOKE_PORT=18197
start_server "$ALLOW_CONFIG"

# Prints the `Allow` header value of a plain OPTIONS (no CORS preflight
# headers — a preflight is `cors`' business, not Part 4's).
allow_of() {
  curl -s -o /dev/null -D - -X OPTIONS "http://127.0.0.1:$SMOKE_PORT$1" |
    tr -d '\r' | sed -n 's/^[Aa]llow: //p'
}

expect_allow() {
  actual=$(allow_of "$1")
  [ "$actual" = "$2" ] || fail "OPTIONS $1 -> Allow: '$actual', expected '$2'"
  ok "OPTIONS $1 -> Allow: $2"
}

WRITABLE_ITEM='/public/features/catalogs/default/collections/smoke_points/items/1'
READONLY_ITEM='/public/features/catalogs/default/collections/smoke_records/items/1'
FEATURE='{"type":"Feature","geometry":{"type":"Point","coordinates":[12.49,41.90]},"properties":{"name":"advertised"}}'

put_status_of() {
  curl -s -o /dev/null -w '%{http_code}' -X PUT \
    -H 'Content-Type: application/geo+json' --data "$FEATURE" \
    "http://127.0.0.1:$SMOKE_PORT$1"
}

# Direction 1 — advertised, and accepted.
expect_allow "$WRITABLE_ITEM" 'GET, PUT, PATCH, DELETE, OPTIONS'
STATUS=$(put_status_of "$WRITABLE_ITEM")
[ "$STATUS" = "204" ] ||
  fail "Allow named PUT on $WRITABLE_ITEM, but a PUT returned $STATUS"
ok 'the PUT that Allow advertised is honoured'

# Direction 2 — not advertised, and refused. The write verbs are gone; `GET`
# is not, because this narrows the methods a resource offers, never the
# resource.
expect_allow "$READONLY_ITEM" 'GET, OPTIONS'
STATUS=$(put_status_of "$READONLY_ITEM")
[ "$STATUS" != "204" ] ||
  fail "Allow withheld PUT on $READONLY_ITEM, but a PUT succeeded"
ok 'the PUT that Allow withheld is refused'

# And refused BY NAME, naming the collection and the capability — never a
# bare status code.
REFUSAL=$(curl -s -X PUT -H 'Content-Type: application/geo+json' \
  --data "$FEATURE" "http://127.0.0.1:$SMOKE_PORT$READONLY_ITEM")
printf '%s' "$REFUSAL" | grep -Fq "does not support 'write'" ||
  fail "the refusal must name the missing capability; got: $REFUSAL"
ok 'the withheld PUT is refused by name'

# `#263`'s "whole deployment or per collection" question, executed on the one
# configuration that can answer it: ONE catalog holding a writable collection
# and a read-only one. The writable `PUT` above landed and the read-only one
# was refused, and the class stands — Requirement 1 clause A binds "for each
# mutable resource", and `smoke_records` was never offered as mutable, so it
# is not a resource the requirement quantifies over and must not narrow the
# claim. (A collection that IS offered as mutable but cannot write does narrow
# it; that half is pinned in `Router::create_replace_delete_conformance_classes`'s
# own unit tests, which can build a lane this config format cannot.)
MIXED_CONFORMANCE=$(body_of '/public/features/catalogs/default/conformance')
expect_body_contains 'features /conformance' "$MIXED_CONFORMANCE" \
  'ogcapi-features-4/1.0/conf/create-replace-delete'

# The read half of the very same URI is untouched — the reason `Allow` above
# still names `GET`.
expect_status "$READONLY_ITEM" 200

# The items-collection resource, same pair. `POST` is the write verb there.
expect_allow '/public/features/catalogs/default/collections/smoke_points/items' \
  'GET, POST, OPTIONS'
expect_allow '/public/features/catalogs/default/collections/smoke_records/items' \
  'GET, OPTIONS'

# The batch-ingest resource has no read representation at all, so a read-only
# collection's Allow there collapses to `OPTIONS` alone.
expect_allow '/public/features/catalogs/default/collections/smoke_points/items/batch' \
  'POST, OPTIONS'
expect_allow '/public/features/catalogs/default/collections/smoke_records/items/batch' \
  'OPTIONS'

# A collection id that resolves to nothing keeps answering exactly as it did
# before `#208`: it has no write capability to describe, and narrowing its
# `Allow` would make OPTIONS a collection-existence oracle.
expect_allow '/public/features/catalogs/default/collections/nope/items/1' \
  'GET, PUT, PATCH, DELETE, OPTIONS'

stop_server

# --- phase 10: a bbox with no bbox-crs on a projected collection ------------

# `#255`, executed. The same two collections phases 6 and 8 use, on the same
# GeoPackage driver — which filters, and neither reprojects a response nor
# transforms an input geometry.
#
# OGC API - Features - Part 1 Requirement 23 (`/req/core/fc-bbox-definition`)
# clause C: "If the bounding box consists of four numbers, the coordinate
# reference system of the values SHALL be interpreted as WGS 84
# longitude/latitude ... unless a different coordinate reference system is
# specified in a parameter `bbox-crs`" — restated by Part 2 Requirement 8
# (`/req/crs/fc-bbox-crs-valid-default-value`). So a bare `bbox` is CRS84,
# always, and against `smoke_mercator` (3857) reading it that way is a real
# coordinate transform this driver cannot do.
#
# The alternative it used to take is the worst outcome available: compare the
# four degrees against metre coordinates and answer `200`. That is a result set
# violating Requirement 24 (`/req/core/fc-bbox-response`) clause A, and no
# client can detect it — which is why a smoke test that only checked status
# codes could never have caught it, and why every assertion below pairs a
# status with something about the body.
#
# So it refuses BY NAME, on every lane that takes a bbox. What this phase
# proves is that the refusal happens on all of them, that it is narrow (no
# bbox, or a `bbox-crs` naming the collection's own storage CRS, are both
# served), and that `smoke_points` (4326) beside it is completely untouched.

BBOX_CRS_CONFIG=$(config_for bbox-crs)
cat >"$BBOX_CRS_CONFIG" <<'YAML'
control_store:
  backend: legacy_file
server:
  port: 18198
storages:
  - id: main
    driver: geopackage
    url_env: TELLURION_SMOKE_GPKG
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: smoke_points
    catalog: default
    storage: main
  - id: smoke_mercator
    catalog: default
    storage: main
YAML

printf 'phase 10: a bbox with no bbox-crs against a projected collection\n'
SMOKE_PORT=18198
start_server "$BBOX_CRS_CONFIG"

FEATURES='/public/features/catalogs/default/collections'
STAC='/public/stac/catalogs/default'
CRS84_URI='http://www.opengis.net/def/crs/OGC/1.3/CRS84'
MERCATOR_URI='http://www.opengis.net/def/crs/EPSG/0/3857'
CRS84_Q='http%3A%2F%2Fwww.opengis.net%2Fdef%2Fcrs%2FOGC%2F1.3%2FCRS84'
MERCATOR_Q='http%3A%2F%2Fwww.opengis.net%2Fdef%2Fcrs%2FEPSG%2F0%2F3857'

# Degrees around `bravo` (9.19, 45.46) and nowhere near `alpha` (12.49, 41.90)
# — so a bbox that had quietly stopped narrowing would return both rows and
# fail here, rather than passing on a status code alone. `alpha`'s NAME is
# deliberately not asserted on: phase 4 wrote over feature 1 through the real
# write path, and its geometry is all this phase relies on.
BBOX='bbox=9,45,10,46'

# Rule 1 first, before any refusal, so a regression that refused everything
# cannot hide behind a passing 400: the CRS84 collection answers the bare bbox
# exactly as it always did, and narrows to the one row the box contains.
POINTS=$(body_of "$FEATURES/smoke_points/items?$BBOX")
expect_body_contains 'features /items (4326, bbox)' "$POINTS" '"numberReturned":1'
expect_body_contains 'features /items (4326, bbox)' "$POINTS" '"bravo"'

# Part 2 Abstract Test 10 (`/conf/crs/bbox-crs-parameter-default`) on the wire:
# "send the same request, but with no `bbox-crs` parameter ... verify that the
# responses include the same features."
#
# Compared over the FEATURES, not the whole body: `links` is the last member of
# this response and its `self` href faithfully echoes whatever parameters the
# request carried (`params::items_href`), so the two bodies differ there BY
# DESIGN and must. Cutting from `"links":` is exact for this shape, and any
# difference in `type`/`numberMatched`/`numberReturned`/`features` still fails.
features_of() {
  printf '%s' "$1" | sed 's/"links":.*//'
}
POINTS_CRS84=$(body_of "$FEATURES/smoke_points/items?$BBOX&bbox-crs=$CRS84_Q")
[ "$(features_of "$POINTS_CRS84")" = "$(features_of "$POINTS")" ] ||
  fail 'an omitted bbox-crs and an explicit CRS84 one must return the same features'
ok 'features /items (4326): omitted bbox-crs == explicit CRS84'

# The projected collection: the same four numbers, refused by name — and the
# refusal names what it cannot do AND what the collection IS served in, so the
# client can send that `bbox-crs` instead of guessing.
expect_status "$FEATURES/smoke_mercator/items?$BBOX" 400
BBOX_REFUSAL=$(curl -s "http://127.0.0.1:$SMOKE_PORT$FEATURES/smoke_mercator/items?$BBOX")
expect_body_contains 'bbox refusal' "$BBOX_REFUSAL" 'CRS84'
expect_body_contains 'bbox refusal' "$BBOX_REFUSAL" 'bbox'
expect_body_contains 'bbox refusal' "$BBOX_REFUSAL" "$MERCATOR_URI"

# ...and the escape hatch that refusal points at actually works: `bbox-crs`
# naming this collection's own storage CRS needs no transform at all, is in the
# `crs` list phase 8 pinned, and is served.
expect_status "$FEATURES/smoke_mercator/items?$BBOX&bbox-crs=$MERCATOR_Q" 200

# The refusal is exactly that narrow. A request with no bbox is untouched...
expect_status "$FEATURES/smoke_mercator/items" 200
# ...and an explicit `bbox-crs=CRS84` is refused exactly as `#227` already made
# it, so this slice moved the DEFAULT into line with the explicit value rather
# than inventing a new verdict for it.
expect_status "$FEATURES/smoke_mercator/items?$BBOX&bbox-crs=$CRS84_Q" 400

# Both STAC lanes reach the same conclusion for the same collection. Neither
# has a `bbox-crs` parameter at all — a STAC bbox is WGS 84 lon/lat and nothing
# else — so the refusal is the only honest answer they have.
expect_status "$STAC/collections/smoke_mercator/items?$BBOX" 400
STAC_POINTS=$(body_of "$STAC/collections/smoke_points/items?$BBOX")
expect_body_contains 'stac /items (4326, bbox)' "$STAC_POINTS" '"bravo"'
expect_body_lacks 'stac /items (4326, bbox)' "$STAC_POINTS" '"numberReturned":2'

expect_status "$STAC/search?collections=smoke_mercator&$BBOX" 400
STAC_SEARCH=$(body_of "$STAC/search?collections=smoke_points&$BBOX")
expect_body_contains 'stac /search (4326, bbox)' "$STAC_SEARCH" '"bravo"'

# The cross-collection fan-out keeps its documented judgment call: the
# collection it cannot serve is skipped rather than failing the whole search,
# and the skip is machine-detectable in its OWN list. Naming it under
# `filterIncapableCollections` would send the client to drop a `filter` this
# request never sent.
FAN_OUT=$(body_of "$STAC/search?$BBOX")
expect_body_contains 'stac /search fan-out' "$FAN_OUT" '"bravo"'
expect_body_contains 'stac /search fan-out' "$FAN_OUT" \
  '"bboxIncapableCollections":["smoke_mercator"]'
expect_body_lacks 'stac /search fan-out' "$FAN_OUT" 'filterIncapableCollections'

stop_server

# --- phase 11: write-reactive tile invalidation (`#142`, `#141`) -------------
#
# Everything asserted here is asserted THROUGH THE TILE CACHE, which is the
# only reason it is a gate. `AppContext::fetch_mvt` caches an empty tile under
# the same key shape as a full one ("a genuinely empty tile is cached so
# repeat requests never re-hit the driver"), and that key carries the
# collection's per-bucket invalidation generation. So:
#
#   * if the write bumps the right bucket, the next fetch of the tile that
#     renders the feature builds a DIFFERENT key, misses, and re-renders;
#   * if it bumps the wrong bucket — or none — the key is unchanged, the
#     cached body comes straight back, and the response is a perfectly
#     ordinary 200/204 carrying pre-write content. No error, no log line,
#     forever. That is the defect, and it is what the fetches below catch.
#
# `smoke_mercator` is the `#142` fixture: the same EPSG:3857 table phase 6
# uses, written to with `Content-Crs` naming its own storage CRS, so the
# feature body — and therefore the outbox payload, verbatim — carries METRES.
# Read as CRS84 those metres clamp to the antimeridian and the Web Mercator
# latitude limit — the north-east corner of the grid, as far from Rome's own
# tile as the grid goes.
#
# `smoke_points` is the rule-1 control: a CRS84 collection, whose behaviour
# must be exactly what it always was — and the `#141` "update to a feature
# with no bbox memory" case, exercised as a real move between two tiles.

INVALIDATION_CONFIG=$(config_for invalidation)
cat >"$INVALIDATION_CONFIG" <<'YAML'
control_store:
  backend: legacy_file
server:
  port: 18199
  tile_invalidation:
    enabled: true
    poll_interval_ms: 100
    batch_size: 200
    bucket_zoom: 4
storages:
  - id: main
    driver: geopackage
    url_env: TELLURION_SMOKE_GPKG
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: smoke_mercator
    catalog: default
    storage: main
    routing: { write: main }
    tile_invalidation: true
    settings: { cache_ttl_s: 3600 }
  - id: smoke_points
    catalog: default
    storage: main
    routing: { write: main }
    tile_invalidation: true
    settings: { cache_ttl_s: 3600 }
YAML

printf 'phase 11: a write is visible in the very next tile fetch (#142, #141)\n'
SMOKE_PORT=18199
start_server "$INVALIDATION_CONFIG"

TILES='/public/tiles/catalogs/default/collections'
# OGC API - Tiles path order is {tileMatrix}/{tileRow}/{tileCol}: row 380,
# column 547 is the z=10 tile covering Rome (12.49E, 41.90N). Zoom 10 rather
# than something shallower on purpose — `smoke_points`' other feature
# (`bravo`, at Milan) shares Rome's tile all the way down to z=6, and the
# delete assertion below needs a tile that holds feature 1 and nothing else.
ROME_TILE='WebMercatorQuad/10/380/547.mvt'
# Where a CRS-blind read of EPSG:3857 metres lands: 1390330 as a longitude and
# 5146501 as a latitude both clamp, to the last column and the first row.
CLAMPED_TILE='WebMercatorQuad/10/0/1023.mvt'

# Polls a tile until it reports `$2`, up to ~10s — the consumer drains on its
# own 100ms cadence, so a fixed sleep would either be flaky or slow. Fails
# with the status it kept seeing, which is the whole diagnosis: a tile stuck
# on its pre-write status IS the stale-tile defect.
wait_for_tile_status() {
  i=0
  while [ "$i" -lt 100 ]; do
    actual=$(status_of "$1")
    [ "$actual" = "$2" ] && { ok "$3"; return 0; }
    i=$((i + 1))
    sleep 0.1
  done
  fail "$3: $1 stayed at $actual, never became $2 (a stale tile is exactly this)"
}

# --- `#142`: a projected-CRS write must reach the tile that renders it ------

# The projected collection has no rows, so its tile over Rome is empty — and
# now cached as empty, which is what a broken invalidation would keep serving.
expect_status "$TILES/smoke_mercator/tiles/$ROME_TILE" 204
expect_status "$TILES/smoke_mercator/tiles/$CLAMPED_TILE" 204

MERCATOR_ITEM='/public/features/catalogs/default/collections/smoke_mercator/items/1'
# 1390330, 5146501 is Rome in EPSG:3857 metres. The header says so; the body
# is in that CRS, and the outbox stores it verbatim.
STATUS=$(curl -s -o /dev/null -w '%{http_code}' -X PUT \
  -H 'Content-Type: application/geo+json' \
  -H 'Content-Crs: <http://www.opengis.net/def/crs/EPSG/0/3857>' \
  --data '{"type":"Feature","geometry":{"type":"Point","coordinates":[1390330,5146501]},"properties":{"name":"rome"}}' \
  "http://127.0.0.1:$SMOKE_PORT$MERCATOR_ITEM")
[ "$STATUS" = "204" ] || fail "the projected-CRS PUT returned $STATUS, expected 204"
ok 'a PUT declaring the collection storage CRS is accepted'

# The write really did land where the header said. Read back with no `crs`
# parameter, because `#227` is in force for exactly this collection: its read
# lane cannot reproject, so it serves — and truthfully labels — EPSG:3857, and
# a request for CRS84 is refused (phase 8 asserts that half). So the metres
# come back as metres, which is the strongest possible confirmation that the
# `Content-Crs` header was honoured rather than ignored.
BACK=$(body_of "$MERCATOR_ITEM")
expect_body_contains 'the projected write, read back' "$BACK" '1390330'

# The gate. Without `#142` this tile keeps answering 204 from the cache: the
# bucket that was invalidated is the clamped corner, not this one.
wait_for_tile_status "$TILES/smoke_mercator/tiles/$ROME_TILE" 200 \
  'the tile that renders a projected-CRS write shows it on the next fetch'

# And the corner tile a CRS-blind read would have bumped still has nothing in
# it. Over HTTP this can only ever confirm the rendering, not the bucket set —
# an empty tile is empty whether or not its generation moved — so the
# narrowness half is pinned where it IS observable, in
# `tellurion-postgis/tests/invalidation_live.rs`, which reads the per-bucket
# generations directly.
expect_status "$TILES/smoke_mercator/tiles/$CLAMPED_TILE" 204

# --- `#141`: a delete must remove the feature from the tile it occupied -----
#
# Deliberately on the collection this phase created itself, so nothing here
# destroys state a later phase might rely on. It also stacks both issues in
# one assertion: the prior extent recorded here is read off a row stored in
# METRES and converted to CRS84 by the storage, which is `#141` standing on
# `#142`.

MERCATOR_TILE_NOW=$(status_of "$TILES/smoke_mercator/tiles/$ROME_TILE")
[ "$MERCATOR_TILE_NOW" = "200" ] || fail 'the projected tile should still render the feature'

STATUS=$(curl -s -o /dev/null -w '%{http_code}' -X DELETE \
  "http://127.0.0.1:$SMOKE_PORT$MERCATOR_ITEM")
[ "$STATUS" = "204" ] || fail "DELETE $MERCATOR_ITEM returned $STATUS, expected 204"
ok 'the projected feature is deleted'

# The gate. A `Delete` obligation carries no geometry at all, so without the
# prior extent its own write transaction recorded, nothing here could know
# which bucket to invalidate — and this tile would go on rendering a feature
# that no longer exists in storage.
wait_for_tile_status "$TILES/smoke_mercator/tiles/$ROME_TILE" 204 \
  'the deleted feature disappears from the tile it used to occupy'

# The feature really is gone from storage too, not merely from a rendering.
expect_status "$MERCATOR_ITEM" 404

# --- `#141` again, and rule 1's control: a CRS84 collection, a MOVE ---------
#
# `smoke_points` feature 1 sits at Rome (phases 4 and 9 both rewrote it there
# through the real write path). Moving it to Naples must invalidate BOTH
# tiles: the one it left and the one it arrived in. The first of those is the
# half that has no source other than the prior extent this write recorded —
# the outbox payload only ever carries where the feature is going.
#
# And it is the rule-1 control at the same time: this collection is CRS84, so
# nothing about `#142` may change what it does.
NAPLES_TILE='WebMercatorQuad/10/384/552.mvt'
expect_status "$TILES/smoke_points/tiles/$ROME_TILE" 200
expect_status "$TILES/smoke_points/tiles/$NAPLES_TILE" 204

POINTS_ITEM='/public/features/catalogs/default/collections/smoke_points/items/1'
STATUS=$(curl -s -o /dev/null -w '%{http_code}' -X PUT \
  -H 'Content-Type: application/geo+json' \
  --data '{"type":"Feature","geometry":{"type":"Point","coordinates":[14.27,40.85]},"properties":{"name":"alpha"}}' \
  "http://127.0.0.1:$SMOKE_PORT$POINTS_ITEM")
[ "$STATUS" = "204" ] || fail "moving the CRS84 feature returned $STATUS, expected 204"
ok 'the CRS84 feature is moved'

wait_for_tile_status "$TILES/smoke_points/tiles/$NAPLES_TILE" 200 \
  'the moved feature appears in the tile it arrived in'
wait_for_tile_status "$TILES/smoke_points/tiles/$ROME_TILE" 204 \
  'and disappears from the tile it left'

# Put it back. This phase runs in numeric order, between 10 and 12, and phase
# 12 asserts `S_INTERSECTS(geom, BBOX(12,41.5,13,42.5))` returns exactly
# feature 1 — so a phase that moved it to Naples and walked away would break a
# later phase through the shared `.gpkg`, which is precisely the kind of
# cross-fixture coupling these scripts exist to keep out. It is also the same
# assertion run in reverse, and the reverse is not free: getting here needs the
# prior extent recorded a second time, for a feature this consumer has now
# invalidated once already.
STATUS=$(curl -s -o /dev/null -w '%{http_code}' -X PUT \
  -H 'Content-Type: application/geo+json' \
  --data '{"type":"Feature","geometry":{"type":"Point","coordinates":[12.49,41.90]},"properties":{"name":"alpha"}}' \
  "http://127.0.0.1:$SMOKE_PORT$POINTS_ITEM")
[ "$STATUS" = "204" ] || fail "restoring the CRS84 feature returned $STATUS, expected 204"
ok 'the CRS84 feature is moved back'

wait_for_tile_status "$TILES/smoke_points/tiles/$ROME_TILE" 200 \
  'the restored feature reappears in the tile it came back to'
wait_for_tile_status "$TILES/smoke_points/tiles/$NAPLES_TILE" 204 \
  'and leaves the one it was briefly in — the fixture is exactly as phase 10 left it'

stop_server

# --- phase 12: a conformance class defined in terms of a form not compiled ---

# `#134`, executed. The GeoPackage driver compiles `S_INTERSECTS` only in a
# restricted positional form — at most one per filter, never beneath
# `OR`/`NOT` — because the R*Tree bbox pre-filter it ANDs into the SQL is a
# sound narrowing only while `AND` is the only thing narrowing.
#
# CQL2 (OGC 21-065r2) defines `basic-spatial-functions` in terms of the
# general form. The class names Basic CQL2 as its Dependency, and Basic
# CQL2's Requirement 1 (`/req/basic-cql2/cql2-filter`) requires "a CQL2 filter
# expression composed of a logically connected series of one or more
# predicates as described by the BNF rule `booleanExpression` ... with the
# exception that the rules ... `spatialPredicate` ... do not have to be
# supported" — declaring the class is exactly what removes `spatialPredicate`
# from that exception list. The class's own two permissions narrow *operands*
# and *literal types*, never position. And its normative Abstract Test Suite
# is explicit: Conformance Test 26 (`/conf/basic-spatial-functions/test-data`)
# asserts exact item counts for `S_INTERSECTS(...) and S_INTERSECTS(...)`,
# `S_INTERSECTS(...) and not S_INTERSECTS(...)` and `S_INTERSECTS(...) or
# S_INTERSECTS(...)`; Conformance Test 27
# (`/conf/basic-spatial-functions/logical`) composes the stored spatial
# predicates under `NOT`/`AND`/`OR` together.
#
# So the class is withheld. What this phase proves is the *pairing*: the three
# general-form compositions are refused BY NAME, the restricted form still
# works, and the class is absent from both the root's `/conformance` and the
# collection document. Either half checked alone would pass on a list that is
# confidently wrong; together they cannot drift apart.

SPATIAL_CLASS_CONFIG=$(config_for spatial-class)
cat >"$SPATIAL_CLASS_CONFIG" <<'YAML'
control_store:
  backend: legacy_file
server:
  port: 18200
storages:
  - id: main
    driver: geopackage
    url_env: TELLURION_SMOKE_GPKG
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: smoke_points
    catalog: default
    storage: main
YAML

printf 'phase 12: basic-spatial-functions is declared only if the general form works\n'
SMOKE_PORT=18200
start_server "$SPATIAL_CLASS_CONFIG"

POINTS_ITEMS='/public/features/catalogs/default/collections/smoke_points/items'
# Two boxes, each covering exactly one of the fixture's two points:
#   A = BBOX(12,41.5,13,42.5)  -> feature 1 (12.49, 41.90)
#   B = BBOX(9,45,10,46)       -> feature 2, `bravo` (9.19, 45.46)
# Asserted on the count and on `bravo`, never on feature 1's `name`: phase 4
# wrote over that through the real write path, and only its geometry is
# something this phase can still rely on.
BOX_A='BBOX%2812%2C41.5%2C13%2C42.5%29'
BOX_B='BBOX%289%2C45%2C10%2C46%29'

# The restricted form: one predicate, AND-position. Untouched by this slice,
# and asserted FIRST so a regression that refused every spatial filter could
# not pass by making the general form "consistently unsupported".
RESTRICTED=$(body_of "$POINTS_ITEMS?filter=S_INTERSECTS%28geom%2C$BOX_A%29")
expect_body_contains 'features /items (restricted S_INTERSECTS)' "$RESTRICTED" '"numberReturned":1'
expect_body_lacks 'features /items (restricted S_INTERSECTS)' "$RESTRICTED" '"bravo"'
# Still AND-position when composed with a non-spatial predicate.
AND_SCALAR=$(body_of "$POINTS_ITEMS?filter=name%20IS%20NOT%20NULL%20AND%20S_INTERSECTS%28geom%2C$BOX_B%29")
expect_body_contains 'features /items (scalar AND S_INTERSECTS)' "$AND_SCALAR" '"numberReturned":1'
expect_body_contains 'features /items (scalar AND S_INTERSECTS)' "$AND_SCALAR" '"bravo"'

# The general form, in Conformance Test 26's own three shapes. Each must be a
# 400 that names the construct — never a 200 answered off the coarse bbox
# candidate set, which for the `OR` shape would be silently wrong.
for composition in \
  "S_INTERSECTS%28geom%2C$BOX_A%29%20AND%20S_INTERSECTS%28geom%2C$BOX_B%29" \
  "S_INTERSECTS%28geom%2C$BOX_A%29%20AND%20NOT%20S_INTERSECTS%28geom%2C$BOX_B%29" \
  "S_INTERSECTS%28geom%2C$BOX_A%29%20OR%20S_INTERSECTS%28geom%2C$BOX_B%29"; do
  expect_status "$POINTS_ITEMS?filter=$composition" 400
  REFUSAL=$(curl -s "http://127.0.0.1:$SMOKE_PORT$POINTS_ITEMS?filter=$composition")
  expect_body_contains 'general-form refusal' "$REFUSAL" 'S_INTERSECTS'
done

# ...so the class is not declared, on either root that folds the driver's own
# declared CQL2 set in.
SPATIAL_CONFORMANCE=$(body_of '/public/features/catalogs/default/conformance')
expect_body_lacks 'features /conformance' "$SPATIAL_CONFORMANCE" \
  'cql2/1.0/conf/basic-spatial-functions'
STAC_SPATIAL_CONFORMANCE=$(body_of '/public/stac/catalogs/default/conformance')
expect_body_lacks 'stac /conformance' "$STAC_SPATIAL_CONFORMANCE" \
  'cql2/1.0/conf/basic-spatial-functions'

# And the narrowing stops there. `basic-cql2` in particular is untouched:
# Requirement 1 excepts `spatialPredicate` by name, so a restricted
# `S_INTERSECTS` is exactly what that exception permits. The Part 3 filtering
# classes describe the `filter` parameter itself, not the expression language,
# and this driver still filters — so they stay too.
for kept in \
  'cql2/1.0/conf/basic-cql2' \
  'cql2/1.0/conf/cql2-text' \
  'cql2/1.0/conf/cql2-json' \
  'cql2/1.0/conf/advanced-comparison-operators' \
  'cql2/1.0/conf/temporal-functions' \
  'ogcapi-features-3/1.0/conf/filter' \
  'ogcapi-features-3/1.0/conf/features-filter'; do
  expect_body_contains 'features /conformance' "$SPATIAL_CONFORMANCE" "$kept"
done

# The per-collection surface (`#105`) resolves independently of the root fold.
# A narrowing that reached only one of them would leave a client reading the
# collection document a claim the root had already withdrawn.
POINTS_DOC=$(body_of '/public/features/catalogs/default/collections/smoke_points')
expect_body_contains 'collection cql2ConformanceClasses' "$POINTS_DOC" 'cql2ConformanceClasses'
expect_body_lacks 'collection cql2ConformanceClasses' "$POINTS_DOC" \
  'cql2/1.0/conf/basic-spatial-functions'
expect_body_contains 'collection cql2ConformanceClasses' "$POINTS_DOC" 'cql2/1.0/conf/basic-cql2'

stop_server

# --- phase 14: the modified-column touch trigger is opt-in and PostGIS-only --
#
# `#151`. `req/optimistic-locking-timestamps` is gated on an operator-declared
# `modified_column` (`#149`), and nothing in this workspace writes that column
# — so `tellurion-ingest locking install-touch-trigger` can provision the
# standard `BEFORE INSERT OR UPDATE ... SET <column> = now()` trigger next to
# the declaration.
#
# This script has no database service (see the header), which is exactly the
# right place to prove the two halves that must hold WITHOUT one:
#
#   * every driver but PostGIS is refused BY NAME. This whole script's storage
#     is a GeoPackage, so the refusal is not hypothetical here — it is the only
#     answer this deployment can ever get, which is also why no assertion below
#     has to go looking for a trigger in the `.gpkg` file: there is no code
#     path that could have put one there.
#   * a deployment that does not ask for the trigger is byte-for-byte what it
#     was. Asserted against a booted server rather than inferred: nothing new
#     is advertised, and the Timestamps class stays absent because no
#     collection here declares a `modified_column` at all.
#
# The PostGIS half is proved by `--dry-run`, which prints the DDL without
# connecting to anything. The config below names an environment variable that
# is deliberately never set, so a command that reached for a database would
# fail — succeeding is the proof it never did.

printf 'phase 14: the modified-column touch trigger is opt-in and PostGIS-only\n'

TOUCH_CONFIG=$(config_for touch)
cat >"$TOUCH_CONFIG" <<'YAML'
control_store:
  backend: legacy_file
server:
  port: 18202
storages:
  - id: main
    driver: geopackage
    url_env: TELLURION_SMOKE_GPKG
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: smoke_points
    catalog: default
    storage: main
YAML

# A GeoPackage collection. SQLite has triggers, but a different dialect and
# different semantics, and `#151` says to keep this PostGIS-only until the
# demand for the SQLite form is real — so this is a named refusal, never a
# silent skip and never an unproved dialect.
if TOUCH_OUT=$("$INGEST" locking install-touch-trigger \
  --config "$TOUCH_CONFIG" --collection smoke_points 2>&1); then
  fail 'locking install-touch-trigger must refuse a geopackage collection'
fi
printf '%s' "$TOUCH_OUT" | grep -Fq "driver 'geopackage'" ||
  fail "the geopackage refusal must name the driver: $TOUCH_OUT"
ok 'locking install-touch-trigger refuses a geopackage collection, naming the driver'
printf '%s' "$TOUCH_OUT" | grep -Fq "'postgis'" ||
  fail "the geopackage refusal must name the driver it does support: $TOUCH_OUT"
ok 'the refusal names postgis as the driver it does support'

# A collection that declares no `modified_column` has nothing to maintain, and
# this command never invents one — there is no `--column` flag, and no
# derivation. The declaration is the only source of truth.
TOUCH_PG_CONFIG=$(config_for touch-pg)
cat >"$TOUCH_PG_CONFIG" <<'YAML'
control_store:
  backend: legacy_file
server:
  port: 18202
storages:
  - id: pg
    driver: postgis
    url_env: TELLURION_SMOKE_UNSET_DATABASE_URL
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: undeclared
    catalog: default
    storage: pg
  - id: declared
    catalog: default
    storage: pg
    table: pg_points
    modified_column: modified
YAML

if TOUCH_OUT=$("$INGEST" locking install-touch-trigger \
  --config "$TOUCH_PG_CONFIG" --collection undeclared 2>&1); then
  fail 'locking install-touch-trigger must refuse a collection with no modified_column'
fi
printf '%s' "$TOUCH_OUT" | grep -Fq 'declares no modified_column' ||
  fail "the undeclared refusal must say so: $TOUCH_OUT"
ok 'locking install-touch-trigger refuses a collection that declares no modified_column'

# The PostGIS DDL itself, printed without a database. Every property asserted
# here is one the trigger's correctness rests on, and each would read as an
# arbitrary choice on its own:
#   * the INSERT arm stamps a row created by `PUT` against a new id, which an
#     `UPDATE`-only trigger would leave with a NULL the server cannot parse;
#   * no `WHEN` guard, so the column moves on exactly the row versions `#150`'s
#     `xmin` witness also moves on — a guard would make the two disagree;
#   * `now()`, not `clock_timestamp()`, so one transaction stamps one value;
#   * `CREATE OR REPLACE` throughout and no `DROP`, so a rerun never leaves the
#     table momentarily untriggered.
TOUCH_DDL=$("$INGEST" locking install-touch-trigger \
  --config "$TOUCH_PG_CONFIG" --collection declared --dry-run 2>&1) ||
  fail "locking install-touch-trigger --dry-run must not need a database: $TOUCH_DDL"
ok 'locking install-touch-trigger --dry-run prints the DDL with no database at all'
expect_body_contains 'touch DDL' "$TOUCH_DDL" 'BEFORE INSERT OR UPDATE ON "pg_points"'
expect_body_contains 'touch DDL' "$TOUCH_DDL" 'NEW."modified" := now();'
expect_body_contains 'touch DDL' "$TOUCH_DDL" 'CREATE OR REPLACE FUNCTION "pg_points_modified_touch"()'
expect_body_contains 'touch DDL' "$TOUCH_DDL" 'CREATE OR REPLACE TRIGGER "pg_points_modified_touch_trg"'
expect_body_contains 'touch DDL' "$TOUCH_DDL" 'tellurion:modified-column-touch'
expect_body_lacks 'touch DDL' "$TOUCH_DDL" 'WHEN ('
expect_body_lacks 'touch DDL' "$TOUCH_DDL" 'clock_timestamp'
expect_body_lacks 'touch DDL' "$TOUCH_DDL" 'DROP'

# And the deployment itself, booted, is untouched. No collection here declares
# a `modified_column`, so the Timestamps class is absent exactly as it was —
# this command exists, and the served contract does not know it.
SMOKE_PORT=18202
start_server "$TOUCH_CONFIG"

TOUCH_CONFORMANCE=$(body_of '/public/features/catalogs/default/conformance')
expect_body_lacks 'features /conformance' "$TOUCH_CONFORMANCE" 'optimistic-locking-timestamps'
TOUCH_DOC=$(body_of '/public/features/catalogs/default/collections/smoke_points')
expect_body_lacks 'collection smoke_points' "$TOUCH_DOC" 'optimistic-locking-timestamps'
# Still serving, so the absence above is an absence and not an outage.
TOUCH_ITEMS=$(body_of '/public/features/catalogs/default/collections/smoke_points/items')
expect_body_contains 'features /items' "$TOUCH_ITEMS" '"FeatureCollection"'
# `Last-Modified` is read from a declared column and never fabricated, so a
# collection with no declaration emits none — with or without a trigger
# anywhere in the world.
TOUCH_HEADERS=$(curl -s -o /dev/null -D - \
  "http://127.0.0.1:$SMOKE_PORT/public/features/catalogs/default/collections/smoke_points/items/1")
printf '%s' "$TOUCH_HEADERS" | grep -iq '^last-modified:' &&
  fail 'a collection declaring no modified_column must emit no Last-Modified'
ok 'no Last-Modified on a collection that declares no modified_column'

stop_server

# --- phase 18: a bearer principal whose token lives in the environment -------
#
# `#144`. The credential-storage seam, end to end against a real process:
# `auth.bearer_tokens[].token_env` names an environment variable, the document
# never holds the value, and the value still authorizes.
#
# What this phase is for that the unit tests are not: a real server writing a
# real log. The claim "a credential moved out of the document does not come
# back out anywhere" is only worth as much as the surfaces it was checked
# against, and stdout of a booted process is one no in-process test observes.
# The same run also proves the LOUD path -- a deployment that has not moved
# its credentials yet still boots, still authorizes, and says so by name.
#
# One secret value is used throughout, distinctive enough that finding it
# anywhere is unambiguous.

# `curl` with an `Authorization: Bearer` header -- the first authenticated
# assertions in this script, so the two helpers live here with the phase that
# needs them rather than in the shared helper block above.
bearer_status_of() {
  curl -s -o /dev/null -w '%{http_code}' \
    -H "Authorization: Bearer $2" "http://127.0.0.1:$SMOKE_PORT$1"
}

bearer_body_of() {
  out=$(curl -s -w '\n%{http_code}' -H "Authorization: Bearer $2" \
    "http://127.0.0.1:$SMOKE_PORT$1")
  code=$(printf '%s' "$out" | tail -n 1)
  [ "$code" = "200" ] || fail "GET $1 (with credential) returned $code, expected 200"
  printf '%s' "$out" | sed '$d'
}

expect_bearer_status() {
  actual=$(bearer_status_of "$1" "$2")
  [ "$actual" = "$3" ] || fail "GET $1 (bearer $4) returned $actual, expected $3"
  ok "GET $1 (bearer $4) -> $3"
}

SEAM_SECRET='s3cret-smoke-token-from-the-environment'
SEAM_TENANT_COLLECTIONS='/public/features/catalogs/default/collections'

SEAM_ENV_CONFIG=$(config_for credential-seam-env)
cat >"$SEAM_ENV_CONFIG" <<'YAML'
control_store:
  backend: legacy_file
server:
  port: 18206
storages:
  - id: main
    driver: geopackage
    url_env: TELLURION_SMOKE_GPKG
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: smoke_points
    catalog: default
    storage: main
auth:
  bearer_tokens:
    - token_env: TELLURION_SMOKE_BEARER
      tenants: [public]
      platform_admin: true
      principal: smoke-service-account
YAML

printf 'phase 18: a bearer token read from the environment, and never echoed\n'
SMOKE_PORT=18206
export TELLURION_SMOKE_BEARER="$SEAM_SECRET"
start_server "$SEAM_ENV_CONFIG"

# Auth is configured, so the tenant lane is closed without a credential --
# proof the principal was really built, not quietly dropped for want of a
# `token:` line.
expect_status "$SEAM_TENANT_COLLECTIONS" 401
# ...and the environment's value IS the credential.
expect_bearer_status "$SEAM_TENANT_COLLECTIONS" "$SEAM_SECRET" 200 'the value'
# The variable NAME is not. A deployment that mixed the two up must be told,
# not admitted.
expect_bearer_status "$SEAM_TENANT_COLLECTIONS" 'TELLURION_SMOKE_BEARER' 403 'the name'

# The one endpoint that echoes the whole raw document back. It returns what
# the document says, by contract -- so the fix is a document that does not say
# it, and this is where that is proved rather than asserted.
SEAM_RAW_CONFIG=$(bearer_body_of '/config' "$SEAM_SECRET")
expect_body_lacks 'GET /config' "$SEAM_RAW_CONFIG" "$SEAM_SECRET"
expect_body_contains 'GET /config' "$SEAM_RAW_CONFIG" 'TELLURION_SMOKE_BEARER'
expect_body_contains 'GET /config' "$SEAM_RAW_CONFIG" 'smoke-service-account'

# The unauthenticated configuration views, which any reader of this port can
# fetch.
SEAM_EFFECTIVE=$(body_of '/config/effective')
expect_body_lacks 'GET /config/effective' "$SEAM_EFFECTIVE" "$SEAM_SECRET"
SEAM_PROFILES=$(body_of '/config/profiles')
expect_body_lacks 'GET /config/profiles' "$SEAM_PROFILES" "$SEAM_SECRET"

# And a 401/403 must not narrate the credential it refused either.
SEAM_DENIED=$(curl -s -H "Authorization: Bearer wrong-$SEAM_SECRET" \
  "http://127.0.0.1:$SMOKE_PORT/config")
expect_body_lacks 'a refused /config request' "$SEAM_DENIED" "$SEAM_SECRET"

# The log of a process that booted with this credential, authenticated three
# requests with it and refused a fourth.
expect_body_lacks 'server log' "$(cat "$WORK/server.log")" "$SEAM_SECRET"
# Nothing to deprecate here, so nothing is said.
expect_body_lacks 'server log' "$(cat "$WORK/server.log")" 'carry an inline'

stop_server

# The pre-`#144` deployment, unchanged: an inline token still boots, still
# authorizes, and is named in the log every time -- loud, and not a refusal.
SEAM_INLINE_CONFIG=$(config_for credential-seam-inline)
cat >"$SEAM_INLINE_CONFIG" <<YAML
control_store:
  backend: legacy_file
server:
  port: 18206
storages:
  - id: main
    driver: geopackage
    url_env: TELLURION_SMOKE_GPKG
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: smoke_points
    catalog: default
    storage: main
auth:
  bearer_tokens:
    - token: $SEAM_SECRET
      tenants: [public]
      principal: legacy-service-account
YAML

start_server "$SEAM_INLINE_CONFIG"
expect_bearer_status "$SEAM_TENANT_COLLECTIONS" "$SEAM_SECRET" 200 'the inline value'
SEAM_INLINE_LOG=$(cat "$WORK/server.log")
expect_body_contains 'server log' "$SEAM_INLINE_LOG" 'auth.bearer_tokens'
expect_body_contains 'server log' "$SEAM_INLINE_LOG" 'token_env'
expect_body_contains 'server log' "$SEAM_INLINE_LOG" '(#144)'
expect_body_contains 'server log' "$SEAM_INLINE_LOG" 'legacy-service-account'
# Named by principal, never by value -- a deprecation notice that printed the
# credential would be the leak it exists to close.
expect_body_lacks 'server log' "$SEAM_INLINE_LOG" "$SEAM_SECRET"
stop_server

# A `token_env` naming a variable that is not set refuses to boot, by name.
# Not a server that starts with one principal quietly missing: that is the
# failure an operator diagnoses for an hour as a revoked credential.
unset TELLURION_SMOKE_BEARER
SEAM_BOOT_LOG="$WORK/credential-seam-boot.log"
# Bounded, and the bound is checked rather than accepted: a regression that
# let this BOOT would otherwise leave the server serving forever and hang the
# whole script, while a bare `timeout` would turn that hang into a passing
# assertion (exit 124 is non-zero, which is exactly what "refused" looks
# like). So all three outcomes are named separately.
#
# `|| SEAM_BOOT_STATUS=$?` rather than a bare call plus `$?`: this script runs
# under `set -e` (line 98), so the refusal this phase is here to observe would
# otherwise terminate the script at the very moment it succeeded.
SEAM_BOOT_STATUS=0
timeout 60 env TELLURION_CONFIG="$SEAM_ENV_CONFIG" TELLURION_SMOKE_GPKG="$GPKG" \
  PORT=18206 "$TELLURION" >"$SEAM_BOOT_LOG" 2>&1 || SEAM_BOOT_STATUS=$?
if [ "$SEAM_BOOT_STATUS" = 0 ]; then
  fail 'a token_env naming an unset variable must not boot'
fi
if [ "$SEAM_BOOT_STATUS" = 124 ]; then
  fail 'a token_env naming an unset variable neither booted nor refused within 60s'
fi
ok 'a token_env naming an unset variable refuses to boot'
expect_body_contains 'boot refusal' "$(cat "$SEAM_BOOT_LOG")" 'TELLURION_SMOKE_BEARER'
expect_body_contains 'boot refusal' "$(cat "$SEAM_BOOT_LOG")" 'auth.bearer_tokens'
stop_server

# --- phase 19: registry backends named, registered, and refused by name ------
#
# `#162`. Before this slice the relational registry backend lived in one
# `Option<Arc<dyn RelationalRegistryFactory>>` slot: `backend: relational`
# selected "the" relational implementation, which is indistinguishable from
# correct while exactly one exists and cannot express a choice the moment a
# second does. It is now a `NamedRegistry` of factories keyed by each driver
# crate's own declared name, selected by `registry.implementation`.
#
# What this phase is for that the unit tests are not: the boot log and the
# boot refusal of a real process. The registered names are enumerated at
# startup precisely so an operator can read "what does this binary actually
# contain" off stdout, and nothing in-process observes stdout. The refusal is
# checked the same way for the same reason.

REGISTRY_SEAM_CONFIG=$(config_for registry-backend-seam)
cat >"$REGISTRY_SEAM_CONFIG" <<'YAML'
control_store:
  backend: legacy_file
server:
  port: 18207
storages:
  - id: main
    driver: geopackage
    url_env: TELLURION_SMOKE_GPKG
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: smoke_points
    catalog: default
    storage: main
YAML

printf 'phase 19: registry backends named at boot, and refused by name\n'
SMOKE_PORT=18207
start_server "$REGISTRY_SEAM_CONFIG"

# This config says nothing about `registry` at all — it is the unconfigured
# deployment, and it must still serve exactly what it always did. Asserted
# BEFORE the log assertions so a regression that broke the file backend reads
# as a serving failure rather than as a missing log line.
expect_status '/public/features/catalogs/default/collections' 200
REGISTRY_SEAM_LOG=$(cat "$WORK/server.log")
# The seam's own boot line: `file` is the direct built-in backend (no driver
# crate, nothing to name), and the relational implementations are exactly what
# this binary registered — `postgis`, under the same name its storage driver
# uses. Both halves of the one `registry.backend` knob are enumerated, because
# both are selected by the same name.
#
# Asserted as separate tokens rather than as `key="value"` pairs: this log is
# written to a terminal-shaped stream and the field NAME carries ANSI styling,
# so the bytes between a key and its value are not the bare `=` they look
# like. The strict, line-scoped form of this assertion lives in
# `tellurion-server/tests/extension_registry_boot.rs`, which reads the JSON
# log where there is no styling to step around.
expect_body_contains 'server log' "$REGISTRY_SEAM_LOG" 'extension registry: catalog/collection registry backend'
expect_body_contains 'server log' "$REGISTRY_SEAM_LOG" 'builtin'
expect_body_contains 'server log' "$REGISTRY_SEAM_LOG" 'relational_registry_implementations'
expect_body_contains 'server log' "$REGISTRY_SEAM_LOG" 'relational_tenant_implementations'
expect_body_contains 'server log' "$REGISTRY_SEAM_LOG" '["postgis"]'
stop_server

# A `registry.implementation` naming something this binary does not contain
# refuses to boot, by name, and says what IS registered. Not a server that
# starts having quietly fallen back to the file backend — that is a
# deployment reading its catalogs out of a YAML file it thought it had
# migrated off, discovered weeks later.
#
# `TELLURION_SMOKE_UNSET_DATABASE_URL` is the one database-shaped variable
# this script guarantees is unset (see the preflight above), so it is
# deliberately NOT what this config names: a URL that fails to resolve would
# refuse for the wrong reason and prove nothing about the implementation name.
# Naming a variable that IS set, pointing at a port nothing listens on, is
# what makes the assertion sharp — the refusal below happens while a
# reachable-looking DSN is sitting right there, because selecting the
# implementation comes before connecting to anything.
REGISTRY_SEAM_UNKNOWN_CONFIG=$(config_for registry-backend-unknown)
cat >"$REGISTRY_SEAM_UNKNOWN_CONFIG" <<'YAML'
control_store:
  backend: legacy_file
server:
  port: 18207
storages:
  - id: db
    driver: postgis
    url_env: TELLURION_SMOKE_REGISTRY_DSN
registry:
  backend: relational
  storage: db
  implementation: not-compiled-in
YAML

REGISTRY_SEAM_BOOT_LOG="$WORK/registry-seam-boot.log"
REGISTRY_SEAM_BOOT_STATUS=0
# Bounded and all three outcomes named, for the reasons phase 18 spells out
# above its own `timeout` call.
timeout 60 env TELLURION_CONFIG="$REGISTRY_SEAM_UNKNOWN_CONFIG" \
  TELLURION_SMOKE_REGISTRY_DSN='postgres://127.0.0.1:1/nonexistent-registry' \
  PORT=18207 "$TELLURION" >"$REGISTRY_SEAM_BOOT_LOG" 2>&1 || REGISTRY_SEAM_BOOT_STATUS=$?
if [ "$REGISTRY_SEAM_BOOT_STATUS" = 0 ]; then
  fail 'a registry.implementation naming an absent backend must not boot'
fi
if [ "$REGISTRY_SEAM_BOOT_STATUS" = 124 ]; then
  fail 'a registry.implementation naming an absent backend neither booted nor refused within 60s'
fi
ok 'a registry.implementation naming an absent backend refuses to boot'
REGISTRY_SEAM_REFUSAL=$(cat "$REGISTRY_SEAM_BOOT_LOG")
expect_body_contains 'boot refusal' "$REGISTRY_SEAM_REFUSAL" 'registry.implementation'
expect_body_contains 'boot refusal' "$REGISTRY_SEAM_REFUSAL" 'not-compiled-in'
# Listing what IS registered is the difference between "that name is wrong"
# and "that name is wrong, and here is the set to pick from."
expect_body_contains 'boot refusal' "$REGISTRY_SEAM_REFUSAL" 'postgis'

# --- phase 21: hierarchical path-scoped administration policy ---------------
#
# `#215`. Everything below this line is asserted against a real process that
# read its bindings and statements out of a boot envelope, seeded them into a
# real durable control store, compiled them at boot and enforced them in its
# own middleware. The in-process tests prove the decision; this proves the
# declaration reaches it.
#
# The fixture is the delegated-administration shape the rule exists for: one
# tenant, two catalogs, and an operator whose authority is bound to exactly
# one of them.
#
# What each assertion is for:
#
#  - the bound catalog answers, so nothing here passes by refusing everything;
#  - the SIBLING catalog answers `404` -- the same `404` a catalog that does
#    not exist gives, because at this depth `403`-versus-`404` would let a
#    catalog administrator enumerate the tenant's other catalogs by probing;
#  - the parent tenant answers `403` -- authority flows down, never up;
#  - the platform administrator, bound at platform scope, reaches all of it;
#  - a traversal resolves to nothing and so keeps the answer it already had;
#  - and `/config/profiles`, which no statement mentions, is untouched.

POLICY_CONFIG=$(config_for scoped-admin)
POLICY_STORE="$WORK/cfg/scoped-admin/control.db"
cat >"$POLICY_CONFIG" <<YAML
control_store:
  backend: sqlite
  path: $POLICY_STORE
server:
  port: 18209
storages:
  - id: main
    driver: geopackage
    url_env: TELLURION_SMOKE_GPKG
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
  - id: other
    tenant: public
collections:
  - id: smoke_points
    catalog: default
    storage: main
auth:
  bearer_tokens:
    - token_env: TELLURION_SMOKE_PLATFORM_ADMIN_TOKEN
      tenants: [public]
      platform_admin: true
      principal: smoke-sysadmin
    - token_env: TELLURION_SMOKE_CATALOG_ADMIN_TOKEN
      tenants: [public]
      principal: smoke-catalog-operator
role_bindings:
  - principal: { issuer: 'urn:tellurion:static', subject: smoke-sysadmin }
    role: sysadmin
    scope: { kind: platform }
  - principal: { issuer: 'urn:tellurion:static', subject: smoke-catalog-operator }
    role: catalog_admin
    scope: { kind: catalog, tenant_id: public, catalog_id: default }
path_policies:
  - id: read-administration
    effect: allow
    methods: [GET]
    patterns: ['/config/effective', '/public/config/**']
    roles: [sysadmin, catalog_admin]
YAML

printf 'phase 21: hierarchical path-scoped administration policy\n'
SMOKE_PORT=18209
rm -f "$POLICY_STORE"
TELLURION_SMOKE_PLATFORM_ADMIN_TOKEN=smoke-platform-admin
TELLURION_SMOKE_CATALOG_ADMIN_TOKEN=smoke-catalog-admin
export TELLURION_SMOKE_PLATFORM_ADMIN_TOKEN TELLURION_SMOKE_CATALOG_ADMIN_TOKEN
start_server "$POLICY_CONFIG"

# The declaration really was read and compiled: the server says so by name,
# once, at activation.
POLICY_LOG=$(cat "$WORK/server.log")
expect_body_contains 'boot log' "$POLICY_LOG" \
  'hierarchical path-scoped administration policy activated'

# Downward: the catalog this operator is bound to.
expect_bearer_status '/public/config/catalogs/default/effective' \
  'smoke-catalog-admin' 200 'the catalog admin'
# Sideways: the sibling catalog, refused as a `404` -- and proved to be the
# SAME answer a catalog that does not exist gives, which is the whole point.
expect_bearer_status '/public/config/catalogs/other/effective' \
  'smoke-catalog-admin' 404 'the catalog admin'
expect_bearer_status '/public/config/catalogs/no-such-catalog/effective' \
  'smoke-catalog-admin' 404 'the catalog admin'
# Upward: the parent tenant, refused as a `403` -- the tenant boundary was
# already crossed, so its existence is not news.
expect_bearer_status '/public/config/effective' \
  'smoke-catalog-admin' 403 'the catalog admin'
# And the platform node, which this operator's binding does not cover either.
expect_bearer_status '/config/effective' 'smoke-catalog-admin' 403 'the catalog admin'

# The platform administrator, bound at platform scope, reaches every scope
# beneath it -- so every refusal above is the scope rule at work and not a
# policy set that grants nothing to anyone.
expect_bearer_status '/config/effective' 'smoke-platform-admin' 200 'the platform admin'
expect_bearer_status '/public/config/effective' \
  'smoke-platform-admin' 200 'the platform admin'
expect_bearer_status '/public/config/catalogs/default/effective' \
  'smoke-platform-admin' 200 'the platform admin'
expect_bearer_status '/public/config/catalogs/other/effective' \
  'smoke-platform-admin' 200 'the platform admin'

# A traversal reaches no decision at all -- it resolves to no catalog, so it
# keeps the answer it already had rather than acquiring a new refusal. That is
# what keeps a governed deployment from changing an answer on a path nobody
# governed.
expect_bearer_status '/public/config/catalogs/%2e%2e/effective' \
  'smoke-platform-admin' 404 'the platform admin'

# A path no statement mentions is not governed at all, and answers exactly as
# it did before this feature existed -- unauthenticated, and 200.
expect_status '/config/profiles' 200

stop_server
unset TELLURION_SMOKE_PLATFORM_ADMIN_TOKEN TELLURION_SMOKE_CATALOG_ADMIN_TOKEN

# --- phase 22: a raster collection's OGC API Maps `/map` (`#37`) ------------
#
# `crates/tellurion-tiles/src/maps.rs` used to say, in its own module doc,
# "No raster-collection support in this slice". A COG- or Zarr-backed
# collection has no vector `TileSource` anywhere in its maps lane, so
# `Router::resolve_maps` refused it and `GET .../map` answered `404` -- even
# though the same collection served raster PNG *tiles* perfectly well.
#
# What this phase proves against a real process, which no in-process test
# can prove together:
#
#   1. the map RENDERS -- a real GeoTIFF, decoded and composited by the real
#      driver, reaching the wire as PNG bytes;
#   2. it carries `Content-Crs` and `Content-Bbox` (Maps Part 1
#      `/req/core/map-response` C/D/E) -- without them a client that supplied
#      no parameters cannot georeference what it got;
#   3. the collection's own COLORMAP classifies it: two collections over the
#      SAME GeoTIFF, alike in everything but `settings.colormap`, must return
#      different bytes. Identical bytes would mean the configuration never
#      reached the render path -- the failure a mere `200` cannot detect;
#   4. a `style` and an over-budget window are refused BY NAME, in
#      problem+json, never clamped or silently unstyled;
#   5. the `map` link is advertised exactly where the capability resolves
#      (OGC 20-058 Requirement 46) -- present for the raster collection AND
#      for the vector one beside it, absent for the record collection whose
#      tiles root does not serve it at all;
#   6. and the GeoPackage-backed vector collection is untouched: its own
#      `/map` still renders.
#
# The binary this phase boots is the only one in this script built with the
# `cog` feature (default-off, like every other file-backed driver). Same
# shape as phase 5's own `--features valkey` build, and for the same reason:
# the capability under test is not in the default binary.

printf 'phase 22: a raster collection served through OGC API Maps\n'
CARGO_PROFILE_DEV_DEBUG=0 cargo build --quiet \
  -p tellurion --features cog >"$WORK/build-cog.log" 2>&1 ||
  { cat "$WORK/build-cog.log" >&2; fail 'cargo build --features cog'; }

# `tellurion-cog`'s own committed single-band gradient fixture: a 32x32 Gray
# GeoTIFF spanning CRS84 [-1.28, 1.28] on both axes whose 16x16 bottom-right
# block carries every value in 0..=255 exactly once. Served straight out of
# the worktree -- no ingest step, because a COG needs no DDL and this script
# provisions no raster of its own.
export TELLURION_SMOKE_COG="$ROOT/crates/tellurion-cog/tests/fixtures/gray_gradient.tif"

RASTER_MAPS_CONFIG=$(config_for raster-maps)
cat >"$RASTER_MAPS_CONFIG" <<'YAML'
control_store:
  backend: legacy_file
server:
  port: 18210
storages:
  - id: main
    driver: geopackage
    url_env: TELLURION_SMOKE_GPKG
  - id: raster
    driver: cog
    url_env: TELLURION_SMOKE_COG
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
    settings:
      protocols: { records: enabled }
collections:
  # The raster collection, under two colormaps over the SAME GeoTIFF -- the
  # only way to show the configured colormap reaches the render path rather
  # than merely being accepted by config validation. `table:` names the
  # driver's own logical name for the store (the file stem), which is how a
  # second collection can point at it.
  - id: gray_stops
    catalog: default
    storage: raster
    table: gray_gradient
    tiles: { minzoom: 8, maxzoom: 8, caps: {} }
    settings:
      colormap:
        kind: stops
        stops:
          - { value: 0.0, rgba: [255, 0, 0, 255] }
          - { value: 128.0, rgba: [0, 255, 0, 255] }
          - { value: 255.0, rgba: [0, 0, 255, 255] }
  - id: gray_viridis
    catalog: default
    storage: raster
    table: gray_gradient
    tiles: { minzoom: 8, maxzoom: 8, caps: {} }
    settings:
      colormap: { kind: ramp, ramp: viridis, min: 0.0, max: 255.0 }
  # The vector collection beside it, on the ordinary GeoPackage storage --
  # the "untouched" half of every assertion below.
  - id: smoke_points
    catalog: default
    storage: main
  # A record collection: no geometry, so the tiles root does not serve it and
  # nothing may advertise a map for it.
  - id: smoke_records
    catalog: default
    storage: main
    kind: record
YAML

SMOKE_PORT=18210
start_server "$RASTER_MAPS_CONFIG"

RASTER_TILES='/public/tiles/catalogs/default/collections'
# The WebMercatorQuad tile z8/x128/y128, in mercator metres, inset by a metre
# on each side. That tile is the fixture's bottom-right quadrant -- the one
# tile carrying its full 0..=255 sample range. Its own bounds are
# [0, -156543.03392804, 156543.03392804, 0]; the inset keeps the window at
# exactly this tile, because `covering_tiles` floors each edge into a tile
# index and a maximum landing exactly on a boundary pulls in the neighbour
# as well.
RASTER_BBOX='bbox=1,-156542.03392804,156542.03392804,-1'
# `#270`: metres, so the request says so. An omitted `bbox-crs` is CRS84
# (Maps Part 1 Requirement 18 clause C) and these numbers are nowhere near
# a valid latitude, so undeclared they are now refused by name -- which is
# phase 24's subject, not this phase's.
RASTER_BBOX_CRS='bbox-crs=http://www.opengis.net/def/crs/EPSG/0/3857'
RASTER_WINDOW="?$RASTER_BBOX&$RASTER_BBOX_CRS&width=128&height=128"
RASTER_MAP="$RASTER_TILES/gray_stops/map$RASTER_WINDOW"

# (1) it renders...
expect_status "$RASTER_MAP" 200
curl -s -o "$WORK/raster-map.png" "http://127.0.0.1:$SMOKE_PORT$RASTER_MAP"
# ...and (2) it says, on the wire, which CRS and which window -- Maps Part 1
# `/req/core/map-response` C, D and E. Without both a client that supplied no
# parameters cannot georeference what it got.
expect_header 'raster /map' "$RASTER_MAP" 'content-type' 'image/png'
expect_header 'raster /map' "$RASTER_MAP" 'content-crs' \
  '<http://www.opengis.net/def/crs/EPSG/0/3857>'
expect_header 'raster /map' "$RASTER_MAP" 'content-bbox' \
  '1,-156542.03392804,156542.03392804,-1'
# Real PNG bytes, not an empty or truncated body.
has_png_signature "$WORK/raster-map.png" ||
  fail 'the raster /map body is not a PNG'
ok 'the raster /map body carries the PNG signature'
[ "$(wc -c <"$WORK/raster-map.png")" -gt 500 ] ||
  fail 'the raster /map body is too small to be a rendered 128x128 image'
ok 'the raster /map body is a rendered image, not a blank stub'

# (3) the collection's OWN colormap classified it. Same GeoTIFF, same window,
# same output size -- only `settings.colormap` differs, and the bytes must.
curl -s -o "$WORK/raster-map-viridis.png" \
  "http://127.0.0.1:$SMOKE_PORT$RASTER_TILES/gray_viridis/map$RASTER_WINDOW"
has_png_signature "$WORK/raster-map-viridis.png" ||
  fail 'the second colormap did not render a PNG'
ok 'the second colormap renders a PNG too'
cmp -s "$WORK/raster-map.png" "$WORK/raster-map-viridis.png" &&
  fail 'two different colormaps over the same GeoTIFF produced byte-identical
  maps -- the configured colormap is not reaching the render path'
ok 'two colormaps over the same GeoTIFF render different maps'

# (4) named refusals, in problem+json, never a degraded 200.
expect_status "$RASTER_MAP&style=basic" 400
RASTER_STYLE_REFUSAL=$(curl -s "http://127.0.0.1:$SMOKE_PORT$RASTER_MAP&style=basic")
expect_body_contains 'raster /map style refusal' "$RASTER_STYLE_REFUSAL" \
  '"code":"CapabilityUnsupported"'
expect_body_contains 'raster /map style refusal' "$RASTER_STYLE_REFUSAL" 'styled-map'

RASTER_HUGE="?bbox=-20037508,-20037508,20037508,20037508&$RASTER_BBOX_CRS&width=16&height=16"
expect_status "$RASTER_TILES/gray_stops/map$RASTER_HUGE" 400
RASTER_BUDGET_REFUSAL=$(curl -s \
  "http://127.0.0.1:$SMOKE_PORT$RASTER_TILES/gray_stops/map$RASTER_HUGE")
expect_body_contains 'raster /map budget refusal' "$RASTER_BUDGET_REFUSAL" \
  '"code":"PixelBudgetExceeded"'

# (5) advertised exactly where it resolves (OGC 20-058 Requirement 46).
# Asserted against the `/collections` LISTING rather than a per-collection
# document: a raster-only collection has no features lane, so
# `/collections/{cid}` is a `404` for it (unchanged by this slice) while the
# listing does carry it -- exactly the `#37` reasoning
# `Router::canonical_descriptor` already states for why a raster collection
# must report `tiles: true` at all.
RASTER_DOC=$(body_of '/public/features/catalogs/default/collections')
expect_body_contains 'collections listing' "$RASTER_DOC" \
  '"href":"/public/tiles/catalogs/default/collections/gray_stops/map"'
expect_body_contains 'collections listing' "$RASTER_DOC" \
  '"rel":"https://www.opengis.net/def/rel/ogc/1.0/map"'
# ...and for the vector collection beside it, on the same listing: this link
# is not a raster special case, it is the collection-map link both lanes earn.
expect_body_contains 'collections listing' "$RASTER_DOC" \
  '"href":"/public/tiles/catalogs/default/collections/smoke_points/map"'
# ...and absent for the record collection, whose tiles root does not serve it
# at all. A link into a route that 404s is exactly the promise this must not
# make.
RECORD_DOC=$(body_of "/public/records/catalogs/default/collections/smoke_records")
expect_body_lacks 'record collection document' "$RECORD_DOC" \
  'collections/smoke_records/map'

# (6) the vector lane is untouched: its own /map still renders, through the
# same handler, from the same request shape.
expect_status "$RASTER_TILES/smoke_points/map?bbox=-20037508,-20037508,20037508,20037508&$RASTER_BBOX_CRS&width=64&height=64" 200

stop_server

# --- phase 24: an omitted `bbox-crs` on `/map` is CRS84 (`#270`) ------------
#
# `maps::parse_crs` used to read an omitted `bbox-crs` as the tile matrix
# set's own CRS (EPSG:3857). OGC 20-058 Requirement 18
# (`/req/spatial-subsetting/bbox-crs`) clause C says otherwise, verbatim:
# "If the bbox-crs is not indicated https://www.opengis.net/def/crs/OGC/1.3/
# CRS84 SHALL be assumed."
#
# Closing that divergence moves bytes, which is exactly why it needs a phase
# through the real binary rather than a parse-level unit test alone. A client
# that today sends metres without declaring `bbox-crs` gets the window it
# meant; under the clause those same numbers are degrees, and it would get a
# wildly different window back under a `200` with nothing saying so. So the
# default change is paired with a guard, and the pair is only honest if both
# halves are visible on the wire:
#
#   1. degrees, undeclared, are READ as degrees -- proved by `Content-Bbox`,
#      which comes back in this lane's own metres and therefore shows the
#      forward projection having happened. The pre-`#270` reading would have
#      echoed the four numbers back unchanged;
#   2. metres, undeclared, are REFUSED BY NAME in problem+json -- and the
#      refusal names `bbox-crs` and the value to supply, because a bare
#      "invalid bbox" would leave the one class of client this breaks with
#      no idea what to do;
#   3. the migration the refusal names actually works: the same window with
#      `bbox-crs` declared renders;
#   4. an explicitly declared `bbox-crs`, in either CRS, is untouched;
#   5. `bbox=-180,-90,180,90` -- the most ordinary CRS84 request there is --
#      is INSIDE the ranges, not refused by an over-eager guard;
#   6. Requirement 18 clause F holds: with no `bbox` at all there is nothing
#      for `bbox-crs` to qualify, so the guard cannot fire;
#   7. the OUTPUT `crs` default did NOT move. Requirement 35 NOTE 2 gives the
#      two parameters different defaults ("The default CRS of the BBOX is
#      ...CRS84 but the default CRS of the map is the native (storage) CRS"),
#      and `#270` changed only the first;
#   8. and the conformance declaration is unchanged -- `conf/crs` stays,
#      `conf/spatial-subsetting` is still NOT declared (it needs `subset`,
#      `subset-crs`, `center` and `center-crs`, none of which exist here),
#      and `conf/collection-map` is still NOT declared (Requirement 47 wants
#      a `crs` list on the collection object that this server does not
#      produce for raster collections -- `#37`'s own deliberate omission).
#      A conformance list is the one place where getting closer to a class is
#      not permission to claim it.

printf 'phase 24: an omitted bbox-crs on /map is CRS84 (`#270`)\n'

BBOX_CRS_CONFIG=$(config_for bbox-crs-default)
cat >"$BBOX_CRS_CONFIG" <<'YAML'
control_store:
  backend: legacy_file
server:
  port: 18212
storages:
  - id: main
    driver: geopackage
    url_env: TELLURION_SMOKE_GPKG
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: smoke_points
    catalog: default
    storage: main
YAML

SMOKE_PORT=18212
start_server "$BBOX_CRS_CONFIG"

BC_MAP='/public/tiles/catalogs/default/collections/smoke_points/map'
BC_MERC='http://www.opengis.net/def/crs/EPSG/0/3857'
BC_CRS84='http://www.opengis.net/def/crs/OGC/1.3/CRS84'
# Degrees around the two fixture points (Rome and Milan), well inside
# +-180/+-90 and therefore plausible CRS84 -- the case the clause governs.
BC_DEG='bbox=9,41,13,46&width=64&height=64'
# Mercator metres: the whole projected world, three orders of magnitude
# outside any CRS84 latitude -- the case the guard governs.
BC_MERC_WIN='bbox=-20037508,-20037508,20037508,20037508&width=64&height=64'

bc_content_bbox() {
  curl -s -o /dev/null -D - "http://127.0.0.1:$SMOKE_PORT$1" |
    tr -d '\r' | grep -i '^content-bbox:' | head -n 1 | cut -d' ' -f2-
}

# (1) undeclared degrees are read as degrees. `Content-Bbox` reports the
# window in the response CRS, which with no `crs` parameter is this lane's
# own metres -- so a longitude of 9 degrees must come back as roughly
# 9 * 111319.49 = 1001875 metres. Under the pre-`#270` reading the same
# request echoed `9,41,13,46` straight back, which this range excludes by
# five orders of magnitude.
BC_UNDECLARED=$(bc_content_bbox "$BC_MAP?$BC_DEG")
BC_MINX=$(printf '%s' "$BC_UNDECLARED" | cut -d, -f1)
awk -v v="$BC_MINX" 'BEGIN { exit !(v > 1001000 && v < 1002500) }' ||
  fail "an omitted bbox-crs must be read as CRS84 degrees: Content-Bbox was
  '$BC_UNDECLARED', whose minimum x ($BC_MINX) is not 9 degrees forward-projected"
ok 'an omitted bbox-crs is read as CRS84 degrees, on the wire'

# (2) undeclared metres are refused BY NAME, never interpreted.
expect_status "$BC_MAP?$BC_MERC_WIN" 400
BC_REFUSAL=$(curl -s "http://127.0.0.1:$SMOKE_PORT$BC_MAP?$BC_MERC_WIN")
expect_body_contains 'undeclared metres refusal' "$BC_REFUSAL" \
  '"code":"BboxCrsRequired"'
# The whole point of the pairing: the refusal has to name the parameter to
# add and the value to give it, or it is no better than the silent wrong
# window it replaces.
expect_body_contains 'undeclared metres refusal' "$BC_REFUSAL" 'bbox-crs'
expect_body_contains 'undeclared metres refusal' "$BC_REFUSAL" "$BC_MERC"
expect_header 'undeclared metres refusal' "$BC_MAP?$BC_MERC_WIN" \
  'content-type' 'application/problem+json'

# (3) the migration the refusal names actually works...
expect_status "$BC_MAP?$BC_MERC_WIN&bbox-crs=$BC_MERC" 200
# ...and (4) a declared bbox-crs is untouched in either CRS. The same four
# numbers under the two declarations must land on two different windows, or
# the declaration is not reaching the parse at all.
expect_status "$BC_MAP?$BC_DEG&bbox-crs=$BC_CRS84" 200
BC_DECLARED_MERC=$(bc_content_bbox "$BC_MAP?$BC_DEG&bbox-crs=$BC_MERC")
[ "$BC_DECLARED_MERC" = '9,41,13,46' ] ||
  fail "a declared bbox-crs of metres must survive the parse verbatim, got
  '$BC_DECLARED_MERC'"
ok 'a declared bbox-crs=EPSG:3857 is read as metres, unchanged by `#270`'
[ "$BC_DECLARED_MERC" != "$BC_UNDECLARED" ] ||
  fail 'the declared and omitted readings of the same four numbers produced the
  same window -- one of the two is not reaching the parse'
ok 'the declared and the omitted readings of the same numbers differ on the wire'

# (5) the exact CRS84 world bbox is inside the ranges, not outside them. A
# guard that refused this would have swapped a silent wrong answer for a loud
# wrong one.
expect_status "$BC_MAP?bbox=-180,-90,180,90&width=64&height=64" 200

# (6) Requirement 18 clause F: "If the bbox parameter is not used, the
# bbox-crs SHALL be ignored" -- so is the guard.
BC_NO_BBOX=$(curl -s "http://127.0.0.1:$SMOKE_PORT$BC_MAP?width=64&height=64")
printf '%s' "$BC_NO_BBOX" | grep -Fq 'BboxCrsRequired' &&
  fail 'clause F: a request with no bbox has nothing for bbox-crs to qualify,
  so it must not be refused for one'
ok 'clause F: a request with no bbox is not touched by the bbox-crs guard'

# (7) the OUTPUT crs default did not move: still this lane's native CRS.
expect_header 'map with no crs parameter' "$BC_MAP?$BC_DEG" 'content-crs' \
  "<$BC_MERC>"

# (8) and the conformance declaration is exactly what it was.
BC_CONFORMANCE=$(body_of '/public/tiles/catalogs/default/conformance')
expect_body_contains 'tiles /conformance' "$BC_CONFORMANCE" \
  'ogcapi-maps-1/1.0/conf/crs'
expect_body_lacks 'tiles /conformance' "$BC_CONFORMANCE" \
  'ogcapi-maps-1/1.0/conf/spatial-subsetting'
expect_body_lacks 'tiles /conformance' "$BC_CONFORMANCE" \
  'ogcapi-maps-1/1.0/conf/collection-map'

stop_server


# --- phase 25: a bbox-less bbox-crs on /map is ignored, value still checked --
#
# `#291`: OGC 20-058 contradicts itself on a `bbox-crs` supplied without a
# `bbox`. Requirement 18 clause F, verbatim: "If the bbox parameter is not
# used, the bbox-crs SHALL be ignored." Section 13.5, verbatim and stated
# unconditionally: "If the CRS in the parameter value bbox-crs, subset-crs or
# center-crs is not supported by the server for this resource, or the
# parameter value is out-of-range, the status code of the response will be
# 400." For an unsupported value with no `bbox`, no server can honour both.
# The recorded decision (`docs/spec-deviations.md`, this repository's spec
# deviation register, whose first entry is exactly this clause pair) ignores
# the parameter's EFFECT and still validates its VALUE. On the wire that is
# three visible facts:
#
#   1. a SUPPORTED `bbox-crs` with no `bbox` changes nothing: same status,
#      same `Content-Crs`/`Content-Bbox`, and the same PNG bytes -- `cmp`ed
#      against the parameterless response, not eyeballed;
#   2. an UNSUPPORTED `bbox-crs` with no `bbox` is still refused BY NAME
#      (`CrsNotSupported`, naming the refused value) -- ignoring an unused
#      parameter is not accepting a nonsense one, and #270's named-refusal
#      contract survives clause F;
#   3. with a `bbox` present nothing moved: the same unsupported value is
#      the same named refusal.

printf 'phase 25: a bbox-less bbox-crs on /map is ignored, its value still checked (`#291`)\n'

BXC_CONFIG=$(config_for bboxless-bbox-crs)
cat >"$BXC_CONFIG" <<'YAML'
control_store:
  backend: legacy_file
server:
  port: 18213
storages:
  - id: main
    driver: geopackage
    url_env: TELLURION_SMOKE_GPKG
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: smoke_points
    catalog: default
    storage: main
YAML

SMOKE_PORT=18213
start_server "$BXC_CONFIG"

BXC_MAP='/public/tiles/catalogs/default/collections/smoke_points/map'
BXC_MERC='http://www.opengis.net/def/crs/EPSG/0/3857'
BXC_CRS84='http://www.opengis.net/def/crs/OGC/1.3/CRS84'
BXC_BOGUS='http://www.opengis.net/def/crs/EPSG/0/2154'

bxc_header() { # $1 = query string, $2 = header name
  curl -s -o /dev/null -D - "http://127.0.0.1:$SMOKE_PORT$BXC_MAP?$1" |
    tr -d '\r' | grep -i "^$2:" | head -n 1 | cut -d' ' -f2-
}

# (1) the parameterless baseline, then the same request with each supported
# bbox-crs declared. `smoke_points` has a derived extent (ingest measured it
# at load), so the baseline renders.
expect_status "$BXC_MAP?width=64&height=64" 200
curl -s -o "$WORK/bxc-baseline.png" \
  "http://127.0.0.1:$SMOKE_PORT$BXC_MAP?width=64&height=64" ||
  fail 'could not fetch the parameterless baseline map'
BXC_BASE_CRS=$(bxc_header 'width=64&height=64' 'content-crs')
BXC_BASE_BBOX=$(bxc_header 'width=64&height=64' 'content-bbox')
for BXC_DECLARED in "$BXC_CRS84" "$BXC_MERC"; do
  expect_status "$BXC_MAP?width=64&height=64&bbox-crs=$BXC_DECLARED" 200
  curl -s -o "$WORK/bxc-ignored.png" \
    "http://127.0.0.1:$SMOKE_PORT$BXC_MAP?width=64&height=64&bbox-crs=$BXC_DECLARED" ||
    fail 'could not fetch the bbox-less bbox-crs map'
  cmp -s "$WORK/bxc-baseline.png" "$WORK/bxc-ignored.png" ||
    fail "clause F: 'bbox-crs=$BXC_DECLARED' with no 'bbox' changed the PNG bytes"
  [ "$(bxc_header "width=64&height=64&bbox-crs=$BXC_DECLARED" 'content-crs')" \
    = "$BXC_BASE_CRS" ] ||
    fail "clause F: 'bbox-crs=$BXC_DECLARED' with no 'bbox' moved Content-Crs"
  [ "$(bxc_header "width=64&height=64&bbox-crs=$BXC_DECLARED" 'content-bbox')" \
    = "$BXC_BASE_BBOX" ] ||
    fail "clause F: 'bbox-crs=$BXC_DECLARED' with no 'bbox' moved Content-Bbox"
done
ok 'a supported bbox-less bbox-crs changes nothing on the wire, byte for byte'

# (2) the value is still validated without a bbox: refused by name, naming
# the value it refuses.
expect_status "$BXC_MAP?width=64&height=64&bbox-crs=$BXC_BOGUS" 400
BXC_REFUSAL=$(curl -s "http://127.0.0.1:$SMOKE_PORT$BXC_MAP?width=64&height=64&bbox-crs=$BXC_BOGUS")
expect_body_contains 'bbox-less unsupported bbox-crs refusal' "$BXC_REFUSAL" \
  '"code":"CrsNotSupported"'
expect_body_contains 'bbox-less unsupported bbox-crs refusal' "$BXC_REFUSAL" \
  "$BXC_BOGUS"
ok 'an unsupported bbox-less bbox-crs is still refused by name (13.5 side)'

# (3) and with a bbox present, nothing moved from #270.
expect_status "$BXC_MAP?bbox=9,41,13,46&width=64&height=64&bbox-crs=$BXC_BOGUS" 400
BXC_WITH_BBOX=$(curl -s "http://127.0.0.1:$SMOKE_PORT$BXC_MAP?bbox=9,41,13,46&width=64&height=64&bbox-crs=$BXC_BOGUS")
expect_body_contains 'unsupported bbox-crs with bbox refusal' "$BXC_WITH_BBOX" \
  '"code":"CrsNotSupported"'

stop_server


# --- phase 23: a bounded COG mosaic, composed and served (`#254`) ------------
#
# `crates/tellurion-cog` serves ONE GeoTIFF per storage. `#254` adds a second
# driver beside it, `cog-mosaic`, which serves one raster TileSet composed
# from a BOUNDED manifest of COG sources -- and puts the manifest itself in
# `ingest`'s hands, not the operator's: every bbox, byte length and SHA-256 in
# it is MEASURED from the object, never transcribed by a human.
#
# What this phase proves against the real binaries, which no in-process test
# can prove together:
#
#   1. `tellurion-ingest cog mosaic` authors the sidecar, with its sources in
#      ascending id order and provenance it measured itself;
#   2. the server boots against that sidecar and serves composed PNG tiles;
#   3. SELECTION reaches the wire: a tile only the western source covers and
#      a tile only the eastern source covers come back as different images;
#   4. COMPOSITION ORDER reaches the wire: the tile on EITHER side of the
#      seam is the overlapping source's own pixels -- so the two tiles, built
#      from two DIFFERENT pairs of sources, are byte-identical, and neither
#      matches the single-source tile beside it. Composed in any other order
#      the seam tiles would show the western/eastern sources instead;
#   5. the composed tile is byte-stable across repeated requests -- the
#      property a completion-ordered composition violates intermittently
#      rather than reliably, and therefore the one worth asserting against a
#      real server with real concurrent reads;
#   6. MVT is refused BY NAME on the mosaic collection, and a tile no source
#      covers is empty (204) rather than a fabricated blank;
#   7. a manifest over the 32-source bound, and a manifest whose SHA-256 no
#      longer matches its object, each refuse the BOOT by name -- never a
#      partially served mosaic, never a silently dropped source.
#
# The binary is the one phase 22 already built with `--features cog`: the
# mosaic driver ships behind that same feature (it composes the very same
# reader and adds no dependency of its own), so there is no third build here.

printf 'phase 23: a bounded COG mosaic composed from a measured manifest\n'

# The mosaic lives in its own directory, NOT beside a config: `config_for`'s
# own doc explains why anything written next to a config looks like a config
# change to the reload watch.
MOSAIC_DIR="$WORK/mosaic"
mkdir -p "$MOSAIC_DIR" || fail 'could not create the mosaic directory'
# `tellurion-cog`'s three committed constituents. Each is a 32x32 flat-colour
# EPSG:4326 GeoTIFF; `mosaic_a_west` spans lon [-1.28, 0], `mosaic_b_east`
# spans lon [0, 1.28], and `mosaic_c_overlap` straddles the seam at lon
# [-0.64, 0.64]. `c` sorts LAST, so wherever it covers it must paint over the
# other two -- which is what assertions (4) below read straight off the wire.
for name in mosaic_a_west mosaic_b_east mosaic_c_overlap; do
  cp "$ROOT/crates/tellurion-cog/tests/fixtures/$name.tif" "$MOSAIC_DIR/" ||
    fail "could not stage the $name fixture"
done

# (1) `ingest` authors the sidecar -- the ONLY place a manifest comes from.
MOSAIC_MANIFEST="$MOSAIC_DIR/smoke_mosaic.yaml"
"$INGEST" cog mosaic \
  --source "$MOSAIC_DIR/mosaic_a_west.tif" \
  --source "$MOSAIC_DIR/mosaic_b_east.tif" \
  --source "$MOSAIC_DIR/mosaic_c_overlap.tif" \
  --output "$MOSAIC_MANIFEST" \
  --collection smoke_mosaic --storage mosaic >"$WORK/mosaic-author.log" 2>&1 ||
  { cat "$WORK/mosaic-author.log" >&2; fail 'ingest cog mosaic'; }
[ -r "$MOSAIC_MANIFEST" ] || fail 'ingest cog mosaic wrote no manifest'
ok 'ingest cog mosaic authored the manifest sidecar'
# Measured, not declared: the SHA-256 in the manifest must be the digest of
# the object itself, computed here independently of the code under test.
MOSAIC_REAL_SHA=$(sha256sum "$MOSAIC_DIR/mosaic_c_overlap.tif" | cut -d' ' -f1)
grep -Fq "$MOSAIC_REAL_SHA" "$MOSAIC_MANIFEST" ||
  fail 'the manifest does not carry the real SHA-256 of its own source object'
ok 'the manifest carries a SHA-256 this script computed independently'
# Ascending id order IS the composition order, so it must be readable off the
# file: `sort -c` fails loudly if the ids are not already sorted.
grep -E '^- id: |^  - id: ' "$MOSAIC_MANIFEST" | sed 's/^ *- id: //' \
  >"$WORK/mosaic-ids.txt"
[ "$(awk 'END { print NR }' "$WORK/mosaic-ids.txt")" = "3" ] ||
  fail 'the manifest does not list exactly the three sources it was given'
sort -c "$WORK/mosaic-ids.txt" 2>/dev/null ||
  fail 'the manifest lists its sources out of ascending id order'
ok 'the manifest lists its three sources in ascending id order'
# ...and it names the driver, not a hand-written config block.
grep -Fq 'driver: cog-mosaic' "$WORK/mosaic-author.log" ||
  fail 'ingest cog mosaic printed no cog-mosaic storage snippet'
ok 'ingest cog mosaic printed the cog-mosaic storage snippet'

export TELLURION_SMOKE_MOSAIC="$MOSAIC_MANIFEST"

MOSAIC_CONFIG=$(config_for mosaic)
cat >"$MOSAIC_CONFIG" <<'YAML'
control_store:
  backend: legacy_file
server:
  port: 18211
storages:
  - id: main
    driver: geopackage
    url_env: TELLURION_SMOKE_GPKG
  - id: mosaic
    driver: cog-mosaic
    url_env: TELLURION_SMOKE_MOSAIC
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: smoke_mosaic
    catalog: default
    storage: mosaic
    tiles: { minzoom: 0, maxzoom: 12, caps: {} }
  # The vector collection beside it, untouched by any of this.
  - id: smoke_points
    catalog: default
    storage: main
YAML

SMOKE_PORT=18211
start_server "$MOSAIC_CONFIG"

# (2) the composed tiles reach the wire as PNG.
MOSAIC_TILES="/public/tiles/catalogs/default/collections/smoke_mosaic/tiles/WebMercatorQuad/10"
# z10 row 511 is the strip just north of the equator, inside every
# constituent's own latitude span. Column 509 is covered ONLY by the west
# source, 514 ONLY by the east source, and 511/512 are the two columns either
# side of the seam, covered by west+overlap and east+overlap respectively.
for column in 509 511 512 514; do
  code=$(curl -s -o "$WORK/mosaic-$column.png" -w '%{http_code}' \
    "http://127.0.0.1:$SMOKE_PORT$MOSAIC_TILES/511/$column.png")
  [ "$code" = "200" ] ||
    fail "GET the mosaic tile at column $column returned $code, expected 200"
  has_png_signature "$WORK/mosaic-$column.png" ||
    fail "the mosaic tile at column $column is not a PNG"
done
ok 'every in-coverage mosaic tile is a real PNG'
expect_header 'mosaic tile' "$MOSAIC_TILES/511/511.png" 'content-type' 'image/png'

# (3) selection reaches the wire: the west-only and east-only tiles are
# different images, so the driver is not serving one source for everything.
cmp -s "$WORK/mosaic-509.png" "$WORK/mosaic-514.png" &&
  fail 'the west-only and east-only mosaic tiles are byte-identical -- source
  selection is not reaching the composed tile'
ok 'the west-only and east-only mosaic tiles are different images'

# (4) composition ORDER reaches the wire. Both seam tiles are composed from a
# DIFFERENT pair of sources (west+overlap, east+overlap) and must nonetheless
# come back byte-identical, because the overlapping source sorts LAST and
# therefore paints over both. Composed in the other order they would be the
# west and east tiles instead -- which is exactly what the two `cmp` refusals
# below rule out.
cmp -s "$WORK/mosaic-511.png" "$WORK/mosaic-512.png" ||
  fail 'the two seam tiles differ, so the last-sorting source did not paint
  over both of its neighbours -- composition is not in ascending source-id order'
ok 'both seam tiles carry the last-sorting sources own pixels'
cmp -s "$WORK/mosaic-511.png" "$WORK/mosaic-509.png" &&
  fail 'the west seam tile equals the west-only tile -- the overlapping source
  was selected but never painted over it'
ok 'the west seam tile is not the west sources own tile'
cmp -s "$WORK/mosaic-512.png" "$WORK/mosaic-514.png" &&
  fail 'the east seam tile equals the east-only tile -- the overlapping source
  was selected but never painted over it'
ok 'the east seam tile is not the east sources own tile'

# (5) byte-stable across repeated requests, with real concurrent reads behind
# each one.
i=0
while [ "$i" -lt 5 ]; do
  curl -s -o "$WORK/mosaic-repeat.png" \
    "http://127.0.0.1:$SMOKE_PORT$MOSAIC_TILES/511/511.png"
  cmp -s "$WORK/mosaic-repeat.png" "$WORK/mosaic-511.png" ||
    fail 'a repeated request for the same composed tile returned different bytes
  -- the composition depends on which constituent read finished first'
  i=$((i + 1))
done
ok 'the composed tile is byte-identical across repeated requests'

# (6) MVT refused BY NAME, and an uncovered tile is empty rather than blank.
expect_status "$MOSAIC_TILES/511/511.mvt" 400
MOSAIC_MVT_REFUSAL=$(curl -s "http://127.0.0.1:$SMOKE_PORT$MOSAIC_TILES/511/511.mvt")
expect_body_contains 'mosaic MVT refusal' "$MOSAIC_MVT_REFUSAL" \
  '"code":"CapabilityUnsupported"'
expect_status '/public/tiles/catalogs/default/collections/smoke_mosaic/tiles/WebMercatorQuad/2/0/0.png' 204
# ...and the vector collection beside it is untouched.
expect_status '/public/features/catalogs/default/collections/smoke_points/items?limit=1' 200

stop_server

# (7) a broken manifest refuses the BOOT, by name. Two of them: one over the
# 32-source bound (structural, refused when the storage is built), and one
# whose recorded SHA-256 no longer matches its object (provenance, refused by
# the eager catalog sweep). Neither may serve anything at all.
mosaic_boot_must_fail() { # $1 = manifest path, $2 = label, $3 = expected text
  require_free_port "$SMOKE_PORT"
  if TELLURION_CONFIG="$MOSAIC_CONFIG" TELLURION_SMOKE_MOSAIC="$1" \
    TELLURION_SMOKE_GPKG="$GPKG" PORT="$SMOKE_PORT" \
    "$TELLURION" >"$WORK/mosaic-boot.log" 2>&1; then
    fail "$2: the server booted instead of refusing"
  fi
  grep -Fq "$3" "$WORK/mosaic-boot.log" ||
    {
      tail -n 20 "$WORK/mosaic-boot.log" >&2
      fail "$2: the boot failed, but not with the named refusal '$3'"
    }
  ok "$2 refuses the boot by name"
}

MOSAIC_TOO_MANY="$MOSAIC_DIR/too_many.yaml"
{
  printf 'version: 1\nsources:\n'
  i=0
  while [ "$i" -lt 33 ]; do
    printf -- '- id: s%03d\n  path: mosaic_a_west.tif\n' "$i"
    printf -- '  bbox: [-1.0, -1.0, 1.0, 1.0]\n  byte_length: 3356\n'
    printf -- '  sha256: %s\n' \
      'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'
    i=$((i + 1))
  done
} >"$MOSAIC_TOO_MANY"
mosaic_boot_must_fail "$MOSAIC_TOO_MANY" \
  'a manifest over the 32-source bound' "over this driver's bound of 32"

MOSAIC_TAMPERED="$MOSAIC_DIR/tampered.yaml"
sed "s/$MOSAIC_REAL_SHA/bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb/" \
  "$MOSAIC_MANIFEST" >"$MOSAIC_TAMPERED"
cmp -s "$MOSAIC_MANIFEST" "$MOSAIC_TAMPERED" &&
  fail 'the tampered manifest is identical to the real one, so this assertion
  would prove nothing'
mosaic_boot_must_fail "$MOSAIC_TAMPERED" \
  'a manifest whose SHA-256 no longer matches its object' 'mosaic_c_overlap'

# --- phase 26: a raster collection advertises only what its driver can do ----
#
# `#287`: a raster-only collection (a COG -- no `FeatureSource`, no vector
# `TileSource`) used to advertise, in its features `/collections` entry, a
# handful of vector capabilities it demonstrably cannot honour: a
# `tilesets-vector` link whose `.mvt` route answers 400 (phase 22 proves that
# refusal on this very driver), `itemType: "feature"` for a collection whose
# `/items` route 404s, a `queryables` link, a `crs` list, and the
# per-collection `cql2ConformanceClasses`/`lockingConformanceClasses`
# members. After `#287` every capability-bearing member is derived from the
# driver's own capability accessors, and where the capability is absent the
# member is ABSENT -- not empty, not null.
#
# The catalog here holds ONLY the raster collection, so every `lacks`
# assertion below reads unambiguously: nothing else in the body could carry
# the member being asserted absent. The vector side of the same coin -- that
# these members are all still PRESENT for a features-capable collection -- is
# phase 1's and phase 6's existing assertions (`crs`, `storageCrs`,
# `cql2ConformanceClasses`, the queryables conformance classes), unchanged.
# The binary is the one phase 22 built with `--features cog`.

printf 'phase 26: a raster-only collection advertises no vector capabilities\n'

RASTER_ADS_CONFIG=$(config_for raster-ads)
cat >"$RASTER_ADS_CONFIG" <<'YAML'
control_store:
  backend: legacy_file
server:
  port: 18214
storages:
  - id: raster
    driver: cog
    url_env: TELLURION_SMOKE_COG
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: gray_stops
    catalog: default
    storage: raster
    table: gray_gradient
    tiles: { minzoom: 8, maxzoom: 8, caps: {} }
YAML

SMOKE_PORT=18214
start_server "$RASTER_ADS_CONFIG"

RASTER_ADS_DOC=$(body_of '/public/features/catalogs/default/collections')

# Still listed, still real: the collection, its tag-derived extent, and the
# lanes its driver genuinely serves (PNG tiles and the collection map).
expect_body_contains 'raster-only listing' "$RASTER_ADS_DOC" '"id":"gray_stops"'
expect_body_contains 'raster-only listing' "$RASTER_ADS_DOC" '"extent"'
expect_body_contains 'raster-only listing' "$RASTER_ADS_DOC" 'tilesets-map'
expect_body_contains 'raster-only listing' "$RASTER_ADS_DOC" \
  '"href":"/public/tiles/catalogs/default/collections/gray_stops/map"'

# Absent, not empty and not null: every member only a `FeatureSource` (or a
# vector `TileSource`, for `tilesets-vector`) could honour.
expect_body_lacks 'raster-only listing' "$RASTER_ADS_DOC" 'tilesets-vector'
expect_body_lacks 'raster-only listing' "$RASTER_ADS_DOC" '"itemType"'
expect_body_lacks 'raster-only listing' "$RASTER_ADS_DOC" 'queryables'
expect_body_lacks 'raster-only listing' "$RASTER_ADS_DOC" '"storageCrs"'
expect_body_lacks 'raster-only listing' "$RASTER_ADS_DOC" '"crs":['
expect_body_lacks 'raster-only listing' "$RASTER_ADS_DOC" 'cql2ConformanceClasses'
expect_body_lacks 'raster-only listing' "$RASTER_ADS_DOC" 'lockingConformanceClasses'
expect_body_lacks 'raster-only listing' "$RASTER_ADS_DOC" '"rel":"items"'

# The refusal behind the withdrawn advertisement is unchanged (`#287` fixed
# the document, never the request path): MVT is still a named 400, and the
# PNG lane the surviving links point at still serves.
expect_status '/public/tiles/catalogs/default/collections/gray_stops/tiles/WebMercatorQuad/8/128/128.png' 200
expect_status '/public/tiles/catalogs/default/collections/gray_stops/tiles/WebMercatorQuad/8/128/128.mvt' 400

stop_server

# --- phase 27: STAC projection facts, derived and declared honestly (`#36`) --
#
# The STAC `projection` extension, derived from the driver with no
# configuration at all. What this phase proves against a real process, which
# no in-process test can prove together:
#
#   1. a COG-backed collection — invisible to the STAC root before this
#      slice, even as its sibling Features root listed it — is listed and
#      described there, and its Collection document carries the `proj:*`
#      facts the driver read out of the GeoTIFF's OWN georeferencing
#      (`gray_gradient.tif`: 32x32 pixels, 0.08 degrees/pixel from origin
#      (-1.28, 1.28), EPSG:4326), as `summaries`, with the extension
#      declared exactly because those fields are emitted;
#   2. a GeoPackage-backed vector collection's Items carry `proj:epsg` from
#      the geometry column's SRID and declare the extension — and carry
#      NOTHING else: no `proj:transform`, no `proj:shape`, no identity
#      default, because a vector table has neither concept and an invented
#      plausible value is worse than an absent one;
#   3. a collection with no driver-read projection facts keeps its
#      Collection document free of `summaries`/`stac_extensions` entirely —
#      absent stays absent, never null.
#
# The binary is the `--features cog` one phases 22/23 already built; the
# GeoTIFF is `tellurion-cog`'s own committed gradient fixture.

printf 'phase 27: STAC projection facts, derived and declared honestly\n'

export TELLURION_SMOKE_COG="$ROOT/crates/tellurion-cog/tests/fixtures/gray_gradient.tif"

STAC_PROJ_CONFIG=$(config_for stac-projection)
cat >"$STAC_PROJ_CONFIG" <<'YAML'
control_store:
  backend: legacy_file
server:
  port: 18215
storages:
  - id: main
    driver: geopackage
    url_env: TELLURION_SMOKE_GPKG
  - id: raster
    driver: cog
    url_env: TELLURION_SMOKE_COG
tenants:
  - id: public
catalogs:
  - id: default
    tenant: public
collections:
  - id: gradient
    catalog: default
    storage: raster
    table: gray_gradient
  - id: smoke_points
    catalog: default
    storage: main
YAML

SMOKE_PORT=18215
start_server "$STAC_PROJ_CONFIG"

STAC_PROJ_ROOT='/public/stac/catalogs/default'
PROJ_URI='https://stac-extensions.github.io/projection/v1.1.0/schema.json'

# (1) the raster collection is on the STAC root at all...
STAC_PROJ_LISTING=$(body_of "$STAC_PROJ_ROOT/collections")
expect_body_contains 'stac /collections (raster tolerance)' "$STAC_PROJ_LISTING" '"gradient"'
expect_body_contains 'stac /collections (raster tolerance)' "$STAC_PROJ_LISTING" '"smoke_points"'
# ...and its document states what the driver read, declared because emitted.
GRADIENT_DOC=$(body_of "$STAC_PROJ_ROOT/collections/gradient")
expect_body_contains 'stac raster collection' "$GRADIENT_DOC" \
  "\"stac_extensions\":[\"$PROJ_URI\"]"
expect_body_contains 'stac raster collection' "$GRADIENT_DOC" '"proj:epsg":[4326]'
expect_body_contains 'stac raster collection' "$GRADIENT_DOC" '"proj:shape":[[32,32]]'
expect_body_contains 'stac raster collection' "$GRADIENT_DOC" \
  '"proj:transform":[[0.08,0.0,-1.28,0.0,-0.08,1.28]]'
# A raster collection still has no items resource — described, not faked.
expect_status "$STAC_PROJ_ROOT/collections/gradient/items" 404

# (2) the vector collection's Items carry the SRID-derived EPSG, and only it.
POINTS_ITEMS=$(body_of "$STAC_PROJ_ROOT/collections/smoke_points/items")
expect_body_contains 'stac vector items' "$POINTS_ITEMS" '"proj:epsg":4326'
expect_body_contains 'stac vector items' "$POINTS_ITEMS" "$PROJ_URI"
expect_body_lacks 'stac vector items' "$POINTS_ITEMS" 'proj:transform'
expect_body_lacks 'stac vector items' "$POINTS_ITEMS" 'proj:shape'

# (3) the vector collection DOCUMENT is untouched: its EPSG lives on its
# Items (where the sidecar override channel and the disagreement log are),
# never as invented collection summaries.
POINTS_DOC=$(body_of "$STAC_PROJ_ROOT/collections/smoke_points")
expect_body_lacks 'stac vector collection' "$POINTS_DOC" 'summaries'
expect_body_lacks 'stac vector collection' "$POINTS_DOC" 'stac_extensions'

stop_server

printf 'PASS: demo smoke, %s checks\n' "$CHECKS"
