# OGC API – EDR Sparse Tile Cubes, STAC UAD Linkage, and Spatiotemporal Filtering (`CQL2` / `INTERSECTS`)

**Status:** Community & SWG Proposal *(background tracked in [`stac-extensions/datacube#36`](https://github.com/stac-extensions/datacube/issues/36))*  
**Date:** November 2026 &middot; **Author:** Carlo Cancellieri ([ccancellieri.github.io](https://ccancellieri.github.io/))  
**Reference Implementations:** [Tellurion (Rust)](https://github.com/ccancellieri/tellurion) &middot; [GeoID / DynaStore (Python)](https://github.com/un-fao/geoid)  
**Live Demos:** [Tellurion Demos Portal](https://ccancellieri.github.io/tellurion-demos/) &middot; [Sparse Datacube Workbench](https://ccancellieri.github.io/tellurion-demos/demos/stac/) &middot; [Render Live Service](https://tellurion-stac-harvest-demo.onrender.com/public/records/catalogs/default/dimensions/dekadal-2021)

---

## Executive Summary

Operational climate and agricultural platforms ingest **hundreds of thousands of sparse, file-per-granule Cloud-Optimized GeoTIFFs (COGs)** on *non-Gregorian calendars* — notably **pentadal** (`72` periods/year in month-aligned CHIRPS/FAO systems; `73` periods/year in annual GPCP/NOAA systems) and **dekadal** (`36` periods/year in FAO ASIS and FEWS NET).

Unlike monolithic **GeoZarr** stores that embed coordinate arrays (`coords`) inside one archive, multi-file COG cubes exhibit high **spatiotemporal sparsity**: for a given Area of Interest ($\text{AOI}$), many theoretical calendar slices contain *no granule* due to cloud masking, swath gaps, late ingestion, or regional footprints.

| Layer | Standards & Extensions | Core Mechanism |
| :--- | :--- | :--- |
| **1. Nominal Grammar ($D$)**<br>*Calendar structure & dimension labels* | **STAC Datacube** + **OGC API – Common Part 2 UAD** (`OGC 20-024` §8 `extent`) + **Records / Features `/collections/{cid}/items`** (or static asset) | • Minimal `count` on `cube:dimensions` ([`#40`](https://github.com/stac-extensions/datacube/pull/40), merged, resolving [`#31`](https://github.com/stac-extensions/datacube/issues/31))<br>• `OGC 20-024` §8 extends `extent` inside `/collections/{collectionId}` metadata (it does not define a `/dimensions` endpoint); external label lists can be linked via STAC `links` ([`#41`](https://github.com/stac-extensions/datacube/issues/41)) or static Parquet/JSON assets (`scale_type` in [`#42`](https://github.com/stac-extensions/datacube/issues/42))<br>• Prototype label lookup in Tellurion: `GET /records/.../dimensions/{id}/items?datetime=...` or CQL2 `T_INTERSECTS(...)` (e.g., `2021-01-12` $\rightarrow$ `2021-01-P3`) |
| **2. Populated Support ($S(\text{AOI}, \Delta t) \subsetneq D$)**<br>*Sparse slice discovery & data extraction* | **OGC API – Coverages Part 3 (`/scenes`)** + **Features Part 3 / CQL2** (`OGC 19-079r2`, `21-065r2`) + **EDR Part 1 v1.1** (`OGC 19-086r6`) + **CoverageJSON** (`OGC 21-069`) | • **Discovery:** `GET /collections/{cid}/scenes?filter=S_INTERSECTS(...)` *(or `itemType`-disambiguated `/items`)* with `Accept: application/vnd.cov+json; profile="http://www.opengis.net/def/profile/covjson/domain"` (`Domain` with populated `axes.t`)<br>• **Extraction:** EDR `/cube`, `/area`, `/position` (or Coverages `/coverage`) returning full `Coverage` / `CoverageCollection` data (with explicit `t.bounds` and `null`-filled gaps) |

---

## 1. Operational Use Case: Irregular Calendars & Sparse COG Archives

Large-scale agro-climatic monitoring catalogs routinely manage two structural realities:

- **Sub-monthly irregular calendars:**
  - **Dekadal (`36/year`):** Days `1–10` (`D1`), `11–20` (`D2`), and `21–EOM` (`D3`, `8–11` days).
  - **Pentadal, monthly-aligned (`72/year`, CHIRPS / CDT / FAO):** `6` pentads per month (`P1`..`P5` = `5` days each; `P6` = `26–EOM`, ranging from `3` days in non-leap February to `6` days in 31-day months).
  - **Pentadal, annual-aligned (`73/year`, GPCP / NOAA):** `73` consecutive `5`-day slices from January 1 (`P73` = `5` or `6` days).
- **Hundreds of thousands of independent COG granules:**
  - Each `(time_slice, spatial_tile)` is stored as its own COG (e.g., S3 object or regional mosaic).
  - Each granule is indexed by **spatial footprint** (`Polygon`) and **temporal bounds** (`[start_datetime, end_datetime]`) in a relational/GeoPackage or STAC/GeoParquet catalog.

*Note:* The evolution from standalone dimension pagination to this composed OGC/STAC pattern is tracked in [`stac-extensions/datacube#36`](https://github.com/stac-extensions/datacube/issues/36). In the reference implementation ([Tellurion](https://github.com/ccancellieri/tellurion)), calendar grammars (`dekadal-*`, `pentadal-*`) live in `tellurion-records`, while sparse extraction and CQL2-filtered `/items` resolution live in `tellurion-edr`.

---

## 2. Architectural Comparison: GeoZarr vs. Sparse COG Tile Cubes vs. GeoPackage Index

A key reason the GeoDataCube and Zarr communities sometimes talk past earth-observation COG operators is the structural contrast between **single-store multidimensional arrays** and **multi-file sparse COG archives**:

| Architecture Aspect | **GeoZarr v2 (`sample_zarr_t0`)** | **Sparse Multi-File COG Cube (`fao_chirps_pentadal_2021_italy`, `esa_worldcover_2021_italy`)** | **GeoPackage / PostGIS Index (`dekadal_sparse_granules`)** |
|---|---|---|---|
| **Storage Layout** | Single hierarchical store (`.zattrs`, `.zarray`, chunks `[t, y, x]`) | Hundreds of thousands of independent COG files (`1 file ≈ 1 granule`), routed via `routing.tiles` (raster) + `routing.features` (index) | Relational table + R-tree virtual index (`rtree_<table>_geom`) storing `(id, geom, pentad_id/dekad_id, start_datetime, end_datetime, status)` |
| **Coordinate Metadata (`coords`)** | Embedded directly inside store attributes / 1D coordinate arrays (Zarr `coords`) | **Cannot exist inside a single COG file** — no individual TIFF holds the global multi-year pentadal/dekadal axis | Stores **extensional** per-granule footprints (`geom`) and actual ingested timestamps ($S$), not missing slots |
| **Calendar Expressiveness** | Fixed array index `[0..N-1]` paired with numeric time vector or `.zattrs` metadata | Intensional calendar $D$ described via `OGC 20-024` §8 `extent` metadata + STAC `count` (`72` or `36`) and optional label collection/asset (prototype `/dimensions/{id}` in Tellurion); explicit irregular `t.bounds` in CoverageJSON | Rows carry canonical IDs (`2021-01-P3`, `2021-01-D1`) resolved via `?datetime=...` / CQL2 label lookup |
| **Temporal & Spatial Sparsity** | Dense rectangular chunk grid; empty slices waste storage or rely on unpopulated chunks | Missing granules are **absent files** (zero storage cost); partial footprints leave non-intersecting pixels unpopulated | Exact spatial R-tree pushdown + WKT geometry intersection reveals populated vs. missing slices in milliseconds |
| **EDR Subset Output (`CoverageJSON`)** | Direct slice readout over `[time, y, x]` chunks (in our demo: `shape=[2, 64, 64]`, `4508` non-null cells) | Multi-item assembly over all matching slices in $\Delta t$: missing slices become `null`, partial granules are masked per-pixel to `matched_bbox` (`shape=[6, 64, 64]`) | Exposed via standard Features/EDR `/items` (`filter=S_INTERSECTS(...)` / `INTERSECTS(...)`) and consumed internally by EDR (`/cube`/`/area`/`/position`/`/radius`) |

This comparison explains why **`coords`-embedded Zarr solutions do not generalize cleanly to multi-file COG archives**:
- In **GeoZarr**, a client opening the store finds coordinate arrays in one place because the entire cube lives inside one Zarr hierarchy.
- In a **Sparse Pentadal/Dekadal COG Tile Cube**, the "cube" is a *virtual* logical projection over thousands of independent GeoTIFF assets distributed across object storage. No single GeoTIFF contains a global `coords` array, so clients rely on STAC (`count` + external dimension `links`, `#40`/`#41`) + OGC UAD for the calendar grammar ($D$) and need a catalog/EDR sparsity mechanism to determine $S(\text{AOI}, \Delta t)$.

---

### 2.2 Formal Evaluation: Can Populated Members Be Discovered Any Better Way in Sparse Multi-File Cubes?

When a cloud-native raster datacube lacks a single monolithic array header (such as `.zmetadata` in GeoZarr or an HDF/NetCDF coordinate variable) and is instead assembled from hundreds of thousands of Cloud-Optimized GeoTIFFs (COGs) or multi-file object-store partitions indexed in STAC/GeoParquet/GeoPackage, four candidate mechanisms exist on the wire:

| Mechanism | Captures Spatial Conditioning $S(\text{AOI}, \Delta t)$? | Payload Overhead on $10^5$ Granules | Non-Gregorian `bounds` (`pentadal`/`dekadal`) | OGC Standards Status | Verdict |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **1. Static STAC `summaries` / inline `cube:dimensions.values`** | **No.** Computes only the global marginal projection $\pi_t(S_{\text{global}})$. A period with data in Sicily still looks "available" over Lombardy where 0 granules exist. | High in Collection JSON ($10^3$–$10^4$ periods) | Inline strings only; no interval bounds per member | STAC Extension | Sufficient for small Zarr; **breaks for large sparse COGs** |
| **2. Client-side OGC Features `/items?bbox=...` pagination** | Yes, after client aggregation | **$O(K \times M)$**: transfers full tile footprints & GeoJSON properties across $K$ periods $\times M$ spatial tiles per period so the UI can compute `DISTINCT(t)` | Client must reconstruct nominal calendar intervals | Standard (`OGC 17-069`) | **Unscalable** above a few hundred granules |
| **3. STAC API Aggregation Extension (`/aggregate`)** | Yes | Compact JSON buckets | Gregorian histogram bins (`10d`/`5d`); drifts from irregular FAO dekads/pentads | Community STAC API ext (non-OGC) | Useful inside STAC-only ecosystems, but **outside OGC EDR / Coverages** |
| **4. Composed UAD + `/items` (or `/scenes`) + CQL2 `S_INTERSECTS` + CoverageJSON `Domain` `profile`** | **Yes — exact $\pi_t(\sigma_{S\_INTERSECTS(g,\text{AOI}) \land t \in \Delta t}(\text{Index}))$** | **`< 1 KB`**: only distinct populated slices in `Domain.axes.t` | Exact `values` (centroids/start) + `bounds` `[start, end]` pairs per irregular period | **Proposed EDR Extension Profile** (`OGC 19-086r6` §8.9 + `OGC 19-079r2` CQL2 + `OGC 21-069` CoverageJSON + `OGC 20-024` UAD + RFC 6906 `profile`) | **Optimal wire contract** while keeping `f=CoverageJSON` strictly for data |

**Conclusion:** Composing **OGC Common Part 2 UAD** (`OGC 20-024`, for $D$) with **CQL2 `S_INTERSECTS`** (`OGC 19-079r2` declared in `/conformance`) and an explicit **RFC 6906 `profile`** (`Accept: application/vnd.cov+json; profile="http://www.opengis.net/def/profile/covjson/domain"`, for $S(\text{AOI}, \Delta t)$) minimizes wire overhead without breaking format orthogonality (`f=CoverageJSON` continues to return extracted data). Meanwhile, the fastest *execution engine* under that endpoint for object-store catalogs is a columnar **GeoParquet / DuckDB / Portolan SDI** manifest (Section 4.5).

### 2.3 Relationship to Coverages CSTD (`19-087` §11), Published `20-024`, and `/scenes` vs. `/items`

Review on OGC Agora (Jérôme Jacovella-St-Louis, OGC API – Coverages & Common Part 2 Editor) identified three alignment points with active OGC specifications:

| Topic | Spec Reference | Alignment with This Proposal |
| :--- | :--- | :--- |
| **Temporal Gap-Filling vs. Slice Discovery** | **OGC API – Coverages (`19-087`) §11** (`#rc_sparse-dimensions-collapse-temporal`, Req. 31) | **Complementary:** CSTD fills `NODATA` from nearby non-empty slices at extraction time; `/items` (`Domain`) inspects exact populated support $S(\text{AOI}, \Delta t)$ upfront without implicit filling. |
| **Uniform Additional Dimensions (UAD)** | **OGC API – Common Part 2 (`20-024`) §8** ([published spec](https://docs.ogc.org/is/20-024/20-024.html)) | **Decoupled:** `20-024` §8 extends the `extent` object inside `/collections/{collectionId}` metadata (`definition`, `variableType`, `grid.cellsCount`); it does not define a `/dimensions` endpoint. STAC `datacube` keeps `count` (`#40`, merged), `sampling` (`#47`), and open discussions `#41`/`#42`. |
| **Data vs. Metadata Boundary** | **OGC API – Coverages Part 3 (Scenes)** (`/collections/{cid}/scenes`) | **Open SWG Question:** Whether populated-domain discovery (`filter` + `Domain` `profile`) belongs on `/scenes` for raster scene archives and `/items` for feature/in-situ EDR collections. |

- **CSTD Gap-Filling vs. Quantitative Time-Series:** CSTD (`Requirement 31`) returns the latest available pixels on or before a target date — ideal for **mosaic compositing and visual overlays**. In **quantitative time-series analysis** (where silent gap-filling across periods would skew accumulations, averages, or anomaly indices) and in **UI time-pickers**, clients still need the exact populated support $S(\text{AOI}, \Delta t) \subsetneq D$ before extraction.
- **Official Publication of `OGC 20-024`:** With `20-024` now published, `stac-extensions/datacube` PR `#40` was scoped down strictly to `count` (`#31`), leaving external dimension links (`#41`) and `scale_type` (`#42`) separate rather than duplicating `20-024` fields inside STAC.
- **`/items` vs. Coverages Part 3 `/scenes` & `components` Precedent (`OGC 19-087` §23.6.1):** OGC API – Common does not define `/items` (in Common, a collection's *data* are the captured measurements themselves). **Features** (and **EDR**, `19-086r9` §8.2.7) use `/items` for **data** (`itemType: "feature"`), whereas **STAC API** (derived from Features) uses `/items` for **scene metadata** pointing to asset data. **OGC API – Coverages Part 3** introduces `/scenes` so a single collection can serve data via `/coverage` and `/items`, while serving STAC scene metadata at `/scenes` (or STAC `/items` disambiguated via `itemType: "record"` / `"stacItem"` per `OGC 20-024` Rec 3). Meanwhile, `OGC 19-087` §23.6.1 already uses a media-type parameter (`Accept: application/cis+json; components="domainset"`) to request domain-only without rangeset — the exact CIS JSON analogue of our CoverageJSON `Domain` `profile`.

## 3. Why OGC API – EDR Part 1 v1.1 Alone Is Insufficient: The "Blind Extraction Problem"

OGC API – Environmental Data Retrieval (EDR) Part 1 v1.1 (`OGC 19-086r6`) replaced whole-file downloads (NetCDF/GRIB/WCS) with targeted `/position`, `/area`, `/cube`, `/trajectory`, `/corridor`, and `/radius` queries returning **CoverageJSON**. For sparse COG tile cubes, however, EDR Part 1 alone exposes a structural gap — the **Blind Extraction Problem**:

1. **Collection metadata (`GET /edr/collections/{cid}`) only exposes the outer envelope ($D$):**
   - `extent.spatial.bbox` reports the union bounding box (often `[-180, -90, 180, 90]`).
   - `extent.temporal.interval` reports the full time span (e.g., `[["2021-01-01T00:00:00Z", "2021-12-31T23:59:59Z"]]`).
   - Neither field reveals *which* discrete pentads (`P1..P6`) or dekads (`D1..D3`) actually intersect a given $\text{AOI}$.
2. **Workarounds without CQL2 on EDR `/items` require multi-endpoint round trips:**
   - Previously, checking whether `2021-01-P3` had data over an $\text{AOI}$ (or distinguishing an *absent granule* from a *zero-valued pixel*) forced clients to query a separate Features endpoint, parse GeoJSON item properties, map raw timestamps back to pentad/dekad IDs via `/dimensions/{id}/items?datetime=...`, and only then query EDR.
   - **Resolution:** Advertise **`ogcapi-features-3` (`OGC 19-079r2`)** in `/conformance` for **CQL2 (`OGC 21-065r2`)** spatial/temporal filters (`S_INTERSECTS`, `T_INTERSECTS`) on **Coverages Part 3 `/scenes`** (or `itemType`-disambiguated `/items`), paired with an explicit **RFC 6906 `profile`** (`Accept: application/vnd.cov+json; profile="http://www.opengis.net/def/profile/covjson/domain"`) for `<1 KB` slice discovery — while keeping **EDR** (`/cube`, `/area`, `/position`) and `f=CoverageJSON` strictly for data extraction.

---

## 4. Standards-Pure Solution: Coverages `/scenes` (or `/items`) + CQL2 (`S_INTERSECTS`) + `CoverageJSON` `Domain` `profile`

Rather than introducing custom paths or non-standard JSON response fields, the architecture composes existing OGC standards across a single discovery-then-extraction contract — using **`/scenes`** (Coverages Part 3) for partitioned scene archives (or **`/items`** with `itemType: "record"`/`"stacItem"` in STAC-hybrid deployments, and `itemType: "feature"` in Features-only vector servers), followed by **EDR** (`/cube`, `/area`, `/position`) or **Coverages** (`/coverage`) for data extraction:

### 4.1 Conformance Classes & Collection Links (100% Standard OGC)
- **Declared Conformance Classes (`GET /edr/catalogs/default/conformance`):**
  - EDR Part 1 v1.1 (`OGC 19-086r6`): `ogcapi-edr-1/1.1/conf/core`, `collections`, `json`, `edr-geojson`, `queries`, `covjson`
  - OGC API – Features Part 3 Filtering (`OGC 19-079r2`): `http://www.opengis.net/spec/ogcapi-features-3/1.0/conf/filter`
  - CQL2 (`OGC 21-065r2`): `http://www.opengis.net/spec/cql2/1.0/conf/basic-cql2`, `cql2-text`, `cql2-json`, `basic-spatial-functions`, `temporal-functions`
- **Collection Advertisement (`GET /edr/catalogs/default/collections/{cid}`):**
  Standard `data_queries.items` (`href: ".../collections/{cid}/items"`, `rel: "items"`, `type: "application/geo+json"`) alongside `area`, `corridor`, `cube`, `instances`, `locations`, `position`, `radius`, and `trajectory`.

### 4.2 Standard Endpoint & Query Contract
```
GET /collections/{collectionId}/scenes        (Coverages Part 3: partitioned raster/vector scenes)
GET /collections/{collectionId}/items         (STAC itemType: "record"/"stacItem", or Features-only vectors)
```

**Supported Standard Filter & Subsetting Parameters:**
- **`filter` + `filter-lang`** (`cql2-text` default or `cql2-json`):
  - **OGC CQL2 (`OGC 21-065r2`):** `S_INTERSECTS(geom, POLYGON((...)))`, `T_INTERSECTS(time, TIMESTAMP('...'))` / `INTERVAL('...', '...')`, `T_DURING`, `T_AFTER`, `T_BEFORE`.
  - **Classic OGC CQL / GeoServer ECQL compatibility alias:** `INTERSECTS(geom, POLYGON((...)))` parses cleanly into the exact same spatial-intersection AST (`Token::SIntersects`).
- **`bbox`** (`minx,miny,maxx,maxy` in CRS84): Pushed down to the GeoPackage R-tree index (`rtree_<table>_geom`).
- **`coords`** (EDR Part 1 WKT Geometry) or **`intersects`** (STAC GeoJSON / WKT): Also compiled to exact `geo::Intersects` filtering alongside `filter=S_INTERSECTS(...)`.
- **`datetime`** (ISO 8601 instant or `start/end` interval): Filters candidate slices overlapping $\Delta t$.
- **`f` / `Accept` & `profile` (Format-Orthogonal Negotiation):**
  - **Default / `f=GeoJSON` (`application/geo+json`):** Returns a standard GeoJSON `FeatureCollection` containing strictly the matching granule footprints (`numberMatched`, `numberReturned`, `id`, `geometry`, `time.interval`, `properties.granule_bbox`).
  - **`f=CoverageJSON` (`application/vnd.cov+json`, `OGC 21-069r2` §10; legacy alias `application/prs.coverage+json`):** Returns **extracted CoverageJSON data** (`Coverage` / `CoverageCollection` with `parameters` and `ranges`), preserving EDR/Coverages format-agnostic semantics (`f` is normative in EDR `OGC 19-086r6`; Common/Features Part 1 use `Accept:`, and `crs` comes from Features Part 2 `OGC 18-058r1`).
  - **Domain-only `profile` (RFC 6906 / W3C Content Negotiation by Profile):** Sending `Accept: application/vnd.cov+json; profile="http://www.opengis.net/def/profile/covjson/domain"` (or `Accept-Profile`) returns a compact (`<1 KB`) **CoverageJSON `"type": "Domain"`** (`"domainType": "Grid"`, `OGC 21-069`) whose `axes.t.values` and `axes.t.bounds` list **strictly the populated intersecting slices** $S(\text{AOI}, \Delta t)$ without `ranges`, with standard `referencing` (`CRS84` geographic CRS + Gregorian `TemporalRS`). No non-standard top-level wrapper fields are added.

Clients can inspect populated slices using GeoJSON features, or request the `Domain` `profile` on the exact same `/items` URL to skip empty windows (`axes.t.values == []`), identify valid pentad/dekad IDs, and issue `/cube` or `/area` solely on populated periods.

### 4.3 Multi-Item Sparse Assembly Rules

In `crates/tellurion-edr/src/sparse.rs`, sparse raster assembly follows four rules:

1. **Full-window index scan (no `limit=1` truncation):** Queries up to `1000` intersecting index features across the full `datetime` interval rather than stopping after a single STAC Item.
2. **Spatial + geometric `intersects` filtering:** Pushes `bbox` down to the GeoPackage R-tree (`rtree_<table>_geom`) and evaluates exact `geo::Intersects` against CQL2 `S_INTERSECTS`, ECQL `INTERSECTS`, `coords`, and `intersects`.
3. **Intra-slice spatial & polygon masking:** For partial granules covering only part of an $\text{AOI}$ (e.g., `2021-01-P3`), pixels outside `matched_bbox` — and outside non-rectangular `area` polygons or `radius` circles — are set to `null` inside the CoverageJSON `NdArray`.
4. **Irregular interval preservation:** Populates `domain.axes.t.bounds` alongside `domain.axes.t.values` so clients can verify variable-length slices (`P6` = `3–6` days, `D3` = `8–11` days).

### 4.4 Web UI Discovery Workflow: Populated Slices in Massive Sparse Cubes

Consider an archive indexing **hundreds of thousands of sparse COG files** (e.g., `1,440` pentads over 20 years $\times$ `500` spatial tiles = `~720,000` STAC Items / GeoPackage rows). A Web UI needs to render a pentad/dekad time-picker for the current map viewport ($\text{AOI}$) **without** paginating through megabytes of raw `limit=100` GeoJSON feature pages just to compute `DISTINCT(time_slice)`.

The two-layer architecture handles this in three standard steps:

1. **Fetch the nominal calendar $D$ (cacheable):**
   Read `OGC 20-024` §8 `extent` metadata on `/collections/{cid}`, and for full irregular label enumeration follow a Collection `link` (or static Parquet/JSON asset) to a Records/Features `/collections/{cid}/items` endpoint (implemented in Tellurion as prototype `GET /records/.../dimensions/{did}/items?datetime=<window>` or `?filter=T_INTERSECTS(...)`) to retrieve theoretical periods (`2021-01-P1..P6`) and their `[start, end]` intervals.
2. **Push down spatial filtering via `/scenes?filter=S_INTERSECTS(...)` (or `itemType`-disambiguated `/items`) with `Domain` `profile`:**
   On viewport pan/zoom, issue one `<1 KB` standard request:
   `GET /collections/{cid}/scenes?filter=S_INTERSECTS(geom,POLYGON((...)))&filter-lang=cql2-text&datetime=<window>`
   with `Accept: application/vnd.cov+json; profile="http://www.opengis.net/def/profile/covjson/domain"`.
   The server evaluates the spatial R-tree + `geo::Intersects` filter and returns a **CoverageJSON `Domain`** whose `axes.t.values` contains strictly the populated slices $S(\text{AOI}, \Delta t)$ (e.g., `["2021-01-P1", "2021-01-P3", "2021-01-P6"]`). Computing $D \setminus S$ client-side disables empty chips (`P2, P4, P5`) before any `/cube` or `/area` raster extraction runs.
3. **Inspect individual granules only on demand:**
   Use default `application/geo+json` (or call `GET /collections/{cid}/items/{itemId}`) only when drilling into a single active slice (`OGC 19-086r6` §8.9). Where a STAC Search engine is present, OGC API – Features and STAC `/search?intersects=...` query the same spatial/CQL2 index.

---

### 4.5 Cloud-Native Virtual Datacubes via DuckDB File Joins & Portolan SDI

This same two-layer pattern extends to **serverless object-storage Spatial Data Infrastructures (SDI)** such as **[Portolan SDI](https://www.portolan-sdi.org/)** paired with **DuckDB** (`tellurion-duckdb` in Tellurion and `OGC API - Joins` workflows in GeoID/DynaStore):

- **Serverless catalog layout:** Portolan SDI stores spatial assets directly on S3-compatible object storage using open formats — **STAC** for catalog discovery, **GeoParquet** / **PMTiles** for footprint indexes, **COG** / **Zarr** for rasters, and `AGENTS.md` / README manifests built for DuckDB and AI agents.
- **Compounded sparsity across joins:** Joining a sparse COG manifest (`s3://.../portolan/chirps_items.parquet`) with secondary Parquet tables (e.g., FAOSTAT crop-stress records, GAUL boundaries, or basin telemetry) compounds sparsity by relational selectivity:
  $$S_{\text{joined}}(\text{AOI}, \Delta t) = \pi_{t}\!\left(\sigma_{\text{S\_INTERSECTS}(g, \text{AOI}) \land t \in \Delta t}(\text{COG\_Manifest}) \bowtie \sigma_{\text{predicate}}(\text{Attribute\_Parquet})\right)$$
  A slice is usable only when **both** a COG intersects the $\text{AOI}$ **and** a matching tabular record exists.
- **Zero-egress pushdown + standard wire delivery:** DuckDB evaluates `SELECT DISTINCT period_id, start_datetime, end_datetime ...` over `read_parquet('s3://...')` using Parquet row-group min/max statistics and HTTP range reads — computing $S_{\text{joined}}(\text{AOI}, \Delta t)$ in milliseconds without touching COG pixel data. Exposing the result via **`GET /collections/{cid}/scenes?filter=S_INTERSECTS(...)`** *(or `itemType`-disambiguated `/items`)* with `Accept: application/vnd.cov+json; profile="http://www.opengis.net/def/profile/covjson/domain"` delivers a `<1 KB` CoverageJSON `Domain` (`OGC 21-069`) to browsers, openEO clients, and AI agents.

---

### 4.6 Generalization to Sparse Temporal Vector Collections (`Coverages /scenes` vs. Features-Only `/items`)

At the abstract OGC model level (ISO 19123 / CIS, Coverages `19-087`, and EDR `19-086r9`), neither Coverages `/scenes` nor EDR `/cube` is raster-specific: a **partitioned vector archive** (e.g., GeoParquet partitions or >100 MB administrative/hydrological polygons indexed behind simplified STAC footprints linking to vector assets or WFS/Features) fits **`/scenes`** just as cleanly as COG tiles.

In practice, however, **conformance is modular**: many pure-vector servers implement **OGC API – Features** (`17-069r4` + `19-079r2`) without implementing Coverages Part 3 (`/scenes`). On those Features-only deployments, `/items` (`itemType: "feature"`) is the only queryable endpoint and hits the **exact same $O(K \times M)$ pagination bottleneck** when discovering populated slices $S(\text{AOI}, \Delta t)$ — because Features lacks `DISTINCT` / `GROUP BY`.

Keeping **`Accept-Profile` / RFC 6906 `profile`** orthogonal to output format unifies **both** deployment patterns under one CQL2 query (`S_INTERSECTS`, `T_INTERSECTS`, `datetime`):
- **Partitioned asset archives (raster COGs *or* partitioned/heavy vectors):** `/scenes` (Coverages Part 3) or `itemType: "record"` / `"stacItem"` `/items` $\rightarrow$ STAC Items (`application/geo+json`) or `<1 KB` `Domain` `profile`.
- **Features-only vector servers (no Coverages Part 3):** `/items` (`itemType: "feature"`) $\rightarrow$ full vector features (`application/geo+json`, FlatGeobuf) by default, or `<1 KB` `Domain` `profile` for temporal-support discovery (future alignment with OGC API – Features).

## 5. Live Verified Outputs from Tellurion (`tellurion-stac-harvest-demo.onrender.com` & Local Verification)

All outputs below were executed against the compiled pure-Rust `tellurion` binary (`--features cog,geopackage,zarr`) serving the hybrid catalog (`demo.yaml`) combining UAD Records Dimensions, real remote S3 ESA WorldCover COG streaming (`ESA_WorldCover_10m_2021_v200_N39E012_Map.tif`), a GeoPackage R-tree granule index (`sparse_dekadal.gpkg`), and a synthetic Zarr v2 cube (`sample_zarr_t0`).

### 5.1 Prototype Dimension Label Collection & Standard `?datetime=` Filtering (`pentadal-2021` & `dekadal-2021`)

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

### 5.2 Spatiotemporal Pruning via CQL2 (`S_INTERSECTS` / `INTERSECTS`) (`fao_chirps_pentadal_2021_italy`)

In `pentadal_sparse_granules`, January 2021 contains:
- `2021-01-P1`: full AOI `[12.0, 39.0, 14.5, 41.5]`
- `2021-01-P3`: **partial SW-only granule** `[12.0, 39.0, 12.6, 39.6]`
- `2021-01-P6`: irregular 6-day period (`Jan 26–31`) full AOI `[12.0, 39.0, 14.5, 41.5]`
- `P2`, `P4`, `P5`: intentionally absent.

**Query A — Southwest AOI via OGC CQL2 `S_INTERSECTS` (`Accept: application/vnd.cov+json; profile="http://www.opengis.net/def/profile/covjson/domain"`):**  
`GET /public/coverages/catalogs/default/collections/fao_chirps_pentadal_2021_italy/scenes?filter=S_INTERSECTS%28geom%2CPOLYGON%28%2812.2+39.2%2C12.5+39.2%2C12.5+39.5%2C12.2+39.5%2C12.2+39.2%29%29%29&filter-lang=cql2-text&datetime=2021-01-01T00%3A00%3A00Z%2F2021-01-31T23%3A59%3A59Z` (`HTTP 200`)
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
*(Using `Accept: application/geo+json` on the exact same URL returns `"type": "FeatureCollection"`, `"numberMatched": 3`, `"numberReturned": 3`, and full GeoJSON STAC scene features `2021-01-P1`, `2021-01-P3`, `2021-01-P6` carrying `properties.granule_bbox` and `time.interval`.)*

**Query B — Northeast AOI via classic OGC CQL / ECQL `INTERSECTS` (`Accept: application/vnd.cov+json; profile="http://www.opengis.net/def/profile/covjson/domain"`):**  
`GET /public/coverages/catalogs/default/collections/fao_chirps_pentadal_2021_italy/scenes?filter=INTERSECTS%28geom%2CPOLYGON%28%2812.7+39.7%2C12.9+39.7%2C12.9+39.9%2C12.7+39.9%2C12.7+39.7%29%29%29&filter-lang=cql2-text&datetime=2021-01-01T00%3A00%3A00Z%2F2021-01-31T23%3A59%3A59Z` (`HTTP 200`)
- **`type`**: `"Domain"` (`domainType: "Grid"`)
- **`axes.t.values`**: `["2021-01-P1", "2021-01-P6"]` (`axes.x`: `12.7..12.9`, `axes.y`: `39.7..39.9`)
- Default GeoJSON format returns `"numberMatched": 2`, `"numberReturned": 2` (`2021-01-P1`, `2021-01-P6`).
- Single-slice lookup works identically: `GET .../scenes/2021-01-P3` returns `HTTP 200`, whereas absent `GET .../scenes/2021-01-P2` returns `HTTP 404` (`NotFound`). Non-standard paths (`/availability`, `/intersection`, `/inverse`) are absent (`HTTP 404`).
- This demonstrates **standards-compliant CQL2 / ECQL spatial-temporal pruning** on partial SW granule `2021-01-P3` (`[12.0, 39.0, 12.6, 39.6]`) on `/scenes` returning CoverageJSON `Domain` via `profile`.

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

## 6. Standards Profile & Tracked Issues

### 6.1 OGC EDR / CQL2 Best-Practice Profile (Zero Custom Endpoints)

- **Composed Standards:** OGC API – EDR Part 1 v1.1 (`OGC 19-086r6` §8.9 `/req/edr/rc-items`) + OGC API – Features Part 3 Filter (`OGC 19-079r2`) + CQL2 (`OGC 21-065r2`) + CoverageJSON (`OGC 21-069`) + OGC API – Common Part 2 UAD (`OGC 20-024` §8).
- **Wire Pattern:**
  - Advertise `ogcapi-features-3/1.0/conf/filter` and `cql2/1.0/conf/*` on `/conformance`.
  - Support `filter=S_INTERSECTS(...)` / `INTERSECTS(...)` / `T_INTERSECTS(...)`, `coords`, `bbox`, and `datetime` on **`GET /collections/{collectionId}/scenes`** (raster scene archives, or `itemType: "record"` / `"stacItem"` `/items`) and **`GET /collections/{collectionId}/items`** (vector `itemType: "feature"`).
  - Keep `f=CoverageJSON` (`application/vnd.cov+json`) and GeoJSON strictly for full data/items, and negotiate the lightweight **CoverageJSON `"type": "Domain"`** (`domainType: "Grid"`) listing *only* the populated intersecting slices $S(\text{AOI}, \Delta t) \subsetneq D$ via an explicit **RFC 6906 `profile`** (`Accept: application/vnd.cov+json; profile="http://www.opengis.net/def/profile/covjson/domain"` or `Accept-Profile`, analogous to `components="domainset"` in `OGC 19-087` §23.6.1).
- **Outcome:** Resolves the *Blind Extraction Problem* and avoids client-side `/items` pagination across virtual multi-file COG/STAC cubes using irregular (`pentadal`, `dekadal`) calendars — while staying 100% conformant with published OGC API specifications.

### 6.2 Tracked Community Discussions & PRs

- **STAC Datacube Evolution:** [`stac-extensions/datacube#36`](https://github.com/stac-extensions/datacube/issues/36) (architectural background) &middot; [`#31`](https://github.com/stac-extensions/datacube/issues/31) / [`PR #40`](https://github.com/stac-extensions/datacube/pull/40) (**merged** — `count` on Dimension Objects) &middot; [`#41`](https://github.com/stac-extensions/datacube/issues/41) (STAC `links` for external dimension endpoints) &middot; [`#42`](https://github.com/stac-extensions/datacube/issues/42) (`scale_type` in `snake_case`).
- **OGC API – EDR, Coverages & Features:** [`opengeospatial/ogcapi-environmental-data-retrieval#716`](https://github.com/opengeospatial/ogcapi-environmental-data-retrieval/issues/716) (EDR vs. Coverages `/scenes` & sparse support discovery) &middot; [`opengeospatial/ogcapi-coverages#218`](https://github.com/opengeospatial/ogcapi-coverages/issues/218) (Part 3 `/scenes` + CoverageJSON `Domain` `profile` live test) &middot; [`opengeospatial/ogcapi-coverages#241`](https://github.com/opengeospatial/ogcapi-coverages/issues/241) (`Prefer: interpolation=none, handling=strict` + `Content-Interpolation`) &middot; [`opengeospatial/ogcapi-features#1132`](https://github.com/opengeospatial/ogcapi-features/issues/1132) (pure-vector sparse temporal `profile` negotiation aligned with [`#1031`](https://github.com/opengeospatial/ogcapi-features/issues/1031)).
