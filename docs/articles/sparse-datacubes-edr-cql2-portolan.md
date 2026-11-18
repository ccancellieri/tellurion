# Sparse Irregular Datacubes on Cloud Storage: Composing STAC UAD, OGC API – EDR CoverageJSON Domains, CQL2 `S_INTERSECTS`, and DuckDB / Portolan SDI

Monolithic multidimensional stores such as GeoZarr or HDF/NetCDF carry their own coordinate arrays inside `.zmetadata` or HDF headers. Operational cloud-native raster catalogs often do not: datasets like FAO ASIS, CHIRPS pentadal/dekadal rainfall, and multi-tile Sentinel/WorldCover archives consist of hundreds of thousands of individual Cloud-Optimized GeoTIFFs (COGs) scattered across object storage over non-Gregorian calendars.

That operational split creates a two-tier metadata problem that neither static STAC collections nor standalone OGC API – EDR Part 1 solves alone:

1. **Irregular non-Gregorian calendars ($D$):** FAO agro-climatic **dekadal** (`36/yr`, with the third dekad spanning day `21..end-of-month`) and **pentadal** (`72/yr`, with the sixth pentad spanning day `26..end-of-month`) axes cannot be generated from fixed ISO 8601 durations (`P10D` / `P5D`), because $365 \bmod 10 \neq 0$ and leap years vary February length.
2. **Spatial-temporal sparsity across multi-file granules ($S(\text{AOI}, \Delta t) \subsetneq D$):** When a GeoTIFF only covers Southwestern Italy during pentad `2021-01-P3`, a user panning to Northeastern Italy must not see `P3` in their timeline filter — and static global `summaries` cannot capture spatial conditioning.

---

## Why Standards-Pure Composition Wins Over Custom Endpoints

Instead of inventing custom endpoints (`/availability`, `/intersection`, `/inverse`) or an isolated paginated dimensions specification, four existing OGC and STAC primitives compose cleanly on the wire:

| Role | Mathematical Set | Standard Mechanism | Wire Output |
| :--- | :--- | :--- | :--- |
| **Nominal calendar grammar** | $D$ (intensional domain) | STAC `cube:dimensions` (`count` via PR #40) + Collection `links` (`rel="dimension"`, #41) to **OGC API – Common Part 2 UAD** `/dimensions/{dimensionId}/items` with `?datetime=` or CQL2 `T_INTERSECTS(...)` | Record Collection / GeoJSON Features with exact nominal `[start, end]` intervals |
| **Populated slice support** | $S(\text{AOI}, \Delta t) \subsetneq D$ | **OGC API – EDR Part 1** (`OGC 19-086r6` §8.9) `GET /collections/{cid}/items` with **CQL2** `filter=S_INTERSECTS(geometry, POLYGON(...))` (or ECQL `INTERSECTS(...)`) and **`f=CoverageJSON`** | `<1 KB` **CoverageJSON `"type": "Domain"`** (`OGC 21-069`) whose `axes.t.values` + `axes.t.bounds` list only populated slices |
| **Pixel extraction** | Data array over $S(\text{AOI}, \Delta t)$ | Standard EDR `/cube`, `/area`, `/position` | CoverageJSON `Coverage` or array payload with zero empty-slice requests |

---

## Convergence with DuckDB File Joins and Portolan SDI

This exact wire pattern scales cleanly to **serverless cloud-bucket Spatial Data Infrastructures** such as **[Portolan SDI](https://www.portolan-sdi.org/)** paired with embedded analytical engines like **DuckDB** (`tellurion-duckdb`):

- **Portolan SDI layout:** Catalogs reside directly on S3-compatible object storage combining **STAC** (discovery), **GeoParquet** and **PMTiles** (vector and item footprint indexes), **COG** and **Zarr** (rasters), and AI-ready `AGENTS.md` / README manifests designed for zero-middleware querying via **DuckDB**.
- **Joined spatiotemporal sparsity:** In agro-climatic workflows, analysts regularly join a GeoParquet manifest of sparse COG granules (`items.parquet`) with secondary cloud Parquet files (FAOSTAT indicators, GAUL administrative boundaries, or station observations):
  $$S_{\text{joined}}(\text{AOI}, \Delta t) = \pi_{t}\!\left(\sigma_{\text{S\_INTERSECTS}(g, \text{AOI}) \land t \in \Delta t}(\text{COG\_Manifest}) \bowtie \sigma_{\text{predicate}}(\text{Attribute\_Parquet})\right)$$
- **Millisecond pushdown to `<1 KB` Domain:** DuckDB pushes spatial bounding-box and temporal predicates directly into Parquet row-group min/max statistics over HTTP range reads — touching zero raster pixels — and Tellurion projects the distinct populated periods straight into a standard **CoverageJSON `Domain`** response on `/edr/.../collections/{cid}/items?filter=S_INTERSECTS(...)&f=CoverageJSON`.

---

## Live Verification in Tellurion

Try the live UAD (`dekadal-2021`, `pentadal-2021`) and CQL2 `S_INTERSECTS` + `CoverageJSON Domain` queries directly in the interactive [STAC & Sparse Datacube Workbench](../../demos/stac/#sparse-cubes-workbench) backed by `https://tellurion-stac-harvest-demo.onrender.com`, and read the canonical change proposal in [`docs/standards/ogc-edr-sparse-tile-cubes.md`](https://github.com/ccancellieri/tellurion/blob/main/docs/standards/ogc-edr-sparse-tile-cubes.md).
