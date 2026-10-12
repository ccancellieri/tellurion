# WorldCover COG verification

Recorded 12 October 2026, before final release packaging. This bounded case validates one source and one sampling contract; it is not a performance or production-capacity benchmark.

## Source and rights

Public object: <https://esa-worldcover.s3.eu-central-1.amazonaws.com/v200/2021/map/ESA_WorldCover_10m_2021_v200_N39E012_Map.tif> (observed length 41,236,803 bytes).

WorldCover 2021 v200, 36,000 × 36,000 pixels, EPSG:4326, CRS84 extent `[12,39,15,42]`, single uint8 palette band, nodata 0, six overviews, 256 palette entries. Palette entries are not distinct land-cover classes.

© ESA WorldCover project 2021 / Contains modified Copernicus Sentinel data (2021) processed by ESA WorldCover consortium. CC BY 4.0. [Dataset DOI](https://doi.org/10.5281/zenodo.7254221), [official attribution](https://esa-worldcover.org/en/data-access).

## Pixel comparison

Five 256 × 256 RGBA outputs: `(z,x,y)` = `(14,8806,6172)`, `(10,550,385)`, `(8,137,96)`, `(8,136,96)`, `(8,137,97)`. These exercise the main image, two overview levels and west/south source edges. All 327,680 RGBA pixels and alpha values matched. [Recorded hashes and counts](worldcover-pixel-results.json).

The oracle uses GDAL 3.12.1 to decode explicitly selected image directories, then a separately implemented inverse-Web-Mercator pixel-centre/source-floor sampler. It does not reuse the Tellurion renderer or GDAL generic warp sampling. A whole-dataset GDAL warp exercised a different sampling operation; its internal selection was not measured.

### Reproduce

Use a working Tellurion public-source preview with the COG feature, GDAL with `gdal_translate`, and Python with NumPy and Rasterio. Register the object above through the documented [public-source workflow](../quickstart/public-demo.md). Copy its returned PNG tile template; the route orders coordinates `z/y/x`, whereas the filenames below order `z/x/y`. Select `profile=embedded-palette`.

Download the five PNG tiles through that template and decode each to raw RGBA bytes, using names `tellurion-z14-x8806-y6172.rgba`, `tellurion-z10-x550-y385.rgba`, `tellurion-z8-x137-y96.rgba`, `tellurion-z8-x136-y96.rgba`, and `tellurion-z8-x137-y97.rgba`. For example, Pillow's `Image.open(png).convert("RGBA").tobytes()` produces the expected 256 × 256 × 4 layout. Temporary sources expire; register again if necessary.

In a scratch directory, set SOURCE to `/vsicurl/` followed by the public object URL and extract:

```sh
gdal_translate -ovr NONE -srcwin 17894 17834 265 201 -outsize 265 201 -r nearest "$SOURCE" gdal-main-z14-window.tif
gdal_translate -ovr 3 -srcwin 0 0 36000 36000 -outsize 2250 2250 -r nearest "$SOURCE" gdal-overview-2250.tif
gdal_translate -ovr 5 -srcwin 0 0 36000 36000 -outsize 562 562 -r nearest "$SOURCE" gdal-overview-562.tif
```

Run `python3 scripts/verify-worldcover-cog.py <scratch-directory> --output <result.json>` from the repository. It emits independent hashes/counts and exits unsuccessfully on any RGBA or alpha mismatch. Results can change if the upstream object or implementation changes; keep source identity and software versions with a new observation.

## Range observations and limits

[Recorded range observations](worldcover-range-results.json) count driver RangeObject reads after registration. Gateway registration probes, browser requests, end-to-end latency and server/CDN caches are outside this boundary. Metadata inspection: one call, 65,536 returned bytes. The z10 tile: four reads, 349,694 returned bytes; its measured repeat had the same counts. This is no evidence of a warm-cache reduction or fewer total HTTP requests across the whole workflow.

The public API does not expose these internal counters. Reproduce byte-level comparisons with the script above; independently instrument the RangeObject delegate if measuring this same driver boundary. Do not substitute browser network totals for these observations.

[Architecture case study](https://ccancellieri.github.io/writing/stac-cog-reproducible-maps/) explains STAC provenance and explicit temporary-source registration separately from this raster oracle.
