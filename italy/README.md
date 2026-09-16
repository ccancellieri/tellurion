# Tellurion Italy: OSM + ESA WorldCover field test

A public, reproducible companion to a small on-prem geospatial experiment in
central Rome:

- **Vector:** 5,603 OpenStreetMap road and rail ways.
- **Raster:** an HTTP-range window from ESA WorldCover 10 m 2021 v200.
- **Analysis:** WorldCover classes touched by the selected OSM line mask.
- **Serving:** the same Rome vectors exposed locally through Tellurion.
- **Comparison:** scoped measurements against QGIS Processing and GeoServer;
  Esri is discussed but not timed without a licensed runtime.

Read the complete site at:
<https://ccancellieri.github.io/tellurion/italy/>

For the format-spanning capability gallery—vector, COG raster, Zarr and 3D
GLB—use the generic [Tellurion demos hub](https://ccancellieri.github.io/tellurion/).
The gallery's [STAC-to-map field note](https://ccancellieri.github.io/tellurion/demos/stac/)
traces ESA WorldCover discovery through source provenance, GeoPackage and COG
storage, raster tile metadata and rendered maps.

## Public API demonstration

The repository includes a Render Blueprint and Docker image for a public,
read-only Tellurion service over the same 5,603-feature Rome road and rail
sample used by the field test. The image downloads the versioned demo kit and
Tellurion 0.3.0 Linux binary, verifies both published SHA-256 checksums, builds
the GeoPackage during the image build, and runs as an unprivileged user.

Live demo site (recommended): <https://ccancellieri.github.io/tellurion/italy/>

Technical API (JSON): [Rome collection metadata](https://tellurion-vector-rome.onrender.com/public/features/catalogs/default/collections/rome_roads)

Technical API checks:

- [OGC API Features conformance](https://tellurion-vector-rome.onrender.com/public/features/catalogs/default/conformance)
- [Rome features (10-item sample)](https://tellurion-vector-rome.onrender.com/public/features/catalogs/default/collections/rome_roads/items?limit=10)
- [Rome vector tile (MVT)](https://tellurion-vector-rome.onrender.com/public/tiles/catalogs/default/collections/rome_roads/tiles/WebMercatorQuad/13/3044/4380.mvt)

The service exposes Tellurion's landing page and the Rome collection through
OGC API Features and OGC API Tiles. No write storage is configured. Render's
`PORT` environment variable overrides the config's local default, so the same
image works locally and as a Render web service.

Render Free is suitable for this personal, non-commercial demonstration, but
it spins down after inactivity and uses an ephemeral filesystem. Its cold-start
or hosted request timings are therefore not benchmark evidence. Use the frozen
local benchmark report below for measured comparisons.

Deployment files:

- [`Dockerfile`](Dockerfile) — checksum-pinned, non-root runtime image.
- [`render.yaml`](render.yaml) — free Frankfurt web-service Blueprint.
- [`deploy/render/vector.yaml`](deploy/render/vector.yaml) — read-only Rome
  collection configuration.
- [`tests/render_deployment_contract.sh`](tests/render_deployment_contract.sh)
  — deployment invariants.

## Download and reproduce

Download the versioned kit from the
[demo-v0.2.0 release](https://github.com/ccancellieri/tellurion/releases/tag/archive-italy-demo-v0.2.0),
then:

```sh
unzip tellurion-italy-demo-v0.2.0.zip
cd tellurion-italy-demo
chmod +x reproduce.sh
./reproduce.sh
```

The default run uses the included input snapshot and does not require network
access. To refresh both public sources:

```sh
./reproduce.sh --refresh
```

The public cross-platform path reproduces the Python/Rasterio analysis.
Tellurion 0.3.0 binaries are now public for macOS Apple Silicon, Linux x86_64
and Windows x86_64 in the
[public product release](https://github.com/ccancellieri/tellurion/releases/tag/archive-tellurion-v0.3.0).
The serving commands below select the matching macOS or Linux archive
automatically; Windows users can run the installer from Git Bash.

The published benchmark remains a Tellurion 0.2.0 measurement. The 0.3.0
download makes the workflow available for new reproductions; it does not
retroactively rename or replace the recorded benchmark.

The published archive checksum is recorded in the release and on the project
site.

## Install Tellurion 0.3.0

The binaries live in GitHub Releases rather than Git history:

- [macOS Apple Silicon](https://github.com/ccancellieri/tellurion/releases/download/archive-tellurion-v0.3.0/tellurion-v0.3.0-aarch64-apple-darwin.tar.gz)
- [Linux x86_64 (static musl)](https://github.com/ccancellieri/tellurion/releases/download/archive-tellurion-v0.3.0/tellurion-v0.3.0-x86_64-unknown-linux-musl.tar.gz)
- [Windows x86_64 (GNU)](https://github.com/ccancellieri/tellurion/releases/download/archive-tellurion-v0.3.0/tellurion-v0.3.0-x86_64-pc-windows-gnu.zip)
- [corresponding source](https://github.com/ccancellieri/tellurion/releases/download/archive-tellurion-v0.3.0/tellurion-v0.3.0-source-b6eb4a5.zip)
- [SHA-256 manifest](https://github.com/ccancellieri/tellurion/releases/download/archive-tellurion-v0.3.0/SHA256SUMS)

From the extracted demo kit:

```sh
./reproduce.sh --install
./reproduce.sh --load-rome
./reproduce.sh --serve-rome
```

Tellurion 0.3.0 uses the Business Source License 1.1. Evaluation,
development, testing and training are free; personal non-commercial production
use is permitted by the Additional Use Grant. Production use by or for a
business, government or other organisation requires a commercial licence
before 2030-07-23. Read the licence files in each archive.

## What was measured

- Python/Rasterio and QGIS Processing produced byte-identical 1,200×900 masks.
  The complete CLI workflow medians were 0.59 s and 4.38 s.
- Tellurion and GeoServer returned the same 100-feature geometry/property set.
  In one warm 10-second localhost window at concurrency 32, they measured
  2,714.74 and 856.35 successful requests/s, with 34.18 and 107.19 ms p95
  latency.
- These are smoke-scale measurements. They do not establish production,
  distributed or Italy-scale capacity.

Read [RESULTS.md](RESULTS.md) and the [benchmark report](evidence/BENCHMARK-REPORT.md)
before reusing a number.

## Repository layout

```text
articles/          Generated static pages for the ten field notes
content/articles/  Source Markdown for those pages
evidence/          Benchmark design, plan and report
reproduce/         Text-only analysis and reproduction scripts
scripts/           Static article-page generator
index.html         Interactive project landing page
styles.css         Shared responsive design
site.js            Small progressive-enhancement layer
NOTICE.md          Data sources, licences and required attribution
```

Binary assets and data snapshots are intentionally excluded from Git history.
They are distributed as GitHub Release assets.

## Local preview

No build step is required:

```sh
python3 -m http.server 8000
```

Open <http://localhost:8000>. To regenerate the ten static article pages after
editing their Markdown:

```sh
node scripts/build-articles.mjs
```

## Sources and licences

- Repository code and original text: **AGPL-3.0**.
- OpenStreetMap data: **ODbL 1.0**, © OpenStreetMap contributors.
- ESA WorldCover 2021 v200: **CC BY 4.0**.

See [NOTICE.md](NOTICE.md) for the required attribution, dataset citation and
legal links. The software licence does not replace the source-data licences.

## Feedback

Open an issue with your operating system, architecture, command, timing,
`metrics.json` and the first unclear or failing step:
<https://github.com/ccancellieri/tellurion/issues/new>.
