# Real use cases for Tellurion's current capabilities

Every scenario below is grounded in what this codebase actually implements
today, verified against source and, where noted, against a real running
server: OGC API Features, OGC API Tiles (MVT vector tiles and PNG raster
tiles), OGC API Styles (MapLibre style documents), STAC (Catalog/
Collections/Items), and 3D building footprints via `tellurion-places`
(extruded footprints as 3D Tiles 1.1). Nothing here depends on capabilities
this codebase does not have — no H3/S2/DGGS indexing, no geometry or 3D
statistics endpoints of any kind exist in this codebase, so no scenario
below relies on them.

## 1. A self-contained offline or edge OGC API server from one file

`crates/tellurion-geopackage` plus the `--no-default-features --features
geopackage` build shape (`crates/tellurion-server/Cargo.toml`) gives a
single native binary and a single `.gpkg` file with no database service, no
container runtime, and — once the file is prepared — no network dependency
at all. That combination fits genuinely disconnected or resource-constrained
deployments a Postgres-backed stack would be the wrong shape for: a field
survey laptop serving its own collected data to a tablet over a local Wi-Fi
hotspot, a vessel or vehicle running an offline basemap and asset inventory,
a demo booth with no venue network to depend on, or a small internal tool
where standing up PostGIS is disproportionate to the data volume. This
repository's own `docs/quickstart/real-data-osm-geopackage.md` demonstrates
this concretely: 3.4 million real OpenStreetMap features (roads, waterways,
buildings, land use for Sicily and Sardinia) served entirely from one 1.16
GiB file, with real OGC API Features and Tiles responses verified against
the running server.

## 2. Publishing OSM-derived vector tiles for a web map

The same `.gpkg` file also serves genuine Mapbox Vector Tiles through OGC
API Tiles (`GET .../tiles/WebMercatorQuad/{z}/{y}/{x}.mvt`), independently
verified in this repository's own demo to decode correctly in GDAL's own
MVT reader with the right layer name, geometry type, feature count, and
property schema. Point a MapLibre GL JS or client-side Leaflet-with-vector-
tiles web map at that endpoint, or connect QGIS to it (see
`docs/quickstart/qgis.md`), and you have a self-hosted alternative to a
third-party vector tile provider for OSM-derived data you've prepared
yourself — useful when licensing, offline requirements, or wanting control
over exactly which OSM layers and properties are exposed rule out a hosted
tile service.

## 3. A styled basemap without a tile-rendering pipeline of your own

`tellurion-styles` serves real MapLibre Style JSON documents
(`application/vnd.mapbox.style+json`) through a read-only OGC API — Styles
surface, and `tellurion-render` uses that same style document to rasterize
PNG map tiles server-side
(`GET .../collections/{cid}/styles/{styleId}/map/tiles/WebMercatorQuad/
{z}/{y}/{x}`) — verified in this repository's demo to produce a real,
visually correct basemap (building fills, outlines, road strokes) from a
hand-written style document over real OSM data. This suits a client that
can't or shouldn't run its own vector-tile styling engine — an embedded
device rendering a fixed-style map, a PDF/print pipeline that needs a
raster tile rather than a vector one, or simply a simpler client
implementation that only ever has to fetch and display a PNG.

## 4. STAC-cataloging raster imagery

`tellurion-stac` implements a real STAC API (Catalog, Collections, Items)
alongside the raster drivers in this workspace (`tellurion-cog` for
Cloud-Optimized GeoTIFF, `tellurion-zarr` for Zarr v2 arrays). Combined with
`tellurion-ingest cog author` — which converts a plain, single-resolution
GeoTIFF into a tiled, Deflate-compressed, overview-pyramided COG — this
supports cataloging and serving imagery collections (aerial or satellite
scenes, derived rasters, categorical/classified rasters with an authored
colormap) through a standards-based discovery API, without needing a
separate STAC catalog implementation bolted onto whatever is already
serving the tiles.

## 5. Serving 3D building footprints

`tellurion-places` extrudes 2D building footprints (the same kind of
polygon data this repository's OSM demo loads — OSM `building=*` tags,
specifically) into 3D Tiles 1.1, the format Cesium and other 3D web clients
consume directly. This suits a use case like a city or campus 3D basemap,
a construction/planning visualization, or any scenario needing a real
extruded-building view rather than a flat 2D map — built from the same
storage driver and the same underlying polygon data already being served
as 2D vector/raster tiles, not a separate data pipeline.

## What this list deliberately leaves out

No DGGS (H3, S2, or any other discrete global grid system) support exists
in this codebase, so no "index my data into H3 cells" scenario is listed —
that would be describing a capability that doesn't exist. Likewise, no
geometry or 3D statistics endpoints exist (no per-collection area/length/
volume aggregation API), so no analytics-style use case is listed either.
If a future capability adds either, this document should grow a new
scenario grounded in it — not before.
