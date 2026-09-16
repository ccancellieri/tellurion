# The analysis boundary: Python/Rasterio, not a server claim

**Scheduled: 2026-08-01**

It is easy to turn a simple diagram into an overbroad product claim. This demo has two deliberately separate lanes.

The first is analysis. Python reads [the generated Rome GeoJSON](../output/rome-roads.geojson), uses Rasterio to create a 1,200×900 byte mask with all touched line pixels, and intersects that grid with ESA WorldCover classes. The implementation is visible in [analyze.py](../analyze.py). This is Python/Rasterio analysis, not a Tellurion raster-analysis engine.

The second is serving. The same Rome vectors can be loaded into a GeoPackage and exposed locally via Tellurion’s feature and tile endpoints. The commands, configuration, and example requests are in [the installation and serving section](../README.md#download-and-run-tellurion-on-premise). Serving an API and performing a raster classification are different jobs, and the series keeps them separate.

That distinction also makes comparison fairer. QGIS Processing is relevant to the raster-mask workflow. GeoServer is relevant to the serving lane. Esri has desktop and enterprise products that cover broader workflows, but no licensed Esri runtime was run for this demo. A capability discussion is not a performance result.

The project’s [benchmark design](../benchmark/BENCHMARK-DESIGN.md) makes these boundaries testable: identical masks before analysis timings; identical GeoPackage, request shapes, and sequential runs before server comparison.

Which boundary do you want a reproducible geospatial demonstration to state more clearly: computation, storage, serving, or client visualization?

#SpatialAnalysis #Rasterio #QGIS #GeoServer #OGCAPI
