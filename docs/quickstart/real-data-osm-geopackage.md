# Serving real OpenStreetMap data from a single GeoPackage

This walks through loading a real, unmodified OpenStreetMap extract into a
`.gpkg` file and serving it with `tellurion --no-default-features --features
geopackage` — no database service, no container runtime. It records exactly
what was run to produce the config in this repository
(`config/example-italy-isole-osm-geopackage.yaml`), including a real problem
that came up serving PNG tiles for this dataset and how it was worked
around, so the steps below are reproducible rather than a description of
something that was only tried in theory.

## What in this guide is verified, and what is not

The original walkthrough was run end to end on Linux x86_64 against the real
extract. The recorded timings and response sizes belong to that run and are not a
benchmark of the current release candidate. Two boundaries are worth knowing:

- **The dataset is a regional subset**, Sicily and Sardinia rather than all
  of Italy. That is a deliberate scope choice, explained in the next
  section — it is real, unmodified OSM data, just not the whole country.
- **The dataset exposed a real polygon-winding defect.** That defect is fixed in
  the current driver and protected by regression tests; the old data-side
  normalization workaround is no longer part of this guide.

Two adjacent pages carry their own caveats: the Windows build steps in
`install.md` have not been executed, and QGIS's native OGC API raster
source in `qgis.md` is unverified (that page's XYZ connection route is the
one that was actually tested).

## Attribution and license (read this first)

The data is © OpenStreetMap contributors, licensed under the Open Database
License (ODbL) 1.0. Any use, redistribution, or on-screen display of it —
including every tile screenshot in this repository's demo material — must
carry that attribution. See <https://www.openstreetmap.org/copyright>.

## The dataset: a regional subset, not all of Italy

The task this config was built for asked for a real Italy OpenStreetMap
extract, pointing at Geofabrik's `italy-latest.osm.pbf` (about 2.06 GiB
compressed: `Content-Length: 2211988935` bytes at the time of writing,
verified with `curl -sI`). That full extract was not used. Converting it
end-to-end into a single-writer SQLite GeoPackage — plausibly tens of
gigabytes once its `lines` and `multipolygons` layers (dense roads,
waterways, land use, and essentially every building footprint in the
country) are expanded into GeoPackage's row-per-feature form — was not a
realistic amount of ingest time or disk to spend inside one working
session, and the on-premise/embedded story this config demonstrates does
not need all of Italy to be a real, honest demonstration.

Instead, this uses Geofabrik's **"isole" (islands) regional sub-extract**
of Italy — Sicily and Sardinia, and their coastal waters:

- Source: `https://download.geofabrik.de/europe/italy/isole-latest.osm.pbf`
  (resolved, at download time, to `italy/isole-260719.osm.pbf`; Geofabrik
  extracts are refreshed daily and the dated filename will differ on a
  later re-download)
- Downloaded size: 213,054,753 bytes
- MD5 (verified against Geofabrik's own `.md5` sidecar file):
  `580b377311b38fe4ef4910ff06dfcf2a`
- This is real, unmodified, unfiltered OpenStreetMap data for a genuine,
  substantial geographic region (two large islands) — not a hand-picked
  bounding box and not synthetic data. It is smaller than the full Italy
  extract, and that is the honest scope of this demo: say so, don't
  silently present it as "Italy."

If you want the full Italy extract, or a different regional sub-extract,
substitute its URL in the download step below — Geofabrik lists Italy's
other regional splits (`centro`, `nord-est`, `nord-ovest`, `sud`) at
<https://download.geofabrik.de/europe/italy.html>, alongside the full
country file.

## Step 1: download

```sh
mkdir -p demo-data/italy-isole-osm && cd demo-data/italy-isole-osm

curl --fail --location --retry 3 --retry-delay 2 --continue-at - \
  --output italy-isole-latest.osm.pbf \
  "https://download.geofabrik.de/europe/italy/isole-latest.osm.pbf"

curl --fail --location --output italy-isole-latest.osm.pbf.md5 \
  "https://download.geofabrik.de/europe/italy/isole-latest.osm.pbf.md5"

# macOS: md5 -q; Linux: md5sum
md5 -q italy-isole-latest.osm.pbf
cat italy-isole-latest.osm.pbf.md5   # compare by eye, or script the comparison
```

`demo-data/` is already in this repository's `.gitignore` — large local
data artifacts never get committed.

## Step 2: figure out the ingestion path (read before copying commands)

`tellurion-ingest geopackage load` batch-applies GeoJSON FeatureCollections and
GeoJSON Text Sequences through the driver's transactional write path. It does not
decode an OpenStreetMap PBF or create arbitrary layers from it. Converting this
multi-layer source through one-feature-at-a-time HTTP writes would not be practical.

The actual path is the intermediate-conversion one the driver's own
provisioning code anticipates: use the host's `ogr2ogr` (GDAL) to write
the PBF layers directly into GeoPackage format, and
let `tellurion-geopackage`'s read path (`crates/tellurion-geopackage/src/
catalog.rs`) discover the result the same way it discovers any other
provisioned table — by reading the GeoPackage spec's own metadata tables
(`gpkg_contents`, `gpkg_geometry_columns`), not by requiring
`tellurion-ingest` to have been the one that wrote them. This works because
the read path's requirements are exactly what GDAL's own GeoPackage writer
already produces: a single `INTEGER PRIMARY KEY` column (GDAL's default is
`fid`), a registered `gpkg_geometry_columns`/`gpkg_contents` row, and an
R\*Tree spatial index in the same shape (GeoPackage Annex L) the format
mandates. This was verified directly against the running server, not
assumed — see "What was actually verified" below.

One consequence worth knowing: a GeoPackage produced this way has no
`<table>_outbox` table (that only comes from `tellurion-ingest geopackage
create-tables`), so a collection loaded this way is **read-only** —
features and tiles serve fine, but a write against it fails cleanly by name
(`OutboxTableMissing`) rather than silently accepting a write this file was
never provisioned to receive.

## Step 3: convert PBF to GeoPackage

GDAL's OSM driver exposes five fixed layers for any `.osm.pbf` file:
`points`, `lines`, `multilinestrings`, `multipolygons`, `other_relations`.
This config uses `lines` (roads, waterways, railways) and `multipolygons`
(buildings, land use, natural areas) — the same two layers the sibling
PostGIS-backed `config/example-italy-osm.yaml` in this repository already
uses for the same reason.

```sh
cd demo-data/italy-isole-osm

ogr2ogr -f GPKG italy-isole.gpkg italy-isole-latest.osm.pbf lines \
  -t_srs EPSG:3857 -lco GEOMETRY_NAME=geom -nlt PROMOTE_TO_MULTI \
  -nln italy_isole_lines -progress

ogr2ogr -f GPKG italy-isole.gpkg italy-isole-latest.osm.pbf multipolygons \
  -t_srs EPSG:3857 -lco GEOMETRY_NAME=geom -nlt PROMOTE_TO_MULTI \
  -nln italy_isole_multipolygons -update -progress
```

`-t_srs EPSG:3857` reprojects at conversion time rather than leaving the
data in OSM's native EPSG:4326 and reprojecting at tile-encode time. This
was a deliberate choice, not the default GDAL would pick on its own — see
the next section for why.

Each `ogr2ogr` invocation ran in about 7 seconds on the machine this was
built on. The resulting file was 1,249,755,136 bytes (about 1.16 GiB), with
1,325,288 features in `italy_isole_lines` and 2,125,493 features in
`italy_isole_multipolygons` (both counts from `ogrinfo -al -so`).
`GEOMETRY_NAME=geom` and `-nlt PROMOTE_TO_MULTI` match the flags this
repository's own `tellurion-ingest`
(`crates/tellurion-ingest/src/ogr2ogr_loader.rs`) already passes when
loading into PostGIS, for consistency.

## The polygon regression this dataset exposed

The first version of this walkthrough uncovered a ring-winding mismatch while
encoding real OSM `MultiPolygon` features. The GeoPackage path produced valid vector
tile bytes whose rings were then misclassified by the rasterization reader, causing a
named geometry-format failure instead of a PNG.

The current shared vector-tile encoder normalizes exterior and interior ring winding
from polygon structure before writing MVT. The implementation lives in
`crates/tellurion-vector-tile/src/geometry.rs`; the GeoPackage end-to-end regression in
`crates/tellurion-geopackage/tests/driver_contract.rs` writes a conventionally wound
`MultiPolygon`, produces an MVT tile, and decodes it again. No GDAL winding rewrite is
required for the commands below.

## Step 4: verify the file is what the driver expects

```sh
sqlite3 italy-isole.gpkg \
  "SELECT table_name, data_type, srs_id FROM gpkg_contents;"
sqlite3 italy-isole.gpkg \
  "SELECT name, pk FROM pragma_table_info('italy_isole_multipolygons') WHERE pk != 0;"
```

Expected: both tables listed with `data_type = features` and `srs_id =
3857`, and exactly one primary-key column named `fid`.

## Step 5: the config

`config/example-italy-isole-osm-geopackage.yaml` in this repository is the
config this produced — two collections (`italy_isole_lines`,
`italy_isole_multipolygons`), no `table`/`geometry`/`pk` overrides (they
derive at boot from the file's own GeoPackage metadata, exactly as
`config/example-geopackage.yaml`'s seeded `demo` collection already does),
a default per-collection fill/stroke style for the unstyled PNG lane, and a
registered MapLibre style (`config/styles/osm-basic.json`) for the styled
PNG lane.

## Step 6: build and run

```sh
cargo build -p tellurion --no-default-features --features geopackage -p tellurion-ingest

TELLURION_GEOPACKAGE_PATH=/absolute/path/to/italy-isole.gpkg \
TELLURION_CONFIG=config/example-italy-isole-osm-geopackage.yaml \
  target/debug/tellurion
```

`--no-default-features --features geopackage` is the acceptance-proof build
shape documented in `crates/tellurion-server/Cargo.toml`'s own `[features]`
comments: the database driver (`postgis`) is fully compiled out, so this is
a genuine zero-database binary, not the default build with an unused driver
along for the ride.

## What was actually verified against the running server

Every claim below was checked against a real, running instance — not
assumed from reading the source:

- Boot-time catalog validation (`Router::validate_catalog`) passed with no
  errors: the two collections' `table`/`geometry`/`pk` derivation against
  the GDAL-authored file's own metadata resolved cleanly.
- `GET /public/features/catalogs/default/collections` listed both
  collections with real spatial extents matching Sicily and Sardinia's
  actual bounding box (roughly 8.1–15.7°E, 35.5–41.3°N for the
  multipolygons layer).
- `GET .../collections/italy_isole_multipolygons/items?bbox=...` returned
  real GeoJSON `MultiPolygon` features (verified building footprints near
  Palermo and Catania, not placeholder or synthetic geometry).
- `GET .../tiles/WebMercatorQuad/14/6313/8800.mvt` returned HTTP 200,
  `content-type: application/vnd.mapbox-vector-tile`, 973,675 bytes. This
  was independently re-decoded with GDAL's own MVT driver (`ogrinfo -al -so`
  on the downloaded tile), reporting a layer named `italy_isole_multipolygons`,
  geometry type `Multi Polygon`, 3,398 features, extent `0..4096` (correct
  MVT tile-local space), and exactly the configured `tile_properties`
  fields (`name`, `building`, `leisure`, `landuse`, `natural`) — confirmation
  from a completely independent implementation, not just "the server
  answered 200."
- `GET .../tiles/WebMercatorQuad/14/6313/8800.png` (with driver-side winding normalization)
  returned HTTP 200, `content-type: image/png`, and decoded as a real
  256×256 8-bit RGBA PNG showing a recognizable Palermo street/building
  pattern when viewed.
- `GET .../collections/italy_isole_multipolygons/styles/osm-basic/map/tiles/
  WebMercatorQuad/14/6313/8800.png` returned HTTP 200, `image/png`, and
  rendered the same area through the registered MapLibre style — visually
  confirmed as a real basemap (tan building fills with brown outlines, over
  a real street layout), not a blank or solid-color image.
- `GET /public/styles/catalogs/default/styles` and `.../styles/osm-basic`
  round-tripped the registered style document with the documented
  `application/vnd.mapbox.style+json` media type.

## Cleaning up

`demo-data/` is gitignored — nothing under it needs manual cleanup before
committing, but it is a real ~1.7 GiB of local disk (the source PBF plus
two GeoPackage copies) if you want to reclaim the space:

```sh
rm -rf demo-data/italy-isole-osm
```
