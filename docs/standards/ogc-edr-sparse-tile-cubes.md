# OGC API – EDR Sparse Tile Cubes, STAC UAD Linkage, and Spatiotemporal Filtering (`CQL2` / `INTERSECTS`)

**Status:** Evolved Community & SWG Proposal (`ogc-dimensions` withdrawn as formal proposal)  
**Date:** November 2026  
**Author:** Carlo Cancellieri — [ccancellieri.github.io](https://ccancellieri.github.io/)  
**Reference Implementations:** [Tellurion (Rust)](https://github.com/ccancellieri/tellurion) &middot; [GeoID / DynaStore (Python)](https://github.com/un-fao/geoid)  
**Reproducible Gallery & Public Demo:** [Tellurion Demos Portal](https://ccancellieri.github.io/tellurion-demos/) &middot; [Live STAC & Sparse Datacube Workbench](https://ccancellieri.github.io/tellurion-demos/demos/stac/) &middot; [Render Live Service](https://tellurion-stac-harvest-demo.onrender.com/public/records/catalogs/default/dimensions/dekadal-2021)

---

## Executive Summary

Operational climate, rainfall, and agricultural monitoring platforms ingest **hundreds of thousands of sparse, file-per-granule Cloud-Optimized GeoTIFFs (COGs)** organized along **non-Gregorian calendars** — most prominently **pentadal** (72 periods/year in month-aligned CHIRPS/FAO systems, 73 periods/year in annual GPCP/NOAA systems) and **dekadal** (36 periods/year in FAO ASIS and FEWS NET). Unlike single-store monolithic **GeoZarr** archives where coordinate axes (`coords`) live inside the Zarr group, multi-file COG tile cubes cannot embed coordinate arrays in a single binary store and suffer from extreme **spatiotemporal sparsity**: many theoretical calendar slices contain no granule for a given Area of Interest ($\text{AOI}$) due to cloud masking, swath gaps, late ingestion, or regional coverage limits.

Following community feedback on [`stac-extensions/datacube#36`](https://github.com/stac-extensions/datacube/issues/36) (`@fmigneault`) and [`#31`](https://github.com/stac-extensions/datacube/issues/31) (`@m-mohr`), we leave the standalone *OGC API – Dimensions* repository as a **withdrawn formal proposal** and replace all bespoke endpoints with a pure-standards synthesis built strictly on existing OGC endpoints and query languages:

1. **Minimal STAC Datacube + OGC API – Common Part 2 UAD + Standard `?datetime=` / CQL2 Filtering:** Keep only `size` (integer cardinality, resolving `#31`), `definition` (URI link-out to `/records/.../dimensions/{id}`), and `variableType` (`categoricalOrdinal` / `categoricalNominal` / etc.) on `cube:dimensions` to define the **intensional domain $D$**. Resolving timestamps or sub-intervals (e.g., mapping `2021-01-12` to `2021-01-P3`) uses standard **`GET /dimensions/{id}/items?datetime=...`** or CQL2 **`?filter=T_INTERSECTS(time, TIMESTAMP('...'))&filter-lang=cql2-text`** — with zero custom inverse endpoints.
2. **OGC API – EDR Part 1 (`/items`, `/cube`, `/area`) + OGC API – Features Part 3 CQL2 (`S_INTERSECTS` / `INTERSECTS`) + CoverageJSON `Domain`:** Subsetting executes through **OGC API – EDR Part 1 v1.1** (`OGC 19-086r6`, returning `CoverageJSON` with explicit temporal `bounds` and `null`-filled gaps) backed by an indexed granule footprint store (GeoPackage R-tree / STAC Items). To discover populated slices $S(\text{AOI}, \Delta t) \subsetneq D$ without inventing custom HTTP paths, clients query standard EDR **`GET /collections/{cid}/items`** (`OGC 19-086r6` Section 8.9 `/req/edr/rc-items`) using standard **CQL2 (`OGC 21-065r2`) / ECQL** predicates (`filter=S_INTERSECTS(geom, POLYGON((...)))` or `INTERSECTS(geom, POLYGON((...)))`), receiving either GeoJSON `FeatureCollection` (`application/geo+json`) or a compact **CoverageJSON `"type": "Domain"`** (`?f=CoverageJSON`, `application/prs.coverage+json`) whose `axes.t` lists strictly the populated intersecting slices.

---

## 1. The FAO Operational Use Case & Why Standalone `ogc-dimensions` Was Retired

### Historical Context (FAO Agro-Informatics & ASIS / CHIRPS Workflows)
*(Note: The earlier `data.review.fao.org` review sandbox cited in April 2026 drafts is decommissioned and retained here strictly as historical context; live verification now runs on [Tellurion](https://github.com/ccancellieri/tellurion).)*

Large-scale operational monitoring catalogs routinely manage:
- **Sub-monthly irregular calendars**:
  - **Dekadal (`36/year`)**: Days `1–10` (`D1`), `11–20` (`D2`), and `21–EOM` (`D3`, 8–11 days).
  - **Pentadal monthly-aligned (`72/year`, CHIRPS / CDT / FAO)**: 6 pentads per month (`P1`..`P5` = 5 days each; `P6` = `26–EOM`, varying between 3 days in non-leap February and 6 days in 31-day months).
  - **Pentadal annual-aligned (`73/year`, GPCP / NOAA)**: 73 consecutive 5-day slices from January 1 (`P73` = 5 or 6 days).
- **Hundreds of thousands of independent COG granules**: Each `(time_slice, spatial_tile)` is stored as its own COG (e.g., S3 object or regional mosaic) and indexed by spatial footprint (`Polygon`) + temporal bounds (`[start_datetime, end_datetime]`) in a relational/GeoPackage catalog.

### Why Standalone Paginated `ogc-dimensions` Was Withdrawn
While the standalone `ogc-dimensions` Records pagination spec solved calendar enumeration ($D$), community feedback (`@fmigneault` on `stac-extensions/datacube#36`) rightly pointed out that:
1. Embedding custom `provider` schemas and re-inventing pagination envelopes inside STAC `cube:dimensions` adds unnecessary boilerplate when **OGC API – Common Part 2 UAD** (`definition`, `variableType`) + optional `size` already lets a collection link cleanly to an external domain definition (`/dimensions/{id}`).
2. Replacing bespoke endpoints with standard **`GET /dimensions/{id}/items?datetime=...`** and CQL2 **`?filter=T_INTERSECTS(...)`** keeps the domain API minimal and standard-compliant.
3. Even with perfect enumeration of $D$, **listing all theoretical timestamps $D$ does not tell an analytical client or Web UI which spatiotemporal slices actually contain COG data in a user's $\text{AOI}$!** That lookup ($S \subsetneq D$) is a **spatiotemporal `intersects` filtering concern** best solved using **standard EDR `/collections/{cid}/items` + OGC Features Part 3 CQL2 (`S_INTERSECTS` / `INTERSECTS`) + `CoverageJSON` (`Domain`)**, without inventing custom endpoints.

Consequently, `ogc-dimensions` stays archived with a *withdrawn formal proposal* notice and further development on `ogc-dimensions/reference-implementation/` is discontinued; the calendar domain rules (`dekadal-2021`, `pentadal-2021`, `dekadal-2000-2026`, `pentadal-2000-2026`) and `?datetime=` / CQL2 filtering live in `tellurion-records` (`dimensions.rs`), while all extraction and CQL2-filtered `/items` resolution live in `tellurion-edr`.

---

## 2. Architectural Comparison: GeoZarr vs. Sparse COG Tile Cubes vs. GeoPackage Index

A key reason the GeoDataCube and Zarr communities sometimes talk past earth-observation COG operators is the structural contrast between **single-store multidimensional arrays** and **multi-file sparse COG archives**:

| Architecture Aspect | **GeoZarr v2 (`sample_zarr_t0`)** | **Sparse Multi-File COG Cube (`fao_chirps_pentadal_2021_italy`, `esa_worldcover_2021_italy`)** | **GeoPackage / PostGIS Index (`dekadal_sparse_granules`)** |
|---|---|---|---|
| **Storage Layout** | Single hierarchical store (`.zattrs`, `.zarray`, chunks `[t, y, x]`) | Hundreds of thousands of independent COG files (`1 file ≈ 1 granule`), routed via `routing.tiles` (raster) + `routing.features` (index) | Relational table + R-tree virtual index (`rtree_<table>_geom`) storing `(id, geom, pentad_id/dekad_id, start_datetime, end_datetime, status)` |
| **Coordinate Metadata (`coords`)** | Embedded directly inside store attributes / 1D coordinate arrays (Zarr `coords`) | **Cannot exist inside a single COG file** — no individual TIFF holds the global multi-year pentadal/dekadal axis | Stores **extensional** per-granule footprints (`geom`) and actual ingested timestamps ($S$), not missing slots |
| **Calendar Expressiveness** | Fixed array index `[0..N-1]` paired with numeric time vector or `.zattrs` metadata | Intensional calendar $D$ defined once via UAD `definition` URI (`size: 72` or `36`); explicit irregular `t.bounds` in CoverageJSON | Rows carry canonical IDs (`2021-01-P3`, `2021-01-D1`) resolved via standard UAD `/items?datetime=...` |
| **Temporal & Spatial Sparsity** | Dense rectangular chunk grid; empty slices waste storage or rely on unpopulated chunks | Missing granules are **absent files** (zero storage cost); partial footprints leave non-intersecting pixels unpopulated | Exact spatial R-tree pushdown + WKT geometry intersection reveals populated vs. missing slices in milliseconds |
| **EDR Subset Output (`CoverageJSON`)** | Direct slice readout over `[time, y, x]` chunks (in our demo: `shape=[2, 64, 64]`, `4508` non-null cells) | Multi-item assembly over all matching slices in $\Delta t$: missing slices become `null`, partial granules are masked per-pixel to `matched_bbox` (`shape=[6, 64, 64]`) | Exposed via standard Features/EDR `/items` (`filter=S_INTERSECTS(...)` / `INTERSECTS(...)`) and consumed internally by EDR (`/cube`/`/area`/`/position`/`/radius`) |

This comparison explains why **`coords`-embedded Zarr solutions do not generalize cleanly to multi-file COG archives**:
- In **GeoZarr**, a client opening the store finds coordinate arrays in one place because the entire cube lives inside one Zarr hierarchy.
- In a **Sparse Pentadal/Dekadal COG Tile Cube**, the "cube" is a *virtual* logical projection over thousands of independent GeoTIFF assets distributed across object storage. No single GeoTIFF contains a global `coords` array, so clients rely on STAC/UAD (`definition`, `size`, `variableType`) for the calendar grammar ($D$) and need a catalog/EDR sparsity mechanism to determine $S(\text{AOI}, \Delta t)$.

---

### 2.2 Formal Evaluation: Can Populated Members Be Discovered Any Better Way in Sparse Multi-File Cubes?

When a cloud-native raster datacube lacks a single monolithic array header (such as `.zmetadata` in GeoZarr or an HDF/NetCDF coordinate variable) and is instead assembled from hundreds of thousands of Cloud-Optimized GeoTIFFs (COGs) or multi-file object-store partitions indexed in STAC/GeoParquet/GeoPackage, four candidate mechanisms exist on the wire:

| Mechanism | Captures Spatial Conditioning $S(\text{AOI}, \Delta t)$? | Payload Overhead on $10^5$ Granules | Non-Gregorian `bounds` (`pentadal`/`dekadal`) | OGC Standards Status | Verdict |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **1. Static STAC `summaries` / inline `cube:dimensions.values`** | **No.** Computes only the global marginal projection $\pi_t(S_{\text{global}})$. A period with data in Sicily still looks "available" over Lombardy where 0 granules exist. | High in Collection JSON ($10^3$–$10^4$ periods) | Inline strings only; no interval bounds per member | STAC Extension | Sufficient for small Zarr; **breaks for large sparse COGs** |
| **2. Client-side OGC Features `/items?bbox=...` pagination** | Yes, after client aggregation | **$O(K \times M)$**: transfers full tile footprints & GeoJSON properties across $K$ periods $\times M$ spatial tiles per period so the UI can compute `DISTINCT(t)` | Client must reconstruct nominal calendar intervals | Standard (`OGC 17-069`) | **Unscalable** above a few hundred granules |
| **3. STAC API Aggregation Extension (`/aggregate`)** | Yes | Compact JSON buckets | Gregorian histogram bins (`10d`/`5d`); drifts from irregular FAO dekads/pentads | Community STAC API ext (non-OGC) | Useful inside STAC-only ecosystems, but **outside OGC EDR / Coverages** |
| **4. Composed UAD + EDR `/collections/{cid}/items?filter=S_INTERSECTS(...)&f=CoverageJSON` (`Domain`)** | **Yes — exact $\pi_t(\sigma_{S\_INTERSECTS(g,\text{AOI}) \land t \in \Delta t}(\text{Index}))$** | **`< 1 KB`**: only distinct populated slices in `Domain.axes.t` | Exact `values` (centroids/start) + `bounds` `[start, end]` pairs per irregular period | **100% Standard (`OGC 19-086r6` §8.9 + `OGC 19-079r2` CQL2 + `OGC 21-069` CoverageJSON + `OGC 20-024` UAD)** | **Optimal wire contract** across both STAC and OGC APIs |

**Conclusion:** No alternate REST contract delivers less wire overhead or stronger standards purity than composing **OGC Common Part 2 UAD** (for the nominal grammar $D$) with **OGC EDR `/collections/{cid}/items` + CQL2 `S_INTERSECTS` + `f=CoverageJSON` (`Domain`)** (for the populated slice support $S(\text{AOI}, \Delta t)$). Meanwhile, the fastest *execution engine* under that endpoint for object-store catalogs is a columnar **GeoParquet / DuckDB / Portolan SDI** manifest (Section 4.5).

## 3. Why OGC API – EDR Part 1 v1.1 Alone Is Insufficient: The "Blind Extraction Problem"

OGC API – Environmental Data Retrieval (EDR) Part 1 v1.1 (`OGC 19-086r6`) dramatically improved scientific web access by replacing whole-file downloads (NetCDF/GRIB/WCS) with targeted `/position`, `/area`, `/cube`, `/trajectory`, `/corridor`, and `/radius` queries returning **CoverageJSON**.

However, in existing EDR Part 1 v1.1 (and even in earlier server prototypes like GeoID's initial `limit=1` STAC lookup), sparse COG tile cubes expose a structural limitation we call the **Blind Extraction Problem**:

1. **Collection metadata (`GET /edr/collections/{cid}`) only exposes the intensional outer envelope ($D$):**
   - `extent.spatial.bbox` reports the union bounding box (or `[-180, -90, 180, 90]`).
   - `extent.temporal.interval` reports `[["2021-01-01T00:00:00Z", "2021-12-31T23:59:59Z"]]`.
   - Nowhere does EDR Part 1 tell the client *which* discrete pentads (`P1..P6`) or dekads (`D1..D3`) inside that envelope actually have pixels intersecting a specific query polygon or bounding box!
2. **Collection metadata (`GET /edr/collections/{cid}`) only exposes the intensional outer envelope ($D$), while clients lacked an integrated CQL2 + CoverageJSON pattern inside EDR:**
   - Historically, to check if `2021-01-P3` had data over an $\text{AOI}$ before requesting an expensive multi-slice `CoverageJSON` raster extraction (or to distinguish an *intentionally missing granule* from a *zero-valued rainfall pixel*), clients had to call a **separate OGC API – Features / CQL2** endpoint (`/features/collections/{cid}/items?filter=S_INTERSECTS(...)`), parse GeoJSON feature properties, map raw timestamps back to pentad/dekad IDs via `/dimensions/{id}/items?datetime=...`, and only then issue the EDR query.
   - As Carlo noted, OGC API – Features already expresses spatial intersection natively through standard **CQL2 (`S_INTERSECTS`) / ECQL (`INTERSECTS`)** on **`/collections/{cid}/items`** — and **OGC API – EDR Part 1 v1.1 (`OGC 19-086r6`, Section 8.9 `/req/edr/rc-items`) already defines `GET /collections/{cid}/items` directly inside EDR!** We do not need to invent custom HTTP endpoints (`/intersection`, `/availability`) or bespoke vocabulary: binding **OGC API – Features Part 3 / CQL2 (`OGC 21-065r2`)** onto **standard EDR `/collections/{cid}/items`** with optional `f=CoverageJSON` (`Domain`) solves spatiotemporal sparsity natively using 100% standard OGC endpoints and filters.

---

## 4. Standards-Pure Solution: EDR `/items` + OGC CQL2 (`S_INTERSECTS` / `INTERSECTS`) + `CoverageJSON` `Domain`

Rather than introducing custom paths or non-standard JSON response fields, Tellurion composes strictly existing OGC standards across one unified `/items` pattern:

### 4.1 Conformance Classes & Collection Links (100% Standard OGC)
- **Declared Conformance Classes (`GET /edr/catalogs/default/conformance`):**
  - EDR Part 1 v1.1 (`OGC 19-086r6`): `ogcapi-edr-1/1.1/conf/core`, `collections`, `json`, `edr-geojson`, `queries`, `covjson`
  - OGC API – Features Part 3 Filtering (`OGC 19-079r2`): `http://www.opengis.net/spec/ogcapi-features-3/1.0/conf/filter`
  - CQL2 (`OGC 21-065r2`): `http://www.opengis.net/spec/cql2/1.0/conf/basic-cql2`, `cql2-text`, `cql2-json`, `basic-spatial-functions`, `temporal-functions`
- **Collection Advertisement (`GET /edr/catalogs/default/collections/{cid}`):**
  Standard `data_queries.items` (`href: ".../collections/{cid}/items"`, `rel: "items"`, `type: "application/geo+json"`) alongside `area`, `corridor`, `cube`, `instances`, `locations`, `position`, `radius`, and `trajectory`.

### 4.2 Standard Endpoint & Query Contract
```
GET /collections/{collectionId}/items
GET /collections/{collectionId}/items/{itemId}
```

**Supported Standard Filter & Subsetting Parameters:**
- **`filter` + `filter-lang`** (`cql2-text` default or `cql2-json`):
  - **OGC CQL2 (`OGC 21-065r2`):** `S_INTERSECTS(geom, POLYGON((...)))`, `T_INTERSECTS(time, TIMESTAMP('...'))` / `INTERVAL('...', '...')`, `T_DURING`, `T_AFTER`, `T_BEFORE`.
  - **Classic OGC CQL / GeoServer ECQL compatibility alias:** `INTERSECTS(geom, POLYGON((...)))` parses cleanly into the exact same spatial-intersection AST (`Token::SIntersects`).
- **`bbox`** (`minx,miny,maxx,maxy` in CRS84): Pushed down to the GeoPackage R-tree index (`rtree_<table>_geom`).
- **`coords`** (EDR Part 1 WKT Geometry) or **`intersects`** (STAC GeoJSON / WKT): Also compiled to exact `geo::Intersects` filtering alongside `filter=S_INTERSECTS(...)`.
- **`datetime`** (ISO 8601 instant or `start/end` interval): Filters candidate slices overlapping $\Delta t$.
- **`f` (Content Negotiation):**
  - Default / `f=GeoJSON` (`application/geo+json`): Returns a standard GeoJSON `FeatureCollection` containing strictly the matching granule footprints (`numberMatched`, `numberReturned`, `id`, `geometry`, `time.interval`, `properties.granule_bbox`).
  - `f=CoverageJSON` (`application/prs.coverage+json`): Returns a compact (`<1 KB`), **strictly standard CoverageJSON `"type": "Domain"`** (`"domainType": "Grid"`, `OGC 21-069`) whose `axes.t.values` and `axes.t.bounds` list **strictly the populated intersecting slices** $S(\text{AOI}, \Delta t)$, with standard `referencing` (`CRS84` geographic CRS + Gregorian `TemporalRS`). No non-standard top-level wrapper fields are added.

Clients can inspect populated slices using plain GeoJSON features, or request `f=CoverageJSON` on the exact same `/items` URL to skip empty windows (`axes.t.values == []`), identify valid pentad/dekad IDs, and issue `/cube` or `/area` solely on populated periods.

### 4.3 Multi-Item Sparse Assembly Rules (Resolving DynaStore / GeoID Gaps)
In `crates/tellurion-edr/src/sparse.rs`, Tellurion resolves four known limitations identified in the reference Python GeoID/DynaStore audit:
1. **No `limit=1` truncation:** Queries up to `1000` intersecting index features across the full `datetime` interval rather than stopping after a single STAC Item.
2. **Spatial + geometric `intersects` filtering:** Pushes `bbox` down to the GeoPackage R-tree (`rtree_<table>_geom`) and evaluates exact `geo::Intersects` against CQL2 `S_INTERSECTS` / ECQL `INTERSECTS` / `coords` / `intersects`.
3. **Intra-slice spatial masking (`matched_bbox`) + exact Area/Radius polygon masking:** For partial granules covering only part of an AOI (such as `2021-01-P3`), pixels outside the intersecting `granule_bbox` (and outside non-rectangular `area` polygons or `radius` circles) are set to `null` inside the CoverageJSON `NdArray`.
4. **Irregular interval preservation:** Populates `domain.axes.t.bounds` alongside `domain.axes.t.values` so downstream consumers can verify variable-length slices (`P6` = 6 days, `D3` = 8–11 days).

### 4.4 Web UI Discovery Workflow: Finding Populated Dimension Members in Massive Sparse Cubes
Suppose an operational archive indexes **hundreds of thousands of sparse COG files** (for instance, `1,440` pentads over 20 years × `500` spatial tiles = `~720,000` STAC Items / GeoPackage rows). A Web UI must populate a compact pentad/dekad time picker showing **which periods actually have data in the user's current map viewport ($\text{AOI}$)** without paginating through hundreds of thousands of per-tile JSON payloads.

Two standard mechanisms work cleanly together without custom APIs:

1. **Why naive client-side `/items` paging at `limit=100` hurts the browser:** Fetching hundreds of raw multi-tile feature pages just to compute `DISTINCT(time_slice)` transfers megabytes of redundant tile geometry.
2. **Two-Step Standards-Pure Workflow (UAD Calendar + EDR/Features `/items` with CQL2 `S_INTERSECTS` + `f=CoverageJSON`):**
   - **Step 1 — Calendar grammar from UAD (cacheable):** Follow `cube:dimensions.time.definition` to `GET /records/.../dimensions/{did}/items?datetime=<window>` (or `?filter=T_INTERSECTS(time, TIMESTAMP('...'))&filter-lang=cql2-text`) to obtain the theoretical intensional members $D$ (e.g., `2021-01-P1..P6`) and their `[start, end]` intervals.
   - **Step 2 — Server-side spatial pushdown via standard EDR `/items?filter=S_INTERSECTS(...)&f=CoverageJSON`:** On viewport pan/zoom, issue a single `<1 KB` standard request:  
     `GET /edr/catalogs/default/collections/{cid}/items?filter=S_INTERSECTS(geom,POLYGON((...)))&filter-lang=cql2-text&datetime=<window>&f=CoverageJSON`  
     Tellurion executes the spatial R-tree + `geo::Intersects` filter server-side and returns a standard **CoverageJSON `Domain`** whose `axes.t.values` contains strictly the populated slices $S(\text{AOI}, \Delta t)$ (e.g., `["2021-01-P1", "2021-01-P3", "2021-01-P6"]`). Computing $D \setminus S$ client-side immediately highlights active vs. missing filter chips (`P2, P4, P5` disabled) and only triggers heavy pixel extraction (`/cube` or `/area`) on populated periods.
   - **Step 3 — Individual granule inspection (only on demand):** Omit `f=CoverageJSON` (or call `GET /collections/{cid}/items/{itemId}`) when a user drills into a single active slice to inspect its footprint or asset metadata (`OGC 19-086r6` Section 8.9). Note that where a full STAC Search engine is available, OGC API – Features (`GET /features/.../items?filter=S_INTERSECTS(...)`) and STAC `/search?intersects=...` operate over the same underlying CQL2/spatial index.

---

### 4.5 Cloud-Native Virtual Datacubes via DuckDB File Joins & Portolan SDI

This exact two-layer pattern extends naturally beyond single-table STAC/GeoPackage indexes to **serverless cloud-bucket Spatial Data Infrastructures (SDI)** — specifically the architecture pioneered by **[Portolan SDI](https://www.portolan-sdi.org/)** and embedded analytics engines like **DuckDB** (`tellurion-duckdb` in Tellurion and `OGC API - Joins` workflows in GeoID/DynaStore):

- **How Portolan SDI organizes serverless catalogs:** Portolan SDI stores spatial assets directly on S3-compatible object storage using open formats without heavyweight database middleware: **STAC** for catalog discovery, **GeoParquet** and **PMTiles** for vector/footprint indexes, **COG** and **Zarr** for gridded rasters, and machine-readable `AGENTS.md` / README manifests built to be queried directly by **DuckDB** and AI agents.
- **Compounded sparsity under multi-file joins:** In real agro-climatic workflows, analysts frequently join a sparse COG footprint manifest in object storage (`s3://.../portolan/chirps_items.parquet`) with one or more secondary Parquet/GeoParquet files (for instance FAOSTAT seasonal crop-stress tables, GAUL administrative boundary masks, or hydrological basin telemetry):
  $$S_{\text{joined}}(\text{AOI}, \Delta t) = \pi_{t}\!\left(\sigma_{\text{S\_INTERSECTS}(g, \text{AOI}) \land t \in \Delta t}(\text{COG\_Manifest}) \bowtie \sigma_{\text{predicate}}(\text{Attribute\_Parquet})\right)$$
  Here, spatiotemporal emptiness is *compounded* by relational join selectivity: only pentads or dekads where **both** a raster COG intersects the AOI **and** a matching tabular observation exists contain usable datacube slices.
- **Why DuckDB + EDR `CoverageJSON Domain` are natural partners:**
  1. **Zero-egress pushdown inside DuckDB:** Evaluating `SELECT DISTINCT period_id, start_datetime, end_datetime ...` over joined `read_parquet('s3://...')` streams exploits Parquet row-group min/max statistics and spatial WKB bounding-box pushdown via HTTP range reads — computing $S_{\text{joined}}(\text{AOI}, \Delta t)$ in milliseconds without opening a single COG pixel file.
  2. **Zero-custom-API wire delivery via OGC EDR:** Exposing that joined result through **`GET /edr/.../collections/{cid}/items?filter=S_INTERSECTS(...)&f=CoverageJSON`** returns a standard `<1 KB` CoverageJSON `"type": "Domain"` (`OGC 21-069`), allowing lightweight browser UIs, openEO clients, and AI agents (following Portolan's agentic discovery model) to see which coordinate members are populated before issuing `/cube` or `/area` extractions.

## 5. Live Verified Outputs from Tellurion (`tellurion-stac-harvest-demo.onrender.com` & Local Verification)

All outputs below were executed against the compiled pure-Rust `tellurion` binary (`--features cog,geopackage,zarr`) serving the hybrid catalog (`demo.yaml`) combining UAD Records Dimensions, real remote S3 ESA WorldCover COG streaming (`ESA_WorldCover_10m_2021_v200_N39E012_Map.tif`), a GeoPackage R-tree granule index (`sparse_dekadal.gpkg`), and a synthetic Zarr v2 cube (`sample_zarr_t0`).

### 5.1 UAD Domain Definition & Standard `?datetime=` Filtering (`pentadal-2021` & `dekadal-2021`)

**`GET /public/records/catalogs/default/dimensions/pentadal-2021`** (`HTTP 200`):
```json
{
  "description": "Single-year 72-pentad calendar (6 monthly pentads/month, P6=26..EOM) aligned with CHIRPS/FAO.",
  "extent": {
    "temporal": {
      "interval": [
        [
          "2021-01-01T00:00:00Z",
          "2021-12-31T23:59:59Z"
        ]
      ]
    }
  },
  "id": "pentadal-2021",
  "itemType": "record",
  "links": [
    {
      "href": "/public/records/catalogs/default/dimensions/pentadal-2021",
      "rel": "self",
      "type": "application/json"
    },
    {
      "href": "/public/records/catalogs/default/dimensions/pentadal-2021/items",
      "rel": "items",
      "type": "application/geo+json"
    }
  ],
  "size": 72,
  "title": "Pentadal Temporal Dimension (2021)",
  "variableType": "categoricalOrdinal"
}
```

**Instant lookup via standard OGC `?datetime=` (`GET /public/records/catalogs/default/dimensions/pentadal-2021/items?datetime=2021-01-12` — `HTTP 200`):**
```json
{
  "features": [
    {
      "geometry": null,
      "id": "2021-01-P3",
      "properties": {
        "end_datetime": "2021-01-15T23:59:59Z",
        "month": 1,
        "ordinal": 2,
        "pentad": 3,
        "start_datetime": "2021-01-11T00:00:00Z",
        "title": "2021-01 Pentad 3",
        "type": "record",
        "year": 2021
      },
      "time": {
        "interval": [
          "2021-01-11T00:00:00Z",
          "2021-01-15T23:59:59Z"
        ]
      },
      "type": "Feature"
    }
  ],
  "links": [
    {
      "href": "/public/records/catalogs/default/dimensions/pentadal-2021/items?offset=0&limit=36&datetime=2021-01-12",
      "rel": "self",
      "type": "application/geo+json"
    },
    {
      "href": "/public/records/catalogs/default/dimensions/pentadal-2021",
      "rel": "collection",
      "type": "application/json"
    }
  ],
  "numberMatched": 1,
  "numberReturned": 1,
  "type": "FeatureCollection"
}
```

**Equivalent OGC CQL2 temporal query (`GET /public/records/catalogs/default/dimensions/pentadal-2021/items?filter=T_INTERSECTS(time,TIMESTAMP('2021-01-12T00:00:00Z'))&filter-lang=cql2-text` — `HTTP 200`):** returns the exact same `"numberMatched": 1`, `"id": "2021-01-P3"` FeatureCollection.

**Interval enumeration (`GET /public/records/catalogs/default/dimensions/pentadal-2021/items?datetime=2021-01-01T00:00:00Z/2021-01-31T23:59:59Z` — `HTTP 200`):** returns `"numberMatched": 6` with IDs `["2021-01-P1", "2021-01-P2", "2021-01-P3", "2021-01-P4", "2021-01-P5", "2021-01-P6"]`.

**Dekadal instant lookup (`GET /public/records/catalogs/default/dimensions/dekadal-2021/items?datetime=2021-01-15` — `HTTP 200`):** returns `"numberMatched": 1`, `"id": "2021-01-D2"` (`2021-01-11T00:00:00Z` to `2021-01-20T23:59:59Z`).

### 5.2 Spatiotemporal Pruning via Standard EDR `/items` + CQL2 (`S_INTERSECTS` / `INTERSECTS`) (`fao_chirps_pentadal_2021_italy`)

In `pentadal_sparse_granules`, January 2021 contains:
- `2021-01-P1`: full AOI `[12.0, 39.0, 14.5, 41.5]`
- `2021-01-P3`: **partial SW-only granule** `[12.0, 39.0, 12.6, 39.6]`
- `2021-01-P6`: irregular 6-day period (`Jan 26–31`) full AOI `[12.0, 39.0, 14.5, 41.5]`
- `P2`, `P4`, `P5`: intentionally absent.

**Query A — Southwest AOI via OGC CQL2 `S_INTERSECTS` (`f=CoverageJSON` — `application/prs.coverage+json`):**  
`GET /public/edr/catalogs/default/collections/fao_chirps_pentadal_2021_italy/items?filter=S_INTERSECTS%28geom%2CPOLYGON%28%2812.2+39.2%2C12.5+39.2%2C12.5+39.5%2C12.2+39.5%2C12.2+39.2%29%29%29&filter-lang=cql2-text&datetime=2021-01-01T00%3A00%3A00Z%2F2021-01-31T23%3A59%3A59Z&f=CoverageJSON` (`HTTP 200`)
```json
{
  "axes": {
    "t": {
      "bounds": [
        [
          "2021-01-01T00:00:00Z",
          "2021-01-05T23:59:59Z"
        ],
        [
          "2021-01-11T00:00:00Z",
          "2021-01-15T23:59:59Z"
        ],
        [
          "2021-01-26T00:00:00Z",
          "2021-01-31T23:59:59Z"
        ]
      ],
      "values": [
        "2021-01-P1",
        "2021-01-P3",
        "2021-01-P6"
      ]
    },
    "x": {
      "num": 1,
      "start": 12.2,
      "stop": 12.5
    },
    "y": {
      "num": 1,
      "start": 39.2,
      "stop": 39.5
    }
  },
  "domainType": "Grid",
  "referencing": [
    {
      "coordinates": [
        "x",
        "y"
      ],
      "system": {
        "id": "http://www.opengis.net/def/crs/OGC/1.3/CRS84",
        "type": "GeographicCRS"
      }
    },
    {
      "coordinates": [
        "t"
      ],
      "system": {
        "calendar": "Gregorian",
        "type": "TemporalRS"
      }
    }
  ],
  "type": "Domain"
}
```
*(Omitting `&f=CoverageJSON` on the exact same URL returns `Content-Type: application/geo+json` with `"type": "FeatureCollection"`, `"numberMatched": 3`, `"numberReturned": 3`, and full GeoJSON features `2021-01-P1`, `2021-01-P3`, `2021-01-P6` carrying `properties.granule_bbox` and `time.interval`.)*

**Query B — Northeast AOI via classic OGC CQL / ECQL `INTERSECTS` (`f=CoverageJSON` — `application/prs.coverage+json`):**  
`GET /public/edr/catalogs/default/collections/fao_chirps_pentadal_2021_italy/items?filter=INTERSECTS%28geom%2CPOLYGON%28%2812.7+39.7%2C12.9+39.7%2C12.9+39.9%2C12.7+39.9%2C12.7+39.7%29%29%29&filter-lang=cql2-text&datetime=2021-01-01T00%3A00%3A00Z%2F2021-01-31T23%3A59%3A59Z&f=CoverageJSON` (`HTTP 200`)
- **`type`**: `"Domain"` (`domainType: "Grid"`)
- **`axes.t.values`**: `["2021-01-P1", "2021-01-P6"]` (`axes.x`: `12.7..12.9`, `axes.y`: `39.7..39.9`)
- Default GeoJSON format returns `"numberMatched": 2`, `"numberReturned": 2` (`2021-01-P1`, `2021-01-P6`).
- Single-slice lookup works identically per `OGC 19-086r6`: `GET .../items/2021-01-P3` returns `HTTP 200` (`Feature`), whereas absent `GET .../items/2021-01-P2` returns `HTTP 404` (`NotFound`). Non-standard paths (`/availability`, `/intersection`, `/inverse`) are absent (`HTTP 404`).
- This proves **100% standards-compliant CQL2 / ECQL spatial-temporal pruning** on partial SW granule `2021-01-P3` (`[12.0, 39.0, 12.6, 39.6]`) natively on standard EDR `/items` returning CoverageJSON `Domain`.

### 5.3 Side-by-Side Live Cube Comparisons (`CoverageJSON` + GeoPackage Features Baseline)

1. **Sparse Pentadal COG Cube (SW full match — `bbox=12.2,39.2,12.5,39.5`):**  
   `GET /public/edr/catalogs/default/collections/fao_chirps_pentadal_2021_italy/cube?bbox=12.2,39.2,12.5,39.5&datetime=2021-01-01T00:00:00Z/2021-01-31T23:59:59Z` (`HTTP 200`)  
   - `shape`: `[6, 64, 64]`, `axisNames`: `["t", "y", "x"]`  
   - `t.values`: `["2021-01-P1", "2021-01-P2", "2021-01-P3", "2021-01-P4", "2021-01-P5", "2021-01-P6"]`  
   - `t.bounds`: explicitly carries 6 `[start, end]` intervals (showing 6-day `P6 = ["2021-01-26T00:00:00Z", "2021-01-31T23:59:59Z"]`)  
   - `per_slice_non_null`: `[1512, 0, 1512, 0, 0, 1512]` (`non_null_total: 4536`; missing `P2`, `P4`, `P5` are all `null`).

2. **Sparse Pentadal COG Cube (Boundary straddle proving intra-slice spatial masking — `bbox=12.4,39.4,12.8,39.8`):**  
   `GET /public/edr/catalogs/default/collections/fao_chirps_pentadal_2021_italy/cube?bbox=12.4,39.4,12.8,39.8&datetime=2021-01-01T00:00:00Z/2021-01-31T23:59:59Z` (`HTTP 200`)  
   - `shape`: `[6, 64, 64]`  
   - `per_slice_non_null`: `[2624, 0, 1024, 0, 0, 2624]` (`non_null_total: 6272`)  
   - Slice `P3` cleanly retains only the `1024` pixels intersecting its SW granule `[12.0, 39.0, 12.6, 39.6]`, masking non-covered pixels to `null` while full-AOI `P1` and `P6` retain `2624` pixels.

3. **Sparse Dekadal COG Cube (`esa_worldcover_2021_italy` — `bbox=12.2,39.2,12.5,39.5`):**  
   `GET /public/edr/catalogs/default/collections/esa_worldcover_2021_italy/cube?bbox=12.2,39.2,12.5,39.5&datetime=2021-01-01T00:00:00Z/2021-01-31T23:59:59Z` (`HTTP 200`)  
   - `shape`: `[3, 64, 64]`, `t.values`: `["2021-01-D1", "2021-01-D2", "2021-01-D3"]`  
   - `per_slice_non_null`: `[1512, 0, 1512]` (`non_null_total: 3024`; missing `D2` is `null`).

4. **Dense GeoZarr v2 Comparison Baseline (`sample_zarr_t0` — `bbox=12.2,39.2,12.8,39.8`):**  
   `GET /public/edr/catalogs/default/collections/sample_zarr_t0/cube?bbox=12.2,39.2,12.8,39.8&datetime=2021-01-01T00:00:00Z/2021-01-10T23:59:59Z` (`HTTP 200`)  
   - `shape`: `[2, 64, 64]`, `t.values`: `["2021-01-P1", "2021-01-P2"]`, `t.bounds`: `null`  
   - `per_slice_non_null`: `[2254, 2254]` (`non_null_total: 4508`, sample pixel values `[51.0, 53.0, 54.0, 55.0, 57.0]`).

5. **GeoPackage Granule Index Baseline (`dekadal_sparse_granules`):**  
   `GET /public/features/catalogs/default/collections/dekadal_sparse_granules/items?bbox=12.0,39.0,14.5,41.5&datetime=2021-01-01T00:00:00Z/2021-01-31T23:59:59Z` (`HTTP 200`)  
   - `numberReturned`: `2` (`2021-01-D1`, `2021-01-D3`), exposing the exact GeoPackage R-tree rows that `tellurion-edr` now consumes directly inside `/items` and `/cube`.

---

## 6. Ready-to-Post Community & SWG Drafts

### 6.1 Follow-up Comment Draft for `stac-extensions/datacube#36` (and `#31`)

> Thanks again `@fmigneault` and `@m-mohr` for the thorough architectural feedback.
>
> After testing the end-to-end pattern against real Cloud-Optimized GeoTIFF, GeoZarr v2, and GeoPackage deployments in **Tellurion** (and building on our earlier GeoID/FAO operational experience), we are leaving the standalone *OGC API – Dimensions* repository with a **withdrawn formal proposal** notice and dropping custom pagination/inverse endpoints in favor of a 100% standard composition:
>
> 1. **STAC `cube:dimensions` stays minimal (UAD Alignment):**
>    - Keep only `size` (integer cardinality, addressing `#31`), `definition` (URI link-out per OGC API – Common Part 2 UAD, e.g., to `/dimensions/pentadal-2021`, queried via standard `/items?datetime=...` or CQL2 `T_INTERSECTS(...)`), and `variableType` (`categoricalOrdinal`, etc.).
>    - No `provider` object, no custom pagination schema inside STAC Datacube, and no new `nominal`/`ordinal` `type` enums.
>
> 2. **Why `coords`-embedded GeoZarr differs from Multi-File Sparse COG Tile Cubes:**
>    - Single-store **GeoZarr** archives can naturally embed 1D `coords` arrays.
>    - Multi-file **Pentadal (`72/yr`) and Dekadal (`36/yr`) COG tile cubes** (hundreds of thousands of independent GeoTIFF granules per product) cannot embed global coordinate arrays in any single file, and frequently suffer from heavy spatiotemporal sparsity (missing granules, partial cloud-free tiles).
>
> 3. **Subsetting & Spatiotemporal `INTERSECTS` Filtering belong in Standard OGC API – EDR `/items` + CQL2 + `CoverageJSON`:**
>    - Rather than burdening STAC or creating new non-standard endpoints, actual multi-item slice extraction runs on **OGC API – EDR Part 1 v1.1** (`CoverageJSON` with explicit `t.bounds` and `null` gap filling), while sparsity interrogation uses **standard EDR `GET /collections/{cid}/items`** (`OGC 19-086r6` Section 8.9) paired with **OGC API – Features Part 3 / CQL2 (`OGC 21-065r2`)** `filter=S_INTERSECTS(geom, POLYGON((...)))` (or classic ECQL `INTERSECTS(...)`) and `&f=CoverageJSON` to return a compact **CoverageJSON `Domain`** (`OGC 21-069`) whose `axes.t` reports strictly the populated slices $S(\text{AOI}, \Delta t) \subsetneq D$.
>
> Full live walkthrough across Pentadal COG, Dekadal COG, GeoZarr, and GeoPackage is documented here: [`docs/EDR_SPARSE_TILE_CUBES_PROPOSAL.md`](https://github.com/ccancellieri/ogc-dimensions/blob/main/docs/EDR_SPARSE_TILE_CUBES_PROPOSAL.md). Happy to align on a minimal PR for `size` + UAD `definition`/`variableType` if useful!

### 6.2 OGC EDR / CQL2 Best-Practice Profile Note (Zero Non-Standard Endpoints)

- **Composed Standards:** OGC API – EDR Part 1 v1.1 (`OGC 19-086r6`, Section 8.9 `/req/edr/rc-items`) + OGC API – Features Part 3 Filter (`OGC 19-079r2`) + CQL2 (`OGC 21-065r2`) + CoverageJSON (`OGC 21-069`)
- **Pattern:** Do not introduce bespoke paths (`/availability`, `/intersection`, `/inverse`). Instead, advertise `ogcapi-features-3/1.0/conf/filter` and `cql2/1.0/conf/*` on the EDR `/conformance` resource, support `filter=S_INTERSECTS(...)` / `INTERSECTS(...)` / `T_INTERSECTS(...)` + `coords` + `bbox` + `datetime` on standard **`GET /collections/{collectionId}/items`**, and allow `f=CoverageJSON` (`application/prs.coverage+json`) to return a lightweight **CoverageJSON `"type": "Domain"`** (`domainType: "Grid"`) listing only the populated intersecting slices ($S(\text{AOI}, \Delta t) \subsetneq D$).
- **Motivation:** Solves the "Blind Extraction Problem" and unscalable client `/items` pagination on virtual multi-file COG/STAC cubes using non-Gregorian (`pentadal`, `dekadal`) calendars while staying 100% conformant with published OGC API specifications.
