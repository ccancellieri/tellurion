# Remote COG, local computation, reproducible default

**Scheduled: 2026-07-29**

A Cloud Optimized GeoTIFF can make a large raster usable without treating it as an all-or-nothing download. For this demonstration, the refresh workflow reads a Rome window from the ESA WorldCover COG through HTTP range requests. The default workflow instead uses the included raster window, so a reviewer can reproduce the published result without a live upstream dependency.

That split is intentional. The default is a fixed artifact; `./reproduce.sh --refresh` is a new observation against then-current source access. Neither should be silently substituted for the other. The source URL, scope, and caveats are documented in [the README](../README.md), while the bundled file is [data/rome-worldcover.tif](../data/rome-worldcover.tif).

The raster calculation itself is Python using Rasterio, NumPy, and Shapely—not a Tellurion analysis engine. It rasterises OSM line geometries onto the WorldCover grid and counts the categorical values under the mask. Tellurion enters later, for the local GeoPackage and OGC API Features/Tiles serving path described in the same README.

This is a hybrid pattern rather than a claim of superiority: keep authoritative public raster access at its source when that works, retain local computation and derived artifacts when control matters, and make the boundary visible. QGIS can support equivalent desktop/Processing workflows; other deployments may reasonably mirror data locally.

Do you use remote COGs directly, maintain a local mirror, or choose per dataset? I would like to hear what made that decision durable in practice.

#COG #Rasterio #GDAL #EarthObservation #OnPrem
