# Tellurion open-data stress demo: Italy first, Europe when the disk is ready

This demo uses an intentionally staged data plan. The existing seeded GeoPackage
remains the offline smoke test. The first real workload is the Geofabrik Italy
OpenStreetMap PBF (about 2.1 GB compressed). Europe is a separate, opt-in profile
(about 32.3 GB compressed) that is prepared for a mounted volume with at least
64 GiB free. The preparation scripts record the exact source checksum and the
filesystem free space before and after download; ingest separately checks the
PostGIS volume that receives expanded tables and indexes, so a result can be reproduced
instead of guessed from a label such as “Europe scale”.

Geofabrik distributes regional OSM extracts under the ODbL 1.0. Every screenshot,
video, and report must retain `© OpenStreetMap contributors`; see the
[Geofabrik Europe extract](https://download.geofabrik.de/europe.html) and
[OpenStreetMap copyright](https://www.openstreetmap.org/copyright) pages.

## The demo tiers

| Tier | What it proves | Disk policy | Recommended use |
|---|---|---:|---|
| Small seeded GeoPackage | routes, styling, protocol smoke tests | negligible | CI and a clean first run |
| Italy OSM PBF | realistic dense roads, waterways, land-use/building polygons | 8 GiB free reserve | first ingest and stress campaign |
| Europe OSM PBF | broad extent, larger indexes, longer cold-cache walks | 64 GiB free reserve | external-disk XL campaign |
| Overture buildings/transportation supplement | cross-source schema/topology and GeoParquet paths | choose a bounded area | optional interoperability chapter |
| Copernicus DEM GLO-30 | raster/COG and reprojection lane | mount/object-store budget | optional raster chapter |

Overture's [Buildings](https://docs.overturemaps.org/guides/buildings/) and
[Transportation](https://docs.overturemaps.org/guides/transportation/) guides
are useful free-to-use supplements when a bounded GeoParquet slice is desired.
The [Copernicus DEM GLO-30 collection](https://dataspace.copernicus.eu/explore-data/data-collections/copernicus-contributing-missions/collections-description/COP-DEM)
is a separate raster source with its own attribution requirements; it should not
be mixed into the OSM disk budget.

## Comparison for this project

| Concern | Tellurion | GeoServer | QGIS Desktop | GeoID/DynaStore |
|---|---|---|---|---|
| Primary role | Rust service with OGC API Features/Tiles, MVT/PNG, optional drivers and embedded GeoPackage/PostGIS | Mature Java server for WMS/WFS/WCS/WMTS and OGC API modules | Desktop authoring, inspection, styling, and client consumption | Python/FastAPI geospatial platform with OGC/STAC/coverage workflows and dynamic cataloging |
| Italy ingest | `tellurion-ingest` delegates DDL to GDAL/`ogr2ogr`, then prints a collection declaration | Import via PostGIS/GeoPackage and publish a layer/workspace | Open the PBF/loaded service and inspect or style it; not the benchmark's ingest server | Strong pipeline/catalog integration, but heavier operational setup for a focused tile benchmark |
| Europe scale | PostGIS profile is the fair stress target; GeoPackage remains a useful small baseline | PostGIS and GeoWebCache are proven, but JVM/cache configuration must be frozen | Excellent exploratory client; not a competing tile-serving process | Good for catalog/asset/control-plane comparisons; isolate tile-serving work from control-plane latency |
| Tile path | MVT and PNG lanes; cache cold/warm and uniform DB-path walks already exist in `bench/scenarios.sh` | WMTS/GeoWebCache and OGC API Tiles; tune and document cache state | Consumes XYZ/WMTS/vector tiles; QGIS docs cover tile connections, not server throughput | OGC API Tiles and cache layers are available in the platform's tile component |
| Strengths | One binary, explicit profiles, low ceremony, protocol-focused benchmark harness | Broad interoperability and mature admin/UI ecosystem | Best visual QA and cartographic iteration | Broad OGC/STAC/coverage integration and dynamic data workflows |
| Trade-offs | Still needs PostGIS/GDAL for the large ingest path; Rust deployment is less familiar to some teams | More JVM/GeoWebCache tuning and configuration to make a fair comparison | Not a headless production tile server; desktop memory and rendering affect observations | More services and moving parts than this narrow demo needs |
| Disk story | Thresholds and manifests are first-class; Europe refuses on the internal disk | Data, indexes, JVM caches, and GeoWebCache need separate accounting | Local cache/project files can obscure source-data size | Object stores, PostGIS, and cache tiers must be reported separately |

The fair comparison is therefore protocol-by-protocol on the same host and the
same loaded extract. QGIS is the visual/client reference, not an apples-to-apples
server competitor. GeoServer should be run with its warmed and cold GeoWebCache
states reported separately. GeoID/DynaStore should be measured on the tile lane
and separately on catalog/control-plane calls so a catalog query cannot hide the
cost of a tile response.

GeoServer's [service documentation](https://docs.geoserver.org/main/en/user/services/index.html)
and [OGC API Tiles module](https://docs.geoserver.org/main/en/user/community/ogc-api/tiles/)
describe its broad service surface. QGIS's [data-source documentation](https://docs.qgis.org/4.2/en/docs/user_manual/managing_data_source/opening_data.html)
covers vector-tile and XYZ connections for the client-side chapter.

## Stress protocol

Run the same extract, schema, host, database, and container images for each
server. Record the commit/image, PostGIS version, CPU/RAM, source and loaded
bytes, index bytes, free space, cache state, and concurrency. Discard warmup
repetitions and report p50/p95/p99, RPS, error rate, RSS, and recovery time.

1. Cold localized MVT walk: measures cache misses around a realistic map area.
2. Warm identical MVT walk: measures cache-hit behavior after step 1.
3. Uniform DB-path MVT walk: samples the full Italy/Europe extent to avoid
   accidentally measuring the cache.
4. PNG and styled PNG: reports rendering overhead separately from MVT.
5. Feature pages and single-item reads: exposes attribute/query cost.
6. Mixed 70/20/10 traffic: report realized per-shape RPS because latency changes
   the request mix; do not treat the configured concurrency split as a result.
7. Load-shed burst and recovery: verify fast 503 rejection and time to healthy
   responses, not just maximum throughput.

The existing harness is intentionally explicit about these lanes. A result table
should stay empty until the run has produced JSON and Prometheus snapshots:

| Profile | Server/config | Cold p95 | Warm p95 | DB-path p95 | RPS | Error % | RSS delta | Disk/index notes |
|---|---|---:|---:|---:|---:|---:|---:|---|
| Italy | Tellurion | pending | pending | pending | pending | pending | pending | fill from manifest + PostGIS |
| Italy | GeoServer | pending | pending | pending | pending | pending | pending | same host/extract |
| Europe | Tellurion | pending | pending | pending | pending | pending | pending | external volume only |
| Europe | GeoServer | pending | pending | pending | pending | pending | pending | external volume only |

“Pending” is deliberate: it prevents the demo from turning a machine-specific
smoke result into a general performance claim.

## Running the Italy chapter

```sh
./bench/data/prepare.sh --profile italy --download
export DATABASE_URL='postgres://...'
./bench/data/ingest-osm.sh
# Start with config/example-italy-osm.yaml, then run a short smoke campaign.
ZMAX=5 DB_ZMIN=8 DB_ZMAX=10 REPS=1 WARMUP=0 DURATION=5s CONCURRENCY=20 \
  BASE_URL=http://localhost:8080 COLLECTION=italy_multipolygons \
  ./bench/scenarios.sh
./bench/summarize.sh
```

For Europe, set `TELLURION_DEMO_DATA_ROOT` to the mounted PBF volume and
`TELLURION_DB_DATA_ROOT` to the host path or mounted volume backing PostGIS, run
`--check-only`, and only then use `--download` and `ingest-osm.sh --profile
europe`. Do not use a path that exists only inside a database container, and do
not copy the Europe PBF back to the internal disk merely to make the demo
command shorter. The 64 GiB database reserve is a conservative preflight guard,
not a capacity guarantee; record actual expanded table and index sizes.
