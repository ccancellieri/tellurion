# Tellurion demos

Development lives in [`demo/gallery` in Tellurion](https://github.com/ccancellieri/tellurion/tree/main/demo/gallery).
This gallery is exported to the existing `tellurion-demos` repository to retain
published URLs. Edit the source there, not a publication copy.
See [versioned demos and publication](docs/demo-versioning.md) and the
[retained demo versions](releases/).
The [Italy field case](italy/) is now included in the same source tree;
[hosting and download migration](italy/MIGRATION.md) is still in progress.

A human-first, reproducible gallery for Tellurion's vector, raster,
multidimensional, 3D, filtering, catalog and server-rendering paths.

**Visual entry point:** <https://ccancellieri.github.io/tellurion-demos/>

The public gallery and each live backend are checked daily by the
[`Daily verification and deployment gate`](https://github.com/ccancellieri/tellurion-demos/actions/workflows/daily.yml).

This repository intentionally separates the public landing pages from the
machine-readable OGC API endpoints. Start with the visual gallery; use the API
links when you want to inspect the protocol payloads.

Tellurion [v0.5.0-rc.1 release-candidate source](https://github.com/ccancellieri/tellurion)
is public and is the primary self-hosted evaluation route. Start with its
[Quickstart](https://github.com/ccancellieri/tellurion#quickstart) and
[installation manual](https://github.com/ccancellieri/tellurion/blob/main/docs/quickstart/install.md).

To evaluate before installing, open the
[temporary public-file preview](https://tellurion-public-demo.onrender.com/ui/)
with a public COG, GeoParquet file, or zipped Shapefile. It accepts no
credentials or sensitive data and creates no persistent tenant or catalog.
Tellurion is self-hosted software: no Tellurion Cloud, SLA or support service
is offered.

## What the gallery demonstrates

| Lane | Public viewer | Tellurion path | Evidence boundary |
|---|---|---|---|
| Vector | [Interactive road map](https://ccancellieri.github.io/tellurion-demos/demos/vector/) | GeoPackage → OGC API Features and MVT | 5,603-feature OpenStreetMap case sample |
| Raster | [Interactive land-cover map](https://ccancellieri.github.io/tellurion-demos/demos/raster/) | COG → OGC API Tiles/Maps PNG | ESA WorldCover sample, correctly attributed |
| Zarr | [Two-slice comparison](https://ccancellieri.github.io/tellurion-demos/demos/zarr/) | Zarr v2 shape `[time, y, x]` → PNG | Synthetic fixed slices; no on-wire dimension-selection claim |
| 3D | [Interactive GLB scene](https://ccancellieri.github.io/tellurion-demos/demos/3d/) | polygons → MVT → extrusion → GLB/3D Tiles 1.1 | Synthetic footprints; no OGC API 3D GeoVolumes conformance claim |
| CQL2 | [Live query workbench](https://ccancellieri.github.io/tellurion-demos/demos/query/) | CQL2 text → GeoPackage filter → GeoJSON | Bounded to 50 features and this driver's advertised filter classes |
| STAC harvest | [Italy live explorer](https://ccancellieri.github.io/tellurion-demos/demos/stac/) | reviewed 17-source country-to-neighbourhood release → STAC/Features resources and dynamic PNG tiles | dynamic PNG tile composition, not a raster OGC API Maps conformance claim; ESA, Microsoft, and CC BY 4.0 attribution retained |
| Maps + Styles | [Server-rendered Rome map](https://ccancellieri.github.io/tellurion-demos/demos/maps/) | MVT mosaic → PNG, optionally painted by MapLibre Style JSON | Maps 1.0 path; Styles surface is draft-aligned and read-only |

Read the STAC harvest field note,
[From STAC discovery to a live map](docs/articles/from-stac-discovery-to-a-live-map.md),
for the verified source-to-service provenance chain and its stated limitations.

The repository contains a reviewed 17-source Italy release and its
country-to-neighbourhood viewer. Italy resource URLs are live for public
evaluation, with the gallery linking the collection, FeatureCollection, TileSet
and representative PNGs. This is a bounded demonstrator, not an availability,
performance, or support commitment. The earlier one-Item Rome release remains
an independently inspectable field note. ESA WorldCover is CC BY 4.0; use is
attributed as © ESA WorldCover project 2021 / Contains modified Copernicus
Sentinel data (2021) processed by ESA WorldCover consortium. The GISCO boundary
has separate terms: Non-commercial use; © EuroGeographics for the administrative
boundaries. See [From one COG to Italy](docs/articles/from-one-cog-to-italy.md)
for the bounded national composition and verification scope.

Render's free services can sleep after inactivity, so a viewer may need a short
cold start. Hosted response times are not benchmark evidence.

## Public services

The Render Blueprint defines five read-only services:

- `tellurion-vector-demo` — GeoPackage-backed Features and vector tiles.
- `tellurion-raster-demo` — Cloud Optimized GeoTIFF-backed raster tiles from `sample_landcover`.
- `tellurion-zarr-demo` — two deterministic fixed slices from Zarr v2 arrays.
- `tellurion-3d-demo` — synthetic footprint extrusion and 3D Tiles 1.1.
- `tellurion-stac-harvest-demo` — preserved ESA WorldCover metadata, a source COG, STAC/Features metadata, and dynamic raster PNG tiles.

No write route is configured. Images run as UID/GID `10001`, accept Render's
dynamic `PORT`, and verify published SHA-256 manifests before installing or
building Tellurion.

## Reproducibility and licences

The raster, Zarr and 3D demo backends use the historical, checksum-pinned
Tellurion 0.3.0 distribution. The vector and live Italy backends build the
same checksum-pinned Tellurion v0.5.0-rc.1 public source snapshot. That source
snapshot and this demo repository are AGPL-3.0; the historical 0.3.0 archives
retain their own BUSL-1.1 terms.
OpenStreetMap and ESA WorldCover retain their respective data licences and
attribution requirements. See [NOTICE.md](NOTICE.md) before reuse.

The Zarr and 3D datasets are generated locally from short, dependency-free
Python scripts. They are explicitly synthetic and deterministic.

The Zarr viewer demonstrates two configured fixed slices. It does not claim on-the-wire dimension selection; that remains outside this demo's evidence.

Deployment entry points are `Dockerfile` for vector, `Dockerfile.raster` for
COG, `Dockerfile-zarr` for Zarr, `Dockerfile-3d` for 3D Tiles, and
`Dockerfile.stac-harvest` for the ESA harvest. The complete five-service
definition is in `render.yaml`.

Run the deployment contract checks:

```sh
sh tests/render_deployment_contract.sh
sh tests/render_raster_contract.sh
sh tests/render_zarr_contract.sh
sh tests/render_3d_contract.sh
```

## Related work

- [Tellurion public proof brief](proof/) — public evidence, evaluation routes, and current deployment boundaries.
- [Tellurion Italy field case](https://github.com/ccancellieri/tellurion-italy-demo) — reproducible OSM/WorldCover analysis and scoped benchmarks.
- [Carlo Cancellieri's portfolio](https://ccancellieri.github.io/) and [LinkedIn](https://www.linkedin.com/in/ccancellieri/).

Built by Carlo Cancellieri, a geospatial platform engineer and former GeoServer
core developer, with a focus on open standards, interoperable APIs, and
high-performance spatial data systems.
