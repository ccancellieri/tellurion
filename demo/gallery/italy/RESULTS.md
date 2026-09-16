# Results at a glance

This is a reproducible on-prem demonstration of Tellurion v0.2.0 using real OpenStreetMap vectors from central Rome and an official ESA WorldCover 2021 Cloud Optimized GeoTIFF.

## Verified result

- 5,603 OSM road and railway ways
- approximately 763.69 km of linework
- 155,417 WorldCover pixels touched by the vector mask
- crossed pixels: 80.10% built-up and 18.68% tree cover
- 5,603 features loaded into a 3 MB GeoPackage
- OGC API Features returned GeoJSON from the packaged v0.2.0 binary
- OGC vector-tile request at `WebMercatorQuad/13/3044/4380.mvt` returned an 85,996-byte MVT

## Matched benchmark results

- Analysis correctness: Python/Rasterio and QGIS produced byte-identical masks
  with 155,417 non-zero pixels.
- Analysis repeat median: Python/Rasterio 0.59 s; QGIS Processing 4.38 s.
- OGC API Features `limit=100`: Tellurion and GeoServer returned the same
  geometry/property set with `numberMatched=5603` and `numberReturned=100`.
- Warm concurrency 32, one 10-second window: Tellurion 2,714.74 successful
  requests/s at 34.18 ms p95; GeoServer 856.35 requests/s at 107.19 ms p95.
- All six service-load rows returned HTTP 200 with 100% success.

Read [the benchmark report](evidence/BENCHMARK-REPORT.md) before interpreting these
figures. The report records payload, startup, process memory, JVM, cache, scale,
and repeat-count limitations. ArcGIS/Esri performance is not measured.

## Run it

```bash
./reproduce.sh
```

The public default reproduces the Python/Rasterio analysis. The following
serving lane installs the public Tellurion 0.3.0 release on macOS Apple
Silicon or Linux x86_64. Windows users can download the GNU archive and run
the installer from Git Bash:

```bash
./reproduce.sh --install
./reproduce.sh --load-rome
./reproduce.sh --serve-rome
```

Then query:

```bash
curl 'http://localhost:8080/public/features/catalogs/default/collections/rome_roads/items?limit=10'
curl 'http://localhost:8080/public/tiles/catalogs/default/collections/rome_roads/tiles/WebMercatorQuad/13/3044/4380.mvt' --output rome.mvt
```

## Interpret carefully

“Road corridor” means raster pixels touched by the selected OSM lines; it is not a policy buffer. Data sources have different vintages, overlapping ways may be double-counted in the approximate line length, and the sample bounding box is not an administrative boundary.

Please reproduce the test, challenge its assumptions and report your environment, command and evidence when sharing feedback.

The performance figures above were measured with Tellurion 0.2.0. Tellurion
0.3.0 is the current downloadable build; new 0.3.0 results should be reported
as a separate run rather than mixed into the historical table.
