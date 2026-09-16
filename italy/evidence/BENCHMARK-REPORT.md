# Tellurion Rome benchmark — critical comparison

## Result in one minute

The two bounded comparisons passed their correctness gates.

- **Analysis:** Python/Rasterio and QGIS Processing produced byte-identical
  1,200 × 900 masks containing 155,417 road pixels.
- **Serving:** Tellurion and GeoServer returned the same unordered set of 100
  geometries and properties, with `numberMatched=5603` and
  `numberReturned=100`. All six load rows were HTTP 200 with 100% success.
- **Esri:** not measured because no licensed ArcGIS runtime was available.

This is a central-Rome smoke workload, not an Italy-scale capacity result.

## Analysis lane

| Workflow | First run | Repeat median (n=10) | p95 | Max repeat RSS |
| --- | ---: | ---: | ---: | ---: |
| Python 3.12 + Rasterio 1.5 | 0.74 s | 0.59 s | 0.74 s | 111.5 MB |
| QGIS 3.42 Processing (`gdal:rasterize`) | 5.63 s | 4.38 s | 5.34 s | 209.3 MB |

QGIS's repeat median was 7.42× the Python/Rasterio workflow and its maximum
repeat RSS was 1.88×. This is end-to-end CLI workflow cost: QGIS startup, its
Processing provider, and different bundled GDAL versions are included. It does
not show that QGIS in general, or GDAL's rasterizer, is 7× slower. QGIS remains
the stronger choice here for interactive inspection, authoring, and its broad
Processing toolbox; the focused Python step is cheaper to automate.

## Serving lane

Both products ran sequentially on the same host and SSD against the same
5,603-feature Rome GeoPackage. Each row is one 10-second warm localhost window
for an OGC API Features GeoJSON page with `limit=100`.

| Concurrency | Tellurion req/s | Tellurion p95 | GeoServer req/s | GeoServer p95 | RPS ratio |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 513.49 | 4.04 ms | 112.94 | 20.49 ms | 4.55× |
| 8 | 2,168.42 | 7.55 ms | 733.92 | 35.42 ms | 2.95× |
| 32 | 2,714.74 | 34.18 ms | 856.35 | 107.19 ms | 3.17× |

The first successful data request arrived 1.09 s after Tellurion was started
and 9.31 s after GeoServer was started. The request itself took 7.42 ms and
8.16 s respectively; GeoServer initialization overlapped that first request,
so this is not steady-state latency or an OS-cold-cache measurement.

After the first response, observed process RSS was 22.5 MB for Tellurion and
498.3 MB for GeoServer. GeoServer was deliberately configured with
`-Xms512m -Xmx2g`; the observed difference is not a claim about GeoServer's
universal minimum footprint.

## Fairness limits

- The semantic feature set matched, but GeoServer's response was 42,898 bytes
  versus Tellurion's 37,547 bytes (14.3% larger). Throughput therefore includes
  product-specific serialization and metadata cost.
- There was one measurement window per concurrency, so the report has no
  run-to-run variance or confidence interval.
- Run order was fixed (Tellurion, then GeoServer); OS caches were not cleared.
- No TLS, authentication, reverse proxy, network hop, write traffic, or
  multi-tenant contention was included.
- MVT throughput was excluded. Both products returned non-empty vector tiles in
  separate verification, but Tellurion's OGC API Tiles path and GeoServer's
  GeoWebCache TMS path are not the same protocol/cache contract.
- GeoServer's runtime used a disposable copy because GeoTools modified the
  GeoPackage during store initialization. That copy was then locked read-only;
  the source input remained unchanged.
- QGIS Desktop/Processing is not QGIS Server. ArcGIS Pro and ArcGIS
  Enterprise/Image Server likewise should not be collapsed into one comparator.

## Product reading

| Product / workflow | What this test supports | What it does not support |
| --- | --- | --- |
| Tellurion v0.2.0 | Lightweight on-prem vector serving; higher throughput and lower latency in this exact feature-page test | Raster-analysis or universal performance claims |
| GeoServer 3.0.0 | Mature, extensible GIS server that served the same data correctly | A claim that this JVM/data-store configuration is optimal |
| Python + Rasterio | Compact, reproducible raster-mask automation | Interactive desktop authoring |
| QGIS Processing | Correct equivalent analysis with a broad desktop ecosystem | QGIS Server performance, or a kernel-only GDAL comparison |
| ArcGIS / Esri | Broad commercial desktop and enterprise analysis capabilities documented by the vendor | Any local performance number; no licensed runtime was run |

## Evidence and reproduction

The versioned [reproduction bundle][bundle] contains the raw analysis and
serving records, environment capture, input checksums, harnesses and validation
scripts. They are release assets rather than Git-tracked files so the public
repository remains small.

[Open the Rome benchmark comparison graphic][graphic].

[bundle]: https://github.com/ccancellieri/tellurion-italy-demo/releases/download/demo-v0.2.0/tellurion-italy-demo-v0.2.0.zip
[graphic]: https://github.com/ccancellieri/tellurion-italy-demo/releases/download/demo-v0.2.0/benchmark-comparison.png
