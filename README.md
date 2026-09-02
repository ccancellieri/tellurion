# Tellurion

**A native Rust geospatial serving engine.** OGC API — Features and OGC API — Tiles
(MVT + PNG) served through routed, pluggable storage drivers by a single native binary.
Out of the box that's a single `.gpkg` file, no services attached; PostGIS is the
storage driver you reach for at scale.

Tellurion is built in Rust on the conviction that a geospatial API server should be
I/O-bound, not runtime-bound: the storage engine does the geometry work (with PostGIS,
in C), Tellurion moves the bytes with zero-copy discipline, and everything in between —
connection pooling, tile caching, rasterization — runs native with no interpreter and no GC.

**Current status:** v0.4.0 is a release candidate. The serving data plane is the
stabilisation focus; the administrative control plane and remote-source browser are
preview features. Tellurion is self-hosted software—no Tellurion Cloud service is
currently offered. See the [maturity guide](docs/maturity.md) before an evaluation.

Evaluate the product in two ways: run the self-contained GeoPackage quickstart below,
or inspect the [public demonstration gallery](https://ccancellieri.github.io/tellurion-demos/).
The gallery is evidence for its named, bounded journeys; it is not a hosted Tellurion
service or an availability commitment. For evaluation feedback, use the
[project author's GitHub profile](https://github.com/ccancellieri).
To measure your own build and dataset, follow the
[reproducible benchmarking guide](docs/benchmarking.md); the repository does not
publish a context-free capacity number.

## Embedded, self-contained deployment

The default installation is self-contained: a single binary plus a single `.gpkg` file
serves, filters, and writes features — and serves MVT tiles — with no external service
of any kind, via the `geopackage` driver (SQLite, bundled — no system library, no
container runtime). A database service (PostGIS) is optional infrastructure you reach
for at scale or with many concurrent writers, not a requirement to run Tellurion at
all; the `deploy/compose` stack is a development and demo convenience for that optional
path, not the only way to stand the server up. See `tellurion-geopackage`'s own crate
docs for what this driver deliberately leaves out of its first slice (spatial
predicates beyond `S_INTERSECTS`, feature-response CRS reprojection, a derived
search index).

## Quickstart

One command provisions a `.gpkg` file, seeds it with ~500 deterministic synthetic
features, and serves it — no database service, no container runtime, and no network
connection string anywhere in this path:

```sh
cargo build -p tellurion -p tellurion-ingest
target/debug/tellurion-ingest demo
```

`demo` composes the three steps below (provision, seed, serve) into one command: it
runs `geopackage create-tables` + `geopackage seed` against `--path` (default
`demo.gpkg`), then hands off to the `tellurion` binary built alongside it, serving
`config/example-geopackage.yaml` — the same minimal reference config the step-by-step
path below points at — with `TELLURION_GEOPACKAGE_PATH` set to that file. `--port`
passes through to the server the same way `PORT` always has; nothing else is
configurable, on purpose. Re-running it against an existing `demo.gpkg` is safe:
provisioning confirms the existing table rather than re-creating it, and seeding
re-upserts the same 500 rows.

```sh
curl http://localhost:8080/public/features/catalogs/default/collections/demo/items?limit=10

curl http://localhost:8080/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad/0/0/0.mvt \
  -H 'Accept: application/vnd.mapbox-vector-tile' -o tile.mvt
```

### Step by step

For more control than `demo` gives you — a different table shape, writing features
one at a time, standing the server up separately from seeding — run the same three
steps by hand:

```sh
cargo build -p tellurion -p tellurion-ingest

# Creates demo.gpkg with a `demo` point table, its GeoPackage metadata rows, R*Tree
# spatial index, and outbox table. SRID 3857 (Web Mercator) serves the tiles lane
# below natively; the default 4326 also serves tiles, reprojected on the fly.
target/debug/tellurion-ingest geopackage create-tables \
  --path demo.gpkg \
  --table demo \
  --geometry geom \
  --srid 3857 \
  --geometry-type POINT \
  --columns name:TEXT

TELLURION_GEOPACKAGE_PATH=demo.gpkg TELLURION_CONFIG=config/example-geopackage.yaml \
  target/debug/tellurion
```

`config/example-geopackage.yaml` is the minimal reference config for this path — see
its own comments for what each line does. Its `control_store` block uses a separate
SQLite file for durable platform
configuration. The YAML content is imported only while that control store is empty.
The quickstart remains intentionally anonymous: it has no active `auth` section and
sets `allow_empty_platform: true` explicitly. This keeps the unauthenticated curl
commands above coherent while making the empty control-plane authority an explicit
demo-only choice.
Afterward the stored snapshot is authoritative: changing the YAML reports drift at
startup but never overwrites changes already made in the store. Delete or move the
control-store file only when you intentionally want a new first-run import.

For a shared deployment, select PostgreSQL without putting credentials in YAML:

```yaml
control_store:
  backend: postgres
  url_env: CONTROL_DATABASE_URL
  poll_interval_ms: 1000
  pooled_proxy: true

initial_sysadmins:
  - issuer: https://identity.example
    subject: platform-operator

auth:
  trusted_issuers:
    - issuer: https://identity.example
      audience: tellurion-placeholder
      claims: { tenants: tenants }
```

The issuer, audience, and subject above are placeholders and contain no credential.
For a configured static token, an exact durable `urn:tellurion:static` binding grants
the same platform authority without requiring `platform_admin: true`; the existing
`platform_admin: true` flag remains the explicit break-glass path.

`url_env` names the environment variable containing the connection URL. Polling is
the correctness path, including behind Pgpool or another pooled proxy; Tellurion does
not depend on session-bound `LISTEN/NOTIFY`. To retain the former continuously watched
file behavior, select it explicitly with
`control_store: { backend: legacy_file }`.

Each dynamic replica polls at `poll_interval_ms` with up to 10% per-process jitter.
Under healthy operation the maximum detection delay is therefore 1.1 times the poll
interval, and the convergence objective is that delay plus snapshot validation and
activation time. Failures retain the last known-good snapshot and retry with exponential
backoff capped at 30 seconds plus jitter (33 seconds maximum). Operators can observe
`tellurion_control_store_revision`, `tellurion_control_applied_revision`,
`tellurion_control_revision_lag`, activation duration, poll and activation failures,
and the last successful refresh timestamp. None of these metrics uses a
revision-valued label.

Settings backed by boot-lifetime resources are rejected during live activation rather
than being reported as applied while the process still serves the old dependency.
Changes to cache topology, file-backed styles, webhook subscriptions, listener/runtime
limits, or background-consumer wiring therefore require a restart; request-time
settings, tenants, catalogs, collections, routing, and authorization continue to swap
atomically.

The fresh `.gpkg` data file has one empty table. In another terminal, write two
features into it over the real write endpoint
(`PUT`/`DELETE`, backed by the same transactional outbox every driver's write lane
uses — see "Writes" below), then read them back and pull an MVT tile:

```sh
curl -X PUT -H 'Content-Type: application/geo+json' \
  -d '{"type":"Feature","geometry":{"type":"Point","coordinates":[500000.0,6000000.0]},"properties":{"name":"alpha"}}' \
  http://localhost:8080/public/features/catalogs/default/collections/demo/items/1

curl -X PUT -H 'Content-Type: application/geo+json' \
  -d '{"type":"Feature","geometry":{"type":"Point","coordinates":[-500000.0,6000000.0]},"properties":{"name":"bravo"}}' \
  http://localhost:8080/public/features/catalogs/default/collections/demo/items/2

curl http://localhost:8080/public/features/catalogs/default/collections/demo/items?limit=10

curl http://localhost:8080/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad/0/0/0.mvt \
  -H 'Accept: application/vnd.mapbox-vector-tile' -o tile.mvt
```

Rather than write features one at a time, populate the whole table in one step —
`geopackage seed` writes the same deterministic synthetic grid the PostGIS-backed
`seed` subcommand below writes into Postgres, through this driver's own transactional
write path (the same outbox+R*Tree machinery the `PUT`s above go through, not raw SQL):

```sh
target/debug/tellurion-ingest geopackage seed --path demo.gpkg --table demo
```

The `PUT`/`DELETE` calls above are what a single write looks like — worth knowing on
their own, not just a stand-in for a seeding tool that didn't exist yet. For a real
(non-synthetic) dataset, `POST .../items/batch` applies many features per request
in bounded, configurable chunks, streaming a compact per-item outcome back (applied,
refused, or unapplied) instead of one all-or-nothing verdict:

```sh
curl -X POST -H 'Content-Type: application/geo+json-seq' --data-binary @features.geojsons \
  http://localhost:8080/public/features/catalogs/default/collections/demo/items/batch
```

`features.geojsons` is an [RFC 8142](https://www.rfc-editor.org/rfc/rfc8142) GeoJSON Text
Sequence: each Feature is UTF-8, preceded by an ASCII record separator (`0x1e`) and
followed by `LF`, carries its own top-level `id`, and includes the RFC 7946 `geometry`
and `properties` members. It is consumed incrementally rather
than buffered whole; a plain GeoJSON `FeatureCollection` body works too for a small
payload (the CLI caps that buffered form at 64 MiB). Responses contain one outcome per
known item plus a terminal summary: `batch_high_water` names the highest sequence created
by this batch, while `outbox_high_water` is a separate current-primary read. Budget,
transport, and chunk failures explicitly mark an incomplete/unknown tail instead of
claiming full input completion. `tellurion-ingest geopackage load`/`postgis load` drive
the identical chunked apply in-process against a `.gpkg` file or an existing PostGIS
table:

```sh
target/debug/tellurion-ingest geopackage load --path demo.gpkg --table demo features.geojsons
```

The batch route is a Tellurion extension: RFC 8142 standardizes its request sequence,
but the route does not by itself advertise an OGC API Features batch-transaction
conformance class.

### Scaling up: PostGIS

A database-backed storage is the deliberate move once a single `.gpkg` file's
one-writer-many-readers ceiling (see "Embedded, self-contained deployment" above) stops
fitting — more concurrent writers, a larger dataset, or an existing PostGIS estate to
point at. Natively installed Postgres/PostGIS or a URL to one already running both work;
the compose stack below is a development and demo convenience for standing one up
locally, never a requirement:

```sh
# local: PostGIS + tellurion
docker compose -f deploy/compose/docker-compose.yml up -d

# one-off: creates and populates the `demo` table declared in config.yaml
docker compose -f deploy/compose/docker-compose.yml --profile seed run --rm seed

curl http://localhost:8080/public/features/catalogs/default/collections
curl http://localhost:8080/public/features/catalogs/default/collections/demo/items?limit=10
curl http://localhost:8080/public/tiles/catalogs/default/collections/demo/tiles/WebMercatorQuad/0/0/0 -H 'Accept: application/vnd.mapbox-vector-tile'
```

## Raster authoring (COG)

`tellurion-ingest cog author` converts a plain, single-resolution GeoTIFF into a
serving-optimized Cloud-Optimized GeoTIFF: tiled, Deflate-compressed, with a
power-of-two overview pyramid down to a level that fits one tile — same
philosophy as the vector side: ingest owns every physical-layout decision, the
server only ever reads what authoring already produced.

```sh
cargo build -p tellurion-ingest
target/debug/tellurion-ingest cog author \
  --input source.tif \
  --output source-cog.tif \
  --collection my_raster
```

Accepted input: single-IFD (no existing overviews), 8-bit grayscale/RGB/RGBA
(stripped or tiled) or 8-bit paletted/categorical (`PhotometricInterpretation`
= Palette, tiled only), uncompressed or Deflate compression, with EPSG:4326
(WGS84 geographic) georeferencing carried through byte-for-byte from the
source's own tags — the same CRS the `cog` driver requires to serve it.
Anything else refuses by name before writing anything.

Categorical (paletted) input downsamples with nearest-neighbor, not
box-average: averaging class indices would produce a value that names no
real class, so a paletted source auto-selects nearest-neighbor with no flag
needed, and its `ColorMap` tag carries through byte-for-byte to every output
level. `--resample nearest` forces the same kernel for a non-paletted,
single-band (Gray8) class raster that has no embedded palette; `--resample
average` forces box-average and is refused outright against a paletted
source, since there's no correct meaning for it. The `cog` driver expands a
paletted COG's embedded palette to RGBA at decode time, the same as any
other raster it serves.

Bounded memory throughout: the source streams in one tile-row band at a
time, and each overview level builds from the previous level's own tiles,
never a full re-read of the source.

The command prints a `storages:`/`collections:` config snippet naming the
output path via `TELLURION_COG_PATH`, ready to paste in — see the `cog`
driver's own crate docs for its config shape.

### Bounded COG mosaics (`cog-mosaic`)

A `cog-mosaic` storage serves ONE raster TileSet composed from several COGs.
It is deliberately bounded, and every bound below is a refusal by name — never
a truncation, a dropped source, or a partially composed tile:

* **1 to 32 sources**, with unique ids, listed in ascending id order.
* **At most four constituent COGs are read concurrently** for any one tile.
  Four is a maximum, not a target.
* **Composition order is ascending source id**, and a later id paints over an
  earlier one wherever its own pixel is not fully transparent. A tile's bytes
  never depend on which constituent read finished first.
* **All-or-error**: if any selected source's read fails, the whole requested
  tile fails. A partially composed tile would be byte-indistinguishable from
  legitimate transparency, which is silent corruption.
* The same per-request pixel budget as the single-COG driver, charged **once**
  across every selected source — not once per source.
* Sources must be local files; an `http(s)` source is refused by name, because
  verifying its recorded SHA-256 means reading all of its bytes.

The manifest is a **sidecar authored by `ingest`**, referenced by the storage's
own locator — never a hand-written block in `config.yaml`. `tellurion-ingest
cog mosaic` scans the constituent COGs and MEASURES each one's bbox (from its
own georeferencing tags), byte length (from the object) and SHA-256 (from the
bytes themselves):

```sh
target/debug/tellurion-ingest cog mosaic \
  --source tile_a.tif --source tile_b.tif --source tile_c.tif \
  --output mosaic.yaml \
  --collection my_mosaic
```

The provenance fields are therefore measured, not declared — a SHA-256
transcribed by hand into YAML is an error nobody notices until the day it
matters. The server validates the manifest it is given (source count,
uniqueness, ordering, bbox shape, and every source's recorded length, digest
and bbox against the real object) and refuses by name if it does not hold. It
never authors or repairs a manifest, and it issues no DDL. A mosaic collection
serves PNG raster tiles and refuses MVT as an unsupported capability, exactly
as a single-COG collection does.

## What it serves

| Protocol | Conformance | Formats |
|---|---|---|
| OGC API — Common | landing page, `/conformance`, `/api` | JSON |
| OGC API — Features Part 1 + Part 2 + Part 3 | Core + GeoJSON (`bbox`, `datetime`, `limit`, keyset paging); CRS (`crs`, `bbox-crs`); Queryables + Queryables as Query Parameters; CQL2 filtering in both `cql2-text` and `cql2-json` (basic, basic spatial functions), including `filter-crs` wherever the driver can transform a filter's spatial literals (PostGIS) and a named 400 wherever it cannot; per-collection queryables document | GeoJSON |
| OGC API — Features Part 4 | transactional item writes: `PUT`/`DELETE` on `/collections/{cid}/items/{fid}`, `POST` create (server-assigned id) on `/collections/{cid}/items`, backed by a transactional outbox; optimistic locking via `ETag`/`If-Match` (per-collection, wherever the write lane can re-verify the precondition inside its own write transaction — PostGIS; refused by name elsewhere rather than checked racily) and `Last-Modified`/`If-Unmodified-Since` (per-collection, only where a real modification-timestamp column is declared; nothing in the server writes that column, and `tellurion-ingest locking install-touch-trigger` optionally provisions the PostGIS trigger that maintains it — opt-in, and every other driver refused by name) | GeoJSON |
| OGC API — Tiles Part 1 | WebMercatorQuad; vector collections serve MVT + PNG, raster collections (tiled Cloud-Optimized GeoTIFF or Zarr v2, either a single array or a multiscale pyramid, pure-Rust readers) serve PNG | MVT, PNG |
| OGC API — Styles | read-only: `/styles`, `/styles/{styleId}`, `/styles/{styleId}/metadata`, plus per-style map tiles | MapLibre Style JSON, PNG |
| 3D Tiles 1.1 | `tileset.json` (implicit quadtree tiling) + tile content, either extruded footprints or true solid geometry (`VolumeSource`) | glTF binary (.glb) |
| STAC API | Core + Collections + OGC API Features + Item Search (`/search`), including Item Search filtering with the same CQL2 classes as Features — `filter-crs` honoured on the one value the STAC Filter Extension pins it to (CRS84, spelled the same way on a `GET` query string and in a `POST` body) and a named 400 for any other, with the Item Search Filter class declared only where a driver can genuinely compile a filter | JSON, GeoJSON |

The PNG lane is MVT-first: tiles are produced once as MVT (PostGIS `ST_AsMVT`), cached,
and rasterized on demand — one cache feeds both formats. 3D places (extruded footprints,
or true solid geometry where a driver exposes it) and styled map tiles reuse the same
MVT-first cache with an extra encoding variant. Raster collections bypass the MVT step:
a tiled, EPSG:4326 Cloud-Optimized GeoTIFF with overviews serves PNG tiles directly
(overview selection, windowed reads, a hard per-request pixel budget), and an MVT
request against a raster collection is a clean capability refusal.

A collection served over both Features and STAC can carry per-item STAC metadata
that has no place in its own feature properties: declaring `stac_metadata: true`
makes the STAC lane — and only the STAC lane — read a per-collection
`"<table>_stac"` sidecar (`tellurion-ingest stac create-tables` provisions it, the
same DDL-ownership rule the outbox, index and asset-records tables follow) and
merge each item's stored document into its Item. Precedence is documented and
one-directional: the sidecar wins over a colliding `properties` key (that is why
it exists — it must be able to correct, not only add), structural members the
lane derives (`id`, `type`, `geometry`, `bbox`, `collection`, `links`, `assets`,
`stac_version`) are never settable from it, and the Features lane never reads the
table at all, so an override can never change what OGC API Features serves for the
same row. The lookup is batched per page (`feature_id = ANY(...)`): one extra
round trip per page, never one per item. A collection that never opts in serves
byte-identical Items; one that opts in without provisioning the table gets a named
refusal, never a silently empty merge.

Per-item *assets* work the same way, over the asset-records table the assets API
already owns. A harvested multi-item collection whose scenes each have their own
source COG or Zarr store declares `stac_item_assets: true`, and the STAC lane —
again, only the STAC lane — projects that collection's item-scoped records
(`"<table>_assets"`, provisioned by `tellurion-ingest assets create-tables`) into
each Item's own `assets` object. No new capability and no new table: this reads
the same `AssetRecordStore` the assets API writes, through one batched
`item_id = ANY(...)` lookup per page — one extra round trip per page, never one
per item — served by the leading column of the index that table already carries.
The rules are explicit: only `available` records are advertised (a pending upload
has no bytes yet and a failed one never will, so neither is offered as a usable
asset), a managed record resolves to the stable `.../assets/{key}/data` resource
while a remote record keeps its external href verbatim, a persisted record wins a
key collision against a capability-derived entry (the mvt/png/glb templates), and
a collection-scoped record stays at Collection scope — it is never flattened onto
every Item. A collection that never opts in serves byte-identical Items; one that
opts in against storage with no asset records, or without provisioning the table,
gets a named refusal.

## Writes

`PUT`/`DELETE /collections/{cid}/items/{fid}` (replace/create-by-caller-
supplied-id, and remove) and `POST /collections/{cid}/items` (create-only,
server-assigned id — a `201` with a `Location` header pointing at the new
item) all write through the same transactional outbox rather than the
storage table directly: a write commits its row change and an outbox
obligation in one transaction, and the outbox is what a downstream consumer
(search index, derived cache) drains.

A collection's primary-key value-space is declared with `id_type`. The
PostGIS driver is where a non-default value actually does something:

```yaml
collections:
  - id: demo
    table: demo
    pk: id
    id_type: uuid   # bigint (default) | uuid | text
```

- `bigint` (the default, and every collection that predates this field) —
  the pk is a `bigserial`/`serial`-backed integer column. Item ids and
  keyset paging tokens parse and compare as `i64`. A `POST` mints a new id
  from the column's own `DEFAULT nextval(...)`, read back via the same
  statement's `RETURNING` clause.
- `uuid` — the pk is a `uuid`-typed column with a server-side default
  (typically `DEFAULT gen_random_uuid()`). Item ids and keyset paging tokens
  parse and compare as `uuid`, never as a string standing in for one:
  keyset paging orders `ORDER BY pk::uuid`, not `pk::text`. A `GET` id
  that isn't a syntactically valid UUID never matches anything (a plain
  `404`, same as an out-of-range integer id on a `bigint` collection); a
  `PUT`/`DELETE` id that isn't one is refused by name as an invalid feature
  id (`400`) rather than silently treated as a no-op, since a caller-supplied
  id on a write is a mistake worth naming. A `POST` mints a new id the same
  omit-from-`INSERT`+`RETURNING` way `bigint` does, just from the column's
  own UUID default instead of a sequence. A collection declaring `id_type:
  uuid` over a table whose pk column isn't actually `uuid`-typed is refused
  by name (not served, and not a partial write) the first time a `POST`
  reaches it; a pk column of the right type but with no server-side default
  to mint from is refused by name too, rather than either case surfacing as
  a raw SQL error.
- `text` — the pk is a `text`/`varchar`-typed column, deliberately with NO
  server-side default expected: unlike `bigint`/`uuid`, `POST` create is
  CALLER-supplied, not server-minted. Send the id as the feature body's own
  top-level `id` member; omitting it is refused by name (`400`) before any
  SQL runs, and an id that's already in use is a named `409`, never a raw
  constraint-violation error. Either way the id comes back via the same
  `INSERT ... RETURNING` statement `bigint`/`uuid` use, so what the response
  reports is exactly what the database stored. `PUT`/`DELETE` need no
  special handling — they've always taken the id from the URL, the same
  caller-supplied shape `text` needs. Keyset paging pins an explicit
  `COLLATE "C"` (byte order) on the pk comparison and `ORDER BY`, rather than
  trusting whichever collation the database happens to default to: a plain
  `text` comparison's ordering is otherwise locale-dependent, so the same
  table could page differently on two databases with different locales.
  A collection declaring `id_type: text` over a table whose pk column isn't
  actually `text`/`varchar`-typed is refused by name the same way a `uuid`
  mismatch is.
- Composite (multi-column) primary keys are not supported, for any
  `id_type`.
- Vector tiles never carry a wire-format (unsigned-integer) MVT feature id
  for any collection, `bigint`, `uuid`, or `text`: the pk is always exposed
  as an ordinary text attribute (tag) on the tile feature instead, so a
  non-`bigint` pk needs no special handling in the tile lane at all.
- The embedded GeoPackage driver's primary key is always `INTEGER` — a
  format-level requirement, not a gap. `id_type` stays `bigint` (the
  default) for every GeoPackage-backed collection; declaring `uuid` or
  `text` against one is refused by name the first time an id reaches that
  driver.

The derived-index half of the write contract ships too: an `IndexSink`
applies outbox obligations to a per-collection index table idempotently and
in order (version-gated, halt-don't-skip), driven by a config-gated
background applier that is off by default. The `tellurion-ingest` CLI owns
provisioning all of it — the relational registry, per-collection outbox
tables, and the index tables (`tellurion-ingest registry create-tables`,
`tellurion-ingest outbox create-tables`, `tellurion-ingest index
create-tables`) — the server itself never runs DDL and refuses cleanly if a
collection points at an index that was never provisioned.

`tellurion-ingest harvest stac <root> --tenant <t> --catalog <c>` walks a
remote STAC API (`GET /collections`, then each collection's items through
`rel=next`) and upserts every item through that same canonical write lane, so
outbox obligations, the derived index and invalidation fire exactly as they do
for any other write. It creates nothing: a remote collection with no
already-published local counterpart is refused by name, remote assets stay
href-only (counted and reported, never fetched), and item properties are
projected onto what the target declares. Because a harvest is a write replay,
a deployment's own STAC surface is a valid source — re-harvesting a catalog
from itself is the supported way to rebuild a derived index against the
current DDL, resumable through `--bookmark` and idempotent by construction.
The report is NDJSON: an id-mapping line per collection, per-item outcomes,
and a summary. `--dry-run` prints the mapping without fetching an item.

External consumers read the same log through
`GET /collections/{cid}/changes`: compact versioned envelopes, keyset-paged
by outbox sequence, never mutation payloads. Optional webhook subscriptions
deliver those envelopes with HMAC-SHA256 signatures and bounded retry.
Platform admins enumerate subscriptions at `GET /config/webhooks` and page
each running subscription's bounded dead-letter ring at
`GET /config/webhooks/{id}/dead-letters`. Subscription edits use the audited,
versioned `PUT /config` path; unchanged subscriptions keep their delivery
cursors across the live rebind, while removed collection pairs are cancelled
and stop holding the consumer-aware retention floor back.

`settings.max_request_body_bytes` caps a `PUT`/`POST` item body before it is
buffered into memory, checked against the streamed length rather than after
the fact. It inherits through the same platform -> tenant -> catalog ->
collection chain as `slow_request_ms`; the nearest declared value wins, and
the module default (1 MiB) applies when nothing in the chain sets it. An
over-limit body is refused with a named `413 application/problem+json`
response before `WriteSink::apply` is ever reached.

## Assets

`GET`/`PUT`/`DELETE /collections/{cid}/assets/{key}` and the item-level
`.../items/{fid}/assets/{key}` register and read a STAC 1.1 Asset Object as a
keyed sub-resource; `GET`/`PUT .../assets/{key}/data` carries the bytes at a
separate URL. An asset registers as either **remote** (an external `href`,
metadata only — deleting it never touches someone else's bytes) or
**managed**: send an RFC 9530 `Repr-Digest: sha-256=:...:` header on the
registering `PUT` and the server tracks it through an explicit
pending -> available -> failed lifecycle, verifying the declared digest
against the uploaded bytes before marking it available; a mismatch fails the
asset by name rather than serving unverified bytes. A managed asset needs a
collection's `object_store` (an id from the top-level `object_stores:` block:
`profile: fs`, or `profile: s3` — any S3-compatible endpoint, with presigned
upload/download and multipart upload, over a hand-rolled SigV4 signer, no
vendor SDK) — a first-class config concept, deliberately never `storages`
above, which is where *feature* data lives, not asset bytes. Object keys
derive only from the asset's own server-generated internal id, never a
client-supplied filename, so path traversal on the `fs` profile is
impossible by construction. `settings.max_asset_bytes` and
`settings.asset_media_types` gate a registration the same way
`max_request_body_bytes` gates a feature write — named `413`/`415` refusals
before any storage I/O. The database-backed driver owns the per-collection
asset-records table; `tellurion-ingest assets create-tables` provisions it,
the same DDL-ownership rule the outbox and index tables already follow.

## Search routing

A collection's `search:` lane names an ordered chain of storages
(`routing.search: [index, main]`): only the first entry is ever measured for
freshness, comparing the write lane's outbox high-water against that entry's
own derived-index high-water; every later entry is always a degraded,
unconditional read from its plain feature source, never a second freshness
attempt. `search.freshness_bound` (sequence-lag units, default `0`) sets how
stale the index may be before the lane prefers the fallback. Routing search
at a storage the collection never provisions via `routing.index` is a named
config-load refusal, not a silent no-op.

## Authentication and authorization

`auth:` (OIDC/JWT bearer tokens) and `policy:` (RBAC role grants, optionally
narrowed by a CQL2 ABAC filter pushed down into the driver's own query) are
both optional and absent by default — a deployment that doesn't configure
them runs fully open, same as before. Writes go through the same checkpoint
as reads: a grant names the `write` lane explicitly (never implied by, and
never implying, any read lane), and a `write` grant carrying a filter is
refused at config load rather than silently ignored — a grant is either
enforced or refused, never dropped. See `config/example.yaml` for the
commented reference shape and
`docs/design/2026-07-18-authorization-policy-layer.md` for the design.

Human identities can be verified against multiple platform-approved
`auth.trusted_issuers`. The unverified JWT issuer is used only to select an
already configured validator; signature, audience, expiry and required `sub`
checks complete before the `(issuer, sub)` identity is usable. Dynamic control
snapshots bind that exact identity to roles. Only a stored `sysadmin` binding at
platform scope grants control-plane administration. Raw tenant/role claims are
inert unless an issuer explicitly enables its registered claim mapping. SAML
remains an upstream concern of an OIDC broker, keeping one token
validation path inside Tellurion. The singular `auth.oidc` form remains
available for existing configurations.

## Design principles

- **Routing is the core concept.** A router resolves `(tenant, catalog, collection)` to
  capability traits (feature source, tile source); storage backends are pluggable drivers
  behind those traits, and configuration/style persistence sit behind store traits of their
  own. Protocol handlers carry zero database dependencies — PostGIS is the first driver,
  not a requirement.
- **The driver's engine does the geometry.** With the PostGIS driver, MVT encoding,
  simplification, and spatial filtering happen in the database; the server never
  re-implements what the engine already does in C.
- **Byte-budgeted caching.** The in-process tile cache is sized as a percentage of
  container memory, never by entry count.
- **Bounded everything.** Per-zoom feature caps, a per-tile vertex budget on top of them
  (`settings.tile_vertex_budget` — a single dense geometry can cost more than thousands
  of simple ones, so feature count alone isn't the whole story), request timeouts with
  a hard ceiling, cancellation on client disconnect, connection pools derived from
  cgroup limits. Under overload Tellurion queues and sheds gracefully — it does not
  fall over.
- **Behavior lives in configuration**, not environment variables. Env vars carry
  infrastructure only (`DATABASE_URL`, `PORT`).
- **Observable by default.** Structured tracing and a Prometheus `/metrics` endpoint,
  including process RSS — native memory is measured, not assumed.

## Cargo features

| Feature | Default | Driver crate | Capabilities |
|---|---|---|---|
| `postgis` | on | `tellurion-postgis` | catalog, features, tiles, writes, outbox, derived index, search, volumes |
| `geopackage` | on | `tellurion-geopackage` | catalog, features, tiles, writes, outbox — embedded, single `.gpkg` file, no service |
| `pmtiles` | off | `tellurion-pmtiles` | catalog, tiles (read-only archive) |
| `flatgeobuf` | off | `tellurion-flatgeobuf` | catalog, features (read-only) |
| `geoparquet` | off | `tellurion-geoparquet` | catalog, features (read-only) |
| `cog` | off | `tellurion-cog` | catalog, raster tiles (read-only) — registers both `cog` (one GeoTIFF) and `cog-mosaic` (a bounded manifest of them) |
| `zarr` | off | `tellurion-zarr` | catalog, raster tiles (read-only) |
| `iceberg` | off | `tellurion-iceberg` | catalog, features (read-only) — REST catalog; table files on the local filesystem or any S3-protocol store |
| `duckdb` | off | `tellurion-duckdb` | catalog, features (read-only), embedded analytical engine |
| `ui` | off | — | embeds the demo UI (`ui/dist`) into the binary |
| `valkey` | off | — | L2 tile-cache backend |

The `tellurion` server crate's `postgis` feature pulls in the PostGIS driver crate.
Protocol crates and `tellurion-core` never depend on it, so the claim in "Design
principles" above is a build you can run, not just an assertion:

```sh
cargo build --workspace --no-default-features
```

builds every crate — including the server binary — with zero database dependency in
the resulting server build graph. A config naming `driver: postgis` still
fails fast at boot in that build (unknown driver), same as any other driver typo.

The same standalone-build proof applies to every other driver/backend feature:
`geopackage`, `pmtiles`, `flatgeobuf`, `geoparquet`, `cog`, `iceberg`, and `duckdb` each
turn on standalone (`--no-default-features --features <name>`) to demonstrate that one
driver serves traffic with every other driver — including PostGIS — compiled out of the
graph. Each feature has its own CI feature-matrix job and a serves-without-the-database
proof test.

## Storage registry and drivers

`registry.backend` selects where catalog/collection declarations come from: `file`
(the default, read once from YAML at boot) or `relational`, which reads them from a
storage the `tellurion-ingest registry` subcommands provision and publish to. The
`tellurion-memory` crate is a small, immutable, GeoJSON-backed reference driver — a
documentation and contract-test fixture for the storage capability contract, not a
production backend the server registers. See `docs/driver-authoring.md` for how to
write a new driver against that contract.

### The DuckDB driver

`tellurion-duckdb` serves OGC API Features straight out of a local `.duckdb` file —
the embedded *analytical* counterpart to the embedded *transactional* GeoPackage
driver above: columnar scans, no database service, bundled engine (no system
`libduckdb`). It never loads DuckDB's `spatial` extension: that extension is fetched
over the network on first `INSTALL spatial`, which this "single binary next to a
local file" driver refuses to depend on even implicitly, so a collection's geometry
column is a plain WKB `BLOB` decoded in Rust (via `geozero`, the same library
`tellurion-geoparquet` already decodes WKB with) rather than DuckDB's own native
`GEOMETRY` type. See `tellurion-duckdb`'s own crate docs for the full reasoning and
for how a table with more than one `BLOB` column needs an explicit `geometry:`
override to disambiguate.

```yaml
storages:
  - id: warehouse
    driver: duckdb
    url_env: TELLURION_DUCKDB_PATH

collections:
  - id: buildings
    catalog: default
    storage: warehouse
    table: buildings       # defaults to the collection id when omitted
    geometry: geom         # required only when the table has more than one BLOB column
    pk: building_id        # required only when the table declares no PRIMARY KEY
```

`tellurion-iceberg` serves OGC API Features out of an Apache Iceberg table resolved
through a **REST catalog** — no SQL catalog, no Glue, no database of its own. It is
read-only: ingest owns all DDL and all physical layout, and this driver never writes
to a table's storage, on any backend.

Everything the driver needs beyond the catalog URL travels in a single locator string
held in the environment variable the storage's `url_env` names — never in
`config.yaml`. Iceberg has no native geometry type, so the geometry column (WKB bytes)
and its four covering bbox columns are operator declarations:

```yaml
storages:
  - id: lake
    driver: iceberg
    url_env: TELLURION_ICEBERG_LOCATOR
```

```sh
# Local-filesystem table:
export TELLURION_ICEBERG_LOCATOR='http://catalog:8181?namespace=geo&table=points&geometry=geom&bbox=xmin,ymin,xmax,ymax'
```

Table files may live on the local filesystem or on **any store speaking the S3
protocol** — AWS S3, MinIO, Ceph RGW, Cloudflare R2 — read through this workspace's
own object-store port and its hand-rolled SigV4 signer rather than a vendor SDK. An
S3-backed table adds four declarations, of which the last two are the *names* of
environment variables holding the credentials; no credential is ever read from
`config.yaml`, which has no field to hold one:

```sh
export TELLURION_ICEBERG_LOCATOR='http://catalog:8181?namespace=geo&table=points&geometry=geom&bbox=xmin,ymin,xmax,ymax&s3_endpoint=https://s3.eu-west-1.amazonaws.com&s3_region=eu-west-1&s3_access_key_env=AWS_ACCESS_KEY_ID&s3_secret_key_env=AWS_SECRET_ACCESS_KEY'
export AWS_ACCESS_KEY_ID=... AWS_SECRET_ACCESS_KEY=...
```

**GCS and ADLS are not supported.** Their native APIs are not the S3 protocol, and a
table on `gs://` or `abfss://` is refused **by name** when the table is loaded —
naming the scheme found and saying it is not implemented — never silently degraded to
another backend and never left to fail as an opaque read error later.

A gated live test exercises the driver against a real, external REST catalog:
set `TELLURION_ICEBERG_LIVE_TEST_LOCATOR` to a complete locator and run
`cargo test -p tellurion-iceberg --test live`. With the variable unset it skips (and
passes), the same convention `tellurion-postgis`'s own live tests use.


## Demo UI

A static HTML5 demo (`ui/`) with one panel per protocol — features, MVT, PNG, styled
tiles, and 3D places, plus a `/metrics` status widget — built with Vite, TypeScript, and
vanilla Custom Elements (no framework runtime), MapLibre GL JS for the 2D map, and
deck.gl (MapLibre-interleaved) for 3D Tiles. UI sources live in `ui/src`, and each panel
is an independent element, so protocol views can be changed without touching the others.

Start a local Tellurion server and the UI together from the repository root:

```sh
./scripts/run-local.sh
```

The command builds the server and demo tool, installs the pinned UI dependencies, creates
a temporary GeoPackage-backed example, and removes that temporary data when it exits.
It stops both processes when either one exits or when you press `Ctrl+C`.

To retain the demo GeoPackage, select ports, or run an existing configuration instead:

```sh
TELLURION_GEOPACKAGE_PATH="$PWD/demo.gpkg" \
TELLURION_PORT=8081 \
TELLURION_UI_PORT=4174 \
./scripts/run-local.sh

TELLURION_APP_CONFIG=/path/to/config.yaml ./scripts/run-local.sh
```

Paths may be absolute or relative to the directory from which the command is invoked.

Run it against a live server during development (proxies API calls to `:8080`):

```sh
cd ui
npm ci
npm run dev
```

Build the static bundle:

```sh
cd ui
npm ci
npm run build   # outputs ui/dist
```

Embed it in the server binary and serve it at `/ui` (default-off `ui` feature):

```sh
cargo build -p tellurion --features ui
```

Building with `--features ui` before `ui/dist` exists fails fast with a message naming
the `npm ci && npm run build` step above — the embed has nothing to embed otherwise.

## Deployment pyramid

One codebase, four artifacts — pick the layer that fits:

1. **Static binary + systemd unit** (`deploy/systemd/`) — appliance installs and clean benchmarks.
   `deploy/systemd/ha/` adds the two-node, VIP-fronted variant: keepalived plus one shared
   PostgreSQL, no cluster software anywhere.
2. **OCI image** (`docker/Dockerfile`) — non-root, arbitrary-UID; runs under Docker and Podman.
   `deploy/podman/tellurion.container` is the rootless Podman Quadlet unit — containers
   supervised by plain systemd, no daemon.
3. **Compose** (`deploy/compose/`) — local development and single-node deployments.
4. **Kubernetes manifests + kustomize overlays** (`deploy/k8s/`) — k3s, EKS, GKE, AKS,
   OpenShift/OKD (restricted SCC compatible), and a vendor-neutral Gateway API overlay,
   plus an opt-in `ha` component (PodDisruptionBudget + replicas) and an HPA example. The
   same YAML works with `podman kube play`.

The target is the conformance floor, not any one distribution: a CNCF-conformant
Kubernetes API, Pod Security Standards `restricted` (asserted in CI over every rendered
overlay), portable ingress, and a published air-gap image list — so Rancher/RKE2, Talos,
Tanzu, EKS Anywhere and the rest work from the same manifest set. `deploy/nomad/` carries
an example Nomad job for the same reason: a static binary needs no particular
orchestrator. See `docs/deployment-topologies.md`, including what to know before running
more than one replica.

## Production operations

`GET /healthz` is dependency-free liveness: it returns 200 whenever the event loop can
serve a request. `GET /readyz` is bounded dependency readiness: it returns 200 only after
the current registry and every configured storage have passed their latest probe, and while
the process is not draining. Initial probing, a timeout or failure, and draining return a
generic 503 Problem Details response; dependency details stay in transition logs.

The optional L2 cache is deliberately excluded from that verdict: a failed L2 tier already
degrades to the in-process L1 cache, every request is still answered correctly, and marking
the process unready would pull a serving instance out of rotation exactly when the cache
stopped absorbing load. It is not excluded from the *report*, though. When — and only when —
an L2 tier is configured and its latest probe failed, the still-200 readiness response names
it:

```json
{"status":"degraded","degradations":[{"component":"cache.l2","backend":"valkey","reason":"unreachable"}]}
```

`reason` is one of `unreachable`, `probe-timeout`, or `never-connected-at-boot`; the
backend's own error text stays in the logs. A deployment that configured no `cache.l2`, and
one whose tier is healthy, both get the same empty 200 they always got — and the companion
`tile_cache_l2_available{backend}` gauge exists only where a tier is configured, so an alert
on it can never fire for a cache nobody asked for.

The related YAML behavior keys and defaults are:

```yaml
server:
  drain_timeout_s: 10
  readiness_probe_interval_s: 5
  readiness_probe_timeout_s: 2
  metrics_tenant_allowlist: []
  metrics_collection_allowlist: []
settings:
  slow_request_ms: 1000
```

`slow_request_ms` inherits independently from platform through tenant and catalog to
collection; the nearest declared value wins. On SIGINT or SIGTERM, Tellurion first makes
readiness false, stops accepting new connections, waits for in-flight work, and exits
when it completes or the drain deadline expires. The base Kubernetes deployment uses a
15-second termination grace period: the default ten-second drain plus five seconds for
process shutdown. Keep that same margin when increasing `drain_timeout_s`. The image
health check calls `/healthz`, not `/readyz`, because Docker exposes a single health state
and a transient downstream outage should not be mistaken for a dead process.

Configuration reload is triggered by `SIGHUP` and by a filesystem watch on the config
file's *directory* — not its inode, so a Kubernetes ConfigMap symlink swap is still seen.
Triggers are debounced, then the document is re-read and fully validated before anything
is swapped: a bad edit is logged by name and the previous configuration keeps serving.

A reload whose document is byte-for-byte identical to the one already serving is not
activated, on the default `registry.backend: file`. That watch cannot filter by filename, so every write to any file in the same
directory — a log file the process itself is appending to, an editor's swap file, a
ConfigMap restaged with unchanged content — arrives as a reload attempt, and every
activation resets the readiness probe generation, dropping `/readyz` to 503 until the next
probe lands. A `config.yaml` sitting beside a busy log file could therefore keep an
entirely healthy instance flapping out of load-balancer rotation. The consequence worth
knowing: **`touch config.yaml` no longer forces a recycle.** Change the document to reload
it; restart the process to recycle it.

Under `registry.backend: relational` every trigger still activates, and deliberately so:
the catalog, collection and tenant tables live outside the document, so a reload against
an unchanged file is exactly how an operator forces those tables to be re-read. Only the
file backend makes the document the whole input to a reload.

A declined activation is never silent. It logs at INFO, naming the path and the version,
and increments `tellurion_config_reload_skipped_unchanged_total`. That counter climbing
while the `tellurion_config_version` gauge holds still is exactly the signal that
something in the config directory is churning without the configuration itself changing.

Per-tenant admission control sits ahead of the concurrency ceiling above, before routing
ever resolves a catalog or collection: a small, bounded queue with a deadline lets a brief
burst wait for a fair-share slot instead of being shed outright, and every tenant's share
of `server.max_concurrency` is a weighted slice of that same ceiling — equal shares by
default — so no single tenant can monopolize it. A rejection, whether the queue was
already at capacity or the deadline elapsed while waiting, answers the same problem+json
shape as every other error, with `Retry-After`. Configured through the same settings
chain as `slow_request_ms`, but consulted only at the platform and tenant levels, since
admission runs before a catalog or collection is ever resolved:

```yaml
settings:
  admission:
    queue_capacity: 32
    queue_deadline_ms: 250
    weight: 1
tenants:
  - id: public
    settings:
      admission: { weight: 2 }
```

`tenant_admission_queue_depth` (a gauge) and the `tenant_admission_admitted_total`,
`tenant_admission_rejected_total`, and `tenant_admission_deadline_expired_total` counters
are labeled by `tenant`, bounded by the same `metrics_tenant_allowlist` described below.

The `http_request_duration_seconds` histogram labels requests by `method`, matched
`path`, `status`, `lane`, `tenant`, and `collection`. Lanes are the fixed set `features`,
`tiles`, `mvt`, `png`, `styled_png`, `places3d`, `styles`, `stac`, `control`, and
`unmatched`. Unmatched paths use the fixed `unmatched` path; a tenant uses its resolved
external id only when listed in `server.metrics_tenant_allowlist`, otherwise `other`.
Control routes use `none`, and an unresolvable tenant uses `unknown`. A collection is
labeled by its fully qualified external `tenant/catalog/collection` only when listed in
`server.metrics_collection_allowlist`; all other collections use `other`, and routes
without a collection use `none`. The tenant-label ceiling is therefore the tenant
allowlist size plus three, and the collection-label ceiling is the collection allowlist
size plus two. Raw paths, query values, credentials, feature/style ids, storage or table
names, internal ids, and errors never become labels.

Aggregate away `collection` for stable dashboards. For example, per-tenant/lane p95
latency and request rates by status are:

```promql
histogram_quantile(
  0.95,
  sum by (le, lane, tenant) (rate(http_request_duration_seconds_bucket[5m]))
)

sum by (status, lane, tenant) (
  rate(http_request_duration_seconds_count[5m])
)
```

When elapsed time is strictly above the effective threshold, exactly one
`event=slow_request` warning records `method`, matched `route`, `lane`, `status`, public
external `tenant`/`catalog`/`collection`, `elapsed_ms`, and the exclusive
`routing_ms`, `query_ms`, `cache_ms`, and `encode_ms` phases. Routing covers external-id
and capability resolution; query covers registry, style, feature, tile, and volume-source
calls; cache covers lookup, coalescing, and cache writes; encode is the non-overlapping
response-production residual. The event omits the raw URI and query string, headers and
credentials, feature ids, physical names, internal ids, and error text. Requests at or
below the threshold do not emit it.

## Licensing

Tellurion 0.4.0 is open-source software under the GNU Affero General Public License
Version 3 (`AGPL-3.0-only`). Commercial use is allowed under that licence. If you
modify Tellurion and let users interact with the modified version over a network,
Section 13 requires a prominent offer of the corresponding source to those users.

Organisations that cannot comply with the AGPL may discuss separate commercial terms
for closed modifications or proprietary redistribution.
Tellurion Cloud is not currently offered: the Community edition is evaluated and
deployed on infrastructure you control.

This summary does not replace the [LICENSE](LICENSE). Read the concise
[licensing guide](docs/licensing.md) and [commercial licensing notice](COMMERCIAL-LICENSE.md).
External source contributions remain paused until a reviewed contributor agreement is
available; issues and reproducible bug reports are welcome—see [CLA.md](CLA.md) and
[CONTRIBUTING.md](CONTRIBUTING.md).

Tellurion is an independent project created and owned by Carlo Cancellieri.
Copyright © 2026 Carlo Cancellieri. See the [copyright notice](COPYRIGHT.md).
