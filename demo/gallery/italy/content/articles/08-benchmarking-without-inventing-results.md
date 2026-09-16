# A benchmark is a contract, not a number-shaped claim

**Scheduled: 2026-08-13**

The Rome benchmark has two lanes and one rule: do not manufacture comparability. In the analysis lane, Python/Rasterio and QGIS Processing produced byte-identical masks on the same 1,200×900 WorldCover grid. Repeat medians were 0.59 s and 4.38 s respectively. That is end-to-end CLI workflow cost, including QGIS startup—not a claim that QGIS or GDAL is universally slower.

In the serving lane, Tellurion and GeoServer ran sequentially on the same host against the same 5,603-feature GeoPackage. Their `limit=100` responses contained the same 100 geometries and properties. In the 10-second warm run at concurrency 32, Tellurion measured 2,714.74 successful requests/s with 34.18 ms p95 latency; GeoServer measured 856.35 requests/s with 107.19 ms p95. All requests succeeded.

Those figures need their limits. GeoServer's payload was 14.3% larger, so the test includes product-specific serialization. There was one window per concurrency, no OS-cache clearing, and a fixed run order. GeoServer ran with a 512 MiB minimum JVM heap; its observed process memory is therefore not a universal minimum. MVT throughput was excluded because the bounded comparison used only the equivalent OGC API Features page.

The raw files, c1/c8/c32 results, first-response measurements, and environment are in [the benchmark report](../benchmark/REPORT.md). Esri has no performance result because no licensed runtime was available. Rome is a smoke-scale workload, not an Italy-capacity claim.

What evidence would you require before trusting a public GIS server comparison?

#Benchmarking #PerformanceEngineering #GeoServer #QGIS #Tellurion
