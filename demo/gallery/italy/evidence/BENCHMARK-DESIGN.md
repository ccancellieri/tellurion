# Tellurion comparison benchmark design

## Decision

Use two independent lanes. The analysis lane compares the existing Python/Rasterio workflow with installed QGIS Processing. The serving lane compares Tellurion with GeoServer using the same Rome GeoPackage. Esri remains an evidence-backed capability and operability comparison because no licensed ArcGIS runtime is available on this host.

## Analysis contract

Both implementations read `output/rome-roads.geojson`, align to the exact 1200×900 grid in `data/rome-worldcover.tif`, burn every touched line pixel with value 1, and produce a Byte GeoTIFF. The masks must have the same geotransform, CRS, dimensions and non-zero pixel count before timings are compared.

Measure process wall time, user/system CPU and peak RSS. Report the first run separately, then the median and p95 of ten repeated runs. These are CLI workflow timings: QGIS process startup is included. The publication screenshot and article-card generation are excluded.

## Serving contract

Tellurion v0.2.0 and GeoServer 3.0.0 serve the identical read-only GeoPackage and run sequentially on the same host. GeoServer runs natively on the same macOS/JVM host, not inside Docker. Compare only:

- GeoJSON feature pages with `limit=10` and `limit=100`;
- one central-Rome MVT, validating that both responses are non-empty vector tiles;
- repeated warm requests at concurrency 1, 8 and 32.

Record endpoint, payload bytes, p50/p95/p99, successful requests per second, error rate, server RSS and exact versions. Restart each server before the first request. Do not call the repeated request a cold-cache measurement; the first response is reported separately.

## Fairness and limitations

- Same files, storage device, client (`oha`), request duration and concurrency.
- No authentication, TLS, reverse proxy or HTTP/2 for either server.
- GeoServer’s stable OGC API Features extension is used. Its final-standard-incompatible community OGC API Tiles module is not used; the stable vector-tile/GeoWebCache path is labelled by its actual protocol.
- Server-side styles and raster services are outside this bounded comparison.
- Rome is a smoke-scale workload. It can expose overhead and saturation behaviour but cannot establish Italy-scale capacity.
- The existing Tellurion 100k-RPS cache figures are not reused as comparative results.
- ArcGIS Pro is a desktop-analysis comparator; ArcGIS Enterprise/Image Server is a server-analysis comparator. Neither receives a performance row without a real licensed run.

## Deliverables

Raw JSON/timing logs, environment manifest, correctness checks, a critical Markdown report, and a compact comparison image suitable for the article series.
